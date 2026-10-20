//! Real PTY/CLI/Ghostty coverage; no SSH server or credentials required.
use super::*;
use std::os::unix::fs::PermissionsExt;

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
    window.present();
    pump_until(|| tty_file.exists() && handles.iter().all(|h| h.health().realized));
    let cli = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("limux-cli");
    let run_hook = |pane: &str, agent: &str| {
        use std::io::Write;
        let mut child = std::process::Command::new("setsid")
            .arg(&cli)
            .args(["--json", "hooks", agent, "stop"])
            .env("LIMUX_NOTIFY_OSC", "1")
            .env("TMUX", "fixture")
            .env("TMUX_PANE", pane)
            .env("LIMUX_SOCKET", dir.path().join("absent.sock"))
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    dir.path().display(),
                    std::env::var("PATH").unwrap()
                ),
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
            .write_all(br#"{"hook_event_name":"Stop","message":"remote completion"}"#)
            .unwrap();
        pump_until(|| child.try_wait().unwrap().is_some());
        child.wait_with_output().unwrap()
    };
    for agent in ["codex", "claude"] {
        let output = run_hook("%7", agent);
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
    let output = run_hook("%999", "codex");
    assert!(String::from_utf8_lossy(&output.stderr).contains("notification not delivered"));
    assert_eq!(
        events.borrow().len(),
        2,
        "invalid pane must not fall back to another client"
    );
    assert!(std::fs::read_to_string(calls)
        .unwrap()
        .contains("list-clients -t $42 -F #{client_tty}"));
    for handle in handles {
        handle.shutdown();
    }
    window.close();
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
