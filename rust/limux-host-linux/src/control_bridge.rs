//! Bridge the limux control socket onto the GTK host state.

use std::io::{self, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use gtk::glib;
use gtk4 as gtk;
use limux_control::auth::{self, SocketControlMode};
use limux_control::request_io::{self, read_request_frame};
use limux_control::socket_path::{bind_listener, resolve_socket_path, SocketMode};
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
    "pane.list",
    "pane.surfaces",
    "pane.create",
    "surface.list",
    "surface.send_text",
    "surface.send_key",
    "notification.create",
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PaneCreateDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PaneCreateType {
    Terminal,
    Browser,
}

/// Parser-level contract for the live-GTK `pane.create` route.
///
/// Request fields accepted by the bridge:
/// - `workspace_id`/`id`, `name`, or `index` target the workspace. Raw
///   handles and `workspace:<id>` refs are accepted and preserved for the GTK
///   layer to resolve.
/// - `surface_id` and `pane_id` identify the source pane. Raw handles and
///   `surface:<id>`/`pane:<id>` refs are accepted. Later GTK work resolves
///   precedence as explicit surface, explicit pane, then safe workspace-local
///   fallback.
/// - `direction` is one of `left|right|up|down`, defaulting to `right`.
/// - `type` is one of `terminal|browser`, defaulting to `terminal`.
/// - `command` is a terminal-only host extension: the host injects it into the
///   newly-created surface after creation. The standalone core dispatcher may
///   accept the field for compatibility but does not launch a process.
///
/// This delivery only implements live-GTK terminal panes. Browser pane support
/// remains a follow-up, so `type=browser` and `url` fail at parse time before
/// any GTK work is scheduled. Responses must keep the existing core/CLI field
/// names: `pane_id`, `pane_ref`, `surface_id`, and `surface_ref`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreatePaneRequest {
    pub target: WorkspaceTarget,
    pub source_pane_id: Option<String>,
    pub source_surface_id: Option<String>,
    pub direction: PaneCreateDirection,
    pub pane_type: PaneCreateType,
    pub command: Option<String>,
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
    ListPanes {
        target: WorkspaceTarget,
        reply: mpsc::Sender<BridgeResult>,
    },
    ListPaneSurfaces {
        target: WorkspaceTarget,
        pane_id: Option<String>,
        reply: mpsc::Sender<BridgeResult>,
    },
    CreatePane {
        request: CreatePaneRequest,
        reply: mpsc::Sender<BridgeResult>,
    },
    ListSurfaces {
        target: WorkspaceTarget,
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
    SendKey {
        target: WorkspaceTarget,
        surface_hint: Option<String>,
        key: String,
        reply: mpsc::Sender<BridgeResult>,
    },
    /// Post a desktop-style notification into the sidebar + toast overlay.
    /// `target` chooses the workspace to flag as unread; if not provided,
    /// the currently-active workspace is used.
    CreateNotification {
        target: WorkspaceTarget,
        title: String,
        subtitle: String,
        body: String,
        reply: mpsc::Sender<BridgeResult>,
    },
}

impl ControlCommand {
    pub fn respond(self, result: BridgeResult) {
        match self {
            Self::Identify { reply, .. }
            | Self::CurrentWorkspace { reply }
            | Self::ListWorkspaces { reply }
            | Self::ListPanes { reply, .. }
            | Self::ListPaneSurfaces { reply, .. }
            | Self::CreatePane { reply, .. }
            | Self::ListSurfaces { reply, .. }
            | Self::CreateWorkspace { reply, .. }
            | Self::SelectWorkspace { reply, .. }
            | Self::RenameWorkspace { reply, .. }
            | Self::CloseWorkspace { reply, .. }
            | Self::SendText { reply, .. }
            | Self::SendKey { reply, .. }
            | Self::CreateNotification { reply, .. } => {
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

fn optional_handle(
    params: &Map<String, Value>,
    keys: &[&str],
) -> Result<Option<String>, BridgeError> {
    for key in keys {
        let Some(value) = params.get(*key) else {
            continue;
        };
        match value {
            Value::Null => {}
            Value::String(raw) => {
                let handle = raw.trim();
                if !handle.is_empty() {
                    return Ok(Some(handle.to_string()));
                }
            }
            Value::Number(number) => {
                let id = number.as_u64().ok_or_else(|| {
                    BridgeError::invalid_params(format!(
                        "{key} must be a non-negative integer or ref handle"
                    ))
                })?;
                return Ok(Some(id.to_string()));
            }
            _ => {
                return Err(BridgeError::invalid_params(format!(
                    "{key} must be a non-negative integer or ref handle"
                )));
            }
        }
    }
    Ok(None)
}

fn optional_ref_handle(
    params: &Map<String, Value>,
    keys: &[&str],
    prefix: &str,
) -> Result<Option<String>, BridgeError> {
    optional_handle(params, keys).map(|handle| {
        handle.map(|handle| {
            handle
                .strip_prefix(prefix)
                .unwrap_or(handle.as_str())
                .to_string()
        })
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

fn looks_like_workspace_handle(raw: &str) -> bool {
    let raw = raw.trim();
    if raw.starts_with("workspace:") {
        return true;
    }
    let value = raw;
    uuid::Uuid::parse_str(value).is_ok() || value.chars().all(|ch| ch.is_ascii_digit())
}

fn parse_optional_workspace_target(
    params: &Map<String, Value>,
    allow_name: bool,
) -> Result<WorkspaceTarget, BridgeError> {
    if let Some(handle) = optional_handle(params, &["workspace_id", "id"])? {
        if allow_name && !looks_like_workspace_handle(&handle) {
            return Ok(WorkspaceTarget::Name(handle));
        }
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

#[cfg_attr(not(test), allow(dead_code))]
fn parse_create_pane_request(
    params: &Map<String, Value>,
) -> Result<CreatePaneRequest, BridgeError> {
    let direction = match optional_string(params, &["direction"])
        .unwrap_or_else(|| "right".to_string())
        .as_str()
    {
        "left" => PaneCreateDirection::Left,
        "right" => PaneCreateDirection::Right,
        "up" => PaneCreateDirection::Up,
        "down" => PaneCreateDirection::Down,
        _ => {
            return Err(BridgeError::invalid_params(
                "pane.create direction must be one of left|right|up|down",
            ));
        }
    };

    let pane_type = match optional_string(params, &["type"])
        .unwrap_or_else(|| "terminal".to_string())
        .as_str()
    {
        "terminal" => PaneCreateType::Terminal,
        "browser" => PaneCreateType::Browser,
        _ => {
            return Err(BridgeError::invalid_params(
                "pane.create type must be one of terminal|browser",
            ));
        }
    };

    if matches!(pane_type, PaneCreateType::Browser) {
        return Err(BridgeError::invalid_params(
            "pane.create live GTK bridge supports type=terminal only",
        ));
    }
    if optional_string(params, &["url"]).is_some() {
        return Err(BridgeError::invalid_params(
            "pane.create url is only supported for browser panes",
        ));
    }

    Ok(CreatePaneRequest {
        target: parse_optional_workspace_target(params, true)?,
        source_pane_id: optional_ref_handle(params, &["pane_id"], "pane:")?,
        source_surface_id: optional_ref_handle(params, &["surface_id"], "surface:")?,
        direction,
        pane_type,
        command: optional_string(params, &["command"]),
    })
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
        "pane.list" | "list-panes" => {
            let target = match parse_optional_workspace_target(params, true) {
                Ok(target) => target,
                Err(error) => return error_response(id, error),
            };
            let (reply, rx) = mpsc::channel();
            (ControlCommand::ListPanes { target, reply }, rx)
        }
        "pane.surfaces" => {
            let target = match parse_optional_workspace_target(params, true) {
                Ok(target) => target,
                Err(error) => return error_response(id, error),
            };
            let (reply, rx) = mpsc::channel();
            (
                ControlCommand::ListPaneSurfaces {
                    target,
                    pane_id: optional_string(params, &["pane_id", "id"]),
                    reply,
                },
                rx,
            )
        }
        "pane.create" | "new-pane" => {
            let request = match parse_create_pane_request(params) {
                Ok(request) => request,
                Err(error) => return error_response(id, error),
            };
            let (reply, rx) = mpsc::channel();
            (ControlCommand::CreatePane { request, reply }, rx)
        }
        "surface.list" | "list-panels" => {
            let target = match parse_optional_workspace_target(params, true) {
                Ok(target) => target,
                Err(error) => return error_response(id, error),
            };
            let (reply, rx) = mpsc::channel();
            (ControlCommand::ListSurfaces { target, reply }, rx)
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
            // allow_name = true: lets agent-team peers address each other by
            // workspace name (e.g. `--workspace codex`) instead of UUID.
            let target = match parse_optional_workspace_target(params, true) {
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
        "surface.send_key" | "send-key" => {
            let Some(key) = optional_string(params, &["key"]) else {
                return error_response(
                    id,
                    BridgeError::invalid_params("surface.send_key requires key"),
                );
            };
            let target = match parse_optional_workspace_target(params, true) {
                Ok(target) => target,
                Err(error) => return error_response(id, error),
            };
            let (reply, rx) = mpsc::channel();
            (
                ControlCommand::SendKey {
                    target,
                    surface_hint: optional_string(params, &["surface_id"]),
                    key,
                    reply,
                },
                rx,
            )
        }
        "notification.create" | "notify" => {
            // Title is required; subtitle and body are optional. This mirrors
            // cmux notify's shape (title/subtitle/body) and maps onto the
            // existing sidebar unread pipeline.
            let Some(title) = optional_string(params, &["title"]) else {
                return error_response(
                    id,
                    BridgeError::invalid_params("notification.create requires title"),
                );
            };
            let subtitle = optional_string(params, &["subtitle"]).unwrap_or_default();
            let body = optional_string(params, &["body", "message"]).unwrap_or_default();
            // allow_name = true: lets agent hooks target a peer by name.
            let target = match parse_optional_workspace_target(params, true) {
                Ok(target) => target,
                Err(error) => return error_response(id, error),
            };
            let (reply, rx) = mpsc::channel();
            (
                ControlCommand::CreateNotification {
                    target,
                    title,
                    subtitle,
                    body,
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
    stream.set_read_timeout(Some(request_io::CLIENT_IDLE_TIMEOUT))?;
    let reader_stream = stream.try_clone()?;
    reader_stream.set_read_timeout(Some(request_io::CLIENT_IDLE_TIMEOUT))?;
    let mut reader = io::BufReader::new(reader_stream);
    let mut writer = stream;
    let mut line_buf = Vec::with_capacity(4096);

    loop {
        if !read_request_frame(&mut reader, &mut line_buf)? {
            return Ok(());
        }

        let input = std::str::from_utf8(&line_buf)
            .map(|line| line.trim_end_matches(['\n', '\r']))
            .unwrap_or("");
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
}

struct ConnectionSlot {
    active_connections: Arc<AtomicUsize>,
}

impl ConnectionSlot {
    fn try_acquire(active_connections: Arc<AtomicUsize>) -> Option<Self> {
        active_connections
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current < request_io::MAX_CONNECTIONS).then_some(current + 1)
            })
            .ok()?;
        Some(Self { active_connections })
    }
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.active_connections.fetch_sub(1, Ordering::AcqRel);
    }
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
            let control_mode = SocketControlMode::from_env();
            let listener = match bind_listener(
                &path,
                SocketMode::Runtime,
                control_mode.requires_owner_only_socket(),
            ) {
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
            let active_connections = Arc::new(AtomicUsize::new(0));

            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        let Some(slot) = ConnectionSlot::try_acquire(active_connections.clone()) else {
                            eprintln!("limux: rejecting control client, too many active connections");
                            continue;
                        };
                        let peer = match auth::authorize_peer(&stream, control_mode) {
                            Ok(peer) => peer,
                            Err(error) => {
                                eprintln!("limux: rejected control client: {error}");
                                continue;
                            }
                        };
                        let dispatch = dispatch.clone();
                        std::thread::Builder::new()
                            .name("limux-ctrl-conn".into())
                            .spawn(move || {
                                let _slot = slot;
                                if let Err(error) = handle_client(stream, dispatch.as_ref()) {
                                    eprintln!(
                                        "limux: control connection error for pid={} uid={}: {error}",
                                        peer.pid, peer.uid
                                    );
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
    fn workspace_target_treats_cli_workspace_id_as_name_when_allowed() {
        let params = json!({
            "workspace_id": "claude"
        });
        let target =
            parse_optional_workspace_target(params.as_object().expect("object params"), true)
                .expect("target should parse");
        assert_eq!(target, WorkspaceTarget::Name("claude".to_string()));
    }

    #[test]
    fn workspace_target_preserves_raw_uuid_workspace_ids_when_names_are_allowed() {
        let workspace_id = "2b8b5ca4-0200-4433-9f7c-d5c9f725be50";
        let params = json!({
            "workspace_id": workspace_id
        });
        let target =
            parse_optional_workspace_target(params.as_object().expect("object params"), true)
                .expect("target should parse");
        assert_eq!(target, WorkspaceTarget::Handle(workspace_id.to_string()));
    }

    #[test]
    fn workspace_select_requires_explicit_target() {
        let params = Map::new();
        let error = parse_required_workspace_target(&params, true, "workspace.select")
            .expect_err("workspace.select should require a target");
        assert_eq!(error.code, INVALID_PARAMS_CODE);
    }

    #[test]
    fn pane_create_contract_accepts_raw_and_ref_targets() {
        let params = json!({
            "workspace_id": 7,
            "surface_id": "surface:11",
            "pane_id": "pane:12",
            "direction": "left",
            "type": "terminal",
            "command": "claude"
        });
        let request = parse_create_pane_request(params.as_object().expect("object params"))
            .expect("pane.create request should parse");

        assert_eq!(request.target, WorkspaceTarget::Handle("7".to_string()));
        assert_eq!(request.source_surface_id, Some("11".to_string()));
        assert_eq!(request.source_pane_id, Some("12".to_string()));
        assert_eq!(request.direction, PaneCreateDirection::Left);
        assert_eq!(request.pane_type, PaneCreateType::Terminal);
        assert_eq!(request.command, Some("claude".to_string()));
    }

    #[test]
    fn pane_create_contract_rejects_invalid_direction_and_type() {
        let bad_direction = json!({ "direction": "diagonal" });
        let error = parse_create_pane_request(bad_direction.as_object().expect("object params"))
            .expect_err("invalid direction should fail");
        assert_eq!(error.code, INVALID_PARAMS_CODE);

        let bad_type = json!({ "type": "webview" });
        let error = parse_create_pane_request(bad_type.as_object().expect("object params"))
            .expect_err("invalid type should fail");
        assert_eq!(error.code, INVALID_PARAMS_CODE);
    }

    #[test]
    fn pane_create_contract_rejects_deferred_browser_fields() {
        let browser = json!({ "type": "browser" });
        let error = parse_create_pane_request(browser.as_object().expect("object params"))
            .expect_err("browser panes are deferred");
        assert_eq!(error.code, INVALID_PARAMS_CODE);

        let url = json!({ "url": "https://example.com" });
        let error = parse_create_pane_request(url.as_object().expect("object params"))
            .expect_err("url is browser-only");
        assert_eq!(error.code, INVALID_PARAMS_CODE);
    }

    #[test]
    fn pane_create_route_queues_create_pane_command() {
        let response = dispatch_request(
            r#"{"id":1,"method":"pane.create","params":{"name":"claude","surface_id":"surface:4:tab","direction":"down","command":"codex"}}"#,
            &|command| match command {
                ControlCommand::CreatePane { request, reply } => {
                    assert_eq!(request.target, WorkspaceTarget::Name("claude".to_string()));
                    assert_eq!(request.source_surface_id, Some("4:tab".to_string()));
                    assert_eq!(request.direction, PaneCreateDirection::Down);
                    assert_eq!(request.command, Some("codex".to_string()));
                    let _ = reply.send(Ok(json!({
                        "pane_id": "9",
                        "pane_ref": "pane:9",
                        "surface_id": "9:tab",
                        "surface_ref": "surface:9:tab"
                    })));
                }
                other => panic!("unexpected command: {other:?}"),
            },
        );

        assert_eq!(response.error, None);
        let result = response.result.expect("pane.create should return a result");
        assert_eq!(result["pane_ref"], "pane:9");
        assert_eq!(result["surface_ref"], "surface:9:tab");
    }

    #[test]
    fn pane_create_route_rejects_invalid_params_before_dispatch() {
        let response = dispatch_request(
            r#"{"id":1,"method":"new-pane","params":{"direction":"diagonal"}}"#,
            &|command| panic!("invalid pane.create should not dispatch: {command:?}"),
        );

        assert_eq!(response.result, None);
        assert_eq!(
            response.error.as_ref().map(|error| error.code),
            Some(INVALID_PARAMS_CODE)
        );
    }
}
