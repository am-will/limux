use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::gdk::prelude::ToplevelExt;
use gtk::glib;
use gtk4 as gtk;
use libadwaita as adw;

use crate::keybind_editor;
use crate::layout_state::{
    self, AppSessionState, LayoutNodeState, LoadedSession, PaneState, SplitOrientation, SplitState,
    WorkspaceState,
};
use crate::pane::{self, PaneCallbacks};
use crate::shortcut_config::{
    self, EditableCapturePolicy, ResolvedShortcutConfig, ShortcutCommand, ShortcutId,
};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct Workspace {
    id: String,
    name: String,
    /// The root widget in the content stack for this workspace.
    root: gtk::Widget,
    /// The sidebar row widget.
    sidebar_row: gtk::ListBoxRow,
    /// Name label in sidebar row.
    name_label: gtk::Label,
    /// Favorite star button in sidebar row.
    favorite_button: gtk::Button,
    /// Notification dot in the sidebar row.
    notify_dot: gtk::Label,
    /// Notification message label in the sidebar row.
    notify_label: gtk::Label,
    /// Whether this workspace has unread notifications.
    unread: bool,
    /// Whether this workspace is favorited/pinned to top.
    favorite: bool,
    /// Last known working directory from the terminal (via OSC 7).
    cwd: Rc<RefCell<Option<String>>>,
    /// The folder path this workspace was opened with.
    folder_path: Option<String>,
    /// Path label shown below workspace name in sidebar.
    #[allow(dead_code)]
    path_label: gtk::Label,
}

struct AppState {
    app: adw::Application,
    window: adw::ApplicationWindow,
    top_bar: Option<adw::HeaderBar>,
    top_bar_visible: bool,
    workspaces: Vec<Workspace>,
    active_idx: usize,
    shortcuts: Rc<ResolvedShortcutConfig>,
    stack: gtk::Stack,
    sidebar_list: gtk::ListBox,
    paned: gtk::Paned,
    new_ws_btn: gtk::Button,
    sidebar_animation: Option<adw::TimedAnimation>,
    sidebar_animation_epoch: u64,
    sidebar_expanded_width: i32,
    persistence_suspended: bool,
    save_queued: bool,
    workspace_dragging: Option<String>,
}

impl AppState {
    fn active_workspace(&self) -> Option<&Workspace> {
        self.workspaces.get(self.active_idx)
    }

    fn workspace_for_widget(&self, widget: &gtk::Widget) -> Option<&Workspace> {
        self.workspaces
            .iter()
            .find(|workspace| widget.is_ancestor(&workspace.root))
    }
}

#[derive(Clone)]
struct WorkspaceSeedSource {
    workspace_cwd: Option<String>,
    workspace_folder_path: Option<String>,
}

#[derive(Clone)]
struct TabDragWorkspaceSeed {
    name: String,
    cwd: Option<String>,
    folder_path: Option<String>,
}

type State = Rc<RefCell<AppState>>;
const SPLIT_RATIO_STATE_KEY: &str = "limux-split-ratio-state";

fn request_session_save(state: &State) {
    let should_schedule = {
        let mut s = state.borrow_mut();
        if s.persistence_suspended || s.save_queued {
            false
        } else {
            s.save_queued = true;
            true
        }
    };

    if !should_schedule {
        return;
    }

    let state = state.clone();
    glib::idle_add_local_once(move || {
        let should_save = {
            let mut s = state.borrow_mut();
            let should_save = s.save_queued && !s.persistence_suspended;
            s.save_queued = false;
            should_save
        };
        if should_save {
            save_session_now(&state);
        }
    });
}

fn save_session_now(state: &State) {
    let session = snapshot_session_state(state);
    if let Err(err) = layout_state::save_session_atomic(&session) {
        eprintln!("limux: failed to save session state: {err}");
    }
}

fn suspend_persistence(state: &State, suspended: bool) {
    state.borrow_mut().persistence_suspended = suspended;
}

fn apply_loaded_session(state: &State, loaded: LoadedSession) {
    suspend_persistence(state, true);

    apply_top_bar_state_immediately(state, loaded.state.top_bar_visible);

    let restored_any = !loaded.state.workspaces.is_empty();
    if restored_any {
        for workspace in &loaded.state.workspaces {
            add_workspace_from_state(state, workspace);
        }
        restore_active_workspace(state, loaded.state.active_workspace_index);
        apply_sidebar_state_immediately(state, &loaded.state.sidebar);
    }

    suspend_persistence(state, false);

    if restored_any || matches!(loaded.source, layout_state::SessionLoadSource::Legacy) {
        save_session_now(state);
    }
}

fn restore_active_workspace(state: &State, index: usize) {
    let maybe_row = {
        let s = state.borrow();
        if s.workspaces.is_empty() {
            None
        } else {
            let clamped = index.min(s.workspaces.len() - 1);
            Some((
                clamped,
                s.workspaces[clamped].sidebar_row.clone(),
                s.sidebar_list.clone(),
            ))
        }
    };

    if let Some((index, row, sidebar_list)) = maybe_row {
        switch_workspace(state, index);
        sidebar_list.select_row(Some(&row));
    }
}

fn apply_sidebar_state_immediately(state: &State, sidebar_state: &layout_state::SidebarState) {
    let (paned, sidebar, width) = {
        let mut s = state.borrow_mut();
        s.sidebar_expanded_width = sidebar_state.width.max(SIDEBAR_WIDTH);
        let sidebar = match s.paned.start_child() {
            Some(sidebar) => sidebar,
            None => return,
        };
        (s.paned.clone(), sidebar, s.sidebar_expanded_width)
    };

    if sidebar_state.visible {
        sidebar.set_visible(true);
        paned.set_position(width);
    } else {
        // Apply restored sidebar visibility directly; using the animated toggle path during
        // startup would create flicker and extra persistence churn while restore is suspended.
        sidebar.set_visible(false);
        paned.set_position(0);
    }
}

fn apply_top_bar_state_immediately(state: &State, visible: bool) {
    state.borrow_mut().top_bar_visible = visible;
    sync_top_bar_visibility(state);
}

fn snapshot_session_state(state: &State) -> AppSessionState {
    let s = state.borrow();
    let sidebar_visible = sidebar_is_visible(&s);
    let sidebar_width = if sidebar_visible {
        s.paned.position()
    } else {
        s.sidebar_expanded_width
    }
    .max(SIDEBAR_WIDTH);

    let workspaces = s
        .workspaces
        .iter()
        .map(|workspace| {
            let cwd = workspace.cwd.borrow().clone();
            let folder_path = workspace.folder_path.clone();
            let working_directory = folder_path.clone().or(cwd.clone());
            WorkspaceState {
                name: workspace.name.clone(),
                favorite: workspace.favorite,
                cwd,
                folder_path,
                layout: snapshot_layout_node(&workspace.root, working_directory.as_deref()),
            }
        })
        .collect();

    layout_state::normalize_session(AppSessionState {
        version: layout_state::SESSION_VERSION,
        active_workspace_index: s.active_idx,
        top_bar_visible: s.top_bar_visible,
        sidebar: layout_state::SidebarState {
            visible: sidebar_visible,
            width: sidebar_width,
        },
        workspaces,
    })
}

fn sidebar_is_visible(state: &AppState) -> bool {
    state
        .paned
        .start_child()
        .map(|sidebar| sidebar.is_visible() && state.paned.position() > 10)
        .unwrap_or(false)
}

fn begin_window_move_from_widget(
    widget: &impl IsA<gtk::Widget>,
    window: &adw::ApplicationWindow,
    device: &gtk::gdk::Device,
    button: i32,
    x: f64,
    y: f64,
    timestamp: u32,
) {
    let Some((surface_x, surface_y)) = widget.translate_coordinates(window, x, y) else {
        return;
    };
    let Some(surface) = window.surface() else {
        return;
    };
    let Ok(toplevel) = surface.dynamic_cast::<gtk::gdk::Toplevel>() else {
        return;
    };
    toplevel.begin_move(device, button, surface_x, surface_y, timestamp);
}

fn split_ratio_state(paned: &gtk::Paned) -> Option<Rc<RefCell<f64>>> {
    unsafe {
        paned
            .data::<Rc<RefCell<f64>>>(SPLIT_RATIO_STATE_KEY)
            .map(|ptr| ptr.as_ref().clone())
    }
}

fn update_split_ratio_state(paned: &gtk::Paned, ratio: f64) {
    let ratio = layout_state::clamp_split_ratio(ratio);
    if let Some(stored_ratio) = split_ratio_state(paned) {
        *stored_ratio.borrow_mut() = ratio;
    } else {
        unsafe {
            paned.set_data(SPLIT_RATIO_STATE_KEY, Rc::new(RefCell::new(ratio)));
        }
    }
}

fn snapshot_layout_node(widget: &gtk::Widget, working_directory: Option<&str>) -> LayoutNodeState {
    if let Some(paned) = widget.downcast_ref::<gtk::Paned>() {
        let size = if paned.orientation() == gtk::Orientation::Horizontal {
            paned.allocation().width()
        } else {
            paned.allocation().height()
        };
        let ratio = layout_state::snapshot_split_ratio(
            paned.position(),
            size,
            split_ratio_state(paned).map(|ratio| *ratio.borrow()),
        );
        update_split_ratio_state(paned, ratio);
        let start = paned
            .start_child()
            .map(|child| snapshot_layout_node(&child, working_directory))
            .unwrap_or_else(|| LayoutNodeState::Pane(PaneState::fallback(working_directory)));
        let end = paned
            .end_child()
            .map(|child| snapshot_layout_node(&child, working_directory))
            .unwrap_or_else(|| LayoutNodeState::Pane(PaneState::fallback(working_directory)));
        return LayoutNodeState::Split(SplitState {
            orientation: if paned.orientation() == gtk::Orientation::Horizontal {
                SplitOrientation::Horizontal
            } else {
                SplitOrientation::Vertical
            },
            ratio,
            start: Box::new(start),
            end: Box::new(end),
        });
    }

    pane::snapshot_pane_state(widget)
        .map(LayoutNodeState::Pane)
        .unwrap_or_else(|| LayoutNodeState::Pane(PaneState::fallback(working_directory)))
}

fn build_workspace_root(
    state: &State,
    shortcuts: &Rc<ResolvedShortcutConfig>,
    ws_id: &str,
    working_directory: Option<&str>,
    layout: Option<&LayoutNodeState>,
) -> gtk::Widget {
    match layout {
        Some(layout) => build_layout_widget(state, shortcuts, ws_id, working_directory, layout),
        None => create_pane_for_workspace(state, shortcuts, ws_id, working_directory, None, false)
            .upcast(),
    }
}

fn build_layout_widget(
    state: &State,
    shortcuts: &Rc<ResolvedShortcutConfig>,
    ws_id: &str,
    working_directory: Option<&str>,
    layout: &LayoutNodeState,
) -> gtk::Widget {
    match layout {
        LayoutNodeState::Pane(pane_state) => create_pane_for_workspace(
            state,
            shortcuts,
            ws_id,
            working_directory,
            Some(pane_state),
            false,
        )
        .upcast(),
        LayoutNodeState::Split(split_state) => {
            let orientation = match split_state.orientation {
                SplitOrientation::Horizontal => gtk::Orientation::Horizontal,
                SplitOrientation::Vertical => gtk::Orientation::Vertical,
            };
            let paned = gtk::Paned::builder()
                .orientation(orientation)
                .hexpand(true)
                .vexpand(true)
                .build();
            update_split_ratio_state(&paned, split_state.ratio);
            attach_split_position_persistence(state, &paned);
            let start = build_layout_widget(
                state,
                shortcuts,
                ws_id,
                working_directory,
                &split_state.start,
            );
            let end =
                build_layout_widget(state, shortcuts, ws_id, working_directory, &split_state.end);
            paned.set_start_child(Some(&start));
            paned.set_end_child(Some(&end));
            apply_split_ratio_after_layout(&paned, orientation, split_state.ratio);
            paned.upcast()
        }
    }
}

fn apply_split_ratio_after_layout(paned: &gtk::Paned, orientation: gtk::Orientation, ratio: f64) {
    let ratio = layout_state::clamp_split_ratio(ratio);
    let apply_ratio = move |paned: &gtk::Paned| {
        let allocation = paned.allocation();
        let size = if orientation == gtk::Orientation::Horizontal {
            allocation.width()
        } else {
            allocation.height()
        };
        if size <= 0 {
            return false;
        }
        paned.set_position(layout_state::split_position_from_ratio(ratio, size));
        update_split_ratio_state(paned, ratio);
        true
    };

    let paned_for_idle = paned.clone();
    glib::idle_add_local_once(move || {
        let _ = apply_ratio(&paned_for_idle);
    });

    let paned_for_map = paned.clone();
    // Hidden workspaces may not have a real allocation during initial restore, so retry when the
    // split is actually mapped instead of collapsing the divider to an arbitrary fallback pixel.
    paned.connect_map(move |_| {
        let _ = apply_ratio(&paned_for_map);
    });
}

fn attach_split_position_persistence(state: &State, paned: &gtk::Paned) {
    update_split_ratio_state(paned, layout_state::DEFAULT_SPLIT_RATIO);
    let state = state.clone();
    paned.connect_position_notify(move |paned| {
        let allocation = paned.allocation();
        let size = if paned.orientation() == gtk::Orientation::Horizontal {
            allocation.width()
        } else {
            allocation.height()
        };
        let ratio = layout_state::snapshot_split_ratio(
            paned.position(),
            size,
            split_ratio_state(paned).map(|ratio| *ratio.borrow()),
        );
        update_split_ratio_state(paned, ratio);
        request_session_save(&state);
    });
}

// ---------------------------------------------------------------------------
// CSS
// ---------------------------------------------------------------------------

const CSS: &str = r#"
.limux-sidebar {
    background-color: rgba(25, 25, 25, 1);
}
.limux-sidebar-row-box {
    padding: 8px 6px 8px 3px;
    border-radius: 6px;
    margin: 2px 3px 2px 1px;
}
.limux-ws-name {
    color: rgba(255, 255, 255, 0.7);
    font-size: 15px;
}
row:selected .limux-ws-name {
    color: white;
}
.limux-ws-star-btn {
    color: rgba(255, 255, 255, 0.45);
    border: none;
    min-height: 0;
    min-width: 0;
    padding: 0 4px;
    font-size: 22px;
}
.limux-ws-star-btn:hover {
    color: rgba(255, 255, 255, 0.9);
}
row:selected .limux-ws-star-btn {
    color: rgba(255, 255, 255, 0.85);
}
.limux-ws-star-btn-active {
    color: #f7c948;
}
.limux-ws-rename-entry {
    min-height: 0;
    padding: 0 4px;
    margin: 0;
}
.limux-notify-dot {
    color: #0091FF;
    font-size: 10px;
    margin-right: 6px;
}
.limux-notify-dot-hidden {
    color: transparent;
    font-size: 10px;
    margin-right: 6px;
}
.limux-notify-msg {
    color: rgba(255, 255, 255, 0.35);
    font-size: 11px;
}
.limux-notify-msg-unread {
    color: rgba(0, 145, 255, 0.8);
    font-size: 11px;
}
.limux-sidebar-row-unread {
    background-color: rgba(0, 145, 255, 0.18);
    border-left: 3px solid #0091FF;
    border-radius: 6px;
    margin-left: 0;
    margin-right: 0;
}
.limux-sidebar-row-unread .limux-ws-name {
    color: white;
    font-weight: 700;
}
.limux-drop-above .limux-sidebar-row-box {
    border-radius: 0;
    box-shadow: 0 -2px 0 0 #0091FF;
}
.limux-drop-below .limux-sidebar-row-box {
    border-radius: 0;
    box-shadow: 0 2px 0 0 #0091FF;
}
.limux-tab-drop-target {
    background-color: rgba(0, 145, 255, 0.18);
    border-radius: 8px;
}
.limux-sidebar row:drop(active) {
    box-shadow: none;
}
.limux-sidebar-title {
    color: rgba(255, 255, 255, 0.5);
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 1px;
}
.limux-sidebar-btn {
    background: rgba(255, 255, 255, 0.08);
    color: rgba(255, 255, 255, 0.7);
    border: 1px solid transparent;
    border-radius: 6px;
    padding: 6px 12px;
    min-height: 0;
    transition: all 200ms ease;
}
.limux-sidebar-btn:hover {
    background: rgba(255, 255, 255, 0.14);
    color: white;
}
.limux-sidebar-btn-trash {
    background: rgba(255, 60, 60, 0.25);
    color: rgba(255, 80, 80, 1);
    border: 1px solid rgba(255, 80, 80, 0.5);
}
.limux-sidebar-btn-trash-hover {
    background: rgba(255, 60, 60, 0.45);
    color: rgba(255, 90, 90, 1);
    border: 1px solid rgba(255, 80, 80, 0.8);
}
.limux-tab-drag-active {
    background-color: rgba(0, 145, 255, 0.12);
    border-width: 1px;
    border-style: dashed;
    border-color: rgba(0, 145, 255, 0.6);
    border-radius: 8px;
}
.limux-sidebar-btn.limux-tab-drop-target {
    background-color: rgba(0, 145, 255, 0.28);
    border-color: rgba(0, 145, 255, 0.9);
}
.limux-ws-path {
    color: rgba(255, 255, 255, 0.3);
    font-size: 12px;
}
row:selected .limux-ws-path {
    color: rgba(255, 255, 255, 0.5);
}
.limux-content {
    background-color: rgba(23, 23, 23, 1);
}
"#;

// ---------------------------------------------------------------------------
// Window construction
// ---------------------------------------------------------------------------

pub fn build_window(app: &adw::Application) {
    let display = gtk::gdk::Display::default().expect("display");
    let shortcuts = Rc::new(shortcut_config::load_shortcuts_for_display(&display));
    for warning in &shortcuts.warnings {
        eprintln!("limux: {warning}");
    }

    // Load CSS
    let provider = gtk::CssProvider::new();
    let all_css = format!(
        "{CSS}\n{}\n{}",
        pane::PANE_CSS,
        keybind_editor::KEYBIND_EDITOR_CSS
    );
    provider.load_from_data(&all_css);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let style_manager = adw::StyleManager::default();
    crate::terminal::sync_color_scheme(style_manager.is_dark());
    style_manager.connect_dark_notify(|style_manager| {
        crate::terminal::sync_color_scheme(style_manager.is_dark());
    });

    // Register custom icons — look for icons dir relative to the executable
    let icon_theme = gtk::IconTheme::for_display(&display);
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    // Try several possible icon locations
    for path in [
        exe_dir
            .as_ref()
            .map(|d| d.join("../../rust/limux-host-linux/icons")),
        exe_dir.as_ref().map(|d| d.join("../icons")),
        Some(std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/icons"
        ))),
    ]
    .iter()
    .flatten()
    {
        if path.exists() {
            icon_theme.add_search_path(path);
        }
    }

    let title = format!("Limux v{}", crate::VERSION);
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(&title)
        .default_width(1400)
        .default_height(900)
        .build();

    // On Wayland compositors with xdg-decoration support, the compositor
    // already provides the window chrome, so keep Limux from rendering a
    // duplicate header bar. X11 continues to use the in-app header.
    let provides_decorations = display
        .clone()
        .downcast::<gdk4_wayland::WaylandDisplay>()
        .ok()
        .map(|display| display.query_registry("zxdg_decoration_manager_v1"))
        .unwrap_or(false);

    let header = if provides_decorations {
        None
    } else {
        let bar = adw::HeaderBar::new();
        bar.set_title_widget(Some(&gtk::Label::builder().label(&title).build()));
        Some(bar)
    };

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::None);
    stack.set_hexpand(true);
    stack.set_vexpand(true);
    stack.add_css_class("limux-content");

    let sidebar_list = gtk::ListBox::new();
    sidebar_list.set_selection_mode(gtk::SelectionMode::Single);
    sidebar_list.add_css_class("navigation-sidebar");

    let sidebar_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .child(&sidebar_list)
        .build();

    let sidebar_title_label = gtk::Label::builder()
        .label("WORKSPACES")
        .xalign(0.0)
        .hexpand(true)
        .margin_start(12)
        .build();
    sidebar_title_label.add_css_class("limux-sidebar-title");

    let sidebar_title = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .margin_top(8)
        .margin_bottom(4)
        .margin_end(6)
        .build();
    sidebar_title.append(&sidebar_title_label);

    {
        let window = window.clone();
        let drag_title = sidebar_title.clone();
        let drag = gtk::GestureClick::new();
        drag.set_button(1);
        drag.connect_pressed(move |gesture, _, x, y| {
            let Some(device) = gesture.current_event_device() else {
                return;
            };
            let button = gesture.current_button() as i32;
            let timestamp = gesture.current_event_time();
            begin_window_move_from_widget(&drag_title, &window, &device, button, x, y, timestamp);
            gesture.set_state(gtk::EventSequenceState::Claimed);
        });
        sidebar_title.add_controller(drag);
    }

    let new_ws_btn = gtk::Button::builder()
        .label("New Workspace")
        .hexpand(true)
        .margin_start(6)
        .margin_end(6)
        .margin_bottom(6)
        .build();
    new_ws_btn.add_css_class("limux-sidebar-btn");

    // Drop target on the button: workspace drags delete, tab drags create a new workspace.
    let btn_drop = gtk::DropTarget::new(glib::Type::STRING, gtk::gdk::DragAction::MOVE);
    btn_drop.set_preload(true);
    {
        let btn = new_ws_btn.clone();
        btn_drop.connect_motion(move |_, _, _| {
            if pane::is_tab_dragging() {
                btn.add_css_class("limux-tab-drop-target");
            } else {
                btn.add_css_class("limux-sidebar-btn-trash-hover");
            }
            gtk::gdk::DragAction::MOVE
        });
    }
    {
        let btn = new_ws_btn.clone();
        btn_drop.connect_leave(move |_| {
            btn.remove_css_class("limux-sidebar-btn-trash-hover");
            btn.remove_css_class("limux-tab-drop-target");
        });
    }
    new_ws_btn.add_controller(btn_drop.clone());

    let sidebar = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .width_request(220)
        .build();
    sidebar.add_css_class("limux-sidebar");
    sidebar.append(&sidebar_title);
    sidebar.append(&sidebar_scroll);
    sidebar.append(&new_ws_btn);

    let main_paned = gtk::Paned::builder()
        .orientation(gtk::Orientation::Horizontal)
        .position(220)
        .shrink_start_child(false)
        .shrink_end_child(false)
        .start_child(&sidebar)
        .end_child(&stack)
        .build();

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    if let Some(ref header) = header {
        vbox.append(header);
    }
    vbox.append(&main_paned);
    window.set_content(Some(&vbox));

    let state: State = Rc::new(RefCell::new(AppState {
        app: app.clone(),
        window: window.clone(),
        top_bar: header.clone(),
        top_bar_visible: true,
        workspaces: Vec::new(),
        active_idx: 0,
        shortcuts,
        stack: stack.clone(),
        sidebar_list: sidebar_list.clone(),
        paned: main_paned.clone(),
        new_ws_btn: new_ws_btn.clone(),
        sidebar_animation: None,
        sidebar_animation_epoch: 0,
        sidebar_expanded_width: SIDEBAR_WIDTH,
        persistence_suspended: false,
        save_queued: false,
        workspace_dragging: None,
    }));

    apply_shortcuts_to_application(app, &state.borrow().shortcuts);

    {
        let state = state.clone();
        window.connect_fullscreened_notify(move |_| {
            sync_top_bar_visibility(&state);
        });
    }

    {
        let state = state.clone();
        main_paned.connect_position_notify(move |paned| {
            let position = paned.position();
            let should_save = if position > 10 {
                let mut s = state.borrow_mut();
                let changed = s.sidebar_expanded_width != position;
                s.sidebar_expanded_width = position;
                changed
            } else {
                false
            };
            if should_save {
                request_session_save(&state);
            }
        });
    }

    register_app_actions(app, &state);
    register_window_actions(&window, &state);
    install_key_capture(&window, &state);

    // Any click anywhere in the window commits an active sidebar rename,
    // UNLESS the click is inside the rename Entry itself.
    {
        let sl = sidebar_list.clone();
        let win = window.clone();
        let click_anywhere = gtk::GestureClick::new();
        click_anywhere.set_propagation_phase(gtk::PropagationPhase::Capture);
        click_anywhere.connect_pressed(move |_, _, x, y| {
            if let Some(entry) = find_active_rename_entry(&sl) {
                // Translate click coords from window to the entry's coordinate space
                if let Some((ex, ey)) = win.translate_coordinates(&entry, x, y) {
                    let alloc = entry.allocation();
                    if ex >= 0.0
                        && ey >= 0.0
                        && ex <= alloc.width() as f64
                        && ey <= alloc.height() as f64
                    {
                        return; // click is inside the entry
                    }
                }
                commit_any_active_rename(&sl);
            }
        });
        window.add_controller(click_anywhere);
    }

    {
        let state = state.clone();
        sidebar_list.connect_row_selected(move |_, row| {
            if let Some(row) = row {
                let idx = row.index() as usize;
                switch_workspace(&state, idx);
            }
        });
    }

    {
        let state = state.clone();
        new_ws_btn.connect_clicked(move |_| {
            add_workspace(&state, None);
        });
    }

    {
        let btn = new_ws_btn.clone();
        pane::on_tab_drag_change(move |dragging| {
            if dragging {
                btn.add_css_class("limux-tab-drag-active");
            } else {
                btn.remove_css_class("limux-tab-drag-active");
                btn.remove_css_class("limux-tab-drop-target");
            }
        });
    }

    {
        let state = state.clone();
        let btn = new_ws_btn.clone();
        btn_drop.connect_drop(move |_, value, _, _| {
            btn.set_label("New Workspace");
            btn.remove_css_class("limux-sidebar-btn-trash");
            btn.remove_css_class("limux-sidebar-btn-trash-hover");
            btn.remove_css_class("limux-tab-drop-target");
            if let Ok(payload) = value.get::<String>() {
                if payload.contains(':') {
                    return create_workspace_for_tab(&state, &payload);
                }
                close_workspace_by_id(&state, &payload);
                return true;
            }
            false
        });
    }

    // Save the full session on window close.
    {
        let state = state.clone();
        window.connect_close_request(move |_| {
            save_session_now(&state);
            glib::Propagation::Proceed
        });
    }

    apply_loaded_session(&state, layout_state::load_session());
    window.present();
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

fn register_window_actions(window: &adw::ApplicationWindow, state: &State) {
    let action_defs: Vec<(&'static str, ShortcutCommand)> = {
        let s = state.borrow();
        s.shortcuts
            .shortcuts
            .iter()
            .filter(|shortcut| shortcut.definition.action_name.starts_with("win."))
            .map(|shortcut| {
                (
                    shortcut.definition.action_basename(),
                    shortcut.definition.command,
                )
            })
            .collect()
    };

    for (name, command) in action_defs {
        let action = gtk::gio::SimpleAction::new(name, None);
        let state = state.clone();
        action.connect_activate(move |_, _| {
            dispatch_shortcut_command(&state, command);
        });
        window.add_action(&action);
    }
}

fn register_app_actions(app: &adw::Application, state: &State) {
    let action_defs: Vec<(&'static str, ShortcutCommand)> = {
        let s = state.borrow();
        s.shortcuts
            .shortcuts
            .iter()
            .filter(|shortcut| shortcut.definition.action_name.starts_with("app."))
            .map(|shortcut| {
                (
                    shortcut.definition.action_basename(),
                    shortcut.definition.command,
                )
            })
            .collect()
    };

    for (name, command) in action_defs {
        if app.lookup_action(name).is_some() {
            continue;
        }
        let action = gtk::gio::SimpleAction::new(name, None);
        let state = state.clone();
        action.connect_activate(move |_, _| {
            dispatch_shortcut_command(&state, command);
        });
        app.add_action(&action);
    }
}

/// Intercept keyboard shortcuts in the CAPTURE phase for window-level bindings.
fn install_key_capture(window: &adw::ApplicationWindow, state: &State) {
    let key_controller = gtk::EventControllerKey::new();
    key_controller.set_propagation_phase(gtk::PropagationPhase::Capture);

    let state = state.clone();
    key_controller.connect_key_pressed(move |controller, keyval, keycode, modifier| {
        let focused_listening_editor = controller
            .widget()
            .and_then(|widget| widget.downcast::<gtk::Window>().ok())
            .map(|window| focused_widget_is_listening_for_keybind_capture(&window))
            .unwrap_or(false);
        if focused_listening_editor {
            return glib::Propagation::Proceed;
        }

        let matched = {
            let s = state.borrow();
            let display = controller.widget().map(|widget| widget.display());
            shortcut_match_from_key_press(&s.shortcuts, display.as_ref(), keyval, keycode, modifier)
        }
        .filter(|matched| {
            let context = controller
                .widget()
                .and_then(|widget| widget.downcast::<gtk::Window>().ok())
                .map(|window| focused_editable_capture_context(&state, &window))
                .unwrap_or_default();
            !shortcut_blocked_by_editable(matched.command, matched.editable_capture_policy, context)
        })
        .map(|matched| dispatch_shortcut_command(&state, matched.command))
        .unwrap_or(false);

        shortcut_dispatch_propagation(matched)
    });

    window.add_controller(key_controller);
}

fn focused_widget_is_listening_for_keybind_capture(window: &gtk::Window) -> bool {
    let mut widget = gtk::prelude::GtkWindowExt::focus(window);
    while let Some(current) = widget {
        if current.has_css_class(keybind_editor::KEYBIND_EDITOR_LISTENING_CSS) {
            return true;
        }
        widget = current.parent();
    }
    false
}

fn focused_widget_is_editable(window: &gtk::Window) -> bool {
    let mut widget = gtk::prelude::GtkWindowExt::focus(window);
    while let Some(current) = widget {
        if current.is::<gtk::Entry>()
            || current.is::<gtk::SearchEntry>()
            || current.is::<gtk::TextView>()
        {
            return true;
        }
        widget = current.parent();
    }
    false
}

fn focused_editable_capture_context(state: &State, window: &gtk::Window) -> EditableCaptureContext {
    let gtk_editable = focused_widget_is_editable(window);
    match focused_shortcut_target(state) {
        pane::FocusedShortcutTarget::Browser(target) => EditableCaptureContext {
            gtk_editable,
            browser_dom_editable: target.is_page_editable(),
            browser_find_active: target.is_find_active(),
        },
        _ => EditableCaptureContext {
            gtk_editable,
            ..EditableCaptureContext::default()
        },
    }
}

fn shortcut_allowed_while_browser_find_active(command: ShortcutCommand) -> bool {
    matches!(
        command,
        ShortcutCommand::SurfaceFindNext
            | ShortcutCommand::SurfaceFindPrevious
            | ShortcutCommand::SurfaceFindHide
    )
}

fn shortcut_blocked_by_editable(
    command: ShortcutCommand,
    policy: EditableCapturePolicy,
    context: EditableCaptureContext,
) -> bool {
    if policy == EditableCapturePolicy::AlwaysCapture {
        return false;
    }

    if context.browser_find_active && shortcut_allowed_while_browser_find_active(command) {
        return false;
    }

    context.gtk_editable || context.browser_dom_editable
}

fn shortcut_dispatch_propagation(matched: bool) -> glib::Propagation {
    if matched {
        glib::Propagation::Stop
    } else {
        glib::Propagation::Proceed
    }
}

#[cfg(test)]
fn shortcut_command_from_key_event(
    shortcuts: &ResolvedShortcutConfig,
    keyval: gtk::gdk::Key,
    modifier: gtk::gdk::ModifierType,
) -> Option<ShortcutCommand> {
    shortcut_config::NormalizedShortcut::from_gdk_key(keyval, modifier)
        .map(|shortcut| shortcut.to_runtime_combo())
        .and_then(|combo| shortcuts.command_for_runtime_combo(&combo))
}

struct MatchedShortcut {
    command: ShortcutCommand,
    editable_capture_policy: EditableCapturePolicy,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct EditableCaptureContext {
    gtk_editable: bool,
    browser_dom_editable: bool,
    browser_find_active: bool,
}

fn shortcut_match_from_key_press(
    shortcuts: &ResolvedShortcutConfig,
    display: Option<&gtk::gdk::Display>,
    keyval: gtk::gdk::Key,
    keycode: u32,
    modifier: gtk::gdk::ModifierType,
) -> Option<MatchedShortcut> {
    shortcut_config::NormalizedShortcut::from_gdk_key_event(display, keyval, keycode, modifier)
        .map(|shortcut| shortcut.to_runtime_combo())
        .and_then(|combo| shortcuts.shortcut_for_runtime_combo(&combo))
        .map(|shortcut| MatchedShortcut {
            command: shortcut.definition.command,
            editable_capture_policy: shortcut.definition.editable_capture_policy,
        })
}

fn dispatch_shortcut_command(state: &State, command: ShortcutCommand) -> bool {
    match command {
        ShortcutCommand::NewWorkspace => {
            add_workspace(state, None);
            true
        }
        ShortcutCommand::CloseWorkspace => {
            close_workspace(state);
            true
        }
        ShortcutCommand::QuitApp => {
            quit_app(state);
            true
        }
        ShortcutCommand::NewInstance => spawn_new_instance(state),
        ShortcutCommand::ToggleSidebar => {
            toggle_sidebar(state);
            true
        }
        ShortcutCommand::ToggleTopBar => {
            toggle_top_bar(state);
            true
        }
        ShortcutCommand::ToggleFullscreen => {
            toggle_fullscreen(state);
            true
        }
        ShortcutCommand::NextWorkspace => {
            cycle_workspace(state, 1);
            true
        }
        ShortcutCommand::PrevWorkspace => {
            cycle_workspace(state, -1);
            true
        }
        ShortcutCommand::CycleTabPrev => {
            cycle_focused_pane_tab(state, -1);
            true
        }
        ShortcutCommand::CycleTabNext => {
            cycle_focused_pane_tab(state, 1);
            true
        }
        ShortcutCommand::SplitDown => {
            split_focused_pane(state, gtk::Orientation::Vertical);
            true
        }
        ShortcutCommand::NewTerminal => {
            add_tab_to_focused_pane(state, false);
            true
        }
        ShortcutCommand::SplitRight => {
            split_focused_pane(state, gtk::Orientation::Horizontal);
            true
        }
        ShortcutCommand::CloseFocusedPane => {
            close_focused_tab(state);
            true
        }
        ShortcutCommand::FocusLeft => {
            focus_pane_in_direction(state, Direction::Left);
            true
        }
        ShortcutCommand::FocusRight => {
            focus_pane_in_direction(state, Direction::Right);
            true
        }
        ShortcutCommand::FocusUp => {
            focus_pane_in_direction(state, Direction::Up);
            true
        }
        ShortcutCommand::FocusDown => {
            focus_pane_in_direction(state, Direction::Down);
            true
        }
        ShortcutCommand::ActivateWorkspace1 => {
            activate_workspace_shortcut(state, 0);
            true
        }
        ShortcutCommand::ActivateWorkspace2 => {
            activate_workspace_shortcut(state, 1);
            true
        }
        ShortcutCommand::ActivateWorkspace3 => {
            activate_workspace_shortcut(state, 2);
            true
        }
        ShortcutCommand::ActivateWorkspace4 => {
            activate_workspace_shortcut(state, 3);
            true
        }
        ShortcutCommand::ActivateWorkspace5 => {
            activate_workspace_shortcut(state, 4);
            true
        }
        ShortcutCommand::ActivateWorkspace6 => {
            activate_workspace_shortcut(state, 5);
            true
        }
        ShortcutCommand::ActivateWorkspace7 => {
            activate_workspace_shortcut(state, 6);
            true
        }
        ShortcutCommand::ActivateWorkspace8 => {
            activate_workspace_shortcut(state, 7);
            true
        }
        ShortcutCommand::ActivateLastWorkspace => {
            activate_last_workspace_shortcut(state);
            true
        }
        ShortcutCommand::OpenBrowserInSplit
        | ShortcutCommand::BrowserFocusLocation
        | ShortcutCommand::BrowserBack
        | ShortcutCommand::BrowserForward
        | ShortcutCommand::BrowserReload
        | ShortcutCommand::BrowserInspector
        | ShortcutCommand::BrowserConsole => dispatch_browser_command(state, command),
        ShortcutCommand::SurfaceFind
        | ShortcutCommand::SurfaceFindNext
        | ShortcutCommand::SurfaceFindPrevious
        | ShortcutCommand::SurfaceFindHide
        | ShortcutCommand::SurfaceUseSelectionForFind => {
            dispatch_terminal_command(state, command) || dispatch_browser_command(state, command)
        }
        ShortcutCommand::TerminalClearScrollback
        | ShortcutCommand::TerminalCopy
        | ShortcutCommand::TerminalPaste
        | ShortcutCommand::TerminalIncreaseFontSize
        | ShortcutCommand::TerminalDecreaseFontSize
        | ShortcutCommand::TerminalResetFontSize => dispatch_terminal_command(state, command),
    }
}

fn apply_shortcuts_to_application(app: &adw::Application, shortcuts: &ResolvedShortcutConfig) {
    for (action_name, accels) in shortcuts.gtk_accel_entries() {
        let accel_refs: Vec<&str> = accels.iter().map(String::as_str).collect();
        app.set_accels_for_action(action_name, &accel_refs);
    }
}

fn apply_shortcut_config(state: &State, shortcuts: ResolvedShortcutConfig) {
    let (app, workspace_roots, shortcuts_rc) = {
        let mut s = state.borrow_mut();
        s.shortcuts = Rc::new(shortcuts);
        (
            s.app.clone(),
            s.workspaces
                .iter()
                .map(|ws| ws.root.clone())
                .collect::<Vec<_>>(),
            s.shortcuts.clone(),
        )
    };

    apply_shortcuts_to_application(&app, &shortcuts_rc);
    for root in workspace_roots {
        refresh_shortcut_tooltips_in_layout(&root, &shortcuts_rc);
    }
}

fn refresh_shortcut_tooltips_in_layout(widget: &gtk::Widget, shortcuts: &ResolvedShortcutConfig) {
    if let Some(paned) = widget.downcast_ref::<gtk::Paned>() {
        if let Some(start) = paned.start_child() {
            refresh_shortcut_tooltips_in_layout(&start, shortcuts);
        }
        if let Some(end) = paned.end_child() {
            refresh_shortcut_tooltips_in_layout(&end, shortcuts);
        }
        return;
    }

    pane::refresh_shortcut_tooltips(widget, shortcuts);
}

fn persist_shortcut_binding(
    state: &State,
    id: ShortcutId,
    binding: Option<shortcut_config::NormalizedShortcut>,
) -> Result<ResolvedShortcutConfig, String> {
    let updated = {
        let s = state.borrow();
        s.shortcuts
            .with_binding(id, binding)
            .map_err(|err| err.to_string())?
    };

    let Some(path) = shortcut_config::config_path() else {
        return Err("config directory unavailable".to_string());
    };

    shortcut_config::write_shortcuts(&path, &updated).map_err(|err| err.to_string())?;
    let display = {
        let s = state.borrow();
        s.stack.display()
    };
    let reloaded = shortcut_config::load_shortcuts_or_default_with_display(&path, Some(&display));
    if !reloaded.warnings.is_empty() {
        return Err(reloaded.warnings.join("; "));
    }

    apply_shortcut_config(state, reloaded.clone());
    Ok(reloaded)
}

fn open_keybind_editor_tab(state: &State, pane_widget: &gtk::Widget) {
    let shortcuts = {
        let s = state.borrow();
        s.shortcuts.clone()
    };
    let on_capture: Rc<
        dyn Fn(
            ShortcutId,
            Option<shortcut_config::NormalizedShortcut>,
        ) -> Result<ResolvedShortcutConfig, String>,
    > = {
        let state = state.clone();
        Rc::new(move |id, binding| persist_shortcut_binding(&state, id, binding))
    };
    pane::add_keybind_editor_tab_to_pane(pane_widget, shortcuts, on_capture);
}

fn activate_workspace_shortcut(state: &State, idx: usize) {
    let row_and_list = {
        let s = state.borrow();
        s.workspaces
            .get(idx)
            .map(|ws| (idx, ws.sidebar_row.clone(), s.sidebar_list.clone()))
    };

    if let Some((idx, row, list)) = row_and_list {
        switch_workspace(state, idx);
        list.select_row(Some(&row));
    }
}

fn activate_last_workspace_shortcut(state: &State) {
    let last_idx = {
        let s = state.borrow();
        if s.workspaces.is_empty() {
            return;
        }
        s.workspaces.len() - 1
    };
    activate_workspace_shortcut(state, last_idx);
}

// ---------------------------------------------------------------------------
// Sidebar row
// ---------------------------------------------------------------------------

fn build_sidebar_row(
    name: &str,
    folder_path: Option<&str>,
) -> (
    gtk::ListBoxRow,
    gtk::Label,
    gtk::Button,
    gtk::Label,
    gtk::Label,
    gtk::Label,
) {
    let notify_dot = gtk::Label::builder().label("\u{25CF}").build();
    notify_dot.add_css_class("limux-notify-dot-hidden");

    let name_label = gtk::Label::builder()
        .label(name)
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    name_label.add_css_class("limux-ws-name");

    let favorite_button = gtk::Button::with_label("\u{2606}");
    favorite_button.add_css_class("flat");
    favorite_button.add_css_class("limux-ws-star-btn");
    favorite_button.set_focus_on_click(false);
    favorite_button.set_valign(gtk::Align::Center);
    favorite_button.set_halign(gtk::Align::End);
    favorite_button.set_tooltip_text(Some("Favorite workspace"));

    let top_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    top_row.append(&notify_dot);
    top_row.append(&name_label);
    top_row.append(&favorite_button);

    let path_label = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .margin_start(8)
        .build();
    path_label.add_css_class("limux-ws-path");
    if let Some(p) = folder_path {
        path_label.set_label(&abbreviate_path(p));
        path_label.set_tooltip_text(Some(p));
        path_label.set_visible(true);
    } else {
        path_label.set_visible(false);
    }

    let notify_label = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .visible(false)
        .margin_start(8)
        .build();
    notify_label.add_css_class("limux-notify-msg");

    let vbox = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .build();
    vbox.add_css_class("limux-sidebar-row-box");
    vbox.append(&top_row);
    vbox.append(&path_label);
    vbox.append(&notify_label);

    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&vbox));

    (
        row,
        name_label,
        favorite_button,
        notify_dot,
        notify_label,
        path_label,
    )
}

/// Abbreviate a path by replacing the home directory with ~.
fn abbreviate_path(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        if path.starts_with(home_str.as_ref()) {
            return format!("~{}", &path[home_str.len()..]);
        }
    }
    path.to_string()
}

// ---------------------------------------------------------------------------
// Workspace management
// ---------------------------------------------------------------------------

fn favorites_prefix_len(flags: &[bool]) -> usize {
    flags.iter().take_while(|is_favorite| **is_favorite).count()
}

#[cfg(test)]
fn workspace_drop_layout_path(layout: &LayoutNodeState) -> Vec<bool> {
    match layout {
        LayoutNodeState::Pane(_) => Vec::new(),
        LayoutNodeState::Split(split) => {
            let mut path = vec![true];
            path.extend(workspace_drop_layout_path(&split.start));
            path
        }
    }
}

fn tab_drag_workspace_seed(
    source: WorkspaceSeedSource,
    title: &str,
    tab_cwd: Option<String>,
) -> TabDragWorkspaceSeed {
    let name = {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            "Workspace".to_string()
        } else {
            trimmed.to_string()
        }
    };
    let cwd = tab_cwd
        .clone()
        .or_else(|| source.workspace_folder_path.clone())
        .or(source.workspace_cwd.clone());
    let folder_path = tab_cwd
        .filter(|cwd| !cwd.trim().is_empty())
        .or(source.workspace_folder_path)
        .filter(|path| !path.trim().is_empty());

    TabDragWorkspaceSeed {
        name,
        cwd,
        folder_path,
    }
}

fn show_workspace_context_menu(state: &State, workspace_id: &str, row: &gtk::ListBoxRow) {
    let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    menu_box.set_margin_top(4);
    menu_box.set_margin_bottom(4);
    menu_box.set_margin_start(4);
    menu_box.set_margin_end(4);

    let rename_btn = gtk::Button::with_label("Rename");
    rename_btn.add_css_class("flat");
    let delete_btn = gtk::Button::with_label("Delete");
    delete_btn.add_css_class("flat");
    delete_btn.add_css_class("destructive-action");

    menu_box.append(&rename_btn);
    menu_box.append(&delete_btn);

    let popover = gtk::Popover::new();
    popover.set_child(Some(&menu_box));
    popover.set_parent(row);
    popover.set_position(gtk::PositionType::Right);

    {
        let state = state.clone();
        let ws_id = workspace_id.to_string();
        let pop = popover.clone();
        rename_btn.connect_clicked(move |_| {
            pop.popdown();
            begin_workspace_inline_rename(&state, &ws_id);
        });
    }
    {
        let state = state.clone();
        let ws_id = workspace_id.to_string();
        let pop = popover.clone();
        delete_btn.connect_clicked(move |_| {
            pop.popdown();
            close_workspace_by_id(&state, &ws_id);
            request_session_save(&state);
        });
    }
    {
        popover.connect_closed(move |p| {
            p.unparent();
        });
    }

    popover.popup();
}

fn clamp_workspace_insert_index_for_pinning(
    favorite_flags_after_removal: &[bool],
    moving_is_favorite: bool,
    proposed_index: usize,
) -> usize {
    let favorites_top = favorites_prefix_len(favorite_flags_after_removal);
    if moving_is_favorite {
        proposed_index.min(favorites_top)
    } else {
        proposed_index.max(favorites_top)
    }
}

fn sync_sidebar_row_order(state: &mut AppState) {
    while let Some(child) = state.sidebar_list.first_child() {
        state.sidebar_list.remove(&child);
    }
    for workspace in &state.workspaces {
        state.sidebar_list.append(&workspace.sidebar_row);
    }
}

fn set_workspace_favorite_visual(workspace: &Workspace) {
    let symbol = if workspace.favorite {
        "\u{2605}"
    } else {
        "\u{2606}"
    };
    workspace.favorite_button.set_label(symbol);
    if workspace.favorite {
        workspace
            .favorite_button
            .add_css_class("limux-ws-star-btn-active");
    } else {
        workspace
            .favorite_button
            .remove_css_class("limux-ws-star-btn-active");
    }
}

/// Find an active rename Entry in the sidebar (if any).
fn find_active_rename_entry(sidebar_list: &gtk::ListBox) -> Option<gtk::Entry> {
    fn find_entry(widget: &gtk::Widget) -> Option<gtk::Entry> {
        if let Some(entry) = widget.downcast_ref::<gtk::Entry>() {
            return Some(entry.clone());
        }
        let mut child = widget.first_child();
        while let Some(c) = child {
            if let Some(entry) = find_entry(&c) {
                return Some(entry);
            }
            child = c.next_sibling();
        }
        None
    }
    let mut row = sidebar_list.first_child();
    while let Some(r) = row {
        if let Some(entry) = find_entry(&r) {
            return Some(entry);
        }
        row = r.next_sibling();
    }
    None
}

/// Find any active rename Entry in the sidebar and trigger its activate signal to commit.
fn commit_any_active_rename(sidebar_list: &gtk::ListBox) {
    let mut row = sidebar_list.first_child();
    while let Some(r) = row {
        // Walk into the row's children to find a gtk::Entry
        fn find_entry(widget: &gtk::Widget) -> Option<gtk::Entry> {
            if let Some(entry) = widget.downcast_ref::<gtk::Entry>() {
                return Some(entry.clone());
            }
            let mut child = widget.first_child();
            while let Some(c) = child {
                if let Some(entry) = find_entry(&c) {
                    return Some(entry);
                }
                child = c.next_sibling();
            }
            None
        }
        if let Some(entry) = find_entry(&r) {
            entry.emit_activate();
            return;
        }
        row = r.next_sibling();
    }
}

fn begin_workspace_inline_rename(state: &State, workspace_id: &str) {
    let (label, current_name) = {
        let s = state.borrow();
        let Some(workspace) = s
            .workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
        else {
            return;
        };
        (workspace.name_label.clone(), workspace.name.clone())
    };

    let Some(parent) = label.parent().and_then(|p| p.downcast::<gtk::Box>().ok()) else {
        return;
    };

    // Avoid stacking multiple rename entries if the user right-clicks repeatedly.
    let mut child = parent.first_child();
    while let Some(widget) = child {
        if widget.is::<gtk::Entry>() {
            return;
        }
        child = widget.next_sibling();
    }

    let entry = gtk::Entry::builder()
        .text(&current_name)
        .hexpand(true)
        .build();
    entry.add_css_class("limux-ws-rename-entry");

    label.set_visible(false);
    parent.insert_child_after(&entry, Some(&label));
    entry.grab_focus();
    entry.select_region(0, -1);

    let commit_guard = Rc::new(std::cell::Cell::new(false));
    let state_for_commit = state.clone();
    let workspace_id = workspace_id.to_string();
    let label_for_commit = label.clone();
    let parent_for_commit = parent.clone();
    let commit = {
        let commit_guard = commit_guard.clone();
        move |entry: &gtk::Entry| {
            if commit_guard.get() {
                return;
            }
            commit_guard.set(true);

            let next_name = entry.text().trim().to_string();
            if !next_name.is_empty() {
                label_for_commit.set_label(&next_name);
                let mut s = state_for_commit.borrow_mut();
                if let Some(workspace) = s
                    .workspaces
                    .iter_mut()
                    .find(|workspace| workspace.id == workspace_id)
                {
                    workspace.name = next_name;
                }
                drop(s);
                request_session_save(&state_for_commit);
            }

            label_for_commit.set_visible(true);
            parent_for_commit.remove(entry);
        }
    };

    {
        let commit = commit.clone();
        entry.connect_activate(move |entry| {
            commit(entry);
        });
    }
    {
        let commit = commit.clone();
        let focus = gtk::EventControllerFocus::new();
        focus.connect_leave(move |controller| {
            if let Some(widget) = controller.widget() {
                if let Some(entry) = widget.downcast_ref::<gtk::Entry>() {
                    commit(entry);
                }
            }
        });
        entry.add_controller(focus);
    }
}

fn reorder_workspace_by_id(
    state: &State,
    source_id: &str,
    target_id: &str,
    drop_below: bool,
) -> bool {
    let (sidebar_list, row_to_select) = {
        let mut s = state.borrow_mut();
        let Some(source_idx) = s
            .workspaces
            .iter()
            .position(|workspace| workspace.id == source_id)
        else {
            return false;
        };
        let Some(target_idx) = s
            .workspaces
            .iter()
            .position(|workspace| workspace.id == target_id)
        else {
            return false;
        };
        if source_idx == target_idx {
            return false;
        }

        let active_workspace_id = s.active_workspace().map(|workspace| workspace.id.clone());
        let moving_workspace = s.workspaces.remove(source_idx);
        let Some(target_idx_after_removal) = s
            .workspaces
            .iter()
            .position(|workspace| workspace.id == target_id)
        else {
            s.workspaces.insert(source_idx, moving_workspace);
            return false;
        };

        // Insert after the target when dropping on the bottom half
        let raw_insert_idx = if drop_below {
            target_idx_after_removal + 1
        } else {
            target_idx_after_removal
        };

        let favorite_flags: Vec<bool> = s
            .workspaces
            .iter()
            .map(|workspace| workspace.favorite)
            .collect();
        let insert_idx = clamp_workspace_insert_index_for_pinning(
            &favorite_flags,
            moving_workspace.favorite,
            raw_insert_idx,
        );
        s.workspaces.insert(insert_idx, moving_workspace);

        if let Some(active_workspace_id) = active_workspace_id {
            if let Some(new_active_idx) = s
                .workspaces
                .iter()
                .position(|workspace| workspace.id == active_workspace_id)
            {
                s.active_idx = new_active_idx;
            }
        }

        sync_sidebar_row_order(&mut s);
        let row_to_select = s
            .workspaces
            .get(s.active_idx)
            .map(|workspace| workspace.sidebar_row.clone());
        (s.sidebar_list.clone(), row_to_select)
    };

    if let Some(row) = row_to_select {
        sidebar_list.select_row(Some(&row));
    }
    request_session_save(state);

    true
}

fn toggle_workspace_favorite(state: &State, workspace_id: &str) {
    let (sidebar_list, row_to_select) = {
        let mut s = state.borrow_mut();
        let Some(idx) = s
            .workspaces
            .iter()
            .position(|workspace| workspace.id == workspace_id)
        else {
            return;
        };

        let active_workspace_id = s.active_workspace().map(|workspace| workspace.id.clone());
        s.workspaces[idx].favorite = !s.workspaces[idx].favorite;
        set_workspace_favorite_visual(&s.workspaces[idx]);

        let workspace = s.workspaces.remove(idx);
        let favorite_flags: Vec<bool> = s
            .workspaces
            .iter()
            .map(|candidate| candidate.favorite)
            .collect();
        let insert_idx = favorites_prefix_len(&favorite_flags);
        s.workspaces.insert(insert_idx, workspace);

        if let Some(active_workspace_id) = active_workspace_id {
            if let Some(new_active_idx) = s
                .workspaces
                .iter()
                .position(|workspace| workspace.id == active_workspace_id)
            {
                s.active_idx = new_active_idx;
            }
        }

        sync_sidebar_row_order(&mut s);
        let row_to_select = s
            .workspaces
            .get(s.active_idx)
            .map(|workspace| workspace.sidebar_row.clone());
        (s.sidebar_list.clone(), row_to_select)
    };

    if let Some(row) = row_to_select {
        sidebar_list.select_row(Some(&row));
    }
    request_session_save(state);
}

fn handle_tab_drop_to_workspace(state: &State, target_workspace_id: &str, payload: &str) -> bool {
    let Some((pane_id, tab_id)) = payload.split_once(':') else {
        return false;
    };
    let Ok(source_pane_id) = pane_id.parse::<u32>() else {
        return false;
    };
    let Some(source_pane) = pane::find_pane_widget_by_id(source_pane_id) else {
        return false;
    };

    let target_pane = {
        let app_state = state.borrow();
        let Some(workspace) = app_state
            .workspaces
            .iter()
            .find(|workspace| workspace.id == target_workspace_id)
        else {
            return false;
        };
        find_leaf_pane(&workspace.root, gtk::Orientation::Horizontal, true)
    };

    pane::move_tab_to_pane(&source_pane, tab_id, &target_pane)
}

fn create_workspace_for_tab(state: &State, payload: &str) -> bool {
    let Some((pane_id, tab_id)) = payload.split_once(':') else {
        return false;
    };
    let Ok(source_pane_id) = pane_id.parse::<u32>() else {
        return false;
    };
    let Some(source_pane) = pane::find_pane_widget_by_id(source_pane_id) else {
        return false;
    };

    let title = pane::tab_title(&source_pane, tab_id).unwrap_or_else(|| "Workspace".to_string());
    let tab_cwd = pane::tab_working_directory(&source_pane, tab_id);
    let seed = {
        let app_state = state.borrow();
        let source = app_state
            .workspace_for_widget(&source_pane)
            .map(|workspace| WorkspaceSeedSource {
                workspace_cwd: workspace.cwd.borrow().clone(),
                workspace_folder_path: workspace.folder_path.clone(),
            })
            .unwrap_or(WorkspaceSeedSource {
                workspace_cwd: None,
                workspace_folder_path: None,
            });
        tab_drag_workspace_seed(source, &title, tab_cwd)
    };

    let shortcuts = {
        let app_state = state.borrow();
        app_state.shortcuts.clone()
    };
    let new_workspace_id = uuid::Uuid::new_v4().to_string();
    let stack_name = format!("ws-{new_workspace_id}");
    let root = create_pane_for_workspace(
        state,
        &shortcuts,
        &new_workspace_id,
        seed.cwd.as_deref(),
        None,
        true,
    );

    let (row, name_label, favorite_button, notify_dot, notify_label, path_label) =
        build_sidebar_row(&seed.name, seed.folder_path.as_deref());
    let row_clone = row.clone();
    {
        let mut app_state = state.borrow_mut();
        app_state.stack.add_named(&root, Some(&stack_name));
        app_state.sidebar_list.append(&row);
        install_workspace_row_interactions(state, &new_workspace_id, &row, &favorite_button);

        app_state.workspaces.push(Workspace {
            id: new_workspace_id.clone(),
            name: seed.name.clone(),
            root: root.clone().upcast(),
            sidebar_row: row,
            name_label,
            favorite_button,
            notify_dot,
            notify_label,
            unread: false,
            favorite: false,
            cwd: Rc::new(RefCell::new(seed.cwd.clone())),
            folder_path: seed.folder_path.clone(),
            path_label,
        });
        app_state.active_idx = app_state.workspaces.len() - 1;
        app_state.stack.set_visible_child_name(&stack_name);
    }

    {
        let sidebar_list = state.borrow().sidebar_list.clone();
        sidebar_list.select_row(Some(&row_clone));
    }

    if pane::move_tab_to_pane(&source_pane, tab_id, &root.clone().upcast()) {
        request_session_save(state);
        return true;
    }
    false
}

fn install_workspace_row_interactions(
    state: &State,
    workspace_id: &str,
    row: &gtk::ListBoxRow,
    favorite_button: &gtk::Button,
) {
    let right_click = gtk::GestureClick::new();
    right_click.set_button(3);
    {
        let state = state.clone();
        let workspace_id = workspace_id.to_string();
        let r = row.clone();
        right_click.connect_pressed(move |_, _, _, _| {
            show_workspace_context_menu(&state, &workspace_id, &r);
        });
    }
    row.add_controller(right_click);

    let drag_source = gtk::DragSource::new();
    drag_source.set_actions(gtk::gdk::DragAction::MOVE);
    {
        let workspace_id = workspace_id.to_string();
        drag_source.connect_prepare(move |_, _, _| {
            let payload = glib::Value::from(&workspace_id);
            Some(gtk::gdk::ContentProvider::for_value(&payload))
        });
    }
    {
        let state = state.clone();
        let row = row.clone();
        let workspace_id = workspace_id.to_string();
        drag_source.connect_drag_begin(move |source, _| {
            let mut s = state.borrow_mut();
            s.workspace_dragging = Some(workspace_id.clone());
            s.new_ws_btn.set_label("\u{1F5D1}\u{FE0E}");
            s.new_ws_btn.add_css_class("limux-sidebar-btn-trash");
            drop(s);
            pane::set_workspace_dragging_all(true);
            let icon = gtk::WidgetPaintable::new(Some(&row));
            source.set_icon(Some(&icon), 0, 0);
        });
    }
    {
        let state = state.clone();
        drag_source.connect_drag_end(move |_, _, _| {
            let mut s = state.borrow_mut();
            s.workspace_dragging = None;
            s.new_ws_btn.set_label("New Workspace");
            s.new_ws_btn.remove_css_class("limux-sidebar-btn-trash");
            s.new_ws_btn
                .remove_css_class("limux-sidebar-btn-trash-hover");
            pane::set_workspace_dragging_all(false);
        });
    }
    row.add_controller(drag_source);

    let drop_target = gtk::DropTarget::new(glib::Type::STRING, gtk::gdk::DragAction::MOVE);
    drop_target.set_preload(true);
    let hover_timer: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
    let drop_handled = Rc::new(Cell::new(false));
    {
        let r = row.clone();
        let state = state.clone();
        let hover_timer = hover_timer.clone();
        let target_workspace_id = workspace_id.to_string();
        let drop_handled = drop_handled.clone();
        drop_target.connect_motion(move |_, _x, y| {
            drop_handled.set(false);
            let h = r.height() as f64;
            r.remove_css_class("limux-drop-above");
            r.remove_css_class("limux-drop-below");
            r.remove_css_class("limux-tab-drop-target");

            let dragged_workspace = state.borrow().workspace_dragging.clone();
            match dragged_workspace {
                Some(ref dragged_workspace_id) if dragged_workspace_id != &target_workspace_id => {
                    if y < h / 2.0 {
                        r.add_css_class("limux-drop-above");
                    } else {
                        r.add_css_class("limux-drop-below");
                    }
                }
                None => {
                    r.add_css_class("limux-tab-drop-target");
                }
                _ => {}
            }

            if hover_timer.borrow().is_none() {
                let state = state.clone();
                let target_workspace_id = target_workspace_id.clone();
                let hover_timer = hover_timer.clone();
                let drop_handled = drop_handled.clone();
                let timer_for_callback = hover_timer.clone();
                let source = glib::timeout_add_local_once(
                    std::time::Duration::from_millis(500),
                    move || {
                        *timer_for_callback.borrow_mut() = None;
                        if drop_handled.get() {
                            return;
                        }
                        let (target_idx, sidebar_row, sidebar_list) = {
                            let app_state = state.borrow();
                            let idx = app_state
                                .workspaces
                                .iter()
                                .position(|workspace| workspace.id == target_workspace_id);
                            let sidebar_row = idx.and_then(|idx| {
                                app_state
                                    .workspaces
                                    .get(idx)
                                    .map(|workspace| workspace.sidebar_row.clone())
                            });
                            (idx, sidebar_row, app_state.sidebar_list.clone())
                        };
                        if let Some(target_idx) = target_idx {
                            switch_workspace(&state, target_idx);
                        }
                        if let Some(sidebar_row) = sidebar_row {
                            sidebar_list.select_row(Some(&sidebar_row));
                        }
                    },
                );
                *hover_timer.borrow_mut() = Some(source);
            }
            gtk::gdk::DragAction::MOVE
        });
    }
    {
        let r = row.clone();
        let hover_timer = hover_timer.clone();
        drop_target.connect_leave(move |_| {
            r.remove_css_class("limux-drop-above");
            r.remove_css_class("limux-drop-below");
            r.remove_css_class("limux-tab-drop-target");
            if let Some(source) = hover_timer.borrow_mut().take() {
                source.remove();
            }
        });
    }
    {
        let state = state.clone();
        let target_workspace_id = workspace_id.to_string();
        let r = row.clone();
        let hover_timer = hover_timer.clone();
        let drop_handled = drop_handled.clone();
        drop_target.connect_drop(move |_dt, value, _, y| {
            drop_handled.set(true);
            r.remove_css_class("limux-drop-above");
            r.remove_css_class("limux-drop-below");
            r.remove_css_class("limux-tab-drop-target");
            if let Some(source) = hover_timer.borrow_mut().take() {
                source.remove();
            }
            if let Ok(payload) = value.get::<String>() {
                if payload.contains(':') {
                    return handle_tab_drop_to_workspace(&state, &target_workspace_id, &payload);
                }
                let drop_below = y >= r.height() as f64 / 2.0;
                if payload != target_workspace_id {
                    return reorder_workspace_by_id(
                        &state,
                        &payload,
                        &target_workspace_id,
                        drop_below,
                    );
                }
            }
            false
        });
    }
    row.add_controller(drop_target);

    {
        let state = state.clone();
        let workspace_id = workspace_id.to_string();
        favorite_button.connect_clicked(move |_| {
            toggle_workspace_favorite(&state, &workspace_id);
        });
    }
}

#[allow(deprecated)]
fn add_workspace(state: &State, _working_directory: Option<&str>) {
    // Open a folder chooser dialog (using FileChooserDialog to avoid portal crashes)
    let window: Option<gtk::Window> = {
        let s = state.borrow();
        s.stack
            .root()
            .and_then(|r| r.downcast::<gtk::Window>().ok())
    };

    let dialog = gtk::FileChooserDialog::new(
        Some("Open Folder as Workspace"),
        window.as_ref(),
        gtk::FileChooserAction::SelectFolder,
        &[
            ("Cancel", gtk::ResponseType::Cancel),
            ("Open", gtk::ResponseType::Accept),
        ],
    );
    dialog.set_modal(true);

    // Start in the home directory
    if let Some(home) = dirs::home_dir() {
        let home_file = gtk::gio::File::for_path(&home);
        let _ = dialog.set_current_folder(Some(&home_file));
    }

    let state = state.clone();
    dialog.connect_response(move |dlg, response| {
        if response == gtk::ResponseType::Accept {
            if let Some(file) = dlg.file() {
                if let Some(path) = file.path() {
                    let path_str = path.to_string_lossy().to_string();
                    let folder_name = path
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_else(|| path_str.clone());
                    create_workspace_with_folder(&state, &folder_name, &path_str);
                }
            }
        }
        dlg.close();
    });

    dialog.show();
}

fn create_workspace_with_folder(state: &State, name: &str, folder_path: &str) {
    let workspace = WorkspaceState {
        name: name.to_string(),
        favorite: false,
        cwd: Some(folder_path.to_string()),
        folder_path: Some(folder_path.to_string()),
        layout: LayoutNodeState::Pane(PaneState::fallback(Some(folder_path))),
    };
    add_workspace_from_state(state, &workspace);
    request_session_save(state);
}

fn add_workspace_from_state(state: &State, workspace: &WorkspaceState) {
    let shortcuts = {
        let s = state.borrow();
        s.shortcuts.clone()
    };
    let id = uuid::Uuid::new_v4().to_string();
    let stack_name = format!("ws-{id}");
    let working_dir = workspace
        .folder_path
        .as_deref()
        .or(workspace.cwd.as_deref());
    let root = build_workspace_root(state, &shortcuts, &id, working_dir, Some(&workspace.layout));
    let mut s = state.borrow_mut();

    s.stack.add_named(&root, Some(&stack_name));

    let (row, name_label, favorite_button, notify_dot, notify_label, path_label) =
        build_sidebar_row(&workspace.name, workspace.folder_path.as_deref());
    s.sidebar_list.append(&row);
    install_workspace_row_interactions(state, &id, &row, &favorite_button);

    let cwd: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(workspace.cwd.clone()));
    let ws = Workspace {
        id,
        name: workspace.name.clone(),
        root,
        sidebar_row: row.clone(),
        name_label,
        favorite_button,
        notify_dot,
        notify_label,
        unread: false,
        favorite: workspace.favorite,
        cwd,
        folder_path: workspace.folder_path.clone(),
        path_label,
    };

    if workspace.favorite {
        set_workspace_favorite_visual(&ws);
    }

    s.workspaces.push(ws);
    let new_idx = s.workspaces.len() - 1;
    s.active_idx = new_idx;
    s.stack.set_visible_child_name(&stack_name);

    let sidebar_list = s.sidebar_list.clone();
    drop(s);

    sidebar_list.select_row(Some(&row));
}

/// Create a PaneWidget wired up with callbacks for a specific workspace.
fn create_pane_for_workspace(
    state: &State,
    shortcuts: &Rc<ResolvedShortcutConfig>,
    ws_id: &str,
    working_directory: Option<&str>,
    initial_state: Option<&PaneState>,
    skip_default_tab: bool,
) -> gtk::Box {
    let state_for_split = state.clone();
    let state_for_close = state.clone();
    let state_for_bell = state.clone();
    let state_for_keybinds = state.clone();
    let state_for_pwd = state.clone();
    let state_for_empty = state.clone();
    let ws_id_split = ws_id.to_string();
    let ws_id_close = ws_id.to_string();
    let ws_id_bell = ws_id.to_string();
    let ws_id_pwd = ws_id.to_string();
    let ws_id_empty = ws_id.to_string();
    let state_for_split_with_tab = state.clone();
    let ws_id_split_with_tab = ws_id.to_string();

    let callbacks = Rc::new(PaneCallbacks {
        on_split: Box::new(move |pane_widget, orientation| {
            split_pane(
                &state_for_split,
                &ws_id_split,
                pane_widget,
                orientation,
                None,
                false,
                false,
                true,
            );
        }),
        on_close_pane: Box::new(move |pane_widget| {
            remove_pane_internal(&state_for_close, &ws_id_close, pane_widget, true);
        }),
        on_bell: Box::new(move || {
            // Defer to avoid RefCell borrow conflicts — bell can fire during state mutation
            let state = state_for_bell.clone();
            let ws_id = ws_id_bell.clone();
            glib::idle_add_local_once(move || {
                mark_workspace_unread(&state, &ws_id);
            });
        }),
        on_open_keybinds: Box::new(move |anchor| {
            open_keybind_editor_tab(&state_for_keybinds, anchor);
        }),
        current_shortcuts: Box::new({
            let state = state.clone();
            move || {
                let s = state.borrow();
                s.shortcuts.clone()
            }
        }),
        on_capture_shortcut: {
            let state = state.clone();
            Rc::new(move |id, binding| persist_shortcut_binding(&state, id, binding))
        },
        on_pwd_changed: Box::new(move |pwd: &str| {
            let state = state_for_pwd.clone();
            let ws_id = ws_id_pwd.clone();
            let pwd = pwd.to_string();
            glib::idle_add_local_once(move || {
                let s = state.borrow();
                if let Some(ws) = s.workspaces.iter().find(|w| w.id == ws_id) {
                    *ws.cwd.borrow_mut() = Some(pwd);
                }
            });
        }),
        on_empty: Box::new(move |pane_widget, reason| {
            let persist = matches!(reason, pane::PaneEmptyReason::ClosedLastTab);
            remove_pane_internal(&state_for_empty, &ws_id_empty, pane_widget, persist);
        }),
        on_state_changed: Box::new({
            let state = state.clone();
            move || request_session_save(&state)
        }),
        on_split_with_tab: Box::new(
            move |source_pane, target_pane, orientation, tab_id, new_pane_first| {
                handle_split_with_tab(
                    &state_for_split_with_tab,
                    &ws_id_split_with_tab,
                    source_pane,
                    target_pane,
                    orientation,
                    &tab_id,
                    new_pane_first,
                );
            },
        ),
    });

    pane::create_pane(
        callbacks,
        shortcuts.clone(),
        working_directory,
        initial_state,
        skip_default_tab,
    )
}

fn close_workspace(state: &State) {
    let id = {
        let s = state.borrow();
        s.active_workspace().map(|w| w.id.clone())
    };
    if let Some(id) = id {
        close_workspace_by_id(state, &id);
    }
}

fn close_workspace_by_id(state: &State, id: &str) {
    let mut s = state.borrow_mut();
    let Some(idx) = s.workspaces.iter().position(|w| w.id == id) else {
        return;
    };

    let ws = s.workspaces.remove(idx);
    s.stack.remove(&ws.root);
    s.sidebar_list.remove(&ws.sidebar_row);

    if s.workspaces.is_empty() {
        s.active_idx = 0;
        drop(s);
        request_session_save(state);
        return;
    }

    let new_idx = idx.min(s.workspaces.len() - 1);
    s.active_idx = new_idx;

    let stack_name = format!("ws-{}", s.workspaces[new_idx].id);
    s.stack.set_visible_child_name(&stack_name);

    let row = s.workspaces[new_idx].sidebar_row.clone();
    let sidebar_list = s.sidebar_list.clone();
    drop(s);

    sidebar_list.select_row(Some(&row));
    request_session_save(state);
}

fn switch_workspace(state: &State, idx: usize) {
    let mut s = state.borrow_mut();
    if idx >= s.workspaces.len() || idx == s.active_idx {
        return;
    }
    s.active_idx = idx;
    let stack_name = format!("ws-{}", s.workspaces[idx].id);
    s.stack.set_visible_child_name(&stack_name);

    // Clear unread
    let ws = &mut s.workspaces[idx];
    if ws.unread {
        ws.unread = false;
        ws.notify_dot.remove_css_class("limux-notify-dot");
        ws.notify_dot.add_css_class("limux-notify-dot-hidden");
        ws.notify_label.remove_css_class("limux-notify-msg-unread");
        ws.notify_label.add_css_class("limux-notify-msg");
        ws.notify_label.set_visible(false);
        // Remove glow pulse from sidebar row
        if let Some(row_box) = ws.sidebar_row.child() {
            row_box.remove_css_class("limux-sidebar-row-unread");
        }
    }
    drop(s);
    request_session_save(state);
}

fn cycle_workspace(state: &State, direction: i32) {
    let (new_idx, row, sidebar_list) = {
        let s = state.borrow();
        let len = s.workspaces.len();
        if len <= 1 {
            return;
        }
        let new_idx = ((s.active_idx as i32 + direction).rem_euclid(len as i32)) as usize;
        (
            new_idx,
            s.workspaces[new_idx].sidebar_row.clone(),
            s.sidebar_list.clone(),
        )
    };
    switch_workspace(state, new_idx);
    sidebar_list.select_row(Some(&row));
}

/// Default sidebar width in pixels.
const SIDEBAR_WIDTH: i32 = 220;

fn sync_top_bar_visibility(state: &State) {
    let (top_bar, preferred_visible, fullscreened) = {
        let s = state.borrow();
        (
            s.top_bar.clone(),
            s.top_bar_visible,
            gtk::prelude::GtkWindowExt::is_fullscreen(&s.window),
        )
    };

    if let Some(top_bar) = top_bar {
        top_bar.set_visible(preferred_visible && !fullscreened);
    }
}

fn toggle_top_bar(state: &State) {
    {
        let mut s = state.borrow_mut();
        s.top_bar_visible = !s.top_bar_visible;
    }
    sync_top_bar_visibility(state);
    request_session_save(state);
}

fn toggle_fullscreen(state: &State) {
    let window = state.borrow().window.clone();
    if gtk::prelude::GtkWindowExt::is_fullscreen(&window) {
        window.unfullscreen();
    } else {
        window.fullscreen();
    }
}

fn toggle_sidebar(state: &State) {
    let (paned, sidebar, current, is_visible, target_width, prior_animation, epoch) = {
        let mut s = state.borrow_mut();
        let Some(sidebar) = s.paned.start_child() else {
            return;
        };
        let current = s.paned.position();
        let is_visible = current > 10; // treat < 10px as collapsed
        if is_visible {
            s.sidebar_expanded_width = current;
        }
        let target_width = s.sidebar_expanded_width.max(SIDEBAR_WIDTH);
        let prior_animation = s.sidebar_animation.take();
        s.sidebar_animation_epoch = s.sidebar_animation_epoch.wrapping_add(1);
        (
            s.paned.clone(),
            sidebar,
            current,
            is_visible,
            target_width,
            prior_animation,
            s.sidebar_animation_epoch,
        )
    };

    if let Some(animation) = prior_animation {
        animation.pause();
    }

    if is_visible {
        // Collapse: animate position to 0, then hide sidebar.
        let target = adw::CallbackAnimationTarget::new({
            let p = paned.clone();
            move |value| {
                p.set_position(value as i32);
            }
        });
        let animation = adw::TimedAnimation::builder()
            .widget(&paned)
            .value_from(current as f64)
            .value_to(0.0)
            .duration(200)
            .easing(adw::Easing::EaseInOutCubic)
            .target(&target)
            .build();
        let state_for_done = state.clone();
        animation.connect_done(move |_| {
            let is_current = {
                let mut s = state_for_done.borrow_mut();
                if s.sidebar_animation_epoch != epoch {
                    false
                } else {
                    s.sidebar_animation = None;
                    true
                }
            };
            if is_current {
                sidebar.set_visible(false);
                request_session_save(&state_for_done);
            }
        });
        state.borrow_mut().sidebar_animation = Some(animation.clone());
        animation.play();
    } else {
        // Expand: make sidebar visible, then animate position from 0 to remembered width.
        sidebar.set_visible(true);
        paned.set_position(0);
        let target = adw::CallbackAnimationTarget::new({
            let p = paned.clone();
            move |value| {
                p.set_position(value as i32);
            }
        });
        let animation = adw::TimedAnimation::builder()
            .widget(&paned)
            .value_from(0.0)
            .value_to(target_width as f64)
            .duration(200)
            .easing(adw::Easing::EaseInOutCubic)
            .target(&target)
            .build();
        let state_for_done = state.clone();
        animation.connect_done(move |_| {
            let is_current = {
                let mut s = state_for_done.borrow_mut();
                if s.sidebar_animation_epoch != epoch {
                    false
                } else {
                    s.sidebar_animation = None;
                    true
                }
            };
            if is_current {
                request_session_save(&state_for_done);
            }
        });
        state.borrow_mut().sidebar_animation = Some(animation.clone());
        animation.play();
    }
}

// ---------------------------------------------------------------------------
// Split / close pane operations
// ---------------------------------------------------------------------------

fn split_pane(
    state: &State,
    ws_id: &str,
    pane_widget: &gtk::Widget,
    orientation: gtk::Orientation,
    initial_state: Option<PaneState>,
    skip_default_tab: bool,
    new_pane_first: bool,
    persist: bool,
) -> gtk::Widget {
    // Use the workspace's folder_path (or current cwd) for the new pane
    let (shortcuts, wd) = {
        let s = state.borrow();
        (
            s.shortcuts.clone(),
            s.workspaces
                .iter()
                .find(|w| w.id == ws_id)
                .and_then(|ws| ws.folder_path.clone().or_else(|| ws.cwd.borrow().clone())),
        )
    };
    let new_pane = create_pane_for_workspace(
        state,
        &shortcuts,
        ws_id,
        wd.as_deref(),
        initial_state.as_ref(),
        skip_default_tab,
    );

    let parent = pane_widget.parent();

    let new_paned = gtk::Paned::builder()
        .orientation(orientation)
        .hexpand(true)
        .vexpand(true)
        .build();
    update_split_ratio_state(&new_paned, layout_state::DEFAULT_SPLIT_RATIO);
    attach_split_position_persistence(state, &new_paned);

    if let Some(parent) = parent {
        if let Some(paned_parent) = parent.downcast_ref::<gtk::Paned>() {
            let is_start = paned_parent
                .start_child()
                .map(|c| c == *pane_widget)
                .unwrap_or(false);
            if is_start {
                paned_parent.set_start_child(Some(&new_paned));
            } else {
                paned_parent.set_end_child(Some(&new_paned));
            }
        } else if let Some(stack) = parent.downcast_ref::<gtk::Stack>() {
            let page_name = format!("ws-{ws_id}");
            stack.remove(pane_widget);
            stack.add_named(&new_paned, Some(&page_name));
            stack.set_visible_child_name(&page_name);
            // Update root reference
            let mut s = state.borrow_mut();
            if let Some(ws) = s.workspaces.iter_mut().find(|w| w.id == ws_id) {
                ws.root = new_paned.clone().upcast();
            }
        }
    }

    if new_pane_first {
        new_paned.set_start_child(Some(&new_pane));
        new_paned.set_end_child(Some(pane_widget));
    } else {
        new_paned.set_start_child(Some(pane_widget));
        new_paned.set_end_child(Some(&new_pane));
    }

    // 50% split after layout
    {
        let np = new_paned.clone();
        glib::idle_add_local_once(move || {
            let alloc = np.allocation();
            let size = if orientation == gtk::Orientation::Horizontal {
                alloc.width()
            } else {
                alloc.height()
            };
            if size > 0 {
                np.set_position(size / 2);
            }
        });
    }
    if persist {
        request_session_save(state);
    }
    new_pane.upcast()
}

fn remove_pane(state: &State, ws_id: &str, pane_widget: &gtk::Widget) {
    remove_pane_internal(state, ws_id, pane_widget, true);
}

fn remove_pane_internal(state: &State, ws_id: &str, pane_widget: &gtk::Widget, persist: bool) {
    let parent = pane_widget.parent();

    let Some(parent) = parent else {
        return;
    };

    if let Some(paned) = parent.downcast_ref::<gtk::Paned>() {
        // Find sibling
        let sibling = if paned
            .start_child()
            .map(|c| c == *pane_widget)
            .unwrap_or(false)
        {
            paned.end_child()
        } else {
            paned.start_child()
        };

        if let Some(sibling) = sibling {
            // Move focus to the sibling's GLArea before detaching to avoid
            // GTK focus tracking warnings on ancestor Paneds.
            if let Some(gl) = find_gl_area(&sibling) {
                gl.grab_focus();
            }

            // Walk up and clear focus_child on all ancestor Paneds
            let mut ancestor = paned.parent();
            while let Some(a) = ancestor {
                if let Some(ap) = a.downcast_ref::<gtk::Paned>() {
                    ap.set_focus_child(gtk::Widget::NONE);
                }
                ancestor = a.parent();
            }
            paned.set_focus_child(gtk::Widget::NONE);
            paned.set_start_child(gtk::Widget::NONE);
            paned.set_end_child(gtk::Widget::NONE);

            if let Some(grandparent) = paned.parent() {
                if let Some(gp_paned) = grandparent.downcast_ref::<gtk::Paned>() {
                    let is_start = gp_paned
                        .start_child()
                        .map(|c| c == paned.clone().upcast::<gtk::Widget>())
                        .unwrap_or(false);
                    if is_start {
                        gp_paned.set_start_child(Some(&sibling));
                    } else {
                        gp_paned.set_end_child(Some(&sibling));
                    }
                } else if let Some(stack) = grandparent.downcast_ref::<gtk::Stack>() {
                    let page_name = format!("ws-{ws_id}");
                    stack.remove(paned);
                    stack.add_named(&sibling, Some(&page_name));
                    stack.set_visible_child_name(&page_name);
                    let mut s = state.borrow_mut();
                    if let Some(ws) = s.workspaces.iter_mut().find(|w| w.id == ws_id) {
                        ws.root = sibling.clone();
                    }
                }
            }
        }
    } else if parent.downcast_ref::<gtk::Stack>().is_some() {
        // This is the only pane in the workspace — close the workspace
        close_workspace_by_id(state, ws_id);
        return;
    }
    if persist {
        request_session_save(state);
    }
}

fn handle_split_with_tab(
    state: &State,
    ws_id: &str,
    source_pane: &gtk::Widget,
    target_pane: &gtk::Widget,
    orientation: gtk::Orientation,
    tab_id: &str,
    new_pane_first: bool,
) {
    let new_pane = split_pane(
        state,
        ws_id,
        target_pane,
        orientation,
        None,
        true,
        new_pane_first,
        false,
    );
    if pane::move_tab_to_pane(source_pane, tab_id, &new_pane) {
        request_session_save(state);
    }
}

/// Find the focused pane widget (a gtk::Box with class limux-pane-toolbar child)
/// by walking up from the currently focused widget.
fn find_leaf_focused_pane(state: &State) -> Option<(String, gtk::Widget)> {
    let (ws_id, root, stack) = {
        let s = state.borrow();
        let ws = s.active_workspace()?;
        (ws.id.clone(), ws.root.clone(), s.stack.clone())
    };

    // Get the window's focus widget and walk up to find a pane Box
    let window = stack.root()?.downcast::<gtk::Window>().ok()?;
    let focus = gtk::prelude::GtkWindowExt::focus(&window)?;

    let mut widget: Option<gtk::Widget> = Some(focus);
    while let Some(w) = widget {
        if let Some(bx) = w.downcast_ref::<gtk::Box>() {
            let mut child = bx.first_child();
            while let Some(c) = child {
                if c.has_css_class("limux-pane-header") {
                    return Some((ws_id, w));
                }
                child = c.next_sibling();
            }
        }
        widget = w.parent();
    }

    let _ = root;
    None
}

fn find_focused_pane(state: &State) -> Option<(String, gtk::Widget)> {
    if let Some(found) = find_leaf_focused_pane(state) {
        return Some(found);
    }

    let (ws_id, root) = {
        let s = state.borrow();
        let ws = s.active_workspace()?;
        (ws.id.clone(), ws.root.clone())
    };

    Some((ws_id, root))
}

fn focused_shortcut_target(state: &State) -> pane::FocusedShortcutTarget {
    let Some((_ws_id, pane_widget)) = find_leaf_focused_pane(state) else {
        return pane::FocusedShortcutTarget::None;
    };
    pane::focused_shortcut_target(&pane_widget)
}

fn show_runtime_error(state: &State, title: &str, detail: &str) {
    let window = state.borrow().window.clone();
    let dialog = gtk::AlertDialog::builder()
        .modal(true)
        .message(title)
        .detail(detail)
        .build();
    dialog.show(Some(&window));
}

fn quit_app(state: &State) {
    save_session_now(state);
    state.borrow().app.quit();
}

fn spawn_new_instance(state: &State) -> bool {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            let detail = format!("Failed to resolve the current Limux executable: {err}");
            eprintln!("limux: {detail}");
            show_runtime_error(state, "Failed to open a new Limux instance", &detail);
            return false;
        }
    };

    match std::process::Command::new(exe).spawn() {
        Ok(_) => true,
        Err(err) => {
            let detail = format!("Failed to launch a new Limux instance: {err}");
            eprintln!("limux: {detail}");
            show_runtime_error(state, "Failed to open a new Limux instance", &detail);
            false
        }
    }
}

fn dispatch_terminal_command(state: &State, command: ShortcutCommand) -> bool {
    let pane::FocusedShortcutTarget::Terminal(target) = focused_shortcut_target(state) else {
        return false;
    };

    match command {
        ShortcutCommand::SurfaceFind => target.show_find(),
        ShortcutCommand::SurfaceFindNext => target.find_next(),
        ShortcutCommand::SurfaceFindPrevious => target.find_previous(),
        ShortcutCommand::SurfaceFindHide => target.hide_find(),
        ShortcutCommand::SurfaceUseSelectionForFind => target.use_selection_for_find(),
        ShortcutCommand::TerminalClearScrollback => target.perform_binding_action("clear_screen"),
        ShortcutCommand::TerminalCopy => target.perform_binding_action("copy_to_clipboard"),
        ShortcutCommand::TerminalPaste => target.perform_binding_action("paste_from_clipboard"),
        ShortcutCommand::TerminalIncreaseFontSize => {
            target.perform_binding_action("increase_font_size:1")
        }
        ShortcutCommand::TerminalDecreaseFontSize => {
            target.perform_binding_action("decrease_font_size:1")
        }
        ShortcutCommand::TerminalResetFontSize => target.perform_binding_action("reset_font_size"),
        _ => false,
    }
}

fn dispatch_browser_command(state: &State, command: ShortcutCommand) -> bool {
    let pane::FocusedShortcutTarget::Browser(target) = focused_shortcut_target(state) else {
        return false;
    };

    match command {
        ShortcutCommand::BrowserFocusLocation => target.focus_location(),
        ShortcutCommand::BrowserBack => target.go_back(),
        ShortcutCommand::BrowserForward => target.go_forward(),
        ShortcutCommand::BrowserReload => target.reload(),
        ShortcutCommand::BrowserInspector => target.show_inspector(),
        ShortcutCommand::BrowserConsole => target.show_console(),
        ShortcutCommand::SurfaceFind => target.show_find(),
        ShortcutCommand::SurfaceFindNext => target.find_next(),
        ShortcutCommand::SurfaceFindPrevious => target.find_previous(),
        ShortcutCommand::SurfaceFindHide => target.hide_find(),
        ShortcutCommand::SurfaceUseSelectionForFind => target.use_selection_for_find(),
        ShortcutCommand::OpenBrowserInSplit => {
            let uri = target.current_uri();
            let Some((ws_id, pane_widget)) = find_leaf_focused_pane(state) else {
                return false;
            };
            let _ = split_pane(
                state,
                &ws_id,
                &pane_widget,
                gtk::Orientation::Horizontal,
                Some(PaneState::browser_only(uri.as_deref())),
                false,
                false,
                true,
            );
            true
        }
        _ => false,
    }
}

fn split_focused_pane(state: &State, orientation: gtk::Orientation) {
    if let Some((ws_id, pane_widget)) = find_focused_pane(state) {
        let _ = split_pane(
            state,
            &ws_id,
            &pane_widget,
            orientation,
            None,
            false,
            false,
            true,
        );
    }
}

fn cycle_focused_pane_tab(state: &State, delta: i32) {
    if let Some((_ws_id, pane_widget)) = find_focused_pane(state) {
        pane::cycle_tab_in_pane(&pane_widget, delta);
    }
}

fn close_focused_tab(state: &State) {
    if let Some((ws_id, pane_widget)) = find_focused_pane(state) {
        let parent = pane_widget.parent();
        // If this is the only pane (parent is Stack), don't close — keep workspace alive
        if let Some(ref p) = parent {
            if p.downcast_ref::<gtk::Stack>().is_some() {
                return;
            }
        }
        remove_pane(state, &ws_id, &pane_widget);
    }
}

fn add_tab_to_focused_pane(_state: &State, _browser: bool) {
    if let Some((_ws_id, pane_widget)) = find_focused_pane(_state) {
        if _browser {
            pane::add_browser_tab_to_pane(&pane_widget);
        } else {
            pane::add_terminal_tab_to_pane(&pane_widget);
        }
    }
}

/// Direction for pane navigation.
enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// Focus the neighboring pane in the given direction by walking the gtk::Paned tree.
fn focus_pane_in_direction(state: &State, direction: Direction) {
    let (_ws_id, pane_widget) = match find_focused_pane(state) {
        Some(v) => v,
        None => return,
    };

    // Determine which axis and sides we care about.
    let (target_orientation, must_be_start) = match direction {
        Direction::Left => (gtk::Orientation::Horizontal, false), // must be end_child to go left
        Direction::Right => (gtk::Orientation::Horizontal, true), // must be start_child to go right
        Direction::Up => (gtk::Orientation::Vertical, false),     // must be end_child to go up
        Direction::Down => (gtk::Orientation::Vertical, true),    // must be start_child to go down
    };

    // Walk up from the focused pane to find a gtk::Paned with the right
    // orientation where the current subtree is on the correct side.
    let mut current: gtk::Widget = pane_widget;
    loop {
        let parent = match current.parent() {
            Some(p) => p,
            None => return, // reached the top without finding a valid split
        };
        if let Some(paned) = parent.downcast_ref::<gtk::Paned>() {
            if paned.orientation() == target_orientation {
                let is_start = paned.start_child().map(|c| c == current).unwrap_or(false);
                if is_start == must_be_start {
                    // Found the split point. Navigate to the sibling subtree.
                    let sibling = if must_be_start {
                        paned.end_child()
                    } else {
                        paned.start_child()
                    };
                    if let Some(sibling) = sibling {
                        // Descend into the sibling to find the nearest leaf pane.
                        // "Nearest" means the edge closest to where we came from.
                        let prefer_start = !must_be_start;
                        let leaf = find_leaf_pane(&sibling, target_orientation, prefer_start);
                        // Find the GLArea inside the pane and focus it directly
                        if let Some(gl) = find_gl_area(&leaf) {
                            gl.grab_focus();
                        }
                    }
                    return;
                }
            }
        }
        current = parent;
    }
}

/// Recursively find the first visible GLArea inside a widget tree.
/// For gtk::Stack containers, only descend into the visible child.
fn find_gl_area(widget: &gtk::Widget) -> Option<gtk::GLArea> {
    if let Some(gl) = widget.downcast_ref::<gtk::GLArea>() {
        return Some(gl.clone());
    }
    // For Stack widgets, only search the visible child
    if let Some(stack) = widget.downcast_ref::<gtk::Stack>() {
        if let Some(visible) = stack.visible_child() {
            return find_gl_area(&visible);
        }
        return None;
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        if let Some(gl) = find_gl_area(&c) {
            return Some(gl);
        }
        child = c.next_sibling();
    }
    None
}

/// Descend a pane/split subtree to find a leaf pane widget.
/// When encountering a gtk::Paned matching `axis`, prefer `start_child` if
/// `prefer_start` is true (to find the nearest edge). For Paned widgets on
/// the other axis, prefer start_child (arbitrary but consistent).
fn find_leaf_pane(widget: &gtk::Widget, axis: gtk::Orientation, prefer_start: bool) -> gtk::Widget {
    if let Some(paned) = widget.downcast_ref::<gtk::Paned>() {
        let pick_start = if paned.orientation() == axis {
            prefer_start
        } else {
            true // arbitrary default for orthogonal splits
        };
        let child = if pick_start {
            paned.start_child()
        } else {
            paned.end_child()
        };
        match child {
            Some(c) => find_leaf_pane(&c, axis, prefer_start),
            None => widget.clone(),
        }
    } else {
        // Leaf pane — this is a pane gtk::Box
        widget.clone()
    }
}

fn mark_workspace_unread(state: &State, ws_id: &str) {
    let mut s = state.borrow_mut();
    let active_idx = s.active_idx;
    if let Some((idx, ws)) = s
        .workspaces
        .iter_mut()
        .enumerate()
        .find(|(_, w)| w.id == ws_id)
    {
        if idx != active_idx {
            ws.unread = true;
            ws.notify_dot.remove_css_class("limux-notify-dot-hidden");
            ws.notify_dot.add_css_class("limux-notify-dot");
            ws.notify_label.set_label("Process needs attention");
            ws.notify_label.remove_css_class("limux-notify-msg");
            ws.notify_label.add_css_class("limux-notify-msg-unread");
            ws.notify_label.set_visible(true);
            // Add glow pulse to the sidebar row box
            if let Some(row_box) = ws.sidebar_row.child() {
                row_box.add_css_class("limux-sidebar-row-unread");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::glib;
    use super::gtk::gdk;
    use super::{
        clamp_workspace_insert_index_for_pinning, favorites_prefix_len,
        shortcut_allowed_while_browser_find_active, shortcut_blocked_by_editable,
        shortcut_command_from_key_event, shortcut_dispatch_propagation, tab_drag_workspace_seed,
        workspace_drop_layout_path, EditableCaptureContext, WorkspaceSeedSource,
    };
    use crate::layout_state::{LayoutNodeState, PaneState, SplitOrientation, SplitState};
    use crate::shortcut_config::{
        default_shortcuts, resolve_shortcuts_from_str, EditableCapturePolicy, ShortcutCommand,
    };

    #[test]
    fn favorites_prefix_len_counts_only_leading_favorites() {
        let flags = [true, true, false, true, false];
        assert_eq!(favorites_prefix_len(&flags), 2);
    }

    #[test]
    fn unpinned_workspace_cannot_move_above_favorites() {
        // Remaining order after removing dragged workspace:
        // [fav, fav, unfav, unfav]
        let after_removal = [true, true, false, false];
        let clamped = clamp_workspace_insert_index_for_pinning(&after_removal, false, 0);
        assert_eq!(clamped, 2);
    }

    #[test]
    fn favorite_workspace_cannot_move_below_unpinned() {
        // Remaining order after removing dragged favorite:
        // [fav, fav, unfav, unfav]
        let after_removal = [true, true, false, false];
        let clamped =
            clamp_workspace_insert_index_for_pinning(&after_removal, true, after_removal.len());
        assert_eq!(clamped, 2);
    }

    #[test]
    fn shortcut_command_from_key_event_uses_default_registry_bindings() {
        let shortcuts = default_shortcuts();

        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::T,
                gdk::ModifierType::CONTROL_MASK
            ),
            Some(ShortcutCommand::NewTerminal)
        );
        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::Page_Down,
                gdk::ModifierType::CONTROL_MASK
            ),
            Some(ShortcutCommand::NextWorkspace)
        );
        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::F,
                gdk::ModifierType::CONTROL_MASK
            ),
            Some(ShortcutCommand::SurfaceFind)
        );
        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::Q,
                gdk::ModifierType::CONTROL_MASK
            ),
            Some(ShortcutCommand::QuitApp)
        );
        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::N,
                gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK
            ),
            Some(ShortcutCommand::NewInstance)
        );
        assert_eq!(
            shortcut_command_from_key_event(&shortcuts, gdk::Key::F11, gdk::ModifierType::empty()),
            Some(ShortcutCommand::ToggleFullscreen)
        );
        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::M,
                gdk::ModifierType::CONTROL_MASK
            ),
            Some(ShortcutCommand::ToggleSidebar)
        );
        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::M,
                gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK
            ),
            Some(ShortcutCommand::ToggleTopBar)
        );
    }

    #[test]
    fn shortcut_command_from_key_event_honors_remaps_and_disables_old_binding() {
        let shortcuts = resolve_shortcuts_from_str(
            r#"{
                "shortcuts": {
                    "toggle_sidebar": "<Ctrl><Alt>b"
                }
            }"#,
        )
        .unwrap();

        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::M,
                gdk::ModifierType::CONTROL_MASK
            ),
            None
        );
        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::B,
                gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK
            ),
            Some(ShortcutCommand::ToggleSidebar)
        );
    }

    #[test]
    fn shortcut_command_from_key_event_respects_explicit_unbinds() {
        let shortcuts = resolve_shortcuts_from_str(
            r#"{
                "shortcuts": {
                    "toggle_sidebar": null
                }
            }"#,
        )
        .unwrap();

        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::M,
                gdk::ModifierType::CONTROL_MASK
            ),
            None
        );
    }

    #[test]
    fn shortcut_command_from_key_event_honors_super_remaps() {
        let shortcuts = resolve_shortcuts_from_str(
            r#"{
                "shortcuts": {
                    "toggle_sidebar": "<Super>b"
                }
            }"#,
        )
        .unwrap();

        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::M,
                gdk::ModifierType::CONTROL_MASK
            ),
            None
        );
        assert_eq!(
            shortcut_command_from_key_event(&shortcuts, gdk::Key::B, gdk::ModifierType::SUPER_MASK),
            Some(ShortcutCommand::ToggleSidebar)
        );
    }

    #[test]
    fn shortcut_dispatch_propagation_stops_only_when_window_claims_shortcut() {
        assert_eq!(shortcut_dispatch_propagation(true), glib::Propagation::Stop);
        assert_eq!(
            shortcut_dispatch_propagation(false),
            glib::Propagation::Proceed
        );
    }

    #[test]
    fn shortcut_blocked_by_editable_only_bypasses_non_global_shortcuts() {
        assert!(shortcut_blocked_by_editable(
            ShortcutCommand::SurfaceFind,
            EditableCapturePolicy::BypassInEditable,
            EditableCaptureContext {
                gtk_editable: true,
                ..EditableCaptureContext::default()
            }
        ));
        assert!(!shortcut_blocked_by_editable(
            ShortcutCommand::SurfaceFind,
            EditableCapturePolicy::AlwaysCapture,
            EditableCaptureContext {
                gtk_editable: true,
                ..EditableCaptureContext::default()
            }
        ));
        assert!(!shortcut_blocked_by_editable(
            ShortcutCommand::SurfaceFind,
            EditableCapturePolicy::BypassInEditable,
            EditableCaptureContext::default()
        ));
    }

    #[test]
    fn shortcut_blocked_by_editable_blocks_dom_editable_browser_content() {
        assert!(shortcut_blocked_by_editable(
            ShortcutCommand::BrowserReload,
            EditableCapturePolicy::BypassInEditable,
            EditableCaptureContext {
                browser_dom_editable: true,
                ..EditableCaptureContext::default()
            }
        ));
    }

    #[test]
    fn browser_find_navigation_shortcuts_are_allowed_while_find_ui_is_active() {
        let context = EditableCaptureContext {
            gtk_editable: true,
            browser_find_active: true,
            ..EditableCaptureContext::default()
        };

        assert!(!shortcut_blocked_by_editable(
            ShortcutCommand::SurfaceFindNext,
            EditableCapturePolicy::BypassInEditable,
            context
        ));
        assert!(!shortcut_blocked_by_editable(
            ShortcutCommand::SurfaceFindPrevious,
            EditableCapturePolicy::BypassInEditable,
            context
        ));
        assert!(!shortcut_blocked_by_editable(
            ShortcutCommand::SurfaceFindHide,
            EditableCapturePolicy::BypassInEditable,
            context
        ));
        assert!(shortcut_blocked_by_editable(
            ShortcutCommand::SurfaceFind,
            EditableCapturePolicy::BypassInEditable,
            context
        ));
    }

    #[test]
    fn browser_find_active_exception_is_limited_to_navigation_shortcuts() {
        assert!(shortcut_allowed_while_browser_find_active(
            ShortcutCommand::SurfaceFindNext
        ));
        assert!(shortcut_allowed_while_browser_find_active(
            ShortcutCommand::SurfaceFindPrevious
        ));
        assert!(shortcut_allowed_while_browser_find_active(
            ShortcutCommand::SurfaceFindHide
        ));
        assert!(!shortcut_allowed_while_browser_find_active(
            ShortcutCommand::SurfaceFind
        ));
    }

    #[test]
    fn workspace_drop_layout_path_prefers_deterministic_startmost_leaf() {
        let layout = LayoutNodeState::Split(SplitState {
            orientation: SplitOrientation::Horizontal,
            ratio: 0.5,
            start: Box::new(LayoutNodeState::Split(SplitState {
                orientation: SplitOrientation::Vertical,
                ratio: 0.5,
                start: Box::new(LayoutNodeState::Pane(PaneState::fallback(Some("/a")))),
                end: Box::new(LayoutNodeState::Pane(PaneState::fallback(Some("/b")))),
            })),
            end: Box::new(LayoutNodeState::Pane(PaneState::fallback(Some("/c")))),
        });

        assert_eq!(workspace_drop_layout_path(&layout), vec![true, true]);
    }

    #[test]
    fn tab_drag_workspace_seed_uses_terminal_cwd_for_folder_path() {
        let seed = tab_drag_workspace_seed(
            WorkspaceSeedSource {
                workspace_cwd: Some("/workspace".to_string()),
                workspace_folder_path: Some("/workspace".to_string()),
            },
            "Project Shell",
            Some("/project".to_string()),
        );

        assert_eq!(seed.name, "Project Shell");
        assert_eq!(seed.cwd.as_deref(), Some("/project"));
        assert_eq!(seed.folder_path.as_deref(), Some("/project"));
    }

    #[test]
    fn tab_drag_workspace_seed_uses_workspace_directory_for_non_terminal_tab() {
        let seed = tab_drag_workspace_seed(
            WorkspaceSeedSource {
                workspace_cwd: Some("/workspace-cwd".to_string()),
                workspace_folder_path: Some("/workspace-folder".to_string()),
            },
            "Browser",
            None,
        );

        assert_eq!(seed.name, "Browser");
        assert_eq!(seed.cwd.as_deref(), Some("/workspace-folder"));
        assert_eq!(seed.folder_path.as_deref(), Some("/workspace-folder"));
    }
}
