use super::*;

pub(super) fn normalize_reorder_insert_index(
    source_idx: usize,
    insert_idx: usize,
) -> Option<usize> {
    if source_idx == insert_idx || source_idx + 1 == insert_idx {
        return None;
    }
    Some(if source_idx < insert_idx {
        insert_idx - 1
    } else {
        insert_idx
    })
}

pub(super) fn next_active_after_tab_removal(
    tab_ids: &[&str],
    active_id: Option<&str>,
    removed_idx: usize,
) -> Option<String> {
    if tab_ids.len() <= 1 {
        return None;
    }
    let removed_id = tab_ids.get(removed_idx).copied()?;
    if active_id != Some(removed_id) {
        return active_id.map(ToOwned::to_owned);
    }
    let next_idx = removed_idx.min(tab_ids.len() - 2);
    tab_ids
        .iter()
        .enumerate()
        .find_map(|(idx, tab_id)| (idx != removed_idx).then_some(*tab_id))
        .and_then(|_| {
            tab_ids
                .iter()
                .enumerate()
                .filter_map(|(idx, tab_id)| (idx != removed_idx).then_some(*tab_id))
                .nth(next_idx)
        })
        .map(ToOwned::to_owned)
}

pub(super) fn classify_content_drop_zone(
    width: f64,
    height: f64,
    x: f64,
    y: f64,
) -> Option<ContentDropZone> {
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    if x < width * 0.25 {
        Some(ContentDropZone::Left)
    } else if x > width * 0.75 {
        Some(ContentDropZone::Right)
    } else if y < height * 0.25 {
        Some(ContentDropZone::Top)
    } else if y > height * 0.75 {
        Some(ContentDropZone::Bottom)
    } else {
        Some(ContentDropZone::Center)
    }
}

pub(super) fn content_drop_preview_rect(zone: ContentDropZone) -> (f64, f64, f64, f64) {
    match zone {
        ContentDropZone::Left => (0.0, 0.0, 0.5, 1.0),
        ContentDropZone::Right => (0.5, 0.0, 0.5, 1.0),
        ContentDropZone::Top => (0.0, 0.0, 1.0, 0.5),
        ContentDropZone::Bottom => (0.0, 0.5, 1.0, 0.5),
        ContentDropZone::Center => (0.25, 0.25, 0.5, 0.5),
    }
}

pub(super) fn effective_drop_target_dimensions(
    preview_width: i32,
    preview_height: i32,
    content_width: i32,
    content_height: i32,
) -> Option<(f64, f64)> {
    let width = preview_width.max(content_width);
    let height = preview_height.max(content_height);
    if width <= 0 || height <= 0 {
        return None;
    }
    Some((width as f64, height as f64))
}

pub(super) fn clear_content_drop_zone(overlay: &gtk::Box) {
    overlay.remove_css_class("limux-drop-preview");
    overlay.remove_css_class("limux-drop-preview-center");
    overlay.set_size_request(-1, -1);
    overlay.set_margin_start(0);
    overlay.set_margin_top(0);
}

pub(super) fn highlight_content_drop_zone(overlay: &gtk::Box, zone: ContentDropZone) {
    clear_content_drop_zone(overlay);
    overlay.add_css_class("limux-drop-preview");
    if zone == ContentDropZone::Center {
        overlay.add_css_class("limux-drop-preview-center");
    }
    let (x_frac, y_frac, width_frac, height_frac) = content_drop_preview_rect(zone);
    let total_width = overlay
        .parent()
        .map(|parent| parent.allocation().width())
        .unwrap_or_else(|| overlay.width())
        .max(1);
    let total_height = overlay
        .parent()
        .map(|parent| parent.allocation().height())
        .unwrap_or_else(|| overlay.height())
        .max(1);
    overlay.set_margin_start((total_width as f64 * x_frac).round() as i32);
    overlay.set_margin_top((total_height as f64 * y_frac).round() as i32);
    overlay.set_size_request(
        (total_width as f64 * width_frac).round() as i32,
        (total_height as f64 * height_frac).round() as i32,
    );
}

pub(super) fn position_indicator(tab_state: &Rc<RefCell<TabState>>, indicator: &gtk::Box, x: f64) {
    let Some(viewport) = indicator.parent() else {
        return;
    };
    let tab_state = tab_state.borrow();
    if tab_state.tabs.is_empty() {
        indicator.set_visible(false);
        return;
    }

    let mut position = 0.0;
    for entry in &tab_state.tabs {
        let Some(bounds) = entry.tab_button.compute_bounds(&viewport) else {
            continue;
        };
        let left = bounds.x() as f64;
        let right = left + bounds.width() as f64;
        let midpoint = left + bounds.width() as f64 / 2.0;
        if x < midpoint {
            position = left;
            break;
        }
        position = right;
    }
    indicator.set_margin_start(position.clamp(0.0, viewport.width().max(0) as f64) as i32);
    indicator.set_visible(true);
}

pub(super) fn insert_index_for_drop(
    tab_state: &Rc<RefCell<TabState>>,
    viewport: &gtk::Widget,
    x: f64,
    ignored_tab_id: Option<&str>,
) -> usize {
    let tab_state = tab_state.borrow();
    for (idx, entry) in tab_state.tabs.iter().enumerate() {
        if ignored_tab_id == Some(entry.id.as_str()) {
            continue;
        }
        let Some(bounds) = entry.tab_button.compute_bounds(viewport) else {
            continue;
        };
        let midpoint = bounds.x() as f64 + bounds.width() as f64 / 2.0;
        if x < midpoint {
            return idx;
        }
    }
    tab_state.tabs.len()
}

pub(super) fn rebuild_tab_strip(tab_strip: &gtk::Box, tab_state: &Rc<RefCell<TabState>>) {
    let buttons: Vec<gtk::Box> = tab_state
        .borrow()
        .tabs
        .iter()
        .map(|entry| entry.tab_button.clone())
        .collect();
    for button in &buttons {
        if button.parent().is_some() {
            tab_strip.remove(button);
        }
    }
    for button in &buttons {
        tab_strip.append(button);
    }
}

pub(super) fn rebind_moved_tab_entry(entry: &mut TabEntry, target: &Rc<PaneInternals>) {
    let interface = (target.callbacks.current_config)()
        .borrow()
        .interface
        .clone();
    entry.activity.configure(&interface);
    match &entry.kind {
        TabKind::Terminal { state } => {
            state
                .handle
                .set_output_activity_enabled(interface.show_tab_activity);
            state.handle.replace_callbacks(make_terminal_callbacks(
                target,
                &entry.id,
                &entry.title_label,
                &state.cwd,
            ));
        }
        TabKind::Browser { state } => {
            *state.callbacks.borrow_mut() = target.callbacks.clone();
        }
        _ => {}
    }
    let (tab_button, unread_dot) =
        build_tab_button_from_label(&entry.title_label, &entry.id, target, &entry.activity);
    entry.tab_button = tab_button;
    entry.unread_dot = unread_dot;
    if entry.pinned {
        apply_pin_visuals(&entry.tab_button, true);
    }
    entry.unread_dot.set_visible(entry.unread);
}

pub(super) fn reorder_tab_to_index(
    tab_strip: &gtk::Box,
    tab_state: &Rc<RefCell<TabState>>,
    callbacks: &Rc<PaneCallbacks>,
    source_id: &str,
    insert_idx: usize,
) -> bool {
    commit_active_tab_rename(tab_state);

    let mut state = tab_state.borrow_mut();
    let Some(source_idx) = state.tabs.iter().position(|entry| entry.id == source_id) else {
        return false;
    };
    let Some(normalized_idx) = normalize_reorder_insert_index(source_idx, insert_idx) else {
        return false;
    };
    let entry = state.tabs.remove(source_idx);
    state.tabs.insert(normalized_idx, entry);
    drop(state);
    rebuild_tab_strip(tab_strip, tab_state);
    queue_active_tab_reveal(tab_strip, tab_state);
    (callbacks.on_state_changed)();
    true
}

pub(super) fn transfer_tab_between_panes(
    source: &Rc<PaneInternals>,
    target: &Rc<PaneInternals>,
    tab_id: &str,
    insert_idx: usize,
) -> bool {
    if source.pane_id == target.pane_id {
        return false;
    }
    commit_active_tab_rename(&source.tab_state);
    commit_active_tab_rename(&target.tab_state);

    let (mut entry, source_next_active) = {
        let mut source_state = source.tab_state.borrow_mut();
        let Some(source_idx) = source_state.tabs.iter().position(|item| item.id == tab_id) else {
            return false;
        };
        let all_ids: Vec<&str> = source_state
            .tabs
            .iter()
            .map(|item| item.id.as_str())
            .collect();
        let next_active =
            next_active_after_tab_removal(&all_ids, source_state.active_tab.as_deref(), source_idx);
        (source_state.tabs.remove(source_idx), next_active)
    };
    let moved_was_unread = entry.unread;

    if let Some(window) = entry
        .content
        .root()
        .and_then(|root| root.downcast::<gtk::Window>().ok())
    {
        gtk::prelude::GtkWindowExt::set_focus(&window, gtk::Widget::NONE);
    }

    if entry.tab_button.parent().is_some() {
        source.tab_strip.remove(&entry.tab_button);
    }

    rebind_moved_tab_entry(&mut entry, target);
    let moved_tab_id = entry.id.clone();
    let content = entry.content.clone();

    {
        let mut target_state = target.tab_state.borrow_mut();
        let clamped_idx = insert_idx.min(target_state.tabs.len());
        target_state.tabs.insert(clamped_idx, entry);
    }
    rebuild_tab_strip(&target.tab_strip, &target.tab_state);

    // Activate the source's replacement before the content leaves: GtkStack
    // maps its first child the instant the visible one is hidden, so this
    // keeps that first child from flashing on screen once
    // `detach_after_repaint` below hides `content`.
    if let Some(next_active) = source_next_active {
        activate_tab(
            &source.tab_strip,
            &source.content_stack,
            &source.tab_state,
            &next_active,
        );
        clear_tab_unread_if_visible(
            &source.tab_state,
            &next_active,
            &source.pane_outer.clone().upcast(),
            &source.callbacks,
        );
    }

    // The tab belongs to the target from now on, so closing either pane
    // before the content arrives treats it as the target's. The content
    // changes stacks once a frame without it has been painted (see
    // `terminal::detach_after_repaint`). Connected ahead of `on_empty`, which
    // tears the source pane down after that same frame.
    {
        let target = Rc::clone(target);
        let moved = content.clone();
        let tab_id = moved_tab_id.clone();
        let from = content.parent();
        crate::terminal::detach_after_repaint(&content, move || {
            // A later move of the same tab may have placed it already.
            if moved.parent() == from {
                crate::terminal::remove_from_stack(&moved);
            }
            let (still_here, active) = {
                let target_state = target.tab_state.borrow();
                (
                    target_state
                        .tabs
                        .iter()
                        .any(|item| item.id == tab_id && item.content == moved),
                    target_state.active_tab.as_deref() == Some(tab_id.as_str()),
                )
            };
            // Closed, or moved on again, before its content got here; or an
            // earlier move of the same tab, run in this frame, placed it.
            if !still_here || moved.parent().is_some() {
                return;
            }
            moved.set_visible(true);
            target.content_stack.add_named(&moved, Some(&tab_id));
            if active {
                activate_tab(
                    &target.tab_strip,
                    &target.content_stack,
                    &target.tab_state,
                    &tab_id,
                );
            }
        });
    }

    if moved_was_unread {
        (source.callbacks.on_unread_changed)();
    }

    if source.tab_state.borrow().tabs.is_empty() {
        (source.callbacks.on_empty)(
            &source.pane_outer.clone().upcast(),
            PaneEmptyReason::MovedLastTabOut,
        );
    }

    activate_tab(
        &target.tab_strip,
        &target.content_stack,
        &target.tab_state,
        &moved_tab_id,
    );
    let target_widget = target.pane_outer.clone().upcast();
    if (target.callbacks.is_pane_visible)(&target_widget) {
        clear_tab_unread_if_visible(
            &target.tab_state,
            &moved_tab_id,
            &target_widget,
            &target.callbacks,
        );
    } else if moved_was_unread {
        (target.callbacks.on_unread_changed)();
    }
    (target.callbacks.on_state_changed)();
    true
}

pub(super) fn install_tab_strip_drop_target(
    tab_overlay: &gtk::Overlay,
    internals: &Rc<PaneInternals>,
) {
    let drop_target = gtk::DropTarget::new(glib::Type::STRING, gtk::gdk::DragAction::MOVE);
    drop_target.set_preload(true);
    {
        let tab_state = internals.tab_state.clone();
        let indicator = internals.drop_indicator.clone();
        let workspace_dragging = internals.workspace_dragging.clone();
        drop_target.connect_motion(move |_, x, _| {
            if workspace_dragging.get() || !is_tab_dragging() {
                indicator.set_visible(false);
                return gtk::gdk::DragAction::empty();
            }
            position_indicator(&tab_state, &indicator, x);
            gtk::gdk::DragAction::MOVE
        });
    }
    {
        let indicator = internals.drop_indicator.clone();
        drop_target.connect_leave(move |_| {
            indicator.set_visible(false);
        });
    }
    {
        let target = Rc::downgrade(internals);
        let indicator = internals.drop_indicator.clone();
        drop_target.connect_drop(move |drop_target, value, x, _| {
            indicator.set_visible(false);
            let Some(target) = target.upgrade() else {
                return false;
            };
            let Ok(raw) = value.get::<String>() else {
                return false;
            };
            let Some(payload) = TabDragPayload::decode(&raw) else {
                return false;
            };
            let Some(viewport) = drop_target.widget() else {
                return false;
            };
            let same_pane = payload.pane_id == target.pane_id;
            let insert_idx = insert_index_for_drop(
                &target.tab_state,
                &viewport,
                x,
                same_pane.then_some(payload.tab_id.as_str()),
            );
            if same_pane {
                return reorder_tab_to_index(
                    &target.tab_strip,
                    &target.tab_state,
                    &target.callbacks,
                    &payload.tab_id,
                    insert_idx,
                );
            }
            let Some(source) = lookup_pane_internals(payload.pane_id) else {
                return false;
            };
            transfer_tab_between_panes(&source, &target, &payload.tab_id, insert_idx)
        });
    }
    tab_overlay.add_controller(drop_target);
}

pub(super) fn set_browser_targeting_enabled(content_stack: &gtk::Stack, enabled: bool) {
    let mut child = content_stack.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        if !widget.has_css_class("limux-browser") {
            continue;
        }
        let webview = widget
            .first_child()
            .and_then(|child| child.next_sibling())
            .and_then(|child| child.next_sibling());
        if let Some(webview) = webview {
            webview.set_can_target(enabled);
        }
    }
}

pub(super) fn install_content_drop_target(internals: &Rc<PaneInternals>) {
    let drop_target = gtk::DropTarget::new(glib::Type::STRING, gtk::gdk::DragAction::MOVE);
    drop_target.set_preload(true);
    // The controller is on content_stack: its handlers hold it and the pane
    // weakly.
    {
        let overlay = internals.content_drop_overlay.clone();
        let content_stack = internals.content_stack.downgrade();
        let workspace_dragging = internals.workspace_dragging.clone();
        drop_target.connect_motion(move |_, x, y| {
            let Some(content_stack) = content_stack.upgrade() else {
                return gtk::gdk::DragAction::empty();
            };
            if workspace_dragging.get() || !is_tab_dragging() {
                clear_content_drop_zone(&overlay);
                return gtk::gdk::DragAction::empty();
            }
            let Some((width, height)) = effective_drop_target_dimensions(
                overlay.width(),
                overlay.height(),
                content_stack.allocation().width(),
                content_stack.allocation().height(),
            ) else {
                clear_content_drop_zone(&overlay);
                return gtk::gdk::DragAction::empty();
            };
            let Some(zone) = classify_content_drop_zone(width, height, x, y) else {
                clear_content_drop_zone(&overlay);
                return gtk::gdk::DragAction::empty();
            };
            highlight_content_drop_zone(&overlay, zone);
            gtk::gdk::DragAction::MOVE
        });
    }
    {
        let overlay = internals.content_drop_overlay.clone();
        drop_target.connect_leave(move |_| {
            clear_content_drop_zone(&overlay);
        });
    }
    {
        let target = Rc::downgrade(internals);
        let overlay = internals.content_drop_overlay.clone();
        drop_target.connect_drop(move |_, value, x, y| {
            clear_content_drop_zone(&overlay);
            let Some(target) = target.upgrade() else {
                return false;
            };
            let Ok(raw) = value.get::<String>() else {
                return false;
            };
            let Some(payload) = TabDragPayload::decode(&raw) else {
                return false;
            };
            let Some((width, height)) = effective_drop_target_dimensions(
                overlay.width(),
                overlay.height(),
                target.content_stack.allocation().width(),
                target.content_stack.allocation().height(),
            ) else {
                return false;
            };
            let Some(zone) = classify_content_drop_zone(width, height, x, y) else {
                return false;
            };
            match zone {
                ContentDropZone::Center => {
                    if payload.pane_id == target.pane_id {
                        return false;
                    }
                    let Some(source) = lookup_pane_internals(payload.pane_id) else {
                        return false;
                    };
                    let insert_idx = target.tab_state.borrow().tabs.len();
                    transfer_tab_between_panes(&source, &target, &payload.tab_id, insert_idx)
                }
                ContentDropZone::Left
                | ContentDropZone::Top
                | ContentDropZone::Right
                | ContentDropZone::Bottom => {
                    let Some(source_widget) = find_pane_widget_by_id(payload.pane_id) else {
                        return false;
                    };
                    let target_widget: gtk::Widget = target.pane_outer.clone().upcast();
                    let (orientation, new_pane_first) = match zone {
                        ContentDropZone::Left => (gtk::Orientation::Horizontal, true),
                        ContentDropZone::Right => (gtk::Orientation::Horizontal, false),
                        ContentDropZone::Top => (gtk::Orientation::Vertical, true),
                        ContentDropZone::Bottom => (gtk::Orientation::Vertical, false),
                        ContentDropZone::Center => unreachable!(),
                    };
                    (target.callbacks.on_split_with_tab)(
                        &source_widget,
                        &target_widget,
                        orientation,
                        payload.tab_id.clone(),
                        new_pane_first,
                    );
                    true
                }
            }
        });
    }
    internals.content_stack.add_controller(drop_target);

    let overlay = internals.content_drop_overlay.clone();
    let content_stack = internals.content_stack.clone();
    let workspace_dragging = internals.workspace_dragging.clone();
    let listener_id = on_tab_drag_change(move |dragging| {
        let visible = dragging && !workspace_dragging.get();
        overlay.set_visible(visible);
        if !visible {
            clear_content_drop_zone(&overlay);
        }
        set_browser_targeting_enabled(&content_stack, !dragging);
    });
    internals.pane_outer.connect_destroy(move |_| {
        remove_tab_drag_listener(listener_id);
    });
}
