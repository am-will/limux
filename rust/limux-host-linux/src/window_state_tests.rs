use super::*;

fn pump_until(mut done: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !done() {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        while glib::MainContext::default().pending() {
            glib::MainContext::default().iteration(false);
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    true
}

fn start(app_id: &str) -> (tempfile::TempDir, State, adw::ApplicationWindow) {
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
    let app = adw::Application::builder().application_id(app_id).build();
    app.register(None::<&gio::Cancellable>).unwrap();
    build_window(&app);
    let state = CONTROL_STATE.with(|slot| slot.borrow().as_ref().unwrap().clone());
    let window = state.borrow().window.clone();
    let pane = first_leaf_pane(&state.borrow().workspaces[0].root);
    assert!(pump_until(|| pane.is_mapped() && pane.width() > 0));
    (temp, state, window)
}

// The session is restored before the window is presented. Reading the sidebar
// as hidden there made a first change that hides it look like no change.
#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn hiding_the_sidebar_is_saved_when_it_is_the_first_change() {
    let (_temp, state, window) = start("dev.limux.SidebarStateTest");
    let toggle = state.borrow().sidebar_toggle.clone().unwrap();
    assert!(toggle.is_active(), "the sidebar is shown at startup");

    toggle_sidebar(&state);
    let shell = state.borrow().sidebar_shell.clone();
    assert!(pump_until(|| !shell.get_visible()));
    window.close();
    assert!(pump_until(|| !window.is_visible()));
    let path = layout_state::canonical_session_path_in(&layout_state::persistence_dir());
    let saved: AppSessionState = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert!(!saved.sidebar.visible);
}

#[test]
#[ignore = "requires an active X11 window and Ghostty resources"]
fn focusing_a_split_pane_reads_its_tab() {
    let (_temp, state, window) = start("dev.limux.SplitUnreadTest");
    assert!(pump_until(|| window.is_active()));
    let ws_id = state.borrow().workspaces[0].id.clone();
    let first = first_leaf_pane(&state.borrow().workspaces[0].root);
    split_focused_pane(&state, gtk::Orientation::Horizontal);
    let second = state.borrow().workspaces[0].split_container.panes()[1].clone();
    assert!(pump_until(
        || second.width() > 0 && crate::terminal::focus_is_within(&second)
    ));

    let surface = pane::active_surface_summary(&first).unwrap();
    let bell = || {
        let target = DesktopNotificationTarget {
            workspace_id: ws_id.clone(),
            pane_id: Some(surface.pane_id),
            tab_id: Some(surface.tab_id.clone()),
        };
        mark_workspace_unread(&state, &ws_id, false, target);
    };
    bell();
    assert!(pane::workspace_has_unread_tabs(&ws_id));
    assert!(dispatch_shortcut_command(
        &state,
        ShortcutCommand::FocusLeft
    ));
    assert!(
        pump_until(|| !pane::workspace_has_unread_tabs(&ws_id)),
        "the focused tab is read"
    );

    // A tab marked by hand stays unread.
    find_gl_area(&second).unwrap().grab_focus();
    let root = state.borrow().workspaces[0].root.clone();
    assert!(pane::mark_active_tab_manual_unread_in_root(&root));
    find_gl_area(&first).unwrap().grab_focus();
    let settled = std::time::Instant::now() + std::time::Duration::from_millis(300);
    assert!(pump_until(|| std::time::Instant::now() >= settled));
    assert!(pane::workspace_has_unread_tabs(&ws_id));
    window.close();
}

#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn close_focused_pane_keeps_the_only_pane_of_a_workspace() {
    let (_temp, state, window) = start("dev.limux.ClosePaneTest");
    let container = state.borrow().workspaces[0].split_container.clone();
    split_focused_pane(&state, gtk::Orientation::Horizontal);
    assert_eq!(container.panes().len(), 2);
    for _ in 0..2 {
        assert!(dispatch_shortcut_command(
            &state,
            ShortcutCommand::CloseFocusedPane
        ));
        assert_eq!(container.panes().len(), 1);
        assert_eq!(state.borrow().workspaces.len(), 1);
    }
    window.close();
}

#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn changed_shortcuts_show_in_the_tooltips_of_open_panes() {
    fn has_tooltip(widget: &gtk::Widget, text: &str) -> bool {
        let mut child = widget.first_child();
        while let Some(current) = child {
            if current.tooltip_text().as_deref() == Some(text) || has_tooltip(&current, text) {
                return true;
            }
            child = current.next_sibling();
        }
        false
    }

    let (_temp, state, window) = start("dev.limux.TooltipTest");
    split_focused_pane(&state, gtk::Orientation::Horizontal);
    let panes = state.borrow().workspaces[0].split_container.panes();
    let unbound = state
        .borrow()
        .shortcuts
        .with_binding(ShortcutId::SplitDown, None)
        .unwrap();
    apply_shortcut_config(&state, unbound);
    for pane in &panes {
        assert!(has_tooltip(pane, "Split down"));
    }
    window.close();
}
