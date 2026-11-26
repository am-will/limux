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
    pane::grab_active_tab_focus(&leaf);
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
                    // Found the split point. Navigate to the sibling subtree,
                    // skipping branches hidden in-place by zoom.
                    let sibling = if must_be_start {
                        paned.end_child()
                    } else {
                        paned.start_child()
                    }
                    .filter(|sibling| sibling.get_visible());
                    if let Some(sibling) = sibling {
                        // A closed pane stays in the old tree until its teardown
                        // frame and must not be returned.
                        let leaf =
                            best_directional_leaf_pane(pane_widget, &sibling, &root, direction)
                                .unwrap_or_else(|| {
                                    let prefer_start = !must_be_start;
                                    find_leaf_pane(&sibling, target_orientation, prefer_start)
                                });
                        return pane::is_live_pane(&leaf).then_some(leaf);
                    }
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
    if !widget.get_visible() {
        return;
    }
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
        let start = paned.start_child().filter(|child| child.get_visible());
        let end = paned.end_child().filter(|child| child.get_visible());
        let pick_start = if paned.orientation() == axis {
            prefer_start
        } else {
            true // arbitrary default for orthogonal splits
        };
        let child = if pick_start {
            start.or(end)
        } else {
            end.or(start)
        };
        match child {
            Some(c) => find_leaf_pane(&c, axis, prefer_start),
            None => widget.clone(),
        }
    } else if let Some(box_widget) = widget.downcast_ref::<gtk::Box>() {
        // A gtk::Box may be a pane widget (leaf) or a SplitTreeContainer bin
        // wrapping a single pane or paned. Descend into non-pane boxes.
        if !pane::is_pane_widget(widget) {
            let mut child = box_widget.first_child();
            while let Some(current) = child {
                if current.get_visible() {
                    return find_leaf_pane(&current, axis, prefer_start);
                }
                child = current.next_sibling();
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
    let app_state = state.borrow();
    if scoped {
        let Some(workspace) = app_state.workspaces.get(preferred_index) else {
            return pane::HintResolution::NotFound;
        };
        let candidates =
            pane::tab_candidates_for_workspace(&workspace.id, surface_hint, terminals_only)
                .into_iter()
                .map(|(rank, pane_id, tab_id)| ((rank, true), (preferred_index, pane_id, tab_id)));
        return pane::resolve_surface_hint(candidates);
    }

    let workspace_indices: HashMap<&str, usize> = app_state
        .workspaces
        .iter()
        .enumerate()
        .map(|(index, workspace)| (workspace.id.as_str(), index))
        .collect();
    let candidates =
        pane::tab_candidates_across_workspaces(&workspace_indices, surface_hint, terminals_only)
            .into_iter()
            .map(|(rank, index, pane_id, tab_id)| {
                ((rank, index == preferred_index), (index, pane_id, tab_id))
            });
    pane::resolve_surface_hint(candidates)
}

/// Where a terminal sits in sidebar order: workspace index, pane index in
/// split order, tab index in its pane.
pub(super) type TerminalPosition = (usize, usize, usize);

/// The next (or previous) unread stop after `current`, wrapping around.
/// `stops` is in ascending sidebar order.
pub(super) fn next_unread_stop(
    stops: &[TerminalPosition],
    current: TerminalPosition,
    forward: bool,
) -> Option<usize> {
    if stops.is_empty() {
        return None;
    }
    if forward {
        stops.iter().position(|stop| *stop > current).or(Some(0))
    } else {
        stops
            .iter()
            .rposition(|stop| *stop < current)
            .or(Some(stops.len() - 1))
    }
}

/// Jump to the next (or previous) unread terminal in sidebar order, across
/// projects and collapsed groups, and focus that exact tab.
pub(super) fn cycle_unread_terminal(state: &State, forward: bool) {
    let (workspaces, active_idx) = {
        let s = state.borrow();
        let workspaces = s
            .workspaces
            .iter()
            .map(|workspace| {
                (
                    workspace.id.clone(),
                    // Every pane, including ones a zoom keeps out of the root.
                    workspace.split_container.panes(),
                    workspace.unread || workspace.manual_unread,
                )
            })
            .collect::<Vec<_>>();
        (workspaces, s.active_idx)
    };
    let focused = find_focused_pane(state);
    let mut current = (active_idx, 0, 0);
    let mut stops = Vec::new();
    let mut targets = Vec::new();
    for (workspace_idx, (workspace_id, panes, workspace_unread)) in workspaces.iter().enumerate() {
        let mut tab_unread = false;
        for (pane_idx, pane_widget) in panes.iter().enumerate() {
            let Some(pane::PaneUnreadTabs {
                pane_id,
                unread: unread_tabs,
                active_tab,
            }) = pane::unread_tabs_in_pane(pane_widget)
            else {
                continue;
            };
            if focused
                .as_ref()
                .is_some_and(|(id, focused_pane)| id == workspace_id && focused_pane == pane_widget)
            {
                current = (workspace_idx, pane_idx, active_tab);
            }
            for (tab_idx, tab_id) in unread_tabs {
                tab_unread = true;
                stops.push((workspace_idx, pane_idx, tab_idx));
                targets.push(DesktopNotificationTarget {
                    workspace_id: workspace_id.clone(),
                    pane_id: Some(pane_id),
                    tab_id: Some(tab_id),
                });
            }
        }
        // A project marked unread without a tab target counts once.
        if *workspace_unread && !tab_unread {
            stops.push((workspace_idx, 0, 0));
            targets.push(DesktopNotificationTarget {
                workspace_id: workspace_id.clone(),
                pane_id: None,
                tab_id: None,
            });
        }
    }
    if let Some(index) = next_unread_stop(&stops, current, forward) {
        activate_desktop_notification_target(state, &targets[index], None);
    }
}

pub(super) fn has_unread(workspace_unread: bool, tab_unread: bool) -> bool {
    workspace_unread || tab_unread
}

pub(super) fn set_workspace_unread_visual(workspace: &Workspace, unread: bool) {
    let show_notify_label = unread && !workspace.notify_label.label().is_empty();
    let visual_state = (unread, show_notify_label);
    if workspace.unread_visual.get() == Some(visual_state) {
        return;
    }
    workspace.unread_visual.set(Some(visual_state));

    if unread {
        workspace
            .notify_dot
            .remove_css_class("limux-notify-dot-hidden");
        workspace.notify_dot.add_css_class("limux-notify-dot");
        workspace.notify_label.remove_css_class("limux-notify-msg");
        workspace
            .notify_label
            .add_css_class("limux-notify-msg-unread");
        workspace.notify_label.set_visible(show_notify_label);
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
    let (root, had_workspace_unread) = {
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
        let had = workspace.unread || workspace.manual_unread;
        workspace.unread = false;
        workspace.manual_unread = false;
        (workspace.root.clone(), had)
    };
    let cleared_tab = pane::clear_active_tab_unread_in_root(&root);
    if had_workspace_unread || cleared_tab {
        sync_workspace_unread(state, ws_id);
    }
}

pub(super) fn clear_visible_tab_unread_on_window_focus(state: &State, ws_id: &str) {
    let (root, had_workspace_unread) = {
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
        let had = workspace.unread;
        workspace.unread = false;
        (workspace.root.clone(), had)
    };
    let cleared_tab = pane::clear_active_tab_unread_in_root_if_not_manual(&root);
    if had_workspace_unread || cleared_tab {
        sync_workspace_unread(state, ws_id);
    }
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

const MAX_DESKTOP_NOTIFICATION_ROUTES: usize = 256;

pub(super) fn record_desktop_notification_route(
    routes: &mut HashMap<u32, DesktopNotificationRoute>,
    order: &mut VecDeque<u32>,
    notification_id: u32,
    route: DesktopNotificationRoute,
) {
    if routes.insert(notification_id, route).is_some() {
        order.retain(|id| *id != notification_id);
    }
    order.push_back(notification_id);
    while routes.len() > MAX_DESKTOP_NOTIFICATION_ROUTES {
        let Some(oldest) = order.pop_front() else {
            break;
        };
        routes.remove(&oldest);
    }
}

fn send_desktop_notification_with_proxy(
    state: State,
    proxy: &gio::DBusProxy,
    request: &DesktopNotificationRequest,
) {
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
            let Some(notification_id) = desktop_notification_id_from_response(&response) else {
                return;
            };

            let mut s = state.borrow_mut();
            let AppState {
                desktop_notification_routes,
                desktop_notification_route_order,
                ..
            } = &mut *s;
            record_desktop_notification_route(
                desktop_notification_routes,
                desktop_notification_route_order,
                notification_id,
                route.clone(),
            );
        },
    );
}

pub(super) fn show_desktop_notification(state: &State, request: DesktopNotificationRequest) {
    if let Some(proxy) = state.borrow().desktop_notification_proxy.clone() {
        send_desktop_notification_with_proxy(state.clone(), &proxy, &request);
        return;
    }

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
            {
                let mut s = state.borrow_mut();
                if s.desktop_notification_proxy.is_none() {
                    s.desktop_notification_proxy = Some(proxy.clone());
                }
            }
            send_desktop_notification_with_proxy(state.clone(), &proxy, &request);
        },
    );
}
