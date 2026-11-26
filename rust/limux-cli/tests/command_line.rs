use std::io::{BufRead, BufReader, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{json, Value};

fn cli(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_limux-cli"));
    command
        .env_clear()
        .env("HOME", home)
        .env("XDG_RUNTIME_DIR", home)
        .env("LIMUX_SOCKET", home.join("cli.sock"))
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null());
    command
}

/// The requests that `args` make from a terminal in `ws-caller`. Replies name
/// the terminal `7:took-it`.
fn caller_requests(home: &Path, args: &[&str]) -> Vec<Value> {
    let socket = home.join("cli.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut requests = Vec::new();
        for line in BufReader::new(stream.try_clone().unwrap()).lines() {
            let request: Value = serde_json::from_str(&line.unwrap()).unwrap();
            let result = json!({"text": "", "surface_id": "7:took-it"});
            let reply = json!({"id": request["id"], "ok": true, "result": result});
            writeln!(stream, "{reply}").unwrap();
            requests.push(request);
        }
        requests
    });
    let output = cli(home)
        .env("LIMUX_WORKSPACE_ID", "ws-caller")
        .env("LIMUX_SURFACE_ID", "3:tab")
        .env("LIMUX_PANE_ID", "3")
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{args:?}: {output:?}");
    std::fs::remove_file(socket).unwrap();
    server.join().unwrap()
}

#[test]
fn untargeted_tmux_commands_act_on_the_callers_workspace() {
    let home = tempfile::tempdir().unwrap();
    for args in [
        &["read-screen"][..],
        &["clear-history"],
        &["break-pane"],
        &["paste-buffer", "--name", "b"],
        &["respawn-pane", "--command", "make"],
        &["pipe-pane", "--command", "cat"],
    ] {
        for request in caller_requests(home.path(), args) {
            assert_eq!(request["params"]["workspace_id"], "ws-caller", "{args:?}");
        }
    }
    for args in [
        &["clear-history", "--surface", "9:other"][..],
        &["break-pane", "--pane", "9"],
    ] {
        let params = &caller_requests(home.path(), args)[0]["params"];
        assert!(params.get("workspace_id").is_none(), "{args:?}: {params}");
    }
}

#[test]
fn respawn_pane_presses_enter_in_the_terminal_that_took_the_command() {
    let home = tempfile::tempdir().unwrap();
    let requests = caller_requests(home.path(), &["respawn-pane", "--command", "make"]);
    let sent: Vec<_> = requests
        .iter()
        .map(|request| (request["method"].as_str().unwrap(), &request["params"]))
        .collect();
    let text = json!({"workspace_id": "ws-caller", "text": "make"});
    let enter = json!({"workspace_id": "ws-caller", "surface_id": "7:took-it", "key": "Enter"});
    assert_eq!(
        sent,
        [("surface.send_text", &text), ("surface.send_key", &enter)]
    );

    let requests = caller_requests(home.path(), &["respawn-pane"]);
    assert_eq!(requests.len(), 1, "no command types nothing: {requests:?}");
    assert_eq!(requests[0]["params"]["key"], "Enter");
}

#[test]
fn hook_uninstall_ignores_no_argument() {
    let home = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| cli(home.path()).args(args).output().unwrap().status;
    let configs = || {
        [".claude/settings.json", ".codex/hooks.json"]
            .map(|path| std::fs::read_to_string(home.path().join(path)).unwrap())
    };
    for agent in ["claude", "codex"] {
        assert!(run(&["hooks", "setup", agent]).success());
    }
    let installed = configs();
    for (args, success) in [
        (&["hooks", "uninstall", "--help"][..], true),
        (&["hooks", "claude", "uninstall", "-h"], true),
        (&["hooks", "uninstall", "claude", "codex"], false),
        (&["hooks", "uninstall", "--agnt", "claude"], false),
        (&["hooks", "uninstall", "--agent"], false),
    ] {
        assert_eq!(run(args).success(), success, "{args:?}");
        assert_eq!(configs(), installed, "{args:?}");
    }
    assert!(run(&["hooks", "uninstall", "--agent=claude"]).success());
    let [claude, codex] = configs();
    assert!(!claude.contains("hooks claude"));
    assert_eq!(codex, installed[1]);
}

#[test]
fn closed_output_and_invalid_arguments_do_not_panic() {
    let home = tempfile::tempdir().unwrap();
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let output = cli(home.path()).arg("--help").stdout(writer).output();
    let output = output.unwrap();
    assert!(output.status.success(), "{output:?}");

    let output = cli(home.path())
        .arg("send")
        .arg(std::ffi::OsStr::from_bytes(b"caf\xe9"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("not valid UTF-8"));
}
