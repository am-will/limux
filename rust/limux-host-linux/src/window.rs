use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use gtk::gdk::prelude::ToplevelExt;
use gtk::gio;
use gtk::glib;
use gtk::glib::variant::ToVariant;
use gtk4 as gtk;
use libadwaita as adw;

use crate::app_config;
use crate::control_bridge::{
    BridgeError, ControlCommand, PaneCreateDirection as BridgePaneCreateDirection, PaneCreateType,
    WorkspaceTarget,
};
use crate::keybind_editor;
use crate::layout_state::{
    self, AppSessionState, LayoutNodeState, LoadedSession, PaneState, WorkspaceState,
};
use crate::pane::{self, PaneCallbacks};
use crate::shortcut_config::{
    self, EditableCapturePolicy, ResolvedShortcutConfig, ShortcutCommand, ShortcutId,
};
use crate::split_tree::{self, SplitTreeContainer};

const PANE_CREATE_COMMAND_READY_INTERVAL_MS: u64 = 50;
const PANE_CREATE_COMMAND_READY_ATTEMPTS: u32 = 40;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct Workspace {
    id: String,
    name: String,
    /// The root widget in the content stack for this workspace.
    root: gtk::Widget,
    /// Manages the split tree data model and async widget rebuild.
    split_container: Rc<SplitTreeContainer>,
    /// The sidebar row widget.
    sidebar_row: gtk::ListBoxRow,
    /// Name label in sidebar row.
    name_label: gtk::Label,
    /// Favorite star button in sidebar row.
    favorite_button: gtk::Button,
    /// Notification dot in the sidebar row.
    notify_dot: gtk::Label,
    /// Notification message label in the sidebar row.
    notify_label: gtk::Label,
    /// Unread state for notifications without a tab target.
    unread: bool,
    /// Whether this workspace is favorited/pinned to top.
    favorite: bool,
    /// Last known working directory from the terminal (via OSC 7).
    cwd: Rc<RefCell<Option<String>>>,
    /// The folder path this workspace was opened with.
    folder_path: Option<String>,
    /// Path label shown below workspace name in sidebar.
    path_label: gtk::Label,
}

pub(crate) struct AppState {
    app: adw::Application,
    window: adw::ApplicationWindow,
    top_bar: Option<adw::HeaderBar>,
    top_bar_visible: bool,
    config: Rc<RefCell<app_config::AppConfig>>,
    css_provider: gtk::CssProvider,
    system_prefers_dark: Rc<Cell<Option<bool>>>,
    workspaces: Vec<Workspace>,
    active_idx: usize,
    shortcuts: Rc<ResolvedShortcutConfig>,
    stack: gtk::Stack,
    sidebar_list: gtk::ListBox,
    sidebar_shell: gtk::Box,
    sidebar_handle: gtk::Box,
    new_ws_btn: gtk::Button,
    sidebar_animation: Option<adw::TimedAnimation>,
    sidebar_animation_epoch: u64,
    sidebar_expanded_width: i32,
    persistence_suspended: bool,
    save_queued: bool,
    workspace_dragging: Option<String>,
    desktop_notification_routes: HashMap<u32, DesktopNotificationRoute>,
    _theme_portal_signal: Option<gio::SignalSubscription>,
    _theme_gnome_settings: Option<gio::Settings>,
    _theme_gnome_signal: Option<glib::SignalHandlerId>,
    _desktop_notification_token_signal: Option<gio::SignalSubscription>,
    _desktop_notification_action_signal: Option<gio::SignalSubscription>,
    _desktop_notification_closed_signal: Option<gio::SignalSubscription>,
}

impl AppState {
    fn active_workspace(&self) -> Option<&Workspace> {
        self.workspaces.get(self.active_idx)
    }

    fn workspace_for_widget(&self, widget: &gtk::Widget) -> Option<&Workspace> {
        self.workspaces
            .iter()
            .find(|workspace| widget.is_ancestor(&workspace.root))
    }
}

fn workspace_ref(id: &str) -> String {
    format!("workspace:{id}")
}

fn pane_ref(id: u32) -> String {
    format!("pane:{id}")
}

fn surface_ref(id: &str) -> String {
    format!("surface:{id}")
}

fn pane_create_response_payload(
    workspace_id: &str,
    workspace_name: &str,
    surface: pane::SurfaceSummary,
) -> serde_json::Value {
    let surface_id = surface.surface_id;
    serde_json::json!({
        "workspace_id": workspace_id,
        "workspace_ref": workspace_ref(workspace_id),
        "workspace": {
            "id": workspace_id,
            "ref": workspace_ref(workspace_id),
            "workspace_id": workspace_id,
            "workspace_ref": workspace_ref(workspace_id),
            "title": workspace_name,
            "name": workspace_name,
        },
        "title": workspace_name,
        "name": workspace_name,
        "pane_id": surface.pane_id.to_string(),
        "pane_ref": pane_ref(surface.pane_id),
        "surface_id": surface_id.clone(),
        "surface_ref": surface_ref(&surface_id),
        "surface_title": surface.title,
        "surface_type": surface.kind,
        "ok": true,
    })
}

fn send_pane_create_response_after_command(
    pane_widget: gtk::Widget,
    surface_id: String,
    command: String,
    response: serde_json::Value,
    reply: std::sync::mpsc::Sender<Result<serde_json::Value, BridgeError>>,
) {
    let mut attempts = 0;
    let mut command_sent = false;
    let mut reply = Some(reply);

    glib::timeout_add_local(
        std::time::Duration::from_millis(PANE_CREATE_COMMAND_READY_INTERVAL_MS),
        move || {
            attempts += 1;

            if let Some((matched_surface_id, handle)) =
                pane::exact_terminal_handle_for_surface(&pane_widget, &surface_id)
            {
                if matched_surface_id == surface_id && !command_sent {
                    command_sent = handle.send_text(&command);
                }
                if command_sent && handle.send_key("Enter") {
                    if let Some(reply) = reply.take() {
                        let _ = reply.send(Ok(response.clone()));
                    }
                    return glib::ControlFlow::Break;
                }
            }

            if attempts >= PANE_CREATE_COMMAND_READY_ATTEMPTS {
                if let Some(reply) = reply.take() {
                    let _ = reply.send(Err(BridgeError::internal(format!(
                        "pane.create command target surface {surface_id} never became writable"
                    ))));
                }
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        },
    );
}

fn normalize_workspace_handle(raw: &str) -> &str {
    raw.trim()
        .strip_prefix("workspace:")
        .unwrap_or_else(|| raw.trim())
}

fn normalize_pane_handle(raw: &str) -> &str {
    raw.trim()
        .strip_prefix("pane:")
        .unwrap_or_else(|| raw.trim())
}

fn parse_pane_handle(raw: &str) -> Option<u32> {
    normalize_pane_handle(raw).parse::<u32>().ok()
}

fn workspace_index_for_target(state: &AppState, target: &WorkspaceTarget) -> Option<usize> {
    match target {
        WorkspaceTarget::Active => (!state.workspaces.is_empty()).then_some(state.active_idx),
        WorkspaceTarget::Handle(handle) => {
            let normalized = normalize_workspace_handle(handle);
            state
                .workspaces
                .iter()
                .position(|workspace| workspace.id == normalized)
        }
        WorkspaceTarget::Name(name) => state
            .workspaces
            .iter()
            .position(|workspace| workspace.name == *name),
        WorkspaceTarget::Index(index) => (*index < state.workspaces.len()).then_some(*index),
    }
}

fn workspace_row(index: usize, selected_idx: usize, workspace: &Workspace) -> serde_json::Value {
    let cwd = workspace.cwd.borrow().clone().unwrap_or_default();
    serde_json::json!({
        "index": index,
        "id": workspace.id.as_str(),
        "ref": workspace_ref(&workspace.id),
        "workspace_id": workspace.id.as_str(),
        "workspace_ref": workspace_ref(&workspace.id),
        "title": workspace.name.as_str(),
        "name": workspace.name.as_str(),
        "selected": index == selected_idx,
        "focused": index == selected_idx,
        "cwd": cwd,
    })
}

fn workspace_payload(state: &AppState, index: usize) -> Option<serde_json::Value> {
    let workspace = state.workspaces.get(index)?;
    Some(serde_json::json!({
        "workspace_id": workspace.id.as_str(),
        "workspace_ref": workspace_ref(&workspace.id),
        "workspace": workspace_row(index, state.active_idx, workspace),
        "title": workspace.name.as_str(),
        "name": workspace.name.as_str(),
    }))
}

fn focused_surface_payload(state: &State) -> Option<serde_json::Value> {
    let (workspace_id, workspace_name, pane_widget) = {
        let app_state = state.borrow();
        let workspace = app_state.active_workspace()?;
        let pane_widget = find_focused_pane(state).map(|(_, pane_widget)| pane_widget)?;
        (workspace.id.clone(), workspace.name.clone(), pane_widget)
    };
    let surface = pane::active_surface_summary(&pane_widget)?;
    let mut payload = serde_json::Map::new();
    payload.insert(
        "workspace_id".to_string(),
        serde_json::Value::String(workspace_id.clone()),
    );
    payload.insert(
        "workspace_ref".to_string(),
        serde_json::Value::String(workspace_ref(&workspace_id)),
    );
    payload.insert(
        "title".to_string(),
        serde_json::Value::String(workspace_name.clone()),
    );
    payload.insert(
        "name".to_string(),
        serde_json::Value::String(workspace_name),
    );
    payload.insert(
        "pane_id".to_string(),
        serde_json::Value::String(surface.pane_id.to_string()),
    );
    payload.insert(
        "pane_ref".to_string(),
        serde_json::Value::String(pane_ref(surface.pane_id)),
    );
    payload.insert(
        "surface_id".to_string(),
        serde_json::Value::String(surface.surface_id.clone()),
    );
    payload.insert(
        "surface_ref".to_string(),
        serde_json::Value::String(surface_ref(&surface.surface_id)),
    );
    if !surface.title.is_empty() {
        payload.insert(
            "surface_title".to_string(),
            serde_json::Value::String(surface.title),
        );
    }
    payload.insert(
        "surface_type".to_string(),
        serde_json::Value::String(surface.kind),
    );
    if let Some(cwd) = surface.cwd.filter(|cwd| !cwd.is_empty()) {
        payload.insert("cwd".to_string(), serde_json::Value::String(cwd));
    }
    if let Some(uri) = surface.uri.filter(|uri| !uri.is_empty()) {
        payload.insert("uri".to_string(), serde_json::Value::String(uri));
    }
    Some(serde_json::Value::Object(payload))
}

fn focused_ids_for_workspace(state: &State, workspace_id: &str) -> (Option<u32>, Option<String>) {
    let is_active = {
        let app_state = state.borrow();
        app_state
            .active_workspace()
            .map(|workspace| workspace.id == workspace_id)
            .unwrap_or(false)
    };
    if !is_active {
        return (None, None);
    }

    let Some((_focused_workspace_id, pane_widget)) = find_focused_pane(state) else {
        return (None, None);
    };
    let Some(surface) = pane::active_surface_summary(&pane_widget) else {
        return (None, None);
    };
    (Some(surface.pane_id), Some(surface.surface_id))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub(crate) enum PaneCreateDirection {
    Left,
    Right,
    Up,
    Down,
}

impl PaneCreateDirection {
    #[allow(dead_code)]
    pub(crate) fn from_str(raw: &str) -> Option<Self> {
        match raw {
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            _ => None,
        }
    }
}

impl From<BridgePaneCreateDirection> for PaneCreateDirection {
    fn from(direction: BridgePaneCreateDirection) -> Self {
        match direction {
            BridgePaneCreateDirection::Left => Self::Left,
            BridgePaneCreateDirection::Right => Self::Right,
            BridgePaneCreateDirection::Up => Self::Up,
            BridgePaneCreateDirection::Down => Self::Down,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PaneCreateSplitPlacement {
    pub(crate) orientation: gtk::Orientation,
    pub(crate) new_pane_first: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub(crate) enum PaneCreateTargetError {
    WorkspaceNotFound,
    InvalidSurfaceId(String),
    InvalidPaneId(u32),
    NoPanes,
}

#[allow(dead_code)]
pub(crate) struct ResolvedPaneCreateTarget {
    pub(crate) workspace_id: String,
    pub(crate) pane_id: u32,
    pub(crate) pane_widget: gtk::Widget,
    pub(crate) placement: PaneCreateSplitPlacement,
}

fn pane_create_split_placement(direction: PaneCreateDirection) -> PaneCreateSplitPlacement {
    match direction {
        PaneCreateDirection::Left => PaneCreateSplitPlacement {
            orientation: gtk::Orientation::Horizontal,
            new_pane_first: true,
        },
        PaneCreateDirection::Right => PaneCreateSplitPlacement {
            orientation: gtk::Orientation::Horizontal,
            new_pane_first: false,
        },
        PaneCreateDirection::Up => PaneCreateSplitPlacement {
            orientation: gtk::Orientation::Vertical,
            new_pane_first: true,
        },
        PaneCreateDirection::Down => PaneCreateSplitPlacement {
            orientation: gtk::Orientation::Vertical,
            new_pane_first: false,
        },
    }
}

fn normalize_surface_handle(raw: &str) -> &str {
    raw.trim()
        .strip_prefix("surface:")
        .unwrap_or_else(|| raw.trim())
}

fn resolve_pane_create_source_id(
    surface_id: Option<&str>,
    pane_id: Option<u32>,
    focused_pane_id: Option<u32>,
    target_workspace_is_active: bool,
    pane_ids: &[u32],
    surface_to_pane: &[(&str, u32)],
) -> Result<u32, PaneCreateTargetError> {
    if pane_ids.is_empty() {
        return Err(PaneCreateTargetError::NoPanes);
    }

    if let Some(surface_id) = surface_id {
        let requested = normalize_surface_handle(surface_id);
        return surface_to_pane
            .iter()
            .find(|(known_surface_id, _)| *known_surface_id == requested)
            .map(|(_, pane_id)| *pane_id)
            .ok_or_else(|| PaneCreateTargetError::InvalidSurfaceId(surface_id.to_string()));
    }

    if let Some(pane_id) = pane_id {
        if pane_ids.contains(&pane_id) {
            return Ok(pane_id);
        }
        return Err(PaneCreateTargetError::InvalidPaneId(pane_id));
    }

    if target_workspace_is_active {
        if let Some(focused_pane_id) = focused_pane_id {
            if pane_ids.contains(&focused_pane_id) {
                return Ok(focused_pane_id);
            }
        }
    }

    pane_ids
        .first()
        .copied()
        .ok_or(PaneCreateTargetError::NoPanes)
}

fn pane_create_target_error(error: PaneCreateTargetError) -> BridgeError {
    match error {
        PaneCreateTargetError::WorkspaceNotFound => BridgeError::not_found("workspace not found"),
        PaneCreateTargetError::InvalidSurfaceId(_) => BridgeError::not_found("surface not found"),
        PaneCreateTargetError::InvalidPaneId(_) => BridgeError::not_found("pane not found"),
        PaneCreateTargetError::NoPanes => BridgeError::not_found("pane not found"),
    }
}

#[allow(dead_code)]
pub(crate) fn resolve_pane_create_target(
    state: &State,
    target: &WorkspaceTarget,
    surface_id: Option<&str>,
    pane_id: Option<u32>,
    direction: PaneCreateDirection,
) -> Result<ResolvedPaneCreateTarget, PaneCreateTargetError> {
    let (workspace_id, workspace_root, target_workspace_is_active) = {
        let app_state = state.borrow();
        let workspace_index = workspace_index_for_target(&app_state, target)
            .ok_or(PaneCreateTargetError::WorkspaceNotFound)?;
        let workspace = &app_state.workspaces[workspace_index];
        (
            workspace.id.clone(),
            workspace.root.clone(),
            workspace_index == app_state.active_idx,
        )
    };

    let pane_summaries = pane::pane_summaries_for_root(&workspace_root);
    let pane_ids = pane_summaries
        .iter()
        .map(|summary| summary.pane_id)
        .collect::<Vec<_>>();
    let surface_summaries = pane::surface_summaries_for_root(&workspace_root);
    let surface_to_pane = surface_summaries
        .iter()
        .map(|surface| (surface.surface_id.as_str(), surface.pane_id))
        .collect::<Vec<_>>();
    let focused_pane_id = target_workspace_is_active
        .then(|| focused_ids_for_workspace(state, &workspace_id).0)
        .flatten();

    let pane_id = resolve_pane_create_source_id(
        surface_id,
        pane_id,
        focused_pane_id,
        target_workspace_is_active,
        &pane_ids,
        &surface_to_pane,
    )?;
    let pane_widget = pane::pane_widget_for_root(&workspace_root, pane_id)
        .ok_or(PaneCreateTargetError::InvalidPaneId(pane_id))?;

    Ok(ResolvedPaneCreateTarget {
        workspace_id,
        pane_id,
        pane_widget,
        placement: pane_create_split_placement(direction),
    })
}

fn pane_list_payload(state: &State, workspace: &Workspace) -> serde_json::Value {
    let (focused_pane_id, _) = focused_ids_for_workspace(state, &workspace.id);
    let panes = pane::pane_summaries_for_root(&workspace.root)
        .into_iter()
        .enumerate()
        .map(|(index, pane)| {
            let mut row = serde_json::Map::new();
            row.insert(
                "pane_id".to_string(),
                serde_json::Value::String(pane.pane_id.to_string()),
            );
            row.insert(
                "pane_ref".to_string(),
                serde_json::Value::String(pane_ref(pane.pane_id)),
            );
            row.insert("index".to_string(), serde_json::json!(index));
            row.insert(
                "surface_count".to_string(),
                serde_json::json!(pane.surface_count),
            );
            let focused = focused_pane_id == Some(pane.pane_id);
            row.insert("focused".to_string(), serde_json::Value::Bool(focused));
            row.insert("selected".to_string(), serde_json::Value::Bool(focused));
            if let Some(surface_id) = pane.active_surface_id {
                row.insert(
                    "surface_id".to_string(),
                    serde_json::Value::String(surface_id.clone()),
                );
                row.insert(
                    "surface_ref".to_string(),
                    serde_json::Value::String(surface_ref(&surface_id)),
                );
            }
            serde_json::Value::Object(row)
        })
        .collect::<Vec<_>>();
    serde_json::json!({ "panes": panes })
}

fn surface_list_payload(
    state: &State,
    workspace: &Workspace,
    pane_filter: Option<u32>,
) -> serde_json::Value {
    let (_, focused_surface_id) = focused_ids_for_workspace(state, &workspace.id);
    let surfaces = pane::surface_summaries_for_root(&workspace.root)
        .into_iter()
        .filter(|surface| pane_filter.is_none_or(|pane_id| surface.pane_id == pane_id))
        .enumerate()
        .map(|(index, surface)| {
            let mut row = serde_json::Map::new();
            row.insert(
                "surface_id".to_string(),
                serde_json::Value::String(surface.surface_id.clone()),
            );
            row.insert(
                "surface_ref".to_string(),
                serde_json::Value::String(surface_ref(&surface.surface_id)),
            );
            row.insert(
                "pane_id".to_string(),
                serde_json::Value::String(surface.pane_id.to_string()),
            );
            row.insert(
                "pane_ref".to_string(),
                serde_json::Value::String(pane_ref(surface.pane_id)),
            );
            row.insert("index".to_string(), serde_json::json!(index));
            row.insert(
                "title".to_string(),
                serde_json::Value::String(surface.title.clone()),
            );
            row.insert(
                "type".to_string(),
                serde_json::Value::String(surface.kind.clone()),
            );
            row.insert(
                "selected".to_string(),
                serde_json::Value::Bool(surface.selected),
            );
            row.insert(
                "focused".to_string(),
                serde_json::Value::Bool(
                    focused_surface_id.as_deref() == Some(surface.surface_id.as_str()),
                ),
            );
            if let Some(cwd) = surface.cwd.filter(|cwd| !cwd.is_empty()) {
                row.insert("cwd".to_string(), serde_json::Value::String(cwd));
            }
            if let Some(uri) = surface.uri.filter(|uri| !uri.is_empty()) {
                row.insert("uri".to_string(), serde_json::Value::String(uri));
            }
            serde_json::Value::Object(row)
        })
        .collect::<Vec<_>>();
    serde_json::json!({ "surfaces": surfaces })
}

fn surface_health_row(
    state: &State,
    workspace: &Workspace,
    index: usize,
    surface: pane::SurfaceSummary,
) -> serde_json::Value {
    let (_, focused_surface_id) = focused_ids_for_workspace(state, &workspace.id);
    let mut row = serde_json::Map::new();
    row.insert("index".to_string(), serde_json::json!(index));
    row.insert(
        "id".to_string(),
        serde_json::Value::String(surface.surface_id.clone()),
    );
    row.insert(
        "ref".to_string(),
        serde_json::Value::String(surface_ref(&surface.surface_id)),
    );
    row.insert(
        "surface_id".to_string(),
        serde_json::Value::String(surface.surface_id.clone()),
    );
    row.insert(
        "surface_ref".to_string(),
        serde_json::Value::String(surface_ref(&surface.surface_id)),
    );
    row.insert(
        "pane_id".to_string(),
        serde_json::Value::String(surface.pane_id.to_string()),
    );
    row.insert(
        "pane_ref".to_string(),
        serde_json::Value::String(pane_ref(surface.pane_id)),
    );
    row.insert(
        "type".to_string(),
        serde_json::Value::String(surface.kind.clone()),
    );
    let focused = focused_surface_id.as_deref() == Some(surface.surface_id.as_str());
    row.insert("focused".to_string(), serde_json::Value::Bool(focused));
    row.insert(
        "selected".to_string(),
        serde_json::Value::Bool(surface.selected),
    );
    row.insert("in_window".to_string(), serde_json::Value::Bool(true));
    row.insert("hidden".to_string(), serde_json::Value::Bool(false));

    if surface.kind == "terminal" {
        if let Some((_surface_id, handle)) =
            pane::terminal_handle_for_root(&workspace.root, Some(&surface.surface_id))
        {
            let health = handle.health();
            row.insert(
                "healthy".to_string(),
                serde_json::Value::Bool(health.realized && !health.process_exited),
            );
            row.insert(
                "realized".to_string(),
                serde_json::Value::Bool(health.realized),
            );
            row.insert(
                "process_exited".to_string(),
                serde_json::Value::Bool(health.process_exited),
            );
            row.insert("columns".to_string(), serde_json::json!(health.columns));
            row.insert("rows".to_string(), serde_json::json!(health.rows));
            row.insert("width_px".to_string(), serde_json::json!(health.width_px));
            row.insert("height_px".to_string(), serde_json::json!(health.height_px));
        } else {
            row.insert("healthy".to_string(), serde_json::Value::Bool(false));
            row.insert("realized".to_string(), serde_json::Value::Bool(false));
            row.insert("process_exited".to_string(), serde_json::Value::Bool(false));
        }
    } else {
        row.insert("healthy".to_string(), serde_json::Value::Bool(true));
        row.insert("realized".to_string(), serde_json::Value::Bool(true));
        row.insert("process_exited".to_string(), serde_json::Value::Bool(false));
    }

    serde_json::Value::Object(row)
}

fn surface_health_payload(
    state: &State,
    workspace: &Workspace,
    surface_hint: Option<&str>,
) -> Result<serde_json::Value, BridgeError> {
    let requested = surface_hint.map(normalize_surface_handle);
    let surfaces = pane::surface_summaries_for_root(&workspace.root)
        .into_iter()
        .filter(|surface| requested.is_none_or(|requested| surface.surface_id == requested))
        .enumerate()
        .map(|(index, surface)| surface_health_row(state, workspace, index, surface))
        .collect::<Vec<_>>();

    if surface_hint.is_some() && surfaces.is_empty() {
        return Err(BridgeError::not_found("surface not found"));
    }

    Ok(serde_json::json!({ "surfaces": surfaces }))
}

#[derive(Clone)]
struct WorkspaceSeedSource {
    workspace_cwd: Option<String>,
    workspace_folder_path: Option<String>,
}

#[derive(Clone)]
struct TabDragWorkspaceSeed {
    name: String,
    cwd: Option<String>,
    folder_path: Option<String>,
}

pub(crate) type State = Rc<RefCell<AppState>>;
thread_local! {
    static CONTROL_STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}
const SPLIT_RATIO_STATE_KEY: &str = "limux-split-ratio-state";
const PORTAL_DESKTOP_SERVICE: &str = "org.freedesktop.portal.Desktop";
const PORTAL_DESKTOP_PATH: &str = "/org/freedesktop/portal/desktop";
const PORTAL_SETTINGS_INTERFACE: &str = "org.freedesktop.portal.Settings";
const PORTAL_APPEARANCE_NAMESPACE: &str = "org.freedesktop.appearance";
const PORTAL_COLOR_SCHEME_KEY: &str = "color-scheme";
const FREEDESKTOP_NOTIFICATIONS_SERVICE: &str = "org.freedesktop.Notifications";
const FREEDESKTOP_NOTIFICATIONS_PATH: &str = "/org/freedesktop/Notifications";
const FREEDESKTOP_NOTIFICATIONS_INTERFACE: &str = "org.freedesktop.Notifications";
const GNOME_INTERFACE_SCHEMA: &str = "org.gnome.desktop.interface";
const GNOME_COLOR_SCHEME_KEY: &str = "color-scheme";
const DESKTOP_NOTIFICATION_DBUS_TIMEOUT_MS: i32 = 1_000;
const DESKTOP_NOTIFICATION_EXPIRE_TIMEOUT_MS: i32 = 10_000;
const PORTAL_THEME_READ_TIMEOUT_MS: i32 = 500;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum PortalColorSchemePreference {
    #[default]
    Unknown,
    Default,
    Dark,
    Light,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DesktopNotificationTarget {
    workspace_id: String,
    pane_id: Option<u32>,
    tab_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DesktopNotificationRoute {
    target: DesktopNotificationTarget,
    activation_token: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DesktopNotificationRequest {
    summary: String,
    body: String,
    sound: app_config::NotificationSound,
    target: DesktopNotificationTarget,
}

impl PortalColorSchemePreference {
    fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Default),
            1 => Some(Self::Dark),
            2 => Some(Self::Light),
            _ => None,
        }
    }

    fn resolved(self, gnome_prefers_dark: Option<bool>) -> Option<bool> {
        match self {
            Self::Dark => Some(true),
            Self::Light => Some(false),
            Self::Default | Self::Unknown => gnome_prefers_dark,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SessionSaveRequest {
    Ignore,
    RetryOnIdle,
    FlushOnIdle,
}

trait SessionSaveAccess {
    fn persistence_suspended(&self) -> bool;
    fn save_queued(&self) -> bool;
    fn set_save_queued(&mut self, queued: bool);
}

impl SessionSaveAccess for AppState {
    fn persistence_suspended(&self) -> bool {
        self.persistence_suspended
    }

    fn save_queued(&self) -> bool {
        self.save_queued
    }

    fn set_save_queued(&mut self, queued: bool) {
        self.save_queued = queued;
    }
}

fn queue_session_save_request<T: SessionSaveAccess>(state: &Rc<RefCell<T>>) -> SessionSaveRequest {
    let Ok(mut s) = state.try_borrow_mut() else {
        return SessionSaveRequest::RetryOnIdle;
    };

    if s.persistence_suspended() || s.save_queued() {
        SessionSaveRequest::Ignore
    } else {
        s.set_save_queued(true);
        SessionSaveRequest::FlushOnIdle
    }
}

fn request_session_save(state: &State) {
    match queue_session_save_request(state) {
        SessionSaveRequest::Ignore => {}
        SessionSaveRequest::RetryOnIdle => {
            let state = state.clone();
            glib::idle_add_local_once(move || {
                request_session_save(&state);
            });
        }
        SessionSaveRequest::FlushOnIdle => {
            let state = state.clone();
            glib::idle_add_local_once(move || {
                let should_save = {
                    let mut s = state.borrow_mut();
                    let should_save = s.save_queued && !s.persistence_suspended;
                    s.save_queued = false;
                    should_save
                };
                if should_save {
                    save_session_now(&state);
                }
            });
        }
    }
}

fn save_session_now(state: &State) {
    let session = snapshot_session_state(state);
    if let Err(err) = layout_state::save_session_atomic(&session) {
        eprintln!("limux: failed to save session state: {err}");
    }
}

fn suspend_persistence(state: &State, suspended: bool) {
    state.borrow_mut().persistence_suspended = suspended;
}

fn stop_session_saves_for_shutdown(state: &State) {
    let mut s = state.borrow_mut();
    s.save_queued = false;
    s.persistence_suspended = true;
}

fn apply_loaded_session(state: &State, mut loaded: LoadedSession) {
    suspend_persistence(state, true);

    apply_top_bar_state_immediately(state, loaded.state.top_bar_visible);

    let restored_any = !loaded.state.workspaces.is_empty();
    if restored_any {
        let restorable_agents = layout_state::RestorableAgentIndex::load();
        for workspace in &mut loaded.state.workspaces {
            layout_state::attach_restorable_agents_to_layout(
                &mut workspace.layout,
                workspace.id.as_deref().unwrap_or(""),
                &restorable_agents,
            );
        }
        for workspace in &loaded.state.workspaces {
            add_workspace_from_state(state, workspace);
        }
        restore_active_workspace(state, loaded.state.active_workspace_index);
    } else {
        let workspace = initial_workspace_state(
            dirs::home_dir().as_deref(),
            std::env::current_dir().ok().as_deref(),
        );
        add_workspace_from_state(state, &workspace);
    }
    apply_sidebar_state_immediately(state, &loaded.state.sidebar);

    suspend_persistence(state, false);

    save_session_now(state);
}

fn restore_active_workspace(state: &State, index: usize) {
    let maybe_row = {
        let s = state.borrow();
        if s.workspaces.is_empty() {
            None
        } else {
            let clamped = index.min(s.workspaces.len() - 1);
            Some((
                clamped,
                s.workspaces[clamped].sidebar_row.clone(),
                s.sidebar_list.clone(),
            ))
        }
    };

    if let Some((index, row, sidebar_list)) = maybe_row {
        switch_workspace(state, index);
        sidebar_list.select_row(Some(&row));
    }
}

fn apply_sidebar_state_immediately(state: &State, sidebar_state: &layout_state::SidebarState) {
    let (sidebar_shell, sidebar_handle, width) = {
        let mut s = state.borrow_mut();
        s.sidebar_expanded_width = sidebar_state.width.max(SIDEBAR_WIDTH);
        (
            s.sidebar_shell.clone(),
            s.sidebar_handle.clone(),
            s.sidebar_expanded_width,
        )
    };

    // Apply restored sidebar visibility directly; using the animated toggle path during
    // startup would create flicker and extra persistence churn while restore is suspended.
    set_sidebar_state_widgets(
        &sidebar_shell,
        &sidebar_handle,
        if sidebar_state.visible { width } else { 0 },
        sidebar_state.visible,
    );
}

fn apply_top_bar_state_immediately(state: &State, visible: bool) {
    state.borrow_mut().top_bar_visible = visible;
    sync_top_bar_visibility(state);
}

fn snapshot_session_state(state: &State) -> AppSessionState {
    let s = state.borrow();
    let restorable_agents = layout_state::RestorableAgentIndex::load();
    let sidebar_visible = sidebar_is_visible(&s);
    let sidebar_width = if sidebar_visible {
        sidebar_width(&s.sidebar_shell)
    } else {
        s.sidebar_expanded_width
    }
    .max(SIDEBAR_WIDTH);

    let workspaces = s
        .workspaces
        .iter()
        .map(|workspace| {
            let cwd = workspace.cwd.borrow().clone();
            let folder_path = workspace.folder_path.clone();
            let working_directory = folder_path.clone().or(cwd.clone());
            let mut layout = workspace
                .split_container
                .tree()
                .snapshot(working_directory.as_deref());
            layout_state::attach_restorable_agents_to_layout(
                &mut layout,
                &workspace.id,
                &restorable_agents,
            );
            WorkspaceState {
                id: Some(workspace.id.clone()),
                name: workspace.name.clone(),
                favorite: workspace.favorite,
                cwd,
                folder_path,
                layout,
            }
        })
        .collect();

    layout_state::normalize_session(AppSessionState {
        version: layout_state::SESSION_VERSION,
        active_workspace_index: s.active_idx,
        top_bar_visible: s.top_bar_visible,
        sidebar: layout_state::SidebarState {
            visible: sidebar_visible,
            width: sidebar_width,
        },
        workspaces,
    })
}

fn sidebar_is_visible(state: &AppState) -> bool {
    state.sidebar_shell.is_visible() && sidebar_width(&state.sidebar_shell) > 10
}

fn begin_window_move_from_widget(
    widget: &impl IsA<gtk::Widget>,
    window: &adw::ApplicationWindow,
    device: &gtk::gdk::Device,
    button: i32,
    x: f64,
    y: f64,
    timestamp: u32,
) {
    let Some((surface_x, surface_y)) = widget.translate_coordinates(window, x, y) else {
        return;
    };
    let Some(surface) = window.surface() else {
        return;
    };
    let Ok(toplevel) = surface.dynamic_cast::<gtk::gdk::Toplevel>() else {
        return;
    };
    toplevel.begin_move(device, button, surface_x, surface_y, timestamp);
}

fn split_ratio_state(paned: &gtk::Paned) -> Option<Rc<RefCell<f64>>> {
    unsafe {
        paned
            .data::<Rc<RefCell<f64>>>(SPLIT_RATIO_STATE_KEY)
            .map(|ptr| ptr.as_ref().clone())
    }
}

pub(crate) fn update_split_ratio_state(paned: &gtk::Paned, ratio: f64) {
    let ratio = layout_state::clamp_split_ratio(ratio);
    if let Some(stored_ratio) = split_ratio_state(paned) {
        *stored_ratio.borrow_mut() = ratio;
    } else {
        unsafe {
            paned.set_data(SPLIT_RATIO_STATE_KEY, Rc::new(RefCell::new(ratio)));
        }
    }
}

fn build_workspace_root(
    state: &State,
    shortcuts: &Rc<ResolvedShortcutConfig>,
    ws_id: &str,
    working_directory: Option<&str>,
    layout: &LayoutNodeState,
) -> (gtk::Widget, Rc<SplitTreeContainer>) {
    let tree_node = split_tree::build_split_node_from_layout(
        state,
        shortcuts,
        ws_id,
        working_directory,
        layout,
    );
    let container = SplitTreeContainer::new_from_tree(state, tree_node);
    let root = container.widget().clone().upcast::<gtk::Widget>();
    (root, container)
}

fn apply_ratio_value(
    paned: &gtk::Paned,
    orientation: gtk::Orientation,
    ratio: f64,
    applying: &Rc<Cell<bool>>,
) -> bool {
    let ratio = layout_state::clamp_split_ratio(ratio);
    let allocation = paned.allocation();
    let size = if orientation == gtk::Orientation::Horizontal {
        allocation.width()
    } else {
        allocation.height()
    };
    if size <= 0 {
        return false;
    }
    applying.set(true);
    paned.set_position(layout_state::split_position_from_ratio(ratio, size));
    update_split_ratio_state(paned, ratio);
    applying.set(false);
    true
}

pub(crate) fn apply_split_ratio_after_layout(
    paned: &gtk::Paned,
    orientation: gtk::Orientation,
    ratio_cell: Rc<RefCell<f64>>,
    applying: Rc<Cell<bool>>,
) {
    // Capture the ratio by value for the initial idle callback so that early
    // position_notify events (which may corrupt the cell) don't affect it.
    let initial_ratio = *ratio_cell.borrow();

    let paned_for_idle = paned.clone();
    let applying_for_idle = applying.clone();
    glib::idle_add_local_once(move || {
        apply_ratio_value(
            &paned_for_idle,
            orientation,
            initial_ratio,
            &applying_for_idle,
        );
    });

    let paned_for_map = paned.clone();
    // Re-apply the current data model ratio on every map event (workspace switches).
    // Reads from the cell so drag-adjusted ratios are restored correctly.
    paned.connect_map(move |_| {
        let ratio = *ratio_cell.borrow();
        apply_ratio_value(&paned_for_map, orientation, ratio, &applying);
    });
}

pub(crate) fn attach_split_position_persistence(state: &State, paned: &gtk::Paned) {
    update_split_ratio_state(paned, layout_state::DEFAULT_SPLIT_RATIO);
    let state = state.clone();
    paned.connect_position_notify(move |paned| {
        let allocation = paned.allocation();
        let size = if paned.orientation() == gtk::Orientation::Horizontal {
            allocation.width()
        } else {
            allocation.height()
        };
        let ratio = layout_state::snapshot_split_ratio(
            paned.position(),
            size,
            split_ratio_state(paned).map(|ratio| *ratio.borrow()),
        );
        update_split_ratio_state(paned, ratio);
        request_session_save(&state);
    });
}

// ---------------------------------------------------------------------------
// CSS
// ---------------------------------------------------------------------------

const HOST_ENTRY_CSS_CLASS: &str = "limux-host-entry";
const WORKSPACE_RENAME_ENTRY_CSS_CLASS: &str = "limux-ws-rename-entry";
const WORKSPACE_RENAME_ENTRY_CSS_CLASSES: [&str; 2] =
    [HOST_ENTRY_CSS_CLASS, WORKSPACE_RENAME_ENTRY_CSS_CLASS];
const SIDEBAR_HANDLE_CSS_CLASS: &str = "limux-sidebar-handle";
const SIDEBAR_HANDLE_CURSOR_NAME: &str = "col-resize";
const SIDEBAR_RESIZE_HANDLE_WIDTH_PX: i32 = 3;

const BASE_CSS: &str = r#"
:root {
    --limux-host-entry-bg: rgba(255, 255, 255, 0.98);
    --limux-host-entry-fg: rgba(15, 23, 42, 0.96);
    --limux-host-entry-border: rgba(15, 23, 42, 0.16);
    --limux-host-entry-border-focus: rgba(0, 145, 255, 0.72);
    --limux-host-entry-placeholder: rgba(15, 23, 42, 0.5);
}
@media (prefers-color-scheme: dark) {
    :root {
        --limux-host-entry-bg: rgba(44, 44, 48, 0.98);
        --limux-host-entry-fg: rgba(255, 255, 255, 0.96);
        --limux-host-entry-border: rgba(255, 255, 255, 0.14);
        --limux-host-entry-border-focus: rgba(0, 145, 255, 0.78);
        --limux-host-entry-placeholder: rgba(255, 255, 255, 0.48);
    }
}
.limux-host-entry {
    background-color: var(--limux-host-entry-bg);
    color: var(--limux-host-entry-fg);
    border: 1px solid var(--limux-host-entry-border);
    border-radius: 6px;
    caret-color: currentColor;
}
.limux-host-entry:focus-within {
    border-color: var(--limux-host-entry-border-focus);
}
.limux-host-entry text {
    background-color: transparent;
    color: var(--limux-host-entry-fg);
}
.limux-host-entry text placeholder {
    color: var(--limux-host-entry-placeholder);
}
.limux-host-entry image {
    color: var(--limux-host-entry-placeholder);
}
.limux-sidebar {
    background-color: @window_bg_color;
    color: @window_fg_color;
    border-right: 1px solid alpha(@window_fg_color, 0.08);
}
.limux-sidebar-row-box {
    padding: 8px 6px 8px 3px;
    border-radius: 6px;
    margin: 2px 3px 2px 1px;
}
.limux-ws-name {
    color: alpha(@window_fg_color, 0.72);
    font-size: 15px;
}
row:selected .limux-ws-name {
    color: @window_fg_color;
}
.limux-ws-star-btn {
    color: alpha(@window_fg_color, 0.45);
    border: none;
    min-height: 0;
    min-width: 0;
    padding: 0 4px;
    font-size: 22px;
}
.limux-ws-star-btn:hover {
    color: alpha(@window_fg_color, 0.9);
}
row:selected .limux-ws-star-btn {
    color: alpha(@window_fg_color, 0.85);
}
.limux-ws-star-btn-active {
    color: @accent_bg_color;
}
.limux-ws-rename-entry {
    min-height: 0;
    padding: 0 4px;
    margin: 0;
}
.limux-notify-dot {
    color: @accent_bg_color;
    font-size: 10px;
    margin-right: 6px;
}
.limux-notify-dot-hidden {
    color: transparent;
    font-size: 10px;
    margin-right: 6px;
}
.limux-notify-msg {
    color: alpha(@window_fg_color, 0.35);
    font-size: 11px;
}
.limux-notify-msg-unread {
    color: alpha(@accent_bg_color, 0.9);
    font-size: 11px;
}
.limux-sidebar-row-unread {
    background-color: alpha(@accent_bg_color, 0.16);
    border-left: 3px solid @accent_bg_color;
    border-radius: 6px;
    margin-left: 0;
    margin-right: 0;
}
.limux-sidebar-row-unread .limux-ws-name {
    color: @window_fg_color;
    font-weight: 700;
}
.limux-drop-above .limux-sidebar-row-box {
    border-radius: 0;
    box-shadow: 0 -2px 0 0 @accent_bg_color;
}
.limux-drop-below .limux-sidebar-row-box {
    border-radius: 0;
    box-shadow: 0 2px 0 0 @accent_bg_color;
}
.limux-tab-drop-target {
    background-color: alpha(@accent_bg_color, 0.18);
    border-radius: 8px;
}
.limux-sidebar row:drop(active) {
    box-shadow: none;
}
.limux-sidebar-title {
    color: alpha(@window_fg_color, 0.55);
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 1px;
}
.limux-sidebar-btn {
    background: alpha(@window_fg_color, 0.08);
    color: alpha(@window_fg_color, 0.7);
    border: 1px solid transparent;
    border-radius: 6px;
    padding: 6px 12px;
    min-height: 0;
    transition: all 200ms ease;
}
.limux-sidebar-btn:hover {
    background: alpha(@window_fg_color, 0.14);
    color: @window_fg_color;
}
.limux-sidebar-btn-trash {
    background: alpha(@error_color, 0.16);
    color: @error_color;
    border: 1px solid alpha(@error_color, 0.4);
}
.limux-sidebar-btn-trash-hover {
    background: alpha(@error_color, 0.26);
    color: @error_color;
    border: 1px solid alpha(@error_color, 0.7);
}
.limux-tab-drag-active {
    background-color: alpha(@accent_bg_color, 0.12);
    border-width: 1px;
    border-style: dashed;
    border-color: alpha(@accent_bg_color, 0.6);
    border-radius: 8px;
}
.limux-sidebar-btn.limux-tab-drop-target {
    background-color: alpha(@accent_bg_color, 0.28);
    border-color: alpha(@accent_bg_color, 0.9);
}
.limux-ws-path {
    color: alpha(@window_fg_color, 0.3);
    font-size: 12px;
}
row:selected .limux-ws-path {
    color: alpha(@window_fg_color, 0.5);
}
.limux-content {
    background-color: @window_bg_color;
}
.limux-sidebar-handle {
    min-width: 3px;
    background-color: alpha(@window_fg_color, 0.08);
}
.limux-sidebar-handle:hover {
    background-color: alpha(@accent_bg_color, 0.45);
}
"#;

const CONTENT_BACKGROUND_RGB: (u8, u8, u8) = (23, 23, 23);

/// Squares off the CSD window corners and drops the client-side shadow
/// when the compositor draws the window decorations (see
/// `compositor_provides_decorations`), so the window fills the KWin frame
/// without transparent gaps at the corners.
const SSD_CSS: &str = r#"
window.csd,
window.csd:backdrop {
    --window-radius: 0px;
    border-radius: 0;
    box-shadow: none;
    outline: none;
}
"#;

/// True when the compositor speaks the KDE server-decoration protocol
/// (KWin). In that case build_window() requests server-side decorations
/// from the compositor and skips the in-app header bar.
fn compositor_provides_decorations() -> bool {
    gtk::gdk::Display::default()
        .and_then(|display| display.downcast::<gdk4_wayland::WaylandDisplay>().ok())
        .map(|display| display.query_registry("org_kde_kwin_server_decoration_manager"))
        .unwrap_or(false)
}

fn app_css(background_opacity: f64, config: &app_config::AppConfig) -> String {
    let ssd_css = if compositor_provides_decorations() {
        SSD_CSS
    } else {
        ""
    };
    format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        build_window_css(background_opacity),
        pane::PANE_CSS,
        keybind_editor::KEYBIND_EDITOR_CSS,
        crate::settings_editor::SETTINGS_CSS,
        crate::settings_editor::ui_scale_css(config),
        ssd_css,
    )
}

fn reload_app_css(state: &State, config: &app_config::AppConfig) {
    let background_opacity =
        sanitize_background_opacity(crate::terminal::ghostty_background_opacity());
    state
        .borrow()
        .css_provider
        .load_from_data(&app_css(background_opacity, config));
}

// ---------------------------------------------------------------------------
// Window construction
// ---------------------------------------------------------------------------

pub fn build_window(app: &adw::Application) {
    let display = gtk::gdk::Display::default().expect("display");
    let gnome_interface_settings = gnome_interface_settings();
    let portal_color_scheme_preference = Rc::new(Cell::new(PortalColorSchemePreference::Unknown));
    let system_prefers_dark = Rc::new(Cell::new(resolve_system_prefers_dark(
        portal_color_scheme_preference.get(),
        gnome_interface_settings.as_ref(),
    )));
    let loaded_config = app_config::load();
    for warning in &loaded_config.warnings {
        eprintln!("limux: {warning}");
    }
    let config = Rc::new(RefCell::new(loaded_config.config));
    let background_opacity =
        sanitize_background_opacity(crate::terminal::ghostty_background_opacity());

    let shortcuts = Rc::new(shortcut_config::load_shortcuts_for_display(&display));
    for warning in &shortcuts.warnings {
        eprintln!("limux: {warning}");
    }

    // Load CSS
    let provider = gtk::CssProvider::new();
    let all_css = app_css(background_opacity, &config.borrow());
    provider.load_from_data(&all_css);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let style_manager = adw::StyleManager::default();
    apply_appearance(
        &style_manager,
        system_prefers_dark.get(),
        &config.borrow().appearance,
    );

    // Register custom icons — look for icons dir relative to the executable
    let icon_theme = gtk::IconTheme::for_display(&display);
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    // Try several possible icon locations
    for path in [
        exe_dir
            .as_ref()
            .map(|d| d.join("../../rust/limux-host-linux/icons")),
        exe_dir.as_ref().map(|d| d.join("../icons")),
        Some(std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/icons"
        ))),
    ]
    .iter()
    .flatten()
    {
        if path.exists() {
            icon_theme.add_search_path(path);
        }
    }

    let title = format!("Limux v{}", crate::VERSION);
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(&title)
        .default_width(1400)
        .default_height(900)
        .build();
    apply_window_background_class(&window, background_opacity);

    // libadwaita forces client-side decorations (it installs an invisible
    // titlebar widget internally), so compositors never draw server-side
    // decorations for Limux on their own. When the compositor speaks the
    // KDE server-decoration protocol (KWin), skip the in-app header bar:
    // build_window() explicitly requests SERVER mode after present(),
    // which makes the compositor draw the window chrome. Everywhere else
    // (mutter, X11, …) the in-app header bar is the only chrome we get.
    let compositor_provides_decorations = compositor_provides_decorations();

    let header = if compositor_provides_decorations {
        None
    } else {
        let bar = adw::HeaderBar::new();
        bar.set_title_widget(Some(&gtk::Label::builder().label(&title).build()));
        Some(bar)
    };

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::None);
    stack.set_hexpand(true);
    stack.set_vexpand(true);
    stack.add_css_class("limux-content");

    let sidebar_list = gtk::ListBox::new();
    sidebar_list.set_selection_mode(gtk::SelectionMode::Single);
    sidebar_list.add_css_class("navigation-sidebar");

    let sidebar_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .child(&sidebar_list)
        .build();

    let sidebar_title_label = gtk::Label::builder()
        .label("WORKSPACES")
        .xalign(0.0)
        .hexpand(true)
        .margin_start(12)
        .build();
    sidebar_title_label.add_css_class("limux-sidebar-title");

    let sidebar_title = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .margin_top(8)
        .margin_bottom(4)
        .margin_end(6)
        .build();
    sidebar_title.append(&sidebar_title_label);

    {
        let window = window.clone();
        let drag_title = sidebar_title.clone();
        let drag = gtk::GestureClick::new();
        drag.set_button(1);
        drag.connect_pressed(move |gesture, _, x, y| {
            let Some(device) = gesture.current_event_device() else {
                return;
            };
            let button = gesture.current_button() as i32;
            let timestamp = gesture.current_event_time();
            begin_window_move_from_widget(&drag_title, &window, &device, button, x, y, timestamp);
            gesture.set_state(gtk::EventSequenceState::Claimed);
        });
        sidebar_title.add_controller(drag);
    }

    let new_ws_btn = gtk::Button::builder()
        .label("New Workspace")
        .hexpand(true)
        .margin_start(6)
        .margin_end(6)
        .margin_bottom(6)
        .build();
    new_ws_btn.add_css_class("limux-sidebar-btn");

    // Drop target on the button: workspace drags delete, tab drags create a new workspace.
    let btn_drop = gtk::DropTarget::new(glib::Type::STRING, gtk::gdk::DragAction::MOVE);
    btn_drop.set_preload(true);
    {
        let btn = new_ws_btn.clone();
        btn_drop.connect_motion(move |_, _, _| {
            if pane::is_tab_dragging() {
                btn.add_css_class("limux-tab-drop-target");
            } else {
                btn.add_css_class("limux-sidebar-btn-trash-hover");
            }
            gtk::gdk::DragAction::MOVE
        });
    }
    {
        let btn = new_ws_btn.clone();
        btn_drop.connect_leave(move |_| {
            btn.remove_css_class("limux-sidebar-btn-trash-hover");
            btn.remove_css_class("limux-tab-drop-target");
        });
    }
    new_ws_btn.add_controller(btn_drop.clone());

    let sidebar = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .build();
    sidebar.add_css_class("limux-sidebar");
    sidebar.append(&sidebar_title);
    sidebar.append(&sidebar_scroll);
    sidebar.append(&new_ws_btn);

    let (main_split, sidebar_shell, sidebar_handle) = build_sidebar_split(&sidebar, &stack);

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    if let Some(ref header) = header {
        vbox.append(header);
    }
    vbox.append(&main_split);
    window.set_content(Some(&vbox));

    let state: State = Rc::new(RefCell::new(AppState {
        app: app.clone(),
        window: window.clone(),
        top_bar: header.clone(),
        top_bar_visible: true,
        config,
        css_provider: provider.clone(),
        system_prefers_dark: system_prefers_dark.clone(),
        workspaces: Vec::new(),
        active_idx: 0,
        shortcuts,
        stack: stack.clone(),
        sidebar_list: sidebar_list.clone(),
        sidebar_shell: sidebar_shell.clone(),
        sidebar_handle: sidebar_handle.clone(),
        new_ws_btn: new_ws_btn.clone(),
        sidebar_animation: None,
        sidebar_animation_epoch: 0,
        sidebar_expanded_width: SIDEBAR_WIDTH,
        persistence_suspended: false,
        save_queued: false,
        workspace_dragging: None,
        desktop_notification_routes: HashMap::new(),
        _theme_portal_signal: None,
        _theme_gnome_settings: None,
        _theme_gnome_signal: None,
        _desktop_notification_token_signal: None,
        _desktop_notification_action_signal: None,
        _desktop_notification_closed_signal: None,
    }));
    CONTROL_STATE.with(|slot| {
        *slot.borrow_mut() = Some(state.clone());
    });

    install_sidebar_resize(&state, &main_split, &sidebar, &sidebar_shell);

    {
        let state = state.clone();
        window.connect_is_active_notify(move |window| {
            if !window.is_active() {
                return;
            }
            let workspace_id = state
                .borrow()
                .active_workspace()
                .map(|workspace| workspace.id.clone());
            if let Some(workspace_id) = workspace_id {
                clear_visible_tab_unread(&state, &workspace_id);
            }
        });
    }

    {
        let state = state.clone();
        let system_prefers_dark = system_prefers_dark.clone();
        style_manager.connect_dark_notify(move |style_manager| {
            sync_ghostty_color_scheme_for_config(
                style_manager,
                system_prefers_dark.get(),
                &state.borrow().config.borrow().appearance,
            );
        });
    }

    let theme_gnome_signal = gnome_interface_settings.as_ref().map(|settings| {
        connect_gnome_appearance_watch(
            settings,
            state.clone(),
            style_manager.clone(),
            system_prefers_dark.clone(),
            portal_color_scheme_preference.clone(),
        )
    });
    {
        let mut s = state.borrow_mut();
        s._theme_gnome_settings = gnome_interface_settings.clone();
        s._theme_gnome_signal = theme_gnome_signal;
    }
    connect_portal_appearance_watch_async(
        gnome_interface_settings.clone(),
        state.clone(),
        style_manager.clone(),
        system_prefers_dark.clone(),
        portal_color_scheme_preference.clone(),
    );
    connect_desktop_notification_watch_async(state.clone());

    apply_shortcuts_to_application(app, &state.borrow().shortcuts);

    {
        let state = state.clone();
        window.connect_fullscreened_notify(move |_| {
            sync_top_bar_visibility(&state);
        });
    }

    register_app_actions(app, &state);
    register_window_actions(&window, &state);
    install_key_capture(&window, &state);

    // Any click anywhere in the window commits an active inline rename,
    // unless the click is inside the rename entry itself.
    {
        let sl = sidebar_list.clone();
        let win = window.clone();
        let click_anywhere = gtk::GestureClick::new();
        click_anywhere.set_propagation_phase(gtk::PropagationPhase::Capture);
        click_anywhere.connect_pressed(move |_, _, x, y| {
            let entry = find_active_rename_entry(&sl).or_else(|| pane::find_tab_rename_entry(&win));
            if let Some(entry) = entry {
                let allocation = entry.allocation();
                commit_inline_rename_for_click(
                    win.translate_coordinates(&entry, x, y),
                    allocation.width(),
                    allocation.height(),
                    || entry.emit_activate(),
                );
            }
        });
        window.add_controller(click_anywhere);
    }

    {
        let state = state.clone();
        sidebar_list.connect_row_selected(move |_, row| {
            if let Some(row) = row {
                let idx = row.index() as usize;
                switch_workspace(&state, idx);
            }
        });
    }

    {
        let state = state.clone();
        new_ws_btn.connect_clicked(move |_| {
            add_workspace(&state, None);
        });
    }

    {
        let btn = new_ws_btn.clone();
        pane::on_tab_drag_change(move |dragging| {
            if dragging {
                btn.add_css_class("limux-tab-drag-active");
            } else {
                btn.remove_css_class("limux-tab-drag-active");
                btn.remove_css_class("limux-tab-drop-target");
            }
        });
    }

    {
        let state = state.clone();
        let btn = new_ws_btn.clone();
        btn_drop.connect_drop(move |_, value, _, _| {
            btn.set_label("New Workspace");
            btn.remove_css_class("limux-sidebar-btn-trash");
            btn.remove_css_class("limux-sidebar-btn-trash-hover");
            btn.remove_css_class("limux-tab-drop-target");
            if let Ok(payload) = value.get::<String>() {
                if payload.contains(':') {
                    return create_workspace_for_tab(&state, &payload);
                }
                close_workspace_by_id(&state, &payload);
                return true;
            }
            false
        });
    }

    // Save the full session on window close.
    {
        let state = state.clone();
        window.connect_close_request(move |_| {
            save_session_now(&state);
            stop_session_saves_for_shutdown(&state);
            let split_containers: Vec<_> = state
                .borrow()
                .workspaces
                .iter()
                .map(|workspace| workspace.split_container.clone())
                .collect();
            for split_container in split_containers {
                split_container.retire_panes();
            }
            CONTROL_STATE.with(|slot| {
                slot.borrow_mut().take();
            });
            glib::Propagation::Proceed
        });
    }

    apply_loaded_session(&state, layout_state::load_session());

    crate::control_bridge::start(dispatch_control_command);

    window.present();

    // Ask the compositor for server-side decorations when it supports the
    // KDE server-decoration protocol (see the header-bar decision above).
    // libadwaita marks the window client-decorated, which makes GtkWindow
    // compute decorated=false at realize time; forcing the GDK-level
    // property after present() makes GDK request SERVER mode from KWin via
    // org_kde_kwin_server_decoration.
    if compositor_provides_decorations {
        if let Some(surface) = window.surface() {
            if let Ok(toplevel) = surface.downcast::<gtk::gdk::Toplevel>() {
                toplevel.set_decorated(true);
            }
        }
    }
}

fn build_window_css(background_opacity: f64) -> String {
    let background_opacity = sanitize_background_opacity(background_opacity);
    let (r, g, b) = CONTENT_BACKGROUND_RGB;
    format!(
        "{BASE_CSS}\n.limux-content {{\n    background-color: rgba({r}, {g}, {b}, {background_opacity:.3});\n}}\n"
    )
}

fn build_sidebar_split(sidebar: &gtk::Box, stack: &gtk::Stack) -> (gtk::Box, gtk::Box, gtk::Box) {
    let sidebar_shell = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(false)
        .vexpand(true)
        .build();
    sidebar_shell.append(sidebar);
    set_sidebar_width(&sidebar_shell, SIDEBAR_WIDTH);

    let sidebar_handle = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .width_request(SIDEBAR_RESIZE_HANDLE_WIDTH_PX)
        .hexpand(false)
        .vexpand(true)
        .build();
    sidebar_handle.add_css_class(SIDEBAR_HANDLE_CSS_CLASS);
    sidebar_handle.set_cursor_from_name(Some(SIDEBAR_HANDLE_CURSOR_NAME));

    let main_split = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .hexpand(true)
        .vexpand(true)
        .build();
    main_split.append(&sidebar_shell);
    main_split.append(&sidebar_handle);
    main_split.append(stack);

    (main_split, sidebar_shell, sidebar_handle)
}

fn install_sidebar_resize(
    state: &State,
    main_split: &gtk::Box,
    sidebar: &gtk::Box,
    sidebar_shell: &gtk::Box,
) {
    let resizing_sidebar = Rc::new(Cell::new(false));
    let drag_origin = Rc::new(Cell::new(SIDEBAR_WIDTH));
    let drag = gtk::GestureDrag::new();

    {
        let drag_origin = drag_origin.clone();
        let sidebar = sidebar.clone();
        let sidebar_shell = sidebar_shell.clone();
        let resizing_sidebar = resizing_sidebar.clone();
        drag.connect_drag_begin(move |gesture, x, _| {
            let current_width = sidebar_width(&sidebar_shell);
            let handle_start = current_width as f64;
            let handle_end = handle_start + SIDEBAR_RESIZE_HANDLE_WIDTH_PX as f64;
            if x < handle_start || x > handle_end {
                gesture.set_state(gtk::EventSequenceState::Denied);
                return;
            }
            resizing_sidebar.set(true);
            drag_origin.set(current_width.max(sidebar_min_width(&sidebar)));
            gesture.set_state(gtk::EventSequenceState::Claimed);
        });
    }

    {
        let drag_origin = drag_origin.clone();
        let sidebar = sidebar.clone();
        let sidebar_shell = sidebar_shell.clone();
        let resizing_sidebar = resizing_sidebar.clone();
        let state = state.clone();
        drag.connect_drag_update(move |_, offset_x, _| {
            if !resizing_sidebar.get() {
                return;
            }
            let min_width = sidebar_min_width(&sidebar);
            let width = (drag_origin.get() as f64 + offset_x).round() as i32;
            let width = width.max(min_width);
            set_sidebar_width(&sidebar_shell, width);
            state.borrow_mut().sidebar_expanded_width = width;
        });
    }

    {
        let sidebar_shell = sidebar_shell.clone();
        let resizing_sidebar = resizing_sidebar.clone();
        let state = state.clone();
        drag.connect_drag_end(move |_, _, _| {
            resizing_sidebar.set(false);
            state.borrow_mut().sidebar_expanded_width = sidebar_width(&sidebar_shell);
            request_session_save(&state);
        });
    }

    main_split.add_controller(drag);
}

fn set_sidebar_width(sidebar_shell: &gtk::Box, width: i32) {
    sidebar_shell.set_width_request(width.max(0));
}

fn set_sidebar_state_widgets(
    sidebar_shell: &gtk::Box,
    sidebar_handle: &gtk::Box,
    width: i32,
    visible: bool,
) {
    set_sidebar_width(sidebar_shell, width);
    sidebar_shell.set_visible(visible);
    sidebar_handle.set_visible(visible);
}

fn sidebar_width(sidebar_shell: &gtk::Box) -> i32 {
    sidebar_shell.width_request().max(0)
}

fn sidebar_min_width(sidebar: &gtk::Box) -> i32 {
    let (minimum, _, _, _) = sidebar.measure(gtk::Orientation::Horizontal, -1);
    minimum.max(1)
}

fn sanitize_background_opacity(background_opacity: f64) -> f64 {
    if background_opacity.is_finite() {
        background_opacity.clamp(0.0, 1.0)
    } else {
        1.0
    }
}

fn use_opaque_window_background(background_opacity: f64) -> bool {
    sanitize_background_opacity(background_opacity) >= 1.0
}

fn apply_window_background_class(window: &adw::ApplicationWindow, background_opacity: f64) {
    if use_opaque_window_background(background_opacity) {
        window.add_css_class("background");
    } else {
        window.remove_css_class("background");
    }
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

fn register_window_actions(window: &adw::ApplicationWindow, state: &State) {
    let action_defs: Vec<(&'static str, ShortcutCommand)> = {
        let s = state.borrow();
        s.shortcuts
            .shortcuts
            .iter()
            .filter(|shortcut| shortcut.definition.action_name.starts_with("win."))
            .map(|shortcut| {
                (
                    shortcut.definition.action_basename(),
                    shortcut.definition.command,
                )
            })
            .collect()
    };

    for (name, command) in action_defs {
        let action = gtk::gio::SimpleAction::new(name, None);
        let state = state.clone();
        action.connect_activate(move |_, _| {
            dispatch_shortcut_command(&state, command);
        });
        window.add_action(&action);
    }
}

fn register_app_actions(app: &adw::Application, state: &State) {
    let action_defs: Vec<(&'static str, ShortcutCommand)> = {
        let s = state.borrow();
        s.shortcuts
            .shortcuts
            .iter()
            .filter(|shortcut| shortcut.definition.action_name.starts_with("app."))
            .map(|shortcut| {
                (
                    shortcut.definition.action_basename(),
                    shortcut.definition.command,
                )
            })
            .collect()
    };

    for (name, command) in action_defs {
        if app.lookup_action(name).is_some() {
            continue;
        }
        let action = gtk::gio::SimpleAction::new(name, None);
        let state = state.clone();
        action.connect_activate(move |_, _| {
            dispatch_shortcut_command(&state, command);
        });
        app.add_action(&action);
    }
}

/// Intercept keyboard shortcuts in the CAPTURE phase for window-level bindings.
fn install_key_capture(window: &adw::ApplicationWindow, state: &State) {
    let key_controller = gtk::EventControllerKey::new();
    key_controller.set_propagation_phase(gtk::PropagationPhase::Capture);

    let state = state.clone();
    key_controller.connect_key_pressed(move |controller, keyval, keycode, modifier| {
        let focused_listening_editor = controller
            .widget()
            .and_then(|widget| widget.downcast::<gtk::Window>().ok())
            .map(|window| focused_widget_is_listening_for_keybind_capture(&window))
            .unwrap_or(false);
        if focused_listening_editor {
            return glib::Propagation::Proceed;
        }

        let matched = {
            let s = state.borrow();
            let display = controller.widget().map(|widget| widget.display());
            shortcut_match_from_key_press(&s.shortcuts, display.as_ref(), keyval, keycode, modifier)
        }
        .filter(|matched| {
            let context = controller
                .widget()
                .and_then(|widget| widget.downcast::<gtk::Window>().ok())
                .map(|window| focused_editable_capture_context(&state, &window))
                .unwrap_or_default();
            !shortcut_blocked_by_editable(matched.command, matched.editable_capture_policy, context)
        })
        .map(|matched| dispatch_shortcut_command(&state, matched.command))
        .unwrap_or(false);

        shortcut_dispatch_propagation(matched)
    });

    window.add_controller(key_controller);
}

fn focused_widget_is_listening_for_keybind_capture(window: &gtk::Window) -> bool {
    let mut widget = gtk::prelude::GtkWindowExt::focus(window);
    while let Some(current) = widget {
        if current.has_css_class(keybind_editor::KEYBIND_EDITOR_LISTENING_CSS) {
            return true;
        }
        widget = current.parent();
    }
    false
}

fn focused_widget_is_editable(window: &gtk::Window) -> bool {
    let mut widget = gtk::prelude::GtkWindowExt::focus(window);
    while let Some(current) = widget {
        if current.is::<gtk::Entry>()
            || current.is::<gtk::SearchEntry>()
            || current.is::<gtk::TextView>()
        {
            return true;
        }
        widget = current.parent();
    }
    false
}

fn focused_editable_capture_context(state: &State, window: &gtk::Window) -> EditableCaptureContext {
    let gtk_editable = focused_widget_is_editable(window);
    match focused_shortcut_target(state) {
        pane::FocusedShortcutTarget::Browser(target) => EditableCaptureContext {
            gtk_editable,
            browser_dom_editable: target.is_page_editable(),
            browser_find_active: target.is_find_active(),
        },
        _ => EditableCaptureContext {
            gtk_editable,
            ..EditableCaptureContext::default()
        },
    }
}

fn shortcut_allowed_while_browser_find_active(command: ShortcutCommand) -> bool {
    matches!(
        command,
        ShortcutCommand::SurfaceFindNext
            | ShortcutCommand::SurfaceFindPrevious
            | ShortcutCommand::SurfaceFindHide
    )
}

fn shortcut_blocked_by_editable(
    command: ShortcutCommand,
    policy: EditableCapturePolicy,
    context: EditableCaptureContext,
) -> bool {
    if policy == EditableCapturePolicy::AlwaysCapture {
        return false;
    }

    if context.browser_find_active && shortcut_allowed_while_browser_find_active(command) {
        return false;
    }

    context.gtk_editable || context.browser_dom_editable
}

fn shortcut_dispatch_propagation(matched: bool) -> glib::Propagation {
    if matched {
        glib::Propagation::Stop
    } else {
        glib::Propagation::Proceed
    }
}

#[cfg(test)]
fn shortcut_command_from_key_event(
    shortcuts: &ResolvedShortcutConfig,
    keyval: gtk::gdk::Key,
    modifier: gtk::gdk::ModifierType,
) -> Option<ShortcutCommand> {
    shortcut_config::NormalizedShortcut::from_gdk_key(keyval, modifier)
        .map(|shortcut| shortcut.to_runtime_combo())
        .and_then(|combo| shortcuts.command_for_runtime_combo(&combo))
}

struct MatchedShortcut {
    command: ShortcutCommand,
    editable_capture_policy: EditableCapturePolicy,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct EditableCaptureContext {
    gtk_editable: bool,
    browser_dom_editable: bool,
    browser_find_active: bool,
}

fn shortcut_match_from_key_press(
    shortcuts: &ResolvedShortcutConfig,
    display: Option<&gtk::gdk::Display>,
    keyval: gtk::gdk::Key,
    keycode: u32,
    modifier: gtk::gdk::ModifierType,
) -> Option<MatchedShortcut> {
    shortcut_config::NormalizedShortcut::from_gdk_key_event(display, keyval, keycode, modifier)
        .map(|shortcut| shortcut.to_runtime_combo())
        .and_then(|combo| shortcuts.shortcut_for_runtime_combo(&combo))
        .map(|shortcut| MatchedShortcut {
            command: shortcut.definition.command,
            editable_capture_policy: shortcut.definition.editable_capture_policy,
        })
}

fn dispatch_shortcut_command(state: &State, command: ShortcutCommand) -> bool {
    match command {
        ShortcutCommand::NewWorkspace => {
            add_workspace(state, None);
            true
        }
        ShortcutCommand::CloseWorkspace => {
            close_workspace(state);
            true
        }
        ShortcutCommand::QuitApp => {
            quit_app(state);
            true
        }
        ShortcutCommand::NewInstance => spawn_new_instance(state),
        ShortcutCommand::ToggleSidebar => {
            toggle_sidebar(state);
            true
        }
        ShortcutCommand::ToggleTopBar => {
            toggle_top_bar(state);
            true
        }
        ShortcutCommand::ToggleFullscreen => {
            toggle_fullscreen(state);
            true
        }
        ShortcutCommand::NextWorkspace => {
            cycle_workspace(state, 1);
            true
        }
        ShortcutCommand::PrevWorkspace => {
            cycle_workspace(state, -1);
            true
        }
        ShortcutCommand::CycleTabPrev => {
            cycle_focused_pane_tab(state, -1);
            true
        }
        ShortcutCommand::CycleTabNext => {
            cycle_focused_pane_tab(state, 1);
            true
        }
        ShortcutCommand::SplitDown => {
            split_focused_pane(state, gtk::Orientation::Vertical);
            true
        }
        ShortcutCommand::NewTerminal => {
            add_tab_to_focused_pane(state, false);
            true
        }
        ShortcutCommand::SplitRight => {
            split_focused_pane(state, gtk::Orientation::Horizontal);
            true
        }
        ShortcutCommand::CloseFocusedPane => {
            close_focused_pane(state);
            true
        }
        ShortcutCommand::CloseFocusedTab => {
            close_focused_tab(state);
            true
        }
        ShortcutCommand::ToggleFocusedPaneZoom => {
            toggle_focused_pane_zoom(state);
            true
        }
        ShortcutCommand::FocusLeft => {
            focus_pane_in_direction(state, Direction::Left);
            true
        }
        ShortcutCommand::FocusRight => {
            focus_pane_in_direction(state, Direction::Right);
            true
        }
        ShortcutCommand::FocusUp => {
            focus_pane_in_direction(state, Direction::Up);
            true
        }
        ShortcutCommand::FocusDown => {
            focus_pane_in_direction(state, Direction::Down);
            true
        }
        ShortcutCommand::ActivateWorkspace1 => {
            activate_workspace_shortcut(state, 0);
            true
        }
        ShortcutCommand::ActivateWorkspace2 => {
            activate_workspace_shortcut(state, 1);
            true
        }
        ShortcutCommand::ActivateWorkspace3 => {
            activate_workspace_shortcut(state, 2);
            true
        }
        ShortcutCommand::ActivateWorkspace4 => {
            activate_workspace_shortcut(state, 3);
            true
        }
        ShortcutCommand::ActivateWorkspace5 => {
            activate_workspace_shortcut(state, 4);
            true
        }
        ShortcutCommand::ActivateWorkspace6 => {
            activate_workspace_shortcut(state, 5);
            true
        }
        ShortcutCommand::ActivateWorkspace7 => {
            activate_workspace_shortcut(state, 6);
            true
        }
        ShortcutCommand::ActivateWorkspace8 => {
            activate_workspace_shortcut(state, 7);
            true
        }
        ShortcutCommand::ActivateLastWorkspace => {
            activate_last_workspace_shortcut(state);
            true
        }
        ShortcutCommand::OpenBrowserInSplit
        | ShortcutCommand::BrowserFocusLocation
        | ShortcutCommand::BrowserBack
        | ShortcutCommand::BrowserForward
        | ShortcutCommand::BrowserReload
        | ShortcutCommand::BrowserInspector
        | ShortcutCommand::BrowserConsole => dispatch_browser_command(state, command),
        ShortcutCommand::SurfaceFind
        | ShortcutCommand::SurfaceFindNext
        | ShortcutCommand::SurfaceFindPrevious
        | ShortcutCommand::SurfaceFindHide
        | ShortcutCommand::SurfaceUseSelectionForFind => {
            dispatch_terminal_command(state, command) || dispatch_browser_command(state, command)
        }
        ShortcutCommand::TerminalClearScrollback
        | ShortcutCommand::TerminalCopy
        | ShortcutCommand::TerminalPaste
        | ShortcutCommand::TerminalIncreaseFontSize
        | ShortcutCommand::TerminalDecreaseFontSize
        | ShortcutCommand::TerminalResetFontSize => dispatch_terminal_command(state, command),
    }
}

fn apply_shortcuts_to_application(app: &adw::Application, shortcuts: &ResolvedShortcutConfig) {
    for (action_name, accels) in shortcuts.gtk_accel_entries() {
        let accel_refs: Vec<&str> = accels.iter().map(String::as_str).collect();
        app.set_accels_for_action(action_name, &accel_refs);
    }
}

fn apply_shortcut_config(state: &State, shortcuts: ResolvedShortcutConfig) {
    let (app, workspace_roots, shortcuts_rc) = {
        let mut s = state.borrow_mut();
        s.shortcuts = Rc::new(shortcuts);
        (
            s.app.clone(),
            s.workspaces
                .iter()
                .map(|ws| ws.root.clone())
                .collect::<Vec<_>>(),
            s.shortcuts.clone(),
        )
    };

    apply_shortcuts_to_application(&app, &shortcuts_rc);
    for root in workspace_roots {
        refresh_shortcut_tooltips_in_layout(&root, &shortcuts_rc);
    }
}

fn refresh_shortcut_tooltips_in_layout(widget: &gtk::Widget, shortcuts: &ResolvedShortcutConfig) {
    if let Some(paned) = widget.downcast_ref::<gtk::Paned>() {
        if let Some(start) = paned.start_child() {
            refresh_shortcut_tooltips_in_layout(&start, shortcuts);
        }
        if let Some(end) = paned.end_child() {
            refresh_shortcut_tooltips_in_layout(&end, shortcuts);
        }
        return;
    }

    pane::refresh_shortcut_tooltips(widget, shortcuts);
}

fn persist_shortcut_binding(
    state: &State,
    id: ShortcutId,
    binding: Option<shortcut_config::NormalizedShortcut>,
) -> Result<ResolvedShortcutConfig, String> {
    let updated = {
        let s = state.borrow();
        s.shortcuts
            .with_binding(id, binding)
            .map_err(|err| err.to_string())?
    };

    let Some(path) = shortcut_config::shortcuts_path() else {
        return Err("config directory unavailable".to_string());
    };

    shortcut_config::write_shortcuts(&path, &updated).map_err(|err| err.to_string())?;
    let display = {
        let s = state.borrow();
        s.stack.display()
    };
    let reloaded = shortcut_config::load_shortcuts_or_default_with_display(&path, Some(&display));
    if !reloaded.warnings.is_empty() {
        return Err(reloaded.warnings.join("; "));
    }

    apply_shortcut_config(state, reloaded.clone());
    Ok(reloaded)
}

fn adw_color_scheme_for(scheme: app_config::ColorScheme) -> adw::ColorScheme {
    match scheme {
        app_config::ColorScheme::System => adw::ColorScheme::Default,
        app_config::ColorScheme::Dark => adw::ColorScheme::ForceDark,
        app_config::ColorScheme::Light => adw::ColorScheme::ForceLight,
    }
}

fn gnome_interface_settings() -> Option<gio::Settings> {
    let schema = gio::SettingsSchemaSource::default()?.lookup(GNOME_INTERFACE_SCHEMA, true)?;
    if !schema.has_key(GNOME_COLOR_SCHEME_KEY) {
        return None;
    }

    Some(gio::Settings::new_full(
        &schema,
        None::<&gio::SettingsBackend>,
        None::<&str>,
    ))
}

fn gnome_prefers_dark_from_raw(raw: &str) -> Option<bool> {
    match raw {
        "prefer-dark" => Some(true),
        "default" | "prefer-light" => Some(false),
        _ => None,
    }
}

fn gnome_prefers_dark(settings: &gio::Settings) -> Option<bool> {
    gnome_prefers_dark_from_raw(settings.string(GNOME_COLOR_SCHEME_KEY).as_str())
}

#[cfg(test)]
fn gtk_system_prefers_dark_from_raw(raw: Option<i32>) -> Option<bool> {
    match raw {
        Some(value) if value == gtk::ffi::GTK_INTERFACE_COLOR_SCHEME_DARK => Some(true),
        Some(value)
            if value == gtk::ffi::GTK_INTERFACE_COLOR_SCHEME_LIGHT
                || value == gtk::ffi::GTK_INTERFACE_COLOR_SCHEME_DEFAULT =>
        {
            Some(false)
        }
        Some(value) if value == gtk::ffi::GTK_INTERFACE_COLOR_SCHEME_UNSUPPORTED => None,
        Some(_) => Some(false),
        None => None,
    }
}

fn resolve_system_prefers_dark(
    portal_color_scheme_preference: PortalColorSchemePreference,
    gnome_interface_settings: Option<&gio::Settings>,
) -> Option<bool> {
    resolved_system_prefers_dark(
        portal_color_scheme_preference,
        gnome_interface_settings.and_then(gnome_prefers_dark),
    )
}

fn resolved_system_prefers_dark(
    portal_color_scheme_preference: PortalColorSchemePreference,
    gnome_prefers_dark: Option<bool>,
) -> Option<bool> {
    portal_color_scheme_preference.resolved(gnome_prefers_dark)
}

fn portal_color_scheme_preference_from_response(
    response: &glib::Variant,
) -> Option<PortalColorSchemePreference> {
    let value = response.try_child_get::<glib::Variant>(0).ok().flatten()?;
    PortalColorSchemePreference::from_raw(value.try_get::<u32>().ok()?)
}

fn portal_setting_changed_preference(
    parameters: &glib::Variant,
) -> Option<PortalColorSchemePreference> {
    let (namespace, key, value) = parameters
        .try_get::<(String, String, glib::Variant)>()
        .ok()?;
    if namespace != PORTAL_APPEARANCE_NAMESPACE || key != PORTAL_COLOR_SCHEME_KEY {
        return None;
    }

    PortalColorSchemePreference::from_raw(value.try_get::<u32>().ok()?)
}

fn sync_system_prefers_dark_change(
    state: &State,
    style_manager: &adw::StyleManager,
    system_prefers_dark: &Cell<Option<bool>>,
    updated_preference: Option<bool>,
) {
    if updated_preference == system_prefers_dark.get() {
        return;
    }

    system_prefers_dark.set(updated_preference);
    sync_ghostty_color_scheme_for_config(
        style_manager,
        updated_preference,
        &state.borrow().config.borrow().appearance,
    );
}

fn sync_portal_color_scheme_preference_change(
    state: &State,
    style_manager: &adw::StyleManager,
    system_prefers_dark: &Cell<Option<bool>>,
    portal_color_scheme_preference: &Cell<PortalColorSchemePreference>,
    gnome_interface_settings: Option<&gio::Settings>,
    updated_preference: PortalColorSchemePreference,
) {
    if updated_preference == portal_color_scheme_preference.get() {
        return;
    }

    portal_color_scheme_preference.set(updated_preference);
    let resolved_preference =
        resolve_system_prefers_dark(updated_preference, gnome_interface_settings);
    sync_system_prefers_dark_change(
        state,
        style_manager,
        system_prefers_dark,
        resolved_preference,
    );
}

fn connect_portal_appearance_watch_async(
    gnome_interface_settings: Option<gio::Settings>,
    state: State,
    style_manager: adw::StyleManager,
    system_prefers_dark: Rc<Cell<Option<bool>>>,
    portal_color_scheme_preference: Rc<Cell<PortalColorSchemePreference>>,
) {
    gio::DBusProxy::for_bus(
        gio::BusType::Session,
        gio::DBusProxyFlags::NONE,
        None::<&gio::DBusInterfaceInfo>,
        PORTAL_DESKTOP_SERVICE,
        PORTAL_DESKTOP_PATH,
        PORTAL_SETTINGS_INTERFACE,
        None::<&gio::Cancellable>,
        move |result| {
            let Ok(proxy) = result else {
                return;
            };

            read_portal_appearance_preference_async(
                &proxy,
                gnome_interface_settings.clone(),
                state.clone(),
                style_manager.clone(),
                system_prefers_dark.clone(),
                portal_color_scheme_preference.clone(),
            );

            let subscription = connect_portal_appearance_watch(
                &proxy,
                gnome_interface_settings.clone(),
                state.clone(),
                style_manager.clone(),
                system_prefers_dark.clone(),
                portal_color_scheme_preference.clone(),
            );
            state.borrow_mut()._theme_portal_signal = subscription;
        },
    );
}

fn read_portal_appearance_preference_async(
    proxy: &gio::DBusProxy,
    gnome_interface_settings: Option<gio::Settings>,
    state: State,
    style_manager: adw::StyleManager,
    system_prefers_dark: Rc<Cell<Option<bool>>>,
    portal_color_scheme_preference: Rc<Cell<PortalColorSchemePreference>>,
) {
    let params = (PORTAL_APPEARANCE_NAMESPACE, PORTAL_COLOR_SCHEME_KEY).to_variant();
    proxy.call(
        "Read",
        Some(&params),
        gio::DBusCallFlags::NONE,
        PORTAL_THEME_READ_TIMEOUT_MS,
        None::<&gio::Cancellable>,
        move |result| {
            let Ok(response) = result else {
                return;
            };
            let Some(updated_preference) = portal_color_scheme_preference_from_response(&response)
            else {
                return;
            };
            sync_portal_color_scheme_preference_change(
                &state,
                &style_manager,
                system_prefers_dark.as_ref(),
                portal_color_scheme_preference.as_ref(),
                gnome_interface_settings.as_ref(),
                updated_preference,
            );
        },
    );
}

fn connect_portal_appearance_watch(
    proxy: &gio::DBusProxy,
    gnome_interface_settings: Option<gio::Settings>,
    state: State,
    style_manager: adw::StyleManager,
    system_prefers_dark: Rc<Cell<Option<bool>>>,
    portal_color_scheme_preference: Rc<Cell<PortalColorSchemePreference>>,
) -> Option<gio::SignalSubscription> {
    let connection = proxy.connection();
    Some(connection.subscribe_to_signal(
        Some(PORTAL_DESKTOP_SERVICE),
        Some(PORTAL_SETTINGS_INTERFACE),
        Some("SettingChanged"),
        Some(PORTAL_DESKTOP_PATH),
        Some(PORTAL_APPEARANCE_NAMESPACE),
        gio::DBusSignalFlags::NONE,
        move |signal| {
            let Some(updated_preference) = portal_setting_changed_preference(signal.parameters)
            else {
                return;
            };

            sync_portal_color_scheme_preference_change(
                &state,
                &style_manager,
                system_prefers_dark.as_ref(),
                portal_color_scheme_preference.as_ref(),
                gnome_interface_settings.as_ref(),
                updated_preference,
            );
        },
    ))
}

fn connect_desktop_notification_watch_async(state: State) {
    gio::DBusProxy::for_bus(
        gio::BusType::Session,
        gio::DBusProxyFlags::NONE,
        None::<&gio::DBusInterfaceInfo>,
        FREEDESKTOP_NOTIFICATIONS_SERVICE,
        FREEDESKTOP_NOTIFICATIONS_PATH,
        FREEDESKTOP_NOTIFICATIONS_INTERFACE,
        None::<&gio::Cancellable>,
        move |result| {
            let Ok(proxy) = result else {
                return;
            };

            let token_subscription =
                connect_desktop_notification_token_watch(&proxy, state.clone());
            let action_subscription =
                connect_desktop_notification_action_watch(&proxy, state.clone());
            let closed_subscription =
                connect_desktop_notification_closed_watch(&proxy, state.clone());
            let mut s = state.borrow_mut();
            s._desktop_notification_token_signal = token_subscription;
            s._desktop_notification_action_signal = action_subscription;
            s._desktop_notification_closed_signal = closed_subscription;
        },
    );
}

fn desktop_notification_id_from_response(response: &glib::Variant) -> Option<u32> {
    response
        .try_child_get::<u32>(0)
        .ok()
        .flatten()
        .or_else(|| response.try_get::<u32>().ok())
}

fn desktop_notification_action_from_signal(parameters: &glib::Variant) -> Option<(u32, String)> {
    parameters.try_get::<(u32, String)>().ok()
}

fn desktop_notification_activation_token_from_signal(
    parameters: &glib::Variant,
) -> Option<(u32, String)> {
    parameters.try_get::<(u32, String)>().ok()
}

fn desktop_notification_closed_id_from_signal(parameters: &glib::Variant) -> Option<u32> {
    parameters.try_get::<(u32, u32)>().ok().map(|(id, _)| id)
}

fn connect_desktop_notification_token_watch(
    proxy: &gio::DBusProxy,
    state: State,
) -> Option<gio::SignalSubscription> {
    let connection = proxy.connection();
    Some(connection.subscribe_to_signal(
        Some(FREEDESKTOP_NOTIFICATIONS_SERVICE),
        Some(FREEDESKTOP_NOTIFICATIONS_INTERFACE),
        Some("ActivationToken"),
        Some(FREEDESKTOP_NOTIFICATIONS_PATH),
        None,
        gio::DBusSignalFlags::NONE,
        move |signal| {
            let Some((notification_id, activation_token)) =
                desktop_notification_activation_token_from_signal(signal.parameters)
            else {
                return;
            };

            let mut s = state.borrow_mut();
            if let Some(route) = s.desktop_notification_routes.get_mut(&notification_id) {
                route.activation_token = Some(activation_token);
            }
        },
    ))
}

fn connect_desktop_notification_action_watch(
    proxy: &gio::DBusProxy,
    state: State,
) -> Option<gio::SignalSubscription> {
    let connection = proxy.connection();
    Some(connection.subscribe_to_signal(
        Some(FREEDESKTOP_NOTIFICATIONS_SERVICE),
        Some(FREEDESKTOP_NOTIFICATIONS_INTERFACE),
        Some("ActionInvoked"),
        Some(FREEDESKTOP_NOTIFICATIONS_PATH),
        None,
        gio::DBusSignalFlags::NONE,
        move |signal| {
            let Some((notification_id, action_key)) =
                desktop_notification_action_from_signal(signal.parameters)
            else {
                return;
            };

            if action_key != "default" {
                return;
            }

            let route = {
                let mut s = state.borrow_mut();
                s.desktop_notification_routes.remove(&notification_id)
            };
            let Some(route) = route else {
                return;
            };

            activate_desktop_notification_target(
                &state,
                &route.target,
                route.activation_token.as_deref(),
            );
        },
    ))
}

fn connect_desktop_notification_closed_watch(
    proxy: &gio::DBusProxy,
    state: State,
) -> Option<gio::SignalSubscription> {
    let connection = proxy.connection();
    Some(connection.subscribe_to_signal(
        Some(FREEDESKTOP_NOTIFICATIONS_SERVICE),
        Some(FREEDESKTOP_NOTIFICATIONS_INTERFACE),
        Some("NotificationClosed"),
        Some(FREEDESKTOP_NOTIFICATIONS_PATH),
        None,
        gio::DBusSignalFlags::NONE,
        move |signal| {
            let Some(notification_id) =
                desktop_notification_closed_id_from_signal(signal.parameters)
            else {
                return;
            };

            state
                .borrow_mut()
                .desktop_notification_routes
                .remove(&notification_id);
        },
    ))
}

fn activate_desktop_notification_target(
    state: &State,
    target: &DesktopNotificationTarget,
    activation_token: Option<&str>,
) {
    let (workspace_idx, row, sidebar_list, window, workspace_changed) = {
        let s = state.borrow();
        let Some((idx, workspace)) = s
            .workspaces
            .iter()
            .enumerate()
            .find(|(_, workspace)| workspace.id == target.workspace_id)
        else {
            return;
        };

        (
            idx,
            workspace.sidebar_row.clone(),
            s.sidebar_list.clone(),
            s.window.clone(),
            idx != s.active_idx,
        )
    };

    if let Some(token) = activation_token.filter(|token| !token.is_empty()) {
        window.set_startup_id(token);
    }
    window.present();
    switch_workspace(state, workspace_idx);
    sidebar_list.select_row(Some(&row));

    let state_for_focus = state.clone();
    let target_for_focus = target.clone();
    if workspace_changed {
        glib::idle_add_local_once(move || {
            glib::idle_add_local_once(move || {
                focus_desktop_notification_target(&state_for_focus, &target_for_focus);
            });
        });
    } else {
        glib::idle_add_local_once(move || {
            focus_desktop_notification_target(&state_for_focus, &target_for_focus);
        });
    }
}

fn focus_desktop_notification_target(state: &State, target: &DesktopNotificationTarget) -> bool {
    let workspace = {
        let s = state.borrow();
        s.workspaces
            .iter()
            .find(|workspace| workspace.id == target.workspace_id)
            .map(|workspace| (workspace.root.clone(), workspace.split_container.clone()))
    };

    if let Some(pane_id) = target.pane_id {
        if let Some(pane_widget) = pane::find_pane_widget_by_id(pane_id) {
            if let Some((root, container)) = workspace.as_ref() {
                if !pane_widget.is_ancestor(root) {
                    if let Some(tab_id) = target.tab_id.as_deref() {
                        pane::activate_tab_in_pane(&pane_widget, tab_id);
                    }
                    if container.reveal_pane(&pane_widget) {
                        return true;
                    }
                }
            }
            if let Some(tab_id) = target.tab_id.as_deref() {
                if pane::activate_tab_in_pane(&pane_widget, tab_id) {
                    return true;
                }
            }

            if pane::focus_active_tab_in_pane(&pane_widget) {
                return true;
            }
        }
    }

    if let Some((root, _)) = workspace {
        focus_workspace_entrypoint(&root);
        return true;
    }

    false
}

fn connect_gnome_appearance_watch(
    settings: &gio::Settings,
    state: State,
    style_manager: adw::StyleManager,
    system_prefers_dark: Rc<Cell<Option<bool>>>,
    portal_color_scheme_preference: Rc<Cell<PortalColorSchemePreference>>,
) -> glib::SignalHandlerId {
    settings.connect_changed(Some(GNOME_COLOR_SCHEME_KEY), move |settings, _| {
        let updated_preference =
            resolve_system_prefers_dark(portal_color_scheme_preference.get(), Some(settings));
        sync_system_prefers_dark_change(
            &state,
            &style_manager,
            system_prefers_dark.as_ref(),
            updated_preference,
        );
    })
}

fn ghostty_prefers_dark(
    scheme: app_config::ColorScheme,
    system_prefers_dark: Option<bool>,
    fallback_dark: bool,
) -> bool {
    match scheme {
        app_config::ColorScheme::Dark => true,
        app_config::ColorScheme::Light => false,
        app_config::ColorScheme::System => system_prefers_dark.unwrap_or(fallback_dark),
    }
}

fn sync_ghostty_color_scheme_for_config(
    style_manager: &adw::StyleManager,
    system_prefers_dark: Option<bool>,
    appearance: &app_config::AppearanceConfig,
) {
    let dark = ghostty_prefers_dark(
        appearance.ghostty_color_scheme,
        system_prefers_dark,
        style_manager.is_dark(),
    );
    crate::terminal::sync_color_scheme(dark);
}

fn apply_appearance(
    style_manager: &adw::StyleManager,
    system_prefers_dark: Option<bool>,
    appearance: &app_config::AppearanceConfig,
) {
    style_manager.set_color_scheme(adw_color_scheme_for(appearance.color_scheme));
    sync_ghostty_color_scheme_for_config(style_manager, system_prefers_dark, appearance);
}

fn open_keybind_editor_tab(state: &State, pane_widget: &gtk::Widget) {
    let shortcuts = {
        let s = state.borrow();
        s.shortcuts.clone()
    };
    let on_capture: Rc<
        dyn Fn(
            ShortcutId,
            Option<shortcut_config::NormalizedShortcut>,
        ) -> Result<ResolvedShortcutConfig, String>,
    > = {
        let state = state.clone();
        Rc::new(move |id, binding| persist_shortcut_binding(&state, id, binding))
    };
    pane::add_keybind_editor_tab_to_pane(pane_widget, shortcuts, on_capture);
}

fn activate_workspace_shortcut(state: &State, idx: usize) {
    let row_and_list = {
        let s = state.borrow();
        s.workspaces
            .get(idx)
            .map(|ws| (idx, ws.sidebar_row.clone(), s.sidebar_list.clone()))
    };

    if let Some((idx, row, list)) = row_and_list {
        switch_workspace(state, idx);
        list.select_row(Some(&row));
    }
}

fn activate_last_workspace_shortcut(state: &State) {
    let last_idx = {
        let s = state.borrow();
        if s.workspaces.is_empty() {
            return;
        }
        s.workspaces.len() - 1
    };
    activate_workspace_shortcut(state, last_idx);
}

// ---------------------------------------------------------------------------
// Sidebar row
// ---------------------------------------------------------------------------

fn build_sidebar_row(
    name: &str,
    folder_path: Option<&str>,
    show_workspace_path: bool,
) -> (
    gtk::ListBoxRow,
    gtk::Label,
    gtk::Button,
    gtk::Label,
    gtk::Label,
    gtk::Label,
) {
    let notify_dot = gtk::Label::builder().label("\u{25CF}").build();
    notify_dot.add_css_class("limux-notify-dot-hidden");

    let name_label = gtk::Label::builder()
        .label(name)
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    name_label.add_css_class("limux-ws-name");

    let favorite_button = gtk::Button::with_label("\u{2606}");
    favorite_button.add_css_class("flat");
    favorite_button.add_css_class("limux-ws-star-btn");
    favorite_button.set_focus_on_click(false);
    favorite_button.set_valign(gtk::Align::Center);
    favorite_button.set_halign(gtk::Align::End);
    favorite_button.set_tooltip_text(Some("Favorite workspace"));

    let top_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    top_row.append(&notify_dot);
    top_row.append(&name_label);
    top_row.append(&favorite_button);

    let path_label = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .margin_start(8)
        .build();
    path_label.add_css_class("limux-ws-path");
    if let Some(p) = folder_path {
        path_label.set_label(&abbreviate_path(p));
        path_label.set_tooltip_text(Some(p));
    }
    path_label.set_visible(workspace_path_visible(folder_path, show_workspace_path));

    let notify_label = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .visible(false)
        .margin_start(8)
        .build();
    notify_label.add_css_class("limux-notify-msg");

    let vbox = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .build();
    vbox.add_css_class("limux-sidebar-row-box");
    vbox.append(&top_row);
    vbox.append(&path_label);
    vbox.append(&notify_label);

    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&vbox));

    (
        row,
        name_label,
        favorite_button,
        notify_dot,
        notify_label,
        path_label,
    )
}

fn workspace_path_visible(folder_path: Option<&str>, show_workspace_path: bool) -> bool {
    show_workspace_path && folder_path.is_some()
}

fn sync_workspace_path_visibility(state: &State, show_workspace_path: bool) {
    let app_state = state.borrow();
    for workspace in &app_state.workspaces {
        workspace.path_label.set_visible(workspace_path_visible(
            workspace.folder_path.as_deref(),
            show_workspace_path,
        ));
    }
}

/// Abbreviate a path by replacing the home directory with ~.
fn abbreviate_path(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        if path.starts_with(home_str.as_ref()) {
            return format!("~{}", &path[home_str.len()..]);
        }
    }
    path.to_string()
}

// ---------------------------------------------------------------------------
// Workspace management
// ---------------------------------------------------------------------------

fn favorites_prefix_len(flags: &[bool]) -> usize {
    flags.iter().take_while(|is_favorite| **is_favorite).count()
}

#[cfg(test)]
fn workspace_drop_layout_path(layout: &LayoutNodeState) -> Vec<bool> {
    match layout {
        LayoutNodeState::Pane(_) => Vec::new(),
        LayoutNodeState::Split(split) => {
            let mut path = vec![true];
            path.extend(workspace_drop_layout_path(&split.start));
            path
        }
    }
}

fn tab_drag_workspace_seed(
    source: WorkspaceSeedSource,
    title: &str,
    tab_cwd: Option<String>,
) -> TabDragWorkspaceSeed {
    let name = {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            "Workspace".to_string()
        } else {
            trimmed.to_string()
        }
    };
    let cwd = tab_cwd
        .clone()
        .or_else(|| source.workspace_folder_path.clone())
        .or(source.workspace_cwd.clone());
    let folder_path = tab_cwd
        .filter(|cwd| !cwd.trim().is_empty())
        .or(source.workspace_folder_path)
        .filter(|path| !path.trim().is_empty());

    TabDragWorkspaceSeed {
        name,
        cwd,
        folder_path,
    }
}

fn next_active_workspace_index(
    remaining_workspace_ids: &[&str],
    preferred_active_workspace_id: Option<&str>,
    removed_idx: usize,
) -> usize {
    if remaining_workspace_ids.is_empty() {
        return 0;
    }
    if let Some(preferred_id) = preferred_active_workspace_id {
        if let Some(idx) = remaining_workspace_ids
            .iter()
            .position(|workspace_id| *workspace_id == preferred_id)
        {
            return idx;
        }
    }
    removed_idx.min(remaining_workspace_ids.len() - 1)
}

fn show_workspace_context_menu(state: &State, workspace_id: &str, row: &gtk::ListBoxRow) {
    let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    menu_box.set_margin_top(4);
    menu_box.set_margin_bottom(4);
    menu_box.set_margin_start(4);
    menu_box.set_margin_end(4);

    let rename_btn = gtk::Button::with_label("Rename");
    rename_btn.add_css_class("flat");
    let delete_btn = gtk::Button::with_label("Delete");
    delete_btn.add_css_class("flat");
    delete_btn.add_css_class("destructive-action");

    menu_box.append(&rename_btn);
    menu_box.append(&delete_btn);

    let popover = gtk::Popover::new();
    popover.set_child(Some(&menu_box));
    popover.set_parent(row);
    popover.set_position(gtk::PositionType::Right);

    {
        let state = state.clone();
        let ws_id = workspace_id.to_string();
        let pop = popover.clone();
        rename_btn.connect_clicked(move |_| {
            pop.popdown();
            begin_workspace_inline_rename(&state, &ws_id);
        });
    }
    {
        let state = state.clone();
        let ws_id = workspace_id.to_string();
        let pop = popover.clone();
        delete_btn.connect_clicked(move |_| {
            pop.popdown();
            close_workspace_by_id(&state, &ws_id);
            request_session_save(&state);
        });
    }
    {
        popover.connect_closed(move |p| {
            p.unparent();
        });
    }

    popover.popup();
}

fn clamp_workspace_insert_index_for_pinning(
    favorite_flags_after_removal: &[bool],
    moving_is_favorite: bool,
    proposed_index: usize,
) -> usize {
    let favorites_top = favorites_prefix_len(favorite_flags_after_removal);
    if moving_is_favorite {
        proposed_index.min(favorites_top)
    } else {
        proposed_index.max(favorites_top)
    }
}

fn sync_sidebar_row_order(state: &mut AppState) {
    while let Some(child) = state.sidebar_list.first_child() {
        state.sidebar_list.remove(&child);
    }
    for workspace in &state.workspaces {
        state.sidebar_list.append(&workspace.sidebar_row);
    }
}

fn set_workspace_favorite_visual(workspace: &Workspace) {
    let symbol = if workspace.favorite {
        "\u{2605}"
    } else {
        "\u{2606}"
    };
    workspace.favorite_button.set_label(symbol);
    if workspace.favorite {
        workspace
            .favorite_button
            .add_css_class("limux-ws-star-btn-active");
    } else {
        workspace
            .favorite_button
            .remove_css_class("limux-ws-star-btn-active");
    }
}

/// Find an active rename Entry in the sidebar (if any).
fn find_active_rename_entry(sidebar_list: &gtk::ListBox) -> Option<gtk::Entry> {
    fn find_entry(widget: &gtk::Widget) -> Option<gtk::Entry> {
        if let Some(entry) = widget.downcast_ref::<gtk::Entry>() {
            return Some(entry.clone());
        }
        let mut child = widget.first_child();
        while let Some(c) = child {
            if let Some(entry) = find_entry(&c) {
                return Some(entry);
            }
            child = c.next_sibling();
        }
        None
    }
    let mut row = sidebar_list.first_child();
    while let Some(r) = row {
        if let Some(entry) = find_entry(&r) {
            return Some(entry);
        }
        row = r.next_sibling();
    }
    None
}

fn commit_inline_rename_for_click(
    translated_click: Option<(f64, f64)>,
    entry_width: i32,
    entry_height: i32,
    commit: impl FnOnce(),
) {
    let click_is_inside = translated_click.is_some_and(|(x, y)| {
        x >= 0.0 && y >= 0.0 && x <= entry_width as f64 && y <= entry_height as f64
    });
    if !click_is_inside {
        commit();
    }
}

fn begin_workspace_inline_rename(state: &State, workspace_id: &str) {
    let (label, current_name) = {
        let s = state.borrow();
        let Some(workspace) = s
            .workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
        else {
            return;
        };
        (workspace.name_label.clone(), workspace.name.clone())
    };

    let Some(parent) = label.parent().and_then(|p| p.downcast::<gtk::Box>().ok()) else {
        return;
    };

    // Avoid stacking multiple rename entries if the user right-clicks repeatedly.
    let mut child = parent.first_child();
    while let Some(widget) = child {
        if widget.is::<gtk::Entry>() {
            return;
        }
        child = widget.next_sibling();
    }

    let entry = gtk::Entry::builder()
        .text(&current_name)
        .hexpand(true)
        .build();
    for css_class in WORKSPACE_RENAME_ENTRY_CSS_CLASSES {
        entry.add_css_class(css_class);
    }

    label.set_visible(false);
    parent.insert_child_after(&entry, Some(&label));
    entry.grab_focus();
    entry.select_region(0, -1);

    let commit_guard = Rc::new(std::cell::Cell::new(false));
    let state_for_commit = state.clone();
    let workspace_id = workspace_id.to_string();
    let label_for_commit = label.clone();
    let parent_for_commit = parent.clone();
    let commit = {
        let commit_guard = commit_guard.clone();
        move |entry: &gtk::Entry| {
            if commit_guard.get() {
                return;
            }
            commit_guard.set(true);

            let next_name = entry.text().trim().to_string();
            if !next_name.is_empty() {
                label_for_commit.set_label(&next_name);
                let mut s = state_for_commit.borrow_mut();
                if let Some(workspace) = s
                    .workspaces
                    .iter_mut()
                    .find(|workspace| workspace.id == workspace_id)
                {
                    workspace.name = next_name;
                }
                drop(s);
                request_session_save(&state_for_commit);
            }

            label_for_commit.set_visible(true);
            parent_for_commit.remove(entry);
        }
    };

    {
        let commit = commit.clone();
        entry.connect_activate(move |entry| {
            commit(entry);
        });
    }
    {
        let commit = commit.clone();
        let focus = gtk::EventControllerFocus::new();
        focus.connect_leave(move |controller| {
            if let Some(widget) = controller.widget() {
                if let Some(entry) = widget.downcast_ref::<gtk::Entry>() {
                    commit(entry);
                }
            }
        });
        entry.add_controller(focus);
    }
}

fn reorder_workspace_by_id(
    state: &State,
    source_id: &str,
    target_id: &str,
    drop_below: bool,
) -> bool {
    let (sidebar_list, row_to_select) = {
        let mut s = state.borrow_mut();
        let Some(source_idx) = s
            .workspaces
            .iter()
            .position(|workspace| workspace.id == source_id)
        else {
            return false;
        };
        let Some(target_idx) = s
            .workspaces
            .iter()
            .position(|workspace| workspace.id == target_id)
        else {
            return false;
        };
        if source_idx == target_idx {
            return false;
        }

        let active_workspace_id = s.active_workspace().map(|workspace| workspace.id.clone());
        let moving_workspace = s.workspaces.remove(source_idx);
        let Some(target_idx_after_removal) = s
            .workspaces
            .iter()
            .position(|workspace| workspace.id == target_id)
        else {
            s.workspaces.insert(source_idx, moving_workspace);
            return false;
        };

        // Insert after the target when dropping on the bottom half
        let raw_insert_idx = if drop_below {
            target_idx_after_removal + 1
        } else {
            target_idx_after_removal
        };

        let favorite_flags: Vec<bool> = s
            .workspaces
            .iter()
            .map(|workspace| workspace.favorite)
            .collect();
        let insert_idx = clamp_workspace_insert_index_for_pinning(
            &favorite_flags,
            moving_workspace.favorite,
            raw_insert_idx,
        );
        s.workspaces.insert(insert_idx, moving_workspace);

        if let Some(active_workspace_id) = active_workspace_id {
            if let Some(new_active_idx) = s
                .workspaces
                .iter()
                .position(|workspace| workspace.id == active_workspace_id)
            {
                s.active_idx = new_active_idx;
            }
        }

        sync_sidebar_row_order(&mut s);
        let row_to_select = s
            .workspaces
            .get(s.active_idx)
            .map(|workspace| workspace.sidebar_row.clone());
        (s.sidebar_list.clone(), row_to_select)
    };

    if let Some(row) = row_to_select {
        sidebar_list.select_row(Some(&row));
    }
    request_session_save(state);

    true
}

fn toggle_workspace_favorite(state: &State, workspace_id: &str) {
    let (sidebar_list, row_to_select) = {
        let mut s = state.borrow_mut();
        let Some(idx) = s
            .workspaces
            .iter()
            .position(|workspace| workspace.id == workspace_id)
        else {
            return;
        };

        let active_workspace_id = s.active_workspace().map(|workspace| workspace.id.clone());
        s.workspaces[idx].favorite = !s.workspaces[idx].favorite;
        set_workspace_favorite_visual(&s.workspaces[idx]);

        let workspace = s.workspaces.remove(idx);
        let favorite_flags: Vec<bool> = s
            .workspaces
            .iter()
            .map(|candidate| candidate.favorite)
            .collect();
        let insert_idx = favorites_prefix_len(&favorite_flags);
        s.workspaces.insert(insert_idx, workspace);

        if let Some(active_workspace_id) = active_workspace_id {
            if let Some(new_active_idx) = s
                .workspaces
                .iter()
                .position(|workspace| workspace.id == active_workspace_id)
            {
                s.active_idx = new_active_idx;
            }
        }

        sync_sidebar_row_order(&mut s);
        let row_to_select = s
            .workspaces
            .get(s.active_idx)
            .map(|workspace| workspace.sidebar_row.clone());
        (s.sidebar_list.clone(), row_to_select)
    };

    if let Some(row) = row_to_select {
        sidebar_list.select_row(Some(&row));
    }
    request_session_save(state);
}

fn handle_tab_drop_to_workspace(state: &State, target_workspace_id: &str, payload: &str) -> bool {
    let Some((pane_id, tab_id)) = payload.split_once(':') else {
        return false;
    };
    let Ok(source_pane_id) = pane_id.parse::<u32>() else {
        return false;
    };
    let Some(source_pane) = pane::find_pane_widget_by_id(source_pane_id) else {
        return false;
    };

    let target_pane = {
        let app_state = state.borrow();
        let Some(workspace) = app_state
            .workspaces
            .iter()
            .find(|workspace| workspace.id == target_workspace_id)
        else {
            return false;
        };
        find_leaf_pane(&workspace.root, gtk::Orientation::Horizontal, true)
    };

    pane::move_tab_to_pane(&source_pane, tab_id, &target_pane)
}

fn create_workspace_for_tab(state: &State, payload: &str) -> bool {
    let Some((pane_id, tab_id)) = payload.split_once(':') else {
        return false;
    };
    let Ok(source_pane_id) = pane_id.parse::<u32>() else {
        return false;
    };
    let Some(source_pane) = pane::find_pane_widget_by_id(source_pane_id) else {
        return false;
    };

    let Some(title) = pane::tab_title(&source_pane, tab_id) else {
        return false;
    };
    let tab_cwd = pane::tab_working_directory(&source_pane, tab_id);
    let seed = {
        let app_state = state.borrow();
        let source = app_state
            .workspace_for_widget(&source_pane)
            .map(|workspace| WorkspaceSeedSource {
                workspace_cwd: workspace.cwd.borrow().clone(),
                workspace_folder_path: workspace.folder_path.clone(),
            })
            .unwrap_or(WorkspaceSeedSource {
                workspace_cwd: None,
                workspace_folder_path: None,
            });
        tab_drag_workspace_seed(source, &title, tab_cwd)
    };
    let previous_active_workspace_id = {
        let app_state = state.borrow();
        app_state
            .active_workspace()
            .map(|workspace| workspace.id.clone())
    };

    let shortcuts = {
        let app_state = state.borrow();
        app_state.shortcuts.clone()
    };
    let new_workspace_id = uuid::Uuid::new_v4().to_string();
    let stack_name = format!("ws-{new_workspace_id}");
    let pane = create_pane_for_workspace(
        state,
        &shortcuts,
        &new_workspace_id,
        seed.cwd.as_deref(),
        None,
        true,
    );
    let split_container = SplitTreeContainer::new(state, pane.clone().upcast());
    let root = split_container.widget().clone();

    let show_workspace_path = state
        .borrow()
        .config
        .borrow()
        .appearance
        .show_workspace_path;
    let (row, name_label, favorite_button, notify_dot, notify_label, path_label) =
        build_sidebar_row(&seed.name, seed.folder_path.as_deref(), show_workspace_path);
    let row_clone = row.clone();
    {
        let mut app_state = state.borrow_mut();
        app_state.stack.add_named(&root, Some(&stack_name));
        app_state.sidebar_list.append(&row);
        install_workspace_row_interactions(state, &new_workspace_id, &row, &favorite_button);

        app_state.workspaces.push(Workspace {
            id: new_workspace_id.clone(),
            name: seed.name.clone(),
            root: root.clone().upcast(),
            split_container,
            sidebar_row: row,
            name_label,
            favorite_button,
            notify_dot,
            notify_label,
            unread: false,
            favorite: false,
            cwd: Rc::new(RefCell::new(seed.cwd.clone())),
            folder_path: seed.folder_path.clone(),
            path_label,
        });
        app_state.active_idx = app_state.workspaces.len() - 1;
        app_state.stack.set_visible_child_name(&stack_name);
    }

    {
        let sidebar_list = state.borrow().sidebar_list.clone();
        sidebar_list.select_row(Some(&row_clone));
    }

    if pane::move_tab_to_pane(&source_pane, tab_id, &pane.clone().upcast()) {
        request_session_save(state);
        return true;
    }
    close_workspace_by_id_internal(
        state,
        &new_workspace_id,
        false,
        previous_active_workspace_id.as_deref(),
    );
    false
}

fn install_workspace_row_interactions(
    state: &State,
    workspace_id: &str,
    row: &gtk::ListBoxRow,
    favorite_button: &gtk::Button,
) {
    let right_click = gtk::GestureClick::new();
    right_click.set_button(3);
    {
        let state = state.clone();
        let workspace_id = workspace_id.to_string();
        let r = row.clone();
        right_click.connect_pressed(move |_, _, _, _| {
            show_workspace_context_menu(&state, &workspace_id, &r);
        });
    }
    row.add_controller(right_click);

    let drag_source = gtk::DragSource::new();
    drag_source.set_actions(gtk::gdk::DragAction::MOVE);
    {
        let workspace_id = workspace_id.to_string();
        drag_source.connect_prepare(move |_, _, _| {
            let payload = glib::Value::from(&workspace_id);
            Some(gtk::gdk::ContentProvider::for_value(&payload))
        });
    }
    {
        let state = state.clone();
        let row = row.clone();
        let workspace_id = workspace_id.to_string();
        drag_source.connect_drag_begin(move |source, _| {
            let mut s = state.borrow_mut();
            s.workspace_dragging = Some(workspace_id.clone());
            s.new_ws_btn.set_label("\u{1F5D1}\u{FE0E}");
            s.new_ws_btn.add_css_class("limux-sidebar-btn-trash");
            drop(s);
            pane::set_workspace_dragging_all(true);
            let icon = gtk::WidgetPaintable::new(Some(&row));
            source.set_icon(Some(&icon), 0, 0);
        });
    }
    {
        let state = state.clone();
        drag_source.connect_drag_end(move |_, _, _| {
            let mut s = state.borrow_mut();
            s.workspace_dragging = None;
            s.new_ws_btn.set_label("New Workspace");
            s.new_ws_btn.remove_css_class("limux-sidebar-btn-trash");
            s.new_ws_btn
                .remove_css_class("limux-sidebar-btn-trash-hover");
            pane::set_workspace_dragging_all(false);
        });
    }
    row.add_controller(drag_source);

    let drop_target = gtk::DropTarget::new(glib::Type::STRING, gtk::gdk::DragAction::MOVE);
    drop_target.set_preload(true);
    let hover_timer: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
    let drop_handled = Rc::new(Cell::new(false));
    {
        let r = row.clone();
        let state = state.clone();
        let hover_timer = hover_timer.clone();
        let target_workspace_id = workspace_id.to_string();
        let drop_handled = drop_handled.clone();
        drop_target.connect_motion(move |_, _x, y| {
            drop_handled.set(false);
            let h = r.height() as f64;
            r.remove_css_class("limux-drop-above");
            r.remove_css_class("limux-drop-below");
            r.remove_css_class("limux-tab-drop-target");

            let dragged_workspace = state.borrow().workspace_dragging.clone();
            match dragged_workspace {
                Some(ref dragged_workspace_id) if dragged_workspace_id != &target_workspace_id => {
                    if y < h / 2.0 {
                        r.add_css_class("limux-drop-above");
                    } else {
                        r.add_css_class("limux-drop-below");
                    }
                }
                None => {
                    r.add_css_class("limux-tab-drop-target");
                }
                _ => {}
            }

            if hover_timer.borrow().is_none() {
                let state = state.clone();
                let target_workspace_id = target_workspace_id.clone();
                let hover_timer = hover_timer.clone();
                let drop_handled = drop_handled.clone();
                let timer_for_callback = hover_timer.clone();
                let source = glib::timeout_add_local_once(
                    std::time::Duration::from_millis(500),
                    move || {
                        *timer_for_callback.borrow_mut() = None;
                        if drop_handled.get() {
                            return;
                        }
                        let (target_idx, sidebar_row, sidebar_list) = {
                            let app_state = state.borrow();
                            let idx = app_state
                                .workspaces
                                .iter()
                                .position(|workspace| workspace.id == target_workspace_id);
                            let sidebar_row = idx.and_then(|idx| {
                                app_state
                                    .workspaces
                                    .get(idx)
                                    .map(|workspace| workspace.sidebar_row.clone())
                            });
                            (idx, sidebar_row, app_state.sidebar_list.clone())
                        };
                        if let Some(target_idx) = target_idx {
                            switch_workspace(&state, target_idx);
                        }
                        if let Some(sidebar_row) = sidebar_row {
                            sidebar_list.select_row(Some(&sidebar_row));
                        }
                    },
                );
                *hover_timer.borrow_mut() = Some(source);
            }
            gtk::gdk::DragAction::MOVE
        });
    }
    {
        let r = row.clone();
        let hover_timer = hover_timer.clone();
        drop_target.connect_leave(move |_| {
            r.remove_css_class("limux-drop-above");
            r.remove_css_class("limux-drop-below");
            r.remove_css_class("limux-tab-drop-target");
            if let Some(source) = hover_timer.borrow_mut().take() {
                source.remove();
            }
        });
    }
    {
        let state = state.clone();
        let target_workspace_id = workspace_id.to_string();
        let r = row.clone();
        let hover_timer = hover_timer.clone();
        let drop_handled = drop_handled.clone();
        drop_target.connect_drop(move |_dt, value, _, y| {
            drop_handled.set(true);
            r.remove_css_class("limux-drop-above");
            r.remove_css_class("limux-drop-below");
            r.remove_css_class("limux-tab-drop-target");
            if let Some(source) = hover_timer.borrow_mut().take() {
                source.remove();
            }
            if let Ok(payload) = value.get::<String>() {
                if payload.contains(':') {
                    return handle_tab_drop_to_workspace(&state, &target_workspace_id, &payload);
                }
                let drop_below = y >= r.height() as f64 / 2.0;
                if payload != target_workspace_id {
                    return reorder_workspace_by_id(
                        &state,
                        &payload,
                        &target_workspace_id,
                        drop_below,
                    );
                }
            }
            false
        });
    }
    row.add_controller(drop_target);

    {
        let state = state.clone();
        let workspace_id = workspace_id.to_string();
        favorite_button.connect_clicked(move |_| {
            toggle_workspace_favorite(&state, &workspace_id);
        });
    }
}

fn add_workspace(state: &State, _working_directory: Option<&str>) {
    show_workspace_path_dialog(state);
}

fn active_window(state: &State) -> Option<gtk::Window> {
    let s = state.borrow();
    s.stack
        .root()
        .and_then(|root| root.downcast::<gtk::Window>().ok())
}

fn show_workspace_path_dialog(state: &State) {
    let dialog = gtk::Window::builder()
        .title("Open Folder as Workspace")
        .modal(true)
        .default_width(520)
        .build();
    if let Some(window) = active_window(state) {
        dialog.set_transient_for(Some(&window));
    }

    let default_folder = dirs::home_dir()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("/"));
    let entry = gtk::Entry::builder()
        .text(default_folder.to_string_lossy())
        .hexpand(true)
        .activates_default(true)
        .build();
    let browse_button = gtk::Button::with_label("Browse...");
    let error_label = gtk::Label::builder()
        .halign(gtk::Align::Start)
        .visible(false)
        .wrap(true)
        .build();
    error_label.add_css_class("error");

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    let path_row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .build();
    path_row.append(&entry);
    path_row.append(&browse_button);
    content.append(&path_row);
    content.append(&error_label);

    let buttons = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .halign(gtk::Align::End)
        .spacing(8)
        .build();
    let cancel_button = gtk::Button::with_label("Cancel");
    let open_button = gtk::Button::with_label("Open");
    open_button.add_css_class("suggested-action");
    buttons.append(&cancel_button);
    buttons.append(&open_button);
    content.append(&buttons);
    dialog.set_child(Some(&content));

    entry.grab_focus();
    entry.select_region(0, -1);
    let state_for_open = state.clone();
    let entry_for_open = entry.clone();
    let error_label_for_open = error_label.clone();
    let dialog_for_open = dialog.clone();
    open_button.connect_clicked(move |_| {
        match validate_workspace_folder_input(entry_for_open.text().as_str()) {
            Ok(selection) => {
                create_workspace_with_folder(
                    &state_for_open,
                    &selection.name,
                    selection.path_text.as_str(),
                );
                dialog_for_open.close();
            }
            Err(message) => {
                error_label_for_open.set_label(&message);
                error_label_for_open.set_visible(true);
                entry_for_open.grab_focus();
            }
        }
    });

    let open_button_for_entry = open_button.clone();
    entry.connect_activate(move |_| {
        open_button_for_entry.emit_clicked();
    });

    let entry_for_browse = entry.clone();
    let error_label_for_browse = error_label.clone();
    let browse_button_for_browse = browse_button.clone();
    let transient_for_browse = active_window(state);
    browse_button.connect_clicked(move |_| {
        error_label_for_browse.set_visible(false);
        browse_button_for_browse.set_sensitive(false);

        let picker = gtk::FileDialog::builder()
            .title("Choose Workspace Folder")
            .accept_label("Choose")
            .modal(true)
            .build();

        if let Ok(selection) = validate_workspace_folder_input(entry_for_browse.text().as_str()) {
            picker.set_initial_folder(Some(&gio::File::for_path(selection.path_text)));
        }

        let entry_for_result = entry_for_browse.clone();
        let error_label_for_result = error_label_for_browse.clone();
        let browse_button_for_result = browse_button_for_browse.clone();
        picker.select_folder(
            transient_for_browse.as_ref(),
            None::<&gio::Cancellable>,
            move |result| {
                browse_button_for_result.set_sensitive(true);
                match result {
                    Ok(file) => {
                        if let Some(path) = file.path() {
                            entry_for_result.set_text(&path.to_string_lossy());
                            entry_for_result.grab_focus();
                            entry_for_result.set_position(-1);
                        }
                    }
                    Err(err) if is_workspace_picker_cancel(&err) => {}
                    Err(err) => {
                        error_label_for_result.set_label(&format!("Folder picker failed: {err}"));
                        error_label_for_result.set_visible(true);
                    }
                }
            },
        );
    });

    let dialog_for_cancel = dialog.clone();
    cancel_button.connect_clicked(move |_| {
        dialog_for_cancel.close();
    });

    // ESC key handler to close the dialog
    let dialog_for_key = dialog.clone();
    let key_controller = gtk::EventControllerKey::new();
    key_controller.connect_key_pressed(move |_controller, keyval, _keycode, _modifier| {
        if keyval == gtk::gdk::Key::Escape {
            dialog_for_key.close();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    dialog.add_controller(key_controller);

    dialog.present();
}

fn is_workspace_picker_cancel(err: &glib::Error) -> bool {
    matches!(
        err.kind::<gtk::DialogError>(),
        Some(gtk::DialogError::Cancelled | gtk::DialogError::Dismissed)
    )
}

#[derive(Debug)]
struct WorkspaceFolderSelection {
    name: String,
    path_text: String,
}

fn validate_workspace_folder_input(input: &str) -> Result<WorkspaceFolderSelection, String> {
    let home_dir = dirs::home_dir();
    let current_dir = std::env::current_dir().ok();
    validate_workspace_folder_input_with_dirs(input, home_dir.as_deref(), current_dir.as_deref())
}

fn validate_workspace_folder_input_with_dirs(
    input: &str,
    home_dir: Option<&Path>,
    current_dir: Option<&Path>,
) -> Result<WorkspaceFolderSelection, String> {
    let path = workspace_folder_path_from_input(input, home_dir, current_dir)?;
    let metadata =
        std::fs::metadata(&path).map_err(|err| format!("Cannot open {}: {err}", path.display()))?;
    if !metadata.is_dir() {
        return Err(format!("{} is not a folder", path.display()));
    }

    let path_text = path.to_string_lossy().to_string();
    let name = path
        .file_name()
        .map(|segment| segment.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path_text.clone());
    Ok(WorkspaceFolderSelection { name, path_text })
}

fn workspace_folder_path_from_input(
    input: &str,
    home_dir: Option<&Path>,
    current_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Enter a folder path".to_string());
    }

    let expanded = if trimmed == "~" {
        home_dir
            .ok_or_else(|| "Home directory is unavailable".to_string())?
            .to_path_buf()
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        home_dir
            .ok_or_else(|| "Home directory is unavailable".to_string())?
            .join(rest)
    } else {
        PathBuf::from(trimmed)
    };

    if expanded.is_absolute() {
        Ok(expanded)
    } else if let Some(current_dir) = current_dir {
        Ok(current_dir.join(expanded))
    } else {
        Err("Current directory is unavailable".to_string())
    }
}

fn create_workspace_with_folder(state: &State, name: &str, folder_path: &str) {
    let workspace = workspace_state_with_folder(name, folder_path);
    add_workspace_from_state(state, &workspace);
    request_session_save(state);
}

fn workspace_state_with_folder(name: &str, folder_path: &str) -> WorkspaceState {
    WorkspaceState {
        id: None,
        name: name.to_string(),
        favorite: false,
        cwd: Some(folder_path.to_string()),
        folder_path: Some(folder_path.to_string()),
        layout: LayoutNodeState::Pane(PaneState::fallback(Some(folder_path))),
    }
}

fn initial_workspace_state(home_dir: Option<&Path>, current_dir: Option<&Path>) -> WorkspaceState {
    let folder = home_dir.or(current_dir).unwrap_or_else(|| Path::new("/"));
    let folder_path = folder.to_string_lossy();
    let name = folder
        .file_name()
        .map(|segment| segment.to_string_lossy())
        .filter(|segment| !segment.is_empty())
        .unwrap_or_else(|| folder_path.clone());
    workspace_state_with_folder(&name, &folder_path)
}

fn dispatch_control_command(command: ControlCommand) {
    CONTROL_STATE.with(|slot| {
        let state = slot.borrow().clone();
        if let Some(state) = state {
            handle_control_command(&state, command);
        } else {
            command.respond(Err(crate::control_bridge::BridgeError::internal(
                "control bridge not initialized",
            )));
        }
    });
}

fn handle_control_command(state: &State, command: ControlCommand) {
    match command {
        ControlCommand::Identify { caller, reply } => {
            let result = {
                let focused = focused_surface_payload(state).unwrap_or(serde_json::Value::Null);
                serde_json::json!({
                    "name": "limux-control",
                    "protocol": "v1+v2",
                    "version": env!("CARGO_PKG_VERSION"),
                    "focused": focused,
                    "caller": caller.unwrap_or_else(|| focused.clone()),
                })
            };
            let _ = reply.send(Ok(result));
        }
        ControlCommand::CurrentWorkspace { reply } => {
            let result = {
                let app_state = state.borrow();
                workspace_payload(&app_state, app_state.active_idx)
            };
            let _ = reply.send(result.ok_or_else(|| {
                crate::control_bridge::BridgeError::not_found("no active workspace")
            }));
        }
        ControlCommand::ListWorkspaces { reply } => {
            let workspaces = {
                let app_state = state.borrow();
                app_state
                    .workspaces
                    .iter()
                    .enumerate()
                    .map(|(index, workspace)| workspace_row(index, app_state.active_idx, workspace))
                    .collect::<Vec<_>>()
            };
            let _ = reply.send(Ok(serde_json::json!({ "workspaces": workspaces })));
        }
        ControlCommand::ListPanes { target, reply } => {
            let resolved = {
                let app_state = state.borrow();
                workspace_index_for_target(&app_state, &target)
            };

            let Some(index) = resolved else {
                let _ = reply.send(Err(crate::control_bridge::BridgeError::not_found(
                    "workspace not found",
                )));
                return;
            };

            let result = {
                let app_state = state.borrow();
                pane_list_payload(state, &app_state.workspaces[index])
            };
            let _ = reply.send(Ok(result));
        }
        ControlCommand::ListPaneSurfaces {
            target,
            pane_id,
            reply,
        } => {
            let resolved = {
                let app_state = state.borrow();
                workspace_index_for_target(&app_state, &target)
            };

            let Some(index) = resolved else {
                let _ = reply.send(Err(crate::control_bridge::BridgeError::not_found(
                    "workspace not found",
                )));
                return;
            };

            let pane_filter = pane_id
                .as_deref()
                .and_then(parse_pane_handle)
                .or_else(|| pane_id.as_deref().and_then(|raw| raw.parse::<u32>().ok()));
            if pane_id.is_some() && pane_filter.is_none() {
                let _ = reply.send(Err(crate::control_bridge::BridgeError::invalid_params(
                    "pane.surfaces requires a valid pane_id",
                )));
                return;
            }

            let result = {
                let app_state = state.borrow();
                surface_list_payload(state, &app_state.workspaces[index], pane_filter)
            };

            if pane_id.is_some()
                && result["surfaces"]
                    .as_array()
                    .is_some_and(|surfaces| surfaces.is_empty())
            {
                let _ = reply.send(Err(crate::control_bridge::BridgeError::not_found(
                    "pane not found",
                )));
                return;
            }

            let _ = reply.send(Ok(result));
        }
        ControlCommand::CreatePane { request, reply } => {
            if !matches!(request.pane_type, PaneCreateType::Terminal) {
                let _ = reply.send(Err(BridgeError::invalid_params(
                    "pane.create live GTK bridge supports type=terminal only",
                )));
                return;
            }

            let source_pane_id = request
                .source_pane_id
                .as_deref()
                .and_then(parse_pane_handle);
            if request.source_pane_id.is_some() && source_pane_id.is_none() {
                let _ = reply.send(Err(BridgeError::invalid_params(
                    "pane.create requires a valid pane_id",
                )));
                return;
            }

            let direction = PaneCreateDirection::from(request.direction);
            let resolved = match resolve_pane_create_target(
                state,
                &request.target,
                request.source_surface_id.as_deref(),
                source_pane_id,
                direction,
            ) {
                Ok(resolved) => resolved,
                Err(error) => {
                    let _ = reply.send(Err(pane_create_target_error(error)));
                    return;
                }
            };

            let workspace_name = {
                let app_state = state.borrow();
                app_state
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.id == resolved.workspace_id)
                    .map(|workspace| workspace.name.clone())
            };
            let Some(workspace_name) = workspace_name else {
                let _ = reply.send(Err(BridgeError::not_found("workspace not found")));
                return;
            };

            let new_pane = split_pane(
                state,
                &resolved.workspace_id,
                &resolved.pane_widget,
                resolved.placement.orientation,
                SplitPaneOptions {
                    initial_state: None,
                    skip_default_tab: false,
                    new_pane_first: resolved.placement.new_pane_first,
                    persist: true,
                },
            );
            let Some(new_pane) = new_pane else {
                let _ = reply.send(Err(BridgeError::invalid_params(
                    "not enough room to split pane",
                )));
                return;
            };

            let Some(surface) = pane::active_surface_summary(&new_pane) else {
                let _ = reply.send(Err(BridgeError::internal(
                    "pane.create did not produce a terminal surface",
                )));
                return;
            };

            let surface_id = surface.surface_id.clone();
            let response =
                pane_create_response_payload(&resolved.workspace_id, &workspace_name, surface);

            if let Some(command) = request.command {
                send_pane_create_response_after_command(
                    new_pane, surface_id, command, response, reply,
                );
                return;
            }

            let _ = reply.send(Ok(response));
        }
        ControlCommand::ListSurfaces { target, reply } => {
            let resolved = {
                let app_state = state.borrow();
                workspace_index_for_target(&app_state, &target)
            };

            let Some(index) = resolved else {
                let _ = reply.send(Err(crate::control_bridge::BridgeError::not_found(
                    "workspace not found",
                )));
                return;
            };

            let result = {
                let app_state = state.borrow();
                surface_list_payload(state, &app_state.workspaces[index], None)
            };
            let _ = reply.send(Ok(result));
        }
        ControlCommand::SurfaceHealth {
            target,
            surface_hint,
            reply,
        } => {
            let resolved = {
                let app_state = state.borrow();
                workspace_index_for_target(&app_state, &target)
            };

            let Some(index) = resolved else {
                let _ = reply.send(Err(crate::control_bridge::BridgeError::not_found(
                    "workspace not found",
                )));
                return;
            };

            let result = {
                let app_state = state.borrow();
                surface_health_payload(state, &app_state.workspaces[index], surface_hint.as_deref())
            };
            let _ = reply.send(result);
        }
        ControlCommand::CreateWorkspace {
            name,
            cwd,
            command,
            reply,
        } => {
            let home = dirs::home_dir()
                .map(|path| path.to_string_lossy().to_string())
                .unwrap_or_default();
            let folder_path = cwd.as_deref().unwrap_or(&home);
            let title = name.unwrap_or_else(|| {
                std::path::Path::new(folder_path)
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| "workspace".to_string())
            });

            create_workspace_with_folder(state, &title, folder_path);

            let result = {
                let app_state = state.borrow();
                workspace_payload(&app_state, app_state.active_idx)
            };

            if let (Some(command), Some(workspace_id)) = (
                command,
                result
                    .as_ref()
                    .and_then(|payload| payload["workspace_id"].as_str())
                    .map(ToOwned::to_owned),
            ) {
                let state = state.clone();
                glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
                    let target = {
                        let app_state = state.borrow();
                        app_state
                            .workspaces
                            .iter()
                            .find(|workspace| workspace.id == workspace_id)
                            .and_then(|workspace| {
                                pane::terminal_handle_for_surface(&workspace.root, None)
                            })
                    };
                    if let Some((_surface_id, handle)) = target {
                        handle.send_text(&command);
                        handle.send_text("\n");
                    }
                });
            }

            let _ = reply.send(result.ok_or_else(|| {
                crate::control_bridge::BridgeError::internal(
                    "workspace.create did not produce a workspace",
                )
            }));
        }
        ControlCommand::SelectWorkspace { target, reply } => {
            let resolved = {
                let app_state = state.borrow();
                workspace_index_for_target(&app_state, &target)
            };

            let Some(index) = resolved else {
                let _ = reply.send(Err(crate::control_bridge::BridgeError::not_found(
                    "workspace not found",
                )));
                return;
            };

            let row = {
                let app_state = state.borrow();
                app_state.workspaces[index].sidebar_row.clone()
            };
            let sidebar_list = state.borrow().sidebar_list.clone();
            switch_workspace(state, index);
            sidebar_list.select_row(Some(&row));

            let result = {
                let app_state = state.borrow();
                workspace_payload(&app_state, index)
            };
            let _ = reply.send(result.ok_or_else(|| {
                crate::control_bridge::BridgeError::not_found("workspace not found")
            }));
        }
        ControlCommand::RenameWorkspace {
            target,
            title,
            reply,
        } => {
            let resolved = {
                let app_state = state.borrow();
                workspace_index_for_target(&app_state, &target)
            };

            let Some(index) = resolved else {
                let _ = reply.send(Err(crate::control_bridge::BridgeError::not_found(
                    "workspace not found",
                )));
                return;
            };

            {
                let mut app_state = state.borrow_mut();
                let workspace = &mut app_state.workspaces[index];
                workspace.name = title.clone();
                workspace.name_label.set_label(&title);
            }
            request_session_save(state);

            let result = {
                let app_state = state.borrow();
                workspace_payload(&app_state, index)
            };
            let _ = reply.send(result.ok_or_else(|| {
                crate::control_bridge::BridgeError::not_found("workspace not found")
            }));
        }
        ControlCommand::CloseWorkspace { target, reply } => {
            let resolved = {
                let app_state = state.borrow();
                if app_state.workspaces.len() <= 1 {
                    None
                } else {
                    workspace_index_for_target(&app_state, &target)
                }
            };

            let Some(index) = resolved else {
                let can_close = state.borrow().workspaces.len() > 1;
                let error = if can_close {
                    crate::control_bridge::BridgeError::not_found("workspace not found")
                } else {
                    crate::control_bridge::BridgeError::conflict("cannot close workspace")
                };
                let _ = reply.send(Err(error));
                return;
            };

            let closed_workspace = {
                let app_state = state.borrow();
                workspace_payload(&app_state, index)
            };
            let workspace_id = state.borrow().workspaces[index].id.clone();
            close_workspace_by_id(state, &workspace_id);

            let _ = reply.send(closed_workspace.ok_or_else(|| {
                crate::control_bridge::BridgeError::not_found("workspace not found")
            }));
        }
        ControlCommand::SendText {
            target,
            surface_hint,
            text,
            reply,
        } => {
            let resolved = {
                let app_state = state.borrow();
                workspace_index_for_target(&app_state, &target)
            };

            let Some(index) = resolved else {
                let _ = reply.send(Err(crate::control_bridge::BridgeError::not_found(
                    "workspace not found",
                )));
                return;
            };

            let target = {
                let app_state = state.borrow();
                let workspace = &app_state.workspaces[index];
                let (_focused_pane_id, focused_surface_id) =
                    focused_ids_for_workspace(state, &workspace.id);
                let resolved_surface_hint =
                    surface_hint.as_deref().or(focused_surface_id.as_deref());
                pane::terminal_handle_for_root(&workspace.root, resolved_surface_hint).map(
                    |(surface_id, handle)| {
                        (
                            serde_json::json!({
                                "workspace_id": workspace.id.as_str(),
                                "workspace_ref": workspace_ref(&workspace.id),
                                "surface_id": surface_id.as_str(),
                                "surface_ref": surface_ref(&surface_id),
                            }),
                            handle,
                        )
                    },
                )
            };

            let Some((mut payload, handle)) = target else {
                let _ = reply.send(Err(crate::control_bridge::BridgeError::not_found(
                    "terminal surface not found",
                )));
                return;
            };

            handle.send_text(&text);
            if let Some(map) = payload.as_object_mut() {
                map.insert("ok".to_string(), serde_json::Value::Bool(true));
            }
            let _ = reply.send(Ok(payload));
        }
        ControlCommand::ReadSurfaceText {
            target,
            surface_hint,
            reply,
        } => {
            let resolved = {
                let app_state = state.borrow();
                workspace_index_for_target(&app_state, &target)
            };

            let Some(index) = resolved else {
                let _ = reply.send(Err(crate::control_bridge::BridgeError::not_found(
                    "workspace not found",
                )));
                return;
            };

            let target = {
                let app_state = state.borrow();
                let workspace = &app_state.workspaces[index];
                pane::terminal_handle_for_root(&workspace.root, surface_hint.as_deref()).map(
                    |(surface_id, handle)| {
                        (
                            serde_json::json!({
                                "workspace_id": workspace.id.as_str(),
                                "workspace_ref": workspace_ref(&workspace.id),
                                "surface_id": surface_id.as_str(),
                                "surface_ref": surface_ref(&surface_id),
                            }),
                            handle,
                        )
                    },
                )
            };

            let Some((mut payload, handle)) = target else {
                let _ = reply.send(Err(crate::control_bridge::BridgeError::not_found(
                    "terminal surface not found",
                )));
                return;
            };

            let Some(text) = handle.read_viewport_text() else {
                let _ = reply.send(Err(crate::control_bridge::BridgeError::internal(
                    "surface.read_text failed",
                )));
                return;
            };
            if let Some(map) = payload.as_object_mut() {
                map.insert("text".to_string(), serde_json::Value::String(text));
            }
            let _ = reply.send(Ok(payload));
        }
        ControlCommand::SendKey {
            target,
            surface_hint,
            key,
            reply,
        } => {
            let resolved = {
                let app_state = state.borrow();
                workspace_index_for_target(&app_state, &target)
            };

            let Some(index) = resolved else {
                let _ = reply.send(Err(crate::control_bridge::BridgeError::not_found(
                    "workspace not found",
                )));
                return;
            };

            let target = {
                let app_state = state.borrow();
                let workspace = &app_state.workspaces[index];
                pane::terminal_handle_for_root(&workspace.root, surface_hint.as_deref()).map(
                    |(surface_id, handle)| {
                        (
                            serde_json::json!({
                                "workspace_id": workspace.id.as_str(),
                                "workspace_ref": workspace_ref(&workspace.id),
                                "surface_id": surface_id.as_str(),
                                "surface_ref": surface_ref(&surface_id),
                            }),
                            handle,
                        )
                    },
                )
            };

            let Some((mut payload, handle)) = target else {
                let _ = reply.send(Err(crate::control_bridge::BridgeError::not_found(
                    "terminal surface not found",
                )));
                return;
            };

            if !handle.send_key(&key) {
                let _ = reply.send(Err(crate::control_bridge::BridgeError::invalid_params(
                    "unsupported key",
                )));
                return;
            }
            if let Some(map) = payload.as_object_mut() {
                map.insert("ok".to_string(), serde_json::Value::Bool(true));
            }
            let _ = reply.send(Ok(payload));
        }
        ControlCommand::CreateNotification {
            target,
            surface_hint,
            title,
            subtitle,
            body,
            reply,
        } => {
            // Resolve the workspace target. `WorkspaceTarget::Active` maps to
            // the currently-focused workspace via workspace_index_for_target.
            let resolved = {
                let app_state = state.borrow();
                workspace_index_for_target(&app_state, &target)
            };

            let Some(preferred_index) = resolved else {
                let _ = reply.send(Err(crate::control_bridge::BridgeError::not_found(
                    "workspace not found",
                )));
                return;
            };

            let (index, tab_target) = surface_hint
                .as_deref()
                .map(|surface| resolve_notification_tab_target(state, preferred_index, surface))
                .unwrap_or((preferred_index, None));

            let (ws_id, root, workspace_is_active, window_active) = {
                let s = state.borrow();
                let workspace = &s.workspaces[index];
                (
                    workspace.id.clone(),
                    workspace.root.clone(),
                    index == s.active_idx,
                    s.window.is_active(),
                )
            };

            // Build the sidebar message: title becomes the bold prefix,
            // subtitle + body are joined with " — " for the body text.
            let combined_body = match (subtitle.is_empty(), body.is_empty()) {
                (true, true) => String::new(),
                (true, false) => body.clone(),
                (false, true) => subtitle.clone(),
                (false, false) => format!("{subtitle} — {body}"),
            };
            let message = workspace_notification_message(&title, &combined_body);
            let source_focused = tab_target.as_ref().is_some_and(|(pane_id, tab_id)| {
                window_active
                    && workspace_is_active
                    && pane::tab_is_visible_in_workspace(&ws_id, &root, *pane_id, tab_id)
            });
            let target = DesktopNotificationTarget {
                workspace_id: ws_id.clone(),
                pane_id: tab_target.as_ref().map(|(pane_id, _)| *pane_id),
                tab_id: tab_target.map(|(_, tab_id)| tab_id),
            };
            if let Some(request) =
                mark_workspace_unread_with_message(state, &ws_id, &message, source_focused, target)
            {
                show_desktop_notification(state, request);
            }

            let payload = serde_json::json!({
                "ok": true,
                "workspace_id": ws_id,
                "workspace_ref": workspace_ref(&ws_id),
                "title": title,
                "subtitle": subtitle,
                "body": body,
            });
            let _ = reply.send(Ok(payload));
        }
    }
}

fn add_workspace_from_state(state: &State, workspace: &WorkspaceState) {
    let shortcuts = {
        let s = state.borrow();
        s.shortcuts.clone()
    };
    let (stack, sidebar_list) = {
        let s = state.borrow();
        (s.stack.clone(), s.sidebar_list.clone())
    };
    let id = workspace
        .id
        .as_deref()
        .filter(|id| uuid::Uuid::parse_str(id).is_ok())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let stack_name = format!("ws-{id}");
    let working_dir = workspace
        .folder_path
        .as_deref()
        .or(workspace.cwd.as_deref());
    let (root, split_container) =
        build_workspace_root(state, &shortcuts, &id, working_dir, &workspace.layout);
    stack.add_named(&root, Some(&stack_name));

    let show_workspace_path = state
        .borrow()
        .config
        .borrow()
        .appearance
        .show_workspace_path;
    let (row, name_label, favorite_button, notify_dot, notify_label, path_label) =
        build_sidebar_row(
            &workspace.name,
            workspace.folder_path.as_deref(),
            show_workspace_path,
        );
    sidebar_list.append(&row);
    install_workspace_row_interactions(state, &id, &row, &favorite_button);

    let cwd: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(workspace.cwd.clone()));
    let ws = Workspace {
        id,
        name: workspace.name.clone(),
        root,
        split_container,
        sidebar_row: row.clone(),
        name_label,
        favorite_button,
        notify_dot,
        notify_label,
        unread: false,
        favorite: workspace.favorite,
        cwd,
        folder_path: workspace.folder_path.clone(),
        path_label,
    };

    if workspace.favorite {
        set_workspace_favorite_visual(&ws);
    }

    {
        let mut s = state.borrow_mut();
        s.workspaces.push(ws);
        s.active_idx = s.workspaces.len() - 1;
    }

    stack.set_visible_child_name(&stack_name);
    sidebar_list.select_row(Some(&row));
}

/// Create a PaneWidget wired up with callbacks for a specific workspace.
pub(crate) fn create_pane_for_workspace(
    state: &State,
    shortcuts: &Rc<ResolvedShortcutConfig>,
    ws_id: &str,
    working_directory: Option<&str>,
    initial_state: Option<&PaneState>,
    skip_default_tab: bool,
) -> gtk::Box {
    let state_for_split = state.clone();
    let state_for_close = state.clone();
    let state_for_bell = state.clone();
    let state_for_desktop_notification = state.clone();
    let state_for_open_url = state.clone();
    let state_for_keybinds = state.clone();
    let state_for_pwd = state.clone();
    let state_for_empty = state.clone();
    let state_for_unread = state.clone();
    let state_for_visibility = state.clone();
    let ws_id_split = ws_id.to_string();
    let ws_id_close = ws_id.to_string();
    let ws_id_bell = ws_id.to_string();
    let ws_id_desktop_notification = ws_id.to_string();
    let ws_id_open_url = ws_id.to_string();
    let ws_id_pwd = ws_id.to_string();
    let ws_id_empty = ws_id.to_string();
    let ws_id_unread = ws_id.to_string();
    let ws_id_visibility = ws_id.to_string();
    let state_for_split_with_tab = state.clone();
    let state_for_config = state.clone();
    let state_for_config_changed = state.clone();
    let ws_id_split_with_tab = ws_id.to_string();
    let ws_id_for_env = ws_id.to_string();

    let callbacks = Rc::new(PaneCallbacks {
        workspace_id: ws_id.to_string(),
        on_split: Box::new(move |pane_widget, orientation| {
            split_pane(
                &state_for_split,
                &ws_id_split,
                pane_widget,
                orientation,
                SplitPaneOptions {
                    initial_state: None,
                    skip_default_tab: false,
                    new_pane_first: false,
                    persist: true,
                },
            );
        }),
        on_close_pane: Box::new(move |pane_widget| {
            remove_pane_internal(&state_for_close, &ws_id_close, pane_widget, true);
        }),
        on_bell: Box::new(move |source_focused: bool, pane_id: u32, tab_id: &str| {
            // Defer to avoid RefCell borrow conflicts — bell can fire during state mutation
            let state = state_for_bell.clone();
            let ws_id = ws_id_bell.clone();
            let tab_id = tab_id.to_string();
            let target = DesktopNotificationTarget {
                workspace_id: ws_id.clone(),
                pane_id: Some(pane_id),
                tab_id: Some(tab_id),
            };
            glib::idle_add_local_once(move || {
                if let Some(request) = mark_workspace_unread(&state, &ws_id, source_focused, target)
                {
                    show_desktop_notification(&state, request);
                }
            });
        }),
        on_desktop_notification: Box::new(
            move |title: &str, body: &str, source_focused: bool, pane_id: u32, tab_id: &str| {
                let state = state_for_desktop_notification.clone();
                let ws_id = ws_id_desktop_notification.clone();
                let tab_id = tab_id.to_string();
                let target = DesktopNotificationTarget {
                    workspace_id: ws_id.clone(),
                    pane_id: Some(pane_id),
                    tab_id: Some(tab_id),
                };
                let message = workspace_notification_message(title, body);
                glib::idle_add_local_once(move || {
                    if let Some(request) = mark_workspace_unread_with_message(
                        &state,
                        &ws_id,
                        &message,
                        source_focused,
                        target,
                    ) {
                        show_desktop_notification(&state, request);
                    }
                });
            },
        ),
        on_open_browser_here: Box::new(move |pane_widget| {
            pane::add_browser_tab_to_pane(pane_widget);
        }),
        on_open_url_in_browser: Box::new(move |pane_widget, url| {
            open_url_in_browser_tab(&state_for_open_url, &ws_id_open_url, pane_widget, url);
        }),
        on_open_keybinds: Box::new(move |anchor| {
            open_keybind_editor_tab(&state_for_keybinds, anchor);
        }),
        current_shortcuts: Box::new({
            let state = state.clone();
            move || {
                let s = state.borrow();
                s.shortcuts.clone()
            }
        }),
        on_capture_shortcut: {
            let state = state.clone();
            Rc::new(move |id, binding| persist_shortcut_binding(&state, id, binding))
        },
        on_pwd_changed: Box::new(move |pwd: &str| {
            let state = state_for_pwd.clone();
            let ws_id = ws_id_pwd.clone();
            let pwd = pwd.to_string();
            glib::idle_add_local_once(move || {
                let s = state.borrow();
                if let Some(ws) = s.workspaces.iter().find(|w| w.id == ws_id) {
                    *ws.cwd.borrow_mut() = Some(pwd);
                }
            });
        }),
        on_empty: Box::new(move |pane_widget, reason| {
            if should_keep_workspace_open_for_empty_pane(&state_for_empty, &ws_id_empty, reason) {
                request_session_save(&state_for_empty);
                return;
            }
            let persist = matches!(
                reason,
                pane::PaneEmptyReason::ClosedLastTerminal | pane::PaneEmptyReason::ClosedLastTab
            );
            remove_pane_internal(&state_for_empty, &ws_id_empty, pane_widget, persist);
        }),
        on_state_changed: Box::new({
            let state = state.clone();
            move || request_session_save(&state)
        }),
        on_unread_changed: Box::new(move || {
            sync_workspace_unread(&state_for_unread, &ws_id_unread);
        }),
        is_pane_visible: Box::new(move |pane_widget| {
            let s = state_for_visibility.borrow();
            s.window.is_active()
                && s.active_workspace().is_some_and(|workspace| {
                    workspace.id == ws_id_visibility && pane_widget.is_ancestor(&workspace.root)
                })
        }),
        on_split_with_tab: Box::new(
            move |source_pane, target_pane, orientation, tab_id, new_pane_first| {
                handle_split_with_tab(
                    &state_for_split_with_tab,
                    &ws_id_split_with_tab,
                    source_pane,
                    target_pane,
                    orientation,
                    &tab_id,
                    new_pane_first,
                );
            },
        ),
        current_config: Box::new(move || {
            let s = state_for_config.borrow();
            s.config.clone()
        }),
        on_config_changed: Rc::new(
            move |previous: &app_config::AppConfig, updated: &app_config::AppConfig| {
                let style_manager = adw::StyleManager::default();
                let system_prefers_dark =
                    state_for_config_changed.borrow().system_prefers_dark.get();
                apply_appearance(&style_manager, system_prefers_dark, &updated.appearance);
                if updated.appearance.ui_scale != previous.appearance.ui_scale {
                    reload_app_css(&state_for_config_changed, updated);
                }
                if updated.appearance.show_workspace_path != previous.appearance.show_workspace_path
                {
                    sync_workspace_path_visibility(
                        &state_for_config_changed,
                        updated.appearance.show_workspace_path,
                    );
                }
                if let Err(err) = app_config::save(updated) {
                    state_for_config_changed
                        .borrow()
                        .config
                        .borrow_mut()
                        .clone_from(previous);
                    apply_appearance(&style_manager, system_prefers_dark, &previous.appearance);
                    if updated.appearance.ui_scale != previous.appearance.ui_scale {
                        reload_app_css(&state_for_config_changed, previous);
                    }
                    if updated.appearance.show_workspace_path
                        != previous.appearance.show_workspace_path
                    {
                        sync_workspace_path_visibility(
                            &state_for_config_changed,
                            previous.appearance.show_workspace_path,
                        );
                    }

                    let detail = format!("Failed to save Limux settings: {err}");
                    eprintln!("limux: {detail}");
                    show_runtime_error(
                        &state_for_config_changed,
                        "Failed to save settings",
                        &detail,
                    );
                }
            },
        ),
        workspace_for_pane: Box::new(move |_pane_widget| Some(ws_id_for_env.clone())),
    });

    pane::create_pane(
        callbacks,
        shortcuts.clone(),
        working_directory,
        initial_state,
        skip_default_tab,
    )
}

fn close_workspace(state: &State) {
    let id = {
        let s = state.borrow();
        s.active_workspace().map(|w| w.id.clone())
    };
    if let Some(id) = id {
        close_workspace_by_id(state, &id);
    }
}

fn close_workspace_by_id(state: &State, id: &str) {
    close_workspace_by_id_internal(state, id, true, None);
}

fn close_workspace_by_id_internal(
    state: &State,
    id: &str,
    persist: bool,
    preferred_active_workspace_id: Option<&str>,
) {
    let split_container = {
        let s = state.borrow();
        s.workspaces
            .iter()
            .find(|workspace| workspace.id == id)
            .map(|workspace| workspace.split_container.clone())
    };
    let Some(split_container) = split_container else {
        return;
    };
    split_container.retire_panes();

    let mut s = state.borrow_mut();
    let Some(idx) = s.workspaces.iter().position(|workspace| workspace.id == id) else {
        return;
    };
    let desired_active_workspace_id = preferred_active_workspace_id
        .map(ToOwned::to_owned)
        .or_else(|| s.active_workspace().map(|workspace| workspace.id.clone()));

    let ws = s.workspaces.remove(idx);
    s.stack.remove(&ws.root);
    s.sidebar_list.remove(&ws.sidebar_row);

    if s.workspaces.is_empty() {
        s.active_idx = 0;
        drop(s);
        if persist {
            request_session_save(state);
        }
        return;
    }

    let remaining_workspace_ids: Vec<&str> = s
        .workspaces
        .iter()
        .map(|workspace| workspace.id.as_str())
        .collect();
    let new_idx = next_active_workspace_index(
        &remaining_workspace_ids,
        desired_active_workspace_id.as_deref(),
        idx,
    );
    s.active_idx = new_idx;

    let stack_name = format!("ws-{}", s.workspaces[new_idx].id);
    s.stack.set_visible_child_name(&stack_name);

    let row = s.workspaces[new_idx].sidebar_row.clone();
    let sidebar_list = s.sidebar_list.clone();
    drop(s);

    sidebar_list.select_row(Some(&row));
    if persist {
        request_session_save(state);
    }
}

fn switch_workspace(state: &State, idx: usize) {
    let active_workspace_id = {
        let s = state.borrow();
        if idx >= s.workspaces.len() {
            return;
        }
        (idx == s.active_idx).then(|| s.workspaces[idx].id.clone())
    };
    if let Some(workspace_id) = active_workspace_id {
        clear_visible_tab_unread(state, &workspace_id);
        return;
    }

    let (stack, stack_name, focus_root, workspace_id) = {
        let mut s = state.borrow_mut();
        s.active_idx = idx;
        let stack = s.stack.clone();
        let stack_name = format!("ws-{}", s.workspaces[idx].id);
        let focus_root = s.workspaces[idx].root.clone();
        let workspace_id = s.workspaces[idx].id.clone();

        (stack, stack_name, focus_root, workspace_id)
    };

    stack.set_visible_child_name(&stack_name);
    clear_visible_tab_unread(state, &workspace_id);
    glib::idle_add_local_once(move || {
        focus_workspace_entrypoint(&focus_root);
    });

    request_session_save(state);
}

fn cycle_workspace(state: &State, direction: i32) {
    let (new_idx, row, sidebar_list) = {
        let s = state.borrow();
        let len = s.workspaces.len();
        if len <= 1 {
            return;
        }
        let new_idx = ((s.active_idx as i32 + direction).rem_euclid(len as i32)) as usize;
        (
            new_idx,
            s.workspaces[new_idx].sidebar_row.clone(),
            s.sidebar_list.clone(),
        )
    };
    switch_workspace(state, new_idx);
    sidebar_list.select_row(Some(&row));
}

fn focus_workspace_entrypoint(root: &gtk::Widget) {
    let pane = first_leaf_pane(root);
    if !pane::focus_active_tab_in_pane(&pane) {
        if let Some(gl) = find_gl_area(&pane) {
            gl.grab_focus();
        } else if pane.is_focusable() || pane.can_focus() {
            pane.grab_focus();
        } else {
            pane.child_focus(gtk::DirectionType::TabForward);
        }
    }
}

fn first_leaf_pane(widget: &gtk::Widget) -> gtk::Widget {
    if pane::is_pane_widget(widget) {
        return widget.clone();
    }

    if let Some(paned) = widget.downcast_ref::<gtk::Paned>() {
        if let Some(child) = paned.start_child().or_else(|| paned.end_child()) {
            return first_leaf_pane(&child);
        }
    }

    if let Some(stack) = widget.downcast_ref::<gtk::Stack>() {
        if let Some(visible) = stack.visible_child() {
            return first_leaf_pane(&visible);
        }
    }

    let mut child = widget.first_child();
    while let Some(current) = child {
        let candidate = first_leaf_pane(&current);
        if pane::is_pane_widget(&candidate) {
            return candidate;
        }
        child = current.next_sibling();
    }

    widget.clone()
}

/// Default sidebar width in pixels.
const SIDEBAR_WIDTH: i32 = 220;

fn sync_top_bar_visibility(state: &State) {
    let (top_bar, preferred_visible, fullscreened) = {
        let s = state.borrow();
        (
            s.top_bar.clone(),
            s.top_bar_visible,
            gtk::prelude::GtkWindowExt::is_fullscreen(&s.window),
        )
    };

    if let Some(top_bar) = top_bar {
        top_bar.set_visible(preferred_visible && !fullscreened);
    }
}

fn toggle_top_bar(state: &State) {
    {
        let mut s = state.borrow_mut();
        s.top_bar_visible = !s.top_bar_visible;
    }
    sync_top_bar_visibility(state);
    request_session_save(state);
}

fn toggle_fullscreen(state: &State) {
    let window = state.borrow().window.clone();
    if gtk::prelude::GtkWindowExt::is_fullscreen(&window) {
        window.unfullscreen();
    } else {
        window.fullscreen();
    }
}

fn toggle_sidebar(state: &State) {
    let (sidebar_shell, sidebar_handle, current, is_visible, target_width, prior_animation, epoch) = {
        let mut s = state.borrow_mut();
        let current = sidebar_width(&s.sidebar_shell);
        let is_visible = current > 10; // treat < 10px as collapsed
        if is_visible {
            s.sidebar_expanded_width = current;
        }
        let target_width = s.sidebar_expanded_width.max(SIDEBAR_WIDTH);
        let prior_animation = s.sidebar_animation.take();
        s.sidebar_animation_epoch = s.sidebar_animation_epoch.wrapping_add(1);
        (
            s.sidebar_shell.clone(),
            s.sidebar_handle.clone(),
            current,
            is_visible,
            target_width,
            prior_animation,
            s.sidebar_animation_epoch,
        )
    };

    if let Some(animation) = prior_animation {
        animation.pause();
    }

    if is_visible {
        // Collapse: animate position to 0, then hide sidebar.
        let target = adw::CallbackAnimationTarget::new({
            let sidebar_shell = sidebar_shell.clone();
            move |value| {
                set_sidebar_width(&sidebar_shell, value as i32);
            }
        });
        let animation = adw::TimedAnimation::builder()
            .widget(&sidebar_shell)
            .value_from(current as f64)
            .value_to(0.0)
            .duration(200)
            .easing(adw::Easing::EaseInOutCubic)
            .target(&target)
            .build();
        let state_for_done = state.clone();
        animation.connect_done(move |_| {
            let is_current = {
                let mut s = state_for_done.borrow_mut();
                if s.sidebar_animation_epoch != epoch {
                    false
                } else {
                    s.sidebar_animation = None;
                    true
                }
            };
            if is_current {
                set_sidebar_state_widgets(&sidebar_shell, &sidebar_handle, 0, false);
                request_session_save(&state_for_done);
            }
        });
        state.borrow_mut().sidebar_animation = Some(animation.clone());
        animation.play();
    } else {
        // Expand: make sidebar visible, then animate position from 0 to remembered width.
        set_sidebar_state_widgets(&sidebar_shell, &sidebar_handle, 0, true);
        let target = adw::CallbackAnimationTarget::new({
            let sidebar_shell = sidebar_shell.clone();
            move |value| {
                set_sidebar_width(&sidebar_shell, value as i32);
            }
        });
        let animation = adw::TimedAnimation::builder()
            .widget(&sidebar_shell)
            .value_from(0.0)
            .value_to(target_width as f64)
            .duration(200)
            .easing(adw::Easing::EaseInOutCubic)
            .target(&target)
            .build();
        let state_for_done = state.clone();
        animation.connect_done(move |_| {
            let is_current = {
                let mut s = state_for_done.borrow_mut();
                if s.sidebar_animation_epoch != epoch {
                    false
                } else {
                    s.sidebar_animation = None;
                    true
                }
            };
            if is_current {
                request_session_save(&state_for_done);
            }
        });
        state.borrow_mut().sidebar_animation = Some(animation.clone());
        animation.play();
    }
}

// ---------------------------------------------------------------------------
// Split / close pane operations
// ---------------------------------------------------------------------------

struct SplitPaneOptions {
    initial_state: Option<PaneState>,
    skip_default_tab: bool,
    new_pane_first: bool,
    persist: bool,
}

fn split_pane(
    state: &State,
    ws_id: &str,
    pane_widget: &gtk::Widget,
    orientation: gtk::Orientation,
    options: SplitPaneOptions,
) -> Option<gtk::Widget> {
    let (shortcuts, wd, container) = {
        let s = state.borrow();
        (
            s.shortcuts.clone(),
            s.workspaces
                .iter()
                .find(|w| w.id == ws_id)
                .and_then(|ws| ws.folder_path.clone().or_else(|| ws.cwd.borrow().clone())),
            s.workspaces
                .iter()
                .find(|w| w.id == ws_id)
                .map(|ws| ws.split_container.clone()),
        )
    };
    let container = container?;
    if !container.can_split(pane_widget, orientation) {
        return None;
    }

    let new_pane = create_pane_for_workspace(
        state,
        &shortcuts,
        ws_id,
        wd.as_deref(),
        options.initial_state.as_ref(),
        options.skip_default_tab,
    );

    // Mutate the data model and trigger async widget tree rebuild.
    // The existing pane's GLArea will be unrealized then re-realized
    // on separate ticks, avoiding the GTK4 GLArea breakage.
    if !container.split(
        pane_widget,
        new_pane.clone().upcast(),
        orientation,
        options.new_pane_first,
        layout_state::DEFAULT_SPLIT_RATIO,
    ) {
        return None;
    }
    if options.persist {
        request_session_save(state);
    }
    Some(new_pane.upcast())
}

fn open_url_in_browser_tab(state: &State, ws_id: &str, pane_widget: &gtk::Widget, url: &str) {
    let right_pane = pane_in_direction(state, pane_widget, Direction::Right);
    crate::browser_link::open_in_right_pane(
        pane_widget,
        right_pane.as_ref(),
        || {
            split_pane(
                state,
                ws_id,
                pane_widget,
                gtk::Orientation::Horizontal,
                SplitPaneOptions {
                    initial_state: Some(PaneState::browser_only(Some(url))),
                    skip_default_tab: false,
                    new_pane_first: false,
                    persist: true,
                },
            )
            .is_some()
        },
        |target| pane::add_browser_tab_to_pane_with_uri(target, Some(url)),
    );
}

fn remove_pane(state: &State, ws_id: &str, pane_widget: &gtk::Widget) {
    remove_pane_internal(state, ws_id, pane_widget, true);
}

fn should_keep_workspace_open_for_empty_pane(
    state: &State,
    ws_id: &str,
    reason: pane::PaneEmptyReason,
) -> bool {
    let s = state.borrow();
    let enabled = s
        .config
        .borrow()
        .workspace
        .keep_open_after_last_terminal_closes;
    let is_only_pane = s
        .workspaces
        .iter()
        .find(|workspace| workspace.id == ws_id)
        .is_some_and(|workspace| workspace.split_container.is_single_pane());

    keep_workspace_open_after_empty_pane(enabled, reason, is_only_pane)
}

fn keep_workspace_open_after_empty_pane(
    enabled: bool,
    reason: pane::PaneEmptyReason,
    is_only_pane: bool,
) -> bool {
    enabled && reason == pane::PaneEmptyReason::ClosedLastTerminal && is_only_pane
}

fn remove_pane_internal(state: &State, ws_id: &str, pane_widget: &gtk::Widget, persist: bool) {
    let container = {
        let s = state.borrow();
        s.workspaces
            .iter()
            .find(|w| w.id == ws_id)
            .map(|ws| ws.split_container.clone())
    };

    let Some(container) = container else { return };

    // If this is the only pane, close the entire workspace
    if container.is_single_pane() {
        close_workspace_by_id(state, ws_id);
        return;
    }

    // Mutate the data model and trigger async widget tree rebuild
    if !container.remove(pane_widget) {
        return;
    }
    pane::retire_pane(pane_widget);
    sync_workspace_unread(state, ws_id);

    if persist {
        request_session_save(state);
    }
}

fn handle_split_with_tab(
    state: &State,
    ws_id: &str,
    source_pane: &gtk::Widget,
    target_pane: &gtk::Widget,
    orientation: gtk::Orientation,
    tab_id: &str,
    new_pane_first: bool,
) {
    if pane::tab_title(source_pane, tab_id).is_none() {
        return;
    }
    let new_pane = split_pane(
        state,
        ws_id,
        target_pane,
        orientation,
        SplitPaneOptions {
            initial_state: None,
            skip_default_tab: true,
            new_pane_first,
            persist: false,
        },
    );
    let Some(new_pane) = new_pane else { return };
    if pane::move_tab_to_pane(source_pane, tab_id, &new_pane) {
        request_session_save(state);
    }
}

/// Find the focused pane widget (a gtk::Box with class limux-pane-toolbar child)
/// by walking up from the currently focused widget.
fn find_leaf_focused_pane(state: &State) -> Option<(String, gtk::Widget)> {
    let (ws_id, root, stack) = {
        let s = state.borrow();
        let ws = s.active_workspace()?;
        (ws.id.clone(), ws.root.clone(), s.stack.clone())
    };

    // Get the window's focus widget and walk up to find a pane Box
    let window = stack.root()?.downcast::<gtk::Window>().ok()?;
    let focus = gtk::prelude::GtkWindowExt::focus(&window)?;

    let mut widget: Option<gtk::Widget> = Some(focus);
    while let Some(w) = widget {
        if let Some(bx) = w.downcast_ref::<gtk::Box>() {
            let mut child = bx.first_child();
            while let Some(c) = child {
                if c.has_css_class("limux-pane-header") {
                    return Some((ws_id, w));
                }
                child = c.next_sibling();
            }
        }
        widget = w.parent();
    }

    let _ = root;
    None
}

fn find_focused_pane(state: &State) -> Option<(String, gtk::Widget)> {
    if let Some(found) = find_leaf_focused_pane(state) {
        return Some(found);
    }

    let (ws_id, root) = {
        let s = state.borrow();
        let ws = s.active_workspace()?;
        (ws.id.clone(), ws.root.clone())
    };

    Some((ws_id, first_leaf_pane(&root)))
}

fn focused_shortcut_target(state: &State) -> pane::FocusedShortcutTarget {
    let Some((_ws_id, pane_widget)) = find_leaf_focused_pane(state) else {
        return pane::FocusedShortcutTarget::None;
    };
    pane::focused_shortcut_target(&pane_widget)
}

fn show_runtime_error(state: &State, title: &str, detail: &str) {
    let window = state.borrow().window.clone();
    let dialog = gtk::AlertDialog::builder()
        .modal(true)
        .message(title)
        .detail(detail)
        .build();
    dialog.show(Some(&window));
}

fn quit_app(state: &State) {
    let window = state.borrow().window.clone();
    window.close();
}

fn spawn_new_instance(state: &State) -> bool {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            let detail = format!("Failed to resolve the current Limux executable: {err}");
            eprintln!("limux: {detail}");
            show_runtime_error(state, "Failed to open a new Limux instance", &detail);
            return false;
        }
    };

    match std::process::Command::new(exe).spawn() {
        Ok(_) => true,
        Err(err) => {
            let detail = format!("Failed to launch a new Limux instance: {err}");
            eprintln!("limux: {detail}");
            show_runtime_error(state, "Failed to open a new Limux instance", &detail);
            false
        }
    }
}

fn dispatch_terminal_command(state: &State, command: ShortcutCommand) -> bool {
    let pane::FocusedShortcutTarget::Terminal(target) = focused_shortcut_target(state) else {
        return false;
    };

    match command {
        ShortcutCommand::SurfaceFind => target.show_find(),
        ShortcutCommand::SurfaceFindNext => target.find_next(),
        ShortcutCommand::SurfaceFindPrevious => target.find_previous(),
        ShortcutCommand::SurfaceFindHide => target.hide_find(),
        ShortcutCommand::SurfaceUseSelectionForFind => target.use_selection_for_find(),
        ShortcutCommand::TerminalClearScrollback => target.perform_binding_action("clear_screen"),
        ShortcutCommand::TerminalCopy => target.copy_selection_to_clipboard(),
        ShortcutCommand::TerminalPaste => target.perform_binding_action("paste_from_clipboard"),
        ShortcutCommand::TerminalIncreaseFontSize => persist_font_size_delta(state, 1.0),
        ShortcutCommand::TerminalDecreaseFontSize => persist_font_size_delta(state, -1.0),
        ShortcutCommand::TerminalResetFontSize => persist_font_size_reset(state),
        _ => false,
    }
}

fn persist_font_size_delta(state: &State, delta: f32) -> bool {
    let current = {
        let s = state.borrow();
        let current = s.config.borrow().font_size;
        current
    };
    let new_size = font_size_after_delta(current, crate::terminal::default_font_size(), delta);

    if let Err(err) = persist_font_size(state, Some(new_size)) {
        show_font_size_save_error(state, err);
        return false;
    }

    broadcast_font_size(new_size);
    true
}

fn persist_font_size_reset(state: &State) -> bool {
    if let Err(err) = persist_font_size(state, None) {
        show_font_size_save_error(state, err);
        return false;
    }

    crate::terminal::broadcast_binding_action("reset_font_size");
    true
}

fn persist_font_size(state: &State, font_size: Option<f32>) -> Result<(), String> {
    let mut updated = {
        let s = state.borrow();
        let updated = s.config.borrow().clone();
        updated
    };
    updated.font_size = font_size;
    app_config::save(&updated)?;

    state.borrow().config.borrow_mut().font_size = font_size;
    Ok(())
}

fn font_size_after_delta(current: Option<f32>, default: f32, delta: f32) -> f32 {
    (current.unwrap_or(default) + delta).clamp(1.0, 255.0)
}

fn show_font_size_save_error(state: &State, err: String) {
    let detail = format!("Failed to save Limux settings: {err}");
    eprintln!("limux: {detail}");
    show_runtime_error(state, "Failed to save settings", &detail);
}

fn broadcast_font_size(size: f32) {
    let action = format!("set_font_size:{size}");
    crate::terminal::broadcast_binding_action(&action);
}

fn browser_command_requires_existing_target(command: ShortcutCommand) -> bool {
    command != ShortcutCommand::OpenBrowserInSplit
}

fn dispatch_browser_command(state: &State, command: ShortcutCommand) -> bool {
    if !browser_command_requires_existing_target(command) {
        let uri = match focused_shortcut_target(state) {
            pane::FocusedShortcutTarget::Browser(target) => target.current_uri(),
            _ => None,
        };
        let Some((ws_id, pane_widget)) = find_leaf_focused_pane(state) else {
            return false;
        };
        return split_pane(
            state,
            &ws_id,
            &pane_widget,
            gtk::Orientation::Horizontal,
            SplitPaneOptions {
                initial_state: Some(PaneState::browser_only(uri.as_deref())),
                skip_default_tab: false,
                new_pane_first: false,
                persist: true,
            },
        )
        .is_some();
    }

    let pane::FocusedShortcutTarget::Browser(target) = focused_shortcut_target(state) else {
        return false;
    };

    match command {
        ShortcutCommand::BrowserFocusLocation => target.focus_location(),
        ShortcutCommand::BrowserBack => target.go_back(),
        ShortcutCommand::BrowserForward => target.go_forward(),
        ShortcutCommand::BrowserReload => target.reload(),
        ShortcutCommand::BrowserInspector => target.show_inspector(),
        ShortcutCommand::BrowserConsole => target.show_console(),
        ShortcutCommand::SurfaceFind => target.show_find(),
        ShortcutCommand::SurfaceFindNext => target.find_next(),
        ShortcutCommand::SurfaceFindPrevious => target.find_previous(),
        ShortcutCommand::SurfaceFindHide => target.hide_find(),
        ShortcutCommand::SurfaceUseSelectionForFind => target.use_selection_for_find(),
        _ => false,
    }
}

fn split_focused_pane(state: &State, orientation: gtk::Orientation) {
    if let Some((ws_id, pane_widget)) = find_focused_pane(state) {
        let _ = split_pane(
            state,
            &ws_id,
            &pane_widget,
            orientation,
            SplitPaneOptions {
                initial_state: None,
                skip_default_tab: false,
                new_pane_first: false,
                persist: true,
            },
        );
    }
}

fn cycle_focused_pane_tab(state: &State, delta: i32) {
    if let Some((_ws_id, pane_widget)) = find_focused_pane(state) {
        pane::cycle_tab_in_pane(&pane_widget, delta);
    }
}

fn close_focused_pane(state: &State) {
    if let Some((ws_id, pane_widget)) = find_focused_pane(state) {
        let parent = pane_widget.parent();
        // If this is the only pane (parent is Stack), don't close — keep workspace alive
        if let Some(ref p) = parent {
            if p.downcast_ref::<gtk::Stack>().is_some() {
                return;
            }
        }
        remove_pane(state, &ws_id, &pane_widget);
    }
}

fn close_focused_tab(state: &State) {
    let Some((_ws_id, pane_widget)) = find_focused_pane(state) else {
        return;
    };

    let config = state.borrow().config.clone();
    let keep_workspace_open = config
        .borrow()
        .workspace
        .keep_open_after_last_terminal_closes;
    if let Some(parent) = pane_widget.parent() {
        if parent.downcast_ref::<gtk::Stack>().is_some()
            && pane::tab_count_in_pane(&pane_widget) <= 1
            && !keep_workspace_open
        {
            return;
        }
    }

    if let Some(tab_id) = pane::active_tab_in_pane(&pane_widget) {
        pane::close_tab_in_pane(&pane_widget, &tab_id);
    }
}

fn toggle_focused_pane_zoom(state: &State) {
    let Some((ws_id, pane_widget)) = find_focused_pane(state) else {
        return;
    };
    let container = {
        let s = state.borrow();
        s.workspaces
            .iter()
            .find(|workspace| workspace.id == ws_id)
            .map(|workspace| workspace.split_container.clone())
    };
    if let Some(container) = container {
        container.toggle_zoom(&pane_widget);
    }
}

fn add_tab_to_focused_pane(_state: &State, _browser: bool) {
    if let Some((_ws_id, pane_widget)) = find_focused_pane(_state) {
        if _browser {
            pane::add_browser_tab_to_pane(&pane_widget);
        } else {
            pane::add_terminal_tab_to_pane(&pane_widget);
        }
    }
}

/// Direction for pane navigation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PaneBounds {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct NeighborScore {
    has_overlap: bool,
    overlap: i32,
    gap: i32,
    center_delta: i32,
}

/// Focus the neighboring pane in the given direction by walking the gtk::Paned tree.
fn focus_pane_in_direction(state: &State, direction: Direction) {
    let (_ws_id, pane_widget) = match find_focused_pane(state) {
        Some(v) => v,
        None => return,
    };

    let Some(leaf) = pane_in_direction(state, &pane_widget, direction) else {
        return;
    };
    if let Some(gl) = find_gl_area(&leaf) {
        gl.grab_focus();
    }
}

fn pane_in_direction(
    state: &State,
    pane_widget: &gtk::Widget,
    direction: Direction,
) -> Option<gtk::Widget> {
    let root = state.borrow().window.clone().upcast::<gtk::Widget>();

    // Determine which axis and sides we care about.
    let (target_orientation, must_be_start) = match direction {
        Direction::Left => (gtk::Orientation::Horizontal, false), // must be end_child to go left
        Direction::Right => (gtk::Orientation::Horizontal, true), // must be start_child to go right
        Direction::Up => (gtk::Orientation::Vertical, false),     // must be end_child to go up
        Direction::Down => (gtk::Orientation::Vertical, true),    // must be start_child to go down
    };

    // Walk up from the focused pane to find a gtk::Paned with the right
    // orientation where the current subtree is on the correct side.
    let mut current: gtk::Widget = pane_widget.clone();
    loop {
        let parent = current.parent()?;
        if let Some(paned) = parent.downcast_ref::<gtk::Paned>() {
            if paned.orientation() == target_orientation {
                let is_start = paned.start_child().map(|c| c == current).unwrap_or(false);
                if is_start == must_be_start {
                    // Found the split point. Navigate to the sibling subtree.
                    let sibling = if must_be_start {
                        paned.end_child()
                    } else {
                        paned.start_child()
                    };
                    if let Some(sibling) = sibling {
                        return Some(
                            best_directional_leaf_pane(pane_widget, &sibling, &root, direction)
                                .unwrap_or_else(|| {
                                    let prefer_start = !must_be_start;
                                    find_leaf_pane(&sibling, target_orientation, prefer_start)
                                }),
                        );
                    }
                    return None;
                }
            }
        }
        current = parent;
    }
}

fn widget_bounds_in_root(widget: &gtk::Widget, root: &gtk::Widget) -> Option<PaneBounds> {
    let allocation = widget.allocation();
    let width = allocation.width();
    let height = allocation.height();
    if width <= 0 || height <= 0 {
        return None;
    }

    let (left, top) = widget.translate_coordinates(root, 0.0, 0.0)?;
    Some(PaneBounds {
        left,
        top,
        right: left + f64::from(width),
        bottom: top + f64::from(height),
    })
}

fn overlap_1d(a_start: f64, a_end: f64, b_start: f64, b_end: f64) -> i32 {
    (a_end.min(b_end) - a_start.max(b_start)).max(0.0).round() as i32
}

fn directional_neighbor_score(
    current: PaneBounds,
    candidate: PaneBounds,
    direction: Direction,
) -> Option<NeighborScore> {
    let (gap, overlap, current_center, candidate_center) = match direction {
        Direction::Left => (
            current.left - candidate.right,
            overlap_1d(current.top, current.bottom, candidate.top, candidate.bottom),
            (current.top + current.bottom) / 2.0,
            (candidate.top + candidate.bottom) / 2.0,
        ),
        Direction::Right => (
            candidate.left - current.right,
            overlap_1d(current.top, current.bottom, candidate.top, candidate.bottom),
            (current.top + current.bottom) / 2.0,
            (candidate.top + candidate.bottom) / 2.0,
        ),
        Direction::Up => (
            current.top - candidate.bottom,
            overlap_1d(current.left, current.right, candidate.left, candidate.right),
            (current.left + current.right) / 2.0,
            (candidate.left + candidate.right) / 2.0,
        ),
        Direction::Down => (
            candidate.top - current.bottom,
            overlap_1d(current.left, current.right, candidate.left, candidate.right),
            (current.left + current.right) / 2.0,
            (candidate.left + candidate.right) / 2.0,
        ),
    };

    if gap < -0.5 {
        return None;
    }

    Some(NeighborScore {
        has_overlap: overlap > 0,
        overlap,
        gap: gap.max(0.0).round() as i32,
        center_delta: (candidate_center - current_center).abs().round() as i32,
    })
}

fn neighbor_score_better(candidate: NeighborScore, best: NeighborScore) -> bool {
    (
        candidate.has_overlap,
        candidate.overlap,
        -candidate.gap,
        -candidate.center_delta,
    ) > (
        best.has_overlap,
        best.overlap,
        -best.gap,
        -best.center_delta,
    )
}

fn collect_leaf_panes(widget: &gtk::Widget, panes: &mut Vec<gtk::Widget>) {
    if pane::is_pane_widget(widget) {
        panes.push(widget.clone());
        return;
    }

    if let Some(paned) = widget.downcast_ref::<gtk::Paned>() {
        if let Some(child) = paned.start_child() {
            collect_leaf_panes(&child, panes);
        }
        if let Some(child) = paned.end_child() {
            collect_leaf_panes(&child, panes);
        }
        return;
    }

    if let Some(stack) = widget.downcast_ref::<gtk::Stack>() {
        if let Some(visible) = stack.visible_child() {
            collect_leaf_panes(&visible, panes);
        }
        return;
    }

    let mut child = widget.first_child();
    while let Some(current) = child {
        collect_leaf_panes(&current, panes);
        child = current.next_sibling();
    }
}

fn best_directional_leaf_pane(
    current_pane: &gtk::Widget,
    sibling_subtree: &gtk::Widget,
    root: &gtk::Widget,
    direction: Direction,
) -> Option<gtk::Widget> {
    let current_bounds = widget_bounds_in_root(current_pane, root)?;
    let mut leaves = Vec::new();
    collect_leaf_panes(sibling_subtree, &mut leaves);

    let mut best: Option<(gtk::Widget, NeighborScore)> = None;
    for leaf in leaves {
        let Some(bounds) = widget_bounds_in_root(&leaf, root) else {
            continue;
        };
        let Some(score) = directional_neighbor_score(current_bounds, bounds, direction) else {
            continue;
        };

        let should_replace = best
            .as_ref()
            .map(|(_, best_score)| neighbor_score_better(score, *best_score))
            .unwrap_or(true);
        if should_replace {
            best = Some((leaf, score));
        }
    }

    best.map(|(leaf, _)| leaf)
}

/// Recursively find the first visible GLArea inside a widget tree.
/// For gtk::Stack containers, only descend into the visible child.
pub(crate) fn find_gl_area(widget: &gtk::Widget) -> Option<gtk::GLArea> {
    if let Some(gl) = widget.downcast_ref::<gtk::GLArea>() {
        return Some(gl.clone());
    }
    // For Stack widgets, only search the visible child
    if let Some(stack) = widget.downcast_ref::<gtk::Stack>() {
        if let Some(visible) = stack.visible_child() {
            return find_gl_area(&visible);
        }
        return None;
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        if let Some(gl) = find_gl_area(&c) {
            return Some(gl);
        }
        child = c.next_sibling();
    }
    None
}

/// Descend a pane/split subtree to find a leaf pane widget.
/// When encountering a gtk::Paned matching `axis`, prefer `start_child` if
/// `prefer_start` is true (to find the nearest edge). For Paned widgets on
/// the other axis, prefer start_child (arbitrary but consistent).
fn find_leaf_pane(widget: &gtk::Widget, axis: gtk::Orientation, prefer_start: bool) -> gtk::Widget {
    if let Some(paned) = widget.downcast_ref::<gtk::Paned>() {
        let pick_start = if paned.orientation() == axis {
            prefer_start
        } else {
            true // arbitrary default for orthogonal splits
        };
        let child = if pick_start {
            paned.start_child()
        } else {
            paned.end_child()
        };
        match child {
            Some(c) => find_leaf_pane(&c, axis, prefer_start),
            None => widget.clone(),
        }
    } else if let Some(box_widget) = widget.downcast_ref::<gtk::Box>() {
        // A gtk::Box may be a pane widget (leaf) or a SplitTreeContainer bin
        // wrapping a single pane or paned. Descend into non-pane boxes.
        if !pane::is_pane_widget(widget) {
            if let Some(first_child) = box_widget.first_child() {
                return find_leaf_pane(&first_child, axis, prefer_start);
            }
        }
        widget.clone()
    } else {
        widget.clone()
    }
}

fn should_emit_desktop_notification(
    desktop_notifications_enabled: bool,
    window_active: bool,
    workspace_is_active: bool,
    source_focused: bool,
) -> bool {
    desktop_notifications_enabled && (!window_active || !workspace_is_active || !source_focused)
}

fn resolve_notification_tab_target(
    state: &State,
    preferred_index: usize,
    surface_hint: &str,
) -> (usize, Option<(u32, String)>) {
    let workspace_ids = state
        .borrow()
        .workspaces
        .iter()
        .map(|workspace| workspace.id.clone())
        .collect::<Vec<_>>();

    match pane::tab_target_for_workspace(&workspace_ids[preferred_index], surface_hint) {
        pane::TabTargetResolution::Unique(pane_id, tab_id) => {
            return (preferred_index, Some((pane_id, tab_id)));
        }
        pane::TabTargetResolution::Ambiguous => return (preferred_index, None),
        pane::TabTargetResolution::NotFound => {}
    }

    let mut target = None;
    for (index, workspace_id) in workspace_ids.iter().enumerate() {
        if index == preferred_index {
            continue;
        }
        match pane::tab_target_for_workspace(workspace_id, surface_hint) {
            pane::TabTargetResolution::Unique(pane_id, tab_id) if target.is_none() => {
                target = Some((index, pane_id, tab_id));
            }
            pane::TabTargetResolution::Unique(_, _) | pane::TabTargetResolution::Ambiguous => {
                return (preferred_index, None);
            }
            pane::TabTargetResolution::NotFound => {}
        }
    }

    target
        .map(|(index, pane_id, tab_id)| (index, Some((pane_id, tab_id))))
        .unwrap_or((preferred_index, None))
}

fn has_unread(workspace_unread: bool, tab_unread: bool) -> bool {
    workspace_unread || tab_unread
}

fn set_workspace_unread_visual(workspace: &Workspace, unread: bool) {
    if unread {
        workspace
            .notify_dot
            .remove_css_class("limux-notify-dot-hidden");
        workspace.notify_dot.add_css_class("limux-notify-dot");
        workspace.notify_label.remove_css_class("limux-notify-msg");
        workspace
            .notify_label
            .add_css_class("limux-notify-msg-unread");
        workspace
            .notify_label
            .set_visible(!workspace.notify_label.label().is_empty());
        if let Some(row_box) = workspace.sidebar_row.child() {
            row_box.add_css_class("limux-sidebar-row-unread");
        }
    } else {
        workspace.notify_dot.remove_css_class("limux-notify-dot");
        workspace
            .notify_dot
            .add_css_class("limux-notify-dot-hidden");
        workspace
            .notify_label
            .remove_css_class("limux-notify-msg-unread");
        workspace.notify_label.add_css_class("limux-notify-msg");
        workspace.notify_label.set_visible(false);
        workspace.notify_label.set_label("");
        if let Some(row_box) = workspace.sidebar_row.child() {
            row_box.remove_css_class("limux-sidebar-row-unread");
        }
    }
}

fn sync_workspace_unread(state: &State, ws_id: &str) {
    let workspace_unread = {
        let s = state.borrow();
        let Some(workspace) = s.workspaces.iter().find(|workspace| workspace.id == ws_id) else {
            return;
        };
        workspace.unread
    };
    let tab_unread = pane::workspace_has_unread_tabs(ws_id);
    let s = state.borrow();
    if let Some(workspace) = s.workspaces.iter().find(|workspace| workspace.id == ws_id) {
        set_workspace_unread_visual(workspace, has_unread(workspace_unread, tab_unread));
    }
}

fn clear_visible_tab_unread(state: &State, ws_id: &str) {
    let root = {
        let mut s = state.borrow_mut();
        if !s.window.is_active() {
            return;
        }
        let Some(workspace) = s
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.id == ws_id)
        else {
            return;
        };
        workspace.unread = false;
        workspace.root.clone()
    };
    pane::clear_active_tab_unread_in_root(&root);
    sync_workspace_unread(state, ws_id);
}

fn mark_workspace_unread(
    state: &State,
    ws_id: &str,
    source_focused: bool,
    target: DesktopNotificationTarget,
) -> Option<DesktopNotificationRequest> {
    mark_workspace_unread_with_message(
        state,
        ws_id,
        "Process needs attention",
        source_focused,
        target,
    )
}

fn workspace_notification_message(title: &str, body: &str) -> String {
    let title = title.trim();
    let body = body.trim();
    match (title.is_empty(), body.is_empty()) {
        (false, false) => format!("{title}: {body}"),
        (false, true) => title.to_string(),
        (true, false) => body.to_string(),
        (true, true) => "Process needs attention".to_string(),
    }
}

fn mark_workspace_unread_with_message(
    state: &State,
    ws_id: &str,
    message: &str,
    source_focused: bool,
    target: DesktopNotificationTarget,
) -> Option<DesktopNotificationRequest> {
    let (workspace_idx, active_idx, workspace_name, window_active, notifications) = {
        let s = state.borrow();
        let workspace_idx = s
            .workspaces
            .iter()
            .position(|workspace| workspace.id == ws_id)?;
        let workspace = &s.workspaces[workspace_idx];
        let notifications = s.config.borrow().notifications;
        (
            workspace_idx,
            s.active_idx,
            workspace.name.clone(),
            s.window.is_active(),
            notifications,
        )
    };
    let workspace_is_active = workspace_idx == active_idx;
    let desktop_request = should_emit_desktop_notification(
        notifications.enabled,
        window_active,
        workspace_is_active,
        source_focused,
    )
    .then(|| DesktopNotificationRequest {
        summary: workspace_name,
        body: message.to_string(),
        sound: notifications.sound,
        target: target.clone(),
    });

    let source_is_unread = !window_active || !workspace_is_active || !source_focused;
    if source_is_unread {
        let tab_target = target.pane_id.zip(target.tab_id.as_deref());
        let tab_was_targeted = tab_target
            .and_then(|(pane_id, tab_id)| {
                pane::mark_tab_unread_in_workspace(ws_id, pane_id, tab_id)
            })
            .is_some();
        let mark_workspace_level = !tab_was_targeted;

        if tab_was_targeted || mark_workspace_level {
            let mut s = state.borrow_mut();
            if let Some(workspace) = s
                .workspaces
                .iter_mut()
                .find(|workspace| workspace.id == ws_id)
            {
                workspace.unread |= mark_workspace_level;
                workspace.notify_label.set_label(message);
            }
            drop(s);
            sync_workspace_unread(state, ws_id);
        }
    }

    desktop_request
}

fn desktop_notification_hints(
    sound: app_config::NotificationSound,
) -> HashMap<String, glib::Variant> {
    let mut hints = HashMap::from([("desktop-entry".to_string(), crate::APP_ID.to_variant())]);

    match sound {
        app_config::NotificationSound::Default => {}
        app_config::NotificationSound::None => {
            hints.insert("suppress-sound".to_string(), true.to_variant());
        }
        _ => {
            if let Some(sound_name) = sound.freedesktop_sound_name() {
                let sound_variant = sound_name.to_variant();
                hints.insert("sound-name".to_string(), sound_variant.clone());
                hints.insert("x-canonical-sound-name".to_string(), sound_variant);
            }
        }
    }

    hints
}

fn desktop_notification_actions() -> Vec<String> {
    vec!["default".to_string(), "Open".to_string()]
}

fn show_desktop_notification(state: &State, request: DesktopNotificationRequest) {
    let state = state.clone();
    gio::DBusProxy::for_bus(
        gio::BusType::Session,
        gio::DBusProxyFlags::NONE,
        None::<&gio::DBusInterfaceInfo>,
        FREEDESKTOP_NOTIFICATIONS_SERVICE,
        FREEDESKTOP_NOTIFICATIONS_PATH,
        FREEDESKTOP_NOTIFICATIONS_INTERFACE,
        None::<&gio::Cancellable>,
        move |result| {
            let Ok(proxy) = result else {
                return;
            };
            let route = DesktopNotificationRoute {
                target: request.target.clone(),
                activation_token: None,
            };

            let params = (
                "Limux",
                0u32,
                crate::APP_ID,
                request.summary.as_str(),
                request.body.as_str(),
                desktop_notification_actions(),
                desktop_notification_hints(request.sound),
                DESKTOP_NOTIFICATION_EXPIRE_TIMEOUT_MS,
            )
                .to_variant();

            proxy.call(
                "Notify",
                Some(&params),
                gio::DBusCallFlags::NONE,
                DESKTOP_NOTIFICATION_DBUS_TIMEOUT_MS,
                None::<&gio::Cancellable>,
                move |result| {
                    let Ok(response) = result else {
                        return;
                    };
                    let Some(notification_id) = desktop_notification_id_from_response(&response)
                    else {
                        return;
                    };

                    state
                        .borrow_mut()
                        .desktop_notification_routes
                        .insert(notification_id, route.clone());
                },
            );
        },
    );
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use super::glib;
    use super::gtk::ffi;
    use super::gtk::gdk;
    use super::ToVariant;
    use super::{
        browser_command_requires_existing_target, build_window_css,
        clamp_workspace_insert_index_for_pinning, commit_inline_rename_for_click,
        desktop_notification_action_from_signal, desktop_notification_actions,
        desktop_notification_activation_token_from_signal,
        desktop_notification_closed_id_from_signal, desktop_notification_id_from_response,
        directional_neighbor_score, favorites_prefix_len, find_leaf_pane, font_size_after_delta,
        ghostty_prefers_dark, gtk_system_prefers_dark_from_raw, has_unread,
        initial_workspace_state, keep_workspace_open_after_empty_pane, next_active_workspace_index,
        pane_create_split_placement, queue_session_save_request, resolve_pane_create_source_id,
        resolved_system_prefers_dark, sanitize_background_opacity,
        shortcut_allowed_while_browser_find_active, shortcut_blocked_by_editable,
        shortcut_command_from_key_event, shortcut_dispatch_propagation,
        should_emit_desktop_notification, tab_drag_workspace_seed, use_opaque_window_background,
        validate_workspace_folder_input_with_dirs, workspace_drop_layout_path,
        workspace_folder_path_from_input, workspace_notification_message, workspace_path_visible,
        Direction, EditableCaptureContext, NeighborScore, PaneBounds, PaneCreateDirection,
        PaneCreateTargetError, PortalColorSchemePreference, SessionSaveAccess, SessionSaveRequest,
        WorkspaceSeedSource, BASE_CSS, HOST_ENTRY_CSS_CLASS, WORKSPACE_RENAME_ENTRY_CSS_CLASS,
        WORKSPACE_RENAME_ENTRY_CSS_CLASSES,
    };
    use crate::layout_state::{LayoutNodeState, PaneState, SplitOrientation, SplitState};
    use crate::shortcut_config::{
        default_shortcuts, resolve_shortcuts_from_str, EditableCapturePolicy, ShortcutCommand,
    };
    #[derive(Default)]
    struct TestSessionSaveState {
        persistence_suspended: bool,
        save_queued: bool,
    }

    impl SessionSaveAccess for TestSessionSaveState {
        fn persistence_suspended(&self) -> bool {
            self.persistence_suspended
        }

        fn save_queued(&self) -> bool {
            self.save_queued
        }

        fn set_save_queued(&mut self, queued: bool) {
            self.save_queued = queued;
        }
    }

    #[test]
    fn favorites_prefix_len_counts_only_leading_favorites() {
        let flags = [true, true, false, true, false];
        assert_eq!(favorites_prefix_len(&flags), 2);
    }

    #[test]
    fn workspace_lifecycle_preference_only_preserves_a_single_closed_pane() {
        assert!(keep_workspace_open_after_empty_pane(
            true,
            crate::pane::PaneEmptyReason::ClosedLastTerminal,
            true,
        ));
        assert!(!keep_workspace_open_after_empty_pane(
            false,
            crate::pane::PaneEmptyReason::ClosedLastTerminal,
            true,
        ));
        assert!(!keep_workspace_open_after_empty_pane(
            true,
            crate::pane::PaneEmptyReason::ClosedLastTerminal,
            false,
        ));
        assert!(!keep_workspace_open_after_empty_pane(
            true,
            crate::pane::PaneEmptyReason::ClosedLastTab,
            true,
        ));
        assert!(!keep_workspace_open_after_empty_pane(
            true,
            crate::pane::PaneEmptyReason::MovedLastTabOut,
            true,
        ));
    }

    #[test]
    fn sanitize_background_opacity_clamps_invalid_values() {
        assert_eq!(sanitize_background_opacity(f64::NAN), 1.0);
        assert_eq!(sanitize_background_opacity(-0.2), 0.0);
        assert_eq!(sanitize_background_opacity(1.7), 1.0);
        assert_eq!(sanitize_background_opacity(0.42), 0.42);
    }

    #[test]
    fn transparent_window_background_only_applies_below_full_opacity() {
        assert!(!use_opaque_window_background(0.8));
        assert!(use_opaque_window_background(1.0));
        assert!(use_opaque_window_background(5.0));
        assert!(use_opaque_window_background(f64::NAN));
    }

    #[test]
    fn directional_neighbor_score_prefers_row_overlap_when_moving_left() {
        let current = PaneBounds {
            left: 100.0,
            top: 100.0,
            right: 200.0,
            bottom: 200.0,
        };
        let top_left = PaneBounds {
            left: 0.0,
            top: 0.0,
            right: 100.0,
            bottom: 100.0,
        };
        let bottom_left = PaneBounds {
            left: 0.0,
            top: 100.0,
            right: 100.0,
            bottom: 200.0,
        };

        let top_score =
            directional_neighbor_score(current, top_left, Direction::Left).expect("top score");
        let bottom_score = directional_neighbor_score(current, bottom_left, Direction::Left)
            .expect("bottom score");

        assert_eq!(
            top_score,
            NeighborScore {
                has_overlap: false,
                overlap: 0,
                gap: 0,
                center_delta: 100,
            }
        );
        assert_eq!(
            bottom_score,
            NeighborScore {
                has_overlap: true,
                overlap: 100,
                gap: 0,
                center_delta: 0,
            }
        );
    }

    #[test]
    fn directional_neighbor_score_prefers_column_overlap_when_moving_up() {
        let current = PaneBounds {
            left: 100.0,
            top: 100.0,
            right: 200.0,
            bottom: 200.0,
        };
        let top_left = PaneBounds {
            left: 0.0,
            top: 0.0,
            right: 100.0,
            bottom: 100.0,
        };
        let top_right = PaneBounds {
            left: 100.0,
            top: 0.0,
            right: 200.0,
            bottom: 100.0,
        };

        let left_score =
            directional_neighbor_score(current, top_left, Direction::Up).expect("left score");
        let right_score =
            directional_neighbor_score(current, top_right, Direction::Up).expect("right score");

        assert_eq!(left_score.overlap, 0);
        assert_eq!(right_score.overlap, 100);
        assert!(right_score.has_overlap);
    }

    #[test]
    fn pane_create_split_placement_maps_direction_to_orientation_and_order() {
        assert_eq!(
            pane_create_split_placement(PaneCreateDirection::Left),
            super::PaneCreateSplitPlacement {
                orientation: super::gtk::Orientation::Horizontal,
                new_pane_first: true,
            }
        );
        assert_eq!(
            pane_create_split_placement(PaneCreateDirection::Right),
            super::PaneCreateSplitPlacement {
                orientation: super::gtk::Orientation::Horizontal,
                new_pane_first: false,
            }
        );
        assert_eq!(
            pane_create_split_placement(PaneCreateDirection::Up),
            super::PaneCreateSplitPlacement {
                orientation: super::gtk::Orientation::Vertical,
                new_pane_first: true,
            }
        );
        assert_eq!(
            pane_create_split_placement(PaneCreateDirection::Down),
            super::PaneCreateSplitPlacement {
                orientation: super::gtk::Orientation::Vertical,
                new_pane_first: false,
            }
        );
    }

    #[test]
    fn pane_create_source_prefers_surface_then_pane_then_active_focus_then_first_leaf() {
        let panes = [10, 20, 30];
        let surfaces = [("10:aaa", 10), ("20:bbb", 20)];

        assert_eq!(
            resolve_pane_create_source_id(
                Some("surface:20:bbb"),
                Some(10),
                Some(30),
                true,
                &panes,
                &surfaces,
            ),
            Ok(20)
        );
        assert_eq!(
            resolve_pane_create_source_id(None, Some(10), Some(30), true, &panes, &surfaces),
            Ok(10)
        );
        assert_eq!(
            resolve_pane_create_source_id(None, None, Some(30), true, &panes, &surfaces),
            Ok(30)
        );
        assert_eq!(
            resolve_pane_create_source_id(None, None, Some(30), false, &panes, &surfaces),
            Ok(10)
        );
    }

    #[test]
    fn pane_create_source_reports_invalid_surface_pane_and_empty_workspace() {
        let panes = [10, 20];
        let surfaces = [("10:aaa", 10)];

        assert_eq!(
            resolve_pane_create_source_id(
                Some("missing"),
                Some(10),
                Some(20),
                true,
                &panes,
                &surfaces,
            ),
            Err(PaneCreateTargetError::InvalidSurfaceId(
                "missing".to_string()
            ))
        );
        assert_eq!(
            resolve_pane_create_source_id(None, Some(99), Some(20), true, &panes, &surfaces),
            Err(PaneCreateTargetError::InvalidPaneId(99))
        );
        assert_eq!(
            resolve_pane_create_source_id(None, None, None, true, &[], &[]),
            Err(PaneCreateTargetError::NoPanes)
        );
    }

    #[test]
    fn build_window_css_uses_resolved_background_opacity() {
        let css = build_window_css(0.42);
        assert!(css.contains(".limux-host-entry"));
        assert!(css.contains(".limux-host-entry text"));
        assert!(css.contains(".limux-host-entry text placeholder"));
        assert!(css.contains(".limux-content"));
        assert!(css.contains("background-color: rgba(23, 23, 23, 0.420);"));
    }

    #[test]
    fn font_size_after_delta_uses_default_when_unset() {
        assert_eq!(font_size_after_delta(None, 12.0, 1.0), 13.0);
    }

    #[test]
    fn font_size_after_delta_clamps_to_supported_range() {
        assert_eq!(font_size_after_delta(Some(1.0), 12.0, -5.0), 1.0);
        assert_eq!(font_size_after_delta(Some(255.0), 12.0, 5.0), 255.0);
    }

    #[test]
    fn base_css_defines_theme_aware_host_entry_styles() {
        assert!(BASE_CSS.contains(":root"));
        assert!(BASE_CSS.contains("@media (prefers-color-scheme: dark)"));
        assert!(BASE_CSS.contains(".limux-host-entry"));
        assert!(BASE_CSS.contains(".limux-host-entry text"));
        assert!(BASE_CSS.contains(".limux-host-entry text placeholder"));
        assert!(BASE_CSS.contains("caret-color: currentColor;"));
    }

    #[test]
    fn workspace_rename_entry_uses_shared_host_entry_class() {
        assert_eq!(
            WORKSPACE_RENAME_ENTRY_CSS_CLASSES,
            [HOST_ENTRY_CSS_CLASS, WORKSPACE_RENAME_ENTRY_CSS_CLASS]
        );
        assert!(BASE_CSS.contains(".limux-ws-rename-entry"));
    }

    #[test]
    fn outside_rename_click_commits_while_inside_click_preserves_editor() {
        for (click, should_commit) in [
            (Some((0.0, 0.0)), false),
            (Some((120.0, 32.0)), false),
            (Some((-0.1, 16.0)), true),
            (Some((60.0, 32.1)), true),
            (None, true),
        ] {
            let entry_present = Cell::new(true);
            commit_inline_rename_for_click(click, 120, 32, || entry_present.set(false));
            assert_eq!(entry_present.get(), !should_commit, "click: {click:?}");
        }
    }

    #[test]
    fn desktop_notification_actions_include_default_open_action() {
        assert_eq!(
            desktop_notification_actions(),
            vec!["default".to_string(), "Open".to_string()]
        );
    }

    #[test]
    fn desktop_notification_response_and_signal_parsers_match_dbus_shapes() {
        assert_eq!(
            desktop_notification_id_from_response(&(42u32,).to_variant()),
            Some(42)
        );
        assert_eq!(
            desktop_notification_action_from_signal(&(42u32, "default".to_string()).to_variant()),
            Some((42, "default".to_string()))
        );
        assert_eq!(
            desktop_notification_activation_token_from_signal(
                &(42u32, "token-123".to_string()).to_variant()
            ),
            Some((42, "token-123".to_string()))
        );
        assert_eq!(
            desktop_notification_closed_id_from_signal(&(42u32, 2u32).to_variant()),
            Some(42)
        );
    }

    #[test]
    fn queue_session_save_request_sets_queued_once() {
        let state = Rc::new(RefCell::new(TestSessionSaveState::default()));

        assert_eq!(
            queue_session_save_request(&state),
            SessionSaveRequest::FlushOnIdle
        );
        assert!(state.borrow().save_queued);
        assert_eq!(
            queue_session_save_request(&state),
            SessionSaveRequest::Ignore
        );
    }

    #[test]
    fn queue_session_save_request_retries_when_state_is_already_borrowed() {
        let state = Rc::new(RefCell::new(TestSessionSaveState::default()));
        let borrow = state.borrow_mut();

        assert_eq!(
            queue_session_save_request(&state),
            SessionSaveRequest::RetryOnIdle
        );

        drop(borrow);
        assert!(!state.borrow().save_queued);
    }

    #[test]
    fn queue_session_save_request_ignores_suspended_persistence() {
        let state = Rc::new(RefCell::new(TestSessionSaveState {
            persistence_suspended: true,
            save_queued: false,
        }));

        assert_eq!(
            queue_session_save_request(&state),
            SessionSaveRequest::Ignore
        );
        assert!(!state.borrow().save_queued);
    }

    #[test]
    fn unpinned_workspace_cannot_move_above_favorites() {
        // Remaining order after removing dragged workspace:
        // [fav, fav, unfav, unfav]
        let after_removal = [true, true, false, false];
        let clamped = clamp_workspace_insert_index_for_pinning(&after_removal, false, 0);
        assert_eq!(clamped, 2);
    }

    #[test]
    fn favorite_workspace_cannot_move_below_unpinned() {
        // Remaining order after removing dragged favorite:
        // [fav, fav, unfav, unfav]
        let after_removal = [true, true, false, false];
        let clamped =
            clamp_workspace_insert_index_for_pinning(&after_removal, true, after_removal.len());
        assert_eq!(clamped, 2);
    }

    #[test]
    fn system_prefers_dark_from_raw_maps_known_values() {
        assert_eq!(
            gtk_system_prefers_dark_from_raw(Some(ffi::GTK_INTERFACE_COLOR_SCHEME_DARK)),
            Some(true)
        );
        assert_eq!(
            gtk_system_prefers_dark_from_raw(Some(ffi::GTK_INTERFACE_COLOR_SCHEME_LIGHT)),
            Some(false)
        );
        assert_eq!(
            gtk_system_prefers_dark_from_raw(Some(ffi::GTK_INTERFACE_COLOR_SCHEME_DEFAULT)),
            Some(false)
        );
        assert_eq!(
            gtk_system_prefers_dark_from_raw(Some(ffi::GTK_INTERFACE_COLOR_SCHEME_UNSUPPORTED)),
            None
        );
    }

    #[test]
    fn portal_color_scheme_preference_resolves_with_gnome_fallback() {
        assert_eq!(
            PortalColorSchemePreference::from_raw(1),
            Some(PortalColorSchemePreference::Dark)
        );
        assert_eq!(
            PortalColorSchemePreference::from_raw(2),
            Some(PortalColorSchemePreference::Light)
        );
        assert_eq!(
            PortalColorSchemePreference::from_raw(0),
            Some(PortalColorSchemePreference::Default)
        );
        assert_eq!(
            resolved_system_prefers_dark(PortalColorSchemePreference::Dark, Some(false)),
            Some(true)
        );
        assert_eq!(
            resolved_system_prefers_dark(PortalColorSchemePreference::Light, Some(true)),
            Some(false)
        );
        assert_eq!(
            resolved_system_prefers_dark(PortalColorSchemePreference::Default, Some(true)),
            Some(true)
        );
        assert_eq!(
            resolved_system_prefers_dark(PortalColorSchemePreference::Unknown, Some(false)),
            Some(false)
        );
    }

    #[test]
    fn ghostty_prefers_dark_uses_system_preference_when_requested() {
        assert!(ghostty_prefers_dark(
            crate::app_config::ColorScheme::System,
            Some(true),
            false
        ));
        assert!(!ghostty_prefers_dark(
            crate::app_config::ColorScheme::System,
            Some(false),
            true
        ));
        assert!(ghostty_prefers_dark(
            crate::app_config::ColorScheme::System,
            None,
            true
        ));
    }

    #[test]
    fn ghostty_prefers_dark_honors_explicit_overrides() {
        assert!(ghostty_prefers_dark(
            crate::app_config::ColorScheme::Dark,
            Some(false),
            false
        ));
        assert!(!ghostty_prefers_dark(
            crate::app_config::ColorScheme::Light,
            Some(true),
            true
        ));
    }

    #[test]
    fn workspace_notification_message_prefers_title_and_body() {
        assert_eq!(
            workspace_notification_message("Codex", "Turn complete"),
            "Codex: Turn complete"
        );
        assert_eq!(workspace_notification_message("Codex", ""), "Codex");
        assert_eq!(
            workspace_notification_message("", "Turn complete"),
            "Turn complete"
        );
        assert_eq!(
            workspace_notification_message("  ", "  "),
            "Process needs attention"
        );
    }

    #[test]
    fn desktop_notifications_only_fire_for_background_workspaces() {
        assert!(should_emit_desktop_notification(true, false, false, false));
        assert!(should_emit_desktop_notification(true, true, false, false));
        assert!(should_emit_desktop_notification(true, true, true, false));
        assert!(!should_emit_desktop_notification(
            false, false, false, false
        ));
        assert!(!should_emit_desktop_notification(true, true, true, true));
    }

    #[test]
    fn workspace_badge_stays_unread_while_any_tab_is_unread() {
        assert!(has_unread(false, true));
        assert!(has_unread(true, false));
        assert!(!has_unread(false, false));
    }

    #[test]
    fn shortcut_command_from_key_event_uses_default_registry_bindings() {
        let shortcuts = default_shortcuts();

        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::T,
                gdk::ModifierType::CONTROL_MASK
            ),
            None
        );
        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::T,
                gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK
            ),
            Some(ShortcutCommand::NewTerminal)
        );
        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::Page_Down,
                gdk::ModifierType::CONTROL_MASK
            ),
            None
        );
        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::Page_Down,
                gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK
            ),
            Some(ShortcutCommand::NextWorkspace)
        );
        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::F,
                gdk::ModifierType::CONTROL_MASK
            ),
            None
        );
        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::F,
                gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK
            ),
            Some(ShortcutCommand::SurfaceFind)
        );
        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::C,
                gdk::ModifierType::CONTROL_MASK
            ),
            None
        );
        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::C,
                gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK
            ),
            Some(ShortcutCommand::TerminalCopy)
        );
        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::W,
                gdk::ModifierType::CONTROL_MASK
            ),
            None
        );
        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::W,
                gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK
            ),
            Some(ShortcutCommand::CloseFocusedTab)
        );
        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::W,
                gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK
            ),
            Some(ShortcutCommand::CloseFocusedPane)
        );
        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::Q,
                gdk::ModifierType::CONTROL_MASK
            ),
            None
        );
        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::Q,
                gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK
            ),
            Some(ShortcutCommand::QuitApp)
        );
        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::N,
                gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK
            ),
            Some(ShortcutCommand::NewInstance)
        );
        assert_eq!(
            shortcut_command_from_key_event(&shortcuts, gdk::Key::F11, gdk::ModifierType::empty()),
            Some(ShortcutCommand::ToggleFullscreen)
        );
        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::M,
                gdk::ModifierType::CONTROL_MASK
            ),
            None
        );
        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::M,
                gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK
            ),
            Some(ShortcutCommand::ToggleSidebar)
        );
        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::M,
                gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK
            ),
            None
        );
        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::M,
                gdk::ModifierType::CONTROL_MASK
                    | gdk::ModifierType::ALT_MASK
                    | gdk::ModifierType::SHIFT_MASK
            ),
            Some(ShortcutCommand::ToggleTopBar)
        );
    }

    #[test]
    fn shortcut_command_from_key_event_honors_remaps_and_disables_old_binding() {
        let shortcuts = resolve_shortcuts_from_str(
            r#"{
                "shortcuts": {
                    "toggle_sidebar": "<Ctrl><Alt>b"
                }
            }"#,
        )
        .unwrap();

        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::M,
                gdk::ModifierType::CONTROL_MASK
            ),
            None
        );
        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::B,
                gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK
            ),
            Some(ShortcutCommand::ToggleSidebar)
        );
    }

    #[test]
    fn shortcut_command_from_key_event_respects_explicit_unbinds() {
        let shortcuts = resolve_shortcuts_from_str(
            r#"{
                "shortcuts": {
                    "toggle_sidebar": null
                }
            }"#,
        )
        .unwrap();

        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::M,
                gdk::ModifierType::CONTROL_MASK
            ),
            None
        );
    }

    #[test]
    fn shortcut_command_from_key_event_honors_super_remaps() {
        let shortcuts = resolve_shortcuts_from_str(
            r#"{
                "shortcuts": {
                    "toggle_sidebar": "<Super>b"
                }
            }"#,
        )
        .unwrap();

        assert_eq!(
            shortcut_command_from_key_event(
                &shortcuts,
                gdk::Key::M,
                gdk::ModifierType::CONTROL_MASK
            ),
            None
        );
        assert_eq!(
            shortcut_command_from_key_event(&shortcuts, gdk::Key::B, gdk::ModifierType::SUPER_MASK),
            Some(ShortcutCommand::ToggleSidebar)
        );
    }

    #[test]
    fn shortcut_dispatch_propagation_stops_only_when_window_claims_shortcut() {
        assert_eq!(shortcut_dispatch_propagation(true), glib::Propagation::Stop);
        assert_eq!(
            shortcut_dispatch_propagation(false),
            glib::Propagation::Proceed
        );
    }

    #[test]
    fn shortcut_blocked_by_editable_only_bypasses_non_global_shortcuts() {
        assert!(shortcut_blocked_by_editable(
            ShortcutCommand::SurfaceFind,
            EditableCapturePolicy::BypassInEditable,
            EditableCaptureContext {
                gtk_editable: true,
                ..EditableCaptureContext::default()
            }
        ));
        assert!(!shortcut_blocked_by_editable(
            ShortcutCommand::SurfaceFind,
            EditableCapturePolicy::AlwaysCapture,
            EditableCaptureContext {
                gtk_editable: true,
                ..EditableCaptureContext::default()
            }
        ));
        assert!(!shortcut_blocked_by_editable(
            ShortcutCommand::SurfaceFind,
            EditableCapturePolicy::BypassInEditable,
            EditableCaptureContext::default()
        ));
    }

    #[test]
    fn shortcut_blocked_by_editable_blocks_dom_editable_browser_content() {
        assert!(shortcut_blocked_by_editable(
            ShortcutCommand::BrowserReload,
            EditableCapturePolicy::BypassInEditable,
            EditableCaptureContext {
                browser_dom_editable: true,
                ..EditableCaptureContext::default()
            }
        ));
    }

    #[test]
    fn browser_find_navigation_shortcuts_are_allowed_while_find_ui_is_active() {
        let context = EditableCaptureContext {
            gtk_editable: true,
            browser_find_active: true,
            ..EditableCaptureContext::default()
        };

        assert!(!shortcut_blocked_by_editable(
            ShortcutCommand::SurfaceFindNext,
            EditableCapturePolicy::BypassInEditable,
            context
        ));
        assert!(!shortcut_blocked_by_editable(
            ShortcutCommand::SurfaceFindPrevious,
            EditableCapturePolicy::BypassInEditable,
            context
        ));
        assert!(!shortcut_blocked_by_editable(
            ShortcutCommand::SurfaceFindHide,
            EditableCapturePolicy::BypassInEditable,
            context
        ));
        assert!(shortcut_blocked_by_editable(
            ShortcutCommand::SurfaceFind,
            EditableCapturePolicy::BypassInEditable,
            context
        ));
    }

    #[test]
    fn browser_find_active_exception_is_limited_to_navigation_shortcuts() {
        assert!(shortcut_allowed_while_browser_find_active(
            ShortcutCommand::SurfaceFindNext
        ));
        assert!(shortcut_allowed_while_browser_find_active(
            ShortcutCommand::SurfaceFindPrevious
        ));
        assert!(shortcut_allowed_while_browser_find_active(
            ShortcutCommand::SurfaceFindHide
        ));
        assert!(!shortcut_allowed_while_browser_find_active(
            ShortcutCommand::SurfaceFind
        ));
    }

    #[test]
    fn open_browser_in_split_does_not_require_an_existing_browser_target() {
        assert!(!browser_command_requires_existing_target(
            ShortcutCommand::OpenBrowserInSplit
        ));
        assert!(browser_command_requires_existing_target(
            ShortcutCommand::BrowserReload
        ));
    }

    #[test]
    fn workspace_drop_layout_path_prefers_deterministic_startmost_leaf() {
        let layout = LayoutNodeState::Split(SplitState {
            orientation: SplitOrientation::Horizontal,
            ratio: 0.5,
            start: Box::new(LayoutNodeState::Split(SplitState {
                orientation: SplitOrientation::Vertical,
                ratio: 0.5,
                start: Box::new(LayoutNodeState::Pane(PaneState::fallback(Some("/a")))),
                end: Box::new(LayoutNodeState::Pane(PaneState::fallback(Some("/b")))),
            })),
            end: Box::new(LayoutNodeState::Pane(PaneState::fallback(Some("/c")))),
        });

        assert_eq!(workspace_drop_layout_path(&layout), vec![true, true]);
    }

    #[test]
    fn next_active_workspace_index_preserves_current_active_workspace() {
        let remaining = ["source-b", "destination", "other"];
        assert_eq!(
            next_active_workspace_index(&remaining, Some("destination"), 0),
            1
        );
    }

    #[test]
    fn next_active_workspace_index_falls_back_to_removed_slot_when_active_is_gone() {
        let remaining = ["left", "right"];
        assert_eq!(next_active_workspace_index(&remaining, Some("gone"), 1), 1);
    }

    #[test]
    fn tab_drag_workspace_seed_uses_terminal_cwd_for_folder_path() {
        let seed = tab_drag_workspace_seed(
            WorkspaceSeedSource {
                workspace_cwd: Some("/workspace".to_string()),
                workspace_folder_path: Some("/workspace".to_string()),
            },
            "Project Shell",
            Some("/project".to_string()),
        );

        assert_eq!(seed.name, "Project Shell");
        assert_eq!(seed.cwd.as_deref(), Some("/project"));
        assert_eq!(seed.folder_path.as_deref(), Some("/project"));
    }

    #[test]
    fn tab_drag_workspace_seed_uses_workspace_directory_for_non_terminal_tab() {
        let seed = tab_drag_workspace_seed(
            WorkspaceSeedSource {
                workspace_cwd: Some("/workspace-cwd".to_string()),
                workspace_folder_path: Some("/workspace-folder".to_string()),
            },
            "Browser",
            None,
        );

        assert_eq!(seed.name, "Browser");
        assert_eq!(seed.cwd.as_deref(), Some("/workspace-folder"));
        assert_eq!(seed.folder_path.as_deref(), Some("/workspace-folder"));
    }

    #[test]
    fn workspace_path_visibility_requires_path_and_enabled_setting() {
        assert!(workspace_path_visible(Some("/workspace"), true));
        assert!(!workspace_path_visible(Some("/workspace"), false));
        assert!(!workspace_path_visible(None, true));
    }

    #[test]
    fn workspace_folder_path_input_expands_home_and_relative_paths() {
        let home = std::path::Path::new("/home/tester");
        let current = std::path::Path::new("/tmp/current");

        assert_eq!(
            workspace_folder_path_from_input("~/project", Some(home), Some(current)).unwrap(),
            std::path::PathBuf::from("/home/tester/project")
        );
        assert_eq!(
            workspace_folder_path_from_input("relative", Some(home), Some(current)).unwrap(),
            std::path::PathBuf::from("/tmp/current/relative")
        );
    }

    #[test]
    fn initial_workspace_uses_home_with_a_terminal_fallback() {
        let workspace = initial_workspace_state(
            Some(std::path::Path::new("/home/tester")),
            Some(std::path::Path::new("/tmp/current")),
        );

        assert_eq!(workspace.name, "tester");
        assert_eq!(workspace.cwd.as_deref(), Some("/home/tester"));
        assert_eq!(workspace.folder_path.as_deref(), Some("/home/tester"));
        let LayoutNodeState::Pane(pane) = workspace.layout else {
            panic!("initial workspace must contain a terminal pane");
        };
        assert_eq!(pane.tabs.len(), 1);
    }

    #[test]
    fn workspace_folder_path_input_rejects_empty_value() {
        assert_eq!(
            workspace_folder_path_from_input("  ", None, None).unwrap_err(),
            "Enter a folder path"
        );
    }

    #[test]
    fn workspace_folder_validation_accepts_existing_directory() {
        let dir = tempfile::tempdir().unwrap();
        let selection =
            validate_workspace_folder_input_with_dirs(dir.path().to_str().unwrap(), None, None)
                .unwrap();

        assert_eq!(selection.path_text, dir.path().to_string_lossy());
        assert_eq!(
            selection.name,
            dir.path().file_name().unwrap().to_string_lossy()
        );
    }

    #[test]
    fn workspace_folder_validation_rejects_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("not-a-folder");
        std::fs::write(&file, "content").unwrap();

        let error = validate_workspace_folder_input_with_dirs(file.to_str().unwrap(), None, None)
            .unwrap_err();

        assert!(error.ends_with(" is not a folder"));
    }

    #[test]
    fn find_leaf_pane_descends_into_non_pane_boxes() {
        use gtk4::prelude::{BoxExt, Cast, WidgetExt};

        // GTK widget tests need a display. Skip silently on headless CI.
        if gtk4::init().is_err() {
            return;
        }

        let make_pane = || {
            let pane = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
            let header = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
            header.add_css_class("limux-pane-header");
            pane.append(&header);
            pane
        };

        // A plain Box wrapping a single child simulates a SplitTreeContainer bin.
        let bin = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        let inner_pane = make_pane();
        bin.append(&inner_pane);

        let leaf = find_leaf_pane(&bin.upcast(), gtk4::Orientation::Horizontal, true);
        assert_eq!(
            leaf,
            inner_pane.clone().upcast::<gtk4::Widget>(),
            "find_leaf_pane should descend through a non-pane Box"
        );

        // A Box that is a pane widget should be returned as-is.
        let pane = make_pane();

        let leaf = find_leaf_pane(&pane.clone().upcast(), gtk4::Orientation::Horizontal, true);
        assert_eq!(
            leaf,
            pane.clone().upcast::<gtk4::Widget>(),
            "find_leaf_pane should treat a pane Box as a leaf"
        );

        // A Paned should descend to its actual leaf.
        let paned = gtk4::Paned::new(gtk4::Orientation::Horizontal);
        let left_pane = make_pane();
        paned.set_start_child(Some(&left_pane));
        paned.set_end_child(Some(&make_pane()));

        let leaf = find_leaf_pane(&paned.upcast(), gtk4::Orientation::Horizontal, true);
        assert_eq!(
            leaf,
            left_pane.clone().upcast::<gtk4::Widget>(),
            "find_leaf_pane should descend through a Paned to its leaf"
        );
    }
}
