use super::*;
use crate::layout_state::TabContentState;
use std::os::unix::fs::PermissionsExt;

#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn ssh_launch_is_explicit_and_not_persisted() {
    let temp = tempfile::tempdir().unwrap();
    for key in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"] {
        let path = temp.path().join(key);
        std::fs::create_dir_all(&path).unwrap();
        std::env::set_var(key, path);
    }
    let marker = temp.path().join("launch");
    std::env::set_var("LIMUX_SSH_TEST_MARKER", &marker);
    let ssh = temp.path().join("ssh");
    std::fs::write(&ssh, "#!/bin/sh\n[ -t 0 ] || exit 91\n[ \"$TERM\" = xterm-256color ] || exit 92\nprintf '%s\\n' \"$@\" >> \"$LIMUX_SSH_TEST_MARKER\"\nexec /bin/sh\n").unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::env::set_var(
        "PATH",
        format!(
            "{}:{}",
            temp.path().display(),
            std::env::var("PATH").unwrap()
        ),
    );

    crate::prepare_ghostty_runtime();
    adw::init().unwrap();
    crate::terminal::init_ghostty();
    let app = adw::Application::builder()
        .application_id("dev.limux.SshTest")
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    build_window(&app);
    let state = CONTROL_STATE.with(|slot| slot.borrow().as_ref().unwrap().clone());
    assert!(
        !marker.exists(),
        "ordinary workspace creation must not run SSH"
    );
    connect_ssh_target(
        &state,
        crate::ssh_hosts::SshTarget::parse("alice@example", "2222").unwrap(),
        None,
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !marker.exists() && std::time::Instant::now() < deadline {
        while glib::MainContext::default().pending() {
            glib::MainContext::default().iteration(false);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let expected = "-t\n-p\n2222\n--\nalice@example\n";
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), expected);
    let snapshot = snapshot_session_state(&state);
    let workspace = snapshot.workspaces.last().unwrap();
    assert_eq!(workspace.name, "SSH: alice@example");
    assert!(workspace.autostart_command.is_none());
    assert!(!serde_json::to_string(&workspace.layout)
        .unwrap()
        .contains("alice@example"));

    let root = state.borrow().active_workspace().unwrap().root.clone();
    let pane = find_leaf_pane(&root, gtk::Orientation::Horizontal, true);
    pane::add_terminal_tab_to_pane(&pane);
    let mut restored = workspace.clone();
    restored.id = None;
    add_workspace_from_state(&state, &restored);
    // Pump the event loop long enough for new surfaces to spawn their children.
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    while std::time::Instant::now() < deadline {
        while glib::MainContext::default().pending() {
            glib::MainContext::default().iteration(false);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap(),
        expected,
        "new tabs and restored workspaces must not reconnect"
    );
    let window = state.borrow().window.clone();
    window.close();
}

fn pump_until(message: &str, condition: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while !condition() && std::time::Instant::now() < deadline {
        pump_for(std::time::Duration::from_millis(10));
    }
    assert!(condition(), "{message}");
}

fn pump_for(duration: std::time::Duration) {
    let deadline = std::time::Instant::now() + duration;
    while std::time::Instant::now() < deadline {
        for _ in 0..100 {
            if !glib::MainContext::default().pending() {
                break;
            }
            glib::MainContext::default().iteration(false);
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn persistent_ssh_restores_retries_moves_and_closes() {
    let temp = tempfile::tempdir().unwrap();
    for key in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"] {
        let path = temp.path().join(key);
        std::fs::create_dir_all(&path).unwrap();
        std::env::set_var(key, path);
    }
    std::env::set_var("LIMUX_SSH_FIXTURE", temp.path());
    let ssh = temp.path().join("ssh");
    std::fs::write(&ssh, r#"#!/usr/bin/python3
import json, os, pathlib, shlex, sys, time
assert os.isatty(0)
assert os.environ['TERM'] == 'xterm-256color'
args = sys.argv[1:]
assert args[:6] == ['-o', 'ServerAliveInterval=15', '-o', 'ServerAliveCountMax=3', '-o', 'ConnectTimeout=10']
assert args[6:11] == ['-t', '-p', '2222', '--', 'alice@example']
remote = shlex.split(args[-1])
assert remote[:5] == ['exec', 'tmux', 'new-session', '-A', '-s']
session = remote[-1]
root = pathlib.Path(os.environ['LIMUX_SSH_FIXTURE'])
log = root / session
with log.open('a') as out:
    out.write(json.dumps(args) + '\n')
    out.flush()
# The first two attempts fail, later attempts run an interactive shell.
attempt = len(log.read_text().splitlines())
if attempt <= 2:
    time.sleep(0.1)
    sys.exit(255)
(root / (session + '.ready')).touch()
os.execv('/bin/sh', ['/bin/sh'])
"#).unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::env::set_var(
        "PATH",
        format!(
            "{}:{}",
            temp.path().display(),
            std::env::var("PATH").unwrap()
        ),
    );
    crate::prepare_ghostty_runtime();
    adw::init().unwrap();
    crate::terminal::init_ghostty();
    let app = adw::Application::builder()
        .application_id("dev.limux.SshPersistenceTest")
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    build_window(&app);
    let state = CONTROL_STATE.with(|slot| slot.borrow().as_ref().unwrap().clone());
    connect_ssh_target(
        &state,
        crate::ssh_hosts::SshTarget::parse("alice@example", "2222").unwrap(),
        Some(true),
    );
    let (root, workspace_id) = {
        let state = state.borrow();
        let workspace = state.active_workspace().unwrap();
        (workspace.root.clone(), workspace.id.clone())
    };
    let source = find_leaf_pane(&root, gtk::Orientation::Horizontal, true);
    let saved = pane::snapshot_pane_state(&source).unwrap();
    let tab = saved.tabs[0].clone();
    let TabContentState::Terminal {
        ssh: Some(ref connection),
        ..
    } = tab.content
    else {
        panic!("missing saved SSH target")
    };
    let session = serde_json::to_value(connection).unwrap()["session"]
        .as_str()
        .unwrap()
        .to_string();
    let attempts = || {
        std::fs::read_to_string(temp.path().join(&session))
            .unwrap_or_default()
            .lines()
            .count()
    };
    pump_until("initial SSH process", || attempts() == 1);
    assert!(pane::rename_tab_in_pane(&source, &tab.id, "Remote work"));
    assert!(pane::set_tab_pinned_in_pane(&source, &tab.id, true));
    // New tabs inherit the target, but must not attach to the original session.
    pane::add_terminal_tab_to_pane(&source);
    let tabs = pane::snapshot_pane_state(&source).unwrap();
    assert_eq!(tabs.tabs.len(), 2);
    assert_ne!(
        serde_json::to_value(&tabs.tabs[0]).unwrap()["ssh"]["session"],
        serde_json::to_value(&tabs.tabs[1]).unwrap()["ssh"]["session"]
    );
    let background_active = tabs.active_tab_id.clone();
    pump_until("transport errors must retry with the same session", || {
        attempts() >= 3
    });
    assert_eq!(
        pane::active_tab_in_pane(&source),
        background_active,
        "background reconnect must not activate its tab"
    );
    assert_eq!(
        pane::tab_title(&source, &tab.id).as_deref(),
        Some("Remote work")
    );
    assert!(pane::snapshot_pane_state(&source).unwrap().tabs[0].pinned);
    // Split inheritance must also allocate a separate remote session.
    let target = split_pane(
        &state,
        &workspace_id,
        &source,
        gtk::Orientation::Horizontal,
        SplitPaneOptions {
            initial_state: None,
            skip_default_tab: false,
            inherit_active_directory: true,
            new_pane_first: false,
            persist: true,
            suppress_initial_autostart: false,
        },
    )
    .unwrap();
    let split = pane::snapshot_pane_state(&target).unwrap();
    assert_eq!(split.tabs.len(), 1);
    let split_json = serde_json::to_value(&split.tabs[0]).unwrap();
    assert_eq!(split_json["ssh"]["destination"], "alice@example");
    assert_ne!(split_json["ssh"]["session"], session);
    pump_for(std::time::Duration::from_millis(200));
    // Schedule a retry, then move the exited tab before the timer fires.
    let (_, handle) = pane::terminal_handle_for_surface(&source, Some(&tab.id)).unwrap();
    assert!(handle.send_text("exit 255\n"));
    pump_for(std::time::Duration::from_millis(400));
    assert!(pane::move_tab_to_pane(&source, &tab.id, &target));
    pump_until("retry must follow a moved tab", || attempts() >= 4);
    assert_eq!(
        pane::tab_title(&target, &tab.id).as_deref(),
        Some("Remote work")
    );
    let focus = target.root().and_then(|root| root.focus());
    assert!(
        focus.is_some_and(|focus| focus.is::<gtk::GLArea>() && focus.is_ancestor(&target)),
        "a focused reconnect must restore terminal keyboard focus"
    );
    // Persist via JSON, close the workspace, then restore the same session IDs.
    let snapshot = snapshot_session_state(&state);
    let workspace = snapshot
        .workspaces
        .iter()
        .find(|w| w.id.as_deref() == Some(&workspace_id))
        .unwrap();
    let mut restored: WorkspaceState =
        serde_json::from_str(&serde_json::to_string(workspace).unwrap()).unwrap();
    close_workspace_by_id(&state, &workspace_id);
    restored.id = None;
    add_workspace_from_state(&state, &restored);
    pump_until("restoring must reuse the saved remote session", || {
        attempts() >= 5
    });
    let root = state.borrow().active_workspace().unwrap().root.clone();
    let target = find_leaf_pane(&root, gtk::Orientation::Horizontal, false);
    let (_, handle) = pane::terminal_handle_for_surface(&target, Some(&tab.id)).unwrap();
    assert!(handle.send_text("exit 255\n"));
    pump_for(std::time::Duration::from_millis(300));
    assert!(pane::set_tab_pinned_in_pane(&target, &tab.id, false));
    assert!(pane::close_tab_in_pane(&target, &tab.id));
    let count = attempts();
    pump_for(std::time::Duration::from_secs(2));
    assert_eq!(
        attempts(),
        count,
        "closing during backoff must cancel the retry"
    );
    // Non-transport failures retain the target and do not retry.
    let error_tab = pane::active_tab_in_pane(&target).unwrap();
    let (_, handle) = pane::terminal_handle_for_surface(&target, Some(&error_tab)).unwrap();
    pump_until("remaining remote shell must be live", || {
        handle.health().realized && !handle.health().process_exited
    });
    assert!(handle.send_text("exit 127\n"));
    pump_until("remote command failure", || handle.health().process_exited);
    pump_for(std::time::Duration::from_secs(2));
    assert!(handle.health().process_exited);
    assert!(pane::snapshot_pane_state(&target)
        .unwrap()
        .tabs
        .iter()
        .any(|t| t.id == error_tab));

    // A clean remote exit closes its tab and never reconnects.
    let source = find_leaf_pane(&root, gtk::Orientation::Horizontal, true);
    let clean_tab = pane::active_tab_in_pane(&source).unwrap();
    let (_, handle) = pane::terminal_handle_for_surface(&source, Some(&clean_tab)).unwrap();
    pump_until("restored shell must be live", || {
        handle.health().realized && !handle.health().process_exited
    });
    assert!(handle.send_text("printf '\\033]7;file://remote/nonexistent-remote-directory\\007'\n"));
    pump_for(std::time::Duration::from_millis(100));
    assert_ne!(
        pane::tab_working_directory(&source, &clean_tab).as_deref(),
        Some("/nonexistent-remote-directory")
    );
    assert!(handle.send_text("exit 0\n"));
    pump_until("clean exit must remove the tab", || {
        pane::tab_count_in_pane(&source) == 0
    });

    // Persistence and automatic retry are independent choices.
    connect_ssh_target(
        &state,
        crate::ssh_hosts::SshTarget::parse("alice@example", "2222").unwrap(),
        Some(false),
    );
    let root = state.borrow().active_workspace().unwrap().root.clone();
    let disabled = find_leaf_pane(&root, gtk::Orientation::Horizontal, true);
    let (_, handle) = pane::terminal_handle_for_surface(&disabled, None).unwrap();
    pump_until("SSH with retries disabled exits", || {
        handle.health().realized && handle.health().process_exited
    });
    pump_for(std::time::Duration::from_secs(2));
    let disabled = pane::snapshot_pane_state(&disabled).unwrap();
    assert_eq!(disabled.tabs.len(), 1);
    let json = serde_json::to_value(&disabled.tabs[0]).unwrap();
    assert_eq!(json["ssh"]["auto_reconnect"], false);
    let session = json["ssh"]["session"].as_str().unwrap();
    assert_eq!(
        std::fs::read_to_string(temp.path().join(session))
            .unwrap()
            .lines()
            .count(),
        1
    );
    let window = state.borrow().window.clone();
    window.close();
}
