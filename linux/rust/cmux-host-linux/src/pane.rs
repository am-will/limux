//! PaneWidget: a tabbed container with action icons in the tab bar.
//!
//! Layout: [tab1 x] [tab2 x] ... ←spacer→ [terminal] [browser] [split-h] [split-v] [close]
//!
//! All on one line. Tabs left-justified, icons right-justified.

use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;

use crate::terminal::{self, TerminalCallbacks};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

pub struct PaneCallbacks {
    pub on_split: Box<dyn Fn(&gtk::Widget, gtk::Orientation)>,
    pub on_close_pane: Box<dyn Fn(&gtk::Widget)>,
    pub on_bell: Box<dyn Fn()>,
    pub on_pwd_changed: Box<dyn Fn(&str)>,
    pub on_empty: Box<dyn Fn(&gtk::Widget)>,
}

// ---------------------------------------------------------------------------
// CSS
// ---------------------------------------------------------------------------

pub const PANE_CSS: &str = r#"
.cmux-pane-header {
    background-color: rgba(30, 30, 30, 1);
    border-bottom: 1px solid rgba(255, 255, 255, 0.06);
    min-height: 30px;
    padding: 0 2px;
}
.cmux-tab {
    background: none;
    border: none;
    border-radius: 4px 4px 0 0;
    padding: 4px 4px 4px 10px;
    color: rgba(255, 255, 255, 0.45);
    min-height: 0;
    font-size: 12px;
}
.cmux-tab:hover {
    color: rgba(255, 255, 255, 0.7);
    background: rgba(255, 255, 255, 0.04);
}
.cmux-tab-active {
    color: white;
    background: rgba(255, 255, 255, 0.08);
}
.cmux-tab-close {
    background: none;
    border: none;
    border-radius: 3px;
    padding: 1px;
    min-height: 0;
    min-width: 0;
    color: rgba(255, 255, 255, 0.25);
    margin-left: 4px;
}
.cmux-tab-close:hover {
    color: rgba(255, 255, 255, 0.8);
    background: rgba(255, 255, 255, 0.1);
}
.cmux-pane-action {
    background: none;
    border: none;
    border-radius: 4px;
    padding: 4px 5px;
    min-height: 0;
    min-width: 0;
    color: rgba(255, 255, 255, 0.35);
}
.cmux-pane-action:hover {
    background: rgba(255, 255, 255, 0.08);
    color: rgba(255, 255, 255, 0.8);
}
.cmux-split-icon {
    border: 1px solid rgba(255, 255, 255, 0.4);
    border-radius: 2px;
    min-width: 16px;
    min-height: 12px;
    padding: 0;
}
.cmux-split-icon:hover {
    border-color: rgba(255, 255, 255, 0.8);
}
.cmux-split-half-v {
    min-width: 6px;
    min-height: 10px;
}
.cmux-split-half-h {
    min-width: 14px;
    min-height: 4px;
}
.cmux-split-btn {
    background: none;
    border: none;
    border-radius: 4px;
    padding: 4px 5px;
    min-height: 0;
    min-width: 0;
}
.cmux-split-btn:hover {
    background: rgba(255, 255, 255, 0.08);
}
.cmux-pin-icon {
    font-size: 9px;
    margin-right: 2px;
}
.cmux-tab-rename-entry {
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

pub fn create_pane(callbacks: Rc<PaneCallbacks>, working_directory: Option<&str>) -> gtk::Box {
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
    header.add_css_class("cmux-pane-header");

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

    let new_term_btn = icon_button("utilities-terminal-symbolic", "New terminal tab");
    let new_browser_btn = icon_button("cmux-globe-symbolic", "New browser tab");
    let split_h_btn = icon_button("cmux-split-horizontal-symbolic", "Split right");
    let split_v_btn = icon_button("cmux-split-vertical-symbolic", "Split down");
    let close_btn = icon_button("window-close-symbolic", "Close pane");

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

    // Add first terminal tab
    add_terminal_tab_inner(
        &tab_strip,
        &content_stack,
        &tab_state,
        &callbacks,
        working_directory,
        &outer,
    );

    // Wire action buttons
    {
        let ts = tab_strip.clone();
        let cs = content_stack.clone();
        let state = tab_state.clone();
        let cb = callbacks.clone();
        let ow = outer.clone();
        new_term_btn.connect_clicked(move |_| {
            add_terminal_tab_inner(&ts, &cs, &state, &cb, None, &ow);
        });
    }
    {
        let ts = tab_strip.clone();
        let cs = content_stack.clone();
        let state = tab_state.clone();
        let cb = callbacks.clone();
        let ow = outer.clone();
        new_browser_btn.connect_clicked(move |_| {
            add_browser_tab_inner(&ts, &cs, &state, &cb, &ow);
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

    outer
}

// ---------------------------------------------------------------------------
// Internal tab state
// ---------------------------------------------------------------------------

struct TabEntry {
    id: String,
    tab_button: gtk::Box,
    #[allow(dead_code)]
    title_label: gtk::Label,
    content: gtk::Widget,
    custom_name: Option<String>,
    pinned: bool,
}

struct TabState {
    tabs: Vec<TabEntry>,
    active_tab: Option<String>,
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
    btn.add_css_class("cmux-pane-action");
    btn
}

/// Create a split-pane icon button with two rectangles separated by a divider.
/// Horizontal = left|right panes, Vertical = top/bottom panes.
#[allow(dead_code)]
fn split_icon_button(orientation: gtk::Orientation, tooltip: &str) -> gtk::Button {
    let icon = gtk::Box::new(orientation, 1);
    icon.add_css_class("cmux-split-icon");

    let (class_name, count) = match orientation {
        gtk::Orientation::Horizontal => ("cmux-split-half-v", 2),
        _ => ("cmux-split-half-h", 2),
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
    btn.add_css_class("cmux-split-btn");
    btn
}

// ---------------------------------------------------------------------------
// Tab creation
// ---------------------------------------------------------------------------

fn add_terminal_tab_inner(
    tab_strip: &gtk::Box,
    content_stack: &gtk::Stack,
    tab_state: &Rc<std::cell::RefCell<TabState>>,
    callbacks: &Rc<PaneCallbacks>,
    working_directory: Option<&str>,
    pane_outer: &gtk::Box,
) {
    let tab_id = next_tab_id();

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
                    (cb_pwd.on_pwd_changed)(pwd);
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
        }
    };

    let term = terminal::create_terminal(working_directory, term_callbacks);

    let widget: gtk::Widget = term.clone().upcast();
    content_stack.add_named(&widget, Some(&tab_id));

    {
        let mut ts = tab_state.borrow_mut();
        ts.tabs.push(TabEntry {
            id: tab_id.clone(),
            tab_button: tab_btn,
            title_label,
            content: widget,
            custom_name: None,
            pinned: false,
        });
    }

    activate_tab(tab_strip, content_stack, tab_state, &tab_id);
    term.grab_focus();
}

fn add_browser_tab_inner(
    tab_strip: &gtk::Box,
    content_stack: &gtk::Stack,
    tab_state: &Rc<std::cell::RefCell<TabState>>,
    callbacks: &Rc<PaneCallbacks>,
    pane_outer: &gtk::Box,
) {
    let tab_id = next_tab_id();
    let (widget, title) = create_browser_widget();

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
            title_label,
            content: widget,
            custom_name: None,
            pinned: false,
        });
    }

    activate_tab(tab_strip, content_stack, tab_state, &tab_id);
}

// Public wrappers for keyboard shortcut use
#[allow(dead_code)]
pub fn add_terminal_tab_to_notebook(
    pane_widget: &gtk::Widget,
    callbacks: &Rc<PaneCallbacks>,
    working_directory: Option<&str>,
) {
    if let Some((header, content_stack, tab_state, pane_outer)) = find_pane_internals(pane_widget) {
        if let Some(tab_strip) = find_tab_strip(&header) {
            add_terminal_tab_inner(
                &tab_strip,
                &content_stack,
                &tab_state,
                callbacks,
                working_directory,
                &pane_outer,
            );
        }
    }
}

#[allow(dead_code)]
pub fn add_browser_tab_to_pane(pane_widget: &gtk::Widget, callbacks: &Rc<PaneCallbacks>) {
    if let Some((header, content_stack, tab_state, pane_outer)) = find_pane_internals(pane_widget) {
        if let Some(tab_strip) = find_tab_strip(&header) {
            add_browser_tab_inner(
                &tab_strip,
                &content_stack,
                &tab_state,
                callbacks,
                &pane_outer,
            );
        }
    }
}

#[allow(dead_code)]
fn find_pane_internals(
    _pane_widget: &gtk::Widget,
) -> Option<(
    gtk::Box,
    gtk::Stack,
    Rc<std::cell::RefCell<TabState>>,
    gtk::Box,
)> {
    // For now, this is not easily retrievable since we don't store references.
    // The toolbar buttons wire directly. Keyboard shortcuts go through the
    // create_pane closures. This is a TODO for later refactoring.
    None
}

#[allow(dead_code)]
fn find_tab_strip(header: &gtk::Box) -> Option<gtk::Box> {
    let mut child = header.first_child();
    while let Some(c) = child {
        if let Ok(bx) = c.clone().downcast::<gtk::Box>() {
            if bx.hexpands() {
                return Some(bx);
            }
        }
        child = c.next_sibling();
    }
    None
}

// ---------------------------------------------------------------------------
// Tab button (label + close)
// ---------------------------------------------------------------------------

fn build_tab_button(
    title: &str,
    tab_id: &str,
    tab_strip: &gtk::Box,
    content_stack: &gtk::Stack,
    tab_state: &Rc<std::cell::RefCell<TabState>>,
    callbacks: &Rc<PaneCallbacks>,
    pane_outer: &gtk::Box,
) -> (gtk::Box, gtk::Label) {
    let pin_icon = gtk::Label::new(None);
    pin_icon.add_css_class("cmux-pin-icon");
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
    close_btn.add_css_class("cmux-tab-close");

    let inner_box = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    inner_box.set_can_target(false); // pass events through
    inner_box.append(&pin_icon);
    inner_box.append(&label);

    // Use an overlay approach: the tab_btn is the event target,
    // inner_box + close_btn are children
    let tab_btn = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    tab_btn.add_css_class("cmux-tab");
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
        click.connect_pressed(move |_, _, _, _| {
            activate_tab(&ts, &cs, &state, &tid);
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
        right_click.connect_pressed(move |_gesture, _, _x, _y| {
            show_tab_context_menu(&tb, &tid, &ts, &cs, &state, &cb, &po, &lbl, &pin);
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
        drop_target.connect_drop(move |_, value, _, _| {
            if let Ok(source_id) = value.get::<String>() {
                if source_id != tid {
                    reorder_tab(&ts, &state, &source_id, &tid);
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

fn show_tab_context_menu(
    tab_btn: &gtk::Box,
    tab_id: &str,
    tab_strip: &gtk::Box,
    content_stack: &gtk::Stack,
    tab_state: &Rc<std::cell::RefCell<TabState>>,
    callbacks: &Rc<PaneCallbacks>,
    pane_outer: &gtk::Box,
    label: &gtk::Label,
    pin_icon: &gtk::Label,
) {
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
        let lbl = label.clone();
        let state = tab_state.clone();
        let tid = tab_id.to_string();
        let menu_ref = menu.clone();
        rename_btn.connect_clicked(move |_| {
            menu_ref.popdown();
            show_rename_dialog(&lbl, &state, &tid);
        });
    }

    // Pin / Unpin
    let is_pinned = tab_state
        .borrow()
        .tabs
        .iter()
        .any(|e| e.id == tab_id && e.pinned);
    let pin_label = if is_pinned { "Unpin" } else { "Pin" };
    let pin_btn = gtk::Button::with_label(pin_label);
    pin_btn.add_css_class("flat");
    {
        let state = tab_state.clone();
        let tid = tab_id.to_string();
        let pin = pin_icon.clone();
        let close = tab_btn.last_child(); // close button
        let menu_ref = menu.clone();
        pin_btn.connect_clicked(move |_| {
            menu_ref.popdown();
            let mut ts = state.borrow_mut();
            if let Some(entry) = ts.find_tab_mut(&tid) {
                entry.pinned = !entry.pinned;
                pin.set_label(if entry.pinned { "📌" } else { "" });
                pin.set_visible(entry.pinned);
                // Hide close button on pinned tabs
                if let Some(close_widget) = &close {
                    close_widget.set_visible(!entry.pinned);
                }
            }
        });
    }

    // Close
    let close_btn = gtk::Button::with_label("Close");
    close_btn.add_css_class("flat");
    {
        let tid = tab_id.to_string();
        let ts = tab_strip.clone();
        let cs = content_stack.clone();
        let state = tab_state.clone();
        let cb = callbacks.clone();
        let po = pane_outer.clone();
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
    {
        let _tb = tab_btn.clone();
        menu.connect_closed(move |popover| {
            popover.unparent();
        });
    }

    menu.popup();
}

fn show_rename_dialog(
    label: &gtk::Label,
    tab_state: &Rc<std::cell::RefCell<TabState>>,
    tab_id: &str,
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
    entry.add_css_class("cmux-tab-rename-entry");

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
    tab_state: &Rc<std::cell::RefCell<TabState>>,
    source_id: &str,
    target_id: &str,
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
    let insert_at = if src_idx < tgt_idx { tgt_idx } else { tgt_idx };
    ts.tabs.insert(insert_at, entry);

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
}

// ---------------------------------------------------------------------------
// Tab activation / removal
// ---------------------------------------------------------------------------

fn activate_tab(
    _tab_strip: &gtk::Box,
    content_stack: &gtk::Stack,
    tab_state: &Rc<std::cell::RefCell<TabState>>,
    tab_id: &str,
) {
    let mut ts = tab_state.borrow_mut();
    ts.active_tab = Some(tab_id.to_string());

    // Update visual state on all tabs
    for entry in &ts.tabs {
        if entry.id == tab_id {
            entry.tab_button.add_css_class("cmux-tab-active");
        } else {
            entry.tab_button.remove_css_class("cmux-tab-active");
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
    tab_state: &Rc<std::cell::RefCell<TabState>>,
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
}

// ---------------------------------------------------------------------------
// Browser widget
// ---------------------------------------------------------------------------

#[cfg(feature = "webkit")]
fn create_browser_widget() -> (gtk::Widget, String) {
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
    nav_bar.add_css_class("cmux-pane-header");
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
        webview.connect_uri_notify(move |wv| {
            if let Some(uri) = wv.uri() {
                let uri_str: String = uri.into();
                entry.set_text(&uri_str);
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
        vbox.connect_map(move |_| {
            if !loaded.get() {
                loaded.set(true);
                wv.load_uri("https://google.com");
            }
        });
    }

    // Suppress unused variable warnings
    let _ = network_session;
    let _ = web_context;

    (vbox.upcast(), "Browser".to_string())
}

#[cfg(not(feature = "webkit"))]
fn create_browser_widget() -> (gtk::Widget, String) {
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
