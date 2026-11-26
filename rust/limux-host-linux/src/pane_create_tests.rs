use super::*;

#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn terminal_commands_retry_until_ready_then_reply_without_a_timer() {
    use std::time::{Duration, Instant};

    let pump_for = |duration: Duration| {
        let context = glib::MainContext::default();
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            for _ in 0..100 {
                if !context.pending() {
                    break;
                }
                context.iteration(false);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    };

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
    let config = temp.path().join("XDG_CONFIG_HOME/ghostty");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("config"),
        "command = direct:/bin/sh\nshell-integration = none\n",
    )
    .unwrap();
    let socket = temp.path().join("limux.sock");
    std::env::set_var("LIMUX_SOCKET", &socket);
    std::env::set_var("LIMUX_SOCKET_PATH", &socket);
    std::env::set_var("LIMUX_AGENT_HOOK_STATE_DIR", temp.path().join("hooks"));
    std::env::set_var("LIMUX_EAGER_RESTORE", "0");

    crate::prepare_ghostty_runtime();
    adw::init().unwrap();
    crate::terminal::init_ghostty();
    let app = adw::Application::builder()
        .application_id("dev.limux.TerminalCommandReadinessTest")
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    build_window(&app);
    let state = CONTROL_STATE.with(|slot| slot.borrow().as_ref().unwrap().clone());
    let (workspace_id, shortcuts) = {
        let state = state.borrow();
        (
            state.active_workspace().unwrap().id.clone(),
            state.shortcuts.clone(),
        )
    };
    let pane = create_pane_for_workspace(
        &state,
        &shortcuts,
        &workspace_id,
        temp.path().to_str(),
        Rc::new(RefCell::new(None)),
        PaneCreationOptions {
            initial_state: None,
            skip_default_tab: false,
            suppress_initial_autostart: true,
            initial_command: None,
        },
    );
    let marker = temp.path().join("commands");
    let response = serde_json::json!({"accepted": true});
    let (reply, rx) = std::sync::mpsc::channel();
    send_response_after_terminal_command(
        "pane.create",
        pane.clone().upcast(),
        None,
        format!("printf 'retry\\n' >> '{}'", marker.display()),
        response.clone(),
        reply,
    );
    // An unrooted terminal cannot realize, so delivery must remain pending.
    assert!(matches!(
        rx.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));

    let pane_window = gtk::Window::builder().child(&pane).build();
    pane_window.present();
    let deadline = Instant::now() + Duration::from_secs(5);
    let retried = loop {
        if let Ok(result) = rx.try_recv() {
            break result.unwrap();
        }
        assert!(Instant::now() < deadline, "unrooted command never retried");
        pump_for(Duration::from_millis(10));
    };
    assert_eq!(retried, response);

    let (reply, rx) = std::sync::mpsc::channel();
    send_response_after_terminal_command(
        "pane.create",
        pane.clone().upcast(),
        None,
        format!("printf 'ready\\n' >> '{}'", marker.display()),
        response.clone(),
        reply,
    );
    assert_eq!(
        rx.try_recv()
            .expect("ready terminal waited for a timer")
            .unwrap(),
        response
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    while std::fs::read_to_string(&marker).unwrap_or_default() != "retry\nready\n" {
        assert!(
            Instant::now() < deadline,
            "commands did not execute in order"
        );
        pump_for(Duration::from_millis(10));
    }
    pump_for(Duration::from_millis(
        PANE_CREATE_COMMAND_READY_INTERVAL_MS * 3,
    ));
    assert_eq!(std::fs::read_to_string(marker).unwrap(), "retry\nready\n");

    pane_window.close();
    let window = state.borrow().window.clone();
    window.close();
}

// Review of #200: the split's teardown waits for a frame, and until the
// rebuild the new pane is in no workspace root, where pane lookups search. A
// caller targeting the pane from `pane.create`'s reply got "pane not found".
#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn pane_create_replies_once_the_new_pane_can_be_targeted() {
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
        .application_id("dev.limux.PaneCreateTest")
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    build_window(&app);
    let state = CONTROL_STATE.with(|slot| slot.borrow().as_ref().unwrap().clone());
    let root = state.borrow().active_workspace().unwrap().root.clone();

    let context = glib::MainContext::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let source = loop {
        let panes = pane::pane_summaries_for_root(&root);
        if let [only] = panes.as_slice() {
            let widget = pane::pane_widget_for_root(&root, only.pane_id).unwrap();
            if widget.is_mapped() && widget.width() > 0 {
                break only.pane_id;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "initial pane never showed"
        );
        if !context.iteration(false) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    };

    let (reply, rx) = std::sync::mpsc::channel();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    handle_control_command(
        &state,
        ControlCommand::CreatePane {
            request: crate::control_bridge::CreatePaneRequest {
                target: WorkspaceTarget::Active,
                source_pane_id: Some(source.to_string()),
                source_surface_id: None,
                direction: BridgePaneCreateDirection::Right,
                pane_type: PaneCreateType::Terminal,
                command: None,
                url: None,
            },
            reply,
        },
    );
    // The pane must be targetable from the moment the reply is sent.
    let response = loop {
        if let Ok(response) = rx.try_recv() {
            break response.expect("pane.create failed");
        }
        assert!(
            std::time::Instant::now() < deadline,
            "pane.create never replied"
        );
        if !context.iteration(false) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    };
    let pane_id: u32 = response["pane_id"].as_str().unwrap().parse().unwrap();
    assert!(
        pane::pane_widget_for_root(&root, pane_id).is_some(),
        "pane.create replied before pane {pane_id} could be targeted"
    );

    let window = state.borrow().window.clone();
    window.close();
}

#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn pane_swap_and_list_preserve_swapped_leaf_order() {
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
        .application_id("dev.limux.PaneSwapOrderTest")
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    build_window(&app);
    let state = CONTROL_STATE.with(|slot| slot.borrow().as_ref().unwrap().clone());
    let root = state.borrow().active_workspace().unwrap().root.clone();

    let context = glib::MainContext::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let first_id = loop {
        let panes = pane::pane_summaries_for_root(&root);
        if let [only] = panes.as_slice() {
            let widget = pane::pane_widget_for_root(&root, only.pane_id).unwrap();
            if widget.is_mapped() && widget.width() > 0 {
                break only.pane_id;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "initial pane never showed"
        );
        context.iteration(true);
    };

    let (reply, rx) = std::sync::mpsc::channel();
    handle_control_command(
        &state,
        ControlCommand::CreatePane {
            request: crate::control_bridge::CreatePaneRequest {
                target: WorkspaceTarget::Active,
                source_pane_id: Some(first_id.to_string()),
                source_surface_id: None,
                direction: BridgePaneCreateDirection::Right,
                pane_type: PaneCreateType::Terminal,
                command: None,
                url: None,
            },
            reply,
        },
    );
    let created = loop {
        if let Ok(response) = rx.try_recv() {
            break response.expect("pane.create failed");
        }
        assert!(
            std::time::Instant::now() < deadline,
            "pane.create never replied"
        );
        context.iteration(true);
    };
    let second_id: u32 = created["pane_id"].as_str().unwrap().parse().unwrap();

    let (swap_reply, swap_rx) = std::sync::mpsc::channel();
    handle_control_command(
        &state,
        ControlCommand::SwapPanes {
            target: WorkspaceTarget::Active,
            first_pane_id: first_id.to_string(),
            second_pane_id: second_id.to_string(),
            reply: swap_reply,
        },
    );
    let swapped = loop {
        if let Ok(response) = swap_rx.try_recv() {
            break response.expect("pane.swap failed");
        }
        assert!(
            std::time::Instant::now() < deadline,
            "pane.swap never replied"
        );
        context.iteration(true);
    };
    let swapped_panes = swapped["panes"].as_array().unwrap();
    assert_eq!(swapped_panes.len(), 2);
    assert_eq!(swapped_panes[0]["pane_id"], second_id.to_string());
    assert_eq!(swapped_panes[0]["index"], 0);
    assert_eq!(swapped_panes[1]["pane_id"], first_id.to_string());
    assert_eq!(swapped_panes[1]["index"], 1);

    let (list_reply, list_rx) = std::sync::mpsc::channel();
    handle_control_command(
        &state,
        ControlCommand::ListPanes {
            target: WorkspaceTarget::Active,
            reply: list_reply,
        },
    );
    let listed = list_rx.recv().unwrap().expect("pane.list failed");
    let listed_panes = listed["panes"].as_array().unwrap();
    assert_eq!(listed_panes[0]["pane_id"], second_id.to_string());
    assert_eq!(listed_panes[0]["index"], 0);
    assert_eq!(listed_panes[1]["pane_id"], first_id.to_string());
    assert_eq!(listed_panes[1]["index"], 1);

    let window = state.borrow().window.clone();
    window.close();
}

#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn input_starts_a_terminal_that_was_never_shown_or_is_rejected() {
    use std::time::{Duration, Instant};

    let temp = tempfile::tempdir().unwrap();
    for key in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"] {
        let path = temp.path().join(key);
        std::fs::create_dir_all(&path).unwrap();
        std::env::set_var(key, path);
    }
    let config = temp.path().join("XDG_CONFIG_HOME/ghostty");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("config"),
        "command = direct:/bin/sh\nshell-integration = none\n",
    )
    .unwrap();
    let socket = temp.path().join("limux.sock");
    std::env::set_var("LIMUX_SOCKET", &socket);
    std::env::set_var("LIMUX_SOCKET_PATH", &socket);

    crate::prepare_ghostty_runtime();
    adw::init().unwrap();
    crate::terminal::init_ghostty();
    let app = adw::Application::builder()
        .application_id("dev.limux.HiddenTerminalInputTest")
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    build_window(&app);
    let state = CONTROL_STATE.with(|slot| slot.borrow().as_ref().unwrap().clone());
    let hidden = state.borrow().active_workspace().unwrap().id.clone();
    add_workspace_from_state(&state, &workspace_state_with_folder("shown", "/"));

    let context = glib::MainContext::default();
    let wait_until = |what: &str, done: &mut dyn FnMut() -> bool| {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < deadline, "{what}");
            if !context.iteration(false) {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    };
    let request = |command: &dyn Fn(
        std::sync::mpsc::Sender<Result<serde_json::Value, BridgeError>>,
    ) -> ControlCommand| {
        let (reply, rx) = std::sync::mpsc::channel();
        handle_control_command(&state, command(reply));
        let mut result = None;
        wait_until("no reply", &mut || {
            result = rx.try_recv().ok();
            result.is_some()
        });
        result.unwrap()
    };
    let target = || WorkspaceTarget::Handle(hidden.clone());
    let send_text = |surface: &str, text: &str| {
        request(&|reply| ControlCommand::SendText {
            target: target(),
            surface_hint: Some(surface.to_string()),
            text: text.to_string(),
            reply,
        })
    };
    let send_key = |surface: &str, key: &str| {
        request(&|reply| ControlCommand::SendKey {
            target: target(),
            surface_hint: Some(surface.to_string()),
            key: key.to_string(),
            reply,
        })
    };

    let created = request(&|reply| ControlCommand::CreatePane {
        request: crate::control_bridge::CreatePaneRequest {
            target: target(),
            source_pane_id: None,
            source_surface_id: None,
            direction: BridgePaneCreateDirection::Right,
            pane_type: PaneCreateType::Terminal,
            command: None,
            url: None,
        },
        reply,
    })
    .unwrap();
    let surface = created["surface_id"].as_str().unwrap();
    let pane_id = created["pane_id"].as_str().unwrap().parse().unwrap();
    let pane = pane::pane_widget_for_workspace(&hidden, pane_id).unwrap();
    let (_, handle) = pane::terminal_handle_for_surface(&pane, Some(surface)).unwrap();
    assert!(!handle.health().realized, "the hidden pane already started");

    let marker = temp.path().join("marker");
    send_text(surface, &format!("printf ran > '{}'", marker.display())).unwrap();
    send_key(surface, "Enter").unwrap();
    wait_until("the hidden terminal never ran the command", &mut || {
        std::fs::read_to_string(&marker).is_ok_and(|text| text == "ran")
    });
    assert_eq!(
        send_key(surface, "not-a-key"),
        Err(BridgeError::invalid_params("unsupported key"))
    );

    // Outside the window it cannot start, as while a split is being rebuilt.
    let shortcuts = state.borrow().shortcuts.clone();
    let unrooted: gtk::Widget = create_pane_for_workspace(
        &state,
        &shortcuts,
        &hidden,
        temp.path().to_str(),
        Rc::new(RefCell::new(None)),
        PaneCreationOptions {
            initial_state: None,
            skip_default_tab: false,
            suppress_initial_autostart: true,
            initial_command: None,
        },
    )
    .upcast();
    let surface = pane::active_surface_summary(&unrooted).unwrap().surface_id;
    assert_eq!(send_text(&surface, "lost"), Err(terminal_not_started()));
    assert_eq!(send_key(&surface, "Enter"), Err(terminal_not_started()));

    let window = state.borrow().window.clone();
    window.close();
}
