use super::*;

fn pump_until(timeout: std::time::Duration, mut done: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + timeout;
    while !done() && std::time::Instant::now() < deadline {
        while glib::MainContext::default().pending() {
            glib::MainContext::default().iteration(false);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn tab_ids(pane: &gtk::Widget) -> Vec<String> {
    pane::snapshot_pane_state(pane)
        .map(|state| state.tabs.into_iter().map(|tab| tab.id).collect())
        .unwrap_or_default()
}

// Issue #201: every new workspace's first tab used to be `terminal-0`, so dropping
// one workspace's first tab onto another left two tabs with the same id in one pane.
#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn moving_a_first_tab_to_another_workspace_keeps_tab_ids_unique() {
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
        .application_id("dev.limux.TabMoveTest")
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    build_window(&app);
    let state = CONTROL_STATE.with(|slot| slot.borrow().as_ref().unwrap().clone());
    add_workspace_from_state(
        &state,
        &WorkspaceState {
            id: None,
            name: "target".to_string(),
            favorite: false,
            cwd: None,
            folder_path: None,
            autostart_command: None,
            layout: LayoutNodeState::Pane(PaneState::fallback(None)),
        },
    );
    pump_until(std::time::Duration::from_millis(300), || false);

    let (source, target_workspace_id, target) = {
        let s = state.borrow();
        assert_eq!(
            s.workspaces.len(),
            2,
            "expected the initial and the added workspace"
        );
        let source = &s.workspaces[0];
        let target = &s.workspaces[1];
        (
            find_leaf_pane(&source.root, gtk::Orientation::Horizontal, true),
            target.id.clone(),
            find_leaf_pane(&target.root, gtk::Orientation::Horizontal, true),
        )
    };
    let source_state = pane::snapshot_pane_state(&source).unwrap();
    let moved_tab_id = source_state.tabs[0].id.clone();
    let target_tab_id = tab_ids(&target)[0].clone();

    let payload = format!("{}:{moved_tab_id}", source_state.pane_id.unwrap());
    assert!(handle_tab_drop_to_workspace(
        &state,
        &target_workspace_id,
        &payload
    ));
    pump_until(std::time::Duration::from_secs(2), || {
        tab_ids(&target).len() >= 2
    });

    let ids = tab_ids(&target);
    assert_eq!(
        ids.len(),
        2,
        "moved tab did not land in the target pane: {ids:?}"
    );
    assert!(
        ids.contains(&moved_tab_id) && ids.contains(&target_tab_id),
        "target pane lost a tab id: {ids:?}"
    );
    assert_ne!(ids[0], ids[1], "duplicate tab ids in one pane: {ids:?}");

    let window = state.borrow().window.clone();
    window.close();
}
