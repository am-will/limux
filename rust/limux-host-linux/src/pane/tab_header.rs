use super::*;

// ---------------------------------------------------------------------------
// Tab button (label + close)
// ---------------------------------------------------------------------------

pub(super) fn new_tab_title_label(title: &str) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(title)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(20)
        .build();
    label.set_can_target(false);
    label
}

pub(super) fn build_tab_button(
    title: &str,
    tab_id: &str,
    internals: &Rc<PaneInternals>,
) -> (gtk::Box, gtk::Label, gtk::Label, TabActivity) {
    let label = new_tab_title_label(title);
    let activity = TabActivity::new(&(internals.callbacks.current_config)().borrow().interface);
    let (tab_button, unread_dot) =
        build_tab_button_from_label(&label, tab_id, internals, &activity);
    (tab_button, label, unread_dot, activity)
}

pub(super) fn build_tab_button_from_label(
    label: &gtk::Label,
    tab_id: &str,
    internals: &Rc<PaneInternals>,
    activity: &TabActivity,
) -> (gtk::Box, gtk::Label) {
    if let Some(parent) = label
        .parent()
        .and_then(|parent| parent.downcast::<gtk::Box>().ok())
    {
        parent.remove(label);
    }

    let pin_icon = gtk::Label::new(None);
    pin_icon.add_css_class("limux-pin-icon");
    pin_icon.set_visible(false);
    pin_icon.set_can_target(false);

    let unread_dot = gtk::Label::new(Some("\u{25CF}"));
    unread_dot.add_css_class("limux-tab-unread-dot");
    unread_dot.set_visible(false);
    unread_dot.set_can_target(false);

    let close_btn = gtk::Button::builder()
        .icon_name("window-close-symbolic")
        .has_frame(false)
        .valign(gtk::Align::Center)
        .build();
    close_btn.add_css_class("limux-tab-close");

    let inner_box = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    inner_box.set_can_target(false);
    inner_box.append(&pin_icon);
    inner_box.append(&unread_dot);
    if let Some(parent) = activity
        .widget()
        .parent()
        .and_then(|parent| parent.downcast::<gtk::Box>().ok())
    {
        parent.remove(activity.widget());
    }
    inner_box.append(activity.widget());
    inner_box.append(label);

    let tab_btn = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    tab_btn.add_css_class("limux-tab");
    tab_btn.append(&inner_box);
    tab_btn.append(&close_btn);

    let click = gtk::GestureClick::new();
    click.set_button(1);
    {
        let tab_id = tab_id.to_string();
        let tab_strip = internals.tab_strip.clone();
        let content_stack = internals.content_stack.clone();
        let tab_state = internals.tab_state.clone();
        let callbacks = internals.callbacks.clone();
        let pane_widget = internals.pane_outer.downgrade();
        let tab_button = tab_btn.downgrade();
        let label = label.clone();
        click.connect_pressed(move |gesture, n_press, _, _| {
            let Some(tab_button) = tab_button.upgrade() else {
                return;
            };
            if handle_tab_interaction_while_renaming(&tab_button, &tab_state) {
                gesture.set_state(gtk::EventSequenceState::Denied);
                return;
            }
            activate_tab(&tab_strip, &content_stack, &tab_state, &tab_id);
            if let Some(pane_widget) = pane_widget.upgrade() {
                clear_tab_unread_if_visible(
                    &tab_state,
                    &tab_id,
                    pane_widget.upcast_ref(),
                    &callbacks,
                );
            }
            (callbacks.on_state_changed)();
            if n_press == 2 {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                let tab_strip = tab_strip.clone();
                let label = label.clone();
                let tab_state = tab_state.clone();
                let tab_id = tab_id.clone();
                let callbacks = callbacks.clone();
                glib::idle_add_local_once(move || {
                    show_rename_dialog(&tab_strip, &label, &tab_state, &tab_id, &callbacks);
                });
            }
        });
    }
    tab_btn.add_controller(click);

    let right_click = gtk::GestureClick::new();
    right_click.set_button(3);
    {
        let tab_id = tab_id.to_string();
        let context = TabContextMenuContext {
            tab_strip: internals.tab_strip.clone(),
            content_stack: internals.content_stack.clone(),
            tab_state: internals.tab_state.clone(),
            callbacks: internals.callbacks.clone(),
            pane_outer: internals.pane_outer.clone(),
            label: label.clone(),
            pin_icon: pin_icon.clone(),
        };
        let tab_button = tab_btn.downgrade();
        let tab_state = internals.tab_state.clone();
        right_click.connect_pressed(move |gesture, _, _, _| {
            let Some(tab_button) = tab_button.upgrade() else {
                return;
            };
            if handle_tab_interaction_while_renaming(&tab_button, &tab_state) {
                gesture.set_state(gtk::EventSequenceState::Denied);
                return;
            }
            show_tab_context_menu(&tab_button, &tab_id, &context);
            gesture.set_state(gtk::EventSequenceState::Claimed);
        });
    }
    tab_btn.add_controller(right_click);

    // Middle-click to close the tab (on release inside bounds, unless pinned or renaming).
    let middle_click = gtk::GestureClick::new();
    middle_click.set_button(2);
    {
        let tab_button = tab_btn.downgrade();
        let tab_state = internals.tab_state.clone();
        middle_click.connect_pressed(move |gesture, _, _, _| {
            let Some(tab_button) = tab_button.upgrade() else {
                return;
            };
            if handle_tab_interaction_while_renaming(&tab_button, &tab_state) {
                gesture.set_state(gtk::EventSequenceState::Denied);
                return;
            }
            gesture.set_state(gtk::EventSequenceState::Claimed);
        });
    }
    {
        let tab_id = tab_id.to_string();
        let tab_state = internals.tab_state.clone();
        let pane_outer = internals.pane_outer.downgrade();
        let tab_button = tab_btn.downgrade();
        middle_click.connect_released(move |_, _, x, y| {
            let Some(tab_button) = tab_button.upgrade() else {
                return;
            };
            let Some(pane_outer) = pane_outer.upgrade() else {
                return;
            };
            let is_renaming = tab_rename_active(&tab_state);
            let inside_bounds = tab_button.contains(x, y);
            let is_pinned = tab_state
                .borrow()
                .tabs
                .iter()
                .any(|entry| entry.id == tab_id && entry.pinned);
            if can_middle_click_close_tab(is_pinned, is_renaming, inside_bounds) {
                close_tab_in_pane(pane_outer.upcast_ref(), &tab_id);
            }
        });
    }
    tab_btn.add_controller(middle_click);

    let drag_source = gtk::DragSource::new();
    drag_source.set_actions(gtk::gdk::DragAction::MOVE);
    {
        let tab_id = tab_id.to_string();
        let pane_id = internals.pane_id;
        let tab_state = internals.tab_state.clone();
        drag_source.connect_prepare(move |_src, _x, _y| {
            if tab_rename_active(&tab_state) {
                return None;
            }
            let payload = glib::Value::from(&TabDragPayload::new(pane_id, &tab_id).encode());
            Some(gtk::gdk::ContentProvider::for_value(&payload))
        });
    }
    {
        let drop_indicator = internals.drop_indicator.clone();
        let tab_state = internals.tab_state.clone();
        drag_source.connect_drag_begin(move |source, _drag| {
            set_tab_dragging(true);
            if let Some((widget, bounds)) = source.widget().and_then(|widget| {
                let parent = drop_indicator.parent()?;
                let bounds = widget.compute_bounds(&parent)?;
                Some((widget, bounds))
            }) {
                position_indicator(
                    &tab_state,
                    &drop_indicator,
                    (bounds.x() + bounds.width()) as f64,
                );
                let icon = gtk::WidgetPaintable::new(Some(&widget));
                source.set_icon(Some(&icon), 0, 0);
            }
        });
    }
    {
        let drop_indicator = internals.drop_indicator.clone();
        let content_overlay = internals.content_drop_overlay.clone();
        drag_source.connect_drag_end(move |_, _, _| {
            set_tab_dragging(false);
            drop_indicator.set_visible(false);
            clear_content_drop_zone(&content_overlay);
        });
    }
    tab_btn.add_controller(drag_source);

    {
        let tab_id = tab_id.to_string();
        let tab_strip = internals.tab_strip.clone();
        let content_stack = internals.content_stack.clone();
        let tab_state = internals.tab_state.clone();
        let callbacks = internals.callbacks.clone();
        let pane_outer = internals.pane_outer.clone();
        close_btn.connect_clicked(move |_| {
            let is_pinned = tab_state
                .borrow()
                .tabs
                .iter()
                .any(|entry| entry.id == tab_id && entry.pinned);
            if !is_pinned {
                remove_tab(
                    &tab_strip,
                    &content_stack,
                    &tab_state,
                    &tab_id,
                    &callbacks,
                    &pane_outer,
                    PaneEmptyReason::ClosedLastTab,
                );
            }
        });
    }

    (tab_btn, unread_dot)
}

pub(super) fn can_middle_click_close_tab(
    is_pinned: bool,
    is_renaming: bool,
    inside_bounds: bool,
) -> bool {
    !is_pinned && !is_renaming && inside_bounds
}

pub(super) fn show_tab_context_menu(
    tab_btn: &gtk::Box,
    tab_id: &str,
    context: &TabContextMenuContext,
) {
    let menu = gtk::PopoverMenu::from_model(None::<&gtk::gio::MenuModel>);
    let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    menu_box.set_margin_top(4);
    menu_box.set_margin_bottom(4);
    menu_box.set_margin_start(4);
    menu_box.set_margin_end(4);

    // Mark as Unread / Read
    let is_unread = context
        .tab_state
        .borrow()
        .tabs
        .iter()
        .any(|e| e.id == tab_id && e.unread);
    let unread_label = if is_unread {
        "Mark as Read"
    } else {
        "Mark as Unread"
    };
    let unread_btn = gtk::Button::with_label(unread_label);
    unread_btn.add_css_class("flat");
    {
        let state = context.tab_state.clone();
        let tid = tab_id.to_string();
        let menu_ref = menu.downgrade();
        let callbacks = context.callbacks.clone();
        unread_btn.connect_clicked(move |_| {
            if let Some(menu) = menu_ref.upgrade() {
                menu.popdown();
            }
            let state = state.clone();
            let tid = tid.clone();
            let callbacks = callbacks.clone();
            glib::idle_add_local_once(move || {
                let mut changed = false;
                if let Some(entry) = state.borrow_mut().find_tab_mut(&tid) {
                    changed = set_tab_manual_unread(entry, !is_unread);
                }
                if changed {
                    (callbacks.on_unread_changed)();
                    (callbacks.on_state_changed)();
                }
            });
        });
    }

    // Rename
    let rename_btn = gtk::Button::with_label("Rename");
    rename_btn.add_css_class("flat");
    {
        let lbl = context.label.clone();
        let state = context.tab_state.clone();
        let tid = tab_id.to_string();
        let menu_ref = menu.downgrade();
        let callbacks = context.callbacks.clone();
        let tab_strip = context.tab_strip.clone();
        rename_btn.connect_clicked(move |_| {
            if let Some(menu) = menu_ref.upgrade() {
                menu.popdown();
            }
            let tab_strip = tab_strip.clone();
            let lbl = lbl.clone();
            let state = state.clone();
            let tid = tid.clone();
            let callbacks = callbacks.clone();
            glib::idle_add_local_once(move || {
                show_rename_dialog(&tab_strip, &lbl, &state, &tid, &callbacks);
            });
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
        let menu_ref = menu.downgrade();
        let callbacks = context.callbacks.clone();
        pin_btn.connect_clicked(move |_| {
            if let Some(menu) = menu_ref.upgrade() {
                menu.popdown();
            }
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
        let menu_ref = menu.downgrade();
        close_btn.connect_clicked(move |_| {
            if let Some(menu) = menu_ref.upgrade() {
                menu.popdown();
            }
            remove_tab(
                &ts,
                &cs,
                &state,
                &tid,
                &cb,
                &po,
                PaneEmptyReason::ClosedLastTab,
            );
        });
    }

    menu_box.append(&unread_btn);
    menu_box.append(&rename_btn);
    menu_box.append(&pin_btn);
    menu_box.append(&close_btn);
    menu.set_child(Some(&menu_box));
    menu.set_parent(tab_btn);
    menu.set_has_arrow(false);

    // Clean up popover when it closes. With the focus still on an item, the
    // unparent would leak it (see `terminal::unset_focus_within`), so the
    // focus goes back to the pane's active tab instead.
    let pane_widget = context.pane_outer.downgrade();
    menu.connect_closed(move |popover| {
        let had_focus = crate::terminal::focus_is_within(popover.upcast_ref());
        crate::terminal::unset_focus_within(popover.upcast_ref());
        popover.unparent();
        if let Some(pane_widget) = pane_widget.upgrade().filter(|_| had_focus) {
            focus_active_tab_in_pane(pane_widget.upcast_ref());
        }
    });

    menu.popup();
}

pub(crate) fn find_tab_rename_entry<W: glib::object::IsA<gtk::Widget>>(
    root: &W,
) -> Option<gtk::Entry> {
    fn find_entry(widget: &gtk::Widget) -> Option<gtk::Entry> {
        if let Some(entry) = widget.downcast_ref::<gtk::Entry>() {
            if entry.has_css_class(TAB_RENAME_ENTRY_CSS_CLASS) {
                return Some(entry.clone());
            }
        }

        let mut child = widget.first_child();
        while let Some(current) = child {
            if let Some(entry) = find_entry(&current) {
                return Some(entry);
            }
            child = current.next_sibling();
        }

        None
    }

    find_entry(root.as_ref())
}

pub(super) fn find_active_tab_rename_entry(
    tab_state: &Rc<RefCell<TabState>>,
) -> Option<gtk::Entry> {
    let buttons: Vec<gtk::Box> = {
        let state = tab_state.borrow();
        if let Some(active_id) = state.active_rename_tab.as_deref() {
            state
                .tabs
                .iter()
                .filter(|entry| entry.id == active_id)
                .map(|entry| entry.tab_button.clone())
                .collect()
        } else {
            state
                .tabs
                .iter()
                .map(|entry| entry.tab_button.clone())
                .collect()
        }
    };

    buttons
        .into_iter()
        .find_map(|button| find_tab_rename_entry(&button))
}

pub(super) fn tab_rename_active(tab_state: &Rc<RefCell<TabState>>) -> bool {
    if find_active_tab_rename_entry(tab_state).is_some() {
        return true;
    }
    tab_state.borrow_mut().active_rename_tab = None;
    false
}

pub(super) fn focus_tab_rename_entry(entry: &gtk::Entry) {
    if !entry.has_focus() {
        entry.grab_focus();
        entry.select_region(0, -1);
    }
}

pub(super) fn commit_active_tab_rename(tab_state: &Rc<RefCell<TabState>>) -> bool {
    let Some(entry) = find_active_tab_rename_entry(tab_state) else {
        tab_state.borrow_mut().active_rename_tab = None;
        return false;
    };
    entry.emit_activate();
    true
}

pub(super) fn handle_tab_interaction_while_renaming(
    tab_button: &gtk::Box,
    tab_state: &Rc<RefCell<TabState>>,
) -> bool {
    if let Some(entry) = find_tab_rename_entry(tab_button) {
        focus_tab_rename_entry(&entry);
        return true;
    }
    commit_active_tab_rename(tab_state)
}

pub(super) fn show_rename_dialog(
    tab_strip: &gtk::Box,
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

    if let Some(entry) = find_tab_rename_entry(&parent) {
        parent.set_can_target(true);
        focus_tab_rename_entry(&entry);
        return;
    }

    if let Some(entry) = find_tab_rename_entry(tab_strip) {
        entry.emit_activate();
    }

    let parent_was_targetable = parent.can_target();
    parent.set_can_target(true);

    let entry = gtk::Entry::builder()
        .text(&current_name)
        .width_chars(15)
        .build();
    for css_class in TAB_RENAME_ENTRY_CSS_CLASSES {
        entry.add_css_class(css_class);
    }

    label.set_visible(false);
    // Insert entry before the close button
    parent.insert_child_after(&entry, Some(label));
    tab_state.borrow_mut().active_rename_tab = Some(tab_id.to_string());
    focus_tab_rename_entry(&entry);

    // On activate (Enter) or blur, commit rename.
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
            if let Some(tab) = state.borrow_mut().find_tab_mut(&tid) {
                tab.rename(&entry.text());
            }
            lbl.set_visible(true);
            if entry.parent().is_some() {
                parent.remove(entry);
            }
            parent.set_can_target(parent_was_targetable);
            if state.borrow().active_rename_tab.as_deref() == Some(tid.as_str()) {
                state.borrow_mut().active_rename_tab = None;
            }
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
        entry.connect_notify_local(Some("has-focus"), move |entry, _| {
            if entry.has_focus() {
                return;
            }
            let do_rename = do_rename.clone();
            let entry = entry.clone();
            glib::idle_add_local_once(move || {
                if !entry.has_focus() && entry.parent().is_some() {
                    do_rename(&entry);
                }
            });
        });
    }
}
