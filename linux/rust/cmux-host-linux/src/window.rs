use std::cell::RefCell;
use std::rc::Rc;

use gtk4 as gtk;
use gtk::prelude::*;
use gtk::glib;
use libadwaita as adw;
use adw::prelude::*;

use vte4::TerminalExt;

use crate::terminal;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct Workspace {
    id: String,
    name: String,
    /// The root widget in the content stack for this workspace.
    root: gtk::Widget,
    /// All VTE terminals in this workspace (flat list for focus tracking).
    terminals: Vec<vte4::Terminal>,
    /// The sidebar row widget.
    sidebar_row: gtk::ListBoxRow,
    /// Notification dot in the sidebar row.
    notify_dot: gtk::Label,
    /// Notification message label in the sidebar row.
    notify_label: gtk::Label,
    /// Whether this workspace has unread notifications.
    unread: bool,
    /// Last notification message.
    last_notification: Option<String>,
}

struct AppState {
    workspaces: Vec<Workspace>,
    active_idx: usize,
    next_number: usize,
    // Widgets we need to mutate later
    stack: gtk::Stack,
    sidebar_list: gtk::ListBox,
    paned: gtk::Paned,
}

impl AppState {
    fn active_workspace(&self) -> Option<&Workspace> {
        self.workspaces.get(self.active_idx)
    }

    fn active_workspace_mut(&mut self) -> Option<&mut Workspace> {
        self.workspaces.get_mut(self.active_idx)
    }

    fn focused_terminal(&self) -> Option<&vte4::Terminal> {
        let ws = self.active_workspace()?;
        ws.terminals.iter().find(|t| t.has_focus())
            .or_else(|| ws.terminals.last())
    }
}

type State = Rc<RefCell<AppState>>;

// ---------------------------------------------------------------------------
// CSS
// ---------------------------------------------------------------------------

const CSS: &str = r#"
.cmux-sidebar {
    background-color: rgba(25, 25, 25, 1);
}
.cmux-sidebar-row-box {
    padding: 6px 12px;
    border-radius: 6px;
    margin: 2px 6px;
}
.cmux-sidebar-row-box:selected {
    background-color: rgba(0, 145, 255, 0.25);
}
.cmux-ws-name {
    color: rgba(255, 255, 255, 0.7);
    font-size: 13px;
}
row:selected .cmux-ws-name {
    color: white;
}
.cmux-notify-dot {
    color: #0091FF;
    font-size: 10px;
    margin-right: 6px;
}
.cmux-notify-dot-hidden {
    color: transparent;
    font-size: 10px;
    margin-right: 6px;
}
.cmux-notify-msg {
    color: rgba(255, 255, 255, 0.35);
    font-size: 11px;
}
.cmux-notify-msg-unread {
    color: rgba(0, 145, 255, 0.8);
    font-size: 11px;
}
.cmux-sidebar-title {
    color: rgba(255, 255, 255, 0.5);
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 1px;
}
.cmux-sidebar-btn {
    background: rgba(255, 255, 255, 0.08);
    color: rgba(255, 255, 255, 0.7);
    border: none;
    border-radius: 6px;
    padding: 6px 12px;
    min-height: 0;
}
.cmux-sidebar-btn:hover {
    background: rgba(255, 255, 255, 0.14);
    color: white;
}
.cmux-content {
    background-color: rgba(23, 23, 23, 1);
}
"#;

// ---------------------------------------------------------------------------
// Window construction
// ---------------------------------------------------------------------------

pub fn build_window(app: &adw::Application) {
    // Load CSS
    let provider = gtk::CssProvider::new();
    provider.load_from_data(CSS);
    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("display"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    // Force dark theme
    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("cmux")
        .default_width(1400)
        .default_height(900)
        .build();

    // Header bar
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&gtk::Label::builder().label("cmux").build()));

    // Content stack — one child per workspace
    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::None);
    stack.set_hexpand(true);
    stack.set_vexpand(true);
    stack.add_css_class("cmux-content");

    // Sidebar
    let sidebar_list = gtk::ListBox::new();
    sidebar_list.set_selection_mode(gtk::SelectionMode::Single);
    sidebar_list.add_css_class("navigation-sidebar");

    let sidebar_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .child(&sidebar_list)
        .build();

    let sidebar_title = gtk::Label::builder()
        .label("WORKSPACES")
        .xalign(0.0)
        .margin_start(12)
        .margin_top(8)
        .margin_bottom(4)
        .build();
    sidebar_title.add_css_class("cmux-sidebar-title");

    let new_ws_btn = gtk::Button::builder()
        .label("New Workspace")
        .build();
    new_ws_btn.add_css_class("cmux-sidebar-btn");

    let sidebar = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .width_request(220)
        .build();
    sidebar.add_css_class("cmux-sidebar");
    sidebar.append(&sidebar_title);
    sidebar.append(&sidebar_scroll);
    sidebar.append(&new_ws_btn);

    // Main horizontal split: sidebar | content
    let paned = gtk::Paned::builder()
        .orientation(gtk::Orientation::Horizontal)
        .position(220)
        .shrink_start_child(false)
        .shrink_end_child(false)
        .start_child(&sidebar)
        .end_child(&stack)
        .build();

    // Assemble window
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    vbox.append(&header);
    vbox.append(&paned);
    window.set_content(Some(&vbox));

    // Shared state
    let state: State = Rc::new(RefCell::new(AppState {
        workspaces: Vec::new(),
        active_idx: 0,
        next_number: 1,
        stack: stack.clone(),
        sidebar_list: sidebar_list.clone(),
        paned: paned.clone(),
    }));

    // Register window actions
    register_actions(&window, &state);

    // Sidebar row selection → switch workspace
    {
        let state = state.clone();
        sidebar_list.connect_row_selected(move |_, row| {
            if let Some(row) = row {
                let idx = row.index() as usize;
                switch_workspace(&state, idx);
            }
        });
    }

    // "New Workspace" button
    {
        let state = state.clone();
        new_ws_btn.connect_clicked(move |_| {
            add_workspace(&state, None);
        });
    }

    // Create first workspace
    add_workspace(&state, None);

    window.present();
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

fn register_actions(window: &adw::ApplicationWindow, state: &State) {
    let action_defs: &[&str] = &[
        "new-workspace",
        "close-workspace",
        "close-pane",
        "toggle-sidebar",
        "split-right",
        "split-down",
        "next-workspace",
        "prev-workspace",
    ];

    for name in action_defs {
        let action = gtk::gio::SimpleAction::new(name, None);
        let state = state.clone();
        let handler_name = name.to_string();
        action.connect_activate(move |_, _| {
            match handler_name.as_str() {
                "new-workspace" => add_workspace(&state, None),
                "close-workspace" => close_workspace(&state),
                "close-pane" => close_focused_pane(&state),
                "toggle-sidebar" => toggle_sidebar(&state),
                "split-right" => split(&state, gtk::Orientation::Horizontal),
                "split-down" => split(&state, gtk::Orientation::Vertical),
                "next-workspace" => cycle_workspace(&state, 1),
                "prev-workspace" => cycle_workspace(&state, -1),
                _ => {}
            }
        });
        window.add_action(&action);
    }
}

// ---------------------------------------------------------------------------
// Sidebar row construction
// ---------------------------------------------------------------------------

/// Build a sidebar row with: [dot] [name]
///                            [notification message]
fn build_sidebar_row(name: &str) -> (gtk::ListBoxRow, gtk::Label, gtk::Label, gtk::Label) {
    let notify_dot = gtk::Label::builder()
        .label("\u{25CF}") // ●
        .build();
    notify_dot.add_css_class("cmux-notify-dot-hidden");

    let name_label = gtk::Label::builder()
        .label(name)
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    name_label.add_css_class("cmux-ws-name");

    let top_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    top_row.append(&notify_dot);
    top_row.append(&name_label);

    let notify_label = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .visible(false)
        .margin_start(16) // indent under name
        .build();
    notify_label.add_css_class("cmux-notify-msg");

    let vbox = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .build();
    vbox.add_css_class("cmux-sidebar-row-box");
    vbox.append(&top_row);
    vbox.append(&notify_label);

    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&vbox));

    (row, name_label, notify_dot, notify_label)
}

// ---------------------------------------------------------------------------
// Workspace management
// ---------------------------------------------------------------------------

fn add_workspace(state: &State, working_directory: Option<&str>) {
    let mut s = state.borrow_mut();
    let number = s.next_number;
    s.next_number += 1;

    let id = uuid::Uuid::new_v4().to_string();
    let name = format!("Terminal {number}");
    let stack_name = format!("ws-{id}");

    // Create terminal
    let term = terminal::create_terminal(working_directory);

    // Connect signals for this terminal
    connect_terminal_signals(state, &term, &id);

    let root: gtk::Widget = term.clone().upcast();

    // Add to stack
    s.stack.add_named(&root, Some(&stack_name));

    // Create sidebar row
    let (row, name_label, notify_dot, notify_label) = build_sidebar_row(&name);
    s.sidebar_list.append(&row);

    // Keep reference to the name_label for title updates
    let name_label_for_title = name_label.clone();

    let ws = Workspace {
        id: id.clone(),
        name,
        root,
        terminals: vec![term.clone()],
        sidebar_row: row.clone(),
        notify_dot,
        notify_label,
        unread: false,
        last_notification: None,
    };

    s.workspaces.push(ws);
    let new_idx = s.workspaces.len() - 1;
    s.active_idx = new_idx;

    s.stack.set_visible_child_name(&stack_name);

    let sidebar_list = s.sidebar_list.clone();
    let term_to_focus = s.workspaces[new_idx].terminals.first().cloned();
    drop(s);

    sidebar_list.select_row(Some(&row));

    // Sync terminal title to workspace name
    {
        let state = state.clone();
        let id = id.clone();
        term.connect_window_title_notify(move |t: &vte4::Terminal| {
            if let Some(title) = t.window_title() {
                let title_str: String = title.into();
                if !title_str.is_empty() {
                    let mut s = state.borrow_mut();
                    if let Some(ws) = s.workspaces.iter_mut().find(|w| w.id == id) {
                        ws.name = title_str.clone();
                        name_label_for_title.set_label(&title_str);
                    }
                }
            }
        });
    }

    if let Some(t) = term_to_focus {
        t.grab_focus();
    }
}

/// Connect bell (notification) and eof signals for a terminal.
fn connect_terminal_signals(state: &State, term: &vte4::Terminal, ws_id: &str) {
    // Bell → notification alert
    {
        let state = state.clone();
        let ws_id = ws_id.to_string();
        term.connect_bell(move |_: &vte4::Terminal| {
            let mut s = state.borrow_mut();
            let active_idx = s.active_idx;
            if let Some((idx, ws)) = s.workspaces.iter_mut().enumerate().find(|(_, w)| w.id == ws_id) {
                // Only mark as unread if this is NOT the active workspace
                if idx != active_idx {
                    ws.unread = true;
                    ws.last_notification = Some("Process needs attention".to_string());

                    // Update sidebar visuals
                    ws.notify_dot.remove_css_class("cmux-notify-dot-hidden");
                    ws.notify_dot.add_css_class("cmux-notify-dot");

                    ws.notify_label.set_label("Process needs attention");
                    ws.notify_label.remove_css_class("cmux-notify-msg");
                    ws.notify_label.add_css_class("cmux-notify-msg-unread");
                    ws.notify_label.set_visible(true);
                }
            }
        });
    }

    // Child exit → close terminal/workspace
    {
        let state = state.clone();
        let ws_id = ws_id.to_string();
        term.connect_child_exited(move |t: &vte4::Terminal, _status: i32| {
            let t = t.clone();
            let state = state.clone();
            let ws_id = ws_id.clone();
            glib::idle_add_local_once(move || {
                remove_terminal_from_workspace(&state, &ws_id, &t);
            });
        });
    }
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
        if let Some(root) = s.stack.root() {
            if let Some(window) = root.downcast_ref::<gtk::Window>() {
                drop(s);
                window.close();
            }
        }
        return;
    }

    let new_idx = if idx >= s.workspaces.len() {
        s.workspaces.len() - 1
    } else {
        idx
    };
    s.active_idx = new_idx;

    let stack_name = format!("ws-{}", s.workspaces[new_idx].id);
    s.stack.set_visible_child_name(&stack_name);

    let row = s.workspaces[new_idx].sidebar_row.clone();
    let sidebar_list = s.sidebar_list.clone();
    let term = s.workspaces[new_idx].terminals.first().cloned();
    drop(s);

    sidebar_list.select_row(Some(&row));
    if let Some(t) = term {
        t.grab_focus();
    }
}

/// Close the focused pane (terminal) in the active workspace.
fn close_focused_pane(state: &State) {
    let (ws_id, term) = {
        let s = state.borrow();
        let ws = match s.active_workspace() {
            Some(ws) => ws,
            None => return,
        };
        // If only one terminal, close the whole workspace
        if ws.terminals.len() <= 1 {
            let id = ws.id.clone();
            drop(s);
            close_workspace_by_id(state, &id);
            return;
        }
        let term = s.focused_terminal().cloned();
        (ws.id.clone(), term)
    };
    if let Some(term) = term {
        remove_terminal_from_workspace(state, &ws_id, &term);
    }
}

fn switch_workspace(state: &State, idx: usize) {
    let term = {
        let mut s = state.borrow_mut();
        if idx >= s.workspaces.len() || idx == s.active_idx {
            return;
        }
        s.active_idx = idx;
        let stack_name = format!("ws-{}", s.workspaces[idx].id);
        s.stack.set_visible_child_name(&stack_name);

        // Clear unread state when switching TO this workspace
        let ws = &mut s.workspaces[idx];
        if ws.unread {
            ws.unread = false;
            ws.notify_dot.remove_css_class("cmux-notify-dot");
            ws.notify_dot.add_css_class("cmux-notify-dot-hidden");
            ws.notify_label.remove_css_class("cmux-notify-msg-unread");
            ws.notify_label.add_css_class("cmux-notify-msg");
        }

        s.workspaces[idx].terminals.first().cloned()
    };

    if let Some(t) = term {
        t.grab_focus();
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
        let row = s.workspaces[new_idx].sidebar_row.clone();
        let sidebar_list = s.sidebar_list.clone();
        (new_idx, row, sidebar_list)
    };
    switch_workspace(state, new_idx);
    sidebar_list.select_row(Some(&row));
}

fn toggle_sidebar(state: &State) {
    let s = state.borrow();
    let start = s.paned.start_child();
    if let Some(sidebar) = start {
        let visible = sidebar.is_visible();
        sidebar.set_visible(!visible);
    }
}

// ---------------------------------------------------------------------------
// Split panes
// ---------------------------------------------------------------------------

fn split(state: &State, orientation: gtk::Orientation) {
    let mut s = state.borrow_mut();
    let ws = match s.active_workspace_mut() {
        Some(ws) => ws,
        None => return,
    };

    let focused_idx = ws.terminals.iter().position(|t| t.has_focus())
        .unwrap_or(ws.terminals.len().saturating_sub(1));

    let focused_term = match ws.terminals.get(focused_idx) {
        Some(t) => t.clone(),
        None => return,
    };

    let new_term = terminal::create_terminal(None);

    // Connect signals for the new terminal
    let ws_id = ws.id.clone();
    // We can't call connect_terminal_signals while s is borrowed, so do it inline
    {
        let state_clone = Rc::clone(state);
        let ws_id2 = ws_id.clone();
        new_term.connect_bell(move |_: &vte4::Terminal| {
            let mut s = state_clone.borrow_mut();
            let active_idx = s.active_idx;
            if let Some((idx, ws)) = s.workspaces.iter_mut().enumerate().find(|(_, w)| w.id == ws_id2) {
                if idx != active_idx {
                    ws.unread = true;
                    ws.last_notification = Some("Process needs attention".to_string());
                    ws.notify_dot.remove_css_class("cmux-notify-dot-hidden");
                    ws.notify_dot.add_css_class("cmux-notify-dot");
                    ws.notify_label.set_label("Process needs attention");
                    ws.notify_label.remove_css_class("cmux-notify-msg");
                    ws.notify_label.add_css_class("cmux-notify-msg-unread");
                    ws.notify_label.set_visible(true);
                }
            }
        });
    }
    {
        let state_clone = Rc::clone(state);
        let ws_id2 = ws_id.clone();
        new_term.connect_child_exited(move |t: &vte4::Terminal, _status: i32| {
            let t = t.clone();
            let state_clone = state_clone.clone();
            let ws_id2 = ws_id2.clone();
            glib::idle_add_local_once(move || {
                remove_terminal_from_workspace(&state_clone, &ws_id2, &t);
            });
        });
    }

    let parent = focused_term.parent();

    let new_paned = gtk::Paned::builder()
        .orientation(orientation)
        .hexpand(true)
        .vexpand(true)
        .build();

    if let Some(parent) = parent {
        if let Some(paned_parent) = parent.downcast_ref::<gtk::Paned>() {
            let is_start = paned_parent.start_child()
                .map(|c| c == focused_term.clone().upcast::<gtk::Widget>())
                .unwrap_or(false);

            if is_start {
                paned_parent.set_start_child(Some(&new_paned));
            } else {
                paned_parent.set_end_child(Some(&new_paned));
            }
        } else if let Some(stack) = parent.downcast_ref::<gtk::Stack>() {
            let page_name = format!("ws-{}", ws_id);
            stack.remove(&focused_term);
            stack.add_named(&new_paned, Some(&page_name));
            stack.set_visible_child_name(&page_name);
            ws.root = new_paned.clone().upcast();
        }
    }

    new_paned.set_start_child(Some(&focused_term));
    new_paned.set_end_child(Some(&new_term));

    // Set split position to 50% after layout
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

    ws.terminals.push(new_term.clone());

    drop(s);
    new_term.grab_focus();
}

fn remove_terminal_from_workspace(state: &State, ws_id: &str, term: &vte4::Terminal) {
    let mut s = state.borrow_mut();
    let ws = match s.workspaces.iter_mut().find(|w| w.id == ws_id) {
        Some(ws) => ws,
        None => return,
    };

    ws.terminals.retain(|t| t != term);

    if ws.terminals.is_empty() {
        let id = ws.id.clone();
        drop(s);
        close_workspace_by_id(state, &id);
        return;
    }

    let term_widget: gtk::Widget = term.clone().upcast();
    if let Some(parent) = term_widget.parent() {
        if let Some(paned) = parent.downcast_ref::<gtk::Paned>() {
            let sibling = if paned.start_child()
                .map(|c| c == term_widget)
                .unwrap_or(false)
            {
                paned.end_child()
            } else {
                paned.start_child()
            };

            if let Some(sibling) = sibling {
                if let Some(grandparent) = paned.parent() {
                    paned.set_start_child(gtk::Widget::NONE);
                    paned.set_end_child(gtk::Widget::NONE);

                    if let Some(gp_paned) = grandparent.downcast_ref::<gtk::Paned>() {
                        let is_start = gp_paned.start_child()
                            .map(|c| c == paned.clone().upcast::<gtk::Widget>())
                            .unwrap_or(false);
                        if is_start {
                            gp_paned.set_start_child(Some(&sibling));
                        } else {
                            gp_paned.set_end_child(Some(&sibling));
                        }
                    } else if let Some(stack) = grandparent.downcast_ref::<gtk::Stack>() {
                        let page_name = format!("ws-{}", ws.id);
                        stack.remove(paned);
                        stack.add_named(&sibling, Some(&page_name));
                        stack.set_visible_child_name(&page_name);
                        ws.root = sibling.clone();
                    }
                }
            }
        }
    }

    if let Some(t) = ws.terminals.first() {
        let t = t.clone();
        drop(s);
        t.grab_focus();
    }
}
