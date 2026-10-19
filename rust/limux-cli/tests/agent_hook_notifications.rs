use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

fn private_command(program: &str, home: &Path, socket: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .env_clear()
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("LIMUX_SOCKET", socket)
        .env("PATH", "/usr/bin:/bin");
    command
}

fn notification_params(mut command: Command, socket: &Path, payload: Value) -> Value {
    let listener = UnixListener::bind(socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let output = child.wait_with_output().unwrap();
                    panic!("hook sent no notification: {output:?}");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("accept hook request: {error}"),
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut line = String::new();
    BufReader::new(stream.try_clone().unwrap())
        .read_line(&mut line)
        .unwrap();
    let request: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(request["method"], "notification.create");
    writeln!(
        stream,
        "{}",
        json!({"id": request["id"], "ok": true, "result": {}})
    )
    .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    std::fs::remove_file(socket).unwrap();
    request["params"].clone()
}

#[test]
fn installed_claude_hooks_distinguish_notifications_from_completion() {
    let home = tempfile::tempdir().unwrap();
    let socket = home.path().join("hook.sock");
    let cli = env!("CARGO_BIN_EXE_limux-cli");
    let output = private_command(cli, home.path(), &socket)
        .args(["hooks", "setup", "claude"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let settings: Value =
        serde_json::from_slice(&std::fs::read(home.path().join(".claude/settings.json")).unwrap())
            .unwrap();
    for (event, title, body) in [
        ("Notification", "Claude needs you", "waiting for input"),
        ("Stop", "Claude finished", "task complete"),
    ] {
        let hook = settings["hooks"][event][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        let mut command = private_command("/bin/sh", home.path(), &socket);
        command.args(["-c", hook]);
        let params = notification_params(command, &socket, json!({"hook_event_name": event}));
        assert_eq!(params["title"], title, "installed {event} hook");
        assert_eq!(params["body"], body, "installed {event} hook");
    }
}

#[test]
fn legacy_claude_stop_route_respects_notification_payload_and_explicit_event() {
    let home = tempfile::tempdir().unwrap();
    let socket = home.path().join("hook.sock");
    let cli = env!("CARGO_BIN_EXE_limux-cli");
    for (args, event, title) in [
        (vec!["stop"], "Notification", "Claude needs you"),
        (vec!["stop"], "Stop", "Claude finished"),
        (vec!["--event", "stop"], "Notification", "Claude finished"),
    ] {
        let mut command = private_command(cli, home.path(), &socket);
        command.args(["--json", "hooks", "claude"]).args(args);
        let params = notification_params(command, &socket, json!({"hook_event_name": event}));
        assert_eq!(params["title"], title);
    }
}
