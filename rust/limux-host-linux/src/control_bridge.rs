//! Bridge between the limux control-plane Unix socket and the GTK UI.
//!
//! Starts a socket listener in a background thread and dispatches incoming
//! commands to the GTK main thread via glib idle callbacks.

use std::io::{self, BufRead, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::mpsc;

use serde_json::{json, Value};

/// A command received from the control socket, to be executed on the GTK thread.
pub enum ControlCommand {
    CreateWorkspace {
        name: Option<String>,
        cwd: Option<String>,
        command: Option<String>,
        reply: mpsc::Sender<Value>,
    },
    ListWorkspaces {
        reply: mpsc::Sender<Value>,
    },
    RenameWorkspace {
        index: usize,
        name: String,
        reply: mpsc::Sender<Value>,
    },
    ActivateWorkspace {
        index: usize,
        reply: mpsc::Sender<Value>,
    },
    CloseWorkspace {
        index: usize,
        reply: mpsc::Sender<Value>,
    },
    SendText {
        index: usize,
        text: String,
        reply: mpsc::Sender<Value>,
    },
}

fn socket_path() -> PathBuf {
    if let Some(path) = std::env::var_os("LIMUX_SOCKET") {
        return PathBuf::from(path);
    }
    if let Some(path) = std::env::var_os("LIMUX_SOCKET_PATH") {
        return PathBuf::from(path);
    }
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    runtime_dir.join("limux").join("limux.sock")
}

fn handle_client(stream: std::os::unix::net::UnixStream, cmd_tx: &mpsc::Sender<ControlCommand>) {
    let reader = io::BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;

    for line in reader.lines() {
        let Ok(line) = line else { break };
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let response = dispatch_request(&line, cmd_tx);
        let mut out = serde_json::to_string(&response).unwrap_or_default();
        out.push('\n');
        if writer.write_all(out.as_bytes()).is_err() {
            break;
        }
        if writer.flush().is_err() {
            break;
        }
    }
}

fn dispatch_request(input: &str, cmd_tx: &mpsc::Sender<ControlCommand>) -> Value {
    let Ok(req) = serde_json::from_str::<Value>(input) else {
        return json!({"ok": false, "error": {"message": "invalid JSON"}});
    };

    // V2: {"id":"…","method":"…","params":{}}
    // V1: {"command":"…","args":{}}
    let (id, method, params) = if let Some(m) = req.get("method").and_then(|v| v.as_str()) {
        (
            req.get("id").cloned().unwrap_or(Value::Null),
            m.to_string(),
            req.get("params").cloned().unwrap_or_else(|| json!({})),
        )
    } else if let Some(c) = req.get("command").and_then(|v| v.as_str()) {
        (
            req.get("id").cloned().unwrap_or(Value::Null),
            c.to_string(),
            req.get("args").cloned().unwrap_or_else(|| json!({})),
        )
    } else {
        return json!({"ok": false, "error": {"message": "missing method or command"}});
    };

    let (reply_tx, reply_rx) = mpsc::channel();

    let cmd = match method.as_str() {
        "workspace.create" | "new-workspace" => ControlCommand::CreateWorkspace {
            name: params
                .get("name")
                .and_then(|v| v.as_str())
                .map(String::from),
            cwd: params.get("cwd").and_then(|v| v.as_str()).map(String::from),
            command: params
                .get("command")
                .and_then(|v| v.as_str())
                .map(String::from),
            reply: reply_tx,
        },
        "workspace.list" | "list-workspaces" => ControlCommand::ListWorkspaces { reply: reply_tx },
        "workspace.rename" | "rename-workspace" => {
            let idx = params.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let name = params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            ControlCommand::RenameWorkspace {
                index: idx,
                name,
                reply: reply_tx,
            }
        }
        "workspace.activate" | "activate-workspace" => {
            let idx = params.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            ControlCommand::ActivateWorkspace {
                index: idx,
                reply: reply_tx,
            }
        }
        "workspace.close" | "close-workspace" => {
            let idx = params.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            ControlCommand::CloseWorkspace {
                index: idx,
                reply: reply_tx,
            }
        }
        "surface.send_text" | "send-text" | "send" => {
            let idx = params.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let text = params
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            ControlCommand::SendText {
                index: idx,
                text,
                reply: reply_tx,
            }
        }
        "system.ping" | "ping" => {
            return json!({"id": id, "ok": true, "result": {"pong": true}});
        }
        _ => {
            return json!({"id": id, "ok": false, "error": {"message": format!("unknown method: {method}")}});
        }
    };

    if cmd_tx.send(cmd).is_err() {
        return json!({"id": id, "ok": false, "error": {"message": "GUI channel closed"}});
    }

    match reply_rx.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(result) => json!({"id": id, "ok": true, "result": result}),
        Err(_) => json!({"id": id, "ok": false, "error": {"message": "timeout"}}),
    }
}

/// Start the control socket server in a background thread.
/// Returns an `mpsc::Receiver` that should be polled from the GTK main loop.
pub fn start() -> mpsc::Receiver<ControlCommand> {
    let (cmd_tx, cmd_rx) = mpsc::channel();

    std::thread::Builder::new()
        .name("limux-control".into())
        .spawn(move || {
            let path = socket_path();
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if path.exists() {
                let _ = std::fs::remove_file(&path);
            }

            let listener = match UnixListener::bind(&path) {
                Ok(l) => l,
                Err(err) => {
                    eprintln!(
                        "limux: control socket bind failed ({}): {err}",
                        path.display()
                    );
                    return;
                }
            };
            eprintln!("limux: control socket at {}", path.display());

            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        let cmd_tx = cmd_tx.clone();
                        std::thread::Builder::new()
                            .name("limux-ctrl-conn".into())
                            .spawn(move || handle_client(stream, &cmd_tx))
                            .ok();
                    }
                    Err(err) => {
                        eprintln!("limux: control accept error: {err}");
                    }
                }
            }
        })
        .expect("failed to spawn control server thread");

    cmd_rx
}
