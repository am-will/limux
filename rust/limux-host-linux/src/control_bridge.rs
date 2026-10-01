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

use crate::workspace_color::WorkspaceColor;

const METHODS: &[&str] = &[
    "system.ping",
    "system.identify",
    "system.capabilities",
    "window.activate",
    "workspace.current",
    "workspace.list",
    "workspace.create",
    "workspace.select",
    "workspace.rename",
    "workspace.set_color",
    "workspace.close",
    "pane.list",
    "pane.surfaces",
    "pane.create",
    "surface.list",
    "surface.health",
    "surface.read_text",
    "surface.send_text",
    "surface.send_key",
    "surface.create",
    "surface.close",
    "surface.focus",
    "pane.focus",
    "tab.action",
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
    ActivateWindow {
        activation_token: Option<String>,
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
    SurfaceHealth {
        target: WorkspaceTarget,
        surface_hint: Option<String>,
        reply: mpsc::Sender<BridgeResult>,
    },
    ReadSurfaceText {
        target: WorkspaceTarget,
        surface_hint: Option<String>,
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
    SetWorkspaceColor {
        target: WorkspaceTarget,
        color: Option<WorkspaceColor>,
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
    /// Open a new tab (surface) inside an existing pane.
    CreateSurface {
        target: WorkspaceTarget,
        pane_hint: Option<String>,
        browser: bool,
        url: Option<String>,
        reply: mpsc::Sender<BridgeResult>,
    },
    /// Close a single surface (tab). The only sub-workspace teardown there is.
    CloseSurface {
        target: WorkspaceTarget,
        surface_hint: Option<String>,
        reply: mpsc::Sender<BridgeResult>,
    },
    /// Make a surface the active tab of its pane.
    FocusSurface {
        target: WorkspaceTarget,
        surface_hint: Option<String>,
        reply: mpsc::Sender<BridgeResult>,
    },
    /// Move keyboard focus to a pane.
    FocusPane {
        target: WorkspaceTarget,
        pane_hint: Option<String>,
        reply: mpsc::Sender<BridgeResult>,
    },
    /// Act on a tab: `rename`, `pin`, `unpin`, `close`, `select`.
    TabAction {
        target: WorkspaceTarget,
        surface_hint: Option<String>,
        action: String,
        title: Option<String>,
        reply: mpsc::Sender<BridgeResult>,
    },
    /// Post a desktop-style notification into the sidebar + toast overlay.
    /// `target` chooses the workspace; `surface_hint` identifies the tab when available.
    CreateNotification {
        target: WorkspaceTarget,
        surface_hint: Option<String>,
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
            | Self::ActivateWindow { reply, .. }
            | Self::CurrentWorkspace { reply }
            | Self::ListWorkspaces { reply }
            | Self::ListPanes { reply, .. }
            | Self::ListPaneSurfaces { reply, .. }
            | Self::CreatePane { reply, .. }
            | Self::ListSurfaces { reply, .. }
            | Self::SurfaceHealth { reply, .. }
            | Self::ReadSurfaceText { reply, .. }
            | Self::CreateWorkspace { reply, .. }
            | Self::SelectWorkspace { reply, .. }
            | Self::RenameWorkspace { reply, .. }
            | Self::SetWorkspaceColor { reply, .. }
            | Self::CloseWorkspace { reply, .. }
            | Self::SendText { reply, .. }
            | Self::SendKey { reply, .. }
            | Self::CreateSurface { reply, .. }
            | Self::CloseSurface { reply, .. }
            | Self::FocusSurface { reply, .. }
            | Self::FocusPane { reply, .. }
            | Self::TabAction { reply, .. }
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

/// A supplied terminal target must remain explicit, including malformed empty
/// handles. Dropping it would redirect input to the active or first terminal.
fn optional_surface_handle(
    params: &Map<String, Value>,
    keys: &[&str],
) -> Result<Option<String>, BridgeError> {
    for key in keys {
        if params.get(*key).is_none_or(Value::is_null) {
            continue;
        }
        let handle = optional_ref_handle(params, &[*key], "surface:")?;
        return handle
            .filter(|value| !value.trim().is_empty())
            .map(|value| Some(value.trim().to_string()))
            .ok_or_else(|| BridgeError::invalid_params(format!("{key} must not be empty")));
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

fn optional_explicit_handle(
    params: &Map<String, Value>,
    keys: &[&str],
    prefix: &str,
) -> Result<Option<String>, BridgeError> {
    let mut selected = None;
    for key in keys {
        if !params.contains_key(*key) {
            continue;
        }
        let handle = optional_ref_handle(params, &[*key], prefix)?
            .filter(|handle| !handle.trim().is_empty())
            .ok_or_else(|| {
                BridgeError::invalid_params(format!("{key} must not be empty or null"))
            })?;
        if selected.is_none() {
            selected = Some(handle);
        }
    }
    Ok(selected)
}

fn parse_lifecycle_workspace_target(
    params: &Map<String, Value>,
) -> Result<WorkspaceTarget, BridgeError> {
    optional_explicit_handle(params, &["workspace_id", "id"], "workspace:")?;
    if let Some(name) = params.get("name") {
        if name.as_str().is_none_or(|name| name.trim().is_empty()) {
            return Err(BridgeError::invalid_params(
                "name must be a non-empty string",
            ));
        }
    }
    optional_index(params, "index")?;
    parse_optional_workspace_target(params, true)
}

fn optional_tab_handle(params: &Map<String, Value>) -> Result<Option<String>, BridgeError> {
    let mut selected = None;
    for key in ["surface_id", "tab_id"] {
        if let Some(handle) = optional_explicit_handle(params, &[key], "surface:")? {
            let handle = handle.strip_prefix("tab:").unwrap_or(&handle).trim();
            if handle.is_empty() {
                return Err(BridgeError::invalid_params(format!(
                    "{key} must not be empty"
                )));
            }
            if selected.is_none() {
                selected = Some(handle.to_string());
            }
        }
    }
    Ok(selected)
}

fn parse_surface_kind(params: &Map<String, Value>) -> Result<(bool, Option<String>), BridgeError> {
    let mut browser = None;
    for key in ["type", "kind"] {
        let Some(value) = params.get(key) else {
            continue;
        };
        let parsed = match value.as_str().map(str::to_ascii_lowercase).as_deref() {
            Some("terminal") => false,
            Some("browser") => true,
            _ => {
                return Err(BridgeError::invalid_params(format!(
                    "{key} must be terminal or browser"
                )))
            }
        };
        if browser.is_some_and(|previous| previous != parsed) {
            return Err(BridgeError::invalid_params("type and kind must agree"));
        }
        browser = Some(parsed);
    }
    let url = params
        .get("url")
        .map(|value| {
            value
                .as_str()
                .map(str::trim)
                .filter(|url| !url.is_empty())
                .map(ToOwned::to_owned)
                .ok_or_else(|| BridgeError::invalid_params("url must be a non-empty string"))
        })
        .transpose()?;
    if browser == Some(false) && url.is_some() {
        return Err(BridgeError::invalid_params(
            "url requires a browser surface",
        ));
    }
    if let Some(url) = &url {
        if url.chars().any(|ch| ch.is_whitespace() || ch.is_control()) {
            return Err(BridgeError::invalid_params(
                "url must be an absolute URI without control characters",
            ));
        }
        let uri = glib::Uri::parse(url, glib::UriFlags::NONE)
            .map_err(|_| BridgeError::invalid_params("url must be an absolute URI"))?;
        if matches!(uri.scheme().as_str(), "http" | "https")
            && uri.host().is_none_or(|host| host.is_empty())
        {
            return Err(BridgeError::invalid_params("url must be an absolute URI"));
        }
    }
    Ok((browser.unwrap_or(url.is_some()), url))
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
/// Read the `color` param of `workspace.set_color`: a palette name, or
/// `"none"` / `null` to clear the colour.
fn parse_workspace_color(
    params: &Map<String, Value>,
) -> Result<Option<WorkspaceColor>, BridgeError> {
    let name = match params.get("color") {
        None => {
            return Err(BridgeError::invalid_params(
                "workspace.set_color requires color",
            ))
        }
        Some(Value::Null) => return Ok(None),
        Some(Value::String(name)) => name.trim(),
        Some(_) => {
            return Err(BridgeError::invalid_params(
                "workspace.set_color color must be a string or null",
            ))
        }
    };
    if name.eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    WorkspaceColor::from_name(name).map(Some).ok_or_else(|| {
        BridgeError::invalid_params(format!(
            "unknown color {name:?}; expected one of: {}, none",
            crate::workspace_color::color_names()
        ))
    })
}

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
        "window.activate" => {
            let activation_token = match params.get("activation_token") {
                None | Some(Value::Null) => None,
                Some(Value::String(token)) if !token.contains('\0') => {
                    (!token.is_empty()).then(|| token.clone())
                }
                _ => {
                    return error_response(
                        id,
                        BridgeError::invalid_params(
                            "activation_token must be a string without NUL characters",
                        ),
                    );
                }
            };
            let (reply, rx) = mpsc::channel();
            (
                ControlCommand::ActivateWindow {
                    activation_token,
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
        "surface.health" | "surface-health" => {
            let target = match parse_optional_workspace_target(params, true) {
                Ok(target) => target,
                Err(error) => return error_response(id, error),
            };
            let surface_hint = match optional_ref_handle(params, &["surface_id", "id"], "surface:")
            {
                Ok(surface_hint) => surface_hint,
                Err(error) => return error_response(id, error),
            };
            let (reply, rx) = mpsc::channel();
            (
                ControlCommand::SurfaceHealth {
                    target,
                    surface_hint,
                    reply,
                },
                rx,
            )
        }
        "surface.read_text" | "read-screen" | "capture-pane" => {
            let target = match parse_optional_workspace_target(params, true) {
                Ok(target) => target,
                Err(error) => return error_response(id, error),
            };
            let surface_hint = match optional_surface_handle(params, &["surface_id", "id"]) {
                Ok(surface_hint) => surface_hint,
                Err(error) => return error_response(id, error),
            };
            let (reply, rx) = mpsc::channel();
            (
                ControlCommand::ReadSurfaceText {
                    target,
                    surface_hint,
                    reply,
                },
                rx,
            )
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
        "workspace.set_color" | "set-workspace-color" => {
            let color = match parse_workspace_color(params) {
                Ok(color) => color,
                Err(error) => return error_response(id, error),
            };
            let target = match parse_optional_workspace_target(params, false) {
                Ok(target) => target,
                Err(error) => return error_response(id, error),
            };
            let (reply, rx) = mpsc::channel();
            (
                ControlCommand::SetWorkspaceColor {
                    target,
                    color,
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
            let Some(text) = params
                .get("text")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
            else {
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
            let surface_hint = match optional_surface_handle(params, &["surface_id"]) {
                Ok(surface_hint) => surface_hint,
                Err(error) => return error_response(id, error),
            };
            let (reply, rx) = mpsc::channel();
            (
                ControlCommand::SendText {
                    target,
                    surface_hint,
                    text,
                    reply,
                },
                rx,
            )
        }
        "surface.create" | "new-surface" => {
            let target = match parse_lifecycle_workspace_target(params) {
                Ok(target) => target,
                Err(error) => return error_response(id, error),
            };
            let (browser, url) = match parse_surface_kind(params) {
                Ok(kind) => kind,
                Err(error) => return error_response(id, error),
            };
            let pane_hint = match optional_explicit_handle(params, &["pane_id"], "pane:") {
                Ok(handle) => handle,
                Err(error) => return error_response(id, error),
            };
            let (reply, rx) = mpsc::channel();
            (
                ControlCommand::CreateSurface {
                    target,
                    pane_hint,
                    browser,
                    url,
                    reply,
                },
                rx,
            )
        }
        "surface.close" | "close-surface" => {
            let target = match parse_lifecycle_workspace_target(params) {
                Ok(target) => target,
                Err(error) => return error_response(id, error),
            };
            let surface_hint = match optional_explicit_handle(params, &["surface_id"], "surface:") {
                Ok(handle) => handle,
                Err(error) => return error_response(id, error),
            };
            let (reply, rx) = mpsc::channel();
            (
                ControlCommand::CloseSurface {
                    target,
                    surface_hint,
                    reply,
                },
                rx,
            )
        }
        "surface.focus" | "focus-surface" => {
            let target = match parse_lifecycle_workspace_target(params) {
                Ok(target) => target,
                Err(error) => return error_response(id, error),
            };
            let surface_hint = match optional_explicit_handle(params, &["surface_id"], "surface:") {
                Ok(handle) => handle,
                Err(error) => return error_response(id, error),
            };
            let (reply, rx) = mpsc::channel();
            (
                ControlCommand::FocusSurface {
                    target,
                    surface_hint,
                    reply,
                },
                rx,
            )
        }
        "pane.focus" | "focus-pane" => {
            let target = match parse_lifecycle_workspace_target(params) {
                Ok(target) => target,
                Err(error) => return error_response(id, error),
            };
            let pane_hint = match optional_explicit_handle(params, &["pane_id"], "pane:") {
                Ok(handle) => handle,
                Err(error) => return error_response(id, error),
            };
            let (reply, rx) = mpsc::channel();
            (
                ControlCommand::FocusPane {
                    target,
                    pane_hint,
                    reply,
                },
                rx,
            )
        }
        "tab.action" | "tab-action" => {
            let Some(action) = optional_string(params, &["action"]) else {
                return error_response(
                    id,
                    BridgeError::invalid_params("tab.action requires action"),
                );
            };
            let target = match parse_lifecycle_workspace_target(params) {
                Ok(target) => target,
                Err(error) => return error_response(id, error),
            };
            let surface_hint = match optional_tab_handle(params) {
                Ok(handle) => handle,
                Err(error) => return error_response(id, error),
            };
            let title = match params.get("title") {
                None => None,
                Some(Value::String(title)) if !title.contains('\0') => Some(title.clone()),
                Some(_) => {
                    return error_response(
                        id,
                        BridgeError::invalid_params(
                            "title must be a string without NUL characters",
                        ),
                    )
                }
            };
            let (reply, rx) = mpsc::channel();
            (
                ControlCommand::TabAction {
                    target,
                    surface_hint,
                    action,
                    title,
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
            let surface_hint = match optional_surface_handle(params, &["surface_id"]) {
                Ok(surface_hint) => surface_hint,
                Err(error) => return error_response(id, error),
            };
            let (reply, rx) = mpsc::channel();
            (
                ControlCommand::SendKey {
                    target,
                    surface_hint,
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
                    surface_hint: optional_string(params, &["surface_id", "tab_id"]),
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

        let response = match std::str::from_utf8(&line_buf) {
            Ok(input) => {
                let input = input.trim_end_matches(['\n', '\r']);
                if input.is_empty() {
                    continue;
                }
                dispatch_request(input, dispatch)
            }
            Err(error) => error_response(
                None,
                BridgeError::new(
                    PARSE_ERROR_CODE,
                    format!("invalid request payload: {error}"),
                ),
            ),
        };
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
            .try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
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
                                    if !matches!(
                                        error.kind(),
                                        io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
                                    ) {
                                        eprintln!(
                                            "limux: control connection error for pid={} uid={}: {error}",
                                            peer.pid, peer.uid
                                        );
                                    }
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
    use std::io::BufRead;

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
    fn window_activate_queues_presentation_with_an_optional_opaque_token() {
        for (params, expected) in [
            (json!({}), None),
            (json!({ "activation_token": null }), None),
            (json!({ "activation_token": "" }), None),
            (
                json!({ "activation_token": " opaque token " }),
                Some(" opaque token "),
            ),
        ] {
            let response = dispatch_request(
                &json!({ "id": "activate", "method": "window.activate", "params": params })
                    .to_string(),
                &|command| match command {
                    ControlCommand::ActivateWindow {
                        activation_token,
                        reply,
                    } => {
                        assert_eq!(activation_token.as_deref(), expected);
                        reply.send(Ok(json!({ "presented": true }))).unwrap();
                    }
                    other => panic!("unexpected command: {other:?}"),
                },
            );
            assert_eq!(response.error, None);
            assert_eq!(response.id, Some(json!("activate")));
            assert_eq!(response.result, Some(json!({ "presented": true })));
        }
    }

    #[test]
    fn window_activate_rejects_invalid_tokens_before_gtk_dispatch() {
        for token in [json!(42), json!(true), json!({}), json!("token\0suffix")] {
            let response = dispatch_request(
                &json!({ "method": "window.activate", "params": { "activation_token": token } })
                    .to_string(),
                &|command| panic!("invalid activation token should not dispatch: {command:?}"),
            );
            assert_eq!(
                response.error.as_ref().map(|error| error.code),
                Some(INVALID_PARAMS_CODE)
            );
        }
    }

    #[test]
    fn send_text_preserves_whitespace_for_every_alias() {
        for method in ["surface.send_text", "send-text", "send"] {
            for text in ["  alpha\nbeta\t \n", "\n\t  ", "  λ 🦀 \n"] {
                let response = dispatch_request(
                    &json!({ "id": 1, "method": method, "params": { "text": text } }).to_string(),
                    &|command| match command {
                        ControlCommand::SendText {
                            text: actual,
                            reply,
                            ..
                        } => {
                            assert_eq!(actual, text);
                            reply.send(Ok(json!({}))).unwrap();
                        }
                        other => panic!("unexpected command: {other:?}"),
                    },
                );
                assert_eq!(response.error, None);
            }
        }
    }

    #[test]
    fn send_text_rejects_missing_empty_and_non_string_text() {
        for params in [json!({}), json!({ "text": "" }), json!({ "text": 1 })] {
            let response = dispatch_request(
                &json!({ "method": "surface.send_text", "params": params }).to_string(),
                &|command| panic!("invalid text should not dispatch: {command:?}"),
            );
            assert_eq!(
                response.error.as_ref().map(|error| error.code),
                Some(INVALID_PARAMS_CODE)
            );
        }
    }

    #[test]
    fn invalid_utf8_returns_parse_error_and_keeps_connection_open() {
        let (client, server) = UnixStream::pair().expect("socket pair should open");
        let server_task = std::thread::spawn(move || {
            handle_client(server, &|command| {
                panic!("ping should not dispatch a GTK command: {command:?}")
            })
        });

        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .expect("read timeout should set");
        let reader_stream = client.try_clone().expect("client should clone");
        let mut reader = io::BufReader::new(reader_stream);
        let mut writer = client;

        writer
            .write_all(b"\xff\n{\"id\":\"after-error\",\"method\":\"system.ping\",\"params\":{}}\n")
            .expect("requests should write");
        writer.flush().expect("requests should flush");

        let mut response_line = String::new();
        reader
            .read_line(&mut response_line)
            .expect("parse error should read");
        let response: Value =
            serde_json::from_str(response_line.trim()).expect("response should be valid json");
        assert_eq!(response["ok"], false);
        assert_eq!(response["error"]["code"], PARSE_ERROR_CODE);

        response_line.clear();
        reader
            .read_line(&mut response_line)
            .expect("ping response should read");
        let response: Value =
            serde_json::from_str(response_line.trim()).expect("response should be valid json");
        assert_eq!(response["id"], "after-error");
        assert_eq!(response["result"]["pong"], true);

        drop(reader);
        drop(writer);
        server_task
            .join()
            .expect("server thread should join")
            .expect("server should stop at EOF");
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
    fn lifecycle_targets_preserve_numeric_and_prefixed_handles() {
        for (method, field) in [
            ("surface.create", "pane_id"),
            ("new-surface", "pane_id"),
            ("pane.focus", "pane_id"),
            ("focus-pane", "pane_id"),
            ("surface.close", "surface_id"),
            ("close-surface", "surface_id"),
            ("surface.focus", "surface_id"),
            ("focus-surface", "surface_id"),
            ("tab.action", "tab_id"),
            ("tab-action", "surface_id"),
        ] {
            for handle in [
                json!(12),
                json!(if field == "pane_id" {
                    "pane:12"
                } else {
                    "surface:12"
                }),
            ] {
                let mut params = json!({"workspace_id": 7, "action": "focus"});
                params[field] = handle;
                let response = dispatch_request(
                    &json!({"method": method, "params": params}).to_string(),
                    &|command| {
                        let (target, handle, reply) = match command {
                            ControlCommand::CreateSurface {
                                target,
                                pane_hint,
                                reply,
                                ..
                            }
                            | ControlCommand::FocusPane {
                                target,
                                pane_hint,
                                reply,
                            } => (target, pane_hint, reply),
                            ControlCommand::CloseSurface {
                                target,
                                surface_hint,
                                reply,
                            }
                            | ControlCommand::FocusSurface {
                                target,
                                surface_hint,
                                reply,
                            }
                            | ControlCommand::TabAction {
                                target,
                                surface_hint,
                                reply,
                                ..
                            } => (target, surface_hint, reply),
                            other => panic!("unexpected command: {other:?}"),
                        };
                        assert_eq!(target, WorkspaceTarget::Handle("7".to_string()));
                        assert_eq!(handle.as_deref(), Some("12"));
                        reply.send(Ok(json!({}))).unwrap();
                    },
                );
                assert_eq!(response.error, None, "{method}");
            }
        }
    }

    #[test]
    fn lifecycle_rejects_malformed_explicit_targets_before_dispatch() {
        for (method, field) in [
            ("surface.create", "pane_id"),
            ("pane.focus", "pane_id"),
            ("surface.close", "surface_id"),
            ("surface.focus", "surface_id"),
            ("tab.action", "surface_id"),
            ("tab.action", "tab_id"),
        ] {
            for key in [field, "workspace_id", "name"] {
                for invalid in [
                    Value::Null,
                    json!(""),
                    json!(" "),
                    json!([]),
                    json!({}),
                    json!(false),
                    json!(-1),
                    json!(1.5),
                ] {
                    let mut params = json!({"action": "close"});
                    params[key] = invalid;
                    let response = dispatch_request(
                        &json!({"method": method, "params": params}).to_string(),
                        &|command| panic!("invalid target reached GTK: {command:?}"),
                    );
                    assert_eq!(
                        response.error.map(|error| error.code),
                        Some(INVALID_PARAMS_CODE),
                        "{method}: {params}"
                    );
                }
            }
        }
        for params in [
            json!({"surface_id": "surface:", "tab_id": "valid"}),
            json!({"surface_id": "valid", "tab_id": []}),
            json!({"surface_id": "valid", "tab_id": "tab:"}),
        ] {
            let mut params = params;
            params["action"] = json!("close");
            let response = dispatch_request(
                &json!({"method": "tab.action", "params": params}).to_string(),
                &|command| panic!("invalid alias reached GTK: {command:?}"),
            );
            assert_eq!(
                response.error.map(|error| error.code),
                Some(INVALID_PARAMS_CODE)
            );
        }
    }

    #[test]
    fn tab_action_preserves_tab_refs_and_empty_titles_but_rejects_nul() {
        for title in [
            json!(""),
            json!("custom"),
            json!("bad\u{0}title"),
            json!([]),
            Value::Null,
        ] {
            let valid = title.as_str().is_some_and(|title| !title.contains('\0'));
            let response = dispatch_request(
                &json!({"method": "tab.action", "params": {"action": "rename", "tab_id": "tab:tab-uuid", "title": title}}).to_string(),
                &|command| {
                    assert!(valid, "invalid title reached GTK");
                    match command {
                        ControlCommand::TabAction { surface_hint, title: actual, reply, .. } => {
                            assert_eq!(surface_hint.as_deref(), Some("tab-uuid"));
                            assert_eq!(actual.as_deref(), title.as_str());
                            reply.send(Ok(json!({}))).unwrap();
                        }
                        other => panic!("unexpected command: {other:?}"),
                    }
                },
            );
            assert_eq!(
                response.error.map(|error| error.code),
                (!valid).then_some(INVALID_PARAMS_CODE)
            );
        }
    }

    #[test]
    fn surface_create_validates_kind_and_url_before_dispatch() {
        for params in [
            json!({"type": "unknown"}),
            json!({"kind": []}),
            json!({"type": null}),
            json!({"type": "terminal", "kind": "browser"}),
            json!({"url": []}),
            json!({"url": ""}),
            json!({"url": "not a URL"}),
            json!({"url": "https://"}),
            json!({"url": "https://example.com/\u{0}"}),
            json!({"url": "https://example.com/a\nb"}),
            json!({"type": "terminal", "url": "about:blank"}),
        ] {
            let response = dispatch_request(
                &json!({"method": "surface.create", "params": params}).to_string(),
                &|command| panic!("invalid surface creation reached GTK: {command:?}"),
            );
            assert_eq!(
                response.error.map(|error| error.code),
                Some(INVALID_PARAMS_CODE),
                "{params}"
            );
        }
        for (params, expected_browser, expected_url) in [
            (json!({}), false, None),
            (json!({"type": "browser"}), true, None),
            (json!({"url": "about:blank"}), true, Some("about:blank")),
            (
                json!({"kind": "browser", "url": "https://example.com/"}),
                true,
                Some("https://example.com/"),
            ),
        ] {
            let response = dispatch_request(
                &json!({"method": "surface.create", "params": params}).to_string(),
                &|command| match command {
                    ControlCommand::CreateSurface {
                        browser,
                        url,
                        reply,
                        ..
                    } => {
                        assert_eq!(browser, expected_browser);
                        assert_eq!(url.as_deref(), expected_url);
                        reply.send(Ok(json!({}))).unwrap();
                    }
                    other => panic!("unexpected command: {other:?}"),
                },
            );
            assert_eq!(response.error, None);
        }
    }

    #[test]
    fn workspace_set_color_parses_names_and_clears() {
        for (color, expected) in [
            (json!("blue"), Some(WorkspaceColor::Blue)),
            (json!(" Pink "), Some(WorkspaceColor::Pink)),
            (json!("none"), None),
            (Value::Null, None),
        ] {
            let response = dispatch_request(
                &json!({"method": "workspace.set_color", "params": {"color": color}}).to_string(),
                &|command| match command {
                    ControlCommand::SetWorkspaceColor {
                        target,
                        color,
                        reply,
                    } => {
                        assert_eq!(target, WorkspaceTarget::Active);
                        assert_eq!(color, expected);
                        reply.send(Ok(json!({}))).unwrap();
                    }
                    other => panic!("unexpected command: {other:?}"),
                },
            );
            assert_eq!(response.error, None);
        }
    }

    #[test]
    fn workspace_set_color_rejects_missing_and_unknown_colors() {
        for params in [
            json!({}),
            json!({"color": "chartreuse"}),
            json!({"color": 3}),
        ] {
            let response = dispatch_request(
                &json!({"method": "workspace.set_color", "params": params}).to_string(),
                &|command| panic!("unexpected command: {command:?}"),
            );
            let error = response.error.expect("invalid color is rejected");
            assert_eq!(error.code, INVALID_PARAMS_CODE);
        }
    }

    #[test]
    fn tab_action_requires_an_action() {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tab.action",
            "params": { "surface_id": "3:terminal-0" },
        })
        .to_string();

        let response = dispatch_request(&request, &|_command| {
            panic!("tab.action must be rejected before it reaches the GTK loop");
        });

        let error = response
            .error
            .expect("tab.action without --action must error");
        assert_eq!(error.code, INVALID_PARAMS_CODE);
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

    #[test]
    fn surface_health_route_accepts_surface_refs() {
        let response = dispatch_request(
            r#"{"id":1,"method":"surface.health","params":{"workspace_id":"codex","surface_id":"surface:4:tab"}}"#,
            &|command| match command {
                ControlCommand::SurfaceHealth {
                    target,
                    surface_hint,
                    reply,
                } => {
                    assert_eq!(target, WorkspaceTarget::Name("codex".to_string()));
                    assert_eq!(surface_hint, Some("4:tab".to_string()));
                    let _ = reply.send(Ok(json!({ "surfaces": [] })));
                }
                other => panic!("unexpected command: {other:?}"),
            },
        );

        assert_eq!(response.error, None);
        assert!(response.result.is_some());
    }

    #[test]
    fn terminal_routes_reject_empty_explicit_targets_before_dispatch() {
        for method in [
            "surface.send_text",
            "send-text",
            "send",
            "surface.send_key",
            "send-key",
            "surface.read_text",
            "read-screen",
            "capture-pane",
        ] {
            for target in ["", "   ", "surface:", " surface:   "] {
                let request = json!({ "id": 1, "method": method, "params": { "surface_id": target, "text": "must not be sent", "key": "Enter" } });
                let response = dispatch_request(&request.to_string(), &|command| {
                    panic!("invalid target dispatched: {command:?}")
                });
                assert_eq!(
                    response.error.as_ref().map(|error| error.code),
                    Some(INVALID_PARAMS_CODE),
                    "{method} target {target:?}"
                );
            }
        }
        let response = dispatch_request(
            r#"{"id":1,"method":"capture-pane","params":{"id":"surface:"}}"#,
            &|command| panic!("empty alias dispatched: {command:?}"),
        );
        assert_eq!(
            response.error.as_ref().map(|error| error.code),
            Some(INVALID_PARAMS_CODE)
        );
    }

    #[test]
    fn terminal_routes_preserve_omitted_and_nonempty_explicit_targets() {
        for method in ["surface.send_text", "surface.send_key", "surface.read_text"] {
            for (target, expected) in [
                (None, None),
                (Some("surface:4:target"), Some("4:target")),
                (Some("missing-surface"), Some("missing-surface")),
            ] {
                let mut params = json!({ "text": "test", "key": "Enter" });
                if let Some(target) = target {
                    params["surface_id"] = json!(target);
                }
                let request = json!({ "id": 1, "method": method, "params": params });
                let response = dispatch_request(&request.to_string(), &|command| {
                    let (surface_hint, reply) = match command {
                        ControlCommand::SendText {
                            surface_hint,
                            reply,
                            ..
                        }
                        | ControlCommand::SendKey {
                            surface_hint,
                            reply,
                            ..
                        }
                        | ControlCommand::ReadSurfaceText {
                            surface_hint,
                            reply,
                            ..
                        } => (surface_hint, reply),
                        other => panic!("unexpected command: {other:?}"),
                    };
                    assert_eq!(surface_hint.as_deref(), expected, "{method}");
                    let _ = reply.send(Ok(json!({})));
                });
                assert_eq!(response.error, None);
            }
        }
    }

    #[test]
    fn read_text_route_accepts_capture_alias_and_surface_refs() {
        let response = dispatch_request(
            r#"{"id":1,"method":"capture-pane","params":{"surface_id":"surface:9:tab"}}"#,
            &|command| match command {
                ControlCommand::ReadSurfaceText {
                    target,
                    surface_hint,
                    reply,
                } => {
                    assert_eq!(target, WorkspaceTarget::Active);
                    assert_eq!(surface_hint, Some("9:tab".to_string()));
                    let _ = reply.send(Ok(json!({ "text": "ready" })));
                }
                other => panic!("unexpected command: {other:?}"),
            },
        );

        assert_eq!(response.error, None);
        assert_eq!(response.result.expect("result")["text"], "ready");
    }

    #[test]
    fn notification_route_preserves_surface_target() {
        let response = dispatch_request(
            r#"{"id":1,"method":"notification.create","params":{"workspace_id":"codex","surface_id":"surface:9:tab","title":"Done"}}"#,
            &|command| match command {
                ControlCommand::CreateNotification {
                    target,
                    surface_hint,
                    title,
                    reply,
                    ..
                } => {
                    assert_eq!(target, WorkspaceTarget::Name("codex".to_string()));
                    assert_eq!(surface_hint, Some("surface:9:tab".to_string()));
                    assert_eq!(title, "Done");
                    let _ = reply.send(Ok(json!({ "ok": true })));
                }
                other => panic!("unexpected command: {other:?}"),
            },
        );

        assert_eq!(response.error, None);
        assert_eq!(response.result.expect("result")["ok"], true);
    }
}
