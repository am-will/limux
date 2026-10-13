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

    let mut refs = weak_refs(&widget_tree(&closed));
    refs.push(toast);
    refs.push(terminal_menu.upcast::<gtk::Widget>().downgrade());
    refs.push(tab_menu.upcast::<gtk::Widget>().downgrade());
    refs.push(pin_menu.upcast::<gtk::Widget>().downgrade());
    drop((terminal, tab));
    remove_pane(&state, &ws_id, &closed);
    drop(closed);
    assert_freed("closed pane", &refs);
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
