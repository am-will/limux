use super::*;

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
        context.iteration(true);
    };

    let (reply, rx) = std::sync::mpsc::channel();
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
        context.iteration(true);
    };
    let pane_id: u32 = response["pane_id"].as_str().unwrap().parse().unwrap();
    assert!(
        pane::pane_widget_for_root(&root, pane_id).is_some(),
        "pane.create replied before pane {pane_id} could be targeted"
    );

    let window = state.borrow().window.clone();
    window.close();
}
