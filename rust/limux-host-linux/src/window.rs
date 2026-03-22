use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use gtk4 as gtk;
use libadwaita as adw;

use crate::pane::{self, PaneCallbacks};

// ---------------------------------------------------------------------------
// Workspace persistence
// ---------------------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct SavedWorkspace {
    name: String,
    favorite: bool,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    folder_path: Option<String>,
}

fn persistence_path() -> std::path::PathBuf {
    let dir = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("~/.local/share"))
        .join("limux");
    std::fs::create_dir_all(&dir).ok();
    dir.join("workspaces.json")
}

fn load_workspaces() -> Vec<SavedWorkspace> {
    let path = persistence_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_workspaces(state: &State) {
    let s = state.borrow();
    let saved: Vec<SavedWorkspace> = s
        .workspaces
        .iter()
        .map(|ws| SavedWorkspace {
            name: ws.name.clone(),
            favorite: ws.favorite,
            cwd: ws.cwd.borrow().clone(),
            folder_path: ws.folder_path.clone(),
        })
        .collect();
    if let Ok(json) = serde_json::to_string_pretty(&saved) {
        let path = persistence_path();
        std::fs::write(&path, json).ok();
    }
}

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
    workspaces: Vec<Workspace>,
    active_idx: usize,
    stack: gtk::Stack,
    sidebar_list: gtk::ListBox,
    paned: gtk::Paned,
    new_ws_btn: gtk::Button,
    expand_btn: gtk::Button,
    sidebar_animation: Option<adw::TimedAnimation>,
    sidebar_animation_epoch: u64,
    sidebar_expanded_width: i32,
}

impl AppState {
    fn active_workspace(&self) -> Option<&Workspace> {
        self.workspaces.get(self.active_idx)
    }
}

type State = Rc<RefCell<AppState>>;

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
    border-top: 2px solid #0091FF;
    border-top-left-radius: 0;
    border-top-right-radius: 0;
    padding-top: 4px;
}
.limux-drop-below .limux-sidebar-row-box {
    border-bottom: 2px solid #0091FF;
    border-bottom-left-radius: 0;
    border-bottom-right-radius: 0;
    padding-bottom: 4px;
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
.limux-ws-path {
    color: rgba(255, 255, 255, 0.3);
    font-size: 12px;
}
row:selected .limux-ws-path {
    color: rgba(255, 255, 255, 0.5);
}
.limux-sidebar-collapse {
    color: rgba(255, 255, 255, 0.4);
    border: none;
    min-height: 0;
    min-width: 0;
    padding: 0 6px;
    font-size: 14px;
}
.limux-sidebar-collapse:hover {
    color: rgba(255, 255, 255, 0.9);
}
.limux-sidebar-expand {
    background-color: rgba(25, 25, 25, 1);
    color: rgba(255, 255, 255, 0.5);
    border: none;
    border-top-right-radius: 6px;
    border-bottom-right-radius: 6px;
    min-width: 0;
    padding: 8px 4px;
    font-size: 13px;
}
.limux-sidebar-expand:hover {
    background-color: rgba(40, 40, 40, 1);
    color: white;
}
.limux-content {
    background-color: rgba(23, 23, 23, 1);
}
"#;

// ---------------------------------------------------------------------------
// Window construction
// ---------------------------------------------------------------------------

pub fn build_window(app: &adw::Application) {
    // Load CSS
    let provider = gtk::CssProvider::new();
    let all_css = format!("{CSS}\n{}", pane::PANE_CSS);
    provider.load_from_data(&all_css);
    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("display"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);

    // Register custom icons — look for icons dir relative to the executable
    let icon_theme = gtk::IconTheme::for_display(&gtk::gdk::Display::default().expect("display"));
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    // Try several possible icon locations
    for candidate in &[
        exe_dir
            .as_ref()
            .map(|d| d.join("../../rust/limux-host-linux/icons")),
        exe_dir.as_ref().map(|d| d.join("../icons")),
        Some(std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/icons"
        ))),
    ] {
        if let Some(path) = candidate {
            if path.exists() {
                icon_theme.add_search_path(path);
            }
        }
    }

    let title = format!("Limux v{}", crate::VERSION);
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(&title)
        .default_width(1400)
        .default_height(900)
        .build();

    // GTK4/libadwaita always uses CSD and ignores the xdg-decoration negotiation.
    // Skip the header bar when the display provides its own window decorations:
    // - X11: WMs always provide SSD (downcast to WaylandDisplay fails → default to true).
    // - Wayland: if the compositor advertises xdg-decoration.
    let provides_decorations = gtk::gdk::Display::default()
        .and_then(|d| d.downcast::<gdk4_wayland::WaylandDisplay>().ok())
        .map(|d| d.query_registry("zxdg_decoration_manager_v1"))
        // Not a Wayland display (X11) → default to showing header bar
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

    let collapse_btn = gtk::Button::with_label("\u{00AB}"); // «
    collapse_btn.add_css_class("flat");
    collapse_btn.add_css_class("limux-sidebar-collapse");
    collapse_btn.set_tooltip_text(Some("Hide sidebar (Ctrl+B)"));

    let sidebar_title = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .margin_top(8)
        .margin_bottom(4)
        .margin_end(6)
        .build();
    sidebar_title.append(&sidebar_title_label);
    sidebar_title.append(&collapse_btn);

    let new_ws_btn = gtk::Button::builder()
        .label("New Workspace")
        .hexpand(true)
        .margin_start(6)
        .margin_end(6)
        .margin_bottom(6)
        .build();
    new_ws_btn.add_css_class("limux-sidebar-btn");

    // Drop target on the button — intensifies when dragging over it
    let btn_drop = gtk::DropTarget::new(glib::Type::STRING, gtk::gdk::DragAction::MOVE);
    btn_drop.set_preload(true);
    {
        let btn = new_ws_btn.clone();
        btn_drop.connect_motion(move |_, _, _| {
            btn.add_css_class("limux-sidebar-btn-trash-hover");
            gtk::gdk::DragAction::MOVE
        });
    }
    {
        let btn = new_ws_btn.clone();
        btn_drop.connect_leave(move |_| {
            btn.remove_css_class("limux-sidebar-btn-trash-hover");
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

    // Expand tab — small button on the left edge when sidebar is hidden
    let expand_btn = gtk::Button::with_label("\u{00BB}"); // »
    expand_btn.add_css_class("limux-sidebar-expand");
    expand_btn.set_tooltip_text(Some("Show sidebar (Ctrl+B)"));
    expand_btn.set_valign(gtk::Align::Center);
    expand_btn.set_halign(gtk::Align::Start);
    expand_btn.set_visible(false);

    let content_overlay = gtk::Overlay::new();
    content_overlay.set_child(Some(&main_paned));
    content_overlay.add_overlay(&expand_btn);

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    if let Some(ref header) = header {
        vbox.append(header);
    }
    vbox.append(&content_overlay);
    window.set_content(Some(&vbox));

    let state: State = Rc::new(RefCell::new(AppState {
        workspaces: Vec::new(),
        active_idx: 0,
        stack: stack.clone(),
        sidebar_list: sidebar_list.clone(),
        paned: main_paned.clone(),
        new_ws_btn: new_ws_btn.clone(),
        expand_btn: expand_btn.clone(),
        sidebar_animation: None,
        sidebar_animation_epoch: 0,
        sidebar_expanded_width: SIDEBAR_WIDTH,
    }));

    {
        let state = state.clone();
        main_paned.connect_position_notify(move |paned| {
            let position = paned.position();
            if position > 10 {
                state.borrow_mut().sidebar_expanded_width = position;
            }
        });
    }

    // Wire collapse button
    {
        let state = state.clone();
        collapse_btn.connect_clicked(move |_| {
            toggle_sidebar(&state);
        });
    }

    // Wire expand button
    {
        let state = state.clone();
        expand_btn.connect_clicked(move |_| {
            toggle_sidebar(&state);
        });
    }

    register_actions(&window, &state);
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

    // Wire up drop-to-delete handler on the New Workspace button
    {
        let state = state.clone();
        let btn = new_ws_btn.clone();
        btn_drop.connect_drop(move |_, value, _, _| {
            btn.set_label("New Workspace");
            btn.remove_css_class("limux-sidebar-btn-trash");
            btn.remove_css_class("limux-sidebar-btn-trash-hover");
            if let Ok(workspace_id) = value.get::<String>() {
                close_workspace_by_id(&state, &workspace_id);
                save_workspaces(&state);
                return true;
            }
            false
        });
    }

    // Save workspaces (including latest CWD) on window close
    {
        let state = state.clone();
        window.connect_close_request(move |_| {
            save_workspaces(&state);
            glib::Propagation::Proceed
        });
    }

    // Restore saved workspaces (if any). Empty start is fine — user clicks "New Workspace".
    let saved = load_workspaces();
    for sw in &saved {
        add_workspace_with_name(
            &state,
            &sw.name,
            sw.favorite,
            sw.cwd.as_deref(),
            sw.folder_path.as_deref(),
        );
    }
    window.present();
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

fn register_actions(window: &adw::ApplicationWindow, state: &State) {
    let action_defs: &[&str] = &[
        "new-workspace",
        "close-workspace",
        "toggle-sidebar",
        "next-workspace",
        "prev-workspace",
    ];

    for name in action_defs {
        let action = gtk::gio::SimpleAction::new(name, None);
        let state = state.clone();
        let handler_name = name.to_string();
        action.connect_activate(move |_, _| match handler_name.as_str() {
            "new-workspace" => add_workspace(&state, None),
            "close-workspace" => close_workspace(&state),
            "toggle-sidebar" => toggle_sidebar(&state),
            "next-workspace" => cycle_workspace(&state, 1),
            "prev-workspace" => cycle_workspace(&state, -1),
            _ => {}
        });
        window.add_action(&action);
    }
}

/// Intercept keyboard shortcuts in the CAPTURE phase for window-level bindings.
fn install_key_capture(window: &adw::ApplicationWindow, state: &State) {
    use gtk::gdk;

    let key_controller = gtk::EventControllerKey::new();
    key_controller.set_propagation_phase(gtk::PropagationPhase::Capture);

    let state = state.clone();
    key_controller.connect_key_pressed(move |_, keyval, _keycode, modifier| {
        let ctrl = modifier.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = modifier.contains(gdk::ModifierType::SHIFT_MASK);

        let matched = match (ctrl, shift, keyval) {
            // Ctrl+Shift+N → new workspace
            (true, true, gdk::Key::N | gdk::Key::n) => {
                add_workspace(&state, None);
                true
            }
            // Ctrl+Shift+W → close workspace
            (true, true, gdk::Key::W | gdk::Key::w) => {
                close_workspace(&state);
                true
            }
            // Ctrl+Shift+Left → prev tab
            (true, true, gdk::Key::Left) => {
                cycle_focused_pane_tab(&state, -1);
                true
            }
            // Ctrl+Shift+Right → next tab
            (true, true, gdk::Key::Right) => {
                cycle_focused_pane_tab(&state, 1);
                true
            }
            // Ctrl+Shift+D → split down
            (true, true, gdk::Key::D | gdk::Key::d) => {
                split_focused_pane(&state, gtk::Orientation::Vertical);
                true
            }
            // Ctrl+Shift+T → new terminal tab in focused pane
            (true, true, gdk::Key::T | gdk::Key::t) => {
                add_tab_to_focused_pane(&state, false);
                true
            }
            // Ctrl+D → split right
            (true, false, gdk::Key::d) => {
                split_focused_pane(&state, gtk::Orientation::Horizontal);
                true
            }
            // Ctrl+W → close focused tab/pane
            (true, false, gdk::Key::w) => {
                close_focused_tab(&state);
                true
            }
            // Ctrl+B → toggle sidebar
            (true, false, gdk::Key::b) => {
                toggle_sidebar(&state);
                true
            }
            // Ctrl+T → new terminal tab
            (true, false, gdk::Key::t) => {
                add_tab_to_focused_pane(&state, false);
                true
            }
            // Ctrl+PageDown → next workspace
            (true, false, gdk::Key::Page_Down) => {
                cycle_workspace(&state, 1);
                true
            }
            // Ctrl+PageUp → prev workspace
            (true, false, gdk::Key::Page_Up) => {
                cycle_workspace(&state, -1);
                true
            }
            // Ctrl+Arrow → focus pane in direction
            (true, false, gdk::Key::Left) => {
                focus_pane_in_direction(&state, Direction::Left);
                true
            }
            (true, false, gdk::Key::Right) => {
                focus_pane_in_direction(&state, Direction::Right);
                true
            }
            (true, false, gdk::Key::Up) => {
                focus_pane_in_direction(&state, Direction::Up);
                true
            }
            (true, false, gdk::Key::Down) => {
                focus_pane_in_direction(&state, Direction::Down);
                true
            }
            // Ctrl+1-9 → switch to workspace by index
            (true, false, key) => {
                let digit = match key {
                    gdk::Key::_1 => Some(0usize),
                    gdk::Key::_2 => Some(1),
                    gdk::Key::_3 => Some(2),
                    gdk::Key::_4 => Some(3),
                    gdk::Key::_5 => Some(4),
                    gdk::Key::_6 => Some(5),
                    gdk::Key::_7 => Some(6),
                    gdk::Key::_8 => Some(7),
                    gdk::Key::_9 => {
                        // Ctrl+9 always goes to last workspace
                        let s = state.borrow();
                        if s.workspaces.is_empty() {
                            None
                        } else {
                            Some(s.workspaces.len() - 1)
                        }
                    }
                    _ => None,
                };
                if let Some(idx) = digit {
                    let row_and_list = {
                        let s = state.borrow();
                        s.workspaces
                            .get(idx)
                            .map(|ws| (ws.sidebar_row.clone(), s.sidebar_list.clone()))
                    };
                    switch_workspace(&state, idx);
                    if let Some((row, list)) = row_and_list {
                        list.select_row(Some(&row));
                    }
                    true
                } else {
                    false
                }
            }
            _ => false,
        };

        if matched {
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });

    window.add_controller(key_controller);
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
            save_workspaces(&state);
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
                save_workspaces(&state_for_commit);
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
    save_workspaces(state);

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
    save_workspaces(state);
}

fn install_workspace_row_interactions(
    state: &State,
    workspace_id: &str,
    row: &gtk::ListBoxRow,
    favorite_button: &gtk::Button,
) {
    // Right click shows context menu with Rename / Delete.
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

    // Drag source for sidebar reordering.
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
        drag_source.connect_drag_begin(move |_, _| {
            let s = state.borrow();
            s.new_ws_btn.set_label("\u{1F5D1}\u{FE0E}");
            s.new_ws_btn.add_css_class("limux-sidebar-btn-trash");
        });
    }
    {
        let state = state.clone();
        drag_source.connect_drag_end(move |_, _, _| {
            let s = state.borrow();
            s.new_ws_btn.set_label("New Workspace");
            s.new_ws_btn.remove_css_class("limux-sidebar-btn-trash");
            s.new_ws_btn
                .remove_css_class("limux-sidebar-btn-trash-hover");
        });
    }
    row.add_controller(drag_source);

    // Drop target for sidebar reordering with visual feedback.
    let drop_target = gtk::DropTarget::new(glib::Type::STRING, gtk::gdk::DragAction::MOVE);
    drop_target.set_preload(true);
    {
        let r = row.clone();
        drop_target.connect_motion(move |_, _x, y| {
            let h = r.height() as f64;
            r.remove_css_class("limux-drop-above");
            r.remove_css_class("limux-drop-below");
            if y < h / 2.0 {
                r.add_css_class("limux-drop-above");
            } else {
                r.add_css_class("limux-drop-below");
            }
            gtk::gdk::DragAction::MOVE
        });
    }
    {
        let r = row.clone();
        drop_target.connect_leave(move |_| {
            r.remove_css_class("limux-drop-above");
            r.remove_css_class("limux-drop-below");
        });
    }
    {
        let state = state.clone();
        let target_workspace_id = workspace_id.to_string();
        let r = row.clone();
        drop_target.connect_drop(move |_dt, value, _, y| {
            r.remove_css_class("limux-drop-above");
            r.remove_css_class("limux-drop-below");
            let drop_below = y >= r.height() as f64 / 2.0;
            if let Ok(source_workspace_id) = value.get::<String>() {
                if source_workspace_id != target_workspace_id {
                    return reorder_workspace_by_id(
                        &state,
                        &source_workspace_id,
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
    let mut s = state.borrow_mut();
    let id = uuid::Uuid::new_v4().to_string();
    let stack_name = format!("ws-{id}");

    let pane_widget = create_pane_for_workspace(state, &id, Some(folder_path));
    let root: gtk::Widget = pane_widget.upcast();

    s.stack.add_named(&root, Some(&stack_name));

    let (row, name_label, favorite_button, notify_dot, notify_label, path_label) =
        build_sidebar_row(name, Some(folder_path));
    s.sidebar_list.append(&row);
    install_workspace_row_interactions(state, &id, &row, &favorite_button);

    let cwd: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(Some(folder_path.to_string())));
    let ws = Workspace {
        id,
        name: name.to_string(),
        root,
        sidebar_row: row.clone(),
        name_label,
        favorite_button,
        notify_dot,
        notify_label,
        unread: false,
        favorite: false,
        cwd,
        folder_path: Some(folder_path.to_string()),
        path_label,
    };

    s.workspaces.push(ws);
    let new_idx = s.workspaces.len() - 1;
    s.active_idx = new_idx;
    s.stack.set_visible_child_name(&stack_name);

    let sidebar_list = s.sidebar_list.clone();
    drop(s);

    sidebar_list.select_row(Some(&row));
    save_workspaces(state);
}

fn add_workspace_with_name(
    state: &State,
    name: &str,
    favorite: bool,
    saved_cwd: Option<&str>,
    folder_path: Option<&str>,
) {
    let mut s = state.borrow_mut();
    let id = uuid::Uuid::new_v4().to_string();
    let stack_name = format!("ws-{id}");

    // Use folder_path for terminal cwd, fall back to saved_cwd
    let working_dir = folder_path.or(saved_cwd);
    let pane_widget = create_pane_for_workspace(state, &id, working_dir);
    let root: gtk::Widget = pane_widget.upcast();

    s.stack.add_named(&root, Some(&stack_name));

    let (row, name_label, favorite_button, notify_dot, notify_label, path_label) =
        build_sidebar_row(name, folder_path);
    s.sidebar_list.append(&row);
    install_workspace_row_interactions(state, &id, &row, &favorite_button);

    let cwd: Rc<RefCell<Option<String>>> =
        Rc::new(RefCell::new(working_dir.map(|s| s.to_string())));
    let ws = Workspace {
        id,
        name: name.to_string(),
        root,
        sidebar_row: row.clone(),
        name_label,
        favorite_button,
        notify_dot,
        notify_label,
        unread: false,
        favorite,
        cwd,
        folder_path: folder_path.map(|s| s.to_string()),
        path_label,
    };

    if favorite {
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
    ws_id: &str,
    working_directory: Option<&str>,
) -> gtk::Box {
    let state_for_split = state.clone();
    let state_for_close = state.clone();
    let state_for_bell = state.clone();
    let state_for_pwd = state.clone();
    let state_for_empty = state.clone();
    let ws_id_split = ws_id.to_string();
    let ws_id_close = ws_id.to_string();
    let ws_id_bell = ws_id.to_string();
    let ws_id_pwd = ws_id.to_string();
    let ws_id_empty = ws_id.to_string();

    let callbacks = Rc::new(PaneCallbacks {
        on_split: Box::new(move |pane_widget, orientation| {
            split_pane(&state_for_split, &ws_id_split, pane_widget, orientation);
        }),
        on_close_pane: Box::new(move |pane_widget| {
            remove_pane(&state_for_close, &ws_id_close, pane_widget);
        }),
        on_bell: Box::new(move || {
            // Defer to avoid RefCell borrow conflicts — bell can fire during state mutation
            let state = state_for_bell.clone();
            let ws_id = ws_id_bell.clone();
            glib::idle_add_local_once(move || {
                mark_workspace_unread(&state, &ws_id);
            });
        }),
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
        on_empty: Box::new(move |pane_widget| {
            remove_pane(&state_for_empty, &ws_id_empty, pane_widget);
        }),
    });

    pane::create_pane(callbacks, working_directory)
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
        save_workspaces(state);
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
    save_workspaces(state);
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

fn toggle_sidebar(state: &State) {
    let (paned, expand_btn, sidebar, current, is_visible, target_width, prior_animation, epoch) = {
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
            s.expand_btn.clone(),
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
        // Collapse: animate position to 0, then hide sidebar, show expand button
        expand_btn.set_visible(true);
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
        let expand_btn_for_done = expand_btn.clone();
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
                expand_btn_for_done.set_visible(true);
            }
        });
        state.borrow_mut().sidebar_animation = Some(animation.clone());
        animation.play();
    } else {
        // Expand: make sidebar visible, then animate position from 0 to remembered width
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
        let expand_btn_for_done = expand_btn.clone();
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
                expand_btn_for_done.set_visible(false);
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
) {
    // Use the workspace's folder_path (or current cwd) for the new pane
    let wd = {
        let s = state.borrow();
        s.workspaces
            .iter()
            .find(|w| w.id == ws_id)
            .and_then(|ws| {
                ws.folder_path
                    .clone()
                    .or_else(|| ws.cwd.borrow().clone())
            })
    };
    let new_pane = create_pane_for_workspace(state, ws_id, wd.as_deref());

    let parent = pane_widget.parent();

    let new_paned = gtk::Paned::builder()
        .orientation(orientation)
        .hexpand(true)
        .vexpand(true)
        .build();

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

    new_paned.set_start_child(Some(pane_widget));
    new_paned.set_end_child(Some(&new_pane));

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
}

fn remove_pane(state: &State, ws_id: &str, pane_widget: &gtk::Widget) {
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
    }
}

/// Find the focused pane widget (a gtk::Box with class limux-pane-toolbar child)
/// by walking up from the currently focused widget.
fn find_focused_pane(state: &State) -> Option<(String, gtk::Widget)> {
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

    Some((ws_id, root))
}

fn split_focused_pane(state: &State, orientation: gtk::Orientation) {
    if let Some((ws_id, pane_widget)) = find_focused_pane(state) {
        split_pane(state, &ws_id, &pane_widget, orientation);
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
    use super::{clamp_workspace_insert_index_for_pinning, favorites_prefix_len};

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
}
