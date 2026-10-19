use super::*;

fn pump_until(mut done: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !done() {
        assert!(
            std::time::Instant::now() < deadline,
            "group restoration timed out"
        );
        while glib::MainContext::default().pending() {
            glib::MainContext::default().iteration(false);
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn header_names(widget: &gtk::Widget) -> Vec<String> {
    let mut names = Vec::new();
    if widget.has_css_class("limux-ws-group-name") {
        names.push(
            widget
                .downcast_ref::<gtk::Label>()
                .unwrap()
                .label()
                .to_string(),
        );
    }
    let mut child = widget.first_child();
    while let Some(widget) = child {
        names.extend(header_names(&widget));
        child = widget.next_sibling();
    }
    names
}

#[test]
#[ignore = "requires GTK and Ghostty resources; exercised by xvfb-smoke-test.sh"]
fn empty_saved_session_restores_group_headers_and_metadata() {
    let temp = tempfile::tempdir().unwrap();
    for key in [
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
    ] {
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
    let groups = vec![
        layout_state::WorkspaceGroupState {
            id: uuid::Uuid::new_v4().to_string(),
            name: "First group".to_string(),
            color: Some(WorkspaceColor::Blue),
            collapsed: true,
            position: 0,
        },
        layout_state::WorkspaceGroupState {
            id: uuid::Uuid::new_v4().to_string(),
            name: "Second group".to_string(),
            color: Some(WorkspaceColor::Teal),
            collapsed: false,
            position: 1,
        },
    ];
    let saved = AppSessionState {
        workspaces: Vec::new(),
        groups: groups.clone(),
        ..AppSessionState::default()
    };
    layout_state::save_session_atomic_in(&layout_state::persistence_dir(), &saved).unwrap();

    crate::prepare_ghostty_runtime();
    adw::init().unwrap();
    crate::terminal::init_ghostty();
    let app = adw::Application::builder()
        .application_id("dev.limux.EmptyGroupsTest")
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    build_window(&app);
    let state = CONTROL_STATE.with(|slot| slot.borrow().as_ref().unwrap().clone());
    let window = state.borrow().window.clone();
    pump_until(|| matches!(state.borrow().session_store, Ok(Some(_))) && window.is_mapped());

    assert_eq!(state.borrow().workspaces.len(), 1, "fallback workspace");
    assert_eq!(snapshot_session_state(&state).groups, groups);
    assert_eq!(state.borrow().group_headers.len(), 2);
    assert_eq!(
        header_names(state.borrow().sidebar_list.upcast_ref()),
        ["First group", "Second group"]
    );
    window.close();
    pump_until(|| !window.is_visible());
}
