use super::*;

use crate::layout_state::{SplitOrientation, SplitState};

fn pump_until(timeout: std::time::Duration, mut done: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + timeout;
    while !done() && std::time::Instant::now() < deadline {
        while glib::MainContext::default().pending() {
            glib::MainContext::default().iteration(false);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn pump_for(duration: std::time::Duration) {
    pump_until(duration, || false);
}

/// `root` and every widget below it.
fn widget_tree(root: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut widgets = vec![root.clone()];
    let mut child = root.first_child();
    while let Some(widget) = child {
        widgets.extend(widget_tree(&widget));
        child = widget.next_sibling();
    }
    widgets
}

fn weak_refs(widgets: &[gtk::Widget]) -> Vec<glib::WeakRef<gtk::Widget>> {
    widgets.iter().map(|widget| widget.downgrade()).collect()
}

fn survivors(refs: &[glib::WeakRef<gtk::Widget>]) -> Vec<&'static str> {
    refs.iter()
        .filter_map(|weak| weak.upgrade())
        .map(|widget| widget.type_().name())
        .collect()
}

fn assert_freed(what: &str, refs: &[glib::WeakRef<gtk::Widget>]) {
    pump_until(std::time::Duration::from_secs(5), || {
        survivors(refs).is_empty()
    });
    let alive = survivors(refs);
    assert!(
        alive.is_empty(),
        "{what}: {} of {} widgets still alive: {alive:?}",
        alive.len(),
        refs.len()
    );
}

fn focus_within(state: &State, widget: &gtk::Widget) -> bool {
    let window = state.borrow().window.clone();
    gtk::prelude::GtkWindowExt::focus(&window)
        .is_some_and(|focus| &focus == widget || focus.is_ancestor(widget))
}

fn assert_focus_within(what: &str, state: &State, widget: &gtk::Widget) {
    pump_until(std::time::Duration::from_secs(5), || {
        focus_within(state, widget)
    });
    let window = state.borrow().window.clone();
    let focus = gtk::prelude::GtkWindowExt::focus(&window);
    assert!(
        focus_within(state, widget),
        "{what}: the focus is on {:?}",
        focus.map(|focus| focus.type_().name())
    );
}

fn shown(widget: &gtk::Widget) -> bool {
    widget.is_mapped() && widget.width() > 0
}

/// Right-clicks `widget` and returns the menu it opens.
fn open_context_menu(widget: &gtk::Widget) -> gtk::Popover {
    let controllers = widget.observe_controllers();
    let right_click = (0..controllers.n_items())
        .filter_map(|i| controllers.item(i).and_downcast::<gtk::GestureClick>())
        .find(|gesture| gesture.button() == 3)
        .expect("a right-click gesture");
    right_click.emit_by_name::<()>("pressed", &[&1i32, &10.0f64, &10.0f64]);
    widget
        .last_child()
        .and_downcast::<gtk::Popover>()
        .expect("a context menu")
}

fn menu_item(menu: &gtk::Popover, label: &str) -> gtk::Button {
    widget_tree(menu.upcast_ref())
        .into_iter()
        .filter_map(|widget| widget.downcast::<gtk::Button>().ok())
        .find(|button| button.label().is_some_and(|text| text == label))
        .unwrap_or_else(|| panic!("a {label:?} menu item"))
}

/// Focuses the menu item labelled `label`, as pressing it does.
fn focus_menu_item(menu: &gtk::Popover, label: &str) -> gtk::Button {
    let item = menu_item(menu, label);
    assert!(item.grab_focus());
    item
}

fn split_options() -> SplitPaneOptions {
    SplitPaneOptions {
        initial_state: None,
        skip_default_tab: false,
        inherit_active_directory: false,
        new_pane_first: false,
        persist: false,
        suppress_initial_autostart: false,
    }
}

/// Adds a workspace, shows it, focuses its last pane, and returns its id and
/// root.
fn show_focused_workspace(state: &State, layout: LayoutNodeState) -> (String, gtk::Widget) {
    add_workspace_from_state(
        state,
        &WorkspaceState {
            color: None,
            group_id: None,
            id: None,
            name: "closed".to_string(),
            favorite: false,
            cwd: None,
            folder_path: None,
            autostart_command: None,
            layout,
        },
    );
    let (index, ws_id, root) = {
        let s = state.borrow();
        let index = s.workspaces.len() - 1;
        let ws = &s.workspaces[index];
        (index, ws.id.clone(), ws.root.clone())
    };
    select_workspace_by_index(state, index);
    let panes = || {
        let mut panes = Vec::new();
        for start in [true, false] {
            let pane = find_leaf_pane(&root, gtk::Orientation::Horizontal, start);
            if !panes.contains(&pane) {
                panes.push(pane);
            }
        }
        panes
    };
    pump_until(std::time::Duration::from_secs(5), || {
        panes().iter().all(shown)
    });
    let last = panes().pop().unwrap();
    assert!(shown(&last), "workspace panes never showed");
    assert!(pane::focus_active_tab_in_pane(&last));
    pump_for(std::time::Duration::from_millis(200));
    assert!(focus_within(state, &root), "the workspace has the focus");
    (ws_id, root)
}

// Issue #202: a closed pane stayed alive for the life of the process, held by
// its own handlers. With the focus inside, GTK 4.22 also kept what was closed
// (see `terminal::unset_focus_within`), so every close here starts focused.
#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn closed_tabs_panes_and_workspaces_free_their_widgets() {
    let temp = tempfile::tempdir().unwrap();
    for key in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"] {
        let path = temp.path().join(key);
        std::fs::create_dir_all(&path).unwrap();
        std::env::set_var(key, path);
    }

    crate::prepare_ghostty_runtime();
    adw::init().unwrap();
    crate::terminal::init_ghostty();
    let app = adw::Application::builder()
        .application_id("dev.limux.PaneCloseTest")
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    build_window(&app);
    let state = CONTROL_STATE.with(|slot| slot.borrow().as_ref().unwrap().clone());
    let wait = std::time::Duration::from_secs(5);
    let (ws_id, first) = {
        let s = state.borrow();
        let ws = s.active_workspace().unwrap();
        let first = find_leaf_pane(&ws.root, gtk::Orientation::Horizontal, true);
        (ws.id.clone(), first)
    };
    pump_until(wait, || shown(&first));

    // A tab closed with another one left to replace it.
    pane::add_terminal_tab_to_pane(&first);
    assert!(pane::focus_active_tab_in_pane(&first));
    pump_for(std::time::Duration::from_millis(200));
    let before = widget_tree(&first);
    let tab_id = pane::active_tab_in_pane(&first).unwrap();
    assert!(pane::close_tab_in_pane(&first, &tab_id));
    // Its content leaves the stack after a frame.
    pump_for(std::time::Duration::from_millis(500));
    let kept = widget_tree(&first);
    let closed_tab: Vec<_> = before.into_iter().filter(|w| !kept.contains(w)).collect();
    assert!(
        closed_tab.iter().any(|w| w.is::<gtk::GLArea>()),
        "the tab's terminal left the pane"
    );
    let refs = weak_refs(&closed_tab);
    drop((closed_tab, kept));
    assert_freed("closed tab", &refs);

    // A pane closed through its only tab, as typing `exit` does.
    let lone = split_pane(
        &state,
        &ws_id,
        &first,
        gtk::Orientation::Horizontal,
        split_options(),
    )
    .expect("split");
    pump_until(wait, || shown(&lone));
    assert!(pane::focus_active_tab_in_pane(&lone));
    pump_for(std::time::Duration::from_millis(200));
    let refs = weak_refs(&widget_tree(&lone));
    let tab_id = pane::active_tab_in_pane(&lone).unwrap();
    assert!(pane::close_tab_in_pane(&lone, &tab_id));
    drop(lone);
    assert_freed("pane closed by its last tab", &refs);
    assert_focus_within("pane closed by its last tab", &state, &first);

    // A pane with two terminal tabs and the keybinds tab, whose tab and
    // terminal menus were used and whose tab is being renamed, closed as its
    // close button does.
    let closed = split_pane(
        &state,
        &ws_id,
        &first,
        gtk::Orientation::Horizontal,
        split_options(),
    )
    .expect("split");
    let shortcuts = state.borrow().shortcuts.clone();
    pane::add_keybind_editor_tab_to_pane(&closed, shortcuts, Rc::new(|_, _| Err(String::new())));
    pane::add_terminal_tab_to_pane(&closed);
    pump_until(wait, || shown(&closed));
    assert!(shown(&closed), "split pane never showed");
    assert_eq!(pane::tab_count_in_pane(&closed), 3);
    assert!(pane::focus_active_tab_in_pane(&closed));
    pump_for(std::time::Duration::from_millis(200));

    let terminal = widget_tree(&closed)
        .into_iter()
        .find(|w| w.is::<gtk::GLArea>() && w.is_mapped())
        .expect("the active terminal");
    // A synthetic right-click has no seat/grab under headless Weston, so the
    // production entry point correctly detaches its refused popup immediately.
    // Build the same menu with the real terminal callbacks and retain it here
    // to exercise focus restoration and lifetime cleanup on close. Refused
    // popup detachment has its own terminal regression.
    let terminal_menu = pane::terminal_handle_for_surface(&closed, None)
        .expect("the active terminal handle")
        .1
        .build_context_menu_for_test();
    terminal_menu.popup();
    focus_menu_item(&terminal_menu, "Paste");
    menu_item(&terminal_menu, "Copy Surface ID").emit_clicked();
    assert!(terminal_menu.parent().is_none(), "the terminal menu closed");
    let toast = widget_tree(&closed)
        .into_iter()
        .find(|widget| widget.has_css_class("limux-toast"))
        .expect("copying the surface ID showed a toast")
        .downgrade();
    assert!(
        focus_within(&state, &terminal),
        "the terminal menu gives the focus back to its terminal as it closes"
    );

    let tab = widget_tree(&closed)
        .into_iter()
        .find(|w| w.has_css_class("limux-tab"))
        .expect("a tab button");
    let pin_menu = open_context_menu(&tab);
    focus_menu_item(&pin_menu, "Pin").emit_clicked();
    assert!(pin_menu.parent().is_none(), "the tab menu closed");
    assert_focus_within("tab menu closed", &state, &closed);
    let tab_menu = open_context_menu(&tab);
    focus_menu_item(&tab_menu, "Rename").emit_clicked();
    assert!(tab_menu.parent().is_none(), "the tab menu closed");
    pump_until(wait, || pane::find_tab_rename_entry(&closed).is_some());
    assert!(
        pane::find_tab_rename_entry(&closed).is_some(),
        "a rename is open"
    );
    assert!(
        active_inline_rename_entry_in(&closed).is_some(),
        "tab rename entry should be indexed for O(1) lookup"
    );

    let mut refs = weak_refs(&widget_tree(&closed));
    refs.push(toast);
    refs.push(terminal_menu.upcast::<gtk::Widget>().downgrade());
    refs.push(tab_menu.upcast::<gtk::Widget>().downgrade());
    refs.push(pin_menu.upcast::<gtk::Widget>().downgrade());
    drop((terminal, tab));
    remove_pane(&state, &ws_id, &closed);
    drop(closed);
    assert_freed("closed pane", &refs);
    assert!(
        active_inline_rename_entry_in(&state.borrow().window).is_none(),
        "closing a pane with an open tab rename must clear the rename index"
    );
    assert_focus_within("closed pane", &state, &first);

    // Two panes closed in one tick, as two shells exiting together: the
    // focus stays where it was.
    let doomed: Vec<_> = (0..2)
        .map(|_| {
            split_pane(
                &state,
                &ws_id,
                &first,
                gtk::Orientation::Horizontal,
                split_options(),
            )
            .expect("split")
        })
        .collect();
    pump_until(wait, || doomed.iter().all(shown));
    assert!(pane::focus_active_tab_in_pane(&first));
    pump_for(std::time::Duration::from_millis(200));
    assert!(focus_within(&state, &first), "the first pane has the focus");
    let refs = weak_refs(&doomed.iter().flat_map(widget_tree).collect::<Vec<_>>());
    for pane in &doomed {
        remove_pane(&state, &ws_id, pane);
    }
    drop(doomed);
    assert_freed("two panes closed in one tick", &refs);
    assert_focus_within("two panes closed in one tick", &state, &first);

    // Workspaces: split, then single-pane, closed directly and through the
    // last terminal's tab. The focus goes back to where it was in the
    // workspace shown instead: `other`, not its first pane.
    let other = split_pane(
        &state,
        &ws_id,
        &first,
        gtk::Orientation::Horizontal,
        split_options(),
    )
    .expect("split");
    pump_until(wait, || shown(&other));
    assert!(pane::focus_active_tab_in_pane(&other));
    pump_for(std::time::Duration::from_millis(200));
    drop(first);
    let split = LayoutNodeState::Split(SplitState {
        orientation: SplitOrientation::Horizontal,
        ratio: 0.5,
        start: Box::new(LayoutNodeState::Pane(PaneState::fallback(None))),
        end: Box::new(LayoutNodeState::Pane(PaneState::fallback(None))),
    });
    let single = || LayoutNodeState::Pane(PaneState::fallback(None));
    for (what, layout, by_last_tab) in [
        ("closed split workspace", split, false),
        ("closed single-pane workspace", single(), false),
        ("workspace closed by its last tab", single(), true),
    ] {
        let (ws_id, root) = show_focused_workspace(&state, layout);
        let refs = weak_refs(&widget_tree(&root));
        if by_last_tab {
            let pane = find_leaf_pane(&root, gtk::Orientation::Horizontal, true);
            let tab_id = pane::active_tab_in_pane(&pane).unwrap();
            assert!(pane::close_tab_in_pane(&pane, &tab_id));
        } else {
            close_workspace_by_id(&state, &ws_id);
        }
        drop(root);
        assert_freed(what, &refs);
        assert_focus_within(what, &state, &other);
    }

    // A workspace kept open after its last terminal exits: the pane stays,
    // the terminal goes although it had the focus.
    state
        .borrow()
        .config
        .borrow_mut()
        .workspace
        .keep_open_after_last_terminal_closes = true;
    let (_, root) = show_focused_workspace(&state, single());
    let kept_pane = find_leaf_pane(&root, gtk::Orientation::Horizontal, true);
    let before = widget_tree(&kept_pane);
    let tab_id = pane::active_tab_in_pane(&kept_pane).unwrap();
    assert!(pane::close_tab_in_pane(&kept_pane, &tab_id));
    pump_for(std::time::Duration::from_millis(500));
    assert!(kept_pane.parent().is_some(), "the pane was kept");
    let kept = widget_tree(&kept_pane);
    let closed_tab: Vec<_> = before.into_iter().filter(|w| !kept.contains(w)).collect();
    assert!(
        closed_tab.iter().any(|w| w.is::<gtk::GLArea>()),
        "the terminal left the kept pane"
    );
    let refs = weak_refs(&closed_tab);
    drop((closed_tab, kept, kept_pane, root));
    assert_freed("terminal of a kept-open workspace", &refs);

    let window = state.borrow().window.clone();
    window.close();
}

fn registered_panes() -> Vec<gtk::Widget> {
    (0..1000).filter_map(pane::find_pane_widget_by_id).collect()
}

/// Splits `first` with a new pane before it, shows both and focuses the new
/// one, so closing it closes the first leaf of the tree.
fn split_before(state: &State, ws_id: &str, first: &gtk::Widget) -> gtk::Widget {
    let lead = split_pane(
        state,
        ws_id,
        first,
        gtk::Orientation::Horizontal,
        SplitPaneOptions {
            new_pane_first: true,
            ..split_options()
        },
    )
    .expect("split");
    pump_until(std::time::Duration::from_secs(5), || {
        shown(&lead) && shown(first)
    });
    assert!(shown(&lead) && shown(first), "split panes never showed");
    assert!(pane::focus_active_tab_in_pane(&lead));
    pump_for(std::time::Duration::from_millis(200));
    lead
}

// A zoom or split within one frame of a pane close used to target the closed
// pane: the old widget tree stays in the workspace until the next frame, and
// its first leaf was found when the focus sat in no pane.
#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn zoom_and_split_within_a_frame_of_a_close_skip_the_closed_pane() {
    let temp = tempfile::tempdir().unwrap();
    for key in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"] {
        let path = temp.path().join(key);
        std::fs::create_dir_all(&path).unwrap();
        std::env::set_var(key, path);
    }

    crate::prepare_ghostty_runtime();
    adw::init().unwrap();
    crate::terminal::init_ghostty();
    let app = adw::Application::builder()
        .application_id("dev.limux.PaneCloseZoomSplitTest")
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    build_window(&app);
    let state = CONTROL_STATE.with(|slot| slot.borrow().as_ref().unwrap().clone());
    let wait = std::time::Duration::from_secs(5);
    let (ws_id, first, container) = {
        let s = state.borrow();
        let ws = s.active_workspace().unwrap();
        let first = find_leaf_pane(&ws.root, gtk::Orientation::Horizontal, true);
        (ws.id.clone(), first, ws.split_container.clone())
    };
    pump_until(wait, || shown(&first));

    // Zoom right after the first leaf closed.
    let lead = split_before(&state, &ws_id, &first);
    remove_pane(&state, &ws_id, &lead);
    toggle_focused_pane_zoom(&state);
    pump_until(wait, || shown(&first));
    assert!(shown(&first), "the zoom left the workspace blank");
    assert!(!container.contains(&lead), "the closed pane is in the tree");
    toggle_focused_pane_zoom(&state);
    pump_for(std::time::Duration::from_millis(300));
    drop(lead);

    // Directional lookups skip it too.
    let lead = split_before(&state, &ws_id, &first);
    remove_pane(&state, &ws_id, &lead);
    assert!(
        pane_in_direction(&state, &first, Direction::Left).is_none(),
        "found the closed pane"
    );
    drop(lead);
    pump_for(std::time::Duration::from_millis(300));

    // A split of the closed pane itself builds nothing.
    let lead = split_before(&state, &ws_id, &first);
    remove_pane(&state, &ws_id, &lead);
    let before = registered_panes().len();
    assert!(
        split_pane(
            &state,
            &ws_id,
            &lead,
            gtk::Orientation::Horizontal,
            split_options()
        )
        .is_none(),
        "split a closed pane"
    );
    assert_eq!(registered_panes().len(), before, "an orphaned pane");
    drop(lead);
    pump_for(std::time::Duration::from_millis(300));

    // Split right after the first leaf closed.
    let lead = split_before(&state, &ws_id, &first);
    remove_pane(&state, &ws_id, &lead);
    let before = registered_panes().len();
    split_focused_pane(&state, gtk::Orientation::Horizontal);
    pump_for(std::time::Duration::from_millis(500));
    let panes = registered_panes();
    assert_eq!(panes.len(), before + 1, "the split built one pane");
    assert!(
        panes.iter().all(shown),
        "a registered pane is not in the tree"
    );

    let window = state.borrow().window.clone();
    window.close();
}

// The workspace menu's items held their own popover, a cycle that kept it and
// its row parent chain alive after every use. The headless compositor has no
// seat, so the popover is never presented nor closed: detach it by hand, as
// its `closed` handler would, and require that nothing else retains it.
#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn workspace_context_menu_never_retains_its_popover() {
    let temp = tempfile::tempdir().unwrap();
    for key in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"] {
        let path = temp.path().join(key);
        std::fs::create_dir_all(&path).unwrap();
        std::env::set_var(key, path);
    }

    crate::prepare_ghostty_runtime();
    adw::init().unwrap();
    crate::terminal::init_ghostty();
    let app = adw::Application::builder()
        .application_id("dev.limux.WorkspaceMenuTest")
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    build_window(&app);
    let state = CONTROL_STATE.with(|slot| slot.borrow().as_ref().unwrap().clone());
    let row = state.borrow().workspaces[0].sidebar_row.clone();
    pump_for(std::time::Duration::from_millis(200));

    let menu = open_context_menu(row.upcast_ref());
    menu_item(&menu, "Rename").emit_clicked();
    let refs = vec![menu.clone().upcast::<gtk::Widget>().downgrade()];
    menu.unparent();
    drop(menu);
    assert_freed("workspace context menu", &refs);
}

// Starts an inline rename with the focus in a terminal and commits it with
// Enter, as a user would: the entry is removed while it holds the focus, so
// unless the commit moves the focus first, GTK hands it to the next focusable
// widget in the row, the star button.
#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn inline_rename_commit_returns_the_focus_to_the_workspace() {
    let temp = tempfile::tempdir().unwrap();
    for key in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"] {
        let path = temp.path().join(key);
        std::fs::create_dir_all(&path).unwrap();
        std::env::set_var(key, path);
    }

    crate::prepare_ghostty_runtime();
    adw::init().unwrap();
    crate::terminal::init_ghostty();
    let app = adw::Application::builder()
        .application_id("dev.limux.InlineRenameFocusTest")
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    build_window(&app);
    let state = CONTROL_STATE.with(|slot| slot.borrow().as_ref().unwrap().clone());
    let (ws_id, root) =
        show_focused_workspace(&state, LayoutNodeState::Pane(PaneState::fallback(None)));

    begin_workspace_inline_rename(&state, &ws_id);
    let sidebar_list = state.borrow().sidebar_list.clone();
    let entry = find_active_rename_entry(&sidebar_list).expect("rename entry");
    assert_eq!(
        active_inline_rename_entry_in(&sidebar_list).as_ref(),
        Some(&entry)
    );
    assert_focus_within("rename started", &state, entry.upcast_ref());

    entry.set_text("renamed");
    entry.emit_activate();
    assert_focus_within("rename committed", &state, &root);
    {
        let s = state.borrow();
        let ws = s.workspaces.iter().find(|ws| ws.id == ws_id).unwrap();
        assert_eq!(ws.name, "renamed");
    }
    assert!(find_active_rename_entry(&sidebar_list).is_none());
    assert!(active_inline_rename_entry_in(&sidebar_list).is_none());

    // A click on a terminal commits the rename on press (the window's
    // capture-phase `click_anywhere`), then the terminal takes the focus. The
    // commit must leave it there, not pull it to the workspace's first pane.
    let split = LayoutNodeState::Split(SplitState {
        orientation: SplitOrientation::Horizontal,
        ratio: 0.5,
        start: Box::new(LayoutNodeState::Pane(PaneState::fallback(None))),
        end: Box::new(LayoutNodeState::Pane(PaneState::fallback(None))),
    });
    let (ws_id, root) = show_focused_workspace(&state, split);
    let clicked = find_leaf_pane(&root, gtk::Orientation::Horizontal, false);
    begin_workspace_inline_rename(&state, &ws_id);
    let entry = find_active_rename_entry(&sidebar_list).expect("rename entry");
    assert_focus_within("split rename started", &state, entry.upcast_ref());
    entry.emit_activate();
    assert!(find_gl_area(&clicked).expect("terminal").grab_focus());
    pump_for(std::time::Duration::from_millis(200));
    assert_focus_within("rename committed by a click", &state, &clicked);
}

#[test]
#[ignore = "requires an active X11 window and Ghostty resources"]
fn passive_rename_focus_restore_preserves_manual_tab_unread() {
    let temp = tempfile::tempdir().unwrap();
    for key in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"] {
        let path = temp.path().join(key);
        std::fs::create_dir_all(&path).unwrap();
        std::env::set_var(key, path);
    }
    let socket = temp.path().join("limux.sock");
    std::env::set_var("LIMUX_SOCKET", &socket);
    std::env::set_var("LIMUX_SOCKET_PATH", &socket);
    crate::prepare_ghostty_runtime();
    adw::init().unwrap();
    crate::terminal::init_ghostty();
    let app = adw::Application::builder()
        .application_id("dev.limux.UnreadFocusRestoreTest")
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    build_window(&app);
    let state = CONTROL_STATE.with(|slot| slot.borrow().as_ref().unwrap().clone());
    let (ws_id, root) =
        show_focused_workspace(&state, LayoutNodeState::Pane(PaneState::fallback(None)));
    let pane = first_leaf_pane(&root);
    let tab_id = pane::active_tab_in_pane(&pane).unwrap();
    let (window, row, sidebar_list) = {
        let s = state.borrow();
        let ws = s.workspaces.iter().find(|ws| ws.id == ws_id).unwrap();
        (
            s.window.clone(),
            ws.sidebar_row.clone(),
            s.sidebar_list.clone(),
        )
    };
    let wait = std::time::Duration::from_secs(5);
    pump_until(wait, || window.is_active());
    assert!(window.is_active(), "Limux must start active");
    assert!(pane::mark_active_tab_manual_unread_in_root(&root));
    sync_workspace_unread(&state, &ws_id);

    let menu = open_context_menu(row.upcast_ref());
    menu_item(&menu, "Rename").emit_clicked();
    pump_until(wait, || find_active_rename_entry(&sidebar_list).is_some());
    let entry = find_active_rename_entry(&sidebar_list).expect("rename entry");
    assert_focus_within("rename started", &state, entry.upcast_ref());
    entry.set_text("renamed while inactive");

    let peer = adw::ApplicationWindow::builder()
        .application(&app)
        .title("Focus peer")
        .build();
    peer.present();
    pump_until(wait, || peer.is_active() && !window.is_active());
    assert!(peer.is_active() && !window.is_active());
    assert!(find_active_rename_entry(&sidebar_list).is_none());
    assert!(state.borrow().pending_focus_restore.is_some());
    assert!(pane::workspace_has_unread_tabs(&ws_id));
    assert_eq!(
        state.borrow().active_workspace().unwrap().name,
        entry.text()
    );

    window.present();
    pump_until(wait, || window.is_active());
    assert!(window.is_active());
    assert_focus_within("passive activation restores terminal focus", &state, &pane);
    assert!(state.borrow().pending_focus_restore.is_none());
    assert!(
        pane::workspace_has_unread_tabs(&ws_id),
        "passive rename focus restoration must preserve a manual tab marker"
    );
    assert!(pane::activate_tab_in_pane(&pane, &tab_id));
    assert!(
        !pane::workspace_has_unread_tabs(&ws_id),
        "explicit tab activation still acknowledges its marker"
    );
    peer.close();
    window.close();
}

#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn tab_unread_changes_preserve_independent_workspace_notifications() {
    let temp = tempfile::tempdir().unwrap();
    for key in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"] {
        let path = temp.path().join(key);
        std::fs::create_dir_all(&path).unwrap();
        std::env::set_var(key, path);
    }
    let ghostty_config = temp.path().join("XDG_CONFIG_HOME/ghostty");
    std::fs::create_dir_all(&ghostty_config).unwrap();
    std::fs::write(
        ghostty_config.join("config"),
        "command = /bin/sh\nshell-integration = none\n",
    )
    .unwrap();
    let socket = temp.path().join("limux.sock");
    std::env::set_var("LIMUX_SOCKET", &socket);
    std::env::set_var("LIMUX_SOCKET_PATH", &socket);

    crate::prepare_ghostty_runtime();
    adw::init().unwrap();
    crate::terminal::init_ghostty();
    let app = adw::Application::builder()
        .application_id("dev.limux.UnreadSourcesTest")
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    build_window(&app);
    let state = CONTROL_STATE.with(|slot| slot.borrow().as_ref().unwrap().clone());
    let (ws_id, root) =
        show_focused_workspace(&state, LayoutNodeState::Pane(PaneState::fallback(None)));
    let pane = first_leaf_pane(&root);
    pane::add_terminal_tab_to_pane(&pane);
    let surface = pane::active_surface_summary(&pane).unwrap();
    let (_, handle) = pane::terminal_handle_for_surface(&pane, Some(&surface.surface_id)).unwrap();
    let wait = std::time::Duration::from_secs(5);
    pump_until(wait, || handle.health().realized);
    assert!(handle.health().realized);
    select_workspace_by_index(&state, 0);

    let target = DesktopNotificationTarget {
        workspace_id: ws_id.clone(),
        pane_id: None,
        tab_id: None,
    };
    mark_workspace_unread_with_message(&state, &ws_id, "Workspace notice", false, target);
    let target = DesktopNotificationTarget {
        workspace_id: ws_id.clone(),
        pane_id: Some(surface.pane_id),
        tab_id: Some(surface.tab_id.clone()),
    };
    mark_workspace_unread_with_message(&state, &ws_id, "Tab notice", false, target);
    assert!(pane::workspace_has_unread_tabs(&ws_id));
    assert!(handle.send_text("exit"));
    assert!(handle.send_key("enter"));
    // Nobody watched it exit, so the tab and its output stay until closed.
    pump_until(wait, || handle.health().process_exited);
    pump_for(std::time::Duration::from_millis(300));
    assert_eq!(
        pane::tab_count_in_pane(&pane),
        2,
        "an unattended exit keeps its tab"
    );
    assert!(pane::close_tab_in_pane(&pane, &surface.tab_id));
    pump_until(wait, || pane::tab_count_in_pane(&pane) == 1);
    assert_eq!(pane::tab_count_in_pane(&pane), 1, "unread terminal exited");
    let (row, notify_dot) = {
        let s = state.borrow();
        let workspace = s.workspaces.iter().find(|w| w.id == ws_id).unwrap();
        assert!(
            workspace.unread,
            "a terminal exit must preserve the independent workspace notice"
        );
        (
            workspace.sidebar_row.clone(),
            workspace.indicator_unread_dot.clone(),
        )
    };
    assert!(!pane::workspace_has_unread_tabs(&ws_id));
    assert!(notify_dot.get_visible());

    let menu = open_context_menu(row.upcast_ref());
    menu_item(&menu, "Mark as Read").emit_clicked();
    pump_until(wait, || !notify_dot.get_visible());
    assert!(!notify_dot.get_visible());
    let index = state
        .borrow()
        .workspaces
        .iter()
        .position(|w| w.id == ws_id)
        .unwrap();
    select_workspace_by_index(&state, index);
    pane::add_terminal_tab_to_pane(&pane);
    assert!(pane::mark_active_tab_manual_unread_in_root(&root));
    sync_workspace_unread(&state, &ws_id);
    assert!(pane::workspace_has_unread_tabs(&ws_id));
    assert!(notify_dot.get_visible());
    assert!(!pane::clear_active_tab_unread_in_root_if_not_manual(&root));
    assert!(pane::workspace_has_unread_tabs(&ws_id));
    let menu = open_context_menu(row.upcast_ref());
    menu_item(&menu, "Mark as Read").emit_clicked();
    pump_until(wait, || !pane::workspace_has_unread_tabs(&ws_id));
    assert!(!pane::workspace_has_unread_tabs(&ws_id));
    assert!(
        !notify_dot.get_visible(),
        "reading a tab-only notice clears it"
    );

    let menu = open_context_menu(row.upcast_ref());
    menu_item(&menu, "Mark as Unread").emit_clicked();
    pump_until(wait, || notify_dot.get_visible());
    {
        let s = state.borrow();
        let workspace = s.workspaces.iter().find(|w| w.id == ws_id).unwrap();
        assert!(workspace.manual_unread);
        assert!(
            !workspace.unread,
            "manual and notification sources are separate"
        );
    }
    let surface = pane::active_surface_summary(&pane).unwrap();
    let (_, handle) = pane::terminal_handle_for_surface(&pane, Some(&surface.surface_id)).unwrap();
    pump_until(wait, || handle.health().realized);
    // Only a watched exit closes on its own; Weston headless has no active
    // window, so there nobody is watching and the exit is held until closed.
    let watched = state.borrow().window.is_active();
    assert!(handle.send_text("exit"));
    assert!(handle.send_key("enter"));
    pump_until(wait, || {
        handle.health().process_exited || pane::tab_count_in_pane(&pane) == 1
    });
    if !watched {
        pump_for(std::time::Duration::from_millis(300));
        assert_eq!(
            pane::tab_count_in_pane(&pane),
            2,
            "an unwatched exit keeps its tab"
        );
        assert!(pane::close_tab_in_pane(&pane, &surface.tab_id));
    }
    pump_until(wait, || pane::tab_count_in_pane(&pane) == 1);
    assert_eq!(pane::tab_count_in_pane(&pane), 1);
    assert!(
        state.borrow().workspaces[index].manual_unread,
        "a terminal exit must preserve the independent manual workspace reminder"
    );
    assert!(notify_dot.get_visible());
    let menu = open_context_menu(row.upcast_ref());
    menu_item(&menu, "Mark as Read").emit_clicked();
    pump_until(wait, || !notify_dot.get_visible());
    assert!(!notify_dot.get_visible());
    assert!(!pane::workspace_has_unread_tabs(&ws_id));

    pane::add_terminal_tab_to_pane(&pane);
    let tab_id = pane::active_tab_in_pane(&pane).unwrap();
    let pane_id = pane::active_surface_summary(&pane).unwrap().pane_id;
    assert_eq!(
        pane::mark_tab_unread_in_workspace(&ws_id, pane_id, &tab_id),
        Some(true)
    );
    sync_workspace_unread(&state, &ws_id);
    assert!(notify_dot.get_visible());
    assert!(pane::close_tab_in_pane(&pane, &tab_id));
    assert!(!pane::workspace_has_unread_tabs(&ws_id));
    assert!(
        !notify_dot.get_visible(),
        "closing a tab-only notice clears it"
    );

    let window = state.borrow().window.clone();
    window.close();
    pump_until(wait, || !window.is_visible());
}

#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn an_exit_driven_through_the_control_socket_closes_its_tab_unwatched() {
    let temp = tempfile::tempdir().unwrap();
    for key in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"] {
        let path = temp.path().join(key);
        std::fs::create_dir_all(&path).unwrap();
        std::env::set_var(key, path);
    }
    let ghostty_config = temp.path().join("XDG_CONFIG_HOME/ghostty");
    std::fs::create_dir_all(&ghostty_config).unwrap();
    std::fs::write(
        ghostty_config.join("config"),
        "command = /bin/sh\nshell-integration = none\n",
    )
    .unwrap();
    let socket = temp.path().join("limux.sock");
    std::env::set_var("LIMUX_SOCKET", &socket);
    std::env::set_var("LIMUX_SOCKET_PATH", &socket);

    crate::prepare_ghostty_runtime();
    adw::init().unwrap();
    crate::terminal::init_ghostty();
    let app = adw::Application::builder()
        .application_id("dev.limux.DrivenExitTest")
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    build_window(&app);
    let state = CONTROL_STATE.with(|slot| slot.borrow().as_ref().unwrap().clone());
    let (ws_id, root) =
        show_focused_workspace(&state, LayoutNodeState::Pane(PaneState::fallback(None)));
    let pane = first_leaf_pane(&root);
    pane::add_terminal_tab_to_pane(&pane);
    let surface = pane::active_surface_summary(&pane).unwrap();
    let (_, handle) = pane::terminal_handle_for_surface(&pane, Some(&surface.surface_id)).unwrap();
    let wait = std::time::Duration::from_secs(5);
    pump_until(wait, || handle.health().realized);
    assert_eq!(pane::tab_count_in_pane(&pane), 2);

    // Nobody is watching: another window has the focus (on Weston headless
    // no window ever has it).
    let peer = adw::ApplicationWindow::builder()
        .application(&app)
        .title("Focus peer")
        .build();
    peer.present();
    let window = state.borrow().window.clone();
    pump_until(wait, || !window.is_active());
    assert!(!window.is_active());

    // A script ends the shell through the socket, as `limux send` and
    // `limux send-key` do: the tab closes as if watched.
    let target = || WorkspaceTarget::Handle(ws_id.clone());
    let (reply, rx) = std::sync::mpsc::channel();
    handle_control_command(
        &state,
        ControlCommand::SendText {
            target: target(),
            surface_hint: Some(surface.surface_id.clone()),
            text: "exit".to_string(),
            reply,
        },
    );
    rx.recv_timeout(wait).unwrap().expect("surface.send_text");
    let (reply, rx) = std::sync::mpsc::channel();
    handle_control_command(
        &state,
        ControlCommand::SendKey {
            target: target(),
            surface_hint: Some(surface.surface_id.clone()),
            key: "enter".to_string(),
            reply,
        },
    );
    rx.recv_timeout(wait).unwrap().expect("surface.send_key");
    pump_until(wait, || pane::tab_count_in_pane(&pane) == 1);
    assert_eq!(
        pane::tab_count_in_pane(&pane),
        1,
        "an exit a script caused closes its tab"
    );
    peer.close();
    window.close();
}

// A command a script sent that then fails leaves output nobody has read: the
// tab is held and marked unread instead of closing with a clean driven exit.
#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn a_failed_exit_driven_through_the_control_socket_is_held() {
    let (state, _temp) = held_tab_test_window("dev.limux.DrivenFailureTest");
    let app = state.borrow().window.application().unwrap();
    let (ws_id, root) =
        show_focused_workspace(&state, LayoutNodeState::Pane(PaneState::fallback(None)));
    let pane = first_leaf_pane(&root);
    pane::add_terminal_tab_to_pane(&pane);
    let surface = pane::active_surface_summary(&pane).unwrap();
    let wait = std::time::Duration::from_secs(5);
    pump_until(wait, || live_terminal(&pane, &surface.surface_id));
    // Past the quick-failure window, as a command that ran for a while is.
    pump_for(std::time::Duration::from_millis(400));

    let peer = adw::ApplicationWindow::builder()
        .application(&app)
        .title("Focus peer")
        .build();
    peer.present();
    let window = state.borrow().window.clone();
    pump_until(wait, || !window.is_active());
    assert!(!window.is_active());

    let target = || WorkspaceTarget::Handle(ws_id.clone());
    // A key the terminal refuses is not input.
    let (reply, rx) = std::sync::mpsc::channel();
    handle_control_command(
        &state,
        ControlCommand::SendKey {
            target: target(),
            surface_hint: Some(surface.surface_id.clone()),
            key: "no-such-key".to_string(),
            reply,
        },
    );
    assert!(rx.recv_timeout(wait).unwrap().is_err());
    let (_, handle) = pane::terminal_handle_for_surface(&pane, Some(&surface.surface_id)).unwrap();
    assert!(
        !handle.driven_within(std::time::Duration::from_secs(60)),
        "a refused key does not count as driving the terminal"
    );

    let (reply, rx) = std::sync::mpsc::channel();
    handle_control_command(
        &state,
        ControlCommand::SendText {
            target: target(),
            surface_hint: Some(surface.surface_id.clone()),
            text: "exit 3".to_string(),
            reply,
        },
    );
    rx.recv_timeout(wait).unwrap().expect("surface.send_text");
    let (reply, rx) = std::sync::mpsc::channel();
    handle_control_command(
        &state,
        ControlCommand::SendKey {
            target: target(),
            surface_hint: Some(surface.surface_id.clone()),
            key: "enter".to_string(),
            reply,
        },
    );
    rx.recv_timeout(wait).unwrap().expect("surface.send_key");
    pump_until(wait, || handle.health().process_exited);
    pump_until(wait, || !buttons_labelled(&root, "Restart").is_empty());
    pump_for(std::time::Duration::from_millis(200));
    assert_eq!(
        pane::tab_count_in_pane(&pane),
        2,
        "a failed command a script sent keeps its tab"
    );
    assert_eq!(buttons_labelled(&root, "Restart").len(), 1);
    assert!(
        pane::workspace_has_unread_tabs(&ws_id),
        "the failure nobody saw is marked unread"
    );
    peer.close();
    window.close();
}

// A command that fails as soon as it starts keeps its error on screen even
// when its tab was built long before its terminal started (a restored project
// shown later): the quick-failure window counts the process's own runtime,
// not the tab's age.
#[test]
#[ignore = "requires a graphical display with an active window and Ghostty resources"]
fn a_quick_failure_in_a_late_started_terminal_is_held() {
    let temp = tempfile::tempdir().unwrap();
    for key in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"] {
        let path = temp.path().join(key);
        std::fs::create_dir_all(&path).unwrap();
        std::env::set_var(key, path);
    }
    let fail = temp.path().join("fail");
    let command = temp.path().join("command.sh");
    std::fs::write(
        &command,
        format!("[ -e '{}' ] && exit 3\nexec /bin/sh\n", fail.display()),
    )
    .unwrap();
    let ghostty_config = temp.path().join("XDG_CONFIG_HOME/ghostty");
    std::fs::create_dir_all(&ghostty_config).unwrap();
    std::fs::write(
        ghostty_config.join("config"),
        format!(
            "command = /bin/sh {}\nshell-integration = none\n",
            command.display()
        ),
    )
    .unwrap();
    let socket = temp.path().join("limux.sock");
    std::env::set_var("LIMUX_SOCKET", &socket);
    std::env::set_var("LIMUX_SOCKET_PATH", &socket);
    std::env::set_var("LIMUX_EAGER_RESTORE", "0");

    crate::prepare_ghostty_runtime();
    adw::init().unwrap();
    crate::terminal::init_ghostty();
    let app = adw::Application::builder()
        .application_id("dev.limux.LateStartFailureTest")
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    build_window(&app);
    let state = CONTROL_STATE.with(|slot| slot.borrow().as_ref().unwrap().clone());
    let wait = std::time::Duration::from_secs(5);
    let window = state.borrow().window.clone();
    pump_until(wait, || window.is_active());
    assert!(window.is_active(), "this test needs an active window");

    // A restored project builds its tabs at once and starts their terminals
    // only when it is first shown.
    add_restored_workspace_from_state(
        &state,
        &WorkspaceState {
            color: None,
            group_id: None,
            id: None,
            name: "late".to_string(),
            favorite: false,
            cwd: None,
            folder_path: None,
            autostart_command: None,
            layout: LayoutNodeState::Pane(PaneState::fallback(None)),
        },
    );
    let (index, ws_id, root) = {
        let s = state.borrow();
        let index = s.workspaces.len() - 1;
        let ws = &s.workspaces[index];
        (index, ws.id.clone(), ws.root.clone())
    };
    let pane = first_leaf_pane(&root);
    let surface = pane::active_surface_summary(&pane).unwrap();
    let (_, handle) = pane::terminal_handle_for_surface(&pane, Some(&surface.surface_id)).unwrap();
    pump_for(std::time::Duration::from_millis(600));
    assert!(!handle.health().realized, "the terminal has not started");

    std::fs::write(&fail, "").unwrap();
    select_workspace_by_index(&state, index);
    pump_until(wait, || handle.health().process_exited);
    pump_until(wait, || !buttons_labelled(&root, "Restart").is_empty());
    pump_for(std::time::Duration::from_millis(200));
    assert!(
        state.borrow().workspaces.iter().any(|w| w.id == ws_id),
        "the failed command's project was closed"
    );
    assert_eq!(
        buttons_labelled(&root, "Restart").len(),
        1,
        "a command that failed at once keeps its output and offers a restart"
    );
    assert!(
        !pane::workspace_has_unread_tabs(&ws_id),
        "the user watched it fail"
    );
    window.close();
}

#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn closing_inactive_workspace_preserves_active_manual_unread() {
    let temp = tempfile::tempdir().unwrap();
    for key in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"] {
        let path = temp.path().join(key);
        std::fs::create_dir_all(&path).unwrap();
        std::env::set_var(key, path);
    }

    crate::prepare_ghostty_runtime();
    adw::init().unwrap();
    crate::terminal::init_ghostty();
    let app = adw::Application::builder()
        .application_id("dev.limux.InactiveCloseUnreadTest")
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    build_window(&app);
    let state = CONTROL_STATE.with(|slot| slot.borrow().as_ref().unwrap().clone());
    let wait = std::time::Duration::from_secs(5);

    let _ = show_focused_workspace(&state, LayoutNodeState::Pane(PaneState::fallback(None)));
    pump_for(std::time::Duration::from_millis(200));
    assert!(state.borrow().workspaces.len() >= 2);

    let (inactive_id, active_id) = {
        let s = state.borrow();
        let active_idx = s.active_idx;
        let inactive_idx = if active_idx == 0 { 1 } else { 0 };
        (
            s.workspaces[inactive_idx].id.clone(),
            s.workspaces[active_idx].id.clone(),
        )
    };

    // Mark the active workspace as manually unread.
    {
        let mut s = state.borrow_mut();
        let ws = s.workspaces.iter_mut().find(|w| w.id == active_id).unwrap();
        ws.manual_unread = true;
    }
    sync_workspace_unread(&state, &active_id);

    // Closing an inactive workspace must NOT clear manual_unread on the active workspace.
    close_workspace_by_id_internal(&state, &inactive_id, false, None);
    pump_for(std::time::Duration::from_millis(100));
    {
        let s = state.borrow();
        let ws = s.workspaces.iter().find(|w| w.id == active_id).unwrap();
        assert!(
            ws.manual_unread,
            "closing an inactive workspace must preserve active manual_unread"
        );
        assert!(
            ws.notify_dot.get_visible(),
            "closing an inactive workspace must preserve the active unread indicator"
        );
    }

    let window = state.borrow().window.clone();
    window.close();
    pump_until(wait, || !window.is_visible());
}

#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn localized_split_and_remove_keep_sibling_gl_areas_realized() {
    let temp = tempfile::tempdir().unwrap();
    for key in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"] {
        let path = temp.path().join(key);
        std::fs::create_dir_all(&path).unwrap();
        std::env::set_var(key, path);
    }

    crate::prepare_ghostty_runtime();
    adw::init().unwrap();
    crate::terminal::init_ghostty();
    let app = adw::Application::builder()
        .application_id("dev.limux.LocalizedSplitSurgeryTest")
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    build_window(&app);
    let state = CONTROL_STATE.with(|slot| slot.borrow().as_ref().unwrap().clone());
    let wait = std::time::Duration::from_secs(5);
    state.borrow().window.set_default_size(1400, 900);

    // Build a 4-pane workspace: ((A / B) | (C / D)).
    let layout = LayoutNodeState::Split(SplitState {
        orientation: SplitOrientation::Horizontal,
        ratio: 0.5,
        start: Box::new(LayoutNodeState::Split(SplitState {
            orientation: SplitOrientation::Vertical,
            ratio: 0.5,
            start: Box::new(LayoutNodeState::Pane(PaneState::fallback(None))),
            end: Box::new(LayoutNodeState::Pane(PaneState::fallback(None))),
        })),
        end: Box::new(LayoutNodeState::Split(SplitState {
            orientation: SplitOrientation::Vertical,
            ratio: 0.5,
            start: Box::new(LayoutNodeState::Pane(PaneState::fallback(None))),
            end: Box::new(LayoutNodeState::Pane(PaneState::fallback(None))),
        })),
    });
    let (ws_id, _root) = show_focused_workspace(&state, layout);
    let container = {
        let s = state.borrow();
        s.workspaces
            .iter()
            .find(|w| w.id == ws_id)
            .unwrap()
            .split_container
            .clone()
    };
    let [pane_a, pane_b, pane_c, pane_d]: [gtk::Widget; 4] = container
        .panes()
        .try_into()
        .expect("four panes in 2x2 split");
    pump_until(wait, || {
        [&pane_a, &pane_b, &pane_c, &pane_d]
            .iter()
            .all(|pane| shown(pane) && find_gl_area(pane).is_some_and(|gl| gl.is_realized()))
    });
    pump_for(std::time::Duration::from_millis(150));

    let track_unrealize = |pane: &gtk::Widget| -> Rc<Cell<u32>> {
        let count = Rc::new(Cell::new(0u32));
        let count_cb = count.clone();
        let gl = find_gl_area(pane).expect("terminal GLArea");
        gl.connect_unrealize(move |_| {
            count_cb.set(count_cb.get() + 1);
        });
        count
    };
    let track_map = |pane: &gtk::Widget| -> Rc<Cell<u32>> {
        let count = Rc::new(Cell::new(0u32));
        let count_cb = count.clone();
        let gl = find_gl_area(pane).expect("terminal GLArea");
        gl.connect_map(move |_| {
            count_cb.set(count_cb.get() + 1);
        });
        count
    };
    let unrealize_a = track_unrealize(&pane_a);
    let map_a = track_map(&pane_a);
    let unrealize_b = track_unrealize(&pane_b);
    let unrealize_c = track_unrealize(&pane_c);
    let unrealize_d = track_unrealize(&pane_d);

    // 1. Splitting A horizontally only reparents A; B, C, and D stay realized.
    // A must map only once (on tick N+1 remount), never spuriously before detach.
    let pane_e = split_pane(
        &state,
        &ws_id,
        &pane_a,
        gtk::Orientation::Horizontal,
        split_options(),
    )
    .expect("localized split of A");
    pump_until(wait, || shown(&pane_e) && shown(&pane_a));
    pump_for(std::time::Duration::from_millis(120));
    assert_eq!(unrealize_a.get(), 1, "split target A reparents once");
    assert_eq!(map_a.get(), 1, "split target A must not map before detach");
    assert_eq!(unrealize_b.get(), 0, "sibling B must not unrealize");
    assert_eq!(unrealize_c.get(), 0, "outer sibling C must not unrealize");
    assert_eq!(unrealize_d.get(), 0, "outer sibling D must not unrealize");
    assert_focus_within("localized split focuses new pane E", &state, &pane_e);

    // 2. Closing B promotes nested Paned (A | E) into OuterPaned.start without
    // touching C or D, and preserves focus in E.
    let refs_b = weak_refs(&widget_tree(&pane_b));
    remove_pane(&state, &ws_id, &pane_b);
    drop(pane_b);
    assert_freed("localized removed pane B", &refs_b);
    pump_until(wait, || shown(&pane_a) && shown(&pane_e));
    assert_eq!(map_a.get(), 2, "promoted nested A maps once on remount");
    assert_eq!(unrealize_c.get(), 0, "closing B must not unrealize C");
    assert_eq!(unrealize_d.get(), 0, "closing B must not unrealize D");
    assert_focus_within("closing unfocused B keeps focus in E", &state, &pane_e);

    // 3. Focus unaffected pane D, then close E: single-leaf A is promoted into
    // OuterPaned.start while D keeps focus and C & D stay realized.
    assert!(pane::focus_active_tab_in_pane(&pane_d));
    pump_for(std::time::Duration::from_millis(80));
    assert_focus_within("focus moved to unaffected pane D", &state, &pane_d);
    let refs_e = weak_refs(&widget_tree(&pane_e));
    remove_pane(&state, &ws_id, &pane_e);
    drop(pane_e);
    assert_freed("localized removed pane E", &refs_e);
    pump_until(wait, || shown(&pane_a));
    assert_eq!(
        map_a.get(),
        3,
        "promoted single-leaf A maps once on remount"
    );
    assert_eq!(unrealize_c.get(), 0, "closing E must not unrealize C");
    assert_eq!(unrealize_d.get(), 0, "closing E must not unrealize D");
    assert_focus_within("closing unfocused E preserves focus in D", &state, &pane_d);

    // 4. Same-frame compound mutation (split A + close C in one tick) falls back
    // cleanly to a full rebuild without leaking C or stranding hidden children.
    let refs_c = weak_refs(&widget_tree(&pane_c));
    let pane_f = split_pane(
        &state,
        &ws_id,
        &pane_a,
        gtk::Orientation::Horizontal,
        split_options(),
    )
    .expect("split A before same-frame close");
    remove_pane(&state, &ws_id, &pane_c);
    drop(pane_c);
    assert_freed("same-frame closed pane C", &refs_c);
    pump_until(wait, || shown(&pane_a) && shown(&pane_f) && shown(&pane_d));
    assert_eq!(container.panes().len(), 3);
    assert!(container.panes().iter().all(shown));

    let window = state.borrow().window.clone();
    window.close();
    pump_until(wait, || !window.is_visible());
}

// Growing a pane into its neighbor's minimum stops there: the first resize
// back takes effect, and repeating the first one changes nothing.
#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn pane_resize_stops_at_the_neighbor_minimum() {
    let (state, _temp) = held_tab_test_window("dev.limux.PaneResizeLimitTest");
    let (ws_id, root) = show_focused_workspace(
        &state,
        LayoutNodeState::Split(SplitState {
            orientation: SplitOrientation::Horizontal,
            ratio: 0.5,
            start: Box::new(LayoutNodeState::Pane(PaneState::fallback(None))),
            end: Box::new(LayoutNodeState::Pane(PaneState::fallback(None))),
        }),
    );
    let first = find_leaf_pane(&root, gtk::Orientation::Horizontal, true);
    let container = {
        let s = state.borrow();
        let workspace = s.workspaces.iter().find(|w| w.id == ws_id).unwrap();
        workspace.split_container.clone()
    };
    let resize = |units: f64| {
        let outcome = container
            .resize_pane(&first, gtk::Orientation::Horizontal, units)
            .expect("the pane is in the workspace");
        pump_for(std::time::Duration::from_millis(200));
        assert_eq!(first.width(), outcome.width);
        outcome
    };

    let limit = resize(200.0);
    assert!(limit.changed);
    let again = resize(200.0);
    assert!(!again.changed, "already at the limit");
    assert_eq!(again.width, limit.width);
    // 10 px per unit, within a pixel of rounding.
    let back = resize(-5.0);
    assert!(back.changed);
    assert!(
        (limit.width - 50 - back.width).abs() <= 1,
        "shrinking by 50 px from {} gave {}",
        limit.width,
        back.width
    );

    let window = state.borrow().window.clone();
    window.close();
}

fn buttons_labelled(root: &gtk::Widget, label: &str) -> Vec<gtk::Button> {
    widget_tree(root)
        .into_iter()
        .filter_map(|widget| widget.downcast::<gtk::Button>().ok())
        .filter(|button| button.label().is_some_and(|text| text == label))
        .collect()
}

fn live_terminal(pane: &gtk::Widget, surface_id: &str) -> bool {
    pane::terminal_handle_for_surface(pane, Some(surface_id))
        .is_some_and(|(_, handle)| handle.health().realized && !handle.health().process_exited)
}

// A process that ends in a workspace nobody is looking at keeps the workspace,
// its group place and the terminal's output, marks it unread and offers a
// restart in place.
#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn unattended_exit_keeps_the_workspace_and_restarts_in_place() {
    let temp = tempfile::tempdir().unwrap();
    for key in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"] {
        let path = temp.path().join(key);
        std::fs::create_dir_all(&path).unwrap();
        std::env::set_var(key, path);
    }
    let ghostty_config = temp.path().join("XDG_CONFIG_HOME/ghostty");
    std::fs::create_dir_all(&ghostty_config).unwrap();
    std::fs::write(
        ghostty_config.join("config"),
        "command = /bin/sh\nshell-integration = none\n",
    )
    .unwrap();
    let socket = temp.path().join("limux.sock");
    std::env::set_var("LIMUX_SOCKET", &socket);
    std::env::set_var("LIMUX_SOCKET_PATH", &socket);

    crate::prepare_ghostty_runtime();
    adw::init().unwrap();
    crate::terminal::init_ghostty();
    let app = adw::Application::builder()
        .application_id("dev.limux.UnattendedExitTest")
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    build_window(&app);
    let state = CONTROL_STATE.with(|slot| slot.borrow().as_ref().unwrap().clone());
    let wait = std::time::Duration::from_secs(5);

    let (ws_id, root) =
        show_focused_workspace(&state, LayoutNodeState::Pane(PaneState::fallback(None)));
    let pane = first_leaf_pane(&root);
    let original = pane::active_tab_in_pane(&pane).unwrap();
    pane::add_terminal_tab_to_pane(&pane);
    assert!(pane::close_tab_in_pane(&pane, &original));
    pump_until(wait, || pane::tab_count_in_pane(&pane) == 1);
    let surface = pane::active_surface_summary(&pane).unwrap();
    pump_until(wait, || live_terminal(&pane, &surface.surface_id));
    let (_, handle) = pane::terminal_handle_for_surface(&pane, Some(&surface.surface_id)).unwrap();
    select_workspace_by_index(&state, 0);

    assert!(handle.send_text("exit 3"));
    assert!(handle.send_key("enter"));
    pump_until(wait, || handle.health().process_exited);
    pump_until(wait, || !buttons_labelled(&root, "Restart").is_empty());
    // The unread mark is applied from an idle callback.
    pump_for(std::time::Duration::from_millis(200));
    let (kept, message) = {
        let s = state.borrow();
        let workspace = s.workspaces.iter().find(|w| w.id == ws_id);
        (
            workspace.is_some(),
            workspace.map(|w| w.notify_label.label().to_string()),
        )
    };
    assert!(
        kept,
        "the only terminal exiting unseen must not close the workspace"
    );
    assert_eq!(message.as_deref(), Some("Process exited with code 3"));
    assert_eq!(pane::tab_count_in_pane(&pane), 1, "the exited tab is held");
    assert!(pane::workspace_has_unread_tabs(&ws_id));

    buttons_labelled(&root, "Restart")[0].emit_clicked();
    pump_until(wait, || live_terminal(&pane, &surface.surface_id));
    assert!(
        live_terminal(&pane, &surface.surface_id),
        "restart runs a new process in the same tab"
    );
    pump_until(wait, || buttons_labelled(&root, "Restart").is_empty());
    assert!(buttons_labelled(&root, "Restart").is_empty());

    // Close on the held banner closes the tab, and with it the workspace.
    let (_, handle) = pane::terminal_handle_for_surface(&pane, Some(&surface.surface_id)).unwrap();
    assert!(handle.send_text("exit 4"));
    assert!(handle.send_key("enter"));
    pump_until(wait, || !buttons_labelled(&root, "Close").is_empty());
    buttons_labelled(&root, "Close")[0].emit_clicked();
    pump_until(wait, || {
        !state.borrow().workspaces.iter().any(|w| w.id == ws_id)
    });
    assert!(!state.borrow().workspaces.iter().any(|w| w.id == ws_id));

    let window = state.borrow().window.clone();
    window.close();
}

// A terminal that exits in a background tab is held; opening that tab later
// puts the keyboard on Restart, not on the dead terminal.
#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn opening_a_held_background_tab_focuses_restart() {
    let (state, _temp) = held_tab_test_window("dev.limux.HeldBackgroundTabTest");
    let wait = std::time::Duration::from_secs(5);

    let (_ws_id, root) =
        show_focused_workspace(&state, LayoutNodeState::Pane(PaneState::fallback(None)));
    let pane = first_leaf_pane(&root);
    let background = pane::active_surface_summary(&pane).unwrap();
    pump_until(wait, || live_terminal(&pane, &background.surface_id));
    let (_, handle) =
        pane::terminal_handle_for_surface(&pane, Some(&background.surface_id)).unwrap();
    pane::add_terminal_tab_to_pane(&pane);
    let front = pane::active_surface_summary(&pane).unwrap();
    assert_ne!(front.tab_id, background.tab_id);
    pump_for(std::time::Duration::from_millis(300));

    assert!(handle.send_text("exit 5"));
    assert!(handle.send_key("enter"));
    pump_until(wait, || handle.health().process_exited);
    pump_until(wait, || !buttons_labelled(&root, "Restart").is_empty());
    pump_for(std::time::Duration::from_millis(200));
    assert_eq!(
        pane::active_tab_in_pane(&pane).as_deref(),
        Some(front.tab_id.as_str()),
        "the exit happened in a background tab"
    );

    assert!(pane::activate_tab_in_pane(&pane, &background.tab_id));
    let restart = buttons_labelled(&root, "Restart")[0].clone();
    pump_until(wait, || restart.has_focus());
    assert!(
        restart.has_focus(),
        "opening a held tab focuses its Restart button"
    );

    let window = state.borrow().window.clone();
    window.close();
}

// Choosing Keybinds in a pane that already has the tab shows that tab.
#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn opening_keybinds_again_shows_the_open_tab() {
    let (state, _temp) = held_tab_test_window("dev.limux.KeybindsAgainTest");
    let (_ws_id, root) =
        show_focused_workspace(&state, LayoutNodeState::Pane(PaneState::fallback(None)));
    let pane = first_leaf_pane(&root);
    let terminal = pane::active_tab_in_pane(&pane).unwrap();
    open_keybind_editor_tab(&state, &pane);
    let keybinds = pane::active_tab_in_pane(&pane).unwrap();
    assert_ne!(keybinds, terminal);
    assert!(pane::activate_tab_in_pane(&pane, &terminal));

    open_keybind_editor_tab(&state, &pane);
    assert_eq!(
        pane::tab_count_in_pane(&pane),
        2,
        "one Keybinds tab per pane"
    );
    assert_eq!(
        pane::active_tab_in_pane(&pane).as_deref(),
        Some(keybinds.as_str())
    );

    let window = state.borrow().window.clone();
    window.close();
}

fn held_tab_test_window(app_id: &str) -> (State, tempfile::TempDir) {
    let temp = tempfile::tempdir().unwrap();
    for key in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"] {
        let path = temp.path().join(key);
        std::fs::create_dir_all(&path).unwrap();
        std::env::set_var(key, path);
    }
    let ghostty_config = temp.path().join("XDG_CONFIG_HOME/ghostty");
    std::fs::create_dir_all(&ghostty_config).unwrap();
    std::fs::write(
        ghostty_config.join("config"),
        "command = /bin/sh\nshell-integration = none\n",
    )
    .unwrap();
    let socket = temp.path().join("limux.sock");
    std::env::set_var("LIMUX_SOCKET", &socket);
    std::env::set_var("LIMUX_SOCKET_PATH", &socket);

    crate::prepare_ghostty_runtime();
    adw::init().unwrap();
    crate::terminal::init_ghostty();
    let app = adw::Application::builder().application_id(app_id).build();
    app.register(None::<&gio::Cancellable>).unwrap();
    build_window(&app);
    let state = CONTROL_STATE.with(|slot| slot.borrow().as_ref().unwrap().clone());
    (state, temp)
}

// Returning to a workspace lands in the pane last used there, not its first
// pane, and the recent-workspace action toggles between the last two.
#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn returning_to_a_workspace_focuses_its_last_pane() {
    let temp = tempfile::tempdir().unwrap();
    for key in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"] {
        let path = temp.path().join(key);
        std::fs::create_dir_all(&path).unwrap();
        std::env::set_var(key, path);
    }
    let ghostty_config = temp.path().join("XDG_CONFIG_HOME/ghostty");
    std::fs::create_dir_all(&ghostty_config).unwrap();
    std::fs::write(
        ghostty_config.join("config"),
        "command = /bin/sh\nshell-integration = none\n",
    )
    .unwrap();
    let socket = temp.path().join("limux.sock");
    std::env::set_var("LIMUX_SOCKET", &socket);
    std::env::set_var("LIMUX_SOCKET_PATH", &socket);

    crate::prepare_ghostty_runtime();
    adw::init().unwrap();
    crate::terminal::init_ghostty();
    let app = adw::Application::builder()
        .application_id("dev.limux.RecentWorkspaceTest")
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    build_window(&app);
    let state = CONTROL_STATE.with(|slot| slot.borrow().as_ref().unwrap().clone());
    let wait = std::time::Duration::from_secs(5);

    let (ws_id, root) =
        show_focused_workspace(&state, LayoutNodeState::Pane(PaneState::fallback(None)));
    let first = first_leaf_pane(&root);
    let second = split_pane(
        &state,
        &ws_id,
        &first,
        gtk::Orientation::Horizontal,
        split_options(),
    )
    .expect("split");
    pump_until(wait, || shown(&second));
    assert!(pane::focus_active_tab_in_pane(&second));
    assert_focus_within("the second pane", &state, &second);
    let index = state
        .borrow()
        .workspaces
        .iter()
        .position(|w| w.id == ws_id)
        .unwrap();

    activate_workspace_shortcut(&state, 0);
    pump_for(std::time::Duration::from_millis(200));
    assert_eq!(state.borrow().active_idx, 0);
    assert_eq!(switch_to_recent_workspace(&state), Some(index));
    assert_focus_within("the last used pane after returning", &state, &second);
    assert_eq!(switch_to_recent_workspace(&state), Some(0));
    // The shortcut goes through the same action.
    assert!(dispatch_shortcut_command(
        &state,
        ShortcutCommand::RecentWorkspace
    ));
    assert_eq!(state.borrow().active_idx, index);
    assert_focus_within("the last used pane after toggling", &state, &second);

    let window = state.borrow().window.clone();
    window.close();
}

// The next-unread action reaches a tab in a hidden workspace and focuses it.
#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn next_unread_terminal_reaches_a_hidden_tab() {
    let temp = tempfile::tempdir().unwrap();
    for key in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"] {
        let path = temp.path().join(key);
        std::fs::create_dir_all(&path).unwrap();
        std::env::set_var(key, path);
    }
    let ghostty_config = temp.path().join("XDG_CONFIG_HOME/ghostty");
    std::fs::create_dir_all(&ghostty_config).unwrap();
    std::fs::write(
        ghostty_config.join("config"),
        "command = /bin/sh\nshell-integration = none\n",
    )
    .unwrap();
    let socket = temp.path().join("limux.sock");
    std::env::set_var("LIMUX_SOCKET", &socket);
    std::env::set_var("LIMUX_SOCKET_PATH", &socket);

    crate::prepare_ghostty_runtime();
    adw::init().unwrap();
    crate::terminal::init_ghostty();
    let app = adw::Application::builder()
        .application_id("dev.limux.NextUnreadTest")
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    build_window(&app);
    let state = CONTROL_STATE.with(|slot| slot.borrow().as_ref().unwrap().clone());
    let wait = std::time::Duration::from_secs(5);

    let (ws_id, root) =
        show_focused_workspace(&state, LayoutNodeState::Pane(PaneState::fallback(None)));
    let pane = first_leaf_pane(&root);
    pane::add_terminal_tab_to_pane(&pane);
    let background = pane::active_surface_summary(&pane).unwrap();
    pane::add_terminal_tab_to_pane(&pane);
    activate_workspace_shortcut(&state, 0);
    pump_for(std::time::Duration::from_millis(200));
    assert_eq!(
        pane::mark_tab_unread_in_workspace(&ws_id, background.pane_id, &background.tab_id),
        Some(true)
    );
    sync_workspace_unread(&state, &ws_id);

    assert!(dispatch_shortcut_command(
        &state,
        ShortcutCommand::NextUnreadTerminal
    ));
    pump_until(wait, || {
        pane::active_tab_in_pane(&pane).as_deref() == Some(background.tab_id.as_str())
            && focus_within(&state, &pane)
    });
    assert_eq!(
        pane::active_tab_in_pane(&pane).as_deref(),
        Some(background.tab_id.as_str())
    );
    assert_focus_within("the unread tab", &state, &pane);
    assert!(
        !pane::workspace_has_unread_tabs(&ws_id),
        "viewing it reads it"
    );

    let window = state.borrow().window.clone();
    window.close();
}

// The find entry has the focus while the bar is open, so the find shortcuts
// must work from there. However the bar closes, the keyboard is back at once.
#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn terminal_find_works_from_its_entry_and_gives_the_focus_back() {
    let (state, _temp) = held_tab_test_window("dev.limux.TerminalFindTest");
    let (_, root) =
        show_focused_workspace(&state, LayoutNodeState::Pane(PaneState::fallback(None)));
    let window = state.borrow().window.clone();
    let gl_area = find_gl_area(&root).expect("terminal");
    let search_bar = widget_tree(gl_area.parent().as_ref().unwrap())
        .into_iter()
        .find_map(|widget| widget.downcast::<gtk::SearchBar>().ok())
        .expect("find bar");
    let widgets = widget_tree(search_bar.upcast_ref());
    let entry = widgets
        .iter()
        .find_map(|widget| widget.downcast_ref::<gtk::SearchEntry>())
        .expect("find entry");
    let close_button = widgets
        .iter()
        .filter_map(|widget| widget.downcast_ref::<gtk::Button>())
        .find(|button| button.has_css_class("close"))
        .expect("find bar close button");
    let blocked = |command| {
        shortcut_blocked_by_editable(
            command,
            EditableCapturePolicy::BypassInEditable,
            focused_editable_capture_context(&state, window.upcast_ref()),
        )
    };

    type Close<'a> = &'a dyn Fn();
    let closings: [(&str, Close); 3] = [
        ("Escape", &|| entry.emit_by_name::<()>("stop-search", &[])),
        ("the close button", &|| close_button.emit_clicked()),
        ("the shortcut", &|| {
            assert!(dispatch_shortcut_command(
                &state,
                ShortcutCommand::SurfaceFindHide
            ));
        }),
    ];
    for (what, close) in closings {
        assert!(dispatch_shortcut_command(
            &state,
            ShortcutCommand::SurfaceFind
        ));
        assert_focus_within(what, &state, entry.upcast_ref());
        for command in [
            ShortcutCommand::SurfaceFindNext,
            ShortcutCommand::SurfaceFindPrevious,
            ShortcutCommand::SurfaceFindHide,
        ] {
            assert!(!blocked(command), "{command:?} in the find entry");
        }
        assert!(blocked(ShortcutCommand::TerminalCopy));

        close();
        assert!(!search_bar.is_search_mode(), "{what} closes the bar");
        assert!(gl_area.is_focus(), "{what} gives the focus back at once");
        assert!(!blocked(ShortcutCommand::TerminalCopy));
    }
    window.close();
}

// The next-unread action unzooms another pane to show the unread tab, and
// that reads it: the action must not keep landing on the same tab.
#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn next_unread_terminal_reads_a_tab_a_zoom_hid() {
    let (state, _temp) = held_tab_test_window("dev.limux.NextUnreadZoomTest");
    let wait = std::time::Duration::from_secs(5);

    let (ws_id, root) =
        show_focused_workspace(&state, LayoutNodeState::Pane(PaneState::fallback(None)));
    let pane = first_leaf_pane(&root);
    let unread = pane::active_surface_summary(&pane).unwrap();
    // Marked by hand: only going to the tab reads it, a passive focus does not.
    assert!(pane::mark_active_tab_manual_unread_in_root(&root));
    sync_workspace_unread(&state, &ws_id);

    let lead = split_before(&state, &ws_id, &pane);
    toggle_focused_pane_zoom(&state);
    pump_until(wait, || shown(&lead) && !shown(&pane));
    assert!(!shown(&pane), "the zoom hides the other pane");
    assert!(pane::workspace_has_unread_tabs(&ws_id));

    assert!(dispatch_shortcut_command(
        &state,
        ShortcutCommand::NextUnreadTerminal
    ));
    pump_until(wait, || {
        shown(&pane) && !pane::workspace_has_unread_tabs(&ws_id)
    });
    assert!(shown(&pane), "the unread tab's pane is shown again");
    assert_eq!(
        pane::active_tab_in_pane(&pane).as_deref(),
        Some(unread.tab_id.as_str())
    );
    assert!(
        !pane::workspace_has_unread_tabs(&ws_id),
        "being taken to the tab reads it"
    );

    let window = state.borrow().window.clone();
    window.close();
}

/// Two panes side by side, the second with two terminal tabs, and the focus in
/// the first.
fn show_focused_first_of_two_panes(state: &State) -> (gtk::Widget, gtk::Widget) {
    let (_ws_id, root) = show_focused_workspace(
        state,
        LayoutNodeState::Split(SplitState {
            orientation: SplitOrientation::Horizontal,
            ratio: 0.5,
            start: Box::new(LayoutNodeState::Pane(PaneState::fallback(None))),
            end: Box::new(LayoutNodeState::Pane(PaneState::fallback(None))),
        }),
    );
    let first = find_leaf_pane(&root, gtk::Orientation::Horizontal, true);
    let second = find_leaf_pane(&root, gtk::Orientation::Horizontal, false);
    pane::add_terminal_tab_to_pane(&second);
    pump_for(std::time::Duration::from_millis(300));
    assert!(pane::focus_active_tab_in_pane(&first));
    pump_for(std::time::Duration::from_millis(200));
    assert_focus_within("before", state, &first);
    (first, second)
}

// Enter commits a tab rename and gives the keyboard back to the terminal, not
// to the tab's close button. Focus that moves elsewhere commits it too.
#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn tab_rename_commit_returns_the_focus_to_its_terminal() {
    let (state, _temp) = held_tab_test_window("dev.limux.TabRenameFocusTest");
    let wait = std::time::Duration::from_secs(5);
    let (first, second) = show_focused_first_of_two_panes(&state);
    let start_rename = || {
        let tab = widget_tree(&first)
            .into_iter()
            .find(|w| w.has_css_class("limux-tab"))
            .expect("a tab button");
        focus_menu_item(&open_context_menu(&tab), "Rename").emit_clicked();
        pump_until(wait, || pane::find_tab_rename_entry(&first).is_some());
        let entry = pane::find_tab_rename_entry(&first).expect("rename entry");
        pump_for(std::time::Duration::from_millis(200));
        assert_focus_within("rename started", &state, entry.upcast_ref());
        entry
    };
    let tab_id = pane::active_tab_in_pane(&first).unwrap();
    let title = || pane::tab_title(&first, &tab_id).unwrap();

    let entry = start_rename();
    entry.set_text("entered");
    entry.emit_activate();
    pump_for(std::time::Duration::from_millis(300));
    let terminal = find_gl_area(&first).expect("terminal").upcast();
    assert_focus_within("rename committed", &state, &terminal);
    assert_eq!(title(), "entered");

    let entry = start_rename();
    entry.set_text("left");
    assert!(find_gl_area(&second).expect("terminal").grab_focus());
    pump_for(std::time::Duration::from_millis(300));
    assert!(pane::find_tab_rename_entry(&first).is_none());
    assert_eq!(title(), "left");
    assert_focus_within("rename left", &state, &second);

    let window = state.borrow().window.clone();
    window.close();
}

// A tab that closes in another pane, or anything that happens in a workspace
// that is not on screen, leaves the keyboard where the user is typing.
#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn only_the_pane_in_use_takes_the_keyboard() {
    let (state, _temp) = held_tab_test_window("dev.limux.KeyboardOwnerTest");
    let settle = || pump_for(std::time::Duration::from_millis(400));
    let (first, second) = show_focused_first_of_two_panes(&state);
    let close_active = |pane: &gtk::Widget| {
        let active = pane::active_tab_in_pane(pane).unwrap();
        assert!(pane::close_tab_in_pane(pane, &active));
        settle();
    };
    close_active(&second);
    assert_eq!(pane::tab_count_in_pane(&second), 1);
    assert_focus_within("tab closed in the other pane", &state, &first);

    pane::add_terminal_tab_to_pane(&first);
    settle();
    close_active(&first);
    assert_focus_within("tab closed in the focused pane", &state, &first);

    pane::add_terminal_tab_to_pane(&second);
    settle();
    let hidden_ws = {
        let s = state.borrow();
        s.active_workspace().unwrap().id.clone()
    };
    let (_ws_id, root) =
        show_focused_workspace(&state, LayoutNodeState::Pane(PaneState::fallback(None)));
    let shown = first_leaf_pane(&root);
    assert!(!second.is_mapped());

    close_active(&second);
    assert_focus_within("tab closed in a hidden workspace", &state, &root);
    pane::add_terminal_tab_to_pane(&second);
    settle();
    assert_focus_within("tab created in a hidden workspace", &state, &root);
    assert!(split_pane(
        &state,
        &hidden_ws,
        &second,
        gtk::Orientation::Vertical,
        split_options(),
    )
    .is_some());
    settle();
    assert_focus_within("split in a hidden workspace", &state, &root);

    // A tab dropped on the sidebar row of a hidden workspace.
    pane::add_terminal_tab_to_pane(&shown);
    settle();
    let moved = pane::active_surface_summary(&shown).unwrap();
    assert!(handle_tab_drop_to_workspace(
        &state,
        &hidden_ws,
        &moved.surface_id
    ));
    settle();
    assert_eq!(pane::tab_count_in_pane(&shown), 1);
    assert_focus_within("tab moved to a hidden workspace", &state, &root);

    let window = state.borrow().window.clone();
    window.close();
}

// Moving the focus between panes also reaches a pane that shows no terminal.
#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn directional_focus_reaches_a_pane_without_a_terminal() {
    let (state, _temp) = held_tab_test_window("dev.limux.DirectionalFocusTest");
    let (first, second) = show_focused_first_of_two_panes(&state);
    open_keybind_editor_tab(&state, &second);
    pump_for(std::time::Duration::from_millis(300));
    assert!(pane::focus_active_tab_in_pane(&first));
    pump_for(std::time::Duration::from_millis(200));

    focus_pane_in_direction(&state, Direction::Right);
    assert_focus_within("focus right", &state, &second);
    focus_pane_in_direction(&state, Direction::Left);
    assert_focus_within("focus left", &state, &first);

    let window = state.borrow().window.clone();
    window.close();
}
