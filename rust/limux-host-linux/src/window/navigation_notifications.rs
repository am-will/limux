use super::*;

/// Direction for pane navigation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PaneBounds {
    pub(super) left: f64,
    pub(super) top: f64,
    pub(super) right: f64,
    pub(super) bottom: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct NeighborScore {
    pub(super) has_overlap: bool,
    pub(super) overlap: i32,
    pub(super) gap: i32,
    pub(super) center_delta: i32,
}

/// Focus the neighboring pane in the given direction by walking the gtk::Paned tree.
pub(super) fn focus_pane_in_direction(state: &State, direction: Direction) {
    let (_ws_id, pane_widget) = match find_focused_pane(state) {
        Some(v) => v,
        None => return,
    };

    let Some(leaf) = pane_in_direction(state, &pane_widget, direction) else {
        return;
    };
    if let Some(gl) = find_gl_area(&leaf) {
        gl.grab_focus();
    }
}

pub(super) fn pane_in_direction(
    state: &State,
    pane_widget: &gtk::Widget,
    direction: Direction,
) -> Option<gtk::Widget> {
    let root = state.borrow().window.clone().upcast::<gtk::Widget>();

    // Determine which axis and sides we care about.
    let (target_orientation, must_be_start) = match direction {
        Direction::Left => (gtk::Orientation::Horizontal, false), // must be end_child to go left
        Direction::Right => (gtk::Orientation::Horizontal, true), // must be start_child to go right
        Direction::Up => (gtk::Orientation::Vertical, false),     // must be end_child to go up
        Direction::Down => (gtk::Orientation::Vertical, true),    // must be start_child to go down
    };

    // Walk up from the focused pane to find a gtk::Paned with the right
    // orientation where the current subtree is on the correct side.
    let mut current: gtk::Widget = pane_widget.clone();
    loop {
        let parent = current.parent()?;
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
                    // A closed pane stays in the old tree until its teardown
                    // frame and must not be returned.
                    return sibling
                        .map(|sibling| {
                            best_directional_leaf_pane(pane_widget, &sibling, &root, direction)
                                .unwrap_or_else(|| {
                                    let prefer_start = !must_be_start;
                                    find_leaf_pane(&sibling, target_orientation, prefer_start)
                                })
                        })
                        .filter(pane::is_live_pane);
                }
            }
        }
        current = parent;
    }
}

pub(super) fn widget_bounds_in_root(
    widget: &gtk::Widget,
    root: &gtk::Widget,
) -> Option<PaneBounds> {
    let allocation = widget.allocation();
    let width = allocation.width();
    let height = allocation.height();
    if width <= 0 || height <= 0 {
        return None;
    }

    let (left, top) = widget.translate_coordinates(root, 0.0, 0.0)?;
    Some(PaneBounds {
        left,
        top,
        right: left + f64::from(width),
        bottom: top + f64::from(height),
    })
}

pub(super) fn overlap_1d(a_start: f64, a_end: f64, b_start: f64, b_end: f64) -> i32 {
    (a_end.min(b_end) - a_start.max(b_start)).max(0.0).round() as i32
}

pub(super) fn directional_neighbor_score(
    current: PaneBounds,
    candidate: PaneBounds,
    direction: Direction,
) -> Option<NeighborScore> {
    let (gap, overlap, current_center, candidate_center) = match direction {
        Direction::Left => (
            current.left - candidate.right,
            overlap_1d(current.top, current.bottom, candidate.top, candidate.bottom),
            (current.top + current.bottom) / 2.0,
            (candidate.top + candidate.bottom) / 2.0,
        ),
        Direction::Right => (
            candidate.left - current.right,
            overlap_1d(current.top, current.bottom, candidate.top, candidate.bottom),
            (current.top + current.bottom) / 2.0,
            (candidate.top + candidate.bottom) / 2.0,
        ),
        Direction::Up => (
            current.top - candidate.bottom,
            overlap_1d(current.left, current.right, candidate.left, candidate.right),
            (current.left + current.right) / 2.0,
            (candidate.left + candidate.right) / 2.0,
        ),
        Direction::Down => (
            candidate.top - current.bottom,
            overlap_1d(current.left, current.right, candidate.left, candidate.right),
            (current.left + current.right) / 2.0,
            (candidate.left + candidate.right) / 2.0,
        ),
    };

    if gap < -0.5 {
        return None;
    }

    Some(NeighborScore {
        has_overlap: overlap > 0,
        overlap,
        gap: gap.max(0.0).round() as i32,
        center_delta: (candidate_center - current_center).abs().round() as i32,
    })
}

pub(super) fn neighbor_score_better(candidate: NeighborScore, best: NeighborScore) -> bool {
    (
        candidate.has_overlap,
        candidate.overlap,
        -candidate.gap,
        -candidate.center_delta,
    ) > (
        best.has_overlap,
        best.overlap,
        -best.gap,
        -best.center_delta,
    )
}

pub(super) fn collect_leaf_panes(widget: &gtk::Widget, panes: &mut Vec<gtk::Widget>) {
    if pane::is_pane_widget(widget) {
        if pane::is_live_pane(widget) {
            panes.push(widget.clone());
        }
        return;
    }

    if let Some(paned) = widget.downcast_ref::<gtk::Paned>() {
        if let Some(child) = paned.start_child() {
            collect_leaf_panes(&child, panes);
        }
        if let Some(child) = paned.end_child() {
            collect_leaf_panes(&child, panes);
        }
        return;
    }

    if let Some(stack) = widget.downcast_ref::<gtk::Stack>() {
        if let Some(visible) = stack.visible_child() {
            collect_leaf_panes(&visible, panes);
        }
        return;
    }

    let mut child = widget.first_child();
    while let Some(current) = child {
        collect_leaf_panes(&current, panes);
        child = current.next_sibling();
    }
}

pub(super) fn best_directional_leaf_pane(
    current_pane: &gtk::Widget,
    sibling_subtree: &gtk::Widget,
    root: &gtk::Widget,
    direction: Direction,
) -> Option<gtk::Widget> {
    let current_bounds = widget_bounds_in_root(current_pane, root)?;
    let mut leaves = Vec::new();
    collect_leaf_panes(sibling_subtree, &mut leaves);

    let mut best: Option<(gtk::Widget, NeighborScore)> = None;
    for leaf in leaves {
        let Some(bounds) = widget_bounds_in_root(&leaf, root) else {
            continue;
        };
        let Some(score) = directional_neighbor_score(current_bounds, bounds, direction) else {
            continue;
        };

        let should_replace = best
            .as_ref()
            .map(|(_, best_score)| neighbor_score_better(score, *best_score))
            .unwrap_or(true);
        if should_replace {
            best = Some((leaf, score));
        }
    }

    best.map(|(leaf, _)| leaf)
}

/// Recursively find the first visible GLArea inside a widget tree.
/// For gtk::Stack containers, only descend into the visible child.
pub(crate) fn find_gl_area(widget: &gtk::Widget) -> Option<gtk::GLArea> {
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
pub(super) fn find_leaf_pane(
    widget: &gtk::Widget,
    axis: gtk::Orientation,
    prefer_start: bool,
) -> gtk::Widget {
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
    } else if let Some(box_widget) = widget.downcast_ref::<gtk::Box>() {
        // A gtk::Box may be a pane widget (leaf) or a SplitTreeContainer bin
        // wrapping a single pane or paned. Descend into non-pane boxes.
        if !pane::is_pane_widget(widget) {
            if let Some(first_child) = box_widget.first_child() {
                return find_leaf_pane(&first_child, axis, prefer_start);
            }
        }
        widget.clone()
    } else {
        widget.clone()
    }
}

pub(super) fn should_emit_desktop_notification(
    desktop_notifications_enabled: bool,
    window_active: bool,
    workspace_is_active: bool,
    source_focused: bool,
) -> bool {
    desktop_notifications_enabled && (!window_active || !workspace_is_active || !source_focused)
}

/// The tab `surface_hint` names, in the workspace `preferred_index` when
/// `scoped`, else in every workspace. Tab ids beat surface ids, which beat
/// stale panes, then the preferred workspace wins; a tie names nothing.
pub(super) fn surface_tab_target(
    state: &State,
    preferred_index: usize,
    scoped: bool,
    surface_hint: &str,
    terminals_only: bool,
) -> pane::HintResolution<(usize, u32, String)> {
    let workspace_ids = state
        .borrow()
        .workspaces
        .iter()
        .map(|workspace| workspace.id.clone())
        .collect::<Vec<_>>();
    let candidates = workspace_ids
        .iter()
        .enumerate()
        .filter(|(index, _)| !scoped || *index == preferred_index)
        .flat_map(|(index, workspace_id)| {
            pane::tab_candidates_for_workspace(workspace_id, surface_hint, terminals_only)
                .into_iter()
                .map(move |(rank, pane_id, tab_id)| {
                    ((rank, index == preferred_index), (index, pane_id, tab_id))
                })
        });
    pane::resolve_surface_hint(candidates)
}

pub(super) fn has_unread(workspace_unread: bool, tab_unread: bool) -> bool {
    workspace_unread || tab_unread
}

pub(super) fn set_workspace_unread_visual(workspace: &Workspace, unread: bool) {
    if unread {
        workspace
            .notify_dot
            .remove_css_class("limux-notify-dot-hidden");
        workspace.notify_dot.add_css_class("limux-notify-dot");
        workspace.notify_label.remove_css_class("limux-notify-msg");
        workspace
            .notify_label
            .add_css_class("limux-notify-msg-unread");
        workspace
            .notify_label
            .set_visible(!workspace.notify_label.label().is_empty());
        if let Some(row_box) = workspace.sidebar_row.child() {
            row_box.add_css_class("limux-sidebar-row-unread");
        }
        workspace
            .indicator_button
            .add_css_class("limux-indicator-pill-unread");
        workspace
            .indicator_unread_dot
            .remove_css_class("limux-indicator-unread-dot-hidden");
        workspace
            .indicator_unread_dot
            .add_css_class("limux-indicator-unread-dot");
        workspace.indicator_unread_dot.set_visible(true);
    } else {
        workspace.notify_dot.remove_css_class("limux-notify-dot");
        workspace
            .notify_dot
            .add_css_class("limux-notify-dot-hidden");
        workspace
            .notify_label
            .remove_css_class("limux-notify-msg-unread");
        workspace.notify_label.add_css_class("limux-notify-msg");
        workspace.notify_label.set_visible(false);
        workspace.notify_label.set_label("");
        if let Some(row_box) = workspace.sidebar_row.child() {
            row_box.remove_css_class("limux-sidebar-row-unread");
        }
        workspace
            .indicator_button
            .remove_css_class("limux-indicator-pill-unread");
        workspace
            .indicator_unread_dot
            .remove_css_class("limux-indicator-unread-dot");
        workspace
            .indicator_unread_dot
            .add_css_class("limux-indicator-unread-dot-hidden");
        workspace.indicator_unread_dot.set_visible(false);
    }
}

pub(super) fn sync_workspace_unread(state: &State, ws_id: &str) {
    let workspace_unread = {
        let s = state.borrow();
        let Some(workspace) = s.workspaces.iter().find(|workspace| workspace.id == ws_id) else {
            return;
        };
        workspace.unread || workspace.manual_unread
    };
    let tab_unread = pane::workspace_has_unread_tabs(ws_id);
    let s = state.borrow();
    if let Some(workspace) = s.workspaces.iter().find(|workspace| workspace.id == ws_id) {
        set_workspace_unread_visual(workspace, has_unread(workspace_unread, tab_unread));
    }
}

pub(super) fn clear_visible_tab_unread(state: &State, ws_id: &str) {
    let root = {
        let mut s = state.borrow_mut();
        if !s.window.is_active() {
            return;
        }
        let Some(workspace) = s
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.id == ws_id)
        else {
            return;
        };
        workspace.unread = false;
        workspace.manual_unread = false;
        workspace.root.clone()
    };
    pane::clear_active_tab_unread_in_root(&root);
    sync_workspace_unread(state, ws_id);
}

pub(super) fn clear_visible_tab_unread_on_window_focus(state: &State, ws_id: &str) {
    let root = {
        let mut s = state.borrow_mut();
        if !s.window.is_active() {
            return;
        }
        let Some(workspace) = s
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.id == ws_id)
        else {
            return;
        };
        workspace.unread = false;
        workspace.root.clone()
    };
    pane::clear_active_tab_unread_in_root_if_not_manual(&root);
    sync_workspace_unread(state, ws_id);
}

pub(super) fn mark_workspace_unread(
    state: &State,
    ws_id: &str,
    source_focused: bool,
    target: DesktopNotificationTarget,
) -> Option<DesktopNotificationRequest> {
    mark_workspace_unread_with_message(
        state,
        ws_id,
        "Process needs attention",
        source_focused,
        target,
    )
}

pub(super) fn workspace_notification_message(title: &str, body: &str) -> String {
    let title = title.trim();
    let body = body.trim();
    match (title.is_empty(), body.is_empty()) {
        (false, false) => format!("{title}: {body}"),
        (false, true) => title.to_string(),
        (true, false) => body.to_string(),
        (true, true) => "Process needs attention".to_string(),
    }
}

pub(super) fn mark_workspace_unread_with_message(
    state: &State,
    ws_id: &str,
    message: &str,
    source_focused: bool,
    target: DesktopNotificationTarget,
) -> Option<DesktopNotificationRequest> {
    let (workspace_idx, active_idx, workspace_name, window_active, notifications) = {
        let s = state.borrow();
        let workspace_idx = s
            .workspaces
            .iter()
            .position(|workspace| workspace.id == ws_id)?;
        let workspace = &s.workspaces[workspace_idx];
        let notifications = s.config.borrow().notifications;
        (
            workspace_idx,
            s.active_idx,
            workspace.name.clone(),
            s.window.is_active(),
            notifications,
        )
    };
    let workspace_is_active = workspace_idx == active_idx;
    let desktop_request = should_emit_desktop_notification(
        notifications.enabled,
        window_active,
        workspace_is_active,
        source_focused,
    )
    .then(|| DesktopNotificationRequest {
        summary: workspace_name,
        body: message.to_string(),
        sound: notifications.sound,
        target: target.clone(),
    });

    let source_is_unread = !window_active || !workspace_is_active || !source_focused;
    if source_is_unread {
        let tab_target = target.pane_id.zip(target.tab_id.as_deref());
        let tab_was_targeted = tab_target
            .and_then(|(pane_id, tab_id)| {
                pane::mark_tab_unread_in_workspace(ws_id, pane_id, tab_id)
            })
            .is_some();
        let mark_workspace_level = !tab_was_targeted;

        if tab_was_targeted || mark_workspace_level {
            let mut s = state.borrow_mut();
            if let Some(workspace) = s
                .workspaces
                .iter_mut()
                .find(|workspace| workspace.id == ws_id)
            {
                workspace.unread |= mark_workspace_level;
                workspace.notify_label.set_label(message);
            }
            drop(s);
            sync_workspace_unread(state, ws_id);
        }
    }

    desktop_request
}

pub(super) fn desktop_notification_hints(
    sound: app_config::NotificationSound,
) -> HashMap<String, glib::Variant> {
    let mut hints = HashMap::from([("desktop-entry".to_string(), crate::APP_ID.to_variant())]);

    match sound {
        app_config::NotificationSound::Default => {}
        app_config::NotificationSound::None => {
            hints.insert("suppress-sound".to_string(), true.to_variant());
        }
        _ => {
            if let Some(sound_name) = sound.freedesktop_sound_name() {
                let sound_variant = sound_name.to_variant();
                hints.insert("sound-name".to_string(), sound_variant.clone());
                hints.insert("x-canonical-sound-name".to_string(), sound_variant);
            }
        }
    }

    hints
}

pub(super) fn desktop_notification_actions() -> Vec<String> {
    vec!["default".to_string(), "Open".to_string()]
}

pub(super) fn show_desktop_notification(state: &State, request: DesktopNotificationRequest) {
    let state = state.clone();
    gio::DBusProxy::for_bus(
        gio::BusType::Session,
        gio::DBusProxyFlags::NONE,
        None::<&gio::DBusInterfaceInfo>,
        FREEDESKTOP_NOTIFICATIONS_SERVICE,
        FREEDESKTOP_NOTIFICATIONS_PATH,
        FREEDESKTOP_NOTIFICATIONS_INTERFACE,
        None::<&gio::Cancellable>,
        move |result| {
            let Ok(proxy) = result else {
                return;
            };
            let route = DesktopNotificationRoute {
                target: request.target.clone(),
                activation_token: None,
            };

            let params = (
                "Limux",
                0u32,
                crate::APP_ID,
                request.summary.as_str(),
                request.body.as_str(),
                desktop_notification_actions(),
                desktop_notification_hints(request.sound),
                DESKTOP_NOTIFICATION_EXPIRE_TIMEOUT_MS,
            )
                .to_variant();

            proxy.call(
                "Notify",
                Some(&params),
                gio::DBusCallFlags::NONE,
                DESKTOP_NOTIFICATION_DBUS_TIMEOUT_MS,
                None::<&gio::Cancellable>,
                move |result| {
                    let Ok(response) = result else {
                        return;
                    };
                    let Some(notification_id) = desktop_notification_id_from_response(&response)
                    else {
                        return;
                    };

                    state
                        .borrow_mut()
                        .desktop_notification_routes
                        .insert(notification_id, route.clone());
                },
            );
        },
    );
}
