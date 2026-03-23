//! PaneWidget: a tabbed container with action icons in the tab bar.
//!
//! Layout: [tab1 x] [tab2 x] ... ←spacer→ [terminal] [browser] [split-h] [split-v] [close]
//!
//! All on one line. Tabs left-justified, icons right-justified.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};

use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;

use crate::layout_state::{PaneState, TabContentState, TabState as SavedTabState};
use crate::terminal::{self, TerminalCallbacks};

// ---------------------------------------------------------------------------
// Global pane registry (for cross-pane tab DnD)
// ---------------------------------------------------------------------------

fn next_pane_id() -> u32 {
    static COUNTER: AtomicU32 = AtomicU32::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

thread_local! {
    static PANE_REGISTRY: RefCell<std::collections::HashMap<u32, std::rc::Weak<PaneInternals>>>
        = RefCell::new(std::collections::HashMap::new());
}

fn register_pane(id: u32, internals: &Rc<PaneInternals>) {
    PANE_REGISTRY.with(|reg| {
        reg.borrow_mut().insert(id, Rc::downgrade(internals));
    });
}

fn unregister_pane(id: u32) {
    PANE_REGISTRY.with(|reg| {
        reg.borrow_mut().remove(&id);
    });
}

fn lookup_pane_internals(id: u32) -> Option<Rc<PaneInternals>> {
    PANE_REGISTRY.with(|reg| reg.borrow().get(&id)?.upgrade())
}

/// Find a pane widget by its pane_id. Returns the pane's outer Box widget.
pub fn find_pane_widget_by_id(pane_id: u32) -> Option<gtk::Widget> {
    lookup_pane_internals(pane_id).map(|i| i.pane_outer.clone().upcast())
}

/// Set the workspace-dragging flag on all registered panes.
pub fn set_workspace_dragging_all(active: bool) {
    PANE_REGISTRY.with(|reg| {
        for weak in reg.borrow().values() {
            if let Some(internals) = weak.upgrade() {
                internals.workspace_dragging.set(active);
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

type PaneSplitCallback = dyn Fn(&gtk::Widget, gtk::Orientation);
type PaneWidgetCallback = dyn Fn(&gtk::Widget);
type PaneSignalCallback = dyn Fn();
type PanePathCallback = dyn Fn(&str);

pub struct PaneCallbacks {
    pub on_split: Box<PaneSplitCallback>,
    pub on_close_pane: Box<PaneWidgetCallback>,
    pub on_bell: Box<PaneSignalCallback>,
    pub on_pwd_changed: Box<PanePathCallback>,
    pub on_empty: Box<PaneWidgetCallback>,
    pub on_state_changed: Box<PaneSignalCallback>,
}

#[derive(Clone)]
struct TabContextMenuContext {
    tab_strip: gtk::Box,
    content_stack: gtk::Stack,
    tab_state: Rc<RefCell<TabState>>,
    callbacks: Rc<PaneCallbacks>,
    pane_outer: gtk::Box,
    label: gtk::Label,
    pin_icon: gtk::Label,
}

// ---------------------------------------------------------------------------
// CSS
// ---------------------------------------------------------------------------

pub const PANE_CSS: &str = r#"
.limux-pane-header {
    background-color: rgba(30, 30, 30, 1);
    border-bottom: 1px solid rgba(255, 255, 255, 0.06);
    min-height: 30px;
    padding: 0 2px;
}
.limux-tab {
    background: none;
    border: none;
    border-radius: 4px 4px 0 0;
    padding: 4px 4px 4px 10px;
    color: rgba(255, 255, 255, 0.45);
    min-height: 0;
    font-size: 12px;
}
.limux-tab:hover {
    color: rgba(255, 255, 255, 0.7);
    background: rgba(255, 255, 255, 0.04);
}
.limux-tab-active {
    color: white;
    background: rgba(255, 255, 255, 0.08);
}
.limux-tab-close {
    background: none;
    border: none;
    border-radius: 3px;
    padding: 1px;
    min-height: 0;
    min-width: 0;
    color: rgba(255, 255, 255, 0.25);
    margin-left: 4px;
}
.limux-tab-close:hover {
    color: rgba(255, 255, 255, 0.8);
    background: rgba(255, 255, 255, 0.1);
}
.limux-pane-action {
    background: none;
    border: none;
    border-radius: 4px;
    padding: 4px 5px;
    min-height: 0;
    min-width: 0;
    color: rgba(255, 255, 255, 0.35);
}
.limux-pane-action:hover {
    background: rgba(255, 255, 255, 0.08);
    color: rgba(255, 255, 255, 0.8);
}
.limux-split-icon {
    border: 1px solid rgba(255, 255, 255, 0.4);
    border-radius: 2px;
    min-width: 16px;
    min-height: 12px;
    padding: 0;
}
.limux-split-icon:hover {
    border-color: rgba(255, 255, 255, 0.8);
}
.limux-split-half-v {
    min-width: 6px;
    min-height: 10px;
}
.limux-split-half-h {
    min-width: 14px;
    min-height: 4px;
}
.limux-split-btn {
    background: none;
    border: none;
    border-radius: 4px;
    padding: 4px 5px;
    min-height: 0;
    min-width: 0;
}
.limux-split-btn:hover {
    background: rgba(255, 255, 255, 0.08);
}
.limux-pin-icon {
    font-size: 9px;
    margin-right: 2px;
}
.limux-tab-rename-entry {
    background: rgba(255, 255, 255, 0.1);
    color: white;
    border: 1px solid rgba(0, 145, 255, 0.5);
    border-radius: 3px;
    padding: 1px 4px;
    min-height: 0;
    font-size: 12px;
}
.limux-tab-drop-indicator {
    background-color: #5b9bd5;
    min-width: 2px;
    margin: 2px 0;
}
.limux-tab-overlay:drop(active) {
    box-shadow: none;
}
"#;

// ---------------------------------------------------------------------------
// PaneWidget builder
// ---------------------------------------------------------------------------

pub fn create_pane(
    callbacks: Rc<PaneCallbacks>,
    working_directory: Option<&str>,
    initial_state: Option<&PaneState>,
) -> gtk::Box {
    // Store workspace working directory for new tabs/splits to inherit
    let ws_wd: Rc<RefCell<Option<String>>> =
        Rc::new(RefCell::new(working_directory.map(|s| s.to_string())));

    let outer = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .vexpand(true)
        .build();

    // The single header line: tabs (left) + action icons (right)
    let header = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(0)
        .build();
    header.add_css_class("limux-pane-header");

    // Tab strip (left side)
    let tab_strip = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(0)
        .hexpand(true)
        .build();

    // Drop-insert indicator (thin vertical line shown during drag)
    let drop_indicator = gtk::Box::new(gtk::Orientation::Vertical, 0);
    drop_indicator.add_css_class("limux-tab-drop-indicator");
    drop_indicator.set_visible(false);
    drop_indicator.set_valign(gtk::Align::Fill);
    drop_indicator.set_halign(gtk::Align::Start);
    drop_indicator.set_hexpand(false);

    // Overlay so the indicator floats above the tab strip
    let tab_overlay = gtk::Overlay::new();
    tab_overlay.add_css_class("limux-tab-overlay");
    tab_overlay.set_child(Some(&tab_strip));
    tab_overlay.add_overlay(&drop_indicator);
    tab_overlay.set_hexpand(true);
    tab_overlay.set_clip_overlay(&drop_indicator, false);

    // Content stack for tab pages
    let content_stack = gtk::Stack::new();
    content_stack.set_transition_type(gtk::StackTransitionType::None);
    content_stack.set_hexpand(true);
    content_stack.set_vexpand(true);

    // Action icons (right side)
    let actions = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(1)
        .build();

    let new_term_btn = icon_button("utilities-terminal-symbolic", "New terminal tab");
    let new_browser_btn = icon_button("limux-globe-symbolic", "New browser tab");
    let split_h_btn = icon_button("limux-split-horizontal-symbolic", "Split right");
    let split_v_btn = icon_button("limux-split-vertical-symbolic", "Split down");
    let close_btn = icon_button("window-close-symbolic", "Close pane");

    actions.append(&new_term_btn);
    actions.append(&new_browser_btn);
    actions.append(&split_h_btn);
    actions.append(&split_v_btn);
    actions.append(&close_btn);

    header.append(&tab_overlay);
    header.append(&actions);

    outer.append(&header);
    outer.append(&content_stack);

    // Shared state for tabs
    let tab_state = Rc::new(std::cell::RefCell::new(TabState {
        tabs: Vec::new(),
        active_tab: None,
    }));

    // Flag set by workspace drag begin/end in window.rs
    let workspace_dragging = Rc::new(Cell::new(false));

    // Build PaneInternals early so callbacks and tab setup can share it
    let pane_id = next_pane_id();
    let internals = Rc::new(PaneInternals {
        pane_id,
        tab_state: tab_state.clone(),
        tab_strip: tab_strip.clone(),
        content_stack: content_stack.clone(),
        pane_outer: outer.clone(),
        callbacks: callbacks.clone(),
        working_directory: ws_wd.clone(),
        drop_indicator: drop_indicator.clone(),
        workspace_dragging: workspace_dragging.clone(),
    });

    // Unified drop target on tab_overlay — determines insertion index from
    // the x coordinate so tabs land *between* existing tabs, not on top.
    // Also manages the drop-insert indicator widget.
    {
        let indicator = drop_indicator.clone();
        let tgt = internals.clone();

        let strip_drop = gtk::DropTarget::new(glib::Type::STRING, gtk::gdk::DragAction::MOVE);

        // Position the indicator during drag motion
        {
            let state = internals.tab_state.clone();
            let indicator = indicator.clone();
            let ws_drag = workspace_dragging.clone();
            strip_drop.connect_motion(move |_, _x, _y| {
                if ws_drag.get() {
                    return gtk::gdk::DragAction::empty();
                }
                position_indicator(&state, &indicator, _x);
                gtk::gdk::DragAction::MOVE
            });
        }

        // Hide indicator when drag leaves this pane
        {
            let indicator = indicator.clone();
            strip_drop.connect_leave(move |_| {
                indicator.set_visible(false);
            });
        }

        // Perform the actual tab move on drop
        strip_drop.connect_drop(move |_, value, x, _| {
            let Ok(drag_data) = value.get::<String>() else {
                indicator.set_visible(false);
                return false;
            };
            let Some((src_pid, src_tid)) = drag_data.split_once(':') else {
                indicator.set_visible(false);
                return false;
            };
            let Ok(src_pane_id) = src_pid.parse::<u32>() else {
                indicator.set_visible(false);
                return false;
            };

            let insert_idx =
                find_insert_index(&tgt.tab_state, x, src_pane_id == tgt.pane_id, src_tid);

            if src_pane_id == tgt.pane_id {
                reorder_tab_to_index(
                    &tgt.tab_strip,
                    &tgt.tab_state,
                    &tgt.callbacks,
                    src_tid,
                    insert_idx,
                );
            } else if let Some(src_internals) = lookup_pane_internals(src_pane_id) {
                move_tab_between_panes(&src_internals, &tgt, src_tid, insert_idx);
            }

            indicator.set_visible(false);
            true
        });
        // Attach to the overlay so the drop target receives events
        // even when the cursor is over a tab button.
        tab_overlay.add_controller(strip_drop);
    }

    if let Some(saved_state) = initial_state {
        restore_tabs_from_state(&internals, working_directory, saved_state);
    } else {
        add_terminal_tab_inner(&internals, working_directory, None);
    }

    // Wire action buttons
    {
        let i = internals.clone();
        let wd = ws_wd.clone();
        new_term_btn.connect_clicked(move |_| {
            let dir = wd.borrow().clone();
            add_terminal_tab_inner(&i, dir.as_deref(), None);
        });
    }
    {
        let i = internals.clone();
        new_browser_btn.connect_clicked(move |_| {
            add_browser_tab_inner(&i, None);
        });
    }
    {
        let pw = outer.clone();
        let cb = callbacks.clone();
        split_h_btn.connect_clicked(move |_| {
            (cb.on_split)(&pw.clone().upcast(), gtk::Orientation::Horizontal);
        });
    }
    {
        let pw = outer.clone();
        let cb = callbacks.clone();
        split_v_btn.connect_clicked(move |_| {
            (cb.on_split)(&pw.clone().upcast(), gtk::Orientation::Vertical);
        });
    }
    {
        let pw = outer.clone();
        let cb = callbacks.clone();
        close_btn.connect_clicked(move |_| {
            (cb.on_close_pane)(&pw.clone().upcast());
        });
    }

    // Store internals on the outer widget so external code to cycle tabs
    register_pane(pane_id, &internals);
    unsafe {
        outer.set_data("limux-pane-internals", internals);
    }

    // Unregister from pane registry when this pane is destroyed
    outer.connect_destroy(move |_| {
        unregister_pane(pane_id);
    });

    outer
}

/// Cycle tabs in the focused pane. `delta`: 1 = next, -1 = prev.
pub fn cycle_tab_in_pane(pane_widget: &gtk::Widget, delta: i32) {
    let outer = pane_widget.downcast_ref::<gtk::Box>();
    let outer = match outer {
        Some(o) => o,
        None => return,
    };
    let internals: Rc<PaneInternals> = unsafe {
        match outer.data::<Rc<PaneInternals>>("limux-pane-internals") {
            Some(ptr) => ptr.as_ref().clone(),
            None => return,
        }
    };

    let ts = internals.tab_state.borrow();
    let len = ts.tabs.len();
    if len <= 1 {
        return;
    }

    let active_idx = ts
        .active_tab
        .as_ref()
        .and_then(|id| ts.tabs.iter().position(|e| e.id == *id))
        .unwrap_or(0);

    let new_idx = (active_idx as i32 + delta).rem_euclid(len as i32) as usize;
    let new_id = ts.tabs[new_idx].id.clone();
    drop(ts);

    activate_tab(
        &internals.tab_strip,
        &internals.content_stack,
        &internals.tab_state,
        &new_id,
    );
    (internals.callbacks.on_state_changed)();
}

// ---------------------------------------------------------------------------
// Internal tab state
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum TabKind {
    Terminal { cwd: Rc<RefCell<Option<String>>> },
    Browser { uri: Rc<RefCell<Option<String>>> },
}

struct TabEntry {
    id: String,
    tab_button: gtk::Box,
    #[allow(dead_code)]
    title_label: gtk::Label,
    content: gtk::Widget,
    custom_name: Option<String>,
    pinned: bool,
    kind: TabKind,
}

struct TabState {
    tabs: Vec<TabEntry>,
    active_tab: Option<String>,
}

/// Shared internals stored on the pane outer Box for external access.
pub struct PaneInternals {
    pub pane_id: u32,
    tab_state: Rc<std::cell::RefCell<TabState>>,
    tab_strip: gtk::Box,
    content_stack: gtk::Stack,
    pane_outer: gtk::Box,
    callbacks: Rc<PaneCallbacks>,
    working_directory: Rc<std::cell::RefCell<Option<String>>>,
    drop_indicator: gtk::Box,
    pub workspace_dragging: Rc<Cell<bool>>,
}

impl TabState {
    fn find_tab_mut(&mut self, id: &str) -> Option<&mut TabEntry> {
        self.tabs.iter_mut().find(|e| e.id == id)
    }
}

fn next_tab_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

// ---------------------------------------------------------------------------
// Icon button helper
// ---------------------------------------------------------------------------

fn icon_button(icon_name: &str, tooltip: &str) -> gtk::Button {
    let btn = gtk::Button::builder()
        .icon_name(icon_name)
        .tooltip_text(tooltip)
        .has_frame(false)
        .build();
    btn.add_css_class("limux-pane-action");
    btn
}

/// Create a split-pane icon button with two rectangles separated by a divider.
/// Horizontal = left|right panes, Vertical = top/bottom panes.
#[allow(dead_code)]
fn split_icon_button(orientation: gtk::Orientation, tooltip: &str) -> gtk::Button {
    let icon = gtk::Box::new(orientation, 1);
    icon.add_css_class("limux-split-icon");

    let (class_name, count) = match orientation {
        gtk::Orientation::Horizontal => ("limux-split-half-v", 2),
        _ => ("limux-split-half-h", 2),
    };

    for _ in 0..count {
        let half = gtk::Box::new(gtk::Orientation::Vertical, 0);
        half.add_css_class(class_name);
        icon.append(&half);
    }

    let btn = gtk::Button::builder()
        .child(&icon)
        .tooltip_text(tooltip)
        .has_frame(false)
        .build();
    btn.add_css_class("limux-split-btn");
    btn
}

// ---------------------------------------------------------------------------
// Tab creation
// ---------------------------------------------------------------------------

struct TerminalTabOptions<'a> {
    id: Option<&'a str>,
    custom_name: Option<&'a str>,
    pinned: bool,
    cwd: Option<&'a str>,
}

struct BrowserTabOptions<'a> {
    id: Option<&'a str>,
    custom_name: Option<&'a str>,
    pinned: bool,
    uri: Option<&'a str>,
}

fn restore_tabs_from_state(
    internals: &Rc<PaneInternals>,
    working_directory: Option<&str>,
    saved_state: &PaneState,
) {
    if saved_state.tabs.is_empty() {
        add_terminal_tab_inner(internals, working_directory, None);
        return;
    }

    for saved_tab in &saved_state.tabs {
        match &saved_tab.content {
            TabContentState::Terminal { cwd } => add_terminal_tab_inner(
                internals,
                cwd.as_deref().or(working_directory),
                Some(TerminalTabOptions {
                    id: Some(saved_tab.id.as_str()),
                    custom_name: saved_tab.custom_name.as_deref(),
                    pinned: saved_tab.pinned,
                    cwd: cwd.as_deref().or(working_directory),
                }),
            ),
            TabContentState::Browser { uri } => add_browser_tab_inner(
                internals,
                Some(BrowserTabOptions {
                    id: Some(saved_tab.id.as_str()),
                    custom_name: saved_tab.custom_name.as_deref(),
                    pinned: saved_tab.pinned,
                    uri: uri.as_deref(),
                }),
            ),
        }
    }

    let active_tab_id = saved_state
        .active_tab_id
        .as_deref()
        .filter(|candidate| {
            internals
                .tab_state
                .borrow()
                .tabs
                .iter()
                .any(|tab| tab.id == *candidate)
        })
        .map(|value| value.to_string())
        .or_else(|| {
            internals
                .tab_state
                .borrow()
                .tabs
                .first()
                .map(|tab| tab.id.clone())
        });

    if let Some(active_tab_id) = active_tab_id {
        activate_tab(
            &internals.tab_strip,
            &internals.content_stack,
            &internals.tab_state,
            &active_tab_id,
        );
    }
}

fn add_terminal_tab_inner(
    internals: &Rc<PaneInternals>,
    working_directory: Option<&str>,
    options: Option<TerminalTabOptions<'_>>,
) {
    let tab_id = options
        .as_ref()
        .and_then(|value| value.id.map(|id| id.to_string()))
        .unwrap_or_else(next_tab_id);

    // Tab label button
    let (tab_btn, title_label) = build_tab_button("Terminal", &tab_id, internals);

    // Build Ghostty terminal callbacks for title/bell/close
    let term_cwd = Rc::new(RefCell::new(
        options
            .as_ref()
            .and_then(|value| value.cwd.map(|cwd| cwd.to_string()))
            .or_else(|| working_directory.map(|cwd| cwd.to_string())),
    ));
    let term_callbacks = {
        let tl = title_label.clone();
        let state_for_title = internals.tab_state.clone();
        let tid_for_title = tab_id.clone();
        let cb_bell = internals.callbacks.clone();
        let ts = internals.tab_strip.clone();
        let cs = internals.content_stack.clone();
        let state_for_close = internals.tab_state.clone();
        let tid_for_close = tab_id.clone();
        let cb_close = internals.callbacks.clone();
        let po = internals.pane_outer.clone();
        let cb_state = internals.callbacks.clone();
        let term_cwd_for_pwd = term_cwd.clone();

        TerminalCallbacks {
            on_title_changed: Box::new(move |title: &str| {
                let has_custom = state_for_title
                    .borrow()
                    .tabs
                    .iter()
                    .any(|e| e.id == tid_for_title && e.custom_name.is_some());
                if has_custom {
                    return;
                }
                if !title.is_empty() {
                    let display = if title.len() > 22 {
                        format!("{}…", &title[..21])
                    } else {
                        title.to_string()
                    };
                    tl.set_label(&display);
                }
            }),
            on_bell: Box::new(move || {
                (cb_bell.on_bell)();
            }),
            on_pwd_changed: Box::new({
                let cb_pwd = internals.callbacks.clone();
                move |pwd: &str| {
                    *term_cwd_for_pwd.borrow_mut() = Some(pwd.to_string());
                    (cb_pwd.on_pwd_changed)(pwd);
                    (cb_state.on_state_changed)();
                }
            }),
            on_close: Box::new(move || {
                let ts = ts.clone();
                let cs = cs.clone();
                let state = state_for_close.clone();
                let tid = tid_for_close.clone();
                let cb = cb_close.clone();
                let po = po.clone();
                glib::idle_add_local_once(move || {
                    remove_tab(&ts, &cs, &state, &tid, &cb, &po);
                });
            }),
            on_split_right: Box::new({
                let cb = internals.callbacks.clone();
                let po = internals.pane_outer.clone();
                move || {
                    let w: gtk::Widget = po.clone().upcast();
                    (cb.on_split)(&w, gtk::Orientation::Horizontal);
                }
            }),
            on_split_down: Box::new({
                let cb = internals.callbacks.clone();
                let po = internals.pane_outer.clone();
                move || {
                    let w: gtk::Widget = po.clone().upcast();
                    (cb.on_split)(&w, gtk::Orientation::Vertical);
                }
            }),
        }
    };

    let term = terminal::create_terminal(working_directory, term_callbacks);

    let widget: gtk::Widget = term.clone().upcast();
    internals.content_stack.add_named(&widget, Some(&tab_id));

    // Append the tab button to the strip AFTER all setup is done
    internals.tab_strip.append(&tab_btn);

    {
        let mut ts = internals.tab_state.borrow_mut();
        ts.tabs.push(TabEntry {
            id: tab_id.clone(),
            tab_button: tab_btn,
            title_label: title_label.clone(),
            content: widget,
            custom_name: options
                .as_ref()
                .and_then(|value| value.custom_name.map(|name| name.to_string())),
            pinned: options.as_ref().map(|value| value.pinned).unwrap_or(false),
            kind: TabKind::Terminal {
                cwd: term_cwd.clone(),
            },
        });
    }

    if let Some(custom_name) = options.as_ref().and_then(|value| value.custom_name) {
        title_label.set_label(custom_name);
    }
    if options.as_ref().map(|value| value.pinned).unwrap_or(false) {
        if let Some(entry) = internals
            .tab_state
            .borrow()
            .tabs
            .iter()
            .find(|entry| entry.id == tab_id)
        {
            apply_pin_visuals(&entry.tab_button, true);
        }
    }

    activate_tab(
        &internals.tab_strip,
        &internals.content_stack,
        &internals.tab_state,
        &tab_id,
    );
    term.grab_focus();
    if options.is_none() {
        (internals.callbacks.on_state_changed)();
    }
}

fn add_browser_tab_inner(internals: &Rc<PaneInternals>, options: Option<BrowserTabOptions<'_>>) {
    let tab_id = options
        .as_ref()
        .and_then(|value| value.id.map(|id| id.to_string()))
        .unwrap_or_else(next_tab_id);
    let saved_uri = Rc::new(RefCell::new(
        options
            .as_ref()
            .and_then(|value| value.uri.map(|uri| uri.to_string())),
    ));
    let (widget, title) = create_browser_widget(
        options.as_ref().and_then(|value| value.uri),
        saved_uri.clone(),
        internals.callbacks.clone(),
    );

    let (tab_btn, title_label) = build_tab_button(&title, &tab_id, internals);

    internals.content_stack.add_named(&widget, Some(&tab_id));

    // Append the tab button to the strip AFTER all setup is done
    internals.tab_strip.append(&tab_btn);

    {
        let mut ts = internals.tab_state.borrow_mut();
        ts.tabs.push(TabEntry {
            id: tab_id.clone(),
            tab_button: tab_btn,
            title_label: title_label.clone(),
            content: widget,
            custom_name: options
                .as_ref()
                .and_then(|value| value.custom_name.map(|name| name.to_string())),
            pinned: options.as_ref().map(|value| value.pinned).unwrap_or(false),
            kind: TabKind::Browser {
                uri: saved_uri.clone(),
            },
        });
    }

    if let Some(custom_name) = options.as_ref().and_then(|value| value.custom_name) {
        title_label.set_label(custom_name);
    }
    if options.as_ref().map(|value| value.pinned).unwrap_or(false) {
        if let Some(entry) = internals
            .tab_state
            .borrow()
            .tabs
            .iter()
            .find(|entry| entry.id == tab_id)
        {
            apply_pin_visuals(&entry.tab_button, true);
        }
    }

    activate_tab(
        &internals.tab_strip,
        &internals.content_stack,
        &internals.tab_state,
        &tab_id,
    );
    if options.is_none() {
        (internals.callbacks.on_state_changed)();
    }
}

// Public wrappers for keyboard shortcut use
#[allow(dead_code)]
pub fn add_terminal_tab_to_pane(pane_widget: &gtk::Widget) {
    if let Some(internals) = find_pane_internals(pane_widget) {
        let dir = internals.working_directory.borrow().clone();
        add_terminal_tab_inner(&internals, dir.as_deref(), None);
    }
}

#[allow(dead_code)]
pub fn add_browser_tab_to_pane(pane_widget: &gtk::Widget) {
    if let Some(internals) = find_pane_internals(pane_widget) {
        add_browser_tab_inner(&internals, None);
    }
}

pub fn snapshot_pane_state(pane_widget: &gtk::Widget) -> Option<PaneState> {
    let internals = find_pane_internals(pane_widget)?;
    let ts = internals.tab_state.borrow();
    let tabs = ts
        .tabs
        .iter()
        .map(|entry| {
            let content = match &entry.kind {
                TabKind::Terminal { cwd } => TabContentState::Terminal {
                    cwd: cwd.borrow().clone(),
                },
                TabKind::Browser { uri } => TabContentState::Browser {
                    uri: uri.borrow().clone(),
                },
            };
            SavedTabState {
                id: entry.id.clone(),
                custom_name: entry.custom_name.clone(),
                pinned: entry.pinned,
                content,
            }
        })
        .collect();
    Some(PaneState {
        active_tab_id: ts.active_tab.clone(),
        tabs,
    })
}

#[allow(dead_code)]
fn find_pane_internals(pane_widget: &gtk::Widget) -> Option<Rc<PaneInternals>> {
    let outer = pane_widget.downcast_ref::<gtk::Box>()?;
    unsafe {
        outer
            .data::<Rc<PaneInternals>>("limux-pane-internals")
            .map(|ptr| ptr.as_ref().clone())
    }
}

fn apply_pin_visuals(tab_button: &gtk::Box, pinned: bool) {
    if let Some(close_widget) = tab_button.last_child() {
        close_widget.set_visible(!pinned);
    }
    if let Some(inner_box) = tab_button
        .first_child()
        .and_then(|child| child.downcast::<gtk::Box>().ok())
    {
        if let Some(pin_icon) = inner_box
            .first_child()
            .and_then(|child| child.downcast::<gtk::Label>().ok())
        {
            pin_icon.set_label(if pinned { "📌" } else { "" });
            pin_icon.set_visible(pinned);
        }
    }
}

/// Move a tab from a source pane to a target pane by widget reference.
/// Used for cross-workspace tab dragging (sidebar drops).
pub fn move_tab_to_pane(source_pane: &gtk::Widget, source_tab_id: &str, target_pane: &gtk::Widget) {
    let Some(src_internals) = find_pane_internals(source_pane) else {
        return;
    };
    let Some(tgt_internals) = find_pane_internals(target_pane) else {
        return;
    };
    // Don't move to the same pane
    if src_internals.pane_id == tgt_internals.pane_id {
        return;
    }
    // Compute length BEFORE the call so the borrow is dropped
    let target_len = tgt_internals.tab_state.borrow().tabs.len();
    move_tab_between_panes(&src_internals, &tgt_internals, source_tab_id, target_len);
}

// ---------------------------------------------------------------------------
// Tab button (label + close)
// ---------------------------------------------------------------------------

fn build_tab_button(
    title: &str,
    tab_id: &str,
    internals: &Rc<PaneInternals>,
) -> (gtk::Box, gtk::Label) {
    let pin_icon = gtk::Label::new(None);
    pin_icon.add_css_class("limux-pin-icon");
    pin_icon.set_visible(false);
    pin_icon.set_can_target(false); // let clicks pass through to parent

    let label = gtk::Label::builder()
        .label(title)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(20)
        .build();
    label.set_can_target(false); // let clicks pass through to parent

    // Close button needs its own click handling, so it stays targetable
    let close_btn = gtk::Button::builder()
        .icon_name("window-close-symbolic")
        .has_frame(false)
        .build();
    close_btn.add_css_class("limux-tab-close");

    let inner_box = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    inner_box.set_can_target(false); // pass events through
    inner_box.append(&pin_icon);
    inner_box.append(&label);

    // Use an overlay approach: the tab_btn is the event target,
    // inner_box + close_btn are children
    let tab_btn = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    tab_btn.add_css_class("limux-tab");
    tab_btn.append(&inner_box);
    tab_btn.append(&close_btn);

    // Left-click on the tab area (not the close button) → activate
    let click = gtk::GestureClick::new();
    click.set_button(1);
    {
        let tid = tab_id.to_string();
        let ts = internals.tab_strip.clone();
        let cs = internals.content_stack.clone();
        let state = internals.tab_state.clone();
        let cb = internals.callbacks.clone();
        click.connect_pressed(move |_, _, _, _| {
            activate_tab(&ts, &cs, &state, &tid);
            (cb.on_state_changed)();
        });
    }
    tab_btn.add_controller(click);

    // Right-click → context menu
    let right_click = gtk::GestureClick::new();
    right_click.set_button(3);
    {
        let tid = tab_id.to_string();
        let ts = internals.tab_strip.clone();
        let cs = internals.content_stack.clone();
        let state = internals.tab_state.clone();
        let cb = internals.callbacks.clone();
        let po = internals.pane_outer.clone();
        let lbl = label.clone();
        let pin = pin_icon.clone();
        let tb = tab_btn.clone();
        let context = TabContextMenuContext {
            tab_strip: ts,
            content_stack: cs,
            tab_state: state,
            callbacks: cb,
            pane_outer: po,
            label: lbl,
            pin_icon: pin,
        };
        right_click.connect_pressed(move |_gesture, _, _x, _y| {
            show_tab_context_menu(&tb, &tid, &context);
        });
    }
    tab_btn.add_controller(right_click);

    // Drag source for reorder (includes pane_id for cross-pane DnD)
    let drag_source = gtk::DragSource::new();
    drag_source.set_actions(gtk::gdk::DragAction::MOVE);
    {
        let tid = tab_id.to_string();
        let po = internals.pane_outer.clone();
        let state = internals.tab_state.clone();
        let indicator_begin = internals.drop_indicator.clone();
        let indicator_end = internals.drop_indicator.clone();
        drag_source.connect_prepare(move |_src, _x, _y| {
            let pid = unsafe {
                po.data::<Rc<PaneInternals>>("limux-pane-internals")
                    .map(|ptr| ptr.as_ref().pane_id)
                    .unwrap_or(0)
            };
            let val = glib::Value::from(&format!("{pid}:{tid}"));
            Some(gtk::gdk::ContentProvider::for_value(&val))
        });
        drag_source.connect_drag_begin(move |src, _drag| {
            if let Some(w) = src.widget() {
                let alloc = w.allocation();
                position_indicator(&state, &indicator_begin, (alloc.x() + alloc.width()) as f64);
                let icon = gtk::WidgetPaintable::new(Some(&w));
                src.set_icon(Some(&icon), 0, 0);
            }
        });
        drag_source.connect_drag_end(move |_, _, _| {
            indicator_end.set_visible(false);
        });
    }
    tab_btn.add_controller(drag_source);

    // Close button click
    {
        let tid = tab_id.to_string();
        let ts = internals.tab_strip.clone();
        let cs = internals.content_stack.clone();
        let state = internals.tab_state.clone();
        let cb = internals.callbacks.clone();
        let po = internals.pane_outer.clone();
        close_btn.connect_clicked(move |_| {
            let is_pinned = state.borrow().tabs.iter().any(|e| e.id == tid && e.pinned);
            if !is_pinned {
                remove_tab(&ts, &cs, &state, &tid, &cb, &po);
            }
        });
    }

    // NOTE: Caller is responsible for appending tab_btn to the tab strip.
    // This avoids triggering GTK signals while the caller may still hold
    // RefCell borrows.

    (tab_btn, label)
}

fn show_tab_context_menu(tab_btn: &gtk::Box, tab_id: &str, context: &TabContextMenuContext) {
    let menu = gtk::PopoverMenu::from_model(None::<&gtk::gio::MenuModel>);
    let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    menu_box.set_margin_top(4);
    menu_box.set_margin_bottom(4);
    menu_box.set_margin_start(4);
    menu_box.set_margin_end(4);

    // Rename
    let rename_btn = gtk::Button::with_label("Rename");
    rename_btn.add_css_class("flat");
    {
        let lbl = context.label.clone();
        let state = context.tab_state.clone();
        let tid = tab_id.to_string();
        let menu_ref = menu.clone();
        let callbacks = context.callbacks.clone();
        rename_btn.connect_clicked(move |_| {
            menu_ref.popdown();
            show_rename_dialog(&lbl, &state, &tid, &callbacks);
        });
    }

    // Pin / Unpin
    let is_pinned = context
        .tab_state
        .borrow()
        .tabs
        .iter()
        .any(|e| e.id == tab_id && e.pinned);
    let pin_label = if is_pinned { "Unpin" } else { "Pin" };
    let pin_btn = gtk::Button::with_label(pin_label);
    pin_btn.add_css_class("flat");
    {
        let state = context.tab_state.clone();
        let tid = tab_id.to_string();
        let pin = context.pin_icon.clone();
        let close = tab_btn.last_child(); // close button
        let menu_ref = menu.clone();
        let callbacks = context.callbacks.clone();
        pin_btn.connect_clicked(move |_| {
            menu_ref.popdown();
            let mut ts = state.borrow_mut();
            if let Some(entry) = ts.find_tab_mut(&tid) {
                entry.pinned = !entry.pinned;
                apply_pin_visuals(&entry.tab_button, entry.pinned);
                pin.set_label(if entry.pinned { "📌" } else { "" });
                pin.set_visible(entry.pinned);
                if let Some(close_widget) = &close {
                    close_widget.set_visible(!entry.pinned);
                }
            }
            drop(ts);
            (callbacks.on_state_changed)();
        });
    }

    // Close
    let close_btn = gtk::Button::with_label("Close");
    close_btn.add_css_class("flat");
    {
        let tid = tab_id.to_string();
        let ts = context.tab_strip.clone();
        let cs = context.content_stack.clone();
        let state = context.tab_state.clone();
        let cb = context.callbacks.clone();
        let po = context.pane_outer.clone();
        let menu_ref = menu.clone();
        close_btn.connect_clicked(move |_| {
            menu_ref.popdown();
            remove_tab(&ts, &cs, &state, &tid, &cb, &po);
        });
    }

    menu_box.append(&rename_btn);
    menu_box.append(&pin_btn);
    menu_box.append(&close_btn);
    menu.set_child(Some(&menu_box));
    menu.set_parent(tab_btn);
    menu.set_has_arrow(false);

    // Clean up popover when it closes
    menu.connect_closed(move |popover| {
        popover.unparent();
    });

    menu.popup();
}

fn show_rename_dialog(
    label: &gtk::Label,
    tab_state: &Rc<RefCell<TabState>>,
    tab_id: &str,
    callbacks: &Rc<PaneCallbacks>,
) {
    let current_name = label.label().to_string();

    // Replace label with an entry temporarily
    let parent = label.parent().and_then(|p| p.downcast::<gtk::Box>().ok());
    let Some(parent) = parent else {
        return;
    };

    let entry = gtk::Entry::builder()
        .text(&current_name)
        .width_chars(15)
        .build();
    entry.add_css_class("limux-tab-rename-entry");

    label.set_visible(false);
    // Insert entry before the close button
    parent.insert_child_after(&entry, Some(label));
    entry.grab_focus();
    entry.select_region(0, -1);

    // On activate (Enter) or focus-out, commit rename
    let lbl = label.clone();
    let state = tab_state.clone();
    let tid = tab_id.to_string();
    let parent_for_cleanup = parent.clone();

    let commit = Rc::new(std::cell::Cell::new(false));

    let do_rename = {
        let commit = commit.clone();
        let lbl = lbl.clone();
        let state = state.clone();
        let tid = tid.clone();
        let parent = parent_for_cleanup.clone();
        let callbacks = callbacks.clone();
        move |entry: &gtk::Entry| {
            if commit.get() {
                return;
            }
            commit.set(true);
            let new_name = entry.text().to_string();
            if !new_name.trim().is_empty() {
                lbl.set_label(&new_name);
                let mut ts = state.borrow_mut();
                if let Some(tab) = ts.find_tab_mut(&tid) {
                    tab.custom_name = Some(new_name);
                }
            }
            lbl.set_visible(true);
            parent.remove(entry);
            (callbacks.on_state_changed)();
        }
    };

    {
        let do_rename = do_rename.clone();
        entry.connect_activate(move |e| {
            do_rename(e);
        });
    }
    {
        let do_rename = do_rename.clone();
        let focus_controller = gtk::EventControllerFocus::new();
        focus_controller.connect_leave(move |ctrl| {
            if let Some(widget) = ctrl.widget() {
                if let Some(entry) = widget.downcast_ref::<gtk::Entry>() {
                    do_rename(entry);
                }
            }
        });
        entry.add_controller(focus_controller);
    }
}

/// Position the drop-insert indicator at the nearest tab boundary for the
/// given x coordinate.  The indicator is shown as a thin vertical line.
fn position_indicator(tab_state: &Rc<std::cell::RefCell<TabState>>, indicator: &gtk::Box, x: f64) {
    let ts = tab_state.borrow();
    if ts.tabs.is_empty() {
        indicator.set_visible(false);
        return;
    }

    // Find the tab boundary closest to x
    let mut pos: i32 = 0;
    for entry in ts.tabs.iter() {
        let alloc = entry.tab_button.allocation();
        let left = alloc.x();
        let right = left + alloc.width();
        let mid = left + alloc.width() / 2;

        if x < mid as f64 {
            pos = left;
            break;
        }
        pos = right;
    }

    drop(ts);

    indicator.set_margin_start(pos);
    indicator.set_visible(true);
}

/// Compute the insertion index for a tab drop at the given x coordinate.
/// Walks tab buttons and finds the first one whose midpoint is past `x`,
/// then returns that index (i.e. "insert before this tab").  If the drop is
/// past every tab, returns the length (append at end).
///
/// For same-pane reorders the source tab is excluded from the midpoint
/// comparison so it doesn't interfere with its own drop.
fn find_insert_index(
    tab_state: &Rc<std::cell::RefCell<TabState>>,
    x: f64,
    same_pane: bool,
    source_tab_id: &str,
) -> usize {
    let ts = tab_state.borrow();
    for (i, entry) in ts.tabs.iter().enumerate() {
        if same_pane && entry.id == source_tab_id {
            continue;
        }
        let alloc = entry.tab_button.allocation();
        let mid = alloc.x() as f64 + alloc.width() as f64 / 2.0;
        if x < mid {
            drop(ts);
            return i;
        }
    }
    let result = ts.tabs.len();
    drop(ts);
    result
}

/// Reorder a tab within the same pane to the given insertion index.
fn reorder_tab_to_index(
    tab_strip: &gtk::Box,
    tab_state: &Rc<RefCell<TabState>>,
    callbacks: &Rc<PaneCallbacks>,
    source_id: &str,
    insert_idx: usize,
) {
    let mut ts = tab_state.borrow_mut();
    let Some(src_idx) = ts.tabs.iter().position(|e| e.id == source_id) else {
        return;
    };

    let entry = ts.tabs.remove(src_idx);
    // After removal, if the source was before the insertion point, the
    // effective target shifted down by one.
    let adjusted = if src_idx < insert_idx {
        insert_idx - 1
    } else {
        insert_idx
    };
    ts.tabs.insert(adjusted, entry);

    let buttons: Vec<gtk::Box> = ts.tabs.iter().map(|e| e.tab_button.clone()).collect();
    drop(ts);

    for btn in &buttons {
        tab_strip.remove(btn);
    }
    for btn in &buttons {
        tab_strip.append(btn);
    }
    let cb = callbacks.clone();
    glib::idle_add_local_once(move || {
        (cb.on_state_changed)();
    });
}

/// Move a tab from one pane to another (for cross-pane drag-and-drop).
/// `insert_idx`: absolute index in the target pane's tab list.
fn move_tab_between_panes(
    src_internals: &Rc<PaneInternals>,
    tgt_internals: &Rc<PaneInternals>,
    src_tab_id: &str,
    insert_idx: usize,
) {
    // ---- Phase 1: data-only operations on the source pane ----

    let (mut entry, content_widget, src_is_empty, src_new_active) = {
        let mut src_ts = src_internals.tab_state.borrow_mut();
        let Some(src_idx) = src_ts.tabs.iter().position(|e| e.id == src_tab_id) else {
            return;
        };
        let entry = src_ts.tabs.remove(src_idx);

        let src_was_active = src_ts.active_tab.as_deref() == Some(src_tab_id);
        let src_is_empty = src_ts.tabs.is_empty();
        let src_new_active = if src_was_active && !src_is_empty {
            let new_idx = src_idx.min(src_ts.tabs.len() - 1);
            Some(src_ts.tabs[new_idx].id.clone())
        } else {
            None
        };

        let content = entry.content.clone();
        (entry, content, src_is_empty, src_new_active)
    };

    // Remove content from source stack synchronously (required before the
    // target-side idle callback can add it — a widget can't have two parents).
    src_internals.content_stack.remove(&content_widget);

    // ---- Phase 2: data-only operations on the target pane ----

    // Save original source tab button BEFORE replacing it with a placeholder.
    let src_tab_btn = entry.tab_button.clone();

    let new_tab_id = next_tab_id();
    entry.id = new_tab_id.clone();
    // Placeholder — the real button is created in the idle callback.
    entry.tab_button = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    entry.content = content_widget.clone();

    {
        let mut tgt_ts = tgt_internals.tab_state.borrow_mut();
        let clamped = insert_idx.min(tgt_ts.tabs.len());
        tgt_ts.tabs.insert(clamped, entry);
    }

    // ---- Phase 3: ALL UI work deferred to idle to avoid RefCell conflicts ----

    let target_strip = tgt_internals.tab_strip.clone();
    let target_cs = tgt_internals.content_stack.clone();
    let target_state = tgt_internals.tab_state.clone();
    let target_cb = tgt_internals.callbacks.clone();
    let target_outer = tgt_internals.pane_outer.clone();
    let content = content_widget;
    let tid = new_tab_id;

    // Clone source pane variables before the single idle callback
    let src_strip = src_internals.tab_strip.clone();
    let src_cs = src_internals.content_stack.clone();
    let src_state = src_internals.tab_state.clone();
    let src_callbacks = src_internals.callbacks.clone();
    let src_outer = src_internals.pane_outer.clone();

    // Build an owned PaneInternals for the target (idle callback requires 'static)
    let tgt = Rc::new(PaneInternals {
        pane_id: tgt_internals.pane_id,
        tab_strip: target_strip,
        content_stack: target_cs,
        tab_state: target_state,
        callbacks: target_cb,
        pane_outer: target_outer,
        working_directory: tgt_internals.working_directory.clone(),
        drop_indicator: tgt_internals.drop_indicator.clone(),
        workspace_dragging: tgt_internals.workspace_dragging.clone(),
    });

    glib::idle_add_local_once(move || {
        // Get the title from the stored entry
        let title = {
            let ts = tgt.tab_state.borrow();
            ts.tabs
                .iter()
                .find(|e| e.id == tid)
                .map(|e| e.title_label.label().to_string())
                .unwrap_or_else(|| "Terminal".to_string())
        };

        // Create the real tab button (this may trigger GTK signals — safe now)
        let (new_tab_btn, new_title_label) = build_tab_button(&title, &tid, &tgt);

        // Add the content widget to the stack
        tgt.content_stack.add_named(&content, Some(&tid));

        // Update the stored entry with the real button, collecting old placeholder
        let placeholder = {
            let mut ts = tgt.tab_state.borrow_mut();
            if let Some(entry) = ts.tabs.iter_mut().find(|e| e.id == tid) {
                let old = entry.tab_button.clone();
                entry.tab_button = new_tab_btn;
                entry.title_label = new_title_label;
                old
            } else {
                return;
            }
        };

        // Remove the placeholder if it's actually in the strip
        if placeholder.parent().is_some() {
            tgt.tab_strip.remove(&placeholder);
        }

        // Rebuild strip order: remove all existing buttons, then re-add in order
        let buttons: Vec<gtk::Box> = tgt
            .tab_state
            .borrow()
            .tabs
            .iter()
            .map(|e| e.tab_button.clone())
            .collect();
        for btn in &buttons {
            if btn.parent().is_some() {
                tgt.tab_strip.remove(btn);
            }
        }
        for btn in &buttons {
            tgt.tab_strip.append(btn);
        }

        activate_tab(&tgt.tab_strip, &tgt.content_stack, &tgt.tab_state, &tid);

        // Source pane cleanup — deferred to avoid RefCell conflicts with GTK
        // callbacks that may fire during widget removal (click/drag handlers).
        // Runs after target setup to ensure pane is fully ready before cleanup.
        src_strip.remove(&src_tab_btn);
        if src_is_empty {
            (src_callbacks.on_empty)(&src_outer.upcast());
        } else if let Some(new_id) = src_new_active {
            activate_tab(&src_strip, &src_cs, &src_state, &new_id);
        }
    });
}

// ---------------------------------------------------------------------------
// Tab activation / removal
// ---------------------------------------------------------------------------

fn activate_tab(
    _tab_strip: &gtk::Box,
    content_stack: &gtk::Stack,
    tab_state: &Rc<RefCell<TabState>>,
    tab_id: &str,
) {
    let mut ts = tab_state.borrow_mut();
    if ts.active_tab.as_deref() == Some(tab_id) {
        return;
    }
    ts.active_tab = Some(tab_id.to_string());

    // Update visual state on all tabs
    for entry in &ts.tabs {
        if entry.id == tab_id {
            entry.tab_button.add_css_class("limux-tab-active");
        } else {
            entry.tab_button.remove_css_class("limux-tab-active");
        }
    }

    // Only switch if the stack already has this child
    if content_stack.child_by_name(tab_id).is_some() {
        content_stack.set_visible_child_name(tab_id);
    }

    // Focus the content — only grab focus on directly focusable widgets (terminals).
    // For containers (browser vbox), focus the first focusable child instead.
    if let Some(entry) = ts.tabs.iter().find(|e| e.id == tab_id) {
        let content = entry.content.clone();
        drop(ts);
        if content.is_focus() || content.can_focus() {
            content.grab_focus();
        } else {
            // Try to find a focusable child (e.g., the WebView inside a Box)
            content.child_focus(gtk::DirectionType::TabForward);
        }
    }
}

fn remove_tab(
    tab_strip: &gtk::Box,
    content_stack: &gtk::Stack,
    tab_state: &Rc<RefCell<TabState>>,
    tab_id: &str,
    callbacks: &Rc<PaneCallbacks>,
    pane_outer: &gtk::Box,
) {
    let mut ts = tab_state.borrow_mut();
    let Some(idx) = ts.tabs.iter().position(|e| e.id == tab_id) else {
        return;
    };
    let entry = ts.tabs.remove(idx);

    tab_strip.remove(&entry.tab_button);
    content_stack.remove(&entry.content);

    if ts.tabs.is_empty() {
        drop(ts);
        (callbacks.on_empty)(&pane_outer.clone().upcast());
        return;
    }

    // Activate neighbor tab
    let new_idx = idx.min(ts.tabs.len() - 1);
    let new_id = ts.tabs[new_idx].id.clone();
    let was_active = ts.active_tab.as_deref() == Some(tab_id);
    drop(ts);

    if was_active {
        activate_tab(tab_strip, content_stack, tab_state, &new_id);
    }
    (callbacks.on_state_changed)();
}

// ---------------------------------------------------------------------------
// Browser widget
// ---------------------------------------------------------------------------

#[cfg(feature = "webkit")]
fn create_browser_widget(
    initial_uri: Option<&str>,
    saved_uri: Rc<RefCell<Option<String>>>,
    callbacks: Rc<PaneCallbacks>,
) -> (gtk::Widget, String) {
    use webkit6::prelude::*;

    // Use a NetworkSession to avoid sandbox issues
    let network_session = webkit6::NetworkSession::default();
    let web_context = webkit6::WebContext::default();

    let webview = webkit6::WebView::builder()
        .hexpand(true)
        .vexpand(true)
        .build();

    // Set permissive settings
    if let Some(settings) = webkit6::prelude::WebViewExt::settings(&webview) {
        settings.set_enable_developer_extras(true);
        settings.set_javascript_can_open_windows_automatically(true);
    }

    let url_entry = gtk::Entry::builder()
        .placeholder_text("Enter URL...")
        .hexpand(true)
        .build();

    let back_btn = icon_button("go-previous-symbolic", "Back");
    let fwd_btn = icon_button("go-next-symbolic", "Forward");
    let reload_btn = icon_button("view-refresh-symbolic", "Reload");

    let nav_bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    nav_bar.add_css_class("limux-pane-header");
    nav_bar.append(&back_btn);
    nav_bar.append(&fwd_btn);
    nav_bar.append(&reload_btn);
    nav_bar.append(&url_entry);

    {
        let wv = webview.clone();
        back_btn.connect_clicked(move |_| {
            wv.go_back();
        });
    }
    {
        let wv = webview.clone();
        fwd_btn.connect_clicked(move |_| {
            wv.go_forward();
        });
    }
    {
        let wv = webview.clone();
        reload_btn.connect_clicked(move |_| {
            wv.reload();
        });
    }
    {
        let wv = webview.clone();
        url_entry.connect_activate(move |entry| {
            let mut url = entry.text().to_string();
            if !url.starts_with("http://") && !url.starts_with("https://") {
                if url.contains('.') {
                    url = format!("https://{url}");
                } else {
                    url = format!("https://www.google.com/search?q={}", url.replace(' ', "+"));
                }
            }
            wv.load_uri(&url);
        });
    }
    {
        let entry = url_entry.clone();
        let saved_uri = saved_uri.clone();
        let callbacks = callbacks.clone();
        let restoring = Rc::new(std::cell::Cell::new(initial_uri.is_some()));
        let restoring_flag = restoring.clone();
        webview.connect_uri_notify(move |wv| {
            if let Some(uri) = wv.uri() {
                let uri_str: String = uri.into();
                entry.set_text(&uri_str);
                if restoring_flag.get() && (uri_str.is_empty() || uri_str == "about:blank") {
                    return;
                }
                restoring_flag.set(false);
                *saved_uri.borrow_mut() = Some(uri_str);
                (callbacks.on_state_changed)();
            }
        });
    }

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    vbox.append(&nav_bar);
    vbox.append(&webview.clone());
    vbox.set_hexpand(true);
    vbox.set_vexpand(true);

    // Load default URL only on the first map. The WebView preserves its
    // page and history across reparenting (splits), so we must not reload.
    {
        let wv = webview.clone();
        let loaded = std::cell::Cell::new(false);
        let initial_uri = initial_uri.map(|value| value.to_string());
        vbox.connect_map(move |_| {
            if !loaded.get() {
                loaded.set(true);
                if let Some(uri) = &initial_uri {
                    wv.load_uri(uri);
                } else {
                    wv.load_uri("https://google.com");
                }
            }
        });
    }

    // Suppress unused variable warnings
    let _ = network_session;
    let _ = web_context;

    (vbox.upcast(), "Browser".to_string())
}

#[cfg(not(feature = "webkit"))]
fn create_browser_widget(
    initial_uri: Option<&str>,
    saved_uri: Rc<RefCell<Option<String>>>,
    _callbacks: Rc<PaneCallbacks>,
) -> (gtk::Widget, String) {
    *saved_uri.borrow_mut() = initial_uri.map(|value| value.to_string());
    let placeholder = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .spacing(12)
        .build();

    let msg = gtk::Label::builder()
        .label("Browser requires webkit6")
        .build();
    msg.set_css_classes(&["dim-label"]);

    let hint = gtk::Label::builder()
        .label("sudo apt install libwebkitgtk-6.0-dev\ncargo build --features webkit")
        .justify(gtk::Justification::Center)
        .build();
    hint.set_css_classes(&["dim-label"]);

    placeholder.append(&msg);
    placeholder.append(&hint);
    placeholder.set_hexpand(true);
    placeholder.set_vexpand(true);

    (placeholder.upcast(), "Browser".to_string())
}
