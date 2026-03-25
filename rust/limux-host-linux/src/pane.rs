//! PaneWidget: a tabbed container with action icons in the tab bar.
//!
//! Layout: [tab1 x] [tab2 x] ... ←spacer→ [terminal] [browser] [split-h] [split-v] [close]
//!
//! All on one line. Tabs left-justified, icons right-justified.

#[cfg(feature = "webkit")]
use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;

use gtk::glib;
#[allow(unused_imports)]
use gtk::prelude::*;
use gtk4 as gtk;
#[cfg(feature = "webkit")]
use webkit6::prelude::*;

use crate::keybind_editor;
use crate::layout_state::{PaneState, TabContentState, TabState as SavedTabState};
use crate::shortcut_config::{NormalizedShortcut, ResolvedShortcutConfig, ShortcutId};
use crate::terminal::{self, TerminalCallbacks};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

type PaneSplitCallback = dyn Fn(&gtk::Widget, gtk::Orientation);
type PaneWidgetCallback = dyn Fn(&gtk::Widget);
type PaneSignalCallback = dyn Fn();
type PanePathCallback = dyn Fn(&str);
type PaneShortcutStateCallback = dyn Fn() -> Rc<ResolvedShortcutConfig>;
type PaneShortcutCaptureCallback =
    dyn Fn(ShortcutId, Option<NormalizedShortcut>) -> Result<ResolvedShortcutConfig, String>;

pub struct PaneCallbacks {
    pub on_split: Box<PaneSplitCallback>,
    pub on_close_pane: Box<PaneWidgetCallback>,
    pub on_bell: Box<PaneSignalCallback>,
    pub on_open_keybinds: Box<PaneWidgetCallback>,
    pub current_shortcuts: Box<PaneShortcutStateCallback>,
    pub on_capture_shortcut: Rc<PaneShortcutCaptureCallback>,
    pub on_pwd_changed: Box<PanePathCallback>,
    pub on_empty: Box<PaneWidgetCallback>,
    pub on_state_changed: Box<PaneSignalCallback>,
}

#[derive(Clone)]
struct TerminalTabState {
    cwd: Rc<RefCell<Option<String>>>,
    handle: terminal::TerminalHandle,
}

#[derive(Clone)]
pub struct TerminalShortcutTarget {
    handle: terminal::TerminalHandle,
}

impl TerminalShortcutTarget {
    pub fn perform_binding_action(&self, action: &str) -> bool {
        self.handle.perform_binding_action(action)
    }

    pub fn show_find(&self) -> bool {
        self.handle.show_find()
    }

    pub fn find_next(&self) -> bool {
        self.handle.find_next()
    }

    pub fn find_previous(&self) -> bool {
        self.handle.find_previous()
    }

    pub fn hide_find(&self) -> bool {
        self.handle.hide_find()
    }

    pub fn use_selection_for_find(&self) -> bool {
        self.handle.use_selection_for_find()
    }
}

#[derive(Clone)]
struct BrowserTabState {
    uri: Rc<RefCell<Option<String>>>,
    handles: BrowserHandles,
}

#[derive(Clone)]
pub struct BrowserShortcutTarget {
    uri: Rc<RefCell<Option<String>>>,
    handles: BrowserHandles,
}

#[derive(Clone)]
pub enum FocusedShortcutTarget {
    None,
    Terminal(TerminalShortcutTarget),
    Browser(BrowserShortcutTarget),
    Keybinds,
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
"#;

// ---------------------------------------------------------------------------
// PaneWidget builder
// ---------------------------------------------------------------------------

pub fn create_pane(
    callbacks: Rc<PaneCallbacks>,
    shortcuts: Rc<ResolvedShortcutConfig>,
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

    // Tab strip (left side, scrollable)
    let tab_strip = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(0)
        .hexpand(true)
        .build();

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

    let new_term_btn = icon_button(
        "utilities-terminal-symbolic",
        &pane_action_tooltip(
            &shortcuts,
            "New terminal tab",
            Some(ShortcutId::NewTerminal),
        ),
    );
    let new_browser_btn = icon_button(
        "limux-globe-symbolic",
        &pane_action_tooltip(&shortcuts, "New browser tab", None),
    );
    let split_h_btn = icon_button(
        "limux-split-horizontal-symbolic",
        &pane_action_tooltip(&shortcuts, "Split right", Some(ShortcutId::SplitRight)),
    );
    let split_v_btn = icon_button(
        "limux-split-vertical-symbolic",
        &pane_action_tooltip(&shortcuts, "Split down", Some(ShortcutId::SplitDown)),
    );
    let close_btn = icon_button(
        "window-close-symbolic",
        &pane_action_tooltip(&shortcuts, "Close pane", Some(ShortcutId::CloseFocusedPane)),
    );

    actions.append(&new_term_btn);
    actions.append(&new_browser_btn);
    actions.append(&split_h_btn);
    actions.append(&split_v_btn);
    actions.append(&close_btn);

    header.append(&tab_strip);
    header.append(&actions);

    outer.append(&header);
    outer.append(&content_stack);

    // Shared state for tabs
    let tab_state = Rc::new(std::cell::RefCell::new(TabState {
        tabs: Vec::new(),
        active_tab: None,
    }));

    if let Some(saved_state) = initial_state {
        restore_tabs_from_state(
            &tab_strip,
            &content_stack,
            &tab_state,
            &callbacks,
            working_directory,
            &outer,
            saved_state,
        );
    } else {
        add_terminal_tab_inner(
            &tab_strip,
            &content_stack,
            &tab_state,
            &callbacks,
            working_directory,
            &outer,
            None,
        );
    }

    // Wire action buttons
    {
        let ts = tab_strip.clone();
        let cs = content_stack.clone();
        let state = tab_state.clone();
        let cb = callbacks.clone();
        let ow = outer.clone();
        let wd = ws_wd.clone();
        new_term_btn.connect_clicked(move |_| {
            let dir = wd.borrow().clone();
            add_terminal_tab_inner(&ts, &cs, &state, &cb, dir.as_deref(), &ow, None);
        });
    }
    {
        let ts = tab_strip.clone();
        let cs = content_stack.clone();
        let state = tab_state.clone();
        let cb = callbacks.clone();
        let ow = outer.clone();
        new_browser_btn.connect_clicked(move |_| {
            add_browser_tab_inner(&ts, &cs, &state, &cb, &ow, None);
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

    // Store internals on the outer widget so external code can cycle tabs
    let internals = Rc::new(PaneInternals {
        tab_state: tab_state.clone(),
        tab_strip: tab_strip.clone(),
        content_stack: content_stack.clone(),
        pane_outer: outer.clone(),
        callbacks: callbacks.clone(),
        working_directory: ws_wd.clone(),
        new_terminal_button: new_term_btn.clone(),
        split_right_button: split_h_btn.clone(),
        split_down_button: split_v_btn.clone(),
        close_pane_button: close_btn.clone(),
    });
    unsafe {
        outer.set_data("limux-pane-internals", internals);
    }

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
    Terminal { state: TerminalTabState },
    Browser { state: BrowserTabState },
    Keybinds,
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
    tab_state: Rc<std::cell::RefCell<TabState>>,
    tab_strip: gtk::Box,
    content_stack: gtk::Stack,
    pane_outer: gtk::Box,
    callbacks: Rc<PaneCallbacks>,
    working_directory: Rc<std::cell::RefCell<Option<String>>>,
    new_terminal_button: gtk::Button,
    split_right_button: gtk::Button,
    split_down_button: gtk::Button,
    close_pane_button: gtk::Button,
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

fn pane_action_tooltip(
    shortcuts: &ResolvedShortcutConfig,
    base: &str,
    shortcut_id: Option<ShortcutId>,
) -> String {
    shortcut_id
        .map(|id| shortcuts.tooltip_text(id, base))
        .unwrap_or_else(|| base.to_string())
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

struct KeybindsTabOptions<'a> {
    id: Option<&'a str>,
    custom_name: Option<&'a str>,
    pinned: bool,
}

struct KeybindsTabInput<'a> {
    shortcuts: Rc<ResolvedShortcutConfig>,
    on_capture: Rc<PaneShortcutCaptureCallback>,
    options: Option<KeybindsTabOptions<'a>>,
}

fn restore_tabs_from_state(
    tab_strip: &gtk::Box,
    content_stack: &gtk::Stack,
    tab_state: &Rc<RefCell<TabState>>,
    callbacks: &Rc<PaneCallbacks>,
    working_directory: Option<&str>,
    pane_outer: &gtk::Box,
    saved_state: &PaneState,
) {
    if saved_state.tabs.is_empty() {
        add_terminal_tab_inner(
            tab_strip,
            content_stack,
            tab_state,
            callbacks,
            working_directory,
            pane_outer,
            None,
        );
        return;
    }

    for saved_tab in &saved_state.tabs {
        match &saved_tab.content {
            TabContentState::Terminal { cwd } => add_terminal_tab_inner(
                tab_strip,
                content_stack,
                tab_state,
                callbacks,
                cwd.as_deref().or(working_directory),
                pane_outer,
                Some(TerminalTabOptions {
                    id: Some(saved_tab.id.as_str()),
                    custom_name: saved_tab.custom_name.as_deref(),
                    pinned: saved_tab.pinned,
                    cwd: cwd.as_deref().or(working_directory),
                }),
            ),
            TabContentState::Browser { uri } => add_browser_tab_inner(
                tab_strip,
                content_stack,
                tab_state,
                callbacks,
                pane_outer,
                Some(BrowserTabOptions {
                    id: Some(saved_tab.id.as_str()),
                    custom_name: saved_tab.custom_name.as_deref(),
                    pinned: saved_tab.pinned,
                    uri: uri.as_deref(),
                }),
            ),
            TabContentState::Keybinds {} => add_keybind_editor_tab_inner(
                tab_strip,
                content_stack,
                tab_state,
                callbacks,
                pane_outer,
                KeybindsTabInput {
                    shortcuts: (callbacks.current_shortcuts)(),
                    on_capture: callbacks.on_capture_shortcut.clone(),
                    options: Some(KeybindsTabOptions {
                        id: Some(saved_tab.id.as_str()),
                        custom_name: saved_tab.custom_name.as_deref(),
                        pinned: saved_tab.pinned,
                    }),
                },
            ),
        }
    }

    let active_tab_id = saved_state
        .active_tab_id
        .as_deref()
        .filter(|candidate| {
            tab_state
                .borrow()
                .tabs
                .iter()
                .any(|tab| tab.id == *candidate)
        })
        .map(|value| value.to_string())
        .or_else(|| tab_state.borrow().tabs.first().map(|tab| tab.id.clone()));

    if let Some(active_tab_id) = active_tab_id {
        activate_tab(tab_strip, content_stack, tab_state, &active_tab_id);
    }
}

fn add_terminal_tab_inner(
    tab_strip: &gtk::Box,
    content_stack: &gtk::Stack,
    tab_state: &Rc<RefCell<TabState>>,
    callbacks: &Rc<PaneCallbacks>,
    working_directory: Option<&str>,
    pane_outer: &gtk::Box,
    options: Option<TerminalTabOptions<'_>>,
) {
    let tab_id = options
        .as_ref()
        .and_then(|value| value.id.map(|id| id.to_string()))
        .unwrap_or_else(next_tab_id);

    // Tab label button
    let (tab_btn, title_label) = build_tab_button(
        "Terminal",
        &tab_id,
        tab_strip,
        content_stack,
        tab_state,
        callbacks,
        pane_outer,
    );

    // Build Ghostty terminal callbacks for title/bell/close
    let term_cwd = Rc::new(RefCell::new(
        options
            .as_ref()
            .and_then(|value| value.cwd.map(|cwd| cwd.to_string()))
            .or_else(|| working_directory.map(|cwd| cwd.to_string())),
    ));
    let term_callbacks = {
        let tl = title_label.clone();
        let state_for_title = tab_state.clone();
        let tid_for_title = tab_id.clone();
        let cb_bell = callbacks.clone();
        let ts = tab_strip.clone();
        let cs = content_stack.clone();
        let state_for_close = tab_state.clone();
        let tid_for_close = tab_id.clone();
        let cb_close = callbacks.clone();
        let po = pane_outer.clone();
        let cb_state = callbacks.clone();
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
                let cb_pwd = callbacks.clone();
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
                let cb = callbacks.clone();
                let po = pane_outer.clone();
                move || {
                    let w: gtk::Widget = po.clone().upcast();
                    (cb.on_split)(&w, gtk::Orientation::Horizontal);
                }
            }),
            on_split_down: Box::new({
                let cb = callbacks.clone();
                let po = pane_outer.clone();
                move || {
                    let w: gtk::Widget = po.clone().upcast();
                    (cb.on_split)(&w, gtk::Orientation::Vertical);
                }
            }),
            on_open_keybinds: Box::new({
                let cb = callbacks.clone();
                let po = pane_outer.clone();
                move |anchor| {
                    let _ = anchor;
                    let pane_widget: gtk::Widget = po.clone().upcast();
                    (cb.on_open_keybinds)(&pane_widget);
                }
            }),
        }
    };

    let term = terminal::create_terminal(working_directory, term_callbacks);

    let widget: gtk::Widget = term.overlay.clone().upcast();
    content_stack.add_named(&widget, Some(&tab_id));

    {
        let mut ts = tab_state.borrow_mut();
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
                state: TerminalTabState {
                    cwd: term_cwd.clone(),
                    handle: term.handle.clone(),
                },
            },
        });
    }

    if let Some(custom_name) = options.as_ref().and_then(|value| value.custom_name) {
        title_label.set_label(custom_name);
    }
    if options.as_ref().map(|value| value.pinned).unwrap_or(false) {
        if let Some(entry) = tab_state
            .borrow()
            .tabs
            .iter()
            .find(|entry| entry.id == tab_id)
        {
            apply_pin_visuals(&entry.tab_button, true);
        }
    }

    activate_tab(tab_strip, content_stack, tab_state, &tab_id);
    term.overlay.grab_focus();
    if options.is_none() {
        (callbacks.on_state_changed)();
    }
}

fn add_browser_tab_inner(
    tab_strip: &gtk::Box,
    content_stack: &gtk::Stack,
    tab_state: &Rc<RefCell<TabState>>,
    callbacks: &Rc<PaneCallbacks>,
    pane_outer: &gtk::Box,
    options: Option<BrowserTabOptions<'_>>,
) {
    let tab_id = options
        .as_ref()
        .and_then(|value| value.id.map(|id| id.to_string()))
        .unwrap_or_else(next_tab_id);
    let saved_uri = Rc::new(RefCell::new(
        options
            .as_ref()
            .and_then(|value| value.uri.map(|uri| uri.to_string())),
    ));
    let (widget, title, handles) = create_browser_widget(
        options.as_ref().and_then(|value| value.uri),
        saved_uri.clone(),
        callbacks.clone(),
    );

    let (tab_btn, title_label) = build_tab_button(
        &title,
        &tab_id,
        tab_strip,
        content_stack,
        tab_state,
        callbacks,
        pane_outer,
    );

    content_stack.add_named(&widget, Some(&tab_id));

    {
        let mut ts = tab_state.borrow_mut();
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
                state: BrowserTabState {
                    uri: saved_uri.clone(),
                    handles,
                },
            },
        });
    }

    if let Some(custom_name) = options.as_ref().and_then(|value| value.custom_name) {
        title_label.set_label(custom_name);
    }
    if options.as_ref().map(|value| value.pinned).unwrap_or(false) {
        if let Some(entry) = tab_state
            .borrow()
            .tabs
            .iter()
            .find(|entry| entry.id == tab_id)
        {
            apply_pin_visuals(&entry.tab_button, true);
        }
    }

    activate_tab(tab_strip, content_stack, tab_state, &tab_id);
    if options.is_none() {
        (callbacks.on_state_changed)();
    }
}

fn add_keybind_editor_tab_inner(
    tab_strip: &gtk::Box,
    content_stack: &gtk::Stack,
    tab_state: &Rc<RefCell<TabState>>,
    callbacks: &Rc<PaneCallbacks>,
    pane_outer: &gtk::Box,
    input: KeybindsTabInput<'_>,
) {
    let tab_id = input
        .options
        .as_ref()
        .and_then(|value| value.id.map(|id| id.to_string()))
        .unwrap_or_else(next_tab_id);

    let (tab_btn, title_label) = build_tab_button(
        "Keybinds",
        &tab_id,
        tab_strip,
        content_stack,
        tab_state,
        callbacks,
        pane_outer,
    );

    let widget = keybind_editor::build_keybind_editor(&input.shortcuts, input.on_capture);
    content_stack.add_named(&widget, Some(&tab_id));

    {
        let mut ts = tab_state.borrow_mut();
        ts.tabs.push(TabEntry {
            id: tab_id.clone(),
            tab_button: tab_btn,
            title_label: title_label.clone(),
            content: widget,
            custom_name: input
                .options
                .as_ref()
                .and_then(|value| value.custom_name.map(|name| name.to_string())),
            pinned: input
                .options
                .as_ref()
                .map(|value| value.pinned)
                .unwrap_or(false),
            kind: TabKind::Keybinds,
        });
    }

    if let Some(custom_name) = input.options.as_ref().and_then(|value| value.custom_name) {
        title_label.set_label(custom_name);
    }
    if input
        .options
        .as_ref()
        .map(|value| value.pinned)
        .unwrap_or(false)
    {
        if let Some(entry) = tab_state
            .borrow()
            .tabs
            .iter()
            .find(|entry| entry.id == tab_id)
        {
            apply_pin_visuals(&entry.tab_button, true);
        }
    }

    activate_tab(tab_strip, content_stack, tab_state, &tab_id);
    if input.options.is_none() {
        (callbacks.on_state_changed)();
    }
}

// Public wrappers for keyboard shortcut use
#[allow(dead_code)]
pub fn add_terminal_tab_to_pane(pane_widget: &gtk::Widget) {
    if let Some(internals) = find_pane_internals(pane_widget) {
        let dir = internals.working_directory.borrow().clone();
        add_terminal_tab_inner(
            &internals.tab_strip,
            &internals.content_stack,
            &internals.tab_state,
            &internals.callbacks,
            dir.as_deref(),
            &internals.pane_outer,
            None,
        );
    }
}

#[allow(dead_code)]
pub fn add_browser_tab_to_pane(pane_widget: &gtk::Widget) {
    add_browser_tab_to_pane_with_uri(pane_widget, None);
}

#[allow(dead_code)]
pub fn add_browser_tab_to_pane_with_uri(pane_widget: &gtk::Widget, uri: Option<&str>) {
    if let Some(internals) = find_pane_internals(pane_widget) {
        let options = uri.map(|uri| BrowserTabOptions {
            id: None,
            custom_name: None,
            pinned: false,
            uri: Some(uri),
        });
        add_browser_tab_inner(
            &internals.tab_strip,
            &internals.content_stack,
            &internals.tab_state,
            &internals.callbacks,
            &internals.pane_outer,
            options,
        );
    }
}

pub fn add_keybind_editor_tab_to_pane(
    pane_widget: &gtk::Widget,
    shortcuts: Rc<ResolvedShortcutConfig>,
    on_capture: Rc<PaneShortcutCaptureCallback>,
) {
    if let Some(internals) = find_pane_internals(pane_widget) {
        if let Some(existing_id) = internals
            .tab_state
            .borrow()
            .tabs
            .iter()
            .find(|entry| matches!(entry.kind, TabKind::Keybinds))
            .map(|entry| entry.id.clone())
        {
            activate_tab(
                &internals.tab_strip,
                &internals.content_stack,
                &internals.tab_state,
                &existing_id,
            );
            (internals.callbacks.on_state_changed)();
            return;
        }

        add_keybind_editor_tab_inner(
            &internals.tab_strip,
            &internals.content_stack,
            &internals.tab_state,
            &internals.callbacks,
            &internals.pane_outer,
            KeybindsTabInput {
                shortcuts,
                on_capture,
                options: None,
            },
        );
    }
}

pub fn refresh_shortcut_tooltips(pane_widget: &gtk::Widget, shortcuts: &ResolvedShortcutConfig) {
    let Some(internals) = find_pane_internals(pane_widget) else {
        return;
    };

    internals
        .new_terminal_button
        .set_tooltip_text(Some(&pane_action_tooltip(
            shortcuts,
            "New terminal tab",
            Some(ShortcutId::NewTerminal),
        )));
    internals
        .split_right_button
        .set_tooltip_text(Some(&pane_action_tooltip(
            shortcuts,
            "Split right",
            Some(ShortcutId::SplitRight),
        )));
    internals
        .split_down_button
        .set_tooltip_text(Some(&pane_action_tooltip(
            shortcuts,
            "Split down",
            Some(ShortcutId::SplitDown),
        )));
    internals
        .close_pane_button
        .set_tooltip_text(Some(&pane_action_tooltip(
            shortcuts,
            "Close pane",
            Some(ShortcutId::CloseFocusedPane),
        )));
}

pub fn snapshot_pane_state(pane_widget: &gtk::Widget) -> Option<PaneState> {
    let internals = find_pane_internals(pane_widget)?;
    let ts = internals.tab_state.borrow();
    let tabs = ts
        .tabs
        .iter()
        .map(|entry| {
            let content = match &entry.kind {
                TabKind::Terminal { state } => TabContentState::Terminal {
                    cwd: state.cwd.borrow().clone(),
                },
                TabKind::Browser { state } => TabContentState::Browser {
                    uri: state.uri.borrow().clone(),
                },
                TabKind::Keybinds => TabContentState::Keybinds {},
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

pub fn focused_shortcut_target(pane_widget: &gtk::Widget) -> FocusedShortcutTarget {
    let Some(internals) = find_pane_internals(pane_widget) else {
        return FocusedShortcutTarget::None;
    };

    let target = {
        let tab_state = internals.tab_state.borrow();
        let Some(active_id) = tab_state.active_tab.as_deref() else {
            return FocusedShortcutTarget::None;
        };
        match tab_state.tabs.iter().find(|entry| entry.id == active_id) {
            Some(TabEntry {
                kind: TabKind::Terminal { state },
                ..
            }) => FocusedShortcutTarget::Terminal(TerminalShortcutTarget {
                handle: state.handle.clone(),
            }),
            Some(TabEntry {
                kind: TabKind::Browser { state },
                ..
            }) => FocusedShortcutTarget::Browser(BrowserShortcutTarget {
                uri: state.uri.clone(),
                handles: state.handles.clone(),
            }),
            Some(TabEntry {
                kind: TabKind::Keybinds,
                ..
            }) => FocusedShortcutTarget::Keybinds,
            None => FocusedShortcutTarget::None,
        }
    };

    target
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

// ---------------------------------------------------------------------------
// Tab button (label + close)
// ---------------------------------------------------------------------------

fn build_tab_button(
    title: &str,
    tab_id: &str,
    tab_strip: &gtk::Box,
    content_stack: &gtk::Stack,
    tab_state: &Rc<RefCell<TabState>>,
    callbacks: &Rc<PaneCallbacks>,
    pane_outer: &gtk::Box,
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
        let ts = tab_strip.clone();
        let cs = content_stack.clone();
        let state = tab_state.clone();
        let callbacks = callbacks.clone();
        click.connect_pressed(move |_, _, _, _| {
            activate_tab(&ts, &cs, &state, &tid);
            (callbacks.on_state_changed)();
        });
    }
    tab_btn.add_controller(click);

    // Right-click → context menu
    let right_click = gtk::GestureClick::new();
    right_click.set_button(3);
    {
        let tid = tab_id.to_string();
        let ts = tab_strip.clone();
        let cs = content_stack.clone();
        let state = tab_state.clone();
        let cb = callbacks.clone();
        let po = pane_outer.clone();
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

    // Drag source for reorder
    let drag_source = gtk::DragSource::new();
    drag_source.set_actions(gtk::gdk::DragAction::MOVE);
    {
        let tid = tab_id.to_string();
        drag_source.connect_prepare(move |_src, _x, _y| {
            let val = glib::Value::from(&tid);
            Some(gtk::gdk::ContentProvider::for_value(&val))
        });
    }
    tab_btn.add_controller(drag_source);

    // Drop target for reorder
    let drop_target = gtk::DropTarget::new(glib::Type::STRING, gtk::gdk::DragAction::MOVE);
    {
        let tid = tab_id.to_string();
        let ts = tab_strip.clone();
        let state = tab_state.clone();
        let callbacks = callbacks.clone();
        drop_target.connect_drop(move |_, value, _, _| {
            if let Ok(source_id) = value.get::<String>() {
                if source_id != tid {
                    reorder_tab(&ts, &state, &source_id, &tid, &callbacks);
                    return true;
                }
            }
            false
        });
    }
    tab_btn.add_controller(drop_target);

    // Close button click
    {
        let tid = tab_id.to_string();
        let ts = tab_strip.clone();
        let cs = content_stack.clone();
        let state = tab_state.clone();
        let cb = callbacks.clone();
        let po = pane_outer.clone();
        close_btn.connect_clicked(move |_| {
            let is_pinned = state.borrow().tabs.iter().any(|e| e.id == tid && e.pinned);
            if !is_pinned {
                remove_tab(&ts, &cs, &state, &tid, &cb, &po);
            }
        });
    }

    tab_strip.append(&tab_btn);

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

fn reorder_tab(
    tab_strip: &gtk::Box,
    tab_state: &Rc<RefCell<TabState>>,
    source_id: &str,
    target_id: &str,
    callbacks: &Rc<PaneCallbacks>,
) {
    let mut ts = tab_state.borrow_mut();

    let Some(src_idx) = ts.tabs.iter().position(|e| e.id == source_id) else {
        return;
    };
    let Some(tgt_idx) = ts.tabs.iter().position(|e| e.id == target_id) else {
        return;
    };

    // Move the tab entry
    let entry = ts.tabs.remove(src_idx);
    ts.tabs.insert(tgt_idx, entry);

    // Rebuild tab strip order
    // Remove all tab buttons then re-add in order
    let buttons: Vec<gtk::Box> = ts.tabs.iter().map(|e| e.tab_button.clone()).collect();
    drop(ts);

    for btn in &buttons {
        tab_strip.remove(btn);
    }
    for btn in &buttons {
        tab_strip.append(btn);
    }
    (callbacks.on_state_changed)();
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

    content_stack.set_visible_child_name(tab_id);

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
#[derive(Clone)]
struct BrowserHandles {
    webview: webkit6::WebView,
    url_entry: gtk::Entry,
    search_bar: gtk::SearchBar,
    search_entry: gtk::SearchEntry,
    find_controller: webkit6::FindController,
    dom_editable: Rc<Cell<bool>>,
}

#[cfg(not(feature = "webkit"))]
#[derive(Clone)]
struct BrowserHandles;

impl BrowserShortcutTarget {
    pub fn current_uri(&self) -> Option<String> {
        self.uri.borrow().clone()
    }

    pub fn focus_location(&self) -> bool {
        self.handles.focus_location()
    }

    pub fn go_back(&self) -> bool {
        self.handles.go_back()
    }

    pub fn go_forward(&self) -> bool {
        self.handles.go_forward()
    }

    pub fn reload(&self) -> bool {
        self.handles.reload()
    }

    pub fn show_inspector(&self) -> bool {
        self.handles.show_inspector()
    }

    pub fn show_console(&self) -> bool {
        self.handles.show_console()
    }

    pub fn show_find(&self) -> bool {
        self.handles.show_find()
    }

    pub fn find_next(&self) -> bool {
        self.handles.find_next()
    }

    pub fn find_previous(&self) -> bool {
        self.handles.find_previous()
    }

    pub fn hide_find(&self) -> bool {
        self.handles.hide_find()
    }

    pub fn use_selection_for_find(&self) -> bool {
        self.handles.use_selection_for_find()
    }

    pub fn is_find_active(&self) -> bool {
        self.handles.is_find_active()
    }

    pub fn is_page_editable(&self) -> bool {
        self.handles.is_page_editable()
    }
}

#[cfg(feature = "webkit")]
impl BrowserHandles {
    fn is_find_active(&self) -> bool {
        self.search_bar.is_search_mode()
    }

    fn is_page_editable(&self) -> bool {
        self.dom_editable.get()
    }

    fn focus_location(&self) -> bool {
        self.url_entry.grab_focus();
        self.url_entry.select_region(0, -1);
        true
    }

    fn go_back(&self) -> bool {
        self.webview.go_back();
        true
    }

    fn go_forward(&self) -> bool {
        self.webview.go_forward();
        true
    }

    fn reload(&self) -> bool {
        self.webview.reload();
        true
    }

    fn show_inspector(&self) -> bool {
        if let Some(inspector) = self.webview.inspector() {
            inspector.show();
            return true;
        }
        false
    }

    fn show_console(&self) -> bool {
        self.show_inspector()
    }

    fn show_find(&self) -> bool {
        self.search_bar.set_search_mode(true);
        self.search_entry.grab_focus();
        self.search_entry.select_region(0, -1);
        if !self.search_entry.text().is_empty() {
            self.search_for_entry_text();
        }
        true
    }

    fn find_next(&self) -> bool {
        if self.is_find_active() {
            self.find_controller.search_next();
            return true;
        }
        false
    }

    fn find_previous(&self) -> bool {
        if self.is_find_active() {
            self.find_controller.search_previous();
            return true;
        }
        false
    }

    fn hide_find(&self) -> bool {
        if !self.is_find_active() {
            return false;
        }
        self.find_controller.search_finish();
        self.search_bar.set_search_mode(false);
        self.webview.grab_focus();
        true
    }

    fn use_selection_for_find(&self) -> bool {
        let search_entry = self.search_entry.clone();
        let search_bar = self.search_bar.clone();
        let find_controller = self.find_controller.clone();
        let webview = self.webview.clone();
        self.webview.evaluate_javascript(
            "window.getSelection ? window.getSelection().toString() : '';",
            None,
            None,
            None::<&gtk::gio::Cancellable>,
            move |result| {
                let Ok(value) = result else {
                    return;
                };
                let selection = value.to_str();
                if selection.is_empty() {
                    return;
                }
                search_bar.set_search_mode(true);
                search_entry.set_text(selection.as_str());
                find_controller.search(
                    selection.as_str(),
                    webkit6::FindOptions::CASE_INSENSITIVE.bits()
                        | webkit6::FindOptions::WRAP_AROUND.bits(),
                    u32::MAX,
                );
                search_entry.grab_focus();
                search_entry.select_region(0, -1);
                webview.queue_draw();
            },
        );
        true
    }

    fn search_for_entry_text(&self) {
        let query = self.search_entry.text();
        if query.is_empty() {
            self.find_controller.search_finish();
            return;
        }
        self.find_controller.search(
            query.as_str(),
            webkit6::FindOptions::CASE_INSENSITIVE.bits()
                | webkit6::FindOptions::WRAP_AROUND.bits(),
            u32::MAX,
        );
    }
}

#[cfg(not(feature = "webkit"))]
impl BrowserHandles {
    fn is_find_active(&self) -> bool {
        false
    }

    fn is_page_editable(&self) -> bool {
        false
    }

    fn focus_location(&self) -> bool {
        false
    }

    fn go_back(&self) -> bool {
        false
    }

    fn go_forward(&self) -> bool {
        false
    }

    fn reload(&self) -> bool {
        false
    }

    fn show_inspector(&self) -> bool {
        false
    }

    fn show_console(&self) -> bool {
        false
    }

    fn show_find(&self) -> bool {
        false
    }

    fn find_next(&self) -> bool {
        false
    }

    fn find_previous(&self) -> bool {
        false
    }

    fn hide_find(&self) -> bool {
        false
    }

    fn use_selection_for_find(&self) -> bool {
        false
    }
}

#[cfg(feature = "webkit")]
const LIMUX_BROWSER_EDITABLE_STATE_HANDLER: &str = "limuxEditableState";

#[cfg(feature = "webkit")]
const LIMUX_BROWSER_EDITABLE_STATE_SCRIPT: &str = r#"
(() => {
  const handler = globalThis.webkit?.messageHandlers?.limuxEditableState;
  if (!handler || typeof handler.postMessage !== 'function') {
    return;
  }

  const nonTextInputTypes = new Set([
    'button',
    'checkbox',
    'color',
    'file',
    'hidden',
    'image',
    'radio',
    'range',
    'reset',
    'submit'
  ]);

  const isEditableElement = (element) => {
    if (!element) {
      return false;
    }
    if (element.isContentEditable) {
      return true;
    }

    const tagName = (element.tagName || '').toUpperCase();
    if (tagName === 'TEXTAREA') {
      return !element.readOnly && !element.disabled;
    }
    if (tagName === 'SELECT') {
      return !element.disabled;
    }
    if (tagName !== 'INPUT') {
      return false;
    }

    const type = (element.type || '').toLowerCase();
    return !nonTextInputTypes.has(type) && !element.readOnly && !element.disabled;
  };

  const publish = () => {
    handler.postMessage(Boolean(isEditableElement(document.activeElement)));
  };

  publish();
  document.addEventListener('focusin', publish, true);
  document.addEventListener('focusout', () => queueMicrotask(publish), true);
  window.addEventListener('pageshow', publish, true);
})();
"#;

#[cfg(feature = "webkit")]
fn create_browser_widget(
    initial_uri: Option<&str>,
    saved_uri: Rc<RefCell<Option<String>>>,
    callbacks: Rc<PaneCallbacks>,
) -> (gtk::Widget, String, BrowserHandles) {
    use webkit6::prelude::*;

    // Use a NetworkSession to avoid sandbox issues
    let network_session = webkit6::NetworkSession::default();
    let web_context = webkit6::WebContext::default();
    let user_content_manager = webkit6::UserContentManager::new();
    let dom_editable = Rc::new(Cell::new(false));
    let _ = user_content_manager
        .register_script_message_handler(LIMUX_BROWSER_EDITABLE_STATE_HANDLER, None);
    user_content_manager.add_script(&webkit6::UserScript::new(
        LIMUX_BROWSER_EDITABLE_STATE_SCRIPT,
        webkit6::UserContentInjectedFrames::AllFrames,
        webkit6::UserScriptInjectionTime::Start,
        &[],
        &[],
    ));
    {
        let dom_editable = dom_editable.clone();
        user_content_manager.connect_script_message_received(
            Some(LIMUX_BROWSER_EDITABLE_STATE_HANDLER),
            move |_, value| {
                dom_editable.set(if value.is_boolean() {
                    value.to_boolean()
                } else {
                    value.to_str().as_str() == "true"
                });
            },
        );
    }

    let webview = webkit6::WebView::builder()
        .user_content_manager(&user_content_manager)
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

    let find_controller = webview
        .find_controller()
        .expect("webkit webview should expose a find controller");
    let search_entry = gtk::SearchEntry::builder()
        .hexpand(true)
        .placeholder_text("Find in page")
        .build();
    let search_bar = gtk::SearchBar::new();
    search_bar.set_show_close_button(true);
    search_bar.connect_entry(&search_entry);
    search_bar.set_child(Some(&search_entry));
    search_bar.set_key_capture_widget(Some(&webview));
    {
        let search_bar = search_bar.clone();
        let find_controller = find_controller.clone();
        let webview = webview.clone();
        search_entry.connect_stop_search(move |_| {
            find_controller.search_finish();
            search_bar.set_search_mode(false);
            webview.grab_focus();
        });
    }
    {
        let dom_editable = dom_editable.clone();
        webview.connect_load_changed(move |_, _| {
            dom_editable.set(false);
        });
    }

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    vbox.append(&nav_bar);
    vbox.append(&search_bar);
    vbox.append(&webview.clone());
    vbox.set_hexpand(true);
    vbox.set_vexpand(true);

    let browser_handles = BrowserHandles {
        webview: webview.clone(),
        url_entry: url_entry.clone(),
        search_bar: search_bar.clone(),
        search_entry: search_entry.clone(),
        find_controller: find_controller.clone(),
        dom_editable,
    };

    {
        let browser_handles = browser_handles.clone();
        search_entry.connect_search_changed(move |_| {
            browser_handles.search_for_entry_text();
        });
    }

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

    (vbox.upcast(), "Browser".to_string(), browser_handles)
}

#[cfg(not(feature = "webkit"))]
fn create_browser_widget(
    initial_uri: Option<&str>,
    saved_uri: Rc<RefCell<Option<String>>>,
    _callbacks: Rc<PaneCallbacks>,
) -> (gtk::Widget, String, BrowserHandles) {
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

    let handles = BrowserHandles;

    (placeholder.upcast(), "Browser".to_string(), handles)
}

#[cfg(test)]
mod tests {
    use super::pane_action_tooltip;
    use crate::shortcut_config::{default_shortcuts, resolve_shortcuts_from_str, ShortcutId};

    #[test]
    fn pane_action_tooltip_reflects_remaps_and_unbinds() {
        let defaults = default_shortcuts();
        assert_eq!(
            pane_action_tooltip(&defaults, "New terminal tab", Some(ShortcutId::NewTerminal)),
            "New terminal tab (Ctrl+T)"
        );
        assert_eq!(
            pane_action_tooltip(&defaults, "New browser tab", None),
            "New browser tab"
        );

        let remapped = resolve_shortcuts_from_str(
            r#"{
                "shortcuts": {
                    "split_right": "<Ctrl><Alt>d"
                }
            }"#,
        )
        .unwrap();
        assert_eq!(
            pane_action_tooltip(&remapped, "Split right", Some(ShortcutId::SplitRight)),
            "Split right (Ctrl+Alt+D)"
        );

        let unbound = resolve_shortcuts_from_str(
            r#"{
                "shortcuts": {
                    "close_focused_pane": null
                }
            }"#,
        )
        .unwrap();
        assert_eq!(
            pane_action_tooltip(&unbound, "Close pane", Some(ShortcutId::CloseFocusedPane)),
            "Close pane"
        );
    }
}
