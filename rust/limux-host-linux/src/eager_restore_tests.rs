use super::*;

use crate::layout_state::{
    AgentLaunchCommandState, RestorableAgentKind, RestorableAgentState, TabContentState, TabState,
};
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

fn pump_for(duration: Duration) {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        for _ in 0..100 {
            if !glib::MainContext::default().pending() {
                break;
            }
            glib::MainContext::default().iteration(false);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn wait_for(message: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready() {
        assert!(Instant::now() < deadline, "{message}");
        pump_for(Duration::from_millis(10));
    }
}

fn workspace(name: &str, tabs: &[&str]) -> WorkspaceState {
    WorkspaceState {
        id: Some(uuid::Uuid::new_v4().to_string()),
        name: name.to_string(),
        favorite: false,
        cwd: None,
        folder_path: None,
        autostart_command: None,
        color: None,
        group_id: None,
        layout: LayoutNodeState::Pane(PaneState {
            pane_id: None,
            active_tab_id: Some(tabs[0].to_string()),
            tabs: tabs
                .iter()
                .map(|id| TabState::terminal(*id, None))
                .collect(),
        }),
    }
}

fn assign_agents(layout: &mut LayoutNodeState, executable: &Path, cwd: &Path) {
    match layout {
        LayoutNodeState::Pane(pane) => {
            for tab in &mut pane.tabs {
                if let TabContentState::Terminal { agent, .. } = &mut tab.content {
                    *agent = Some(RestorableAgentState {
                        kind: RestorableAgentKind::Claude,
                        session_id: tab.id.clone(),
                        cwd: Some(cwd.display().to_string()),
                        launch_command: Some(AgentLaunchCommandState {
                            executable: executable.display().to_string(),
                            arguments: vec!["claude".to_string()],
                            cwd: None,
                            environment: Default::default(),
                            captured_at: None,
                        }),
                        restore_on_startup: true,
                    });
                }
            }
        }
        LayoutNodeState::Split(split) => {
            assign_agents(&mut split.start, executable, cwd);
            assign_agents(&mut split.end, executable, cwd);
        }
    }
}

fn start(mut session: AppSessionState) -> (tempfile::TempDir, State) {
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
        "command = /bin/sh\nshell-integration = none\n",
    )
    .unwrap();
    std::fs::create_dir(temp.path().join("started")).unwrap();
    let executable = temp.path().join("agent-fixture");
    std::fs::write(
        &executable,
        format!(
            "#!/bin/sh\n\
             : > \"{dir}/started/$LIMUX_TAB_ID\"\n\
             if [ \"$LIMUX_TAB_ID\" = b1 ]; then\n\
               stty -echo -icanon min 1 time 0\n\
               printf '\\033[?1004h'\n\
               : > \"{dir}/focus-ready\"\n\
               dd bs=1 count=6 of=\"{dir}/focus-event\" 2>/dev/null\n\
               stty sane\n\
             fi\n\
             exec /bin/sh\n",
            dir = temp.path().display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    for workspace in &mut session.workspaces {
        if workspace.autostart_command.is_none() {
            assign_agents(&mut workspace.layout, &executable, temp.path());
        }
    }
    let socket = temp.path().join("limux.sock");
    std::env::set_var("LIMUX_SOCKET", &socket);
    std::env::set_var("LIMUX_SOCKET_PATH", &socket);
    std::env::set_var("LIMUX_AGENT_HOOK_STATE_DIR", temp.path().join("hooks"));
    // Each test starts the actual sweep explicitly, after installing observations.
    std::env::set_var("LIMUX_EAGER_RESTORE", "0");
    layout_state::save_session_atomic_in(&layout_state::persistence_dir(), &session).unwrap();
    crate::prepare_ghostty_runtime();
    adw::init().unwrap();
    crate::terminal::init_ghostty();
    let app = adw::Application::builder()
        .application_id("dev.limux.EagerRestoreTest")
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    build_window(&app);
    let state = CONTROL_STATE.with(|slot| slot.borrow().as_ref().unwrap().clone());
    wait_for("initial workspace must map", || {
        state.borrow().window.is_mapped()
    });
    pump_for(Duration::from_millis(100));
    (temp, state)
}

fn pane_named(state: &State, name: &str) -> gtk::Widget {
    state
        .borrow()
        .workspaces
        .iter()
        .find(|workspace| workspace.name == name)
        .unwrap()
        .split_container
        .panes()[0]
        .clone()
}

fn tab_stack(widget: &gtk::Widget, tab: &str) -> gtk::Stack {
    find_tab_stack(widget, tab).unwrap_or_else(|| panic!("missing stack for {tab}"))
}

fn find_tab_stack(widget: &gtk::Widget, tab: &str) -> Option<gtk::Stack> {
    if let Some(stack) = widget
        .downcast_ref::<gtk::Stack>()
        .filter(|stack| stack.child_by_name(tab).is_some())
    {
        return Some(stack.clone());
    }
    let mut child = widget.first_child();
    while let Some(widget) = child {
        if let Some(stack) = find_tab_stack(&widget, tab) {
            return Some(stack);
        }
        child = widget.next_sibling();
    }
    None
}

fn select(state: &State, name: &str, pane: &gtk::Widget, tab: &str) {
    let index = state
        .borrow()
        .workspaces
        .iter()
        .position(|workspace| workspace.name == name)
        .unwrap();
    select_workspace_by_index(state, index);
    assert!(pane::activate_tab_in_pane(pane, tab));
    pump_for(Duration::from_millis(40));
}

fn assert_selection(state: &State, name: &str, pane: &gtk::Widget, tab: &str) {
    let s = state.borrow();
    let workspace = s.active_workspace().unwrap();
    assert_eq!(workspace.name, name);
    assert_eq!(
        s.stack.visible_child().as_ref(),
        Some(&workspace.root),
        "warm-up changed the visible workspace"
    );
    assert_eq!(
        s.sidebar_list.selected_row().as_ref(),
        Some(&workspace.sidebar_row)
    );
    assert_eq!(pane::active_tab_in_pane(pane).as_deref(), Some(tab));
    let stack = tab_stack(pane, tab);
    assert_eq!(
        stack.visible_child_name().as_deref(),
        Some(tab),
        "warm-up changed the visible tab"
    );
    let content = stack.child_by_name(tab).unwrap();
    assert!(
        gtk::prelude::GtkWindowExt::focus(&s.window)
            .is_some_and(|focus| focus.is_ancestor(&content)),
        "warm-up stole keyboard focus"
    );
}

fn type_into_focused_terminal(state: &State, text: &str) {
    let focus = gtk::prelude::GtkWindowExt::focus(&state.borrow().window).unwrap();
    let controllers = focus.observe_controllers();
    let keys = (0..controllers.n_items())
        .filter_map(|index| {
            controllers
                .item(index)
                .and_downcast::<gtk::EventControllerKey>()
        })
        .next()
        .unwrap();
    for byte in text.bytes() {
        let key = if byte == b'\n' {
            0xff0d_u32
        } else {
            u32::from(byte)
        };
        let keyval = unsafe { glib::translate::from_glib::<u32, gtk::gdk::Key>(key) };
        let keycode = focus
            .display()
            .map_keyval(keyval)
            .and_then(|keys| keys.first().map(|key| key.keycode()))
            .unwrap();
        keys.emit_by_name::<bool>(
            "key-pressed",
            &[&key, &keycode, &gtk::gdk::ModifierType::empty()],
        );
        keys.emit_by_name::<()>(
            "key-released",
            &[&key, &keycode, &gtk::gdk::ModifierType::empty()],
        );
    }
}

fn close(state: &State) {
    let window = state.borrow().window.clone();
    window.close();
    wait_for("window must close", || !window.is_visible());
}

#[test]
#[ignore = "requires GTK and Ghostty resources; exercised by xvfb-smoke-test.sh"]
fn eager_restore_preserves_live_selection_focus_and_agent_bindings() {
    let (temp, state) = start(AppSessionState {
        workspaces: vec![
            workspace("A", &["a0", "a1"]),
            workspace("B", &["b0", "b1"]),
            workspace("C", &["c0", "c1"]),
        ],
        ..Default::default()
    });
    let a = pane_named(&state, "A");
    let b = pane_named(&state, "B");
    let c = pane_named(&state, "C");
    select(&state, "A", &a, "a0");
    let warmup_maps = Rc::new(Cell::new(0));
    for content in [tab_stack(&a, "a1").child_by_name("a1").unwrap(), b.clone()] {
        let count = warmup_maps.clone();
        content.connect_map(move |_| count.set(count.get() + 1));
    }
    run_startup_eager_restore(&state);
    pump_for(Duration::from_millis(120));
    assert_eq!(warmup_maps.get(), 0, "background warm-up mapped content");
    assert_selection(&state, "A", &a, "a0");
    select(&state, "C", &c, "c1");
    let typed = temp.path().join("typed-tab");
    type_into_focused_terminal(
        &state,
        &format!("printf %s \"$LIMUX_TAB_ID\" > {}\n", typed.display()),
    );
    let deadline = Instant::now() + Duration::from_millis(800);
    while Instant::now() < deadline {
        pump_for(Duration::from_millis(10));
        assert_selection(&state, "C", &c, "c1");
    }
    wait_for("input must reach the selected terminal", || typed.exists());
    assert_eq!(std::fs::read_to_string(typed).unwrap(), "c1");
    assert_eq!(warmup_maps.get(), 0, "background warm-up mapped content");
    for tab in ["a0", "a1", "b0", "b1", "c0", "c1"] {
        assert!(
            temp.path().join("started").join(tab).exists(),
            "{tab} was not resumed"
        );
    }
    let saved = snapshot_session_state(&state);
    assert_eq!(saved.workspaces[saved.active_workspace_index].name, "C");
    for workspace in &saved.workspaces {
        let LayoutNodeState::Pane(pane) = &workspace.layout else {
            panic!("expected pane")
        };
        for tab in &pane.tabs {
            let TabContentState::Terminal {
                agent: Some(agent), ..
            } = &tab.content
            else {
                panic!("lost agent binding")
            };
            assert_eq!(agent.session_id, tab.id);
        }
    }
    let (reply, result) = std::sync::mpsc::channel();
    handle_control_command(&state, ControlCommand::CurrentWorkspace { reply });
    assert_eq!(result.recv().unwrap().unwrap()["name"], "C");
    select(&state, "A", &a, "a1");
    let handle = pane::terminal_handle_for_surface(&a, Some("a1")).unwrap().1;
    wait_for(
        "a warmed terminal must render normally when selected",
        || {
            let health = handle.health();
            health.columns > 0 && health.rows > 0
        },
    );
    wait_for("background focus-reporting agent must start", || {
        temp.path().join("focus-ready").exists()
    });
    pump_for(Duration::from_millis(100));
    let focus_event = temp.path().join("focus-event");
    assert_eq!(
        std::fs::read(&focus_event).unwrap(),
        b"\x1b[O",
        "a hidden terminal received a native focus-in event"
    );
    select(&state, "B", &b, "b1");
    wait_for("selection must focus the resumed agent", || {
        std::fs::read(&focus_event).is_ok_and(|bytes| bytes == b"\x1b[O\x1b[I")
    });
    let workspace_id = state.borrow().active_workspace().unwrap().id.clone();
    let surface = pane::terminal_handle_for_surface(&b, Some("b1")).unwrap().0;
    let cli = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("limux-cli");
    let mut cleanup = std::process::Command::new(cli)
        .args(["--json", "hooks", "claude", "cleanup"])
        .env("LIMUX_WORKSPACE_ID", workspace_id)
        .env("LIMUX_SURFACE_ID", surface)
        .env("LIMUX_TAB_ID", "b1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("build limux-cli before running this GTK regression");
    std::io::Write::write_all(
        &mut cleanup.stdin.take().unwrap(),
        br#"{"session_id":"b1","hook_event_name":"Cleanup"}"#,
    )
    .unwrap();
    wait_for("explicit agent cleanup must finish", || {
        cleanup.try_wait().unwrap().is_some()
    });
    assert!(cleanup.wait().unwrap().success());
    save_session_now(&state);
    wait_for("agent cleanup must be persisted", || {
        matches!(state.borrow().session_store, Ok(Some(_)))
    });
    let saved = layout_state::load_session();
    for workspace in saved.state.workspaces {
        let LayoutNodeState::Pane(pane) = workspace.layout else {
            panic!("expected pane")
        };
        for tab in pane.tabs {
            let TabContentState::Terminal { agent, .. } = tab.content else {
                panic!("expected terminal")
            };
            assert_eq!(agent.is_some(), tab.id != "b1", "{}", tab.id);
        }
    }
    close(&state);
}

#[test]
#[ignore = "requires GTK and Ghostty resources; exercised by xvfb-smoke-test.sh"]
fn eager_restore_starts_surviving_tabs_after_an_earlier_workspace_closes() {
    let (temp, state) = start(AppSessionState {
        workspaces: vec![
            workspace("A", &["a0"]),
            workspace("B", &["b0"]),
            workspace("C", &["c0", "c1"]),
        ],
        ..Default::default()
    });
    let a = pane_named(&state, "A");
    let b = pane_named(&state, "B");
    let c = pane_named(&state, "C");
    select(&state, "A", &a, "a0");
    let closing_state = state.clone();
    let closing_id = state.borrow().workspaces[1].id.clone();
    find_gl_area(&b).unwrap().connect_realize(move |_| {
        let state = closing_state.clone();
        let id = closing_id.clone();
        glib::idle_add_local_full(glib::Priority::HIGH, move || {
            close_workspace_by_id(&state, &id);
            glib::ControlFlow::Break
        });
    });
    run_startup_eager_restore(&state);
    wait_for("B must close while startup work is queued", || {
        state
            .borrow()
            .workspaces
            .iter()
            .all(|workspace| workspace.name != "B")
    });
    wait_for(
        "every surviving C tab must start without being selected",
        || {
            ["c0", "c1"]
                .iter()
                .all(|tab| temp.path().join("started").join(tab).exists())
        },
    );
    assert!(!c.is_mapped(), "background workspace was exposed");
    assert_selection(&state, "A", &a, "a0");
    close(&state);
}

#[test]
#[ignore = "requires GTK and Ghostty resources; exercised by xvfb-smoke-test.sh"]
fn eager_restore_follows_moved_tabs_and_skips_closed_tabs_after_reordering() {
    let (temp, state) = start(AppSessionState {
        workspaces: vec![
            workspace("A", &["a0"]),
            workspace("B", &["b0", "b1"]),
            workspace("C", &["c0", "c1"]),
        ],
        ..Default::default()
    });
    let a = pane_named(&state, "A");
    let b = pane_named(&state, "B");
    let c = pane_named(&state, "C");
    select(&state, "A", &a, "a0");
    run_startup_eager_restore(&state);
    assert!(pane::move_tab_to_pane(&b, "b1", &c));
    assert!(pane::close_tab_in_pane(&c, "c1"));
    let (b_id, c_id) = {
        let s = state.borrow();
        (s.workspaces[1].id.clone(), s.workspaces[2].id.clone())
    };
    assert!(reorder_workspace_by_id(&state, &c_id, &b_id, false));
    select(&state, "A", &a, "a0");
    wait_for("moved and reordered terminal tabs must start", || {
        ["b0", "b1", "c0"]
            .iter()
            .all(|tab| temp.path().join("started").join(tab).exists())
    });
    assert!(
        !temp.path().join("started/c1").exists(),
        "closed tab was resumed"
    );
    let s = state.borrow();
    assert_eq!(s.active_workspace().unwrap().name, "A");
    assert_eq!(
        s.stack.visible_child().as_ref(),
        Some(&s.workspaces[0].root)
    );
    assert_eq!(pane::active_tab_in_pane(&a).as_deref(), Some("a0"));
    drop(s);
    assert!(!b.is_mapped() && !c.is_mapped());
    close(&state);
}
