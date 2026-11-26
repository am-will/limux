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

/// The params of the one request that `args` make from a terminal in `ws-caller`.
fn caller_request(home: &Path, args: &[&str]) -> Value {
    let socket = home.join("cli.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        let request: Value = serde_json::from_str(&line).unwrap();
        let reply = json!({"id": request["id"], "ok": true, "result": {"text": ""}});
        writeln!(stream, "{reply}").unwrap();
        request["params"].clone()
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
        let params = caller_request(home.path(), args);
        assert_eq!(params["workspace_id"], "ws-caller", "{args:?}");
    }
    for args in [
        &["clear-history", "--surface", "9:other"][..],
        &["break-pane", "--pane", "9"],
    ] {
        let params = caller_request(home.path(), args);
        assert!(params.get("workspace_id").is_none(), "{args:?}: {params}");
    }
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
