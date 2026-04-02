//! Bridge the limux control socket onto the GTK host state.

use std::io::{self, BufRead, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc;
use std::time::Duration;

use gtk::glib;
use gtk4 as gtk;
use limux_control::socket_path::{resolve_socket_path, SocketMode};
use limux_protocol::{parse_v1_command_envelope, V2Request, V2Response};
use serde_json::{json, Map, Value};

const METHODS: &[&str] = &[
    "system.ping",
    "system.identify",
    "system.capabilities",
    "workspace.current",
    "workspace.list",
    "workspace.create",
    "workspace.select",
    "workspace.rename",
    "workspace.close",
    "surface.send_text",
];

const PARSE_ERROR_CODE: i64 = -32700;
const INVALID_PARAMS_CODE: i64 = -32602;
const UNKNOWN_METHOD_CODE: i64 = -32601;
const INTERNAL_ERROR_CODE: i64 = -32603;
const NOT_FOUND_CODE: i64 = -32004;
const CONFLICT_CODE: i64 = -32009;

type BridgeResult = Result<Value, BridgeError>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkspaceTarget {
    Active,
    Handle(String),
    Name(String),
    Index(usize),
}

#[derive(Debug)]
pub enum ControlCommand {
    Identify {
        caller: Option<Value>,
        reply: mpsc::Sender<BridgeResult>,
    },
    CurrentWorkspace {
        reply: mpsc::Sender<BridgeResult>,
    },
    ListWorkspaces {
        reply: mpsc::Sender<BridgeResult>,
    },
    CreateWorkspace {
        name: Option<String>,
        cwd: Option<String>,
        command: Option<String>,
        reply: mpsc::Sender<BridgeResult>,
    },
    SelectWorkspace {
        target: WorkspaceTarget,
        reply: mpsc::Sender<BridgeResult>,
    },
    RenameWorkspace {
        target: WorkspaceTarget,
        title: String,
        reply: mpsc::Sender<BridgeResult>,
    },
    CloseWorkspace {
        target: WorkspaceTarget,
        reply: mpsc::Sender<BridgeResult>,
    },
    SendText {
        target: WorkspaceTarget,
        surface_hint: Option<String>,
        text: String,
        reply: mpsc::Sender<BridgeResult>,
    },
}

impl ControlCommand {
    pub fn respond(self, result: BridgeResult) {
        match self {
            Self::Identify { reply, .. }
            | Self::CurrentWorkspace { reply }
            | Self::ListWorkspaces { reply }
            | Self::CreateWorkspace { reply, .. }
            | Self::SelectWorkspace { reply, .. }
            | Self::RenameWorkspace { reply, .. }
            | Self::CloseWorkspace { reply, .. }
            | Self::SendText { reply, .. } => {
                let _ = reply.send(result);
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BridgeError {
    code: i64,
    message: String,
    data: Option<Value>,
}

impl BridgeError {
    fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS_CODE, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(NOT_FOUND_CODE, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(CONFLICT_CODE, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(INTERNAL_ERROR_CODE, message)
    }
}

fn parse_request(input: &str) -> Result<V2Request, BridgeError> {
    if let Ok(request) = serde_json::from_str::<V2Request>(input) {
        return Ok(request);
    }

    match parse_v1_command_envelope(input) {
        Ok(v1) => Ok(v1.into_v2_request(None)),
        Err(error) => Err(BridgeError::new(
            PARSE_ERROR_CODE,
            format!("invalid request payload: {error}"),
        )
        .with_data(json!({ "raw": input }))),
    }
}

fn params_object(params: &Value) -> Result<&Map<String, Value>, BridgeError> {
    params
        .as_object()
        .ok_or_else(|| BridgeError::invalid_params("params must be a JSON object"))
}

fn optional_string(params: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        params
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn optional_index(params: &Map<String, Value>, key: &str) -> Result<Option<usize>, BridgeError> {
    let Some(value) = params.get(key) else {
        return Ok(None);
    };

    if let Some(index) = value.as_u64() {
        return Ok(Some(index as usize));
    }

    Err(BridgeError::invalid_params(format!(
        "{key} must be a non-negative integer"
    )))
}

fn parse_optional_workspace_target(
    params: &Map<String, Value>,
    allow_name: bool,
) -> Result<WorkspaceTarget, BridgeError> {
    if let Some(handle) = optional_string(params, &["workspace_id", "id"]) {
        return Ok(WorkspaceTarget::Handle(handle));
    }
    if allow_name {
        if let Some(name) = optional_string(params, &["name"]) {
            return Ok(WorkspaceTarget::Name(name));
        }
    }
    if let Some(index) = optional_index(params, "index")? {
        return Ok(WorkspaceTarget::Index(index));
    }
    Ok(WorkspaceTarget::Active)
}

fn parse_required_workspace_target(
    params: &Map<String, Value>,
    allow_name: bool,
    method: &str,
) -> Result<WorkspaceTarget, BridgeError> {
    let target = parse_optional_workspace_target(params, allow_name)?;
    if matches!(target, WorkspaceTarget::Active) {
        Err(BridgeError::invalid_params(format!(
            "{method} requires workspace_id/id, name, or index"
        )))
    } else {
        Ok(target)
    }
}

fn handle_method(
    id: Option<Value>,
    method: &str,
    params: Value,
    dispatch: &dyn Fn(ControlCommand),
) -> V2Response {
    let params = match params_object(&params) {
        Ok(params) => params,
        Err(error) => return error_response(id, error),
    };

    let queued = match method {
        "system.ping" | "ping" => return V2Response::success(id, json!({ "pong": true })),
        "system.capabilities" => {
            return V2Response::success(id, json!({ "commands": METHODS, "methods": METHODS }));
        }
        "system.identify" => {
            let (reply, rx) = mpsc::channel();
            (
                ControlCommand::Identify {
                    caller: params.get("caller").cloned(),
                    reply,
                },
                rx,
            )
        }
        "workspace.current" => {
            let (reply, rx) = mpsc::channel();
            (ControlCommand::CurrentWorkspace { reply }, rx)
        }
        "workspace.list" | "list-workspaces" => {
            let (reply, rx) = mpsc::channel();
            (ControlCommand::ListWorkspaces { reply }, rx)
        }
        "workspace.create" | "new-workspace" => {
            let (reply, rx) = mpsc::channel();
            (
                ControlCommand::CreateWorkspace {
                    name: optional_string(params, &["name", "title"]),
                    cwd: optional_string(params, &["cwd"]),
                    command: optional_string(params, &["command"]),
                    reply,
                },
                rx,
            )
        }
        "workspace.select" | "workspace.activate" | "activate-workspace" => {
            let target = match parse_required_workspace_target(params, true, method) {
                Ok(target) => target,
                Err(error) => return error_response(id, error),
            };
            let (reply, rx) = mpsc::channel();
            (ControlCommand::SelectWorkspace { target, reply }, rx)
        }
        "workspace.rename" | "rename-workspace" => {
            let Some(title) = optional_string(params, &["title", "name"]) else {
                return error_response(
                    id,
                    BridgeError::invalid_params("workspace.rename requires title/name"),
                );
            };
            let target = match parse_optional_workspace_target(params, false) {
                Ok(target) => target,
                Err(error) => return error_response(id, error),
            };
            let (reply, rx) = mpsc::channel();
            (
                ControlCommand::RenameWorkspace {
                    target,
                    title,
                    reply,
                },
                rx,
            )
        }
        "workspace.close" | "close-workspace" => {
            let target = match parse_optional_workspace_target(params, false) {
                Ok(target) => target,
                Err(error) => return error_response(id, error),
            };
            let (reply, rx) = mpsc::channel();
            (ControlCommand::CloseWorkspace { target, reply }, rx)
        }
        "surface.send_text" | "send-text" | "send" => {
            let Some(text) = optional_string(params, &["text"]) else {
                return error_response(
                    id,
                    BridgeError::invalid_params("surface.send_text requires text"),
                );
            };
            let target = match parse_optional_workspace_target(params, false) {
                Ok(target) => target,
                Err(error) => return error_response(id, error),
            };
            let (reply, rx) = mpsc::channel();
            (
                ControlCommand::SendText {
                    target,
                    surface_hint: optional_string(params, &["surface_id"]),
                    text,
                    reply,
                },
                rx,
            )
        }
        _ => {
            return error_response(
                id,
                BridgeError::new(UNKNOWN_METHOD_CODE, format!("unknown method: {method}")),
            );
        }
    };

    let (command, reply_rx) = queued;

    dispatch(command);

    match reply_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(result)) => V2Response::success(id, result),
        Ok(Err(error)) => error_response(id, error),
        Err(_) => error_response(id, BridgeError::internal("control command timed out")),
    }
}

fn error_response(id: Option<Value>, error: BridgeError) -> V2Response {
    V2Response::error(id, error.code, error.message, error.data)
}

fn dispatch_request(input: &str, dispatch: &dyn Fn(ControlCommand)) -> V2Response {
    match parse_request(input) {
        Ok(request) => handle_method(request.id, &request.method, request.params, dispatch),
        Err(error) => error_response(None, error),
    }
}

fn handle_client(
    stream: UnixStream,
    dispatch: &(dyn Fn(ControlCommand) + Send + Sync + 'static),
) -> io::Result<()> {
    let reader_stream = stream.try_clone()?;
    let reader = io::BufReader::new(reader_stream);
    let mut writer = stream;

    for line in reader.lines() {
        let line = line?;
        let input = line.trim();
        if input.is_empty() {
            continue;
        }

        let response = dispatch_request(input, dispatch);
        let mut payload = serde_json::to_string(&response)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
        payload.push('\n');
        writer.write_all(payload.as_bytes())?;
        writer.flush()?;
    }

    Ok(())
}

/// Start the control socket server in a background thread and dispatch each
/// command onto the GTK main context.
pub fn start(dispatch: fn(ControlCommand)) {
    let context = glib::MainContext::default();
    let dispatch = std::sync::Arc::new(move |command: ControlCommand| {
        context.invoke(move || dispatch(command));
    });

    std::thread::Builder::new()
        .name("limux-control".into())
        .spawn(move || {
            let path = resolve_socket_path(None, SocketMode::Runtime);
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if path.exists() {
                let _ = std::fs::remove_file(&path);
            }

            let listener = match UnixListener::bind(&path) {
                Ok(listener) => listener,
                Err(error) => {
                    eprintln!(
                        "limux: control socket bind failed ({}): {error}",
                        path.display()
                    );
                    return;
                }
            };

            eprintln!("limux: control socket at {}", path.display());

            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        let dispatch = dispatch.clone();
                        std::thread::Builder::new()
                            .name("limux-ctrl-conn".into())
                            .spawn(move || {
                                if let Err(error) = handle_client(stream, dispatch.as_ref()) {
                                    eprintln!("limux: control connection error: {error}");
                                }
                            })
                            .ok();
                    }
                    Err(error) => {
                        eprintln!("limux: control accept error: {error}");
                    }
                }
            }
        })
        .expect("failed to spawn control server thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_v2_request_directly() {
        let request = parse_request(r#"{"id":"1","method":"system.ping","params":{}}"#)
            .expect("v2 request should parse");
        assert_eq!(request.id, Some(Value::String("1".to_string())));
        assert_eq!(request.method, "system.ping");
    }

    #[test]
    fn parses_v1_request_envelope() {
        let request = parse_request(r#"{"command":"workspace.create","args":{"cwd":"/tmp"}}"#)
            .expect("v1 request should parse");
        assert_eq!(request.method, "workspace.create");
        assert_eq!(request.params["cwd"], "/tmp");
    }

    #[test]
    fn workspace_target_prefers_handle_over_index() {
        let params = json!({
            "workspace_id": "workspace:abc",
            "index": 2
        });
        let target =
            parse_optional_workspace_target(params.as_object().expect("object params"), true)
                .expect("target should parse");
        assert_eq!(target, WorkspaceTarget::Handle("workspace:abc".to_string()));
    }

    #[test]
    fn workspace_select_requires_explicit_target() {
        let params = Map::new();
        let error = parse_required_workspace_target(&params, true, "workspace.select")
            .expect_err("workspace.select should require a target");
        assert_eq!(error.code, INVALID_PARAMS_CODE);
    }
}
