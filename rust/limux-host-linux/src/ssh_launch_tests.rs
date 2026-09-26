use super::*;
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
