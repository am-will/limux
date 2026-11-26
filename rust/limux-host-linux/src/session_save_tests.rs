use super::*;

fn pump_until(mut done: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !done() {
        assert!(
            std::time::Instant::now() < deadline,
            "session save timed out"
        );
        while glib::MainContext::default().pending() {
            glib::MainContext::default().iteration(false);
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn start(app_id: &str) -> (tempfile::TempDir, State, adw::ApplicationWindow) {
    let temp = tempfile::tempdir().unwrap();
    for key in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"] {
        let path = temp.path().join(key);
        std::fs::create_dir_all(&path).unwrap();
        std::env::set_var(key, path);
    }
    crate::prepare_ghostty_runtime();
    adw::init().unwrap();
    crate::terminal::init_ghostty();
    let app = adw::Application::builder().application_id(app_id).build();
    app.register(None::<&gio::Cancellable>).unwrap();
    build_window(&app);
    let state = CONTROL_STATE.with(|slot| slot.borrow().as_ref().unwrap().clone());
    let window = state.borrow().window.clone();
    pump_until(|| matches!(state.borrow().session_store, Ok(Some(_))) && window.is_mapped());
    (temp, state, window)
}

fn saved_session() -> AppSessionState {
    let path = layout_state::canonical_session_path_in(&layout_state::persistence_dir());
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
#[ignore = "requires GTK; exercised by xvfb-smoke-test.sh"]
fn close_during_pending_save_persists_the_latest_snapshot() {
    let (_temp, state, window) = start("dev.limux.SessionSaveTest");

    let directory = layout_state::persistence_dir();
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(directory.join("session.lock"))
        .unwrap();
    lock.lock().unwrap();
    state.borrow_mut().workspaces[0].name = "first queued snapshot".into();
    request_session_save(&state);
    pump_until(|| matches!(state.borrow().session_store, Ok(None)));
    state.borrow_mut().workspaces[0].name = "edit while saving".into();
    window.close();
    assert!(state.borrow().session_close_pending.is_some());
    assert!(window.is_visible(), "close must wait for the final save");

    let dispatched = Rc::new(Cell::new(false));
    let tick = dispatched.clone();
    glib::timeout_add_local_once(std::time::Duration::from_millis(30), move || {
        tick.set(true);
    });
    pump_until(|| dispatched.get());
    assert!(window.is_visible());
    state.borrow_mut().workspaces[0].name = "latest edit while closing".into();
    drop(lock);
    pump_until(|| !window.is_visible());
    assert_eq!(
        saved_session().workspaces[0].name,
        "latest edit while closing"
    );
    assert!(state.borrow().persistence_suspended);
    assert!(state.borrow().session_save_timer.is_none());
}

// Issue #288: a pane kept open without tabs got a new tab id in every snapshot,
// so a close never found its snapshot saved and saved again forever.
#[test]
#[ignore = "requires GTK; exercised by xvfb-smoke-test.sh"]
fn close_completes_with_a_pane_kept_open_without_tabs() {
    let (_temp, state, window) = start("dev.limux.SessionCloseEmptyPaneTest");
    let root = state.borrow().workspaces[0].root.clone();
    state
        .borrow()
        .config
        .borrow_mut()
        .workspace
        .keep_open_after_last_terminal_closes = true;
    let pane = first_leaf_pane(&root);
    let tab_id = pane::active_tab_in_pane(&pane).unwrap();
    assert!(pane::close_tab_in_pane(&pane, &tab_id));
    assert_eq!(pane::tab_count_in_pane(&pane), 0);

    window.close();
    pump_until(|| !window.is_visible());
    let saved = saved_session();
    let LayoutNodeState::Pane(pane) = &saved.workspaces[0].layout else {
        panic!("the workspace keeps its only pane");
    };
    assert_eq!(pane.tabs.len(), 1, "the pane restores with a terminal");
    assert_ne!(pane.tabs[0].id, tab_id, "the closed tab does not come back");
}

#[test]
#[ignore = "requires GTK; exercised by xvfb-smoke-test.sh"]
fn close_completes_while_the_session_keeps_changing() {
    let (_temp, state, window) = start("dev.limux.SessionCloseChurnTest");
    // Runs in every main-loop iteration, so it edits the session during each save.
    let editing = state.clone();
    glib::idle_add_local_full(glib::Priority::DEFAULT, move || {
        let mut s = editing.borrow_mut();
        if s.persistence_suspended {
            return glib::ControlFlow::Break;
        }
        s.workspaces[0].name.push('x');
        glib::ControlFlow::Continue
    });
    window.close();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while window.is_visible() {
        assert!(
            std::time::Instant::now() < deadline,
            "the close never completed"
        );
        glib::MainContext::default().iteration(false);
    }
    assert_eq!(
        saved_session().workspaces[0].name,
        state.borrow().workspaces[0].name,
        "the close saved the last change"
    );
}
