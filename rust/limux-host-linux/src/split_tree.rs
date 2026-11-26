use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;

use crate::layout_state::{self, LayoutNodeState, PaneState, SplitOrientation, SplitState};
use crate::pane;
use crate::window::{apply_split_ratio_after_layout, request_session_save, State};

// ---------------------------------------------------------------------------
// SplitNode — runtime data model for the split tree
// ---------------------------------------------------------------------------

/// Runtime split tree node. Source of truth for the split layout.
/// The widget tree is rebuilt from this on every structural change.
pub(crate) enum SplitNode {
    Leaf {
        pane_widget: gtk::Widget,
    },
    Split {
        orientation: gtk::Orientation,
        /// Shared with the Paned's position_notify handler so resize drags
        /// update the data model directly.
        ratio: Rc<RefCell<f64>>,
        left: Box<SplitNode>,
        right: Box<SplitNode>,
    },
}

impl SplitNode {
    pub(crate) fn is_leaf(&self) -> bool {
        matches!(self, SplitNode::Leaf { .. })
    }

    pub(crate) fn collect_panes(&self, panes: &mut Vec<gtk::Widget>) {
        match self {
            SplitNode::Leaf { pane_widget } => panes.push(pane_widget.clone()),
            SplitNode::Split { left, right, .. } => {
                left.collect_panes(panes);
                right.collect_panes(panes);
            }
        }
    }

    /// Find the leaf containing `target` and replace it with `replacement`.
    pub(crate) fn replace(&mut self, target: &gtk::Widget, replacement: SplitNode) -> bool {
        match self {
            SplitNode::Leaf { pane_widget } => {
                if pane_widget == target {
                    *self = replacement;
                    true
                } else {
                    false
                }
            }
            SplitNode::Split { left, right, .. } => {
                // Check containment first to route ownership to the correct subtree
                if left.contains_pane(target) {
                    left.replace(target, replacement)
                } else {
                    right.replace(target, replacement)
                }
            }
        }
    }

    fn contains_pane(&self, target: &gtk::Widget) -> bool {
        match self {
            SplitNode::Leaf { pane_widget } => pane_widget == target,
            SplitNode::Split { left, right, .. } => {
                left.contains_pane(target) || right.contains_pane(target)
            }
        }
    }

    /// The pane nearest to `target` in the sibling that `remove` promotes.
    fn successor_of(&self, target: &gtk::Widget) -> Option<gtk::Widget> {
        let SplitNode::Split { left, right, .. } = self else {
            return None;
        };
        let mut panes = Vec::new();
        if matches!(left.as_ref(), SplitNode::Leaf { pane_widget } if pane_widget == target) {
            right.collect_panes(&mut panes);
            return panes.into_iter().next();
        }
        if matches!(right.as_ref(), SplitNode::Leaf { pane_widget } if pane_widget == target) {
            left.collect_panes(&mut panes);
            return panes.pop();
        }
        left.successor_of(target)
            .or_else(|| right.successor_of(target))
    }

    /// Find the leaf containing `target` and promote its sibling in place.
    pub(crate) fn remove(&mut self, target: &gtk::Widget) -> bool {
        match self {
            SplitNode::Leaf { .. } => false,
            SplitNode::Split { left, right, .. } => {
                if matches!(left.as_ref(), SplitNode::Leaf { pane_widget } if pane_widget == target)
                {
                    // Target is left child — promote right sibling.
                    *self = std::mem::replace(
                        right.as_mut(),
                        SplitNode::Leaf {
                            pane_widget: target.clone(),
                        },
                    );
                    return true;
                }
                if matches!(right.as_ref(), SplitNode::Leaf { pane_widget } if pane_widget == target)
                {
                    // Target is right child — promote left sibling.
                    *self = std::mem::replace(
                        left.as_mut(),
                        SplitNode::Leaf {
                            pane_widget: target.clone(),
                        },
                    );
                    return true;
                }
                left.remove(target) || right.remove(target)
            }
        }
    }

    pub(crate) fn swap_panes(&mut self, first: &gtk::Widget, second: &gtk::Widget) -> bool {
        if !self.contains_pane(first) || !self.contains_pane(second) {
            return false;
        }
        if first == second {
            return true;
        }
        self.swap_panes_in_place(first, second);
        true
    }

    fn swap_panes_in_place(&mut self, first: &gtk::Widget, second: &gtk::Widget) {
        match self {
            SplitNode::Leaf { pane_widget } => {
                if pane_widget == first {
                    *pane_widget = second.clone();
                } else if pane_widget == second {
                    *pane_widget = first.clone();
                }
            }
            SplitNode::Split { left, right, .. } => {
                left.swap_panes_in_place(first, second);
                right.swap_panes_in_place(first, second);
            }
        }
    }

    fn nearest_split_ratio_for_pane(
        &self,
        target: &gtk::Widget,
        axis: gtk::Orientation,
    ) -> Option<(Rc<RefCell<f64>>, SplitChildSide)> {
        match self {
            SplitNode::Leaf { .. } => None,
            SplitNode::Split {
                orientation,
                ratio,
                left,
                right,
            } => {
                if left.contains_pane(target) {
                    left.nearest_split_ratio_for_pane(target, axis).or_else(|| {
                        (*orientation == axis).then(|| (ratio.clone(), SplitChildSide::Start))
                    })
                } else if right.contains_pane(target) {
                    right
                        .nearest_split_ratio_for_pane(target, axis)
                        .or_else(|| {
                            (*orientation == axis).then(|| (ratio.clone(), SplitChildSide::End))
                        })
                } else {
                    None
                }
            }
        }
    }

    /// Snapshot to the serializable layout format for session persistence.
    pub(crate) fn snapshot(&self, working_directory: Option<&str>) -> LayoutNodeState {
        match self {
            SplitNode::Leaf { pane_widget } => pane::snapshot_pane_state(pane_widget)
                .map(LayoutNodeState::Pane)
                .unwrap_or_else(|| LayoutNodeState::Pane(PaneState::fallback(working_directory))),
            SplitNode::Split {
                orientation,
                ratio,
                left,
                right,
            } => LayoutNodeState::Split(SplitState {
                orientation: if *orientation == gtk::Orientation::Horizontal {
                    SplitOrientation::Horizontal
                } else {
                    SplitOrientation::Vertical
                },
                ratio: *ratio.borrow(),
                start: Box::new(left.snapshot(working_directory)),
                end: Box::new(right.snapshot(working_directory)),
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SplitChildSide {
    Start,
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PaneResizeOutcome {
    pub(crate) changed: bool,
    pub(crate) width: i32,
    pub(crate) height: i32,
}

fn apply_pane_resize_delta(current_ratio: f64, side: SplitChildSide, raw_delta: f64) -> f64 {
    let signed_delta = match side {
        SplitChildSide::Start => raw_delta,
        SplitChildSide::End => -raw_delta,
    };
    layout_state::clamp_split_ratio(current_ratio + signed_delta)
}

fn expected_resized_child_extent(
    extent: i32,
    max_position: i32,
    handle_size: i32,
    clamped_position: i32,
    side: SplitChildSide,
) -> i32 {
    match side {
        SplitChildSide::Start => clamped_position.max(0),
        SplitChildSide::End => {
            let allocatable = if max_position > 0 {
                max_position
            } else {
                (extent - handle_size.max(0)).max(0)
            };
            (allocatable - clamped_position).max(0)
        }
    }
}

#[derive(Clone)]
enum ParentSlot {
    Bin(gtk::Box),
    PanedStart(gtk::Paned),
    PanedEnd(gtk::Paned),
}

impl ParentSlot {
    fn of_child(bin: &gtk::Box, child: &gtk::Widget) -> Option<Self> {
        let parent = child.parent()?;
        if parent == *bin.upcast_ref::<gtk::Widget>() {
            return Some(Self::Bin(bin.clone()));
        }
        let paned = parent.downcast::<gtk::Paned>().ok()?;
        if !paned.is_ancestor(bin) {
            return None;
        }
        if paned.start_child().as_ref() == Some(child) {
            Some(Self::PanedStart(paned))
        } else if paned.end_child().as_ref() == Some(child) {
            Some(Self::PanedEnd(paned))
        } else {
            None
        }
    }

    fn current_child(&self) -> Option<gtk::Widget> {
        match self {
            Self::Bin(bin) => bin.first_child(),
            Self::PanedStart(paned) => paned.start_child(),
            Self::PanedEnd(paned) => paned.end_child(),
        }
    }

    fn replace(&self, old: &gtk::Widget, replacement: Option<&gtk::Widget>) {
        match self {
            Self::Bin(bin) => {
                if old.parent().as_ref() == Some(bin.upcast_ref::<gtk::Widget>()) {
                    bin.remove(old);
                }
                if let Some(replacement) = replacement {
                    bin.append(replacement);
                }
            }
            Self::PanedStart(paned) => {
                paned.set_start_child(replacement);
            }
            Self::PanedEnd(paned) => {
                paned.set_end_child(replacement);
            }
        }
    }
}

/// Manages the workspace's split layout. Holds a SplitNode data model (source
/// of truth) and a gtk::Box container for the built widget tree. Simple split
/// and remove operations surgically replace only the affected parent slot so
/// unaffected sibling panes stay mounted and realized without GL context
/// teardown; compound same-frame mutations and swap/zoom fall back to an atomic
/// full-tree rebuild.
pub(crate) struct SplitTreeContainer {
    tree: RefCell<SplitNode>,
    bin: gtk::Box,
    rebuild_source: RefCell<Option<glib::SourceId>>,
    teardown_pending: Cell<bool>,
    /// Set while a localized split/remove surgery is waiting for its repaint
    /// frame. Another structural mutation arriving in the same frame flips
    /// `local_surgery_fallback` so the surgery hands off to a full rebuild.
    local_surgery_teardown: Cell<bool>,
    local_surgery_fallback: Cell<bool>,
    after_rebuild: RefCell<Vec<Box<dyn FnOnce()>>>,
    /// Weak: `remove` saves the focus before a pane closes, and that focus is
    /// usually inside the closing pane.
    last_focused: RefCell<Option<glib::WeakRef<gtk::Widget>>>,
    zoomed_pane: RefCell<Option<gtk::Widget>>,
    state: State,
}

impl SplitTreeContainer {
    /// Create a new container with a single pane (no splits).
    pub(crate) fn new(state: &State, initial_pane: gtk::Widget) -> Rc<Self> {
        let bin = gtk::Box::new(gtk::Orientation::Vertical, 0);
        bin.set_hexpand(true);
        bin.set_vexpand(true);
        bin.append(&initial_pane);

        Rc::new(Self {
            tree: RefCell::new(SplitNode::Leaf {
                pane_widget: initial_pane,
            }),
            bin,
            rebuild_source: RefCell::new(None),
            teardown_pending: Cell::new(false),
            local_surgery_teardown: Cell::new(false),
            local_surgery_fallback: Cell::new(false),
            after_rebuild: RefCell::default(),
            last_focused: RefCell::new(None),
            zoomed_pane: RefCell::new(None),
            state: state.clone(),
        })
    }

    /// Create a container from a pre-built tree (for session restore).
    pub(crate) fn new_from_tree(state: &State, node: SplitNode) -> Rc<Self> {
        let bin = gtk::Box::new(gtk::Orientation::Vertical, 0);
        bin.set_hexpand(true);
        bin.set_vexpand(true);

        // Build the initial widget tree synchronously (no async needed on first build)
        let widget = build_widget_tree(&node, state);
        bin.append(&widget);

        Rc::new(Self {
            tree: RefCell::new(node),
            bin,
            rebuild_source: RefCell::new(None),
            teardown_pending: Cell::new(false),
            local_surgery_teardown: Cell::new(false),
            local_surgery_fallback: Cell::new(false),
            after_rebuild: RefCell::default(),
            last_focused: RefCell::new(None),
            zoomed_pane: RefCell::new(None),
            state: state.clone(),
        })
    }

    /// The container widget to add to the gtk::Stack.
    pub(crate) fn widget(&self) -> &gtk::Box {
        &self.bin
    }

    /// Borrow the tree for reading (e.g. session snapshot).
    pub(crate) fn tree(&self) -> std::cell::Ref<'_, SplitNode> {
        self.tree.borrow()
    }

    /// Whether the tree is a single leaf (no splits).
    pub(crate) fn is_single_pane(&self) -> bool {
        self.tree.borrow().is_leaf()
    }

    /// All pane widgets in this workspace, left-to-right / top-to-bottom.
    ///
    /// Used by the startup eager-restore sweep, which must reach panes that
    /// are nested inside splits and not just the leading pane.
    pub(crate) fn panes(&self) -> Vec<gtk::Widget> {
        let mut panes = Vec::new();
        self.tree.borrow().collect_panes(&mut panes);
        panes
    }

    pub(crate) fn retire_panes(&self) {
        let mut panes = Vec::new();
        self.tree.borrow().collect_panes(&mut panes);
        for pane_widget in panes {
            pane::retire_pane(&pane_widget);
        }
    }

    /// The pane on screen once the pending rebuild lands: the zoomed one, or
    /// the model's first leaf.
    pub(crate) fn shown_pane(&self) -> gtk::Widget {
        if let Some(zoomed) = self.zoomed_pane.borrow().as_ref() {
            return zoomed.clone();
        }
        let mut panes = Vec::new();
        self.tree.borrow().collect_panes(&mut panes);
        panes.swap_remove(0)
    }

    pub(crate) fn contains(&self, target: &gtk::Widget) -> bool {
        self.tree.borrow().contains_pane(target)
    }

    pub(crate) fn toggle_zoom(self: &Rc<Self>, target: &gtk::Widget) -> bool {
        if self.zoomed_pane.borrow().is_some() {
            self.restore_zoom();
            false
        } else if !self.contains(target) {
            false
        } else {
            self.zoom_pane(target);
            true
        }
    }

    pub(crate) fn reveal_pane(self: &Rc<Self>, target: &gtk::Widget) -> bool {
        let should_restore = self
            .zoomed_pane
            .borrow()
            .as_ref()
            .is_some_and(|zoomed| zoomed != target);
        if !should_restore {
            return false;
        }
        self.zoomed_pane.borrow_mut().take();
        *self.last_focused.borrow_mut() = Some(target.downgrade());
        if self.can_toggle_zoom_in_place(target) {
            self.restore_zoom_in_place(Some(target));
        } else {
            self.trigger_rebuild();
        }
        true
    }

    fn can_toggle_zoom_in_place(&self, target: &gtk::Widget) -> bool {
        !self.teardown_pending.get()
            && self.rebuild_source.borrow().is_none()
            && target.is_ancestor(&self.bin)
    }

    fn zoom_pane(self: &Rc<Self>, target: &gtk::Widget) {
        self.save_focus();
        *self.zoomed_pane.borrow_mut() = Some(target.clone());
        *self.last_focused.borrow_mut() = Some(target.downgrade());
        if self.can_toggle_zoom_in_place(target) {
            apply_zoom_visibility(&self.bin, target);
            if !pane::focus_active_tab_in_pane(target) {
                target.grab_focus();
            }
            crate::window::apply_top_bar_mode(&self.state);
        } else {
            self.trigger_rebuild();
        }
    }

    fn restore_zoom(self: &Rc<Self>) {
        self.save_focus();
        let previously_zoomed = self.zoomed_pane.borrow_mut().take();
        let can_restore_in_place = !self.teardown_pending.get()
            && self.rebuild_source.borrow().is_none()
            && self.bin.first_child().is_some()
            && previously_zoomed
                .as_ref()
                .is_none_or(|pane| pane.is_ancestor(&self.bin));
        if can_restore_in_place {
            self.restore_zoom_in_place(None);
        } else {
            self.trigger_rebuild();
        }
    }

    fn restore_zoom_in_place(&self, focus_override: Option<&gtk::Widget>) {
        if let Some(root_child) = self.bin.first_child() {
            let tree = self.tree.borrow();
            restore_zoom_visibility_and_ratios(&root_child, &tree);
        }
        if let Some(target) = focus_override {
            if !pane::focus_active_tab_in_pane(target) {
                target.grab_focus();
            }
        } else {
            let focused = self
                .last_focused
                .borrow()
                .as_ref()
                .and_then(|w| w.upgrade());
            if let Some(focused) = focused {
                if !pane::focus_active_tab_in_pane(&focused) {
                    focused.grab_focus();
                }
            }
        }
        crate::window::apply_top_bar_mode(&self.state);
    }

    /// Split a pane. Mutates the data model, then triggers async rebuild.
    pub(crate) fn can_split(&self, target: &gtk::Widget, orientation: gtk::Orientation) -> bool {
        pane_has_room_to_split(target, orientation)
    }

    pub(crate) fn split(
        self: &Rc<Self>,
        target: &gtk::Widget,
        new_pane: gtk::Widget,
        orientation: gtk::Orientation,
        new_pane_first: bool,
        ratio: f64,
    ) -> bool {
        if !self.contains(target) || !pane_has_room_to_split(target, orientation) {
            return false;
        }

        self.save_focus();
        let was_zoomed = self.zoomed_pane.borrow_mut().take().is_some();
        *self.last_focused.borrow_mut() = Some(new_pane.downgrade());

        let shared_ratio = Rc::new(RefCell::new(layout_state::clamp_split_ratio(ratio)));
        let shared_ratio_for_surgery = shared_ratio.clone();
        let new_pane_for_surgery = new_pane.clone();
        let new_node = if new_pane_first {
            SplitNode::Split {
                orientation,
                ratio: shared_ratio,
                left: Box::new(SplitNode::Leaf {
                    pane_widget: new_pane,
                }),
                right: Box::new(SplitNode::Leaf {
                    pane_widget: target.clone(),
                }),
            }
        } else {
            SplitNode::Split {
                orientation,
                ratio: shared_ratio,
                left: Box::new(SplitNode::Leaf {
                    pane_widget: target.clone(),
                }),
                right: Box::new(SplitNode::Leaf {
                    pane_widget: new_pane,
                }),
            }
        };

        let replaced = {
            let mut tree = self.tree.borrow_mut();
            tree.replace(target, new_node)
        };

        if replaced
            && (was_zoomed
                || !self.try_localized_split(
                    target,
                    new_pane_for_surgery,
                    orientation,
                    new_pane_first,
                    shared_ratio_for_surgery,
                ))
        {
            self.trigger_rebuild();
        }
        replaced
    }

    /// Remove a pane. Mutates the data model, then applies localized surgery
    /// when settled or falls back to an async full rebuild.
    pub(crate) fn remove(self: &Rc<Self>, target: &gtk::Widget) -> bool {
        self.save_focus();

        let (removed, successor) = {
            let mut tree = self.tree.borrow_mut();
            let successor = tree.successor_of(target);
            (tree.remove(target), successor)
        };

        if removed {
            let was_zoomed = self.zoomed_pane.borrow_mut().take().is_some();
            // The focus was in the closed pane: the pane that takes its place
            // gets it after the rebuild.
            let focus = self
                .last_focused
                .borrow()
                .as_ref()
                .and_then(|w| w.upgrade());
            if focus.is_some_and(|focus| &focus == target || focus.is_ancestor(target)) {
                *self.last_focused.borrow_mut() = successor.map(|pane| pane.downgrade());
            }
            if was_zoomed || !self.try_localized_remove(target) {
                self.trigger_rebuild();
            }
        }
        removed
    }

    pub(crate) fn swap(self: &Rc<Self>, first: &gtk::Widget, second: &gtk::Widget) -> bool {
        if first == second {
            return self.contains(first);
        }
        self.save_focus();
        let swapped = self.tree.borrow_mut().swap_panes(first, second);
        if swapped {
            self.zoomed_pane.borrow_mut().take();
            self.trigger_rebuild();
        }
        swapped
    }

    pub(crate) fn resize_pane(
        &self,
        target: &gtk::Widget,
        axis: gtk::Orientation,
        signed_units: f64,
    ) -> Option<PaneResizeOutcome> {
        if !self.contains(target) {
            return None;
        }
        let mut width = target.width();
        let mut height = target.height();
        let Some((ratio_cell, side)) = self
            .tree
            .borrow()
            .nearest_split_ratio_for_pane(target, axis)
        else {
            return Some(PaneResizeOutcome {
                changed: false,
                width,
                height,
            });
        };

        let mut ancestor = target.parent();
        let mut matched_paned: Option<gtk::Paned> = None;
        while let Some(widget) = ancestor {
            if let Ok(paned) = widget.clone().downcast::<gtk::Paned>() {
                if paned.orientation() == axis {
                    matched_paned = Some(paned);
                    break;
                }
            }
            ancestor = widget.parent();
        }

        let paned_extent = matched_paned.as_ref().map_or(0, |paned| {
            if axis == gtk::Orientation::Horizontal {
                paned.width()
            } else {
                paned.height()
            }
        });
        let raw_delta = if paned_extent > 0 {
            (signed_units * 10.0) / f64::from(paned_extent)
        } else {
            signed_units * 0.02
        };

        let current = *ratio_cell.borrow();
        let mut updated = apply_pane_resize_delta(current, side, raw_delta);

        if let Some(paned) = matched_paned {
            if paned_both_children_visible(&paned) {
                let applying = split_ratio_control(&paned)
                    .map(|control| control.applying)
                    .unwrap_or_else(|| Rc::new(Cell::new(false)));
                let applied = crate::window::apply_ratio_value(&paned, axis, updated, &applying);
                if applied && paned_extent > 0 {
                    // The divider stops at the pane minimums. A ratio past them
                    // would make the opposite resize undo the excess first.
                    updated = layout_state::snapshot_split_ratio(
                        paned.position(),
                        paned_extent,
                        Some(updated),
                    );
                    let handle_size = paned.measure(axis, -1).0;
                    let resized_extent = expected_resized_child_extent(
                        paned_extent,
                        paned.max_position(),
                        handle_size,
                        paned.position(),
                        side,
                    );
                    if axis == gtk::Orientation::Horizontal {
                        width = resized_extent;
                    } else {
                        height = resized_extent;
                    }
                }
            }
        }
        *ratio_cell.borrow_mut() = updated;
        Some(PaneResizeOutcome {
            changed: updated != current,
            width,
            height,
        })
    }

    /// Run `f` once the widget tree matches the model: at once, or after the
    /// pending rebuild. Until then a new pane is in the model only, and pane
    /// lookups, which search the workspace root, cannot find it.
    pub(crate) fn after_pending_rebuild(&self, f: impl FnOnce() + 'static) {
        if self.teardown_pending.get() || self.rebuild_source.borrow().is_some() {
            self.after_rebuild.borrow_mut().push(Box::new(f));
        } else {
            f();
        }
    }

    fn can_start_local_surgery(&self) -> bool {
        !self.teardown_pending.get()
            && !self.local_surgery_teardown.get()
            && self.rebuild_source.borrow().is_none()
    }

    /// Surgically split `target` in its direct parent slot without unparenting
    /// or unrealizing unaffected sibling panes.
    fn try_localized_split(
        self: &Rc<Self>,
        target: &gtk::Widget,
        new_pane: gtk::Widget,
        orientation: gtk::Orientation,
        new_pane_first: bool,
        ratio: Rc<RefCell<f64>>,
    ) -> bool {
        if !self.can_start_local_surgery() {
            return false;
        }
        let Some(parent_slot) = ParentSlot::of_child(&self.bin, target) else {
            return false;
        };

        self.teardown_pending.set(true);
        self.local_surgery_teardown.set(true);
        self.local_surgery_fallback.set(false);

        let target_widget = target.clone();
        let target_for_hide = target.clone();
        let hidden = Rc::new(RefCell::new(Vec::<gtk::Widget>::new()));
        let hidden_for_hide = hidden.clone();
        let container = Rc::downgrade(self);

        crate::terminal::run_after_repaint(
            target,
            move || {
                crate::terminal::unset_focus_within(&target_for_hide);
                hide_subtree_contents_for_repaint(
                    &target_for_hide,
                    &mut hidden_for_hide.borrow_mut(),
                );
            },
            move || {
                let Some(container) = container.upgrade() else {
                    return;
                };
                let fallback = container.local_surgery_fallback.replace(false);
                container.local_surgery_teardown.set(false);
                container.teardown_pending.set(false);

                if fallback || parent_slot.current_child().as_ref() != Some(&target_widget) {
                    // Trigger the full rebuild first so `old` is hidden/detached
                    // before restoring inner child visibility (avoids a spurious
                    // map -> refresh -> unmap cycle while still mounted).
                    container.trigger_rebuild();
                    for widget in hidden.borrow_mut().drain(..) {
                        widget.set_visible(true);
                    }
                    return;
                }

                // Defensively clear focus before detaching (handles unrealized
                // no-clock paths and focus re-entry during the repaint frame).
                crate::terminal::unset_focus_within(&target_widget);

                // Replace `target` in its parent slot with a temporary
                // placeholder for one idle tick: `target` unrealizes now while
                // the parent Paned keeps its exact slot allocation (so sibling
                // panes never resize or flicker), and the new split mounts on
                // tick N+1. Restore inner visibility only after unparenting.
                let placeholder = make_surgery_placeholder();
                parent_slot.replace(&target_widget, Some(placeholder.upcast_ref()));
                for widget in hidden.borrow_mut().drain(..) {
                    widget.set_visible(true);
                }

                container.schedule_localized_mount(
                    parent_slot,
                    placeholder,
                    move |container, slot, placeholder| {
                        let paned = create_split_paned(orientation, &ratio, &container.state);
                        if new_pane_first {
                            paned.set_start_child(Some(&new_pane));
                            paned.set_end_child(Some(&target_widget));
                        } else {
                            paned.set_start_child(Some(&target_widget));
                            paned.set_end_child(Some(&new_pane));
                        }
                        let mounted: gtk::Widget = paned.upcast();
                        slot.replace(placeholder, Some(&mounted));
                        mounted
                    },
                );
            },
        );
        true
    }

    /// Surgically remove `target` by promoting its direct sibling subtree into
    /// its parent `Paned`'s slot, leaving all outer branches mounted and realized.
    fn try_localized_remove(self: &Rc<Self>, target: &gtk::Widget) -> bool {
        if !self.can_start_local_surgery() {
            return false;
        }
        let Some(parent_paned) = target
            .parent()
            .and_then(|parent| parent.downcast::<gtk::Paned>().ok())
        else {
            return false;
        };
        if !parent_paned.is_ancestor(&self.bin) {
            return false;
        }
        let sibling = if parent_paned.start_child().as_ref() == Some(target) {
            parent_paned.end_child()
        } else if parent_paned.end_child().as_ref() == Some(target) {
            parent_paned.start_child()
        } else {
            None
        };
        let Some(sibling) = sibling else {
            return false;
        };
        let Some(grandparent_slot) = ParentSlot::of_child(&self.bin, parent_paned.upcast_ref())
        else {
            return false;
        };

        self.teardown_pending.set(true);
        self.local_surgery_teardown.set(true);
        self.local_surgery_fallback.set(false);

        let target_for_hide = target.clone();
        let sibling_for_hide = sibling.clone();
        let parent_for_hide = parent_paned.clone();
        let sibling_hidden = Rc::new(RefCell::new(Vec::<gtk::Widget>::new()));
        let sibling_hidden_for_hide = sibling_hidden.clone();
        let container = Rc::downgrade(self);

        let parent_anchor: gtk::Widget = parent_paned.clone().upcast();
        crate::terminal::run_after_repaint(
            &parent_anchor,
            move || {
                crate::terminal::unset_focus_within(parent_for_hide.upcast_ref());
                let mut discarded_hidden = Vec::new();
                hide_subtree_contents_for_repaint(&target_for_hide, &mut discarded_hidden);
                hide_subtree_contents_for_repaint(
                    &sibling_for_hide,
                    &mut sibling_hidden_for_hide.borrow_mut(),
                );
            },
            move || {
                let Some(container) = container.upgrade() else {
                    return;
                };
                let fallback = container.local_surgery_fallback.replace(false);
                container.local_surgery_teardown.set(false);
                container.teardown_pending.set(false);

                if fallback
                    || grandparent_slot.current_child().as_ref()
                        != Some(parent_paned.upcast_ref::<gtk::Widget>())
                {
                    // Trigger the full rebuild first so `old` is hidden/detached
                    // before restoring inner visibility on surviving children.
                    container.trigger_rebuild();
                    for widget in sibling_hidden.borrow_mut().drain(..) {
                        widget.set_visible(true);
                    }
                    return;
                }

                // Defensively clear focus inside `parent_paned` (including the
                // GtkPaned divider itself) before unparenting.
                crate::terminal::unset_focus_within(parent_paned.upcast_ref());

                // Detach `parent_paned` (replacing it with a placeholder in the
                // grandparent slot so outer splits keep their allocation) and
                // release both children immediately so the removed pane and old
                // Paned drop cleanly while `sibling` unrealizes on tick N.
                // Restore `sibling` inner visibility only after unparenting so
                // nothing maps spuriously before unrealize.
                let placeholder = make_surgery_placeholder();
                grandparent_slot.replace(parent_paned.upcast_ref(), Some(placeholder.upcast_ref()));
                parent_paned.set_start_child(gtk::Widget::NONE);
                parent_paned.set_end_child(gtk::Widget::NONE);
                for widget in sibling_hidden.borrow_mut().drain(..) {
                    widget.set_visible(true);
                }

                container.schedule_localized_mount(
                    grandparent_slot,
                    placeholder,
                    move |_container, slot, placeholder| {
                        slot.replace(placeholder, Some(&sibling));
                        sibling
                    },
                );
            },
        );
        true
    }

    fn schedule_localized_mount(
        self: &Rc<Self>,
        slot: ParentSlot,
        placeholder: gtk::Box,
        mount: impl FnOnce(&Rc<Self>, &ParentSlot, &gtk::Widget) -> gtk::Widget + 'static,
    ) {
        if let Some(source) = self.rebuild_source.take() {
            source.remove();
        }
        let container = Rc::downgrade(self);
        let mut mount = Some(mount);
        let source = glib::idle_add_local_full(glib::Priority::HIGH, move || {
            let Some(container) = container.upgrade() else {
                return glib::ControlFlow::Break;
            };
            container.rebuild_source.replace(None);
            if slot.current_child().as_ref() != Some(placeholder.upcast_ref::<gtk::Widget>()) {
                container.trigger_rebuild();
                return glib::ControlFlow::Break;
            }
            if let Some(mount) = mount.take() {
                let mounted = mount(&container, &slot, placeholder.upcast_ref());
                container.finish_post_mount(&mounted);
            }
            if container.rebuild_source.borrow().is_none() && !container.teardown_pending.get() {
                let waiting = container.after_rebuild.take();
                for f in waiting {
                    f();
                }
            }
            glib::ControlFlow::Break
        });
        self.rebuild_source.replace(Some(source));
    }

    fn finish_post_mount(&self, refresh_root: &gtk::Widget) {
        refresh_terminal_displays_after_rebuild(refresh_root);

        // Newly created panes are tracked as pane containers rather than the
        // inner terminal/browser widget, so restore through the pane helper
        // when possible and fall back to plain widget focus otherwise.
        let focused = self
            .last_focused
            .borrow()
            .as_ref()
            .and_then(|w| w.upgrade());
        if let Some(focused) = focused {
            if !focused.has_focus() && !pane::focus_active_tab_in_pane(&focused) {
                focused.grab_focus();
            }
        }
        crate::window::apply_top_bar_mode(&self.state);
    }

    /// Tear down the old widget tree and schedule a rebuild on the next idle
    /// tick. The one-tick separation between unrealize (teardown) and realize
    /// (rebuild) is what prevents GLArea breakage. The teardown itself waits
    /// for a frame without the old tree (see `terminal::detach_after_repaint`).
    fn trigger_rebuild(self: &Rc<Self>) {
        // Cancel any pending rebuild or localized tick-N+1 mount
        if let Some(source) = self.rebuild_source.take() {
            source.remove();
        }
        // A localized surgery still waiting for its repaint frame will restore
        // hidden children and hand off to a full rebuild once that frame paints.
        if self.local_surgery_teardown.get() {
            self.local_surgery_fallback.set(true);
            return;
        }
        // A teardown still waiting for its frame rebuilds from the latest tree.
        if self.teardown_pending.get() {
            return;
        }
        let Some(old) = self.bin.first_child() else {
            self.schedule_rebuild();
            return;
        };

        // Clearing the bin tears down the old widget tree: unrealize cascades
        // to all GLAreas in the subtree.
        self.teardown_pending.set(true);
        let bin = self.bin.clone();
        let container = Rc::downgrade(self);
        crate::terminal::detach_after_repaint(&old.clone(), move || {
            bin.remove(&old);
            // A lone pane is the old root itself; the rebuild reuses it.
            old.set_visible(true);
            if let Some(container) = container.upgrade() {
                // Old Paneds are already unrealized and detached from the window.
                // Unparent surviving leaf panes now so `do_rebuild` mounts the new
                // tree on the next idle tick without an extra detach pass.
                detach_panes_from_old_tree(&container.tree.borrow());
                container.teardown_pending.set(false);
                // Rebuild on the next idle tick. The tick separation between
                // unrealize (above) and realize (rebuild) is critical.
                container.schedule_rebuild();
            }
        });
    }

    /// Schedule the actual rebuild on the next idle tick.
    fn schedule_rebuild(self: &Rc<Self>) {
        if self.rebuild_source.borrow().is_some() {
            return;
        }
        let container = Rc::clone(self);
        // Ahead of control requests: until the rebuild, the bin holds no pane
        // for them to find.
        let source = glib::idle_add_local_full(glib::Priority::HIGH, move || {
            container.rebuild_source.replace(None);
            container.do_rebuild();
            // Unless it had to queue another pass.
            if container.rebuild_source.borrow().is_none() {
                let waiting = container.after_rebuild.take();
                for f in waiting {
                    f();
                }
            }
            glib::ControlFlow::Break
        });
        self.rebuild_source.replace(Some(source));
    }

    /// Build new widget tree from data model, attach atomically.
    fn do_rebuild(self: &Rc<Self>) {
        // Pane widgets may still be parented to old (floating) Paneds from
        // the previous tree. GTK4 won't let us add them to new containers
        // until they're unparented. Detach them all first.
        let tree = self.tree.borrow();
        let zoomed = self.zoomed_pane.borrow().clone();
        if tree_has_pane_parents(&tree) {
            detach_panes_from_old_tree(&tree);
            self.schedule_rebuild();
            return;
        }
        let widget = build_widget_tree(&tree, &self.state);
        self.bin.append(&widget);
        if let Some(pane) = zoomed {
            if pane.is_ancestor(&self.bin) {
                apply_zoom_visibility(&self.bin, &pane);
            }
        }
        self.finish_post_mount(self.bin.upcast_ref());
    }

    fn save_focus(&self) {
        // A teardown under way unset the focus on purpose (see
        // `terminal::unset_focus_within`): the focus saved before it still
        // stands.
        if self.teardown_pending.get() || self.rebuild_source.borrow().is_some() {
            return;
        }
        let focus = self
            .bin
            .root()
            .and_then(|r| r.downcast::<gtk::Window>().ok())
            .and_then(|w| gtk::prelude::GtkWindowExt::focus(&w));
        *self.last_focused.borrow_mut() = focus.map(|focus| focus.downgrade());
    }
}

impl Drop for SplitTreeContainer {
    fn drop(&mut self) {
        if let Some(source) = self.rebuild_source.take() {
            source.remove();
        }
    }
}

// ---------------------------------------------------------------------------
// Widget tree helpers
// ---------------------------------------------------------------------------

/// Detach pane widgets from their old parents (floating Paneds left over
/// from the previous widget tree). GTK4 requires a widget to have no parent
/// before it can be added to a new container.
fn detach_panes_from_old_tree(node: &SplitNode) {
    match node {
        SplitNode::Leaf { pane_widget } => {
            if let Some(parent) = pane_widget.parent() {
                if let Some(paned) = parent.downcast_ref::<gtk::Paned>() {
                    // Detach from the old Paned by clearing whichever slot holds us
                    if paned
                        .start_child()
                        .map(|c| c == *pane_widget)
                        .unwrap_or(false)
                    {
                        paned.set_start_child(gtk::Widget::NONE);
                    } else {
                        paned.set_end_child(gtk::Widget::NONE);
                    }
                }
            }
        }
        SplitNode::Split { left, right, .. } => {
            detach_panes_from_old_tree(left);
            detach_panes_from_old_tree(right);
        }
    }
}

const SPLIT_RATIO_CONTROL_KEY: &str = "limux-split-ratio-control";

#[derive(Clone)]
struct SplitRatioControl {
    applying: Rc<Cell<bool>>,
    last_size: Rc<Cell<i32>>,
    restoring: Rc<Cell<bool>>,
    restore_epoch: Rc<Cell<u64>>,
}

fn split_ratio_control(paned: &gtk::Paned) -> Option<SplitRatioControl> {
    unsafe {
        paned
            .data::<SplitRatioControl>(SPLIT_RATIO_CONTROL_KEY)
            .map(|ptr| ptr.as_ref().clone())
    }
}

pub(crate) fn is_applying_split_ratio(paned: &gtk::Paned) -> bool {
    unsafe {
        paned
            .data::<SplitRatioControl>(SPLIT_RATIO_CONTROL_KEY)
            .is_some_and(|ptr| {
                let control = ptr.as_ref();
                control.applying.get() || control.restoring.get()
            })
    }
}

pub(crate) fn paned_both_children_visible(paned: &gtk::Paned) -> bool {
    paned.start_child().is_some_and(|child| child.get_visible())
        && paned.end_child().is_some_and(|child| child.get_visible())
}

/// Returns true when `widget` is a descendant of `root` and no widget along
/// the path from `widget` up to `root` has `visible == false`.
///
/// Unlike `gtk::Widget::is_visible()`, this works for inactive `gtk::Stack`
/// workspace pages (which hide pages via `child_visible`, not `visible`) and
/// distinguishes panes hidden in-place by zoom from visible panes.
pub(crate) fn is_visible_within_root(widget: &gtk::Widget, root: &gtk::Widget) -> bool {
    if !widget.is_ancestor(root) {
        return false;
    }
    let mut current = Some(widget.clone());
    while let Some(node) = current {
        if node == *root {
            return true;
        }
        if !node.get_visible() {
            return false;
        }
        current = node.parent();
    }
    false
}

/// Hide every sibling branch along the path from `target` up to `bin`, keeping
/// the `gtk::Paned` widget tree and hidden `GLArea` contexts realized in place.
fn apply_zoom_visibility(bin: &gtk::Box, target: &gtk::Widget) {
    let mut current = target.clone();
    if !current.get_visible() {
        current.set_visible(true);
    }
    while let Some(parent) = current.parent() {
        if parent == *bin {
            break;
        }
        if !parent.get_visible() {
            parent.set_visible(true);
        }
        if let Some(paned) = parent.downcast_ref::<gtk::Paned>() {
            let sibling = if paned.start_child().as_ref() == Some(&current) {
                paned.end_child()
            } else {
                paned.start_child()
            };
            if let Some(sibling) = sibling {
                if sibling.get_visible() {
                    crate::terminal::unset_focus_within(&sibling);
                    sibling.set_visible(false);
                }
            }
        }
        current = parent;
    }
}

/// Unhide every branch hidden by zoom and restore stored `SplitNode` ratios
/// without rebuilding the widget tree.
fn restore_zoom_visibility_and_ratios(widget: &gtk::Widget, node: &SplitNode) {
    if !widget.get_visible() {
        widget.set_visible(true);
    }
    if let (
        Some(paned),
        SplitNode::Split {
            orientation,
            ratio,
            left,
            right,
        },
    ) = (widget.downcast_ref::<gtk::Paned>(), node)
    {
        if let Some(start) = paned.start_child() {
            restore_zoom_visibility_and_ratios(&start, left);
        }
        if let Some(end) = paned.end_child() {
            restore_zoom_visibility_and_ratios(&end, right);
        }

        let control = split_ratio_control(paned);
        let applying = control
            .as_ref()
            .map(|control| control.applying.clone())
            .unwrap_or_else(|| Rc::new(Cell::new(false)));
        let restoring = control
            .as_ref()
            .map(|control| control.restoring.clone())
            .unwrap_or_else(|| Rc::new(Cell::new(false)));
        let restore_epoch = control
            .as_ref()
            .map(|control| control.restore_epoch.clone())
            .unwrap_or_else(|| Rc::new(Cell::new(0)));
        let epoch = restore_epoch.get().wrapping_add(1);
        restore_epoch.set(epoch);
        restoring.set(true);
        if let Some(control) = control.as_ref() {
            control.last_size.set(0);
        }

        // Wait one frame layout pass after unhiding siblings so GTK's
        // `GtkPaned::size_allocate` refreshes its internal `last_allocation`,
        // `max_position`, and child sizes before we apply the saved ratio.
        // Without waiting past Frame 1 LAYOUT, a resize that happened while
        // zoomed leaves GTK's stale `last_allocation` intact and causes Frame 1
        // LAYOUT to rescale `position` and corrupt the stored ratio.
        let ratio_cell = ratio.clone();
        let orientation = *orientation;
        let last_size = control.map(|control| control.last_size);
        let waited_for_layout = Cell::new(false);
        paned.add_tick_callback(move |paned, _| {
            if restore_epoch.get() != epoch {
                return glib::ControlFlow::Break;
            }
            if !paned_both_children_visible(paned) {
                restoring.set(false);
                return glib::ControlFlow::Break;
            }
            if !waited_for_layout.replace(true) {
                return glib::ControlFlow::Continue;
            }
            let size = if orientation == gtk::Orientation::Horizontal {
                paned.width()
            } else {
                paned.height()
            };
            if size <= 0 {
                return glib::ControlFlow::Continue;
            }
            let ratio_val = *ratio_cell.borrow();
            let applied =
                crate::window::apply_ratio_value(paned, orientation, ratio_val, &applying);
            if applied {
                if let Some(last_size) = last_size.as_ref() {
                    last_size.set(size);
                }
                restoring.set(false);
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }
}

fn tree_has_pane_parents(node: &SplitNode) -> bool {
    match node {
        SplitNode::Leaf { pane_widget } => pane_widget.parent().is_some(),
        SplitNode::Split { left, right, .. } => {
            tree_has_pane_parents(left) || tree_has_pane_parents(right)
        }
    }
}

fn make_surgery_placeholder() -> gtk::Box {
    let placeholder = gtk::Box::new(gtk::Orientation::Vertical, 0);
    placeholder.set_hexpand(true);
    placeholder.set_vexpand(true);
    placeholder
}

/// Hide visual contents inside `widget` before a localized one-frame repaint
/// wait without hiding the outer pane or `Paned` slot itself, so unaffected
/// sibling panes keep their exact allocation and never resize or flicker.
fn hide_subtree_contents_for_repaint(widget: &gtk::Widget, hidden: &mut Vec<gtk::Widget>) {
    if let Some(paned) = widget.downcast_ref::<gtk::Paned>() {
        if let Some(start) = paned.start_child() {
            hide_subtree_contents_for_repaint(&start, hidden);
        }
        if let Some(end) = paned.end_child() {
            hide_subtree_contents_for_repaint(&end, hidden);
        }
        return;
    }

    let mut hid_child = false;
    let mut child = widget.first_child();
    while let Some(current) = child {
        let next = current.next_sibling();
        if current.is_visible() {
            current.set_visible(false);
            hidden.push(current);
            hid_child = true;
        }
        child = next;
    }
    if !hid_child && widget.is_visible() {
        widget.set_visible(false);
        hidden.push(widget.clone());
    }
}

fn create_split_paned(
    orientation: gtk::Orientation,
    ratio: &Rc<RefCell<f64>>,
    state: &State,
) -> gtk::Paned {
    let paned = gtk::Paned::builder()
        .orientation(orientation)
        .hexpand(true)
        .vexpand(true)
        // Allow either child to be shrunk below its minimum size so
        // the saved split ratio (e.g. 50/50) is honored even when one
        // pane has wider tabs than the other. Without this, gtk::Paned
        // clamps the position to respect the larger pane's minimum
        // width, producing visibly uneven splits.
        .shrink_start_child(true)
        .shrink_end_child(true)
        .build();
    paned.set_resize_start_child(true);
    paned.set_resize_end_child(true);

    let state_for_drag = state.clone();
    install_split_ratio_tracking(
        &paned,
        ratio,
        Some(Box::new(move || {
            request_session_save(&state_for_drag);
        })),
    );
    paned
}

/// Build a GTK widget tree from the SplitNode data model.
fn build_widget_tree(node: &SplitNode, state: &State) -> gtk::Widget {
    match node {
        SplitNode::Leaf { pane_widget } => {
            if !pane_widget.get_visible() {
                pane_widget.set_visible(true);
            }
            pane_widget.clone()
        }
        SplitNode::Split {
            orientation,
            ratio,
            left,
            right,
        } => {
            let paned = create_split_paned(*orientation, ratio, state);
            let left_widget = build_widget_tree(left, state);
            let right_widget = build_widget_tree(right, state);
            paned.set_start_child(Some(&left_widget));
            paned.set_end_child(Some(&right_widget));

            paned.upcast()
        }
    }
}

pub(crate) fn paned_extent(paned: &gtk::Paned, orientation: gtk::Orientation) -> i32 {
    if orientation == gtk::Orientation::Horizontal {
        paned.width()
    } else {
        paned.height()
    }
}

fn install_split_ratio_tracking(
    paned: &gtk::Paned,
    ratio: &Rc<RefCell<f64>>,
    on_user_drag: Option<Box<dyn Fn()>>,
) {
    // Flag to suppress position_notify during programmatic set_position calls
    // (initial layout, resize re-apply, and workspace re-map). Without this,
    // set_position triggers position_notify which recalculates the ratio from
    // the not-yet-stable pixel position, corrupting the stored ratio.
    let applying = Rc::new(Cell::new(false));

    // Track the extent we last saw so position_notify can distinguish user
    // drags (extent unchanged -> recompute ratio and save session) from
    // layout-driven auto-adjusts (extent changed -> preserve ratio by
    // re-applying position = ratio * new_extent).
    let last_size = Rc::new(Cell::new(0i32));
    let restoring = Rc::new(Cell::new(false));
    let restore_epoch = Rc::new(Cell::new(0u64));
    // Shared with connect_position_notify so a resize bounce back to last_size
    // before the coalesced idle runs is never misclassified as a user drag.
    let reapply_pending = Rc::new(Cell::new(false));
    let orientation = paned.orientation();
    unsafe {
        paned.set_data(
            SPLIT_RATIO_CONTROL_KEY,
            SplitRatioControl {
                applying: applying.clone(),
                last_size: last_size.clone(),
                restoring: restoring.clone(),
                restore_epoch,
            },
        );
    }
    let shared_ratio = ratio.clone();
    let applying_for_notify = applying.clone();
    let restoring_for_notify = restoring.clone();
    let last_size_for_notify = last_size.clone();
    let reapply_pending_for_notify = reapply_pending.clone();
    paned.connect_position_notify(move |paned| {
        if applying_for_notify.get()
            || restoring_for_notify.get()
            || reapply_pending_for_notify.get()
            || !paned_both_children_visible(paned)
        {
            return;
        }
        let size = paned_extent(paned, orientation);
        if size <= 0 || last_size_for_notify.get() != size {
            return;
        }
        // A drag past a child's minimum would let GTK clip that pane.
        let clamped = clamp_paned_position(paned, paned.position());
        if clamped != paned.position() {
            applying_for_notify.set(true);
            paned.set_position(clamped);
            applying_for_notify.set(false);
        }
        let new_ratio = layout_state::clamp_split_ratio(layout_state::snapshot_split_ratio(
            paned.position(),
            size,
            Some(*shared_ratio.borrow()),
        ));
        *shared_ratio.borrow_mut() = new_ratio;
        if let Some(ref on_drag) = on_user_drag {
            on_drag();
        }
    });

    // Both children shrink, so GTK4 updates `max-position` whenever this
    // Paned's allocated extent changes (first layout, window resize, sidebar
    // toggle/drag, parent split drag). Re-apply the live ratio in a coalesced
    // high-priority one-shot idle instead of keeping a permanent
    // add_tick_callback open, which would keep GdkFrameClock updating forever.
    let shared_ratio_for_resize = ratio.clone();
    let applying_for_resize = applying.clone();
    let restoring_for_resize = restoring;
    let last_size_for_resize = last_size.clone();
    paned.connect_max_position_notify(move |paned| {
        if restoring_for_resize.get() || !paned_both_children_visible(paned) {
            return;
        }
        last_size_for_resize.set(0);
        if reapply_pending.replace(true) {
            return;
        }
        let paned_weak = paned.downgrade();
        let reapply_pending = reapply_pending.clone();
        let shared_ratio = shared_ratio_for_resize.clone();
        let applying = applying_for_resize.clone();
        let restoring = restoring_for_resize.clone();
        let last_size = last_size_for_resize.clone();
        glib::idle_add_local_full(glib::Priority::HIGH_IDLE, move || {
            reapply_pending.set(false);
            let Some(paned) = paned_weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if restoring.get() || !paned_both_children_visible(&paned) {
                return glib::ControlFlow::Break;
            }
            let size = paned_extent(&paned, orientation);
            if size <= 0 {
                return glib::ControlFlow::Break;
            }
            let ratio = *shared_ratio.borrow();
            if crate::window::apply_ratio_value(&paned, orientation, ratio, &applying) {
                last_size.set(size);
            }
            glib::ControlFlow::Break
        });
    });

    apply_split_ratio_after_layout(paned, orientation, ratio.clone(), applying, last_size);
}

fn pane_has_room_to_split(target: &gtk::Widget, orientation: gtk::Orientation) -> bool {
    let allocation = target.allocation();
    let size = if orientation == gtk::Orientation::Horizontal {
        allocation.width()
    } else {
        allocation.height()
    };
    size <= 0 || split_extent_has_room(size, orientation)
}

fn minimum_split_extent(orientation: gtk::Orientation) -> i32 {
    if orientation == gtk::Orientation::Horizontal {
        pane::MIN_PANE_WIDTH
    } else {
        pane::MIN_PANE_HEIGHT
    }
}

/// Clamps `position` so neither child of `paned` drops below its minimum
/// extent (see `layout_state::clamp_split_position`).
pub(crate) fn clamp_paned_position(paned: &gtk::Paned, position: i32) -> i32 {
    let orientation = paned.orientation();
    let child_min = |child: Option<gtk::Widget>| {
        child.map_or(0, |child| subtree_min_extent(&child, orientation))
    };
    layout_state::clamp_split_position(
        position,
        child_min(paned.start_child()),
        child_min(paned.end_child()),
        paned.max_position(),
    )
}

/// Minimum extent of a split subtree along `orientation`: leaves contribute
/// the pane minimum, nested splits sum them (same orientation) or take the
/// larger (cross orientation).
fn subtree_min_extent(widget: &gtk::Widget, orientation: gtk::Orientation) -> i32 {
    if !widget.get_visible() {
        return 0;
    }
    let Some(paned) = widget.downcast_ref::<gtk::Paned>() else {
        // The pane minimum, not the measured one: the tab strip does not
        // scroll, so the measured width grows with every tab and would skew
        // even splits (the reason the paned children shrink).
        return minimum_split_extent(orientation);
    };
    let start = paned
        .start_child()
        .map_or(0, |child| subtree_min_extent(&child, orientation));
    let end = paned
        .end_child()
        .map_or(0, |child| subtree_min_extent(&child, orientation));
    let along = paned.orientation() == orientation;
    let handle = if along {
        // Both children of Limux splits are shrinkable, so GTK's minimum
        // extent along the Paned's axis is the separator size. Measure it
        // directly: extent - max_position undercounts when the current
        // allocation is smaller than the handle (GTK clamps that allocation
        // to one pixel).
        paned.measure(orientation, -1).0
    } else {
        0
    };
    layout_state::nested_split_min_extent(start, end, along, handle)
}

fn split_extent_has_room(size: i32, orientation: gtk::Orientation) -> bool {
    size >= minimum_split_extent(orientation) * 2
}

fn refresh_terminal_displays_after_rebuild(root: &gtk::Widget) {
    // Realize, map, and resize signals already sync freshly mounted GLAreas;
    // run one coalesced post-layout idle pass over mapped active tabs only.
    let idle_root = root.downgrade();
    glib::idle_add_local_once(move || {
        if let Some(root) = idle_root.upgrade() {
            pane::refresh_terminal_displays_in_root(&root);
        }
    });
}

// ---------------------------------------------------------------------------
// Conversion from serialized LayoutNodeState to runtime SplitNode
// ---------------------------------------------------------------------------

/// Build a SplitNode tree from a persisted LayoutNodeState.
pub(crate) fn build_split_node_from_layout(
    state: &State,
    shortcuts: &Rc<crate::shortcut_config::ResolvedShortcutConfig>,
    ws_id: &str,
    working_directory: Option<&str>,
    autostart_command: &Rc<RefCell<Option<String>>>,
    layout: &LayoutNodeState,
) -> SplitNode {
    match layout {
        LayoutNodeState::Pane(pane_state) => {
            let pane = crate::window::create_pane_for_workspace(
                state,
                shortcuts,
                ws_id,
                working_directory,
                autostart_command.clone(),
                crate::window::PaneCreationOptions {
                    initial_state: Some(pane_state),
                    skip_default_tab: false,
                    suppress_initial_autostart: false,
                    initial_command: None,
                },
            );
            SplitNode::Leaf {
                pane_widget: pane.upcast(),
            }
        }
        LayoutNodeState::Split(split_state) => {
            let orientation = match split_state.orientation {
                SplitOrientation::Horizontal => gtk::Orientation::Horizontal,
                SplitOrientation::Vertical => gtk::Orientation::Vertical,
            };
            SplitNode::Split {
                orientation,
                ratio: Rc::new(RefCell::new(layout_state::clamp_split_ratio(
                    split_state.ratio,
                ))),
                left: Box::new(build_split_node_from_layout(
                    state,
                    shortcuts,
                    ws_id,
                    working_directory,
                    autostart_command,
                    &split_state.start,
                )),
                right: Box::new(build_split_node_from_layout(
                    state,
                    shortcuts,
                    ws_id,
                    working_directory,
                    autostart_command,
                    &split_state.end,
                )),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires GTK; exercised by xvfb-smoke-test.sh"]
    fn split_ratio_callbacks_release_unmapped_paned() {
        gtk::init().expect("initialize GTK");
        for orientation in [gtk::Orientation::Horizontal, gtk::Orientation::Vertical] {
            let paned = gtk::Paned::new(orientation);
            let weak = paned.downgrade();
            install_split_ratio_tracking(&paned, &Rc::new(RefCell::new(0.5)), None);
            drop(paned);
            assert!(
                weak.upgrade().is_none(),
                "callbacks retained the split widget"
            );
        }
    }

    #[test]
    #[ignore = "requires GTK; exercised by xvfb-smoke-test.sh"]
    fn split_ratio_tracks_resizes_without_idle_frame_clock_or_spurious_saves() {
        let pump_for = |duration: std::time::Duration| {
            let context = glib::MainContext::default();
            let deadline = std::time::Instant::now() + duration;
            while std::time::Instant::now() < deadline {
                for _ in 0..100 {
                    if !context.pending() {
                        break;
                    }
                    context.iteration(false);
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };

        gtk::init().expect("initialize GTK");

        let start = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let end = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let paned = gtk::Paned::builder()
            .orientation(gtk::Orientation::Horizontal)
            .shrink_start_child(true)
            .shrink_end_child(true)
            .hexpand(true)
            .vexpand(true)
            .build();
        paned.set_resize_start_child(true);
        paned.set_resize_end_child(true);
        paned.set_start_child(Some(&start));
        paned.set_end_child(Some(&end));

        let ratio = Rc::new(RefCell::new(0.35));
        let user_drag_saves = Rc::new(Cell::new(0u32));
        let saves_cb = user_drag_saves.clone();
        install_split_ratio_tracking(
            &paned,
            &ratio,
            Some(Box::new(move || {
                saves_cb.set(saves_cb.get() + 1);
            })),
        );

        let window = gtk::Window::new();
        window.set_default_size(800, 500);
        window.set_child(Some(&paned));
        window.present();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !(paned.is_mapped() && paned.width() == 800 && (paned.position() - 280).abs() <= 4) {
            assert!(
                std::time::Instant::now() < deadline,
                "initial split ratio was not applied (width={}, pos={})",
                paned.width(),
                paned.position()
            );
            pump_for(std::time::Duration::from_millis(15));
        }
        pump_for(std::time::Duration::from_millis(120));
        assert_eq!(
            user_drag_saves.get(),
            0,
            "initial layout must not trigger user-drag session saves"
        );

        // Once settled, GdkFrameClock must not keep updating while idle.
        let frame_clock = paned.frame_clock().expect("mapped paned has frame clock");
        let idle_updates = Rc::new(Cell::new(0u32));
        let idle_updates_cb = idle_updates.clone();
        let clock_handler = frame_clock.connect_update(move |_| {
            idle_updates_cb.set(idle_updates_cb.get() + 1);
        });
        pump_for(std::time::Duration::from_millis(250));
        assert_eq!(
            idle_updates.get(),
            0,
            "idle split Paned kept GdkFrameClock updating continuously"
        );
        frame_clock.disconnect(clock_handler);

        // Shrinking to 600px (0.35 * 600 = 210px < MIN_PANE_WIDTH = 260px, while
        // 600px still fits two MIN_PANE_WIDTH panes) clamps position to 260px without
        // corrupting the stored ratio or triggering a session save, and re-expanding
        // restores ~420px instead of GTK's drifted linear scaling (~520px).
        window.set_default_size(600, 500);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !(paned.width() == 600 && (paned.position() - pane::MIN_PANE_WIDTH).abs() <= 4) {
            assert!(
                std::time::Instant::now() < deadline,
                "shrunk split did not clamp to MIN_PANE_WIDTH (width={}, pos={})",
                paned.width(),
                paned.position()
            );
            pump_for(std::time::Duration::from_millis(15));
        }
        assert!(
            (*ratio.borrow() - 0.35).abs() < 1e-9,
            "clamped shrink corrupted stored ratio: {}",
            *ratio.borrow()
        );
        assert_eq!(
            user_drag_saves.get(),
            0,
            "clamped shrink must not trigger user-drag session saves"
        );

        // Programmatic window re-expand preserves the saved ratio via notify::max-position.
        window.set_default_size(1200, 500);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !(paned.width() == 1200 && (paned.position() - 420).abs() <= 4) {
            assert!(
                std::time::Instant::now() < deadline,
                "resized split ratio was not re-applied (width={}, pos={})",
                paned.width(),
                paned.position()
            );
            pump_for(std::time::Duration::from_millis(15));
        }
        assert!(
            (*ratio.borrow() - 0.35).abs() < 1e-9,
            "re-expand corrupted stored ratio: {}",
            *ratio.borrow()
        );
        assert_eq!(
            user_drag_saves.get(),
            0,
            "programmatic window resize must not trigger user-drag session saves"
        );

        // Genuine user drag at stable width updates the ratio and triggers persistence.
        paned.set_position(600);
        assert!(
            (*ratio.borrow() - 0.5).abs() < 0.02,
            "user drag did not update stored ratio: {}",
            *ratio.borrow()
        );
        assert_eq!(user_drag_saves.get(), 1);

        window.close();
        pump_for(std::time::Duration::from_millis(30));
    }

    #[test]
    #[ignore = "requires GTK; exercised by xvfb-smoke-test.sh"]
    fn split_ratio_survives_bounce_unmapped_updates_and_nested_resizes() {
        let pump_for = |duration: std::time::Duration| {
            let context = glib::MainContext::default();
            let deadline = std::time::Instant::now() + duration;
            while std::time::Instant::now() < deadline {
                for _ in 0..100 {
                    if !context.pending() {
                        break;
                    }
                    context.iteration(false);
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        let make_paned = || {
            let paned = gtk::Paned::builder()
                .orientation(gtk::Orientation::Horizontal)
                .shrink_start_child(true)
                .shrink_end_child(true)
                .hexpand(true)
                .vexpand(true)
                .build();
            paned.set_resize_start_child(true);
            paned.set_resize_end_child(true);
            paned
        };

        gtk::init().expect("initialize GTK");

        // 1. Pre-map ratio mutation + resize bounce before idle + stack unmap/remap.
        let paned = make_paned();
        paned.set_start_child(Some(&gtk::Box::new(gtk::Orientation::Vertical, 0)));
        paned.set_end_child(Some(&gtk::Box::new(gtk::Orientation::Vertical, 0)));

        let ratio = Rc::new(RefCell::new(0.35));
        let user_drag_saves = Rc::new(Cell::new(0u32));
        let saves_cb = user_drag_saves.clone();
        install_split_ratio_tracking(
            &paned,
            &ratio,
            Some(Box::new(move || {
                saves_cb.set(saves_cb.get() + 1);
            })),
        );
        // Updating the model before initial layout must not be clobbered by a stale initial ratio.
        *ratio.borrow_mut() = 0.65;

        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        stack.add_named(&paned, Some("split"));
        stack.add_named(&gtk::Box::new(gtk::Orientation::Vertical, 0), Some("other"));
        stack.set_visible_child_name("split");

        let window = gtk::Window::new();
        window.set_default_size(800, 500);
        window.set_child(Some(&stack));
        window.present();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !(paned.is_mapped() && paned.width() == 800 && (paned.position() - 520).abs() <= 4) {
            assert!(
                std::time::Instant::now() < deadline,
                "pre-map updated split ratio was not applied (width={}, pos={})",
                paned.width(),
                paned.position()
            );
            pump_for(std::time::Duration::from_millis(15));
        }
        assert_eq!(user_drag_saves.get(), 0);

        // Simulate a resize bounce that returns to `last_size` (800) with a drifted
        // position before the coalesced idle executes.
        paned.notify("max-position");
        paned.set_position(300);
        assert!(
            (*ratio.borrow() - 0.65).abs() < 1e-9,
            "resize bounce corrupted stored ratio: {}",
            *ratio.borrow()
        );
        assert_eq!(
            user_drag_saves.get(),
            0,
            "resize bounce must not trigger a user-drag session save"
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while (paned.position() - 520).abs() > 4 {
            assert!(
                std::time::Instant::now() < deadline,
                "coalesced idle did not repair bounced split position (pos={})",
                paned.position()
            );
            pump_for(std::time::Duration::from_millis(15));
        }
        assert_eq!(user_drag_saves.get(), 0);

        // Hide the split behind another stack page, resize the window, then switch back.
        stack.set_visible_child_name("other");
        pump_for(std::time::Duration::from_millis(40));
        window.set_default_size(1100, 500);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while window.width() != 1100 {
            assert!(
                std::time::Instant::now() < deadline,
                "window did not resize while split was unmapped (width={})",
                window.width()
            );
            pump_for(std::time::Duration::from_millis(15));
        }
        stack.set_visible_child_name("split");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !(paned.is_mapped() && paned.width() == 1100 && (paned.position() - 715).abs() <= 4) {
            assert!(
                std::time::Instant::now() < deadline,
                "remapped split did not re-apply ratio after unmapped resize (width={}, pos={})",
                paned.width(),
                paned.position()
            );
            pump_for(std::time::Duration::from_millis(15));
        }
        assert!((*ratio.borrow() - 0.65).abs() < 1e-9);
        assert_eq!(user_drag_saves.get(), 0);

        window.close();
        pump_for(std::time::Duration::from_millis(30));

        // 2. Nested split resize propagation: dragging an outer handle updates an
        // inner Paned via notify::max-position without inner saves or idle ticks.
        let inner = make_paned();
        inner.set_start_child(Some(&gtk::Box::new(gtk::Orientation::Vertical, 0)));
        inner.set_end_child(Some(&gtk::Box::new(gtk::Orientation::Vertical, 0)));
        let outer = make_paned();
        outer.set_start_child(Some(&inner));
        outer.set_end_child(Some(&gtk::Box::new(gtk::Orientation::Vertical, 0)));

        let outer_ratio = Rc::new(RefCell::new(0.55));
        let inner_ratio = Rc::new(RefCell::new(0.45));
        let inner_saves = Rc::new(Cell::new(0u32));
        let inner_saves_cb = inner_saves.clone();
        install_split_ratio_tracking(&outer, &outer_ratio, None);
        install_split_ratio_tracking(
            &inner,
            &inner_ratio,
            Some(Box::new(move || {
                inner_saves_cb.set(inner_saves_cb.get() + 1);
            })),
        );

        let nested_window = gtk::Window::new();
        nested_window.set_default_size(1200, 500);
        nested_window.set_child(Some(&outer));
        nested_window.present();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !(outer.width() == 1200
            && (outer.position() - 660).abs() <= 4
            && (inner.position() - 297).abs() <= 6)
        {
            assert!(
                std::time::Instant::now() < deadline,
                "nested splits did not settle initially (outer={}/{}, inner={}/{})",
                outer.width(),
                outer.position(),
                inner.width(),
                inner.position()
            );
            pump_for(std::time::Duration::from_millis(15));
        }
        assert_eq!(inner_saves.get(), 0);

        // Drag the outer handle wider; inner must preserve its 0.45 ratio (~405px of 900px).
        outer.set_position(900);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !(inner.width() >= 895 && (inner.position() - 405).abs() <= 6) {
            assert!(
                std::time::Instant::now() < deadline,
                "outer drag did not propagate ratio to inner split (inner={}/{})",
                inner.width(),
                inner.position()
            );
            pump_for(std::time::Duration::from_millis(15));
        }
        assert!(
            (*inner_ratio.borrow() - 0.45).abs() < 1e-9,
            "outer drag corrupted inner ratio: {}",
            *inner_ratio.borrow()
        );
        assert_eq!(
            inner_saves.get(),
            0,
            "outer drag must not trigger a user-drag save on the inner split"
        );

        // Settled nested splits must also keep GdkFrameClock idle.
        pump_for(std::time::Duration::from_millis(80));
        let frame_clock = inner.frame_clock().expect("mapped nested paned has clock");
        let idle_updates = Rc::new(Cell::new(0u32));
        let idle_updates_cb = idle_updates.clone();
        let clock_handler = frame_clock.connect_update(move |_| {
            idle_updates_cb.set(idle_updates_cb.get() + 1);
        });
        pump_for(std::time::Duration::from_millis(200));
        assert_eq!(
            idle_updates.get(),
            0,
            "nested split Paned kept GdkFrameClock updating while idle"
        );
        frame_clock.disconnect(clock_handler);

        nested_window.close();
        pump_for(std::time::Duration::from_millis(30));

        // 3. Default 50/50 split on an even width (800px -> pos 400 matches GTK4's
        // initial default position before `position-set` is true): `apply_ratio_value`
        // must still mark `position-set` so later child minimum-width growth cannot
        // skew the split or trigger a false user-drag save.
        let half_paned = make_paned();
        let half_start = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let half_end = gtk::Box::new(gtk::Orientation::Vertical, 0);
        half_start.set_size_request(pane::MIN_PANE_WIDTH, -1);
        half_end.set_size_request(pane::MIN_PANE_WIDTH, -1);
        half_paned.set_start_child(Some(&half_start));
        half_paned.set_end_child(Some(&half_end));

        let half_ratio = Rc::new(RefCell::new(0.5));
        let half_saves = Rc::new(Cell::new(0u32));
        let half_saves_cb = half_saves.clone();
        install_split_ratio_tracking(
            &half_paned,
            &half_ratio,
            Some(Box::new(move || {
                half_saves_cb.set(half_saves_cb.get() + 1);
            })),
        );

        let half_window = gtk::Window::new();
        half_window.set_default_size(800, 500);
        half_window.set_child(Some(&half_paned));
        half_window.present();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !(half_paned.is_mapped()
            && half_paned.width() == 800
            && half_paned.is_position_set()
            && (half_paned.position() - 400).abs() <= 2)
        {
            assert!(
                std::time::Instant::now() < deadline,
                "50/50 split did not lock position-set (width={}, pos={}, set={})",
                half_paned.width(),
                half_paned.position(),
                half_paned.is_position_set()
            );
            pump_for(std::time::Duration::from_millis(15));
        }
        assert_eq!(half_saves.get(), 0);

        // Growing one child's minimum width at the same 800px width must keep the
        // 50/50 split locked at 400px with no ratio drift or user-drag save.
        half_start.set_size_request(450, -1);
        pump_for(std::time::Duration::from_millis(120));
        assert!(half_paned.is_position_set());
        assert!(
            (half_paned.position() - 400).abs() <= 2,
            "child minimum growth skewed 50/50 split position to {}",
            half_paned.position()
        );
        assert!(
            (*half_ratio.borrow() - 0.5).abs() < 1e-9,
            "child minimum growth corrupted 50/50 stored ratio: {}",
            *half_ratio.borrow()
        );
        assert_eq!(
            half_saves.get(),
            0,
            "child minimum growth must not trigger a user-drag save"
        );

        half_window.close();
        pump_for(std::time::Duration::from_millis(30));
    }

    #[test]
    #[ignore = "requires GTK; exercised by xvfb-smoke-test.sh"]
    fn restoring_split_respects_nested_pane_minimums() {
        fn wait_for(condition: impl Fn() -> bool, failure: &str) {
            let context = glib::MainContext::default();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !condition() {
                assert!(std::time::Instant::now() < deadline, "{failure}");
                while context.pending() {
                    context.iteration(false);
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }

        gtk::init().expect("initialize GTK");

        let make_pane = || {
            let pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
            pane.set_size_request(-1, pane::MIN_PANE_HEIGHT);
            pane
        };
        let nested_start = make_pane();
        let nested_end = make_pane();
        let nested = gtk::Paned::builder()
            .orientation(gtk::Orientation::Vertical)
            .shrink_start_child(true)
            .shrink_end_child(true)
            .build();
        nested.set_start_child(Some(&nested_start));
        nested.set_end_child(Some(&nested_end));

        let outer_end = make_pane();
        let restored = gtk::Paned::builder()
            .orientation(gtk::Orientation::Vertical)
            .shrink_start_child(true)
            .shrink_end_child(true)
            .build();
        restored.set_start_child(Some(&nested));
        restored.set_end_child(Some(&outer_end));
        // Begin with the nested Paned below its handle size, as a tiny saved
        // parent ratio can leave it before the restored ratios are applied.
        restored.set_position(1);

        let window = gtk::Window::new();
        window.set_default_size(800, 700);
        window.set_child(Some(&restored));
        window.present();

        wait_for(
            || {
                restored.is_mapped()
                    && restored.height() > 0
                    && nested.is_mapped()
                    && nested.height() > 0
            },
            "split fixture did not receive a GTK allocation",
        );

        let handle_size = nested.measure(gtk::Orientation::Vertical, -1).0;
        assert!(
            handle_size > 0,
            "GTK separator should have a measurable size"
        );
        assert!(
            nested.height() <= handle_size,
            "fixture should start with a nested Paned shorter than its handle"
        );
        let nested_minimum = pane::MIN_PANE_HEIGHT * 2 + handle_size;
        let applying = Rc::new(Cell::new(false));
        assert!(crate::window::apply_ratio_value(
            &restored,
            gtk::Orientation::Vertical,
            0.1085,
            &applying,
        ));
        assert!(
            restored.position() >= nested_minimum,
            "restoring a low ratio allocated {} px to a nested split with a {nested_minimum} px minimum",
            restored.position()
        );
        assert!(
            restored.max_position() - restored.position() >= pane::MIN_PANE_HEIGHT,
            "restoring a low ratio left the outer end pane below its minimum"
        );

        wait_for(
            || nested.height() == restored.position(),
            "restored split position was not allocated to its nested child",
        );

        assert!(crate::window::apply_ratio_value(
            &nested,
            gtk::Orientation::Vertical,
            0.1085,
            &applying,
        ));
        assert!(
            nested.position() >= pane::MIN_PANE_HEIGHT,
            "restoring a low ratio left the nested start pane at {} px",
            nested.position()
        );
        assert!(
            nested.max_position() - nested.position() >= pane::MIN_PANE_HEIGHT,
            "restoring a low ratio left the nested end pane below its minimum"
        );

        wait_for(
            || {
                nested_start.height() == nested.position()
                    && nested_start.allocation().y() == 0
                    && nested_end.height() >= pane::MIN_PANE_HEIGHT
                    && nested_end.allocation().y() + nested_end.height() <= nested.height()
                    && outer_end.height() >= pane::MIN_PANE_HEIGHT
                    && outer_end.allocation().y() + outer_end.height() <= restored.height()
            },
            "restored split children did not fit their allocated bounds",
        );
        assert_eq!(
            nested_start.allocation().y(),
            0,
            "restoring the inner split shifted its start pane out of view"
        );
        assert!(nested_start.height() >= pane::MIN_PANE_HEIGHT);
        assert!(nested_end.height() >= pane::MIN_PANE_HEIGHT);
        assert!(outer_end.height() >= pane::MIN_PANE_HEIGHT);

        window.close();
    }

    #[test]
    fn split_extent_requires_room_for_both_children() {
        assert!(!split_extent_has_room(
            pane::MIN_PANE_WIDTH * 2 - 1,
            gtk::Orientation::Horizontal
        ));
        assert!(split_extent_has_room(
            pane::MIN_PANE_WIDTH * 2,
            gtk::Orientation::Horizontal
        ));
        assert!(!split_extent_has_room(
            pane::MIN_PANE_HEIGHT * 2 - 1,
            gtk::Orientation::Vertical
        ));
        assert!(split_extent_has_room(
            pane::MIN_PANE_HEIGHT * 2,
            gtk::Orientation::Vertical
        ));
    }

    #[test]
    fn pane_resize_delta_grows_requested_side_for_start_and_end_children() {
        // Positive delta (right/down) grows a Start child by increasing split ratio,
        // and grows an End child by decreasing split ratio.
        assert!((apply_pane_resize_delta(0.5, SplitChildSide::Start, 0.1) - 0.6).abs() < 1e-9);
        assert!((apply_pane_resize_delta(0.5, SplitChildSide::End, 0.1) - 0.4).abs() < 1e-9);
        // Negative delta (left/up) shrinks a Start child and shrinks an End child.
        assert!((apply_pane_resize_delta(0.5, SplitChildSide::Start, -0.1) - 0.4).abs() < 1e-9);
        assert!((apply_pane_resize_delta(0.5, SplitChildSide::End, -0.1) - 0.6).abs() < 1e-9);
    }

    #[test]
    fn expected_resized_child_extent_computes_post_resize_frame_before_reallocation() {
        assert_eq!(
            expected_resized_child_extent(800, 799, 1, 450, SplitChildSide::Start),
            450
        );
        assert_eq!(
            expected_resized_child_extent(800, 799, 1, 350, SplitChildSide::End),
            449
        );
        // Falls back to extent - handle_size when max_position is not yet populated.
        assert_eq!(
            expected_resized_child_extent(800, 0, 4, 300, SplitChildSide::End),
            496
        );
    }

    #[test]
    #[ignore = "requires GTK; exercised by xvfb-smoke-test.sh"]
    fn split_node_swaps_leaf_panes_and_locates_nearest_split_ratio() {
        if gtk::init().is_err() {
            return;
        }
        let a: gtk::Widget = gtk::Box::new(gtk::Orientation::Vertical, 0).upcast();
        let b: gtk::Widget = gtk::Box::new(gtk::Orientation::Vertical, 0).upcast();
        let c: gtk::Widget = gtk::Box::new(gtk::Orientation::Vertical, 0).upcast();
        let inner_ratio = Rc::new(RefCell::new(0.4));
        let outer_ratio = Rc::new(RefCell::new(0.6));
        let mut tree = SplitNode::Split {
            orientation: gtk::Orientation::Horizontal,
            ratio: outer_ratio.clone(),
            left: Box::new(SplitNode::Leaf {
                pane_widget: a.clone(),
            }),
            right: Box::new(SplitNode::Split {
                orientation: gtk::Orientation::Vertical,
                ratio: inner_ratio.clone(),
                left: Box::new(SplitNode::Leaf {
                    pane_widget: b.clone(),
                }),
                right: Box::new(SplitNode::Leaf {
                    pane_widget: c.clone(),
                }),
            }),
        };

        let mut before = Vec::new();
        tree.collect_panes(&mut before);
        assert_eq!(before, vec![a.clone(), b.clone(), c.clone()]);
        assert!(tree.swap_panes(&a, &c));
        let mut after = Vec::new();
        tree.collect_panes(&mut after);
        assert_eq!(after, vec![c.clone(), b.clone(), a.clone()]);

        let (found_vertical, vertical_side) = tree
            .nearest_split_ratio_for_pane(&b, gtk::Orientation::Vertical)
            .expect("vertical ancestor split");
        assert!(Rc::ptr_eq(&found_vertical, &inner_ratio));
        assert_eq!(vertical_side, SplitChildSide::Start);

        let (found_vertical_end, vertical_end_side) = tree
            .nearest_split_ratio_for_pane(&a, gtk::Orientation::Vertical)
            .expect("vertical ancestor split for swapped end leaf");
        assert!(Rc::ptr_eq(&found_vertical_end, &inner_ratio));
        assert_eq!(vertical_end_side, SplitChildSide::End);

        let (found_horizontal, horizontal_side) = tree
            .nearest_split_ratio_for_pane(&b, gtk::Orientation::Horizontal)
            .expect("horizontal ancestor split");
        assert!(Rc::ptr_eq(&found_horizontal, &outer_ratio));
        assert_eq!(horizontal_side, SplitChildSide::End);

        let (found_horizontal_start, horizontal_start_side) = tree
            .nearest_split_ratio_for_pane(&c, gtk::Orientation::Horizontal)
            .expect("horizontal ancestor split for swapped start leaf");
        assert!(Rc::ptr_eq(&found_horizontal_start, &outer_ratio));
        assert_eq!(horizontal_start_side, SplitChildSide::Start);
    }

    #[test]
    #[ignore = "requires GTK; exercised by xvfb-smoke-test.sh"]
    fn in_place_zoom_preserves_widget_realization_and_split_ratios() {
        fn wait_for(condition: impl Fn() -> bool, failure: &str) {
            let context = glib::MainContext::default();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !condition() {
                assert!(std::time::Instant::now() < deadline, "{failure}");
                while context.pending() {
                    context.iteration(false);
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }

        gtk::init().expect("initialize GTK");

        let make_leaf = || {
            let leaf = gtk::Box::new(gtk::Orientation::Vertical, 0);
            leaf.set_hexpand(true);
            leaf.set_vexpand(true);
            leaf.upcast::<gtk::Widget>()
        };
        let leaf_a = make_leaf();
        let leaf_b = make_leaf();
        let leaf_c = make_leaf();

        let unrealize_count = Rc::new(Cell::new(0usize));
        for leaf in [&leaf_a, &leaf_b, &leaf_c] {
            let count = unrealize_count.clone();
            leaf.connect_unrealize(move |_| {
                count.set(count.get() + 1);
            });
        }

        let outer_ratio = Rc::new(RefCell::new(0.35));
        let inner_ratio = Rc::new(RefCell::new(0.65));

        let inner_paned = gtk::Paned::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .shrink_start_child(true)
            .shrink_end_child(true)
            .build();
        inner_paned.set_resize_start_child(true);
        inner_paned.set_resize_end_child(true);
        install_split_ratio_tracking(&inner_paned, &inner_ratio, None);
        inner_paned.set_start_child(Some(&leaf_b));
        inner_paned.set_end_child(Some(&leaf_c));

        let outer_paned = gtk::Paned::builder()
            .orientation(gtk::Orientation::Horizontal)
            .hexpand(true)
            .vexpand(true)
            .shrink_start_child(true)
            .shrink_end_child(true)
            .build();
        outer_paned.set_resize_start_child(true);
        outer_paned.set_resize_end_child(true);
        install_split_ratio_tracking(&outer_paned, &outer_ratio, None);
        outer_paned.set_start_child(Some(&leaf_a));
        outer_paned.set_end_child(Some(&inner_paned));

        let bin = gtk::Box::new(gtk::Orientation::Vertical, 0);
        bin.set_hexpand(true);
        bin.set_vexpand(true);
        bin.append(&outer_paned);

        let tree = SplitNode::Split {
            orientation: gtk::Orientation::Horizontal,
            ratio: outer_ratio.clone(),
            left: Box::new(SplitNode::Leaf {
                pane_widget: leaf_a.clone(),
            }),
            right: Box::new(SplitNode::Split {
                orientation: gtk::Orientation::Vertical,
                ratio: inner_ratio.clone(),
                left: Box::new(SplitNode::Leaf {
                    pane_widget: leaf_b.clone(),
                }),
                right: Box::new(SplitNode::Leaf {
                    pane_widget: leaf_c.clone(),
                }),
            }),
        };

        let window = gtk::Window::new();
        window.set_default_size(1000, 600);
        window.set_child(Some(&bin));
        window.present();

        wait_for(
            || {
                leaf_a.is_realized()
                    && leaf_b.is_realized()
                    && leaf_c.is_realized()
                    && outer_paned.width() >= 900
                    && (outer_paned.position() - (outer_paned.width() as f64 * 0.35).round() as i32)
                        .abs()
                        <= 4
                    && (inner_paned.position()
                        - (inner_paned.height() as f64 * 0.65).round() as i32)
                        .abs()
                        <= 4
            },
            "initial split fixture did not settle at saved ratios",
        );

        let root_widget: gtk::Widget = bin.clone().upcast();
        assert!(is_visible_within_root(&leaf_a, &root_widget));
        assert!(is_visible_within_root(&leaf_b, &root_widget));
        assert!(is_visible_within_root(&leaf_c, &root_widget));

        // Zoom leaf_b: hides leaf_a and leaf_c in place without unrealizing any leaf.
        apply_zoom_visibility(&bin, &leaf_b);
        wait_for(
            || leaf_b.width() == bin.width() && leaf_b.height() == bin.height(),
            "zoomed leaf did not expand to fill the workspace bin",
        );
        assert_eq!(unrealize_count.get(), 0, "zoom unrealized a leaf widget");
        assert!(leaf_a.is_realized() && leaf_b.is_realized() && leaf_c.is_realized());
        assert!(!is_visible_within_root(&leaf_a, &root_widget));
        assert!(is_visible_within_root(&leaf_b, &root_widget));
        assert!(!is_visible_within_root(&leaf_c, &root_widget));
        assert!((*outer_ratio.borrow() - 0.35).abs() < 1e-6);
        assert!((*inner_ratio.borrow() - 0.65).abs() < 1e-6);

        // Unzoom in place: unhides siblings and restores both saved split ratios.
        restore_zoom_visibility_and_ratios(outer_paned.upcast_ref(), &tree);
        wait_for(
            || {
                leaf_a.is_mapped()
                    && leaf_b.is_mapped()
                    && leaf_c.is_mapped()
                    && (outer_paned.position() - (outer_paned.width() as f64 * 0.35).round() as i32)
                        .abs()
                        <= 4
                    && (inner_paned.position()
                        - (inner_paned.height() as f64 * 0.65).round() as i32)
                        .abs()
                        <= 4
            },
            "unzoom did not restore saved split ratios",
        );
        assert_eq!(unrealize_count.get(), 0, "unzoom unrealized a leaf widget");
        assert!(is_visible_within_root(&leaf_a, &root_widget));
        assert!(is_visible_within_root(&leaf_b, &root_widget));
        assert!(is_visible_within_root(&leaf_c, &root_widget));
        assert!((*outer_ratio.borrow() - 0.35).abs() < 1e-6);
        assert!((*inner_ratio.borrow() - 0.65).abs() < 1e-6);

        window.close();
    }

    #[test]
    #[ignore = "requires GTK; exercised by xvfb-smoke-test.sh"]
    fn in_place_unzoom_after_window_resize_preserves_ratios_and_clamping() {
        fn wait_for(condition: impl Fn() -> bool, failure: &str) {
            let context = glib::MainContext::default();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !condition() {
                assert!(std::time::Instant::now() < deadline, "{failure}");
                while context.pending() {
                    context.iteration(false);
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }

        gtk::init().expect("initialize GTK");

        let make_leaf = || {
            let leaf = gtk::Box::new(gtk::Orientation::Vertical, 0);
            leaf.set_hexpand(true);
            leaf.set_vexpand(true);
            leaf.upcast::<gtk::Widget>()
        };
        let leaf_a = make_leaf();
        let leaf_b = make_leaf();
        let leaf_c = make_leaf();

        let outer_ratio = Rc::new(RefCell::new(0.35));
        let inner_ratio = Rc::new(RefCell::new(0.60));

        // Nested same-axis horizontal split: (A | (B | C)) so window width changes
        // affect BOTH paned axes while zoomed.
        let inner_paned = gtk::Paned::builder()
            .orientation(gtk::Orientation::Horizontal)
            .hexpand(true)
            .vexpand(true)
            .shrink_start_child(true)
            .shrink_end_child(true)
            .build();
        inner_paned.set_resize_start_child(true);
        inner_paned.set_resize_end_child(true);
        install_split_ratio_tracking(&inner_paned, &inner_ratio, None);
        inner_paned.set_start_child(Some(&leaf_b));
        inner_paned.set_end_child(Some(&leaf_c));

        let outer_paned = gtk::Paned::builder()
            .orientation(gtk::Orientation::Horizontal)
            .hexpand(true)
            .vexpand(true)
            .shrink_start_child(true)
            .shrink_end_child(true)
            .build();
        outer_paned.set_resize_start_child(true);
        outer_paned.set_resize_end_child(true);
        install_split_ratio_tracking(&outer_paned, &outer_ratio, None);
        outer_paned.set_start_child(Some(&leaf_a));
        outer_paned.set_end_child(Some(&inner_paned));

        let bin = gtk::Box::new(gtk::Orientation::Vertical, 0);
        bin.set_hexpand(true);
        bin.set_vexpand(true);
        bin.append(&outer_paned);

        let tree = SplitNode::Split {
            orientation: gtk::Orientation::Horizontal,
            ratio: outer_ratio.clone(),
            left: Box::new(SplitNode::Leaf {
                pane_widget: leaf_a.clone(),
            }),
            right: Box::new(SplitNode::Split {
                orientation: gtk::Orientation::Horizontal,
                ratio: inner_ratio.clone(),
                left: Box::new(SplitNode::Leaf {
                    pane_widget: leaf_b.clone(),
                }),
                right: Box::new(SplitNode::Leaf {
                    pane_widget: leaf_c.clone(),
                }),
            }),
        };

        let window = gtk::Window::new();
        window.set_default_size(1100, 600);
        window.set_child(Some(&bin));
        window.present();

        let expected_pos = |paned: &gtk::Paned, ratio: f64| {
            clamp_paned_position(
                paned,
                layout_state::split_position_from_ratio(ratio, paned.width()),
            )
        };

        wait_for(
            || {
                outer_paned.width() >= 1000
                    && (outer_paned.position() - expected_pos(&outer_paned, 0.35)).abs() <= 4
                    && (inner_paned.position() - expected_pos(&inner_paned, 0.60)).abs() <= 4
            },
            "initial nested horizontal split did not settle",
        );

        // 1. Zoom leaf_a, shrink window from 1100 -> 760, then unzoom.
        apply_zoom_visibility(&bin, &leaf_a);
        wait_for(
            || leaf_a.width() == bin.width(),
            "leaf_a did not expand on zoom",
        );
        window.set_default_size(760, 600);
        wait_for(
            || outer_paned.width() <= 800,
            "window did not shrink while zoomed",
        );

        restore_zoom_visibility_and_ratios(outer_paned.upcast_ref(), &tree);
        wait_for(
            || {
                leaf_a.is_mapped()
                    && leaf_b.is_mapped()
                    && leaf_c.is_mapped()
                    && !is_applying_split_ratio(&outer_paned)
                    && !is_applying_split_ratio(&inner_paned)
                    && (outer_paned.position() - expected_pos(&outer_paned, 0.35)).abs() <= 4
                    && (inner_paned.position() - expected_pos(&inner_paned, 0.60)).abs() <= 4
            },
            "unzoom after shrink did not restore saved ratios",
        );
        assert!(
            (*outer_ratio.borrow() - 0.35).abs() < 1e-6,
            "outer ratio drifted after shrink-while-zoomed: {}",
            *outer_ratio.borrow()
        );
        assert!(
            (*inner_ratio.borrow() - 0.60).abs() < 1e-6,
            "inner ratio drifted after shrink-while-zoomed: {}",
            *inner_ratio.borrow()
        );

        // 2. Zoom leaf_b, grow window from 760 -> 1200, then unzoom.
        apply_zoom_visibility(&bin, &leaf_b);
        wait_for(
            || leaf_b.width() == bin.width(),
            "leaf_b did not expand on zoom",
        );
        window.set_default_size(1200, 600);
        wait_for(
            || outer_paned.width() >= 1150,
            "window did not grow while zoomed",
        );

        restore_zoom_visibility_and_ratios(outer_paned.upcast_ref(), &tree);
        wait_for(
            || {
                leaf_a.is_mapped()
                    && leaf_b.is_mapped()
                    && leaf_c.is_mapped()
                    && !is_applying_split_ratio(&outer_paned)
                    && !is_applying_split_ratio(&inner_paned)
                    && (outer_paned.position() - expected_pos(&outer_paned, 0.35)).abs() <= 4
                    && (inner_paned.position() - expected_pos(&inner_paned, 0.60)).abs() <= 4
            },
            "unzoom after grow did not restore saved ratios",
        );
        assert!(
            (*outer_ratio.borrow() - 0.35).abs() < 1e-6,
            "outer ratio drifted after grow-while-zoomed: {}",
            *outer_ratio.borrow()
        );
        assert!(
            (*inner_ratio.borrow() - 0.60).abs() < 1e-6,
            "inner ratio drifted after grow-while-zoomed: {}",
            *inner_ratio.borrow()
        );

        window.close();
    }
}
