//! Real PTY/CLI/Ghostty coverage; no SSH server or credentials required.
use super::*;
use std::os::unix::fs::PermissionsExt;

struct TerminalCleanup {
    handles: Vec<TerminalHandle>,
    window: gtk::Window,
}

impl Drop for TerminalCleanup {
    fn drop(&mut self) {
        for handle in &self.handles {
            handle.shutdown();
        }
        self.window.close();
    }
}

#[track_caller]
fn pump_until(mut condition: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !condition() {
        for _ in 0..100 {
            if !glib::MainContext::default().pending() {
                break;
            }
            glib::MainContext::default().iteration(false);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "notification fixture timed out"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[ignore = "requires graphical display, Ghostty resources and built limux-cli"]
fn detached_agent_hook_notifies_only_its_originating_terminal() {
    crate::prepare_ghostty_runtime();
    gtk::init().unwrap();
    init_ghostty();
    let dir = tempfile::tempdir().unwrap();
    let tty_file = dir.path().join("client-tty");
    let calls = dir.path().join("tmux-arguments");
    let tmux = dir.path().join("tmux");
    std::fs::write(
        &tmux,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> '{}'
case "$*" in
'display-message -p -t %7 #{{session_id}}') printf '$42\n';;
'list-panes -s -t $42 -F #{{pid}}:#{{pane_id}}') printf '1:%%7\n';;
'list-clients -t $42 -F #{{client_tty}}') cat '{}';;
*) exit 23;;
esac
"#,
            calls.display(),
            tty_file.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&tmux, std::fs::Permissions::from_mode(0o700)).unwrap();
    let events = Rc::new(RefCell::new(Vec::new()));
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let mut handles = Vec::new();
    for index in 0..2 {
        let captured = events.clone();
        let mut callbacks = TerminalCallbacks::disconnected();
        callbacks.on_desktop_notification = Box::new(move |title, body, _| {
            captured
                .borrow_mut()
                .push((index, title.to_string(), body.to_string()))
        });
        let command = if index == 0 {
            format!("/bin/sh -c 'tty > {}; exec sleep 60'", tty_file.display())
        } else {
            "sleep 60".into()
        };
        let terminal = create_terminal(
            None,
            TerminalOptions {
                startup_command: Some(command),
                ..Default::default()
            },
            callbacks,
        );
        row.append(&terminal.root);
        handles.push(terminal.handle);
    }
    let window = gtk::Window::builder()
        .default_width(1100)
        .default_height(600)
        .child(&row)
        .build();
    let _cleanup = TerminalCleanup {
        handles: handles.clone(),
        window: window.clone(),
    };
    window.present();
    pump_until(|| tty_file.exists() && handles.iter().all(|h| h.health().realized));
    let cli = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("limux-cli");
    let run_hook = |pane: &str, agent: &str, message: &str| {
        use std::io::Write;
        let mut child = std::process::Command::new("setsid")
            .arg(&cli)
            .args(["--json", "hooks", agent, "stop"])
            .env_clear()
            .env("HOME", dir.path())
            .env("XDG_STATE_HOME", dir.path().join("state"))
            .env("LIMUX_NOTIFY_OSC", "1")
            .env("TMUX", "/tmp/fixture,1,42")
            .env("TMUX_PANE", pane)
            .env("LIMUX_SOCKET", dir.path().join("absent.sock"))
            .env(
                "PATH",
                format!("{}:{}", dir.path().display(), "/usr/bin:/bin"),
            )
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(
                serde_json::json!({"hook_event_name": "Stop", "message": message})
                    .to_string()
                    .as_bytes(),
            )
            .unwrap();
        pump_until(|| child.try_wait().unwrap().is_some());
        child.wait_with_output().unwrap()
    };
    for agent in ["codex", "claude"] {
        let output = run_hook("%7", agent, "remote completion");
        assert!(output.status.success());
        assert!(
            output.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("hook stdout remains JSON");
        assert_eq!(json["continue"], true);
        // Ghostty limits desktop notifications to one per second globally.
        let until = std::time::Instant::now() + Duration::from_millis(1100);
        pump_until(|| std::time::Instant::now() >= until);
    }
    pump_until(|| events.borrow().len() == 2);
    assert_eq!(
        *events.borrow(),
        vec![
            (0, "Codex finished".into(), "remote completion".into()),
            (0, "Claude finished".into(), "remote completion".into())
        ]
    );
    for message in ["x".repeat(3000), "界".repeat(1000)] {
        let count = events.borrow().len();
        let output = run_hook("%7", "claude", &message);
        assert!(output.status.success());
        assert!(output.stderr.is_empty(), "{output:?}");
        pump_until(|| events.borrow().len() == count + 1);
        {
            let received = &events.borrow()[count];
            assert_eq!(received.0, 0);
            assert_eq!(received.1, "Claude finished");
            assert!(!received.2.is_empty());
            assert!(received.2.len() < message.len());
            assert!(message.starts_with(&received.2));
        }
        let until = std::time::Instant::now() + Duration::from_millis(1100);
        pump_until(|| std::time::Instant::now() >= until);
    }
    let output = run_hook("%999", "codex", "remote completion");
    assert!(String::from_utf8_lossy(&output.stderr).contains("notification not delivered"));
    assert_eq!(
        events.borrow().len(),
        4,
        "invalid pane must not fall back to another client"
    );
    assert!(std::fs::read_to_string(calls)
        .unwrap()
        .contains("list-clients -t $42 -F #{client_tty}"));
}

#[test]
#[ignore = "requires graphical display, tmux, Ghostty resources and built limux-cli"]
fn linked_tmux_sessions_keep_the_originating_notification_client() {
    crate::prepare_ghostty_runtime();
    gtk::init().unwrap();
    init_ghostty();
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("tmux.sock");
    struct Server(std::path::PathBuf);
    impl Drop for Server {
        fn drop(&mut self) {
            let _ = std::process::Command::new("tmux")
                .args(["-S"])
                .arg(&self.0)
                .arg("kill-server")
                .env_remove("TMUX")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
    let _server = Server(socket.clone());
    let tmux = |args: &[&str]| {
        let output = std::process::Command::new("tmux")
            .arg("-S")
            .arg(&socket)
            .args(["-f", "/dev/null"])
            .args(args)
            .env_clear()
            .env("HOME", dir.path())
            .env("PATH", "/usr/bin:/bin")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    };
    tmux(&["new-session", "-d", "-s", "origin", "sleep 60"]);
    let pane = tmux(&["display-message", "-p", "-t", "origin", "#{pane_id}"]);
    let origin_context = tmux(&[
        "display-message",
        "-p",
        "-t",
        "origin",
        "#{socket_path},#{pid},#{session_id}",
    ]);
    // TMUX stores the numeric session ID without the display-format '$' prefix.
    let origin_context = origin_context.replace(",$", ",");
    tmux(&["new-session", "-d", "-s", "linked", "-t", "origin"]);
    let linked_context = tmux(&[
        "display-message",
        "-p",
        "-t",
        "linked",
        "#{socket_path},#{pid},#{session_id}",
    ])
    .replace(",$", ",");
    let events = Rc::new(RefCell::new(Vec::new()));
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let mut handles = Vec::new();
    let quoted_socket = format!("'{}'", socket.to_str().unwrap().replace('\'', "'\\''"));
    for (index, session) in ["origin", "linked"].iter().enumerate() {
        let captured = events.clone();
        let mut callbacks = TerminalCallbacks::disconnected();
        callbacks.on_desktop_notification = Box::new(move |title, body, _| {
            captured
                .borrow_mut()
                .push((index, title.to_string(), body.to_string()));
        });
        let terminal = create_terminal(
            None,
            TerminalOptions {
                startup_command: Some(format!(
                    "env -u TMUX -u TMUX_PANE tmux -S {quoted_socket} attach-session -t {session}"
                )),
                ..Default::default()
            },
            callbacks,
        );
        row.append(&terminal.root);
        handles.push(terminal.handle);
    }
    let window = gtk::Window::builder()
        .default_width(1100)
        .default_height(600)
        .child(&row)
        .build();
    let _cleanup = TerminalCleanup {
        handles: handles.clone(),
        window: window.clone(),
    };
    window.present();
    pump_until(|| {
        tmux(&["list-clients", "-F", "#{session_name}"])
            .lines()
            .count()
            == 2
    });
    let cli = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("limux-cli");
    let run_hook = |context: &str, message: &str| {
        use std::io::Write;
        let mut child = std::process::Command::new("setsid")
            .arg(&cli)
            .args(["--json", "hooks", "codex", "stop"])
            .env_clear()
            .env("HOME", dir.path())
            .env("XDG_STATE_HOME", dir.path().join("state"))
            .env("PATH", "/usr/bin:/bin")
            .env("TMUX", context)
            .current_dir(dir.path())
            .env("TMUX_PANE", &pane)
            .env("LIMUX_NOTIFY_OSC", "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(
                serde_json::json!({"hook_event_name": "Stop", "message": message})
                    .to_string()
                    .as_bytes(),
            )
            .unwrap();
        pump_until(|| child.try_wait().unwrap().is_some());
        child.wait_with_output().unwrap()
    };
    for (index, context) in [&origin_context, &linked_context].iter().enumerate() {
        let message = format!("completion from {index}");
        let output = run_hook(context, &message);
        assert!(output.status.success(), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["continue"], true);
        pump_until(|| events.borrow().len() == index + 1);
        assert_eq!(
            events.borrow()[index],
            (index, "Codex finished".to_string(), message)
        );
        let until = std::time::Instant::now() + Duration::from_millis(1100);
        pump_until(|| std::time::Instant::now() >= until);
    }
    let relative_context = origin_context.replacen(socket.to_str().unwrap(), "tmux.sock", 1);
    let output = run_hook(&relative_context, "relative socket completion");
    assert!(output.stderr.is_empty(), "{output:?}");
    pump_until(|| events.borrow().len() == 3);
    assert_eq!(
        events.borrow()[2],
        (
            0,
            "Codex finished".to_string(),
            "relative socket completion".to_string()
        )
    );
    let stale = format!("{},999", origin_context.rsplit_once(',').unwrap().0);
    let output = run_hook(&stale, "stale session");
    assert!(String::from_utf8_lossy(&output.stderr).contains("notification not delivered"));
    assert_eq!(events.borrow().len(), 3);
    let (server, session) = origin_context.rsplit_once(',').unwrap();
    let (socket, pid) = server.rsplit_once(',').unwrap();
    let stale_server = format!("{socket},{},{session}", pid.parse::<u32>().unwrap() + 1);
    let output = run_hook(&stale_server, "stale server");
    assert!(String::from_utf8_lossy(&output.stderr).contains("notification not delivered"));
    assert_eq!(events.borrow().len(), 3);
    tmux(&["new-session", "-d", "-s", "other", "sleep 60"]);
    let other_context = tmux(&[
        "display-message",
        "-p",
        "-t",
        "other",
        "#{socket_path},#{pid},#{session_id}",
    ])
    .replace(",$", ",");
    let output = run_hook(&other_context, "wrong membership");
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("does not belong to its originating tmux session"));
    assert_eq!(events.borrow().len(), 3);
    tmux(&["detach-client", "-s", "origin"]);
    let output = run_hook(&origin_context, "detached client");
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("notification session has no attached tmux client"));
    assert_eq!(events.borrow().len(), 3);
}

#[test]
#[ignore = "opt-in authenticated localhost SSH/Codex test; submits one short model turn"]
fn live_codex_completion_reaches_local_terminal() {
    let command = std::env::var("LIMUX_NOTIFICATION_TEST_COMMAND")
        .expect("private SSH/tmux/Codex launch command");
    crate::prepare_ghostty_runtime();
    gtk::init().unwrap();
    init_ghostty();
    let events = Rc::new(RefCell::new(Vec::new()));
    let captured = events.clone();
    let mut callbacks = TerminalCallbacks::disconnected();
    callbacks.on_desktop_notification = Box::new(move |title, body, _| {
        captured
            .borrow_mut()
            .push((title.to_string(), body.to_string()))
    });
    let terminal = create_terminal(
        None,
        TerminalOptions {
            startup_command: Some(command),
            ..Default::default()
        },
        callbacks,
    );
    let window = gtk::Window::builder()
        .default_width(1200)
        .default_height(800)
        .child(&terminal.root)
        .build();
    window.present();
    let handle = terminal.handle;
    let wait = |condition: &dyn Fn() -> bool| {
        let deadline = std::time::Instant::now() + Duration::from_secs(90);
        while !condition() {
            for _ in 0..100 {
                if !glib::MainContext::default().pending() {
                    break;
                }
                glib::MainContext::default().iteration(false);
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Codex test timed out: {:?}",
                handle.read_viewport_text()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    wait(&|| {
        handle
            .read_viewport_text()
            .is_some_and(|s| s.contains("OpenAI Codex") && s.contains("Ask Codex"))
    });
    handle.send_text("Reply with exactly OK. Do not use tools.");
    assert!(handle.send_key("Enter"));
    wait(&|| {
        events
            .borrow()
            .iter()
            .any(|(title, _)| title == "Codex finished")
    });
    eprintln!("PASS: real Codex Stop hook → provisioned helper → remote tmux client PTY → SSH → local Ghostty notification callback");
    handle.shutdown();
    window.close();
}
