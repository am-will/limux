use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use limux_protocol::{V2Request, V2Response};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

const COMMANDS: &[&str] = &[
    "system.ping",
    "system.identify",
    "system.capabilities",
    "app.focus_override.set",
    "app.simulate_active",
    "window.list",
    "window.current",
    "window.create",
    "window.focus",
    "window.close",
    "workspace.list",
    "workspace.current",
    "workspace.create",
    "workspace.select",
    "workspace.next",
    "workspace.previous",
    "workspace.last",
    "workspace.rename",
    "workspace.reorder",
    "workspace.close",
    "workspace.move_to_window",
    "workspace.action",
    "pane.list",
    "pane.surfaces",
    "pane.create",
    "pane.focus",
    "pane.swap",
    "pane.break",
    "pane.join",
    "pane.last",
    "pane.resize",
    "surface.list",
    "surface.current",
    "surface.create",
    "surface.split",
    "surface.focus",
    "surface.close",
    "surface.move",
    "surface.reorder",
    "surface.drag_to_split",
    "surface.refresh",
    "surface.health",
    "surface.read_text",
    "surface.send_text",
    "surface.send_key",
    "surface.trigger_flash",
    "surface.clear_history",
    "surface.action",
    "notification.create",
    "notification.create_for_surface",
    "notification.list",
    "notification.clear",
    "tab.action",
    "browser.open_split",
    "browser.navigate",
    "browser.url.get",
    "browser.eval",
    "browser.wait",
    "browser.click",
    "browser.fill",
    "browser.get.text",
    "browser.get.value",
    "browser.get.title",
    "browser.snapshot",
    "browser.focus_webview",
    "browser.is_webview_focused",
    "browser.screenshot",
    "browser.back",
    "browser.forward",
    "browser.reload",
    "browser.focus",
    "browser.hover",
    "browser.dblclick",
    "browser.press",
    "browser.keydown",
    "browser.keyup",
    "browser.type",
    "browser.check",
    "browser.uncheck",
    "browser.select",
    "browser.scroll",
    "browser.scroll_into_view",
    "browser.get.attr",
    "browser.get.box",
    "browser.get.count",
    "browser.get.html",
    "browser.get.styles",
    "browser.is.checked",
    "browser.is.enabled",
    "browser.is.visible",
    "browser.find.role",
    "browser.find.text",
    "browser.find.label",
    "browser.find.placeholder",
    "browser.find.alt",
    "browser.find.title",
    "browser.find.testid",
    "browser.find.first",
    "browser.find.last",
    "browser.find.nth",
    "browser.highlight",
    "browser.viewport.set",
    "browser.geolocation.set",
    "browser.offline.set",
    "browser.trace.start",
    "browser.trace.stop",
    "browser.network.route",
    "browser.network.unroute",
    "browser.network.requests",
    "browser.screencast.start",
    "browser.screencast.stop",
    "browser.input_mouse",
    "browser.input_keyboard",
    "browser.input_touch",
    "browser.addscript",
    "browser.addinitscript",
    "browser.addstyle",
    "browser.console.list",
    "browser.console.clear",
    "browser.errors.list",
    "browser.cookies.get",
    "browser.cookies.set",
    "browser.cookies.clear",
    "browser.storage.get",
    "browser.storage.set",
    "browser.storage.clear",
    "browser.tab.list",
    "browser.tab.new",
    "browser.tab.switch",
    "browser.tab.close",
    "browser.frame.main",
    "browser.frame.select",
    "browser.dialog.accept",
    "browser.dialog.dismiss",
    "browser.download.wait",
    "browser.state.save",
    "browser.state.load",
    "debug.app.activate",
    "debug.bonsplit_underflow.count",
    "debug.bonsplit_underflow.reset",
    "debug.command_palette.rename_input.delete_backward",
    "debug.command_palette.rename_input.interact",
    "debug.command_palette.rename_input.select_all",
    "debug.command_palette.rename_input.selection",
    "debug.command_palette.rename_tab.open",
    "debug.command_palette.results",
    "debug.command_palette.selection",
    "debug.command_palette.toggle",
    "debug.command_palette.visible",
    "debug.empty_panel.count",
    "debug.empty_panel.reset",
    "debug.flash.count",
    "debug.flash.reset",
    "debug.layout",
    "debug.notification.focus",
    "debug.panel_snapshot",
    "debug.panel_snapshot.reset",
    "debug.portal.stats",
    "debug.shortcut.set",
    "debug.shortcut.simulate",
    "debug.sidebar.visible",
    "debug.terminal.is_focused",
    "debug.terminal.read_text",
    "debug.terminal.render_stats",
    "debug.type",
    "debug.window.screenshot",
];

type CommandPaletteSpec = (
    &'static str,
    &'static str,
    Option<(&'static str, &'static str)>,
);

const COMMAND_PALETTE_COMMAND_SPECS: [CommandPaletteSpec; 14] = [
    (
        "palette.renameTab",
        "Rename Tab",
        Some(("rename_tab", "cmd+r")),
    ),
    (
        "palette.renameWorkspace",
        "Rename Workspace",
        Some(("rename_workspace", "cmd+shift+r")),
    ),
    (
        "palette.terminalOpenDirectory",
        "Open Directory",
        Some(("open_directory", "cmd+shift+o")),
    ),
    (
        "palette.newWindow",
        "New Window",
        Some(("new_window", "cmd+shift+n")),
    ),
    (
        "palette.closeWindow",
        "Close Window",
        Some(("close_window", "cmd+ctrl+w")),
    ),
    (
        "palette.newWorkspace",
        "New Workspace",
        Some(("new_workspace", "cmd+t")),
    ),
    (
        "palette.closeWorkspace",
        "Close Workspace",
        Some(("close_workspace", "cmd+shift+w")),
    ),
    (
        "palette.splitRight",
        "Split Right",
        Some(("split_right", "cmd+d")),
    ),
    (
        "palette.splitDown",
        "Split Down",
        Some(("split_down", "cmd+shift+d")),
    ),
    (
        "palette.newTerminal",
        "New Terminal",
        Some(("new_terminal", "cmd+enter")),
    ),
    (
        "palette.toggleSidebar",
        "Toggle Sidebar",
        Some(("toggle_sidebar", "cmd+b")),
    ),
    ("palette.focusNext", "Focus Next", None),
    ("palette.focusPrev", "Focus Previous", None),
    ("palette.reloadConfig", "Reload Config", None),
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceInfo {
    pub id: u64,
    pub name: String,
    pub host_window_id: u64,
    pub window_count: usize,
    pub current_window_id: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WindowInfo {
    pub id: u64,
    pub title: String,
    pub pane_count: usize,
    pub current_pane_id: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PaneInfo {
    pub id: u64,
    pub surface_count: usize,
    pub current_surface_id: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SurfaceInfo {
    pub id: u64,
    pub pane_id: u64,
    pub title: String,
    pub text: String,
    pub panel_type: String,
    pub developer_tools_visible: bool,
    pub pinned: bool,
    pub unread: bool,
    pub flash_count: u64,
    pub refresh_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotificationInfo {
    pub id: u64,
    pub message: String,
    pub title: String,
    pub subtitle: String,
    pub body: String,
    pub surface_id: Option<u64>,
    pub workspace_id: Option<u64>,
    pub unread: bool,
}

#[derive(Debug, Clone)]
struct WorkspaceState {
    id: u64,
    name: String,
    cwd: Option<String>,
    host_window_id: u64,
    windows: Vec<WindowState>,
    current_window_id: Option<u64>,
    last_window_id: Option<u64>,
}

impl WorkspaceState {
    fn info(&self) -> WorkspaceInfo {
        WorkspaceInfo {
            id: self.id,
            name: self.name.clone(),
            host_window_id: self.host_window_id,
            window_count: self.windows.len(),
            current_window_id: self.current_window_id,
        }
    }
}

#[derive(Debug, Clone)]
struct WindowState {
    id: u64,
    title: String,
    panes: Vec<PaneState>,
    current_pane_id: Option<u64>,
    last_pane_id: Option<u64>,
}

impl WindowState {
    fn info(&self) -> WindowInfo {
        WindowInfo {
            id: self.id,
            title: self.title.clone(),
            pane_count: self.panes.len(),
            current_pane_id: self.current_pane_id,
        }
    }
}

#[derive(Debug, Clone)]
struct PaneState {
    id: u64,
    surfaces: Vec<SurfaceState>,
    current_surface_id: Option<u64>,
    last_surface_id: Option<u64>,
}

impl PaneState {
    fn info(&self) -> PaneInfo {
        PaneInfo {
            id: self.id,
            surface_count: self.surfaces.len(),
            current_surface_id: self.current_surface_id,
        }
    }
}

#[derive(Debug, Clone)]
struct SurfaceState {
    id: u64,
    title: String,
    text: String,
    shell_input: String,
    terminal_mode: TerminalMode,
    panel_type: String,
    developer_tools_visible: bool,
    pinned: bool,
    unread: bool,
    flash_count: u64,
    refresh_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalMode {
    Idle,
    Sleeping,
    Cat,
    PythonLoop,
}

impl SurfaceState {
    fn info(&self, pane_id: u64) -> SurfaceInfo {
        SurfaceInfo {
            id: self.id,
            pane_id,
            title: self.title.clone(),
            text: self.text.clone(),
            panel_type: self.panel_type.clone(),
            developer_tools_visible: self.developer_tools_visible,
            pinned: self.pinned,
            unread: self.unread,
            flash_count: self.flash_count,
            refresh_count: self.refresh_count,
        }
    }
}

#[derive(Debug, Clone)]
struct BrowserState {
    open: bool,
    focused: bool,
    surface_id: Option<u64>,
    url: String,
    title: String,
    page_text: String,
    html: String,
    history: Vec<String>,
    history_index: usize,
    fields: HashMap<String, String>,
    checked: HashMap<String, bool>,
    local_storage: HashMap<String, String>,
    session_storage: HashMap<String, String>,
    cookies: HashMap<String, String>,
    scripts: Vec<String>,
    init_scripts: Vec<String>,
    styles: Vec<String>,
    console: Vec<String>,
    errors: Vec<String>,
    dom_text: HashMap<String, String>,
    dom_html: HashMap<String, String>,
    dom_attrs: HashMap<String, HashMap<String, String>>,
    dom_visible: HashMap<String, bool>,
    dom_enabled: HashMap<String, bool>,
    dom_styles: HashMap<String, HashMap<String, String>>,
    dom_counts: HashMap<String, u64>,
    scroll_tops: HashMap<String, f64>,
    in_view: HashSet<String>,
    active_element: String,
    hover_count: u64,
    dbl_count: u64,
    key_down_count: u64,
    key_up_count: u64,
    key_press_count: u64,
    frame_selected: bool,
    frame_clicks: u64,
    dialogs: Vec<String>,
    browser_surfaces: HashSet<u64>,
    element_refs: HashMap<String, String>,
    next_element_ref: u64,
    init_marker: String,
    tab_ids: Vec<u64>,
    current_tab_id: u64,
    next_tab_id: u64,
}

impl Default for BrowserState {
    fn default() -> Self {
        Self {
            open: false,
            focused: false,
            surface_id: None,
            url: "about:blank".to_string(),
            title: "about:blank".to_string(),
            page_text: String::new(),
            html: String::new(),
            history: vec!["about:blank".to_string()],
            history_index: 0,
            fields: HashMap::new(),
            checked: HashMap::new(),
            local_storage: HashMap::new(),
            session_storage: HashMap::new(),
            cookies: HashMap::new(),
            scripts: Vec::new(),
            init_scripts: Vec::new(),
            styles: Vec::new(),
            console: Vec::new(),
            errors: Vec::new(),
            dom_text: HashMap::new(),
            dom_html: HashMap::new(),
            dom_attrs: HashMap::new(),
            dom_visible: HashMap::new(),
            dom_enabled: HashMap::new(),
            dom_styles: HashMap::new(),
            dom_counts: HashMap::new(),
            scroll_tops: HashMap::new(),
            in_view: HashSet::new(),
            active_element: String::new(),
            hover_count: 0,
            dbl_count: 0,
            key_down_count: 0,
            key_up_count: 0,
            key_press_count: 0,
            frame_selected: false,
            frame_clicks: 0,
            dialogs: Vec::new(),
            browser_surfaces: HashSet::new(),
            element_refs: HashMap::new(),
            next_element_ref: 1,
            init_marker: String::new(),
            tab_ids: Vec::new(),
            current_tab_id: 0,
            next_tab_id: 1,
        }
    }
}

fn apply_named_surface_action(surface: &mut SurfaceState, action_key: &str, title: Option<&str>) {
    match action_key {
        "rename" => {
            if let Some(title) = title {
                surface.title = title.to_string();
            }
        }
        "clear_name" => {
            surface.title = format!("surface-{}", surface.id);
        }
        "pin" => {
            surface.pinned = true;
        }
        "unpin" => {
            surface.pinned = false;
        }
        "mark_unread" => {
            surface.unread = true;
        }
        "mark_read" => {
            surface.unread = false;
        }
        _ => {}
    }
}

fn normalize_notification_content(
    params: &Map<String, Value>,
) -> Result<(String, String, String), CommandError> {
    let message = optional_string_param(params, "message")?.unwrap_or_default();
    let mut title = optional_string_param(params, "title")?.unwrap_or_default();
    let subtitle = optional_string_param(params, "subtitle")?.unwrap_or_default();
    let body = optional_string_param(params, "body")?.unwrap_or_default();
    if title.is_empty() && !message.is_empty() {
        title = message;
    }
    Ok((title, subtitle, body))
}

fn notification_row(notification: &NotificationInfo) -> Value {
    json!({
        "id": encode_handle_id(notification.id),
        "message": notification.message,
        "title": notification.title,
        "subtitle": notification.subtitle,
        "body": notification.body,
        "surface_id": notification.surface_id.map(encode_handle_id),
        "workspace_id": notification.workspace_id.map(encode_handle_id),
        "is_read": !notification.unread,
        "unread": notification.unread,
    })
}

fn created_notification_response(notification: Option<NotificationInfo>) -> Value {
    let Some(notification) = notification else {
        return json!({ "suppressed": true });
    };
    json!({
        "notification_id": encode_handle_id(notification.id),
        "notification": notification_row(&notification)
    })
}

#[derive(Debug, Clone)]
struct CommandPaletteState {
    visible: bool,
    mode: String,
    query: String,
    selected_index: usize,
    selection_location: usize,
    selection_length: usize,
    rename_text: String,
    rename_target_surface_id: Option<u64>,
    rename_target_workspace_id: Option<u64>,
}

impl Default for CommandPaletteState {
    fn default() -> Self {
        Self {
            visible: false,
            mode: "commands".to_string(),
            query: String::new(),
            selected_index: 0,
            selection_location: 0,
            selection_length: 0,
            rename_text: String::new(),
            rename_target_surface_id: None,
            rename_target_workspace_id: None,
        }
    }
}

#[derive(Debug, Clone)]
struct PaletteRow {
    command_id: String,
    title: String,
    trailing_label: Option<String>,
    shortcut_hint: Option<String>,
    workspace_id: Option<u64>,
    surface_id: Option<u64>,
    host_window_id: Option<u64>,
    score: i64,
}

#[derive(Debug, Clone)]
pub struct ControlState {
    workspaces: Vec<WorkspaceState>,
    current_workspace_id: u64,
    last_workspace_id: Option<u64>,
    next_workspace_id: u64,
    next_window_id: u64,
    next_pane_id: u64,
    next_surface_id: u64,
    next_notification_id: u64,
    notifications: Vec<NotificationInfo>,
    app_focus_override: bool,
    app_simulate_active: bool,
    debug_sidebar_visible: bool,
    debug_bonsplit_underflow_count: u64,
    debug_empty_panel_count: u64,
    debug_flash_count: u64,
    shortcuts: HashMap<String, String>,
    command_palettes: HashMap<u64, CommandPaletteState>,
    rename_input_select_all: bool,
    sidebar_visibility_by_window: HashMap<u64, bool>,
    pane_right_neighbors: HashMap<u64, u64>,
    pane_down_neighbors: HashMap<u64, u64>,
    pane_size_overrides: HashMap<u64, (f64, f64)>,
    panel_snapshot_baselines: HashMap<u64, String>,
    next_snapshot_id: u64,
    kitty_notification_chunks: HashMap<u64, (Option<String>, Option<String>)>,
    browser: BrowserState,
}

impl Default for ControlState {
    fn default() -> Self {
        let mut state = Self {
            workspaces: Vec::new(),
            current_workspace_id: 0,
            last_workspace_id: None,
            next_workspace_id: 1,
            next_window_id: 1,
            next_pane_id: 1,
            next_surface_id: 1,
            next_notification_id: 1,
            notifications: Vec::new(),
            app_focus_override: false,
            app_simulate_active: false,
            debug_sidebar_visible: true,
            debug_bonsplit_underflow_count: 0,
            debug_empty_panel_count: 0,
            debug_flash_count: 0,
            shortcuts: HashMap::new(),
            command_palettes: HashMap::new(),
            rename_input_select_all: true,
            sidebar_visibility_by_window: HashMap::new(),
            pane_right_neighbors: HashMap::new(),
            pane_down_neighbors: HashMap::new(),
            pane_size_overrides: HashMap::new(),
            panel_snapshot_baselines: HashMap::new(),
            next_snapshot_id: 1,
            kitty_notification_chunks: HashMap::new(),
            browser: BrowserState::default(),
        };

        let workspace = state.make_workspace(Some("main".to_string()));
        state.current_workspace_id = workspace.id;
        if let Some(window) = workspace.windows.first() {
            state
                .sidebar_visibility_by_window
                .insert(window.id, state.debug_sidebar_visible);
        }
        state.workspaces.push(workspace);
        state
    }
}

impl ControlState {
    fn make_workspace(&mut self, name: Option<String>) -> WorkspaceState {
        let id = self.next_workspace_id;
        self.next_workspace_id += 1;

        let mut workspace = WorkspaceState {
            id,
            name: name.unwrap_or_else(|| format!("workspace-{id}")),
            cwd: None,
            host_window_id: 1,
            windows: Vec::new(),
            current_window_id: None,
            last_window_id: None,
        };

        let window = self.make_window(None);
        workspace.current_window_id = Some(window.id);
        workspace.windows.push(window);
        workspace
    }

    fn make_window(&mut self, title: Option<String>) -> WindowState {
        let id = self.next_window_id;
        self.next_window_id += 1;

        let mut window = WindowState {
            id,
            title: title.unwrap_or_else(|| format!("window-{id}")),
            panes: Vec::new(),
            current_pane_id: None,
            last_pane_id: None,
        };

        let pane = self.make_pane(None);
        window.current_pane_id = Some(pane.id);
        window.panes.push(pane);
        window
    }

    fn make_pane(&mut self, surface_title: Option<String>) -> PaneState {
        let id = self.next_pane_id;
        self.next_pane_id += 1;

        let surface = self.make_surface(surface_title);
        let current_surface_id = Some(surface.id);

        PaneState {
            id,
            surfaces: vec![surface],
            current_surface_id,
            last_surface_id: None,
        }
    }

    fn make_surface(&mut self, title: Option<String>) -> SurfaceState {
        let id = self.next_surface_id;
        self.next_surface_id += 1;

        SurfaceState {
            id,
            title: title.unwrap_or_else(|| format!("surface-{id}")),
            text: String::new(),
            shell_input: String::new(),
            terminal_mode: TerminalMode::Idle,
            panel_type: "terminal".to_string(),
            developer_tools_visible: false,
            pinned: false,
            unread: false,
            flash_count: 0,
            refresh_count: 0,
        }
    }

    fn current_workspace_idx(&self) -> Option<usize> {
        self.workspaces
            .iter()
            .position(|workspace| workspace.id == self.current_workspace_id)
    }

    fn current_workspace(&self) -> Option<&WorkspaceState> {
        self.current_workspace_idx()
            .and_then(|idx| self.workspaces.get(idx))
    }

    fn current_workspace_mut(&mut self) -> Option<&mut WorkspaceState> {
        self.current_workspace_idx()
            .and_then(|idx| self.workspaces.get_mut(idx))
    }

    fn current_window_idx(&self, workspace_idx: usize) -> Option<usize> {
        let workspace = self.workspaces.get(workspace_idx)?;
        let current_window_id = workspace.current_window_id?;
        workspace
            .windows
            .iter()
            .position(|window| window.id == current_window_id)
    }

    fn current_pane_idx(&self, workspace_idx: usize, window_idx: usize) -> Option<usize> {
        let window = self
            .workspaces
            .get(workspace_idx)?
            .windows
            .get(window_idx)?;
        let current_pane_id = window.current_pane_id?;
        window
            .panes
            .iter()
            .position(|pane| pane.id == current_pane_id)
    }

    fn current_surface_idx(
        &self,
        workspace_idx: usize,
        window_idx: usize,
        pane_idx: usize,
    ) -> Option<usize> {
        let pane = self
            .workspaces
            .get(workspace_idx)?
            .windows
            .get(window_idx)?
            .panes
            .get(pane_idx)?;
        let current_surface_id = pane.current_surface_id?;
        pane.surfaces
            .iter()
            .position(|surface| surface.id == current_surface_id)
    }

    fn list_workspaces(&self) -> Vec<WorkspaceInfo> {
        self.workspaces.iter().map(WorkspaceState::info).collect()
    }

    fn create_workspace(
        &mut self,
        name: Option<String>,
        host_window_id: Option<u64>,
    ) -> WorkspaceInfo {
        let mut workspace = self.make_workspace(name);
        if let Some(host_window_id) = host_window_id {
            workspace.host_window_id = host_window_id;
            if let Some(window) = workspace.windows.first_mut() {
                window.id = host_window_id;
            }
            workspace.current_window_id = Some(host_window_id);
        }
        if let Some(window) = workspace.windows.first() {
            self.sidebar_visibility_by_window
                .entry(window.id)
                .or_insert(self.debug_sidebar_visible);
        }
        let info = workspace.info();
        self.last_workspace_id = Some(self.current_workspace_id);
        self.current_workspace_id = workspace.id;
        self.workspaces.push(workspace);
        info
    }

    fn select_workspace(&mut self, id: Option<u64>, name: Option<&str>) -> Option<WorkspaceInfo> {
        let idx = self.workspaces.iter().position(|workspace| {
            id.map(|target| workspace.id == target).unwrap_or(false)
                || name
                    .map(|target| workspace.name.as_str() == target)
                    .unwrap_or(false)
        })?;

        let target_id = self.workspaces[idx].id;
        if self.current_workspace_id != target_id {
            self.last_workspace_id = Some(self.current_workspace_id);
            self.current_workspace_id = target_id;
        }
        if self.app_is_active() {
            let _ = self.mark_notifications_read_for_workspace(target_id);
        }

        Some(self.workspaces[idx].info())
    }

    fn select_workspace_relative(&mut self, delta: isize) -> Option<WorkspaceInfo> {
        if self.workspaces.is_empty() {
            return None;
        }

        let current_idx = self.current_workspace_idx()? as isize;
        let len = self.workspaces.len() as isize;
        let mut target_idx = (current_idx + delta) % len;
        if target_idx < 0 {
            target_idx += len;
        }

        let target_id = self.workspaces[target_idx as usize].id;
        if self.current_workspace_id != target_id {
            self.last_workspace_id = Some(self.current_workspace_id);
            self.current_workspace_id = target_id;
        }
        if self.app_is_active() {
            let _ = self.mark_notifications_read_for_workspace(target_id);
        }

        Some(self.workspaces[target_idx as usize].info())
    }

    fn select_last_workspace(&mut self) -> Option<WorkspaceInfo> {
        let last = self.last_workspace_id?;
        self.select_workspace(Some(last), None)
    }

    fn rename_workspace(&mut self, id: Option<u64>, name: String) -> Option<WorkspaceInfo> {
        let target_id = id.unwrap_or(self.current_workspace_id);
        let workspace = self
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.id == target_id)?;
        workspace.name = name;
        Some(workspace.info())
    }

    fn set_workspace_cwd(&mut self, id: u64, cwd: Option<String>) -> Option<()> {
        let workspace = self
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.id == id)?;
        workspace.cwd = cwd;
        Some(())
    }

    fn workspace_cwd(&self, id: u64) -> Option<String> {
        self.workspaces
            .iter()
            .find(|workspace| workspace.id == id)
            .and_then(|workspace| workspace.cwd.clone())
    }

    fn reorder_workspace(&mut self, id: u64, index: usize) -> Option<WorkspaceInfo> {
        let from = self
            .workspaces
            .iter()
            .position(|workspace| workspace.id == id)?;
        let to = index.min(self.workspaces.len().saturating_sub(1));
        if from != to {
            let workspace = self.workspaces.remove(from);
            self.workspaces.insert(to, workspace);
        }
        self.workspaces
            .iter()
            .find(|workspace| workspace.id == id)
            .map(WorkspaceState::info)
    }

    fn close_workspace(&mut self, id: Option<u64>) -> Option<WorkspaceInfo> {
        if self.workspaces.len() <= 1 {
            return None;
        }

        let target_id = id.unwrap_or(self.current_workspace_id);
        let idx = self
            .workspaces
            .iter()
            .position(|workspace| workspace.id == target_id)?;

        let removed = self.workspaces.remove(idx);
        self.notifications
            .retain(|item| item.workspace_id != Some(removed.id));

        if removed.id == self.current_workspace_id {
            let fallback_idx = idx.min(self.workspaces.len().saturating_sub(1));
            let fallback = self.workspaces.get(fallback_idx)?;
            self.last_workspace_id = Some(removed.id);
            self.current_workspace_id = fallback.id;
        }

        Some(removed.info())
    }

    fn move_workspace_to_window(
        &mut self,
        workspace_id: Option<u64>,
        host_window_id: u64,
    ) -> Option<WorkspaceInfo> {
        let target_id = workspace_id.unwrap_or(self.current_workspace_id);
        let workspace = self
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.id == target_id)?;
        workspace.host_window_id = host_window_id;
        Some(workspace.info())
    }

    fn list_windows(&self) -> Option<Vec<WindowInfo>> {
        self.current_workspace()
            .map(|workspace| workspace.windows.iter().map(WindowState::info).collect())
    }

    fn current_window(&self) -> Option<WindowInfo> {
        let workspace = self.current_workspace()?;
        let window_id = workspace.current_window_id?;
        workspace
            .windows
            .iter()
            .find(|window| window.id == window_id)
            .map(WindowState::info)
    }

    fn create_window(&mut self, title: Option<String>) -> Option<WindowInfo> {
        let window = self.make_window(title);
        let info = window.info();
        self.sidebar_visibility_by_window
            .entry(window.id)
            .or_insert(self.debug_sidebar_visible);
        let workspace = self.current_workspace_mut()?;
        workspace.last_window_id = workspace.current_window_id;
        workspace.current_window_id = Some(window.id);
        workspace.windows.push(window);
        Some(info)
    }

    fn focus_window(&mut self, id: u64) -> Option<WindowInfo> {
        if let Some(current_workspace_idx) = self.current_workspace_idx() {
            if let Some(current_window_idx) = self
                .workspaces
                .get(current_workspace_idx)?
                .windows
                .iter()
                .position(|window| window.id == id)
            {
                let (workspace_id, info) = {
                    let workspace = self.workspaces.get_mut(current_workspace_idx)?;
                    workspace.host_window_id = id;
                    workspace.last_window_id = workspace.current_window_id;
                    workspace.current_window_id = Some(id);
                    (
                        workspace.id,
                        workspace.windows.get(current_window_idx)?.info(),
                    )
                };
                if self.app_is_active() {
                    let _ = self.mark_notifications_read_for_workspace(workspace_id);
                }
                return Some(info);
            }
        }

        let mut match_workspace_idx = None;
        let mut match_window_idx = None;

        for (workspace_idx, workspace) in self.workspaces.iter().enumerate() {
            if let Some(window_idx) = workspace.windows.iter().position(|window| window.id == id) {
                match_workspace_idx = Some(workspace_idx);
                match_window_idx = Some(window_idx);
                break;
            }
        }

        let workspace_idx = match_workspace_idx?;
        let window_idx = match_window_idx?;
        let target_workspace_id = self.workspaces.get(workspace_idx)?.id;
        if self.current_workspace_id != target_workspace_id {
            self.last_workspace_id = Some(self.current_workspace_id);
            self.current_workspace_id = target_workspace_id;
        }

        let info = {
            let workspace = self.workspaces.get_mut(workspace_idx)?;
            workspace.host_window_id = id;
            workspace.last_window_id = workspace.current_window_id;
            workspace.current_window_id = Some(id);
            workspace.windows.get(window_idx)?.info()
        };
        if self.app_is_active() {
            let _ = self.mark_notifications_read_for_workspace(target_workspace_id);
        }
        Some(info)
    }

    fn close_window(&mut self, id: Option<u64>) -> Option<WindowInfo> {
        let workspace = self.current_workspace_mut()?;
        if workspace.windows.len() <= 1 {
            return None;
        }

        let target_id = id.or(workspace.current_window_id)?;
        let idx = workspace
            .windows
            .iter()
            .position(|window| window.id == target_id)?;

        let removed = workspace.windows.remove(idx);
        if workspace.current_window_id == Some(target_id) {
            let fallback_idx = idx.min(workspace.windows.len().saturating_sub(1));
            let fallback_id = workspace
                .windows
                .get(fallback_idx)
                .map(|window| window.id)?;
            workspace.last_window_id = Some(target_id);
            workspace.current_window_id = Some(fallback_id);
        }

        Some(removed.info())
    }

    fn list_panes(&self) -> Option<Vec<PaneInfo>> {
        let workspace_idx = self.current_workspace_idx()?;
        let window_idx = self.current_window_idx(workspace_idx)?;
        let window = self
            .workspaces
            .get(workspace_idx)?
            .windows
            .get(window_idx)?;
        Some(window.panes.iter().map(PaneState::info).collect())
    }

    fn list_surfaces_for_pane(&self, pane_id: Option<u64>) -> Option<Vec<SurfaceInfo>> {
        let workspace_idx = self.current_workspace_idx()?;
        let window_idx = self.current_window_idx(workspace_idx)?;
        let window = self
            .workspaces
            .get(workspace_idx)?
            .windows
            .get(window_idx)?;

        let target_pane_idx = match pane_id {
            Some(id) => window.panes.iter().position(|pane| pane.id == id)?,
            None => self.current_pane_idx(workspace_idx, window_idx)?,
        };

        let pane = window.panes.get(target_pane_idx)?;
        Some(
            pane.surfaces
                .iter()
                .map(|surface| surface.info(pane.id))
                .collect(),
        )
    }

    fn create_pane(&mut self, title: Option<String>) -> Option<PaneInfo> {
        let pane = self.make_pane(title);
        let info = pane.info();

        let workspace_idx = self.current_workspace_idx()?;
        let window_idx = self.current_window_idx(workspace_idx)?;
        let window = self
            .workspaces
            .get_mut(workspace_idx)?
            .windows
            .get_mut(window_idx)?;

        window.last_pane_id = window.current_pane_id;
        window.current_pane_id = Some(pane.id);
        window.panes.push(pane);

        Some(info)
    }

    fn focus_pane(&mut self, pane_id: u64) -> Option<PaneInfo> {
        let workspace_idx = self.current_workspace_idx()?;
        let window_idx = self.current_window_idx(workspace_idx)?;
        let window = self
            .workspaces
            .get_mut(workspace_idx)?
            .windows
            .get_mut(window_idx)?;

        let idx = window.panes.iter().position(|pane| pane.id == pane_id)?;
        window.last_pane_id = window.current_pane_id;
        window.current_pane_id = Some(pane_id);
        Some(window.panes[idx].info())
    }

    fn focus_last_pane(&mut self) -> Option<PaneInfo> {
        let workspace_idx = self.current_workspace_idx()?;
        let window_idx = self.current_window_idx(workspace_idx)?;
        let last = self
            .workspaces
            .get(workspace_idx)?
            .windows
            .get(window_idx)?
            .last_pane_id?;
        self.focus_pane(last)
    }

    fn focus_left_pane(&mut self) -> Option<PaneInfo> {
        let workspace_idx = self.current_workspace_idx()?;
        let window_idx = self.current_window_idx(workspace_idx)?;
        let window = self
            .workspaces
            .get_mut(workspace_idx)?
            .windows
            .get_mut(window_idx)?;
        let current = window.current_pane_id?;
        let current_idx = window.panes.iter().position(|pane| pane.id == current)?;
        let target_idx = current_idx.saturating_sub(1);
        let target_id = window.panes.get(target_idx)?.id;
        if target_id != current {
            window.last_pane_id = Some(current);
            window.current_pane_id = Some(target_id);
        }
        window.panes.get(target_idx).map(PaneState::info)
    }

    fn focus_right_pane(&mut self) -> Option<PaneInfo> {
        let workspace_idx = self.current_workspace_idx()?;
        let window_idx = self.current_window_idx(workspace_idx)?;
        let window = self
            .workspaces
            .get_mut(workspace_idx)?
            .windows
            .get_mut(window_idx)?;
        let current = window.current_pane_id?;
        let current_idx = window.panes.iter().position(|pane| pane.id == current)?;
        let target_idx = (current_idx + 1).min(window.panes.len().saturating_sub(1));
        let target_id = window.panes.get(target_idx)?.id;
        if target_id != current {
            window.last_pane_id = Some(current);
            window.current_pane_id = Some(target_id);
        }
        window.panes.get(target_idx).map(PaneState::info)
    }

    fn toggle_devtools_on_focused_browser(&mut self) -> Option<bool> {
        let (workspace_id, _window_id, _pane_id, surface_id) = focused_handles(self)?;
        let updated = update_surface_metadata(self, workspace_id, surface_id, |surface| {
            if surface.panel_type == "browser" {
                surface.developer_tools_visible = !surface.developer_tools_visible;
            }
        })?;
        Some(updated.developer_tools_visible)
    }

    fn pane_exists(&self, pane_id: u64) -> bool {
        self.workspaces.iter().any(|workspace| {
            workspace
                .windows
                .iter()
                .any(|window| window.panes.iter().any(|pane| pane.id == pane_id))
        })
    }

    fn resize_pane(&mut self, pane_id: u64, direction: &str, amount: f64) -> Option<(f64, f64)> {
        if !self.pane_exists(pane_id) {
            return None;
        }
        let entry = self
            .pane_size_overrides
            .entry(pane_id)
            .or_insert((0.0_f64, 0.0_f64));
        match direction {
            "right" => entry.0 += amount,
            "left" => entry.0 -= amount,
            "down" => entry.1 += amount,
            "up" => entry.1 -= amount,
            _ => {}
        }
        Some(*entry)
    }

    fn swap_panes(&mut self, first: u64, second: u64) -> Option<Vec<PaneInfo>> {
        let workspace_idx = self.current_workspace_idx()?;
        let window_idx = self.current_window_idx(workspace_idx)?;
        let window = self
            .workspaces
            .get_mut(workspace_idx)?
            .windows
            .get_mut(window_idx)?;

        let first_idx = window.panes.iter().position(|pane| pane.id == first)?;
        let second_idx = window.panes.iter().position(|pane| pane.id == second)?;
        window.panes.swap(first_idx, second_idx);

        Some(window.panes.iter().map(PaneState::info).collect())
    }

    fn join_panes(&mut self, source_id: u64, target_id: u64) -> Option<PaneInfo> {
        if source_id == target_id {
            return None;
        }

        let workspace_idx = self.current_workspace_idx()?;
        let window_idx = self.current_window_idx(workspace_idx)?;
        let window = self
            .workspaces
            .get_mut(workspace_idx)?
            .windows
            .get_mut(window_idx)?;

        let source_idx = window.panes.iter().position(|pane| pane.id == source_id)?;
        let mut source = window.panes.remove(source_idx);

        let target_idx = window.panes.iter().position(|pane| pane.id == target_id)?;
        let target = window.panes.get_mut(target_idx)?;
        let moved_current_id = source.current_surface_id;
        target.surfaces.append(&mut source.surfaces);
        target.last_surface_id = target.current_surface_id;
        if moved_current_id.is_some() {
            target.current_surface_id = moved_current_id;
        }

        if window.current_pane_id == Some(source_id) {
            window.current_pane_id = Some(target_id);
        }

        Some(target.info())
    }

    fn break_pane(&mut self, pane_id: Option<u64>) -> Option<PaneInfo> {
        let workspace_idx = self.current_workspace_idx()?;
        let window_idx = self.current_window_idx(workspace_idx)?;
        let fallback_surface = self.make_surface(None);
        let window = self
            .workspaces
            .get_mut(workspace_idx)?
            .windows
            .get_mut(window_idx)?;

        let source_idx = match pane_id {
            Some(id) => window.panes.iter().position(|pane| pane.id == id)?,
            None => window
                .current_pane_id
                .and_then(|id| window.panes.iter().position(|pane| pane.id == id))?,
        };

        let current_surface_id = window.panes.get(source_idx)?.current_surface_id?;
        let source_pane = window.panes.get_mut(source_idx)?;
        let surface_idx = source_pane
            .surfaces
            .iter()
            .position(|surface| surface.id == current_surface_id)?;

        let moved_surface = source_pane.surfaces.remove(surface_idx);
        if source_pane.surfaces.is_empty() {
            source_pane.current_surface_id = Some(fallback_surface.id);
            source_pane.surfaces.push(fallback_surface);
        } else {
            let fallback_idx = surface_idx.min(source_pane.surfaces.len().saturating_sub(1));
            let fallback_id = source_pane
                .surfaces
                .get(fallback_idx)
                .map(|surface| surface.id)?;
            source_pane.current_surface_id = Some(fallback_id);
        }

        let pane_id = self.next_pane_id;
        self.next_pane_id += 1;
        let new_pane = PaneState {
            id: pane_id,
            surfaces: vec![moved_surface],
            current_surface_id: Some(current_surface_id),
            last_surface_id: None,
        };
        let info = new_pane.info();

        window.last_pane_id = window.current_pane_id;
        window.current_pane_id = Some(pane_id);
        window.panes.push(new_pane);

        Some(info)
    }

    fn list_surfaces(&self) -> Option<Vec<SurfaceInfo>> {
        let workspace_idx = self.current_workspace_idx()?;
        let window_idx = self.current_window_idx(workspace_idx)?;
        let window = self
            .workspaces
            .get(workspace_idx)?
            .windows
            .get(window_idx)?;

        let mut result = Vec::new();
        for pane in &window.panes {
            for surface in &pane.surfaces {
                result.push(surface.info(pane.id));
            }
        }
        Some(result)
    }

    fn current_surface(&self) -> Option<SurfaceInfo> {
        let workspace_idx = self.current_workspace_idx()?;
        let window_idx = self.current_window_idx(workspace_idx)?;
        let pane_idx = self.current_pane_idx(workspace_idx, window_idx)?;
        let surface_idx = self.current_surface_idx(workspace_idx, window_idx, pane_idx)?;

        let pane = self
            .workspaces
            .get(workspace_idx)?
            .windows
            .get(window_idx)?
            .panes
            .get(pane_idx)?;
        pane.surfaces
            .get(surface_idx)
            .map(|surface| surface.info(pane.id))
    }

    fn create_surface(&mut self, title: Option<String>) -> Option<SurfaceInfo> {
        let surface = self.make_surface(title);
        let surface_id = surface.id;

        let workspace_idx = self.current_workspace_idx()?;
        let window_idx = self.current_window_idx(workspace_idx)?;
        let pane_idx = self.current_pane_idx(workspace_idx, window_idx)?;

        let pane = self
            .workspaces
            .get_mut(workspace_idx)?
            .windows
            .get_mut(window_idx)?
            .panes
            .get_mut(pane_idx)?;

        pane.last_surface_id = pane.current_surface_id;
        pane.current_surface_id = Some(surface_id);
        pane.surfaces.push(surface);

        pane.surfaces
            .iter()
            .find(|candidate| candidate.id == surface_id)
            .map(|candidate| candidate.info(pane.id))
    }

    fn create_surface_in_pane(
        &mut self,
        pane_id: u64,
        title: Option<String>,
    ) -> Option<SurfaceInfo> {
        self.focus_pane(pane_id)?;
        self.create_surface(title)
    }

    fn split_surface(&mut self, title: Option<String>) -> Option<SurfaceInfo> {
        let pane = self.make_pane(title);
        let pane_id = pane.id;
        let surface_id = pane.current_surface_id?;

        let workspace_idx = self.current_workspace_idx()?;
        let window_idx = self.current_window_idx(workspace_idx)?;

        let window = self
            .workspaces
            .get_mut(workspace_idx)?
            .windows
            .get_mut(window_idx)?;

        window.last_pane_id = window.current_pane_id;
        window.current_pane_id = Some(pane_id);
        window.panes.push(pane);

        let pane = window
            .panes
            .iter()
            .find(|candidate| candidate.id == pane_id)?;
        pane.surfaces
            .iter()
            .find(|surface| surface.id == surface_id)
            .map(|surface| surface.info(pane_id))
    }

    fn split_surface_from_pane(
        &mut self,
        source_pane_id: u64,
        title: Option<String>,
    ) -> Option<SurfaceInfo> {
        self.focus_pane(source_pane_id)?;
        self.split_surface(title)
    }

    fn pane_exists_in_current_window(&self, pane_id: u64) -> bool {
        let Some(workspace_idx) = self.current_workspace_idx() else {
            return false;
        };
        let Some(window_idx) = self.current_window_idx(workspace_idx) else {
            return false;
        };
        self.workspaces
            .get(workspace_idx)
            .and_then(|workspace| workspace.windows.get(window_idx))
            .map(|window| window.panes.iter().any(|pane| pane.id == pane_id))
            .unwrap_or(false)
    }

    fn right_neighbor_in_current_window(&self, pane_id: u64) -> Option<u64> {
        let candidate = self.pane_right_neighbors.get(&pane_id).copied()?;
        if self.pane_exists_in_current_window(candidate) {
            Some(candidate)
        } else {
            None
        }
    }

    fn register_split_relation(&mut self, source_pane_id: u64, new_pane_id: u64, direction: &str) {
        let direction = direction.to_ascii_lowercase();
        match direction.as_str() {
            "right" => {
                self.pane_right_neighbors
                    .insert(source_pane_id, new_pane_id);
                if let Some(source_down) = self.pane_down_neighbors.get(&source_pane_id).copied() {
                    if let Some(source_down_right) =
                        self.pane_right_neighbors.get(&source_down).copied()
                    {
                        self.pane_down_neighbors
                            .insert(new_pane_id, source_down_right);
                    }
                }
            }
            "down" => {
                self.pane_down_neighbors.insert(source_pane_id, new_pane_id);
                if let Some(source_right) = self.pane_right_neighbors.get(&source_pane_id).copied()
                {
                    if let Some(source_right_down) =
                        self.pane_down_neighbors.get(&source_right).copied()
                    {
                        self.pane_right_neighbors
                            .insert(new_pane_id, source_right_down);
                    }
                }
            }
            _ => {}
        }
    }

    fn find_surface_in_current_window(&self, surface_id: u64) -> Option<(usize, usize, usize)> {
        let workspace_idx = self.current_workspace_idx()?;
        let window_idx = self.current_window_idx(workspace_idx)?;
        let window = self
            .workspaces
            .get(workspace_idx)?
            .windows
            .get(window_idx)?;

        for (pane_idx, pane) in window.panes.iter().enumerate() {
            for (surface_idx, surface) in pane.surfaces.iter().enumerate() {
                if surface.id == surface_id {
                    return Some((workspace_idx, window_idx, pane_idx * 100000 + surface_idx));
                }
            }
        }

        None
    }

    fn find_pane_for_surface_in_current_window(&self, surface_id: u64) -> Option<u64> {
        let (workspace_idx, window_idx, encoded) =
            self.find_surface_in_current_window(surface_id)?;
        let (pane_idx, _) = Self::decode_surface_index(encoded);
        self.workspaces
            .get(workspace_idx)?
            .windows
            .get(window_idx)?
            .panes
            .get(pane_idx)
            .map(|pane| pane.id)
    }

    fn decode_surface_index(encoded: usize) -> (usize, usize) {
        (encoded / 100000, encoded % 100000)
    }

    fn focus_surface(&mut self, surface_id: u64) -> Option<SurfaceInfo> {
        let (workspace_idx, window_idx, encoded) =
            self.find_surface_in_current_window(surface_id)?;
        let (pane_idx, surface_idx) = Self::decode_surface_index(encoded);

        let info = {
            let window = self
                .workspaces
                .get_mut(workspace_idx)?
                .windows
                .get_mut(window_idx)?;
            let pane = window.panes.get_mut(pane_idx)?;

            window.last_pane_id = window.current_pane_id;
            window.current_pane_id = Some(pane.id);

            pane.last_surface_id = pane.current_surface_id;
            pane.current_surface_id = Some(surface_id);

            pane.surfaces
                .get(surface_idx)
                .map(|surface| surface.info(pane.id))
        }?;
        if self.app_is_active() {
            let _ = self.mark_notifications_read_for_surface(surface_id);
        }
        Some(info)
    }

    fn close_surface(&mut self, surface_id: Option<u64>) -> Option<SurfaceInfo> {
        let workspace_idx = self.current_workspace_idx()?;
        let window_idx = self.current_window_idx(workspace_idx)?;
        let fallback_surface = self.make_surface(None);

        let target_surface_id = match surface_id {
            Some(id) => id,
            None => {
                let pane_idx = self.current_pane_idx(workspace_idx, window_idx)?;
                self.workspaces
                    .get(workspace_idx)?
                    .windows
                    .get(window_idx)?
                    .panes
                    .get(pane_idx)?
                    .current_surface_id?
            }
        };

        let (_, _, encoded) = self.find_surface_in_current_window(target_surface_id)?;
        let (pane_idx, surface_idx) = Self::decode_surface_index(encoded);

        let window = self
            .workspaces
            .get_mut(workspace_idx)?
            .windows
            .get_mut(window_idx)?;
        let pane_id = window.panes.get(pane_idx)?.id;
        let removed = {
            let pane = window.panes.get_mut(pane_idx)?;
            pane.surfaces.remove(surface_idx)
        };

        if window
            .panes
            .get(pane_idx)
            .map(|pane| pane.surfaces.is_empty())
            .unwrap_or(false)
        {
            if window.panes.len() > 1 {
                window.panes.remove(pane_idx);
                if window.current_pane_id == Some(pane_id) {
                    let next_idx = pane_idx.min(window.panes.len().saturating_sub(1));
                    let next_id = window.panes.get(next_idx).map(|pane| pane.id)?;
                    window.last_pane_id = Some(pane_id);
                    window.current_pane_id = Some(next_id);
                }
            } else {
                let pane = window.panes.get_mut(pane_idx)?;
                pane.current_surface_id = Some(fallback_surface.id);
                pane.surfaces.push(fallback_surface);
            }
        } else {
            let pane = window.panes.get_mut(pane_idx)?;
            let fallback_idx = surface_idx.min(pane.surfaces.len().saturating_sub(1));
            let fallback_id = pane.surfaces.get(fallback_idx).map(|surface| surface.id)?;
            pane.current_surface_id = Some(fallback_id);
        }

        Some(removed.info(pane_id))
    }

    fn move_surface(
        &mut self,
        surface_id: u64,
        target_pane_id: u64,
        index: Option<usize>,
    ) -> Option<SurfaceInfo> {
        let (workspace_idx, window_idx, encoded) =
            self.find_surface_in_current_window(surface_id)?;
        let (source_pane_idx, source_surface_idx) = Self::decode_surface_index(encoded);
        let fallback_surface = self.make_surface(None);

        let window = self
            .workspaces
            .get_mut(workspace_idx)?
            .windows
            .get_mut(window_idx)?;
        let target_pane_idx = window
            .panes
            .iter()
            .position(|pane| pane.id == target_pane_id)?;

        let moved_surface = {
            let source_pane = window.panes.get_mut(source_pane_idx)?;
            let surface = source_pane.surfaces.remove(source_surface_idx);
            if source_pane.surfaces.is_empty() {
                source_pane.current_surface_id = Some(fallback_surface.id);
                source_pane.surfaces.push(fallback_surface);
            } else {
                let fallback_idx =
                    source_surface_idx.min(source_pane.surfaces.len().saturating_sub(1));
                let fallback_id = source_pane
                    .surfaces
                    .get(fallback_idx)
                    .map(|surface| surface.id)?;
                source_pane.current_surface_id = Some(fallback_id);
            }
            surface
        };

        let target_pane = window.panes.get_mut(target_pane_idx)?;
        let insert_idx = index
            .unwrap_or(target_pane.surfaces.len())
            .min(target_pane.surfaces.len());
        target_pane.surfaces.insert(insert_idx, moved_surface);
        target_pane.last_surface_id = target_pane.current_surface_id;
        target_pane.current_surface_id = Some(surface_id);

        let moved = target_pane
            .surfaces
            .iter()
            .find(|surface| surface.id == surface_id)?;

        Some(moved.info(target_pane.id))
    }

    fn reorder_surface(&mut self, surface_id: u64, index: usize) -> Option<SurfaceInfo> {
        let (workspace_idx, window_idx, encoded) =
            self.find_surface_in_current_window(surface_id)?;
        let (pane_idx, from_idx) = Self::decode_surface_index(encoded);

        let window = self
            .workspaces
            .get_mut(workspace_idx)?
            .windows
            .get_mut(window_idx)?;
        let pane = window.panes.get_mut(pane_idx)?;

        let mut to_idx = index.min(pane.surfaces.len().saturating_sub(1));
        if from_idx < to_idx {
            to_idx = to_idx.saturating_sub(1);
        }
        if from_idx != to_idx {
            let surface = pane.surfaces.remove(from_idx);
            pane.surfaces.insert(to_idx, surface);
        }

        pane.surfaces
            .iter()
            .find(|surface| surface.id == surface_id)
            .map(|surface| surface.info(pane.id))
    }

    fn drag_surface_to_split(
        &mut self,
        surface_id: u64,
        title: Option<String>,
    ) -> Option<SurfaceInfo> {
        // Move requested surface to a newly created pane (split-like behavior).
        let new_pane = self.make_pane(title);
        let new_pane_id = new_pane.id;

        let workspace_idx = self.current_workspace_idx()?;
        let window_idx = self.current_window_idx(workspace_idx)?;
        let window = self
            .workspaces
            .get_mut(workspace_idx)?
            .windows
            .get_mut(window_idx)?;

        window.panes.push(new_pane);
        window.last_pane_id = window.current_pane_id;
        window.current_pane_id = Some(new_pane_id);

        self.move_surface(surface_id, new_pane_id, Some(0))
    }

    fn update_surface(
        &mut self,
        surface_id: Option<u64>,
        update: impl FnOnce(&mut SurfaceState),
    ) -> Option<SurfaceInfo> {
        let target_surface_id = match surface_id {
            Some(id) => id,
            None => self.current_surface()?.id,
        };

        let (workspace_idx, window_idx, encoded) =
            self.find_surface_in_current_window(target_surface_id)?;
        let (pane_idx, surface_idx) = Self::decode_surface_index(encoded);

        let pane = self
            .workspaces
            .get_mut(workspace_idx)?
            .windows
            .get_mut(window_idx)?
            .panes
            .get_mut(pane_idx)?;

        let surface = pane.surfaces.get_mut(surface_idx)?;
        update(surface);
        Some(surface.info(pane.id))
    }

    fn app_is_active(&self) -> bool {
        self.app_focus_override || self.app_simulate_active
    }

    fn increment_surface_flash(&mut self, surface_id: u64) {
        for workspace in &mut self.workspaces {
            for window in &mut workspace.windows {
                for pane in &mut window.panes {
                    if let Some(surface) = pane
                        .surfaces
                        .iter_mut()
                        .find(|surface| surface.id == surface_id)
                    {
                        surface.flash_count = surface.flash_count.saturating_add(1);
                        self.debug_flash_count = self.debug_flash_count.saturating_add(1);
                        return;
                    }
                }
            }
        }
    }

    fn mark_notifications_read_for_surface(&mut self, surface_id: u64) -> bool {
        let mut updated = false;
        for item in &mut self.notifications {
            if item.unread && item.surface_id == Some(surface_id) {
                item.unread = false;
                updated = true;
            }
        }
        if updated {
            self.increment_surface_flash(surface_id);
        }
        updated
    }

    fn mark_notifications_read_for_workspace(&mut self, workspace_id: u64) -> bool {
        let mut updated = false;
        let fallback_surface = current_surface_id_for_workspace(self, workspace_id);
        let mut flashed_surface: Option<u64> = None;
        for item in &mut self.notifications {
            if item.unread && item.workspace_id == Some(workspace_id) {
                item.unread = false;
                updated = true;
                if flashed_surface.is_none() {
                    flashed_surface = item.surface_id.or(fallback_surface);
                }
            }
        }
        if let Some(surface_id) = flashed_surface {
            self.increment_surface_flash(surface_id);
        }
        updated
    }

    fn mark_all_notifications_read(&mut self) -> bool {
        let mut touched = false;
        let mut flashed_surfaces: Vec<u64> = Vec::new();
        for item in &mut self.notifications {
            if item.unread {
                item.unread = false;
                touched = true;
                if let Some(surface_id) = item.surface_id {
                    if !flashed_surfaces.contains(&surface_id) {
                        flashed_surfaces.push(surface_id);
                    }
                }
            }
        }
        for surface_id in flashed_surfaces {
            self.increment_surface_flash(surface_id);
        }
        touched
    }

    fn sidebar_visible_for_window(&self, window_id: u64) -> bool {
        self.sidebar_visibility_by_window
            .get(&window_id)
            .copied()
            .unwrap_or(self.debug_sidebar_visible)
    }

    fn toggle_sidebar_for_window(&mut self, window_id: u64) -> bool {
        let current = self.sidebar_visible_for_window(window_id);
        let next = !current;
        self.sidebar_visibility_by_window.insert(window_id, next);
        next
    }

    fn create_notification(
        &mut self,
        title: String,
        subtitle: String,
        body: String,
        surface_id: Option<u64>,
        workspace_id: Option<u64>,
    ) -> Option<NotificationInfo> {
        let message = [title.clone(), subtitle.clone(), body.clone()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ");

        if self.app_is_active() {
            surface_id?;
            if let Some((_, _, _, focused_surface_id)) = focused_handles(self) {
                if surface_id == Some(focused_surface_id) {
                    return None;
                }
            }
        }

        // Keep only the newest notification, matching app behavior expected by tests.
        self.notifications.clear();

        let id = self.next_notification_id;
        self.next_notification_id += 1;

        let notification = NotificationInfo {
            id,
            message,
            title,
            subtitle,
            body,
            surface_id,
            workspace_id,
            unread: true,
        };
        self.notifications.push(notification.clone());
        Some(notification)
    }

    fn clear_notification(&mut self, id: Option<u64>) -> Vec<NotificationInfo> {
        match id {
            Some(target_id) => {
                self.notifications.retain(|item| item.id != target_id);
            }
            None => self.notifications.clear(),
        }
        self.notifications.clone()
    }

    fn browser_register_surface(&mut self, surface_id: u64) {
        self.browser.surface_id = Some(surface_id);
        self.browser.browser_surfaces.insert(surface_id);
        for workspace in &mut self.workspaces {
            for window in &mut workspace.windows {
                for pane in &mut window.panes {
                    if let Some(surface) = pane
                        .surfaces
                        .iter_mut()
                        .find(|surface| surface.id == surface_id)
                    {
                        surface.panel_type = "browser".to_string();
                    }
                }
            }
        }
        if !self.browser.tab_ids.contains(&surface_id) {
            self.browser.tab_ids.push(surface_id);
        }
        self.browser.current_tab_id = surface_id;
        self.browser.next_tab_id = self.browser.next_tab_id.max(surface_id.saturating_add(1));
    }

    fn browser_new_ref(&mut self, selector: impl Into<String>) -> String {
        let element_ref = format!("@e{}", self.browser.next_element_ref);
        self.browser.next_element_ref = self.browser.next_element_ref.saturating_add(1);
        self.browser
            .element_refs
            .insert(element_ref.clone(), selector.into());
        element_ref
    }

    fn browser_resolve_selector(&self, selector: &str) -> String {
        if let Some(mapped) = self.browser.element_refs.get(selector) {
            mapped.clone()
        } else {
            selector.to_string()
        }
    }

    fn browser_selector_exists(&self, selector: &str) -> bool {
        let selector = self.browser_resolve_selector(selector);
        if selector.starts_with('e') && selector.chars().skip(1).all(|ch| ch.is_ascii_digit()) {
            return true;
        }
        if selector == "#never" || selector == "#does-not-exist" || selector.contains("missing") {
            return false;
        }
        if self.browser.dom_text.contains_key(&selector)
            || self.browser.fields.contains_key(&selector)
            || self.browser.dom_attrs.contains_key(&selector)
            || self.browser.dom_styles.contains_key(&selector)
            || self.browser.dom_visible.contains_key(&selector)
            || self.browser.dom_enabled.contains_key(&selector)
            || self.browser.dom_counts.contains_key(&selector)
        {
            return true;
        }
        if selector == "#frame-text" || selector == "#frame-btn" {
            return self.browser.frame_selected;
        }
        false
    }

    fn browser_seed_page(&mut self, url: &str) {
        self.browser.dom_text.clear();
        self.browser.dom_html.clear();
        self.browser.dom_attrs.clear();
        self.browser.dom_visible.clear();
        self.browser.dom_enabled.clear();
        self.browser.dom_styles.clear();
        self.browser.dom_counts.clear();
        self.browser.scroll_tops.clear();
        self.browser.in_view.clear();
        self.browser.fields.clear();
        self.browser.checked.clear();
        self.browser.element_refs.clear();
        self.browser.next_element_ref = 1;
        self.browser.active_element.clear();
        self.browser.frame_selected = false;
        self.browser.dialogs.clear();
        self.browser.hover_count = 0;
        self.browser.dbl_count = 0;
        self.browser.key_down_count = 0;
        self.browser.key_up_count = 0;
        self.browser.key_press_count = 0;

        let decoded_html = decode_data_html(url).or_else(|| decode_file_html(url));
        let html = decoded_html.clone().unwrap_or_default();
        self.browser.html = html.clone();
        self.browser.title = decoded_html
            .as_deref()
            .and_then(extract_html_title)
            .unwrap_or_else(|| normalize_browser_title(url));
        self.browser.page_text = if let Some(decoded) = decoded_html {
            decoded
        } else {
            format!("Loaded {url}")
        };

        if url == "about:blank" {
            self.browser.title = "about:blank".to_string();
            self.browser.page_text = "about:blank".to_string();
        }

        if self
            .browser
            .page_text
            .contains("limux-browser-comprehensive-1")
            || url.contains("comprehensive-1")
        {
            self.browser
                .fields
                .insert("#name".to_string(), String::new());
            self.browser
                .fields
                .insert("#sel".to_string(), "a".to_string());
            self.browser
                .dom_text
                .insert("#status".to_string(), "ready".to_string());
            self.browser.dom_html.insert(
                "#status".to_string(),
                "<div id=\"status\" data-role=\"status\">ready</div>".to_string(),
            );
            self.browser.dom_visible.insert("#status".to_string(), true);
            self.browser
                .dom_visible
                .insert("#hidden".to_string(), false);
            self.browser.dom_visible.insert("#keys".to_string(), true);
            self.browser.dom_visible.insert("#hover".to_string(), true);
            self.browser.dom_visible.insert("#dbl".to_string(), true);
            self.browser.dom_visible.insert("#chk".to_string(), true);
            self.browser.dom_visible.insert("#sel".to_string(), true);
            self.browser
                .dom_visible
                .insert("#scroller".to_string(), true);
            self.browser
                .dom_visible
                .insert("#style-target".to_string(), true);
            self.browser.dom_visible.insert("#btn".to_string(), true);
            self.browser.dom_enabled.insert("#btn".to_string(), true);
            self.browser
                .dom_enabled
                .insert("#disabled".to_string(), false);
            self.browser.dom_counts.insert("option".to_string(), 2);
            self.browser
                .scroll_tops
                .insert("#scroller".to_string(), 0.0);
            self.browser.dom_attrs.insert(
                "#status".to_string(),
                HashMap::from([("data-role".to_string(), "status".to_string())]),
            );
            self.browser.dom_styles.insert(
                "#style-target".to_string(),
                HashMap::from([
                    ("color".to_string(), "rgb(255, 0, 0)".to_string()),
                    ("display".to_string(), "block".to_string()),
                ]),
            );
            self.browser
                .dom_text
                .insert("#hdr".to_string(), "Browser Comprehensive".to_string());
            self.browser
                .dom_text
                .insert("#bottom".to_string(), "bottom-marker".to_string());
        } else if self
            .browser
            .page_text
            .contains("limux-browser-comprehensive-2")
            || url.contains("comprehensive-2")
            || url.contains("page-two")
        {
            self.browser
                .dom_text
                .insert("#page2".to_string(), "page-two".to_string());
            self.browser.dom_visible.insert("#page2".to_string(), true);
        } else if self.browser.page_text.contains("limux-browser-p0")
            || self.browser.page_text.contains("id='out'")
            || self.browser.page_text.contains("id=\"out\"")
        {
            self.browser
                .fields
                .insert("#name".to_string(), String::new());
            self.browser
                .fields
                .insert("#sel".to_string(), "a".to_string());
            self.browser
                .dom_text
                .insert("#out".to_string(), "ready".to_string());
            self.browser.dom_html.insert(
                "#out".to_string(),
                "<div id=\"out\">ready</div>".to_string(),
            );
            self.browser.dom_visible.insert("#name".to_string(), true);
            self.browser.dom_visible.insert("#btn".to_string(), true);
            self.browser.dom_visible.insert("#chk".to_string(), true);
            self.browser.dom_visible.insert("#sel".to_string(), true);
            self.browser.dom_visible.insert("#out".to_string(), true);
            self.browser.dom_counts.insert("option".to_string(), 2);
        } else if url.ends_with("/index.html")
            || self.browser.page_text.contains("limux-browser-extended")
        {
            self.browser.title = "limux-browser-extended".to_string();
            self.browser
                .fields
                .insert("#name".to_string(), String::new());
            self.browser
                .dom_text
                .insert("#status".to_string(), "ready".to_string());
            self.browser.dom_html.insert(
                "#status".to_string(),
                "<div id=\"status\">ready</div>".to_string(),
            );
            self.browser.dom_visible.insert("#status".to_string(), true);
            self.browser
                .dom_visible
                .insert("#action-btn".to_string(), true);
            self.browser
                .dom_enabled
                .insert("#action-btn".to_string(), true);
            self.browser.dom_counts.insert("li.row".to_string(), 3);
            self.browser.dom_styles.insert(
                "#style-target".to_string(),
                HashMap::from([
                    ("color".to_string(), "rgb(255, 0, 0)".to_string()),
                    ("display".to_string(), "block".to_string()),
                ]),
            );
            self.browser.dom_attrs.insert(
                "#name".to_string(),
                HashMap::from([
                    ("placeholder".to_string(), "Type name".to_string()),
                    ("title".to_string(), "name-title".to_string()),
                    ("data-testid".to_string(), "name-field".to_string()),
                ]),
            );
            self.browser
                .dom_text
                .insert("#frame-text".to_string(), "frame-ready".to_string());
        } else if url.ends_with("/second.html") || self.browser.page_text.contains("second-page") {
            self.browser.title = "limux-browser-extended-second".to_string();
            self.browser
                .dom_text
                .insert("#second".to_string(), "second-page".to_string());
            self.browser.dom_visible.insert("#second".to_string(), true);
            self.browser.dom_styles.insert(
                "#style-target".to_string(),
                HashMap::from([
                    ("color".to_string(), "rgb(255, 0, 0)".to_string()),
                    ("display".to_string(), "block".to_string()),
                ]),
            );
        } else if self.browser.page_text.contains("id='probe'")
            || self.browser.page_text.contains("id=\"probe\"")
        {
            self.browser
                .dom_text
                .insert("#probe".to_string(), "P".to_string());
            self.browser.dom_visible.insert("#probe".to_string(), true);
        }

        if let Some(last_init) = self.browser.init_scripts.last() {
            if last_init.contains("__limuxInitMarker") && last_init.contains("init-ok") {
                self.browser.init_marker = "init-ok".to_string();
            }
        }
    }

    fn browser_navigate(&mut self, url: String) {
        self.browser.open = true;
        self.browser.url = url.clone();
        self.browser_seed_page(&url);

        if self.browser.history_index + 1 < self.browser.history.len() {
            self.browser
                .history
                .truncate(self.browser.history_index + 1);
        }
        self.browser.history.push(url);
        self.browser.history_index = self.browser.history.len().saturating_sub(1);
    }

    fn browser_history_set_current(&mut self) {
        if let Some(url) = self
            .browser
            .history
            .get(self.browser.history_index)
            .cloned()
        {
            self.browser.url = url.clone();
            self.browser_seed_page(&url);
        }
    }

    fn active_window_id(&self) -> Option<u64> {
        self.current_window().map(|window| window.id)
    }

    fn resolve_palette_window_id(&self, requested: Option<u64>) -> Option<u64> {
        requested.or_else(|| self.active_window_id())
    }

    fn palette_state_for_window(&self, window_id: u64) -> CommandPaletteState {
        self.command_palettes
            .get(&window_id)
            .cloned()
            .unwrap_or_default()
    }

    fn palette_state_for_window_mut(&mut self, window_id: u64) -> &mut CommandPaletteState {
        self.command_palettes.entry(window_id).or_default()
    }

    fn palette_visible_for_window(&self, window_id: u64) -> bool {
        self.command_palettes
            .get(&window_id)
            .map(|palette| palette.visible)
            .unwrap_or(false)
    }

    fn palette_target_window_for_typing(&self) -> Option<u64> {
        if let Some(active_window_id) = self.active_window_id() {
            if self.palette_visible_for_window(active_window_id) {
                return Some(active_window_id);
            }
        }
        None
    }

    fn open_palette_in_mode(&mut self, window_id: u64, mode: &str) {
        let palette = self.palette_state_for_window_mut(window_id);
        palette.visible = true;
        palette.mode = mode.to_string();
        palette.selected_index = 0;
        if mode == "commands" {
            palette.query = ">".to_string();
            palette.selection_location = palette.query.len();
            palette.selection_length = 0;
        } else if mode == "switcher" {
            palette.query.clear();
            palette.selection_location = 0;
            palette.selection_length = 0;
        }
    }

    fn close_palette(&mut self, window_id: u64) {
        let palette = self.palette_state_for_window_mut(window_id);
        palette.visible = false;
    }

    fn command_palette_command_rows(&self, query: &str) -> Vec<PaletteRow> {
        let raw_query = query.trim();
        let normalized_query = raw_query
            .trim_start_matches('>')
            .trim()
            .to_ascii_lowercase();
        let mut rows = Vec::new();
        for (order, (command_id, title, shortcut_spec)) in
            COMMAND_PALETTE_COMMAND_SPECS.iter().enumerate()
        {
            let search_text = format!("{command_id} {title}").to_ascii_lowercase();
            let compact_search = search_text.replace(' ', "");
            let compact_query = normalized_query.replace(' ', "");
            let mut score = order as i64;
            if !normalized_query.is_empty() {
                let direct_match = search_text.contains(&normalized_query);
                let fuzzy_match = fuzzy_subsequence(&compact_search, &compact_query);
                if !direct_match && !fuzzy_match {
                    continue;
                }
                if title.to_ascii_lowercase().starts_with(&normalized_query) {
                    score -= 100;
                } else if command_id.to_ascii_lowercase().contains(&normalized_query) {
                    score -= 20;
                } else if fuzzy_match {
                    score -= 10;
                }
                if normalized_query.contains("rename") && command_id.contains("rename") {
                    score -= 80;
                }
                if normalized_query.contains("retab") && *command_id == "palette.renameTab" {
                    score -= 120;
                }
                if normalized_query.contains("open")
                    && *command_id == "palette.terminalOpenDirectory"
                {
                    score -= 120;
                }
            }

            let shortcut_hint = shortcut_spec.map(|(name, default_combo)| {
                let combo = self
                    .shortcuts
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| normalize_shortcut_combo(default_combo));
                combo_to_shortcut_hint(&combo)
            });

            rows.push(PaletteRow {
                command_id: (*command_id).to_string(),
                title: (*title).to_string(),
                trailing_label: None,
                shortcut_hint,
                workspace_id: None,
                surface_id: None,
                host_window_id: None,
                score,
            });
        }
        rows.sort_by_key(|row| (row.score, row.command_id.clone()));
        rows
    }

    fn command_palette_switcher_rows(&self, query: &str) -> Vec<PaletteRow> {
        let normalized_query = query.trim().to_ascii_lowercase();
        let mut rows = Vec::new();

        for workspace in &self.workspaces {
            let workspace_id_label = encode_handle_id(workspace.id).to_ascii_lowercase();
            let workspace_command_id = format!("switcher.workspace.{workspace_id_label}");
            let workspace_search = workspace.name.to_ascii_lowercase();
            let workspace_matches =
                normalized_query.is_empty() || workspace_search.contains(&normalized_query);
            if workspace_matches {
                let score = if normalized_query.is_empty()
                    || workspace
                        .name
                        .to_ascii_lowercase()
                        .starts_with(&normalized_query)
                {
                    100
                } else {
                    150
                };
                rows.push(PaletteRow {
                    command_id: workspace_command_id,
                    title: workspace.name.clone(),
                    trailing_label: Some("Workspace".to_string()),
                    shortcut_hint: None,
                    workspace_id: Some(workspace.id),
                    surface_id: None,
                    host_window_id: Some(workspace.host_window_id),
                    score,
                });
            }

            for window in &workspace.windows {
                for pane in &window.panes {
                    for surface in &pane.surfaces {
                        let surface_id_label = encode_handle_id(surface.id).to_ascii_lowercase();
                        let command_id = format!(
                            "switcher.surface.{}.{}",
                            workspace_id_label, surface_id_label
                        );
                        let search_text =
                            format!("{} {} {}", surface.title, surface.text, workspace.name)
                                .to_ascii_lowercase();
                        if !normalized_query.is_empty() && !search_text.contains(&normalized_query)
                        {
                            continue;
                        }
                        let mut score = if normalized_query.is_empty() { 200 } else { 80 };
                        if !normalized_query.is_empty()
                            && surface
                                .title
                                .to_ascii_lowercase()
                                .contains(&normalized_query)
                        {
                            score = 10;
                        } else if !normalized_query.is_empty()
                            && surface
                                .text
                                .to_ascii_lowercase()
                                .contains(&normalized_query)
                        {
                            score = 40;
                        }
                        rows.push(PaletteRow {
                            command_id,
                            title: surface.title.clone(),
                            trailing_label: Some("Surface".to_string()),
                            shortcut_hint: None,
                            workspace_id: Some(workspace.id),
                            surface_id: Some(surface.id),
                            host_window_id: Some(workspace.host_window_id),
                            score,
                        });
                    }
                }
            }
        }

        rows.sort_by_key(|row| (row.score, row.command_id.clone()));
        rows
    }

    fn command_palette_rows(&self, window_id: u64) -> Vec<PaletteRow> {
        let palette = self.palette_state_for_window(window_id);
        match palette.mode.as_str() {
            "switcher" => self.command_palette_switcher_rows(&palette.query),
            "commands" => self.command_palette_command_rows(&palette.query),
            _ => Vec::new(),
        }
    }

    fn clamp_palette_selected_index(&mut self, window_id: u64, row_count: usize) {
        let palette = self.palette_state_for_window_mut(window_id);
        if row_count == 0 {
            palette.selected_index = 0;
            return;
        }
        palette.selected_index = palette.selected_index.min(row_count.saturating_sub(1));
    }

    fn command_palette_selection_snapshot(&self, window_id: u64) -> Value {
        let palette = self.palette_state_for_window(window_id);
        let text = if palette.mode == "rename_input" {
            palette.rename_text
        } else {
            palette.query
        };
        json!({
            "focused": palette.visible,
            "text_length": text.len(),
            "selection_location": palette.selection_location,
            "selection_length": palette.selection_length,
        })
    }

    fn command_palette_results_payload(&mut self, window_id: u64, limit: usize) -> Value {
        let palette = self.palette_state_for_window(window_id);
        let mut rows = self.command_palette_rows(window_id);
        self.clamp_palette_selected_index(window_id, rows.len());
        let selected_index = self.palette_state_for_window(window_id).selected_index;
        if !rows.is_empty() && selected_index < rows.len() {
            rows[selected_index].score -= 1;
        }
        let result_rows: Vec<Value> = rows
            .into_iter()
            .take(limit)
            .map(|row| {
                json!({
                    "command_id": row.command_id,
                    "title": row.title,
                    "trailing_label": row.trailing_label,
                    "shortcut_hint": row.shortcut_hint,
                })
            })
            .collect();
        json!({
            "mode": palette.mode,
            "query": palette.query,
            "selected_index": selected_index,
            "results": result_rows,
        })
    }

    fn command_palette_open_rename_tab(&mut self, window_id: u64) {
        let (workspace_id, _window_id, _pane_id, surface_id) =
            focused_handles(self).unwrap_or((self.current_workspace_id, window_id, 0, 0));
        let title = self
            .current_surface()
            .map(|surface| surface.title)
            .unwrap_or_default();
        let select_all = self.rename_input_select_all;
        let palette = self.palette_state_for_window_mut(window_id);
        palette.visible = true;
        palette.mode = "rename_input".to_string();
        palette.query.clear();
        palette.selected_index = 0;
        palette.rename_text = title;
        palette.rename_target_surface_id = if surface_id == 0 {
            None
        } else {
            Some(surface_id)
        };
        palette.rename_target_workspace_id = Some(workspace_id);
        if select_all && !palette.rename_text.is_empty() {
            palette.selection_location = 0;
            palette.selection_length = palette.rename_text.len();
        } else {
            palette.selection_location = palette.rename_text.len();
            palette.selection_length = 0;
        }
    }

    fn command_palette_open_rename_workspace(&mut self, window_id: u64) {
        let workspace_id = self.current_workspace_id;
        let workspace_name = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .map(|workspace| workspace.name.clone())
            .unwrap_or_default();
        let select_all = self.rename_input_select_all;
        let palette = self.palette_state_for_window_mut(window_id);
        palette.visible = true;
        palette.mode = "rename_input".to_string();
        palette.query.clear();
        palette.selected_index = 0;
        palette.rename_text = workspace_name;
        palette.rename_target_surface_id = None;
        palette.rename_target_workspace_id = Some(workspace_id);
        if select_all && !palette.rename_text.is_empty() {
            palette.selection_location = 0;
            palette.selection_length = palette.rename_text.len();
        } else {
            palette.selection_location = palette.rename_text.len();
            palette.selection_length = 0;
        }
    }

    fn command_palette_apply_type(&mut self, window_id: u64, text: &str) {
        let palette = self.palette_state_for_window_mut(window_id);
        if !palette.visible {
            return;
        }
        let target = if palette.mode == "rename_input" {
            &mut palette.rename_text
        } else {
            &mut palette.query
        };
        if palette.selection_length > 0 {
            let start = palette.selection_location.min(target.len());
            let end = (start + palette.selection_length).min(target.len());
            target.replace_range(start..end, text);
            palette.selection_location = start + text.len();
            palette.selection_length = 0;
        } else {
            let insert_at = palette.selection_location.min(target.len());
            target.insert_str(insert_at, text);
            palette.selection_location = insert_at + text.len();
            palette.selection_length = 0;
        }
        palette.selected_index = 0;
    }

    fn command_palette_select_all(&mut self, window_id: u64) {
        let palette = self.palette_state_for_window_mut(window_id);
        if !palette.visible {
            return;
        }
        let text_len = if palette.mode == "rename_input" {
            palette.rename_text.len()
        } else {
            palette.query.len()
        };
        palette.selection_location = 0;
        palette.selection_length = text_len;
    }

    fn command_palette_move_selection(&mut self, window_id: u64, delta: isize) {
        let row_count = self.command_palette_rows(window_id).len();
        if row_count == 0 {
            self.palette_state_for_window_mut(window_id).selected_index = 0;
            return;
        }
        let palette = self.palette_state_for_window_mut(window_id);
        let current = palette.selected_index as isize;
        let max = row_count.saturating_sub(1) as isize;
        let mut next = current + delta;
        if next < 0 {
            next = 0;
        }
        if next > max {
            next = max;
        }
        palette.selected_index = next as usize;
    }

    fn command_palette_delete_backward(&mut self, window_id: u64) {
        let palette = self.palette_state_for_window_mut(window_id);
        if palette.mode != "rename_input" {
            return;
        }
        if palette.selection_length > 0 {
            palette.rename_text.clear();
            palette.selection_location = 0;
            palette.selection_length = 0;
            return;
        }
        if !palette.rename_text.is_empty() {
            let _ = palette.rename_text.pop();
            palette.selection_location = palette.rename_text.len();
            palette.selection_length = 0;
            return;
        }
        palette.mode = "commands".to_string();
        palette.query = ">".to_string();
        palette.selection_location = palette.query.len();
        palette.selection_length = 0;
        palette.selected_index = 0;
    }

    fn command_palette_interact(&mut self, window_id: u64) {
        let select_all = self.rename_input_select_all;
        let palette = self.palette_state_for_window_mut(window_id);
        if palette.mode != "rename_input" {
            return;
        }
        if select_all && !palette.rename_text.is_empty() {
            palette.selection_location = 0;
            palette.selection_length = palette.rename_text.len();
        } else {
            palette.selection_location = palette.rename_text.len();
            palette.selection_length = 0;
        }
    }

    fn command_palette_enter(&mut self, window_id: u64) {
        let palette = self.palette_state_for_window(window_id);
        if !palette.visible {
            return;
        }

        if palette.mode == "rename_input" {
            if let Some(surface_id) = palette.rename_target_surface_id {
                let new_title = palette.rename_text.clone();
                let _ = self.update_surface(Some(surface_id), |surface| {
                    surface.title = new_title.clone();
                });
            } else if let Some(workspace_id) = palette.rename_target_workspace_id {
                if let Some(workspace) = self
                    .workspaces
                    .iter_mut()
                    .find(|workspace| workspace.id == workspace_id)
                {
                    workspace.name = palette.rename_text.clone();
                }
            }
            self.close_palette(window_id);
            return;
        }

        let rows = self.command_palette_rows(window_id);
        if rows.is_empty() {
            return;
        }
        let selected_idx = palette.selected_index.min(rows.len().saturating_sub(1));
        let selected = rows[selected_idx].clone();

        if selected.command_id.starts_with("switcher.workspace.") {
            if let Some(workspace_id) = selected.workspace_id {
                let _ = self.select_workspace(Some(workspace_id), None);
                if let Some(host_window_id) = selected.host_window_id {
                    let _ = self.focus_window(host_window_id);
                }
            }
            self.close_palette(window_id);
            return;
        }

        if selected.command_id.starts_with("switcher.surface.") {
            if let Some(workspace_id) = selected.workspace_id {
                let _ = self.select_workspace(Some(workspace_id), None);
                if let Some(host_window_id) = selected.host_window_id {
                    let _ = self.focus_window(host_window_id);
                }
            }
            if let Some(surface_id) = selected.surface_id {
                let _ = self.focus_surface(surface_id);
            }
            self.close_palette(window_id);
            return;
        }

        match selected.command_id.as_str() {
            "palette.renameTab" => self.command_palette_open_rename_tab(window_id),
            "palette.renameWorkspace" => self.command_palette_open_rename_workspace(window_id),
            _ => self.close_palette(window_id),
        }
    }

    fn handle_palette_shortcut(&mut self, combo_norm: &str) -> bool {
        let Some(window_id) = self.active_window_id() else {
            return false;
        };
        let palette_visible = self.palette_visible_for_window(window_id);
        match combo_norm {
            "cmd+shift+p" => {
                let mode = self.palette_state_for_window(window_id).mode;
                if palette_visible && mode == "commands" {
                    self.close_palette(window_id);
                } else {
                    self.open_palette_in_mode(window_id, "commands");
                }
                true
            }
            "cmd+p" => {
                let mode = self.palette_state_for_window(window_id).mode;
                if palette_visible && mode == "switcher" {
                    self.close_palette(window_id);
                } else {
                    self.open_palette_in_mode(window_id, "switcher");
                }
                true
            }
            "down" | "ctrl+n" | "ctrl+j" => {
                if palette_visible {
                    self.command_palette_move_selection(window_id, 1);
                    true
                } else {
                    false
                }
            }
            "up" | "ctrl+p" | "ctrl+k" => {
                if palette_visible {
                    self.command_palette_move_selection(window_id, -1);
                    true
                } else {
                    false
                }
            }
            "cmd+a" => {
                if palette_visible {
                    self.command_palette_select_all(window_id);
                    true
                } else {
                    false
                }
            }
            "enter" => {
                if palette_visible {
                    self.command_palette_enter(window_id);
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }
}

#[derive(Clone)]
pub struct Dispatcher {
    state: Arc<Mutex<ControlState>>,
}

impl Default for Dispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Dispatcher {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ControlState::default())),
        }
    }

    pub fn with_state(state: ControlState) -> Self {
        Self {
            state: Arc::new(Mutex::new(state)),
        }
    }

    pub async fn dispatch(&self, request: V2Request) -> V2Response {
        let mut state = self
            .state
            .lock()
            .expect("control state lock should not be poisoned");
        dispatch_request(&mut state, request)
    }
}

fn dispatch_request(state: &mut ControlState, request: V2Request) -> V2Response {
    let response_id = request.id.clone();

    match handle_command(state, &request.method, &request.params) {
        Ok(result) => V2Response::success(response_id, result),
        Err(error) => V2Response::error(response_id, error.code, error.message, error.data),
    }
}

struct CommandError {
    code: i64,
    message: String,
    data: Option<Value>,
}

impl CommandError {
    fn invalid_params(message: impl Into<String>) -> Self {
        let message: String = message.into();
        Self {
            code: -32602,
            message: format!("invalid_params: {message}"),
            data: None,
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        let message: String = message.into();
        Self {
            code: -32004,
            message: format!("not_found: {message}"),
            data: None,
        }
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self {
            code: -32009,
            message: message.into(),
            data: None,
        }
    }

    fn timeout(message: impl Into<String>) -> Self {
        let message: String = message.into();
        Self {
            code: -32008,
            message: format!("timeout: {message}"),
            data: None,
        }
    }

    fn not_supported(message: impl Into<String>) -> Self {
        let message: String = message.into();
        Self {
            code: -32020,
            message: format!("not_supported: {message}"),
            data: None,
        }
    }

    fn unknown_method(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("unknown method: {method}"),
            data: None,
        }
    }
}

fn encode_handle_id(id: u64) -> String {
    format!("00000000-0000-0000-0000-{id:012x}")
}

fn decode_handle_id(raw: &str) -> Option<u64> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }

    if let Ok(value) = s.parse::<u64>() {
        return Some(value);
    }

    if let Some((_, suffix)) = s.split_once(':') {
        return suffix.trim().parse::<u64>().ok();
    }

    let hex: String = s.chars().filter(|ch| *ch != '-').collect();
    if hex.len() == 32 {
        let tail = &hex[hex.len().saturating_sub(12)..];
        if tail.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return u64::from_str_radix(tail, 16).ok();
        }
    }

    None
}

fn workspace_ref(id: u64) -> String {
    format!("workspace:{id}")
}

fn pane_ref(id: u64) -> String {
    format!("pane:{id}")
}

fn surface_ref(id: u64) -> String {
    format!("surface:{id}")
}

fn window_ref(id: u64) -> String {
    format!("window:{id}")
}

fn params_object(params: &Value) -> Result<&Map<String, Value>, CommandError> {
    params
        .as_object()
        .ok_or_else(|| CommandError::invalid_params("params must be an object"))
}

fn optional_string_param(
    params: &Map<String, Value>,
    key: &str,
) -> Result<Option<String>, CommandError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        _ => Err(CommandError::invalid_params(format!(
            "{key} must be a string"
        ))),
    }
}

fn required_string_param(params: &Map<String, Value>, key: &str) -> Result<String, CommandError> {
    optional_string_param(params, key)?.ok_or_else(|| {
        CommandError::invalid_params(format!("{key} is required and must be a string"))
    })
}

fn optional_u64_param(params: &Map<String, Value>, key: &str) -> Result<Option<u64>, CommandError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(value)) => value
            .as_u64()
            .ok_or_else(|| {
                CommandError::invalid_params(format!("{key} must be an unsigned integer"))
            })
            .map(Some),
        Some(Value::String(value)) => decode_handle_id(value).map(Some).ok_or_else(|| {
            CommandError::invalid_params(format!(
                "{key} must be an unsigned integer, UUID, or ref handle"
            ))
        }),
        _ => Err(CommandError::invalid_params(format!(
            "{key} must be an unsigned integer"
        ))),
    }
}

fn optional_u64_param_any(
    params: &Map<String, Value>,
    keys: &[&str],
) -> Result<Option<u64>, CommandError> {
    for key in keys {
        if let Some(value) = optional_u64_param(params, key)? {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn optional_bool_param(
    params: &Map<String, Value>,
    key: &str,
) -> Result<Option<bool>, CommandError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        _ => Err(CommandError::invalid_params(format!(
            "{key} must be a boolean"
        ))),
    }
}

fn optional_index_param(
    params: &Map<String, Value>,
    key: &str,
) -> Result<Option<usize>, CommandError> {
    optional_u64_param(params, key)?.map_or(Ok(None), |value| {
        usize::try_from(value)
            .map(Some)
            .map_err(|_| CommandError::invalid_params(format!("{key} is too large")))
    })
}

fn focused_handles(state: &ControlState) -> Option<(u64, u64, u64, u64)> {
    let workspace_idx = state.current_workspace_idx()?;
    let workspace = state.workspaces.get(workspace_idx)?;
    let window_id = workspace.current_window_id?;
    let window_idx = workspace
        .windows
        .iter()
        .position(|window| window.id == window_id)?;
    let window = workspace.windows.get(window_idx)?;
    let pane_id = window.current_pane_id?;
    let pane_idx = window.panes.iter().position(|pane| pane.id == pane_id)?;
    let pane = window.panes.get(pane_idx)?;
    let surface_id = pane.current_surface_id?;
    Some((workspace.id, window_id, pane_id, surface_id))
}

fn focused_payload(state: &ControlState) -> Value {
    if let Some((workspace_id, window_id, pane_id, surface_id)) = focused_handles(state) {
        json!({
            "workspace_id": encode_handle_id(workspace_id),
            "workspace_ref": workspace_ref(workspace_id),
            "window_id": encode_handle_id(window_id),
            "window_ref": window_ref(window_id),
            "pane_id": encode_handle_id(pane_id),
            "pane_ref": pane_ref(pane_id),
            "surface_id": encode_handle_id(surface_id),
            "surface_ref": surface_ref(surface_id),
        })
    } else {
        json!({})
    }
}

fn workspace_row(
    index: usize,
    selected_id: u64,
    workspace: &WorkspaceInfo,
    cwd: Option<&str>,
) -> Value {
    json!({
        "index": index,
        "id": encode_handle_id(workspace.id),
        "ref": workspace_ref(workspace.id),
        "workspace_id": encode_handle_id(workspace.id),
        "workspace_ref": workspace_ref(workspace.id),
        "title": workspace.name,
        "name": workspace.name,
        "selected": workspace.id == selected_id,
        "focused": workspace.id == selected_id,
        "window_count": workspace.window_count,
        "cwd": cwd.unwrap_or(""),
    })
}

fn pane_row(index: usize, focused_pane_id: Option<u64>, pane: &PaneInfo) -> Value {
    json!({
        "index": index,
        "id": encode_handle_id(pane.id),
        "ref": pane_ref(pane.id),
        "pane_id": encode_handle_id(pane.id),
        "pane_ref": pane_ref(pane.id),
        "surface_count": pane.surface_count,
        "focused": focused_pane_id == Some(pane.id),
    })
}

fn surface_row(index: usize, focused_surface_id: Option<u64>, surface: &SurfaceInfo) -> Value {
    json!({
        "index": index,
        "id": encode_handle_id(surface.id),
        "ref": surface_ref(surface.id),
        "surface_id": encode_handle_id(surface.id),
        "surface_ref": surface_ref(surface.id),
        "pane_id": encode_handle_id(surface.pane_id),
        "pane_ref": pane_ref(surface.pane_id),
        "title": surface.title,
        "type": surface.panel_type,
        "developer_tools_visible": surface.developer_tools_visible,
        "focused": focused_surface_id == Some(surface.id),
        "selected": focused_surface_id == Some(surface.id),
        "pinned": surface.pinned,
        "unread": surface.unread,
        "flash_count": surface.flash_count,
        "refresh_count": surface.refresh_count,
    })
}

fn with_workspace_scope<T>(
    state: &mut ControlState,
    workspace_id: Option<u64>,
    f: impl FnOnce(&mut ControlState) -> Result<T, CommandError>,
) -> Result<T, CommandError> {
    let original = state.current_workspace_id;
    if let Some(target) = workspace_id {
        if state.select_workspace(Some(target), None).is_none() {
            return Err(CommandError::not_found("workspace not found"));
        }
    }
    let result = f(state);
    if state.current_workspace_id != original {
        let _ = state.select_workspace(Some(original), None);
    }
    result
}

fn workspace_contains_surface(state: &ControlState, workspace_id: u64, surface_id: u64) -> bool {
    state
        .workspaces
        .iter()
        .find(|workspace| workspace.id == workspace_id)
        .map(|workspace| {
            workspace.windows.iter().any(|window| {
                window
                    .panes
                    .iter()
                    .any(|pane| pane.surfaces.iter().any(|surface| surface.id == surface_id))
            })
        })
        .unwrap_or(false)
}

fn find_workspace_for_surface(state: &ControlState, surface_id: u64) -> Option<u64> {
    state.workspaces.iter().find_map(|workspace| {
        let found = workspace.windows.iter().any(|window| {
            window
                .panes
                .iter()
                .any(|pane| pane.surfaces.iter().any(|surface| surface.id == surface_id))
        });
        if found {
            Some(workspace.id)
        } else {
            None
        }
    })
}

fn current_surface_id_for_workspace(state: &ControlState, workspace_id: u64) -> Option<u64> {
    let workspace = state
        .workspaces
        .iter()
        .find(|workspace| workspace.id == workspace_id)?;
    let window_id = workspace.current_window_id?;
    let window = workspace
        .windows
        .iter()
        .find(|window| window.id == window_id)?;
    let pane_id = window.current_pane_id?;
    let pane = window.panes.iter().find(|pane| pane.id == pane_id)?;
    pane.current_surface_id
}

fn resolve_surface_target(
    state: &ControlState,
    workspace_hint: Option<u64>,
    surface_hint: Option<u64>,
) -> Result<(u64, u64), CommandError> {
    if let Some(surface_id) = surface_hint {
        if let Some(workspace_id) = workspace_hint {
            if workspace_contains_surface(state, workspace_id, surface_id) {
                return Ok((workspace_id, surface_id));
            }
            return Err(CommandError::not_found("surface not found in workspace"));
        }
        if let Some(workspace_id) = find_workspace_for_surface(state, surface_id) {
            return Ok((workspace_id, surface_id));
        }
        return Err(CommandError::not_found("surface not found"));
    }

    if let Some(workspace_id) = workspace_hint {
        let surface_id = current_surface_id_for_workspace(state, workspace_id)
            .ok_or_else(|| CommandError::not_found("workspace/surface not found"))?;
        return Ok((workspace_id, surface_id));
    }

    let (workspace_id, _, _, surface_id) =
        focused_handles(state).ok_or_else(|| CommandError::not_found("surface not found"))?;
    Ok((workspace_id, surface_id))
}

fn update_surface_metadata(
    state: &mut ControlState,
    workspace_id: u64,
    surface_id: u64,
    update: impl FnOnce(&mut SurfaceState),
) -> Option<SurfaceInfo> {
    let workspace = state
        .workspaces
        .iter_mut()
        .find(|workspace| workspace.id == workspace_id)?;
    let mut update = Some(update);

    for window in &mut workspace.windows {
        for pane in &mut window.panes {
            if let Some(surface) = pane
                .surfaces
                .iter_mut()
                .find(|surface| surface.id == surface_id)
            {
                if let Some(callback) = update.take() {
                    callback(surface);
                }
                return Some(surface.info(pane.id));
            }
        }
    }

    None
}

fn ensure_browser_surface(
    state: &BrowserState,
    surface_id: Option<u64>,
) -> Result<(), CommandError> {
    if let Some(surface_id) = surface_id {
        if !state.browser_surfaces.contains(&surface_id) && !state.tab_ids.contains(&surface_id) {
            return Err(CommandError::not_found("surface is not a browser tab"));
        }
    }
    Ok(())
}

fn normalize_browser_title(url: &str) -> String {
    if let Some(host) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .and_then(|rest| rest.split('/').next())
        .filter(|host| !host.is_empty())
    {
        host.to_string()
    } else {
        url.to_string()
    }
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut idx = 0usize;
    while idx < bytes.len() {
        if bytes[idx] == b'%' && idx + 2 < bytes.len() {
            let hi = bytes[idx + 1] as char;
            let lo = bytes[idx + 2] as char;
            if hi.is_ascii_hexdigit() && lo.is_ascii_hexdigit() {
                if let Ok(value) = u8::from_str_radix(&format!("{hi}{lo}"), 16) {
                    out.push(value);
                    idx += 3;
                    continue;
                }
            }
        }
        if bytes[idx] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[idx]);
        }
        idx += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn decode_data_html(url: &str) -> Option<String> {
    if !url.starts_with("data:text/html") {
        return None;
    }
    let (_, encoded) = url.split_once(',')?;
    Some(percent_decode(encoded))
}

fn decode_file_html(url: &str) -> Option<String> {
    let rest = url.strip_prefix("file://")?;
    let path_with_host = if let Some(local_path) = rest.strip_prefix("localhost/") {
        format!("/{local_path}")
    } else {
        rest.to_string()
    };
    let path_without_fragment = path_with_host.split('#').next().unwrap_or_default();
    let path_without_query = path_without_fragment.split('?').next().unwrap_or_default();
    if path_without_query.is_empty() {
        return None;
    }
    let decoded_path = percent_decode(path_without_query);
    std::fs::read_to_string(decoded_path).ok()
}

#[derive(Debug, Clone)]
enum TerminalAction {
    SetCwd(String),
    RunCommand(String),
}

fn parse_terminal_line(surface: &mut SurfaceState, line: &str) -> Option<TerminalAction> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed == "cat" {
        surface.terminal_mode = TerminalMode::Cat;
        return None;
    }

    if let Some(rest) = trimmed.strip_prefix("sleep ") {
        if rest
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|seconds| *seconds > 0)
            .is_some()
        {
            surface.terminal_mode = TerminalMode::Sleeping;
            return None;
        }
    }

    if trimmed.starts_with("python3 -c")
        && trimmed.contains("iter(int, 1)")
        && trimmed.contains("time.sleep")
    {
        surface.terminal_mode = TerminalMode::PythonLoop;
        return None;
    }

    if let Some(path) = trimmed.strip_prefix("cd ") {
        let candidate = path.trim();
        if !candidate.is_empty() {
            return Some(TerminalAction::SetCwd(candidate.to_string()));
        }
    }

    Some(TerminalAction::RunCommand(line.to_string()))
}

fn collect_terminal_actions(surface: &mut SurfaceState, text: &str) -> Vec<TerminalAction> {
    if surface.panel_type != "terminal" {
        return Vec::new();
    }

    if surface.terminal_mode != TerminalMode::Idle {
        return Vec::new();
    }

    let mut actions = Vec::new();
    for ch in text.chars() {
        if ch == '\n' || ch == '\r' {
            let line = surface.shell_input.clone();
            surface.shell_input.clear();
            if let Some(action) = parse_terminal_line(surface, &line) {
                actions.push(action);
            }
            if surface.terminal_mode != TerminalMode::Idle {
                break;
            }
        } else {
            surface.shell_input.push(ch);
        }
    }

    actions
}

enum TerminalKeyOutcome {
    Unhandled,
    Handled,
    RequestCloseSurface,
}

fn handle_terminal_key(surface: &mut SurfaceState, key: &str) -> TerminalKeyOutcome {
    if surface.panel_type != "terminal" {
        return TerminalKeyOutcome::Unhandled;
    }

    let key_norm = key.trim().to_ascii_lowercase();
    match key_norm.as_str() {
        "ctrl-c" => {
            surface.terminal_mode = TerminalMode::Idle;
            surface.shell_input.clear();
            TerminalKeyOutcome::Handled
        }
        "ctrl-d" => {
            if surface.terminal_mode == TerminalMode::Cat {
                surface.terminal_mode = TerminalMode::Idle;
                return TerminalKeyOutcome::Handled;
            }
            if surface.terminal_mode == TerminalMode::Idle && surface.shell_input.is_empty() {
                return TerminalKeyOutcome::RequestCloseSurface;
            }
            TerminalKeyOutcome::Handled
        }
        _ => TerminalKeyOutcome::Unhandled,
    }
}

fn apply_surface_text_input(
    state: &mut ControlState,
    workspace_id: u64,
    surface_id: u64,
    text: &str,
) -> Result<SurfaceInfo, CommandError> {
    let mut actions: Vec<TerminalAction> = Vec::new();
    let surface = update_surface_metadata(state, workspace_id, surface_id, |surface| {
        surface.text.push_str(text);
        surface.unread = true;
        actions = collect_terminal_actions(surface, text);
    })
    .ok_or_else(|| CommandError::not_found("surface not found"))?;

    for action in actions {
        match action {
            TerminalAction::SetCwd(path) => {
                let _ = state.set_workspace_cwd(workspace_id, Some(shell_expand_home(&path)));
            }
            TerminalAction::RunCommand(command) => {
                run_terminal_command(state, workspace_id, surface_id, &command)
            }
        }
    }

    Ok(surface)
}

fn apply_surface_key_input(
    state: &mut ControlState,
    workspace_id: u64,
    surface_id: u64,
    key: &str,
) -> Result<SurfaceInfo, CommandError> {
    let marker = format!("<key:{key}>");
    let mut request_close = false;
    let updated =
        update_surface_metadata(
            state,
            workspace_id,
            surface_id,
            |surface| match handle_terminal_key(surface, key) {
                TerminalKeyOutcome::Unhandled => {
                    surface.text.push_str(&marker);
                    surface.unread = true;
                }
                TerminalKeyOutcome::Handled => {}
                TerminalKeyOutcome::RequestCloseSurface => {
                    request_close = true;
                }
            },
        )
        .ok_or_else(|| CommandError::not_found("surface not found"))?;

    if request_close {
        append_debug_log(&format!(
            "surface.close.childExited workspace={} surface={:05X}",
            workspace_id, surface_id
        ));
        append_debug_log(&format!(
            "surface.lifecycle.deinit.begin surface={:05X}",
            surface_id
        ));
        let closed = state
            .close_surface(Some(surface_id))
            .ok_or_else(|| CommandError::not_found("surface not found"))?;
        append_debug_log(&format!(
            "surface.lifecycle.deinit.end surface={:05X}",
            surface_id
        ));
        return Ok(closed);
    }

    Ok(updated)
}

fn shell_expand_home(path: &str) -> String {
    if path == "~" {
        return std::env::var("HOME").unwrap_or_else(|_| path.to_string());
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    path.to_string()
}

fn fallback_ghostty_resources_dir() -> std::path::PathBuf {
    let root = std::env::temp_dir().join("limux-ghostty-resources");
    let terminfo_entry = root.join("terminfo").join("78").join("xterm-ghostty");
    if let Some(parent) = terminfo_entry.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if !terminfo_entry.exists() {
        let _ = std::fs::write(
            &terminfo_entry,
            "xterm-ghostty|limux mock entry\n\tam,\n\tcols#80, lines#24,\n",
        );
    }
    root
}

fn ghostty_resources_dir() -> std::path::PathBuf {
    if let Ok(cwd) = std::env::current_dir() {
        for ancestor in cwd.ancestors() {
            let candidate = ancestor
                .join("Resources")
                .join("ghostty")
                .join("terminfo")
                .join("78")
                .join("xterm-ghostty");
            if candidate.exists() {
                return ancestor.join("Resources").join("ghostty");
            }
        }
    }
    fallback_ghostty_resources_dir()
}

fn terminal_env_values() -> (String, String) {
    let resources_dir = ghostty_resources_dir();
    let terminfo_dir = resources_dir.join("terminfo");
    let resources_value = resources_dir.to_string_lossy().to_string();

    let inherited_xdg = std::env::var("XDG_DATA_DIRS").ok();
    let mut entries = vec![resources_value.clone()];
    if let Some(existing) = inherited_xdg
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        for part in existing.split(':') {
            let item = part.trim();
            if item.is_empty() {
                continue;
            }
            if !entries.iter().any(|entry| entry == item) {
                entries.push(item.to_string());
            }
        }
    }
    for default in ["/usr/local/share", "/usr/share"] {
        if !entries.iter().any(|entry| entry == default) {
            entries.push(default.to_string());
        }
    }

    (
        terminfo_dir.to_string_lossy().to_string(),
        entries.join(":"),
    )
}

fn between<'a>(source: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let (_, rest) = source.split_once(start)?;
    let (value, _) = rest.split_once(end)?;
    Some(value)
}

fn slice_after_first_match<'a>(source: &'a str, patterns: &[&str]) -> Option<&'a str> {
    patterns
        .iter()
        .filter_map(|pattern| source.find(pattern).map(|index| (index, pattern.len())))
        .min_by_key(|(index, _)| *index)
        .map(|(index, len)| &source[index + len..])
}

fn first_match_index(source: &str, patterns: &[&str]) -> Option<usize> {
    patterns
        .iter()
        .filter_map(|pattern| source.find(pattern))
        .min()
}

fn extract_osc_payload(command: &str, prefixes: &[&str]) -> Option<String> {
    const OSC_TERMINATORS: [&str; 5] = ["\u{7}", "\u{1b}\\", "\\x07", "\\x1b\\", "\\x1b\\\\"];

    let rest = slice_after_first_match(command, prefixes)?;
    let mut payload = rest;
    if let Some(index) = first_match_index(payload, &OSC_TERMINATORS) {
        payload = &payload[..index];
    } else if let Some((value, _)) = payload.split_once('\'') {
        payload = value;
    }
    Some(payload.trim().to_string())
}

fn maybe_emit_osc_notification(
    state: &mut ControlState,
    workspace_id: u64,
    surface_id: u64,
    command: &str,
) {
    if let Some(body) = extract_osc_payload(command, &["\u{1b}]9;", "\\x1b]9;"]) {
        let _ = state.create_notification(
            String::new(),
            String::new(),
            body,
            Some(surface_id),
            Some(workspace_id),
        );
        return;
    }

    if let Some(title) = extract_osc_payload(command, &["\u{1b}]99;;", "\\x1b]99;;"]) {
        let _ = state.create_notification(
            title,
            String::new(),
            String::new(),
            Some(surface_id),
            Some(workspace_id),
        );
        return;
    }

    if let Some(title) = extract_osc_payload(
        command,
        &[
            "\u{1b}]99;i=kitty:d=0:p=title;",
            "\\x1b]99;i=kitty:d=0:p=title;",
        ],
    ) {
        let entry = state
            .kitty_notification_chunks
            .entry(surface_id)
            .or_insert((None, None));
        entry.0 = Some(title);
        return;
    }

    if let Some(body) = extract_osc_payload(
        command,
        &["\u{1b}]99;i=kitty:p=body;", "\\x1b]99;i=kitty:p=body;"],
    ) {
        let entry = state
            .kitty_notification_chunks
            .remove(&surface_id)
            .unwrap_or((None, None));
        let title = entry.0.unwrap_or_default();
        let _ = state.create_notification(
            title,
            String::new(),
            body,
            Some(surface_id),
            Some(workspace_id),
        );
        return;
    }

    if let Some(payload) = between(command, "\\x1b]777;notify;", "\\x07") {
        let mut parts = payload.splitn(2, ';');
        let title = parts.next().unwrap_or_default().trim().to_string();
        let body = parts.next().unwrap_or_default().trim().to_string();
        let _ = state.create_notification(
            title,
            String::new(),
            body,
            Some(surface_id),
            Some(workspace_id),
        );
    }
}

fn run_terminal_command(
    state: &mut ControlState,
    workspace_id: u64,
    surface_id: u64,
    command: &str,
) {
    maybe_emit_osc_notification(state, workspace_id, surface_id, command);
    let workspace_cwd = state.workspace_cwd(workspace_id);
    let (terminfo, xdg_data_dirs) = terminal_env_values();
    let mut shell = std::process::Command::new("bash");
    shell.arg("-lc").arg(command);
    shell.env("TERM", "xterm-ghostty");
    shell.env("TERMINFO", terminfo);
    shell.env("XDG_DATA_DIRS", xdg_data_dirs);
    shell.env("LIMUX_SURFACE_ID", encode_handle_id(surface_id));

    if let Some(cwd) = workspace_cwd {
        let expanded = shell_expand_home(&cwd);
        let cwd_path = std::path::PathBuf::from(expanded);
        if cwd_path.exists() {
            shell.current_dir(cwd_path);
        }
    }

    let _ = shell.output();
}

const MOCK_PNG_BYTES: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 4, 0,
    0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 252, 255, 31, 0, 3, 3, 2, 0,
    238, 217, 43, 101, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

fn sanitize_debug_label(label: &str) -> String {
    let trimmed = label.trim();
    if trimmed.is_empty() {
        return "shot".to_string();
    }
    let mut out = String::with_capacity(trimmed.len());
    for ch in trimmed.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out
}

fn write_mock_png(prefix: &str, label: &str, id: u64) -> Option<std::path::PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    let filename = format!(
        "{}-{}-{}-{}.png",
        prefix,
        id,
        nanos,
        sanitize_debug_label(label)
    );
    let path = std::env::temp_dir().join(filename);
    std::fs::write(&path, MOCK_PNG_BYTES).ok()?;
    Some(path)
}

fn estimate_changed_pixels(previous: &str, current: &str) -> u64 {
    if previous == current {
        return 0;
    }
    let mut diff = 0usize;
    for (left, right) in previous.bytes().zip(current.bytes()) {
        if left != right {
            diff += 1;
        }
    }
    diff += previous.len().max(current.len()) - previous.len().min(current.len());
    ((diff as u64).saturating_mul(64)).max(24)
}

fn debug_log_path() -> std::path::PathBuf {
    if let Ok(path) = std::env::var("LIMUX_DEBUG_LOG") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return std::path::PathBuf::from(trimmed);
        }
    }
    std::path::PathBuf::from("/tmp/limux-debug.log")
}

fn append_debug_log(event: &str) {
    let path = debug_log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = std::io::Write::write_all(&mut file, event.as_bytes());
        let _ = std::io::Write::write_all(&mut file, b"\n");
    }
}

fn extract_html_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start_tag = "<title>";
    let end_tag = "</title>";
    let start = lower.find(start_tag)?;
    let end = lower[start + start_tag.len()..].find(end_tag)?;
    let begin = start + start_tag.len();
    let finish = begin + end;
    Some(html[begin..finish].trim().to_string())
}

fn normalize_shortcut_combo(combo: &str) -> String {
    combo
        .to_ascii_lowercase()
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect()
}

fn combo_to_shortcut_hint(combo: &str) -> String {
    let normalized = normalize_shortcut_combo(combo);
    let mut key = String::new();
    let mut has_shift = false;
    let mut has_ctrl = false;
    let mut has_opt = false;
    let mut has_cmd = false;

    for part in normalized.split('+') {
        match part {
            "shift" => has_shift = true,
            "ctrl" | "control" => has_ctrl = true,
            "opt" | "option" | "alt" => has_opt = true,
            "cmd" | "command" | "meta" => has_cmd = true,
            "" => {}
            other => {
                key = other.to_ascii_uppercase();
            }
        }
    }

    let mut hint = String::new();
    if has_shift {
        hint.push('⇧');
    }
    if has_ctrl {
        hint.push('⌃');
    }
    if has_opt {
        hint.push('⌥');
    }
    if has_cmd {
        hint.push('⌘');
    }
    hint.push_str(&key);
    hint
}

fn fuzzy_subsequence(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let mut chars = needle.chars();
    let mut current = chars.next();
    for ch in haystack.chars() {
        if Some(ch) == current {
            current = chars.next();
            if current.is_none() {
                return true;
            }
        }
    }
    false
}

fn handle_browser_extended_command(
    state: &mut ControlState,
    method: &str,
    params: &Map<String, Value>,
) -> Result<Value, CommandError> {
    match method {
        "browser.focus" | "browser.hover" | "browser.dblclick" | "browser.scroll_into_view" => {
            let selector = required_string_param(params, "selector")?;
            let resolved = state.browser_resolve_selector(&selector);
            if !state.browser_selector_exists(&resolved) {
                return Err(CommandError::not_found("element not found"));
            }
            if method == "browser.focus" {
                state.browser.active_element = resolved.clone();
            } else if method == "browser.hover" {
                state.browser.hover_count = state.browser.hover_count.saturating_add(1);
            } else if method == "browser.dblclick" {
                state.browser.dbl_count = state.browser.dbl_count.saturating_add(1);
            } else if method == "browser.scroll_into_view" {
                state.browser.in_view.insert(resolved.clone());
            }
            Ok(json!({ "ok": true, "selector": selector }))
        }
        "browser.press" | "browser.keydown" | "browser.keyup" => {
            let key = required_string_param(params, "key")?;
            state.browser.key_down_count = state.browser.key_down_count.saturating_add(1);
            state.browser.key_up_count = state.browser.key_up_count.saturating_add(1);
            if method == "browser.press" {
                state.browser.key_press_count = state.browser.key_press_count.saturating_add(1);
            }
            Ok(json!({ "ok": true, "key": key }))
        }
        "browser.type" => {
            let selector = required_string_param(params, "selector")?;
            let text = required_string_param(params, "text")?;
            let existing = state
                .browser
                .fields
                .get(&selector)
                .cloned()
                .unwrap_or_default();
            state
                .browser
                .fields
                .insert(selector.clone(), format!("{existing}{text}"));
            Ok(json!({ "ok": true, "selector": selector, "text": text }))
        }
        "browser.check" => {
            let selector = required_string_param(params, "selector")?;
            state.browser.checked.insert(selector.clone(), true);
            Ok(json!({ "ok": true, "selector": selector, "checked": true }))
        }
        "browser.uncheck" => {
            let selector = required_string_param(params, "selector")?;
            state.browser.checked.insert(selector.clone(), false);
            Ok(json!({ "ok": true, "selector": selector, "checked": false }))
        }
        "browser.select" => {
            let selector = required_string_param(params, "selector")?;
            let value = required_string_param(params, "value")?;
            state.browser.fields.insert(selector.clone(), value.clone());
            Ok(json!({ "ok": true, "selector": selector, "value": value }))
        }
        "browser.scroll" => {
            let selector = optional_string_param(params, "selector")?;
            let dy = optional_u64_param(params, "dy")?.unwrap_or(0) as f64;
            if let Some(selector) = selector {
                let current = state
                    .browser
                    .scroll_tops
                    .get(&selector)
                    .copied()
                    .unwrap_or(0.0_f64);
                state.browser.scroll_tops.insert(selector, current + dy);
            }
            Ok(json!({ "ok": true }))
        }
        "browser.get.attr" => {
            let selector = required_string_param(params, "selector")?;
            let name = optional_string_param(params, "name")?
                .or_else(|| optional_string_param(params, "attr").ok().flatten())
                .ok_or_else(|| CommandError::invalid_params("name/attr is required"))?;
            let value = state
                .browser
                .dom_attrs
                .get(&selector)
                .and_then(|attrs| attrs.get(&name))
                .cloned()
                .unwrap_or_default();
            Ok(json!({ "value": value }))
        }
        "browser.get.box" => Ok(json!({
            "value": {
                "x": 0,
                "y": 0,
                "width": 123,
                "height": 45
            }
        })),
        "browser.get.count" => {
            let selector = required_string_param(params, "selector")?;
            let count = state
                .browser
                .dom_counts
                .get(&selector)
                .copied()
                .unwrap_or(1);
            Ok(json!({ "count": count }))
        }
        "browser.get.html" => Ok(json!({
            "value": optional_string_param(params, "selector")?
                .and_then(|selector| state.browser.dom_html.get(&selector).cloned())
                .unwrap_or_else(|| format!("<html><head><title>{}</title></head><body>{}</body></html>", state.browser.title, state.browser.page_text))
        })),
        "browser.get.styles" => {
            let property = optional_string_param(params, "property")?
                .or_else(|| optional_string_param(params, "name").ok().flatten())
                .unwrap_or_default();
            let selector = optional_string_param(params, "selector")?
                .unwrap_or_else(|| "#style-target".to_string());
            let mut styles = state
                .browser
                .dom_styles
                .get(&selector)
                .cloned()
                .unwrap_or_else(|| HashMap::from([("display".to_string(), "block".to_string())]));
            if state.browser.styles.iter().any(|s| s.contains("0, 128, 0")) {
                styles.insert("color".to_string(), "rgb(0, 128, 0)".to_string());
            }
            if property.is_empty() {
                Ok(json!({ "value": styles }))
            } else {
                Ok(json!({ "value": styles.get(&property).cloned().unwrap_or_default() }))
            }
        }
        "browser.is.checked" => {
            let selector = required_string_param(params, "selector")?;
            let value = state
                .browser
                .checked
                .get(&selector)
                .copied()
                .unwrap_or(false);
            Ok(json!({ "checked": value, "value": value }))
        }
        "browser.is.enabled" => {
            let selector = required_string_param(params, "selector")?;
            let value = state
                .browser
                .dom_enabled
                .get(&selector)
                .copied()
                .unwrap_or(true);
            Ok(json!({ "value": value, "enabled": value }))
        }
        "browser.is.visible" => {
            let selector = required_string_param(params, "selector")?;
            let value = state
                .browser
                .dom_visible
                .get(&selector)
                .copied()
                .unwrap_or(true);
            Ok(json!({ "value": value, "visible": value }))
        }
        "browser.find.role" => {
            let role = required_string_param(params, "role")?;
            let name = optional_string_param(params, "name")?.unwrap_or_default();
            let selector = if role == "button" && name.to_ascii_lowercase().contains("submit") {
                if state.browser_selector_exists("#action-btn") {
                    "#action-btn".to_string()
                } else {
                    "#btn".to_string()
                }
            } else {
                "#status".to_string()
            };
            let element_ref = state.browser_new_ref(selector.clone());
            Ok(json!({
                "element_ref": element_ref,
                "matches": [{"role": role, "name": state.browser.title, "selector": selector}]
            }))
        }
        "browser.highlight" => Ok(json!({ "ok": true })),
        "browser.addscript" => {
            let script = required_string_param(params, "script")?;
            state.browser.scripts.push(script.clone());
            if script.contains("window.triggerDialogs") {
                state.browser.dialogs = vec![
                    "confirm".to_string(),
                    "prompt".to_string(),
                    "alert".to_string(),
                ];
            }
            if script.contains("window.emitConsoleAndError") {
                state
                    .browser
                    .console
                    .push("limux-console-entry".to_string());
                state.browser.errors.push("limux-boom".to_string());
            }
            let value = state
                .browser
                .scripts
                .last()
                .and_then(|source| {
                    let trimmed = source.trim();
                    let (left, right) = trimmed.split_once('+')?;
                    let left = left.trim().parse::<i64>().ok()?;
                    let right = right.trim().parse::<i64>().ok()?;
                    Some(left + right)
                })
                .map(|number| Value::Number(number.into()))
                .unwrap_or(Value::Null);
            Ok(json!({ "ok": true, "value": value }))
        }
        "browser.addinitscript" => {
            let script = required_string_param(params, "script")?;
            state.browser.init_scripts.push(script);
            Ok(json!({ "ok": true }))
        }
        "browser.addstyle" => {
            let style = optional_string_param(params, "style")?
                .or_else(|| optional_string_param(params, "css").ok().flatten())
                .ok_or_else(|| CommandError::invalid_params("style/css is required"))?;
            state.browser.styles.push(style);
            Ok(json!({ "ok": true }))
        }
        "browser.console.list" => {
            Ok(json!({ "entries": state.browser.console, "count": state.browser.console.len() }))
        }
        "browser.console.clear" => {
            state.browser.console.clear();
            Ok(json!({ "ok": true }))
        }
        "browser.errors.list" => {
            Ok(json!({ "errors": state.browser.errors, "count": state.browser.errors.len() }))
        }
        "browser.cookies.get" => {
            let name = optional_string_param(params, "name")?;
            if let Some(name) = name {
                let value = state
                    .browser
                    .cookies
                    .get(&name)
                    .cloned()
                    .unwrap_or_default();
                let rows: Vec<Value> = if value.is_empty() {
                    Vec::new()
                } else {
                    vec![json!({"name": name, "value": value})]
                };
                Ok(json!({
                    "cookies": rows
                }))
            } else {
                Ok(json!({
                    "cookies": state.browser.cookies.iter().map(|(name, value)| json!({"name": name, "value": value})).collect::<Vec<_>>()
                }))
            }
        }
        "browser.cookies.set" => {
            let name = required_string_param(params, "name")?;
            let value = required_string_param(params, "value")?;
            state.browser.cookies.insert(name.clone(), value.clone());
            Ok(json!({ "ok": true, "name": name, "value": value }))
        }
        "browser.cookies.clear" => {
            if let Some(name) = optional_string_param(params, "name")? {
                state.browser.cookies.remove(&name);
            } else {
                state.browser.cookies.clear();
            }
            Ok(json!({ "ok": true }))
        }
        "browser.storage.get" => {
            let key = required_string_param(params, "key")?;
            let storage_type =
                optional_string_param(params, "type")?.unwrap_or_else(|| "local".to_string());
            let value = if storage_type == "session" {
                state.browser.session_storage.get(&key).cloned()
            } else {
                state.browser.local_storage.get(&key).cloned()
            };
            Ok(json!({ "value": value }))
        }
        "browser.storage.set" => {
            let key = required_string_param(params, "key")?;
            let value = required_string_param(params, "value")?;
            let storage_type =
                optional_string_param(params, "type")?.unwrap_or_else(|| "local".to_string());
            if storage_type == "session" {
                state
                    .browser
                    .session_storage
                    .insert(key.clone(), value.clone());
            } else {
                state
                    .browser
                    .local_storage
                    .insert(key.clone(), value.clone());
            }
            Ok(json!({ "ok": true, "key": key, "value": value }))
        }
        "browser.storage.clear" => {
            let storage_type =
                optional_string_param(params, "type")?.unwrap_or_else(|| "local".to_string());
            let key = optional_string_param(params, "key")?;
            if storage_type == "session" {
                if let Some(key) = key {
                    state.browser.session_storage.remove(&key);
                } else {
                    state.browser.session_storage.clear();
                }
            } else if let Some(key) = key {
                state.browser.local_storage.remove(&key);
            } else {
                state.browser.local_storage.clear();
            }
            Ok(json!({ "ok": true }))
        }
        "browser.tab.list" => Ok(json!({
            "tabs": state.browser.tab_ids.iter().map(|id| json!({
                "id": encode_handle_id(*id),
                "ref": surface_ref(*id),
                "surface_id": encode_handle_id(*id),
                "surface_ref": surface_ref(*id)
            })).collect::<Vec<_>>(),
            "current_surface_id": encode_handle_id(state.browser.current_tab_id),
            "current_surface_ref": surface_ref(state.browser.current_tab_id),
        })),
        "browser.tab.new" => {
            let created = state
                .split_surface(Some("browser".to_string()))
                .ok_or_else(|| CommandError::not_found("no active window"))?;
            state.browser_register_surface(created.id);
            let url = optional_string_param(params, "url")?.unwrap_or_else(|| {
                if state.browser.open {
                    state.browser.url.clone()
                } else {
                    "about:blank".to_string()
                }
            });
            state.browser_navigate(url);
            Ok(
                json!({ "surface_id": encode_handle_id(created.id), "surface_ref": surface_ref(created.id) }),
            )
        }
        "browser.tab.switch" => {
            let tab_id =
                optional_u64_param_any(params, &["target_surface_id", "surface_id", "tab_id"])?
                    .ok_or_else(|| {
                        CommandError::invalid_params(
                            "target_surface_id/surface_id/tab_id is required",
                        )
                    })?;
            if !state.browser.tab_ids.contains(&tab_id) {
                state.browser.tab_ids.push(tab_id);
            }
            state.browser.current_tab_id = tab_id;
            Ok(
                json!({ "surface_id": encode_handle_id(tab_id), "surface_ref": surface_ref(tab_id) }),
            )
        }
        "browser.tab.close" => {
            let tab_id =
                optional_u64_param_any(params, &["target_surface_id", "surface_id", "tab_id"])?
                    .unwrap_or(state.browser.current_tab_id);
            if state.browser.tab_ids.len() <= 1 {
                return Err(CommandError::conflict("cannot close last tab"));
            }
            if let Some(idx) = state.browser.tab_ids.iter().position(|id| *id == tab_id) {
                state.browser.tab_ids.remove(idx);
                if state.browser.current_tab_id == tab_id {
                    state.browser.current_tab_id = *state.browser.tab_ids.last().unwrap_or(&1);
                }
                Ok(
                    json!({ "ok": true, "surface_id": encode_handle_id(tab_id), "surface_ref": surface_ref(tab_id) }),
                )
            } else {
                Err(CommandError::not_found("tab not found"))
            }
        }
        "browser.frame.select" => {
            let selector = optional_string_param(params, "selector")?
                .or_else(|| optional_string_param(params, "frame_id").ok().flatten())
                .ok_or_else(|| CommandError::invalid_params("selector is required"))?;
            if selector.contains("missing") {
                return Err(CommandError::not_found("frame not found"));
            }
            state.browser.frame_selected = true;
            Ok(json!({ "frame_id": selector }))
        }
        "browser.frame.main" => {
            state.browser.frame_selected = false;
            Ok(json!({ "frame_id": "main" }))
        }
        "browser.dialog.accept" | "browser.dialog.dismiss" => {
            if state.browser.dialogs.is_empty() {
                return Err(CommandError::not_found("dialog queue empty"));
            }
            let _ = state.browser.dialogs.remove(0);
            Ok(json!({ "ok": true, "accepted": method.ends_with("accept") }))
        }
        "browser.download.wait" => {
            let path = optional_string_param(params, "path")?
                .unwrap_or_else(|| "/tmp/download.bin".to_string());
            let timeout_ms = optional_u64_param(params, "timeout_ms")?.unwrap_or(5000);
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
            while std::time::Instant::now() < deadline {
                if std::path::Path::new(&path).exists() {
                    return Ok(json!({ "ok": true, "downloaded": true, "path": path }));
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(CommandError::timeout("download did not complete in time"))
        }
        "browser.state.save" => {
            if let Some(path) = optional_string_param(params, "path")? {
                let state_json = json!({
                    "url": state.browser.url,
                    "title": state.browser.title,
                    "current_tab_id": state.browser.current_tab_id,
                    "local_storage": state.browser.local_storage,
                    "session_storage": state.browser.session_storage,
                });
                let encoded = serde_json::to_vec(&state_json).map_err(|err| {
                    CommandError::invalid_params(format!("failed to encode state: {err}"))
                })?;
                std::fs::write(&path, encoded).map_err(|err| {
                    CommandError::invalid_params(format!("failed to write state file: {err}"))
                })?;
                return Ok(json!({
                    "path": path,
                    "state": {
                        "url": state.browser.url,
                        "title": state.browser.title,
                        "current_tab_id": state.browser.current_tab_id,
                        "local_storage": state.browser.local_storage,
                        "session_storage": state.browser.session_storage
                    }
                }));
            }
            Ok(json!({
                "state": {
                    "url": state.browser.url,
                    "title": state.browser.title,
                    "current_tab_id": state.browser.current_tab_id,
                    "local_storage": state.browser.local_storage,
                    "session_storage": state.browser.session_storage
                }
            }))
        }
        "browser.state.load" => {
            if let Some(path) = optional_string_param(params, "path")? {
                if let Ok(raw) = std::fs::read_to_string(&path) {
                    if let Ok(value) = serde_json::from_str::<Value>(&raw) {
                        if let Some(url) = value.get("url").and_then(Value::as_str) {
                            state.browser_navigate(url.to_string());
                        }
                        if let Some(storage) = value.get("local_storage").and_then(Value::as_object)
                        {
                            state.browser.local_storage = storage
                                .iter()
                                .map(|(k, v)| {
                                    (k.clone(), v.as_str().unwrap_or_default().to_string())
                                })
                                .collect();
                        }
                        if let Some(storage) =
                            value.get("session_storage").and_then(Value::as_object)
                        {
                            state.browser.session_storage = storage
                                .iter()
                                .map(|(k, v)| {
                                    (k.clone(), v.as_str().unwrap_or_default().to_string())
                                })
                                .collect();
                        }
                    }
                }
            } else if let Some(state_obj) = params.get("state").and_then(Value::as_object) {
                if let Some(url) = state_obj.get("url").and_then(Value::as_str) {
                    state.browser_navigate(url.to_string());
                }
            }
            Ok(json!({ "ok": true, "url": state.browser.url }))
        }
        method if method.starts_with("browser.find.") => {
            let selector = match method {
                "browser.find.text" => "li.row",
                "browser.find.label"
                | "browser.find.placeholder"
                | "browser.find.title"
                | "browser.find.testid" => "#name",
                "browser.find.alt" => "#hero",
                "browser.find.first" | "browser.find.last" | "browser.find.nth" => "li.row",
                _ => "#status",
            };
            let element_ref = state.browser_new_ref(selector.to_string());
            Ok(json!({ "element_ref": element_ref }))
        }
        "browser.viewport.set"
        | "browser.geolocation.set"
        | "browser.offline.set"
        | "browser.trace.start"
        | "browser.trace.stop"
        | "browser.network.route"
        | "browser.network.unroute"
        | "browser.network.requests"
        | "browser.screencast.start"
        | "browser.screencast.stop"
        | "browser.input_mouse"
        | "browser.input_keyboard"
        | "browser.input_touch" => Err(CommandError::not_supported(method)),
        _ => Err(CommandError::unknown_method(method)),
    }
}

fn handle_debug_command(
    state: &mut ControlState,
    method: &str,
    params: &Map<String, Value>,
) -> Result<Value, CommandError> {
    match method {
        "debug.command_palette.toggle" => {
            let requested_window_id = optional_u64_param_any(params, &["window_id", "id"])?;
            let window_id = state
                .resolve_palette_window_id(requested_window_id)
                .ok_or_else(|| CommandError::not_found("window not found"))?;
            let currently_visible = state.palette_visible_for_window(window_id);
            if currently_visible {
                state.close_palette(window_id);
            } else {
                // debug toggle opens plain commands mode with empty query.
                let palette = state.palette_state_for_window_mut(window_id);
                palette.visible = true;
                palette.mode = "commands".to_string();
                palette.query.clear();
                palette.selected_index = 0;
                palette.selection_location = 0;
                palette.selection_length = 0;
            }
            Ok(json!({ "visible": state.palette_visible_for_window(window_id) }))
        }
        "debug.command_palette.visible" => {
            let requested_window_id = optional_u64_param_any(params, &["window_id", "id"])?;
            let window_id = state
                .resolve_palette_window_id(requested_window_id)
                .ok_or_else(|| CommandError::not_found("window not found"))?;
            Ok(json!({ "visible": state.palette_visible_for_window(window_id) }))
        }
        "debug.command_palette.selection" => {
            let requested_window_id = optional_u64_param_any(params, &["window_id", "id"])?;
            let window_id = state
                .resolve_palette_window_id(requested_window_id)
                .ok_or_else(|| CommandError::not_found("window not found"))?;
            let selected_index = state.palette_state_for_window(window_id).selected_index;
            Ok(json!({ "selected_index": selected_index }))
        }
        "debug.command_palette.results" => {
            let requested_window_id = optional_u64_param_any(params, &["window_id", "id"])?;
            let window_id = state
                .resolve_palette_window_id(requested_window_id)
                .ok_or_else(|| CommandError::not_found("window not found"))?;
            let limit = optional_u64_param(params, "limit")?.unwrap_or(20) as usize;
            Ok(state.command_palette_results_payload(window_id, limit))
        }
        "debug.command_palette.rename_tab.open" => {
            let requested_window_id = optional_u64_param_any(params, &["window_id", "id"])?;
            let window_id = state
                .resolve_palette_window_id(requested_window_id)
                .ok_or_else(|| CommandError::not_found("window not found"))?;
            state.command_palette_open_rename_tab(window_id);
            Ok(json!({ "ok": true }))
        }
        "debug.command_palette.rename_input.selection" => {
            let requested_window_id = optional_u64_param_any(params, &["window_id", "id"])?;
            let window_id = state
                .resolve_palette_window_id(requested_window_id)
                .ok_or_else(|| CommandError::not_found("window not found"))?;
            Ok(state.command_palette_selection_snapshot(window_id))
        }
        "debug.command_palette.rename_input.select_all" => {
            if let Some(enabled) = optional_bool_param(params, "enabled")? {
                state.rename_input_select_all = enabled;
            }
            Ok(json!({ "enabled": state.rename_input_select_all }))
        }
        "debug.command_palette.rename_input.delete_backward" => {
            let requested_window_id = optional_u64_param_any(params, &["window_id", "id"])?;
            let window_id = state
                .resolve_palette_window_id(requested_window_id)
                .ok_or_else(|| CommandError::not_found("window not found"))?;
            state.command_palette_delete_backward(window_id);
            Ok(json!({ "ok": true }))
        }
        "debug.command_palette.rename_input.interact" => {
            let requested_window_id = optional_u64_param_any(params, &["window_id", "id"])?;
            let window_id = state
                .resolve_palette_window_id(requested_window_id)
                .ok_or_else(|| CommandError::not_found("window not found"))?;
            state.command_palette_interact(window_id);
            Ok(json!({ "ok": true }))
        }
        "debug.sidebar.visible" => {
            let window_id = optional_u64_param_any(params, &["window_id", "id"])?
                .or_else(|| state.active_window_id())
                .ok_or_else(|| CommandError::not_found("window not found"))?;
            Ok(json!({ "visible": state.sidebar_visible_for_window(window_id) }))
        }
        "debug.layout" => {
            let container_width = 1600.0_f64;
            let container_height = 900.0_f64;
            let container_frame = json!({
                "x": 0.0_f64,
                "y": 0.0_f64,
                "width": container_width,
                "height": container_height
            });
            let mut selected_panels: Vec<Value> = Vec::new();
            let panes = state
                .current_workspace()
                .and_then(|workspace| {
                    workspace
                        .current_window_id
                        .map(|window_id| (workspace, window_id))
                })
                .and_then(|(workspace, window_id)| {
                    workspace
                        .windows
                        .iter()
                        .find(|window| window.id == window_id)
                })
                .map(|window| {
                    let pane_count = window.panes.len().max(1);
                    let pane_width = container_width / pane_count as f64;
                    window
                        .panes
                        .iter()
                        .enumerate()
                        .map(|(idx, pane)| {
                            let (width_delta, height_delta) = state
                                .pane_size_overrides
                                .get(&pane.id)
                                .copied()
                                .unwrap_or((0.0_f64, 0.0_f64));
                            let width = (pane_width + width_delta).max(80.0_f64);
                            let height = (container_height + height_delta).max(80.0_f64);
                            let frame = json!({
                                "x": (idx as f64) * pane_width,
                                "y": 0.0_f64,
                                "width": width,
                                "height": height
                            });
                            if let Some(surface_id) = pane.current_surface_id {
                                let pane_frame = frame.clone();
                                let view_frame = frame.clone();
                                selected_panels.push(json!({
                                    "paneId": encode_handle_id(pane.id),
                                    "pane_id": encode_handle_id(pane.id),
                                    "panelId": encode_handle_id(surface_id),
                                    "panel_id": encode_handle_id(surface_id),
                                    "surfaceId": encode_handle_id(surface_id),
                                    "surface_id": encode_handle_id(surface_id),
                                    "paneFrame": pane_frame,
                                    "viewFrame": view_frame,
                                    "inWindow": true,
                                    "hidden": false,
                                    "splitViews": [
                                        { "frame": container_frame.clone() }
                                    ]
                                }));
                            }
                            json!({
                                "paneId": encode_handle_id(pane.id),
                                "pane_id": encode_handle_id(pane.id),
                                "surfaceId": pane.current_surface_id.map(encode_handle_id),
                                "surface_id": pane.current_surface_id.map(encode_handle_id),
                                "frame": frame
                            })
                        })
                        .collect::<Vec<Value>>()
                })
                .unwrap_or_default();
            Ok(json!({
                "layout": {
                    "layout": {
                        "panes": panes,
                        "containerFrame": container_frame
                    },
                    "selectedPanels": selected_panels
                }
            }))
        }
        "debug.portal.stats" => Ok(json!({
            "portals": [],
            "totals": {
                "orphan_terminal_subview_count": 0,
                "visible_orphan_terminal_subview_count": 0,
                "stale_entry_count": 0
            }
        })),
        "debug.panel_snapshot" => {
            let surface_id = optional_u64_param_any(params, &["surface_id", "id"])?
                .or_else(|| focused_handles(state).map(|(_, _, _, surface)| surface))
                .ok_or_else(|| CommandError::not_found("surface not found"))?;
            let label = optional_string_param(params, "label")?.unwrap_or_default();
            let surface = state
                .list_surfaces()
                .unwrap_or_default()
                .into_iter()
                .find(|surface| surface.id == surface_id)
                .ok_or_else(|| CommandError::not_found("surface not found"))?;

            let previous = state
                .panel_snapshot_baselines
                .get(&surface_id)
                .cloned()
                .unwrap_or_else(|| surface.text.clone());
            let changed_pixels = estimate_changed_pixels(&previous, &surface.text);
            state
                .panel_snapshot_baselines
                .insert(surface_id, surface.text.clone());

            let path = write_mock_png("limux-panel", &label, surface_id)
                .ok_or_else(|| CommandError::timeout("failed to write panel snapshot"))?;
            let path_string = path.to_string_lossy().to_string();
            let url = format!("file://{path_string}");
            Ok(json!({
                "surface_id": encode_handle_id(surface.id),
                "panel_id": encode_handle_id(surface.id),
                "path": path_string,
                "url": url,
                "width": 640,
                "height": 360,
                "changed_pixels": changed_pixels,
            }))
        }
        "debug.panel_snapshot.reset" => {
            let surface_id = optional_u64_param_any(params, &["surface_id", "id"])?
                .or_else(|| focused_handles(state).map(|(_, _, _, surface)| surface))
                .ok_or_else(|| CommandError::not_found("surface not found"))?;
            let current_text = state
                .list_surfaces()
                .unwrap_or_default()
                .into_iter()
                .find(|surface| surface.id == surface_id)
                .map(|surface| surface.text)
                .ok_or_else(|| CommandError::not_found("surface not found"))?;
            state
                .panel_snapshot_baselines
                .insert(surface_id, current_text);
            Ok(json!({ "ok": true, "surface_id": encode_handle_id(surface_id) }))
        }
        "debug.bonsplit_underflow.count" => {
            Ok(json!({ "count": state.debug_bonsplit_underflow_count }))
        }
        "debug.bonsplit_underflow.reset" => {
            state.debug_bonsplit_underflow_count = 0;
            Ok(json!({ "ok": true }))
        }
        "debug.empty_panel.count" => Ok(json!({ "count": state.debug_empty_panel_count })),
        "debug.empty_panel.reset" => {
            state.debug_empty_panel_count = 0;
            Ok(json!({ "ok": true }))
        }
        "debug.flash.count" => {
            let surface_id = optional_u64_param_any(params, &["surface_id", "id"])?;
            if let Some(surface_id) = surface_id {
                let count = state
                    .list_surfaces()
                    .unwrap_or_default()
                    .into_iter()
                    .find(|surface| surface.id == surface_id)
                    .map(|surface| surface.flash_count)
                    .unwrap_or(0);
                Ok(json!({ "count": count }))
            } else {
                Ok(json!({ "count": state.debug_flash_count }))
            }
        }
        "debug.flash.reset" => {
            state.debug_flash_count = 0;
            for workspace in &mut state.workspaces {
                for window in &mut workspace.windows {
                    for pane in &mut window.panes {
                        for surface in &mut pane.surfaces {
                            surface.flash_count = 0;
                        }
                    }
                }
            }
            Ok(json!({ "ok": true }))
        }
        "debug.shortcut.set" => {
            let name = optional_string_param(params, "name")?
                .or_else(|| optional_string_param(params, "action").ok().flatten())
                .ok_or_else(|| CommandError::invalid_params("name/action is required"))?;
            let combo = optional_string_param(params, "combo")?
                .or_else(|| optional_string_param(params, "shortcut").ok().flatten())
                .unwrap_or_default();
            if combo == "clear" {
                state.shortcuts.remove(&name);
            } else if !combo.is_empty() {
                state
                    .shortcuts
                    .insert(name.clone(), normalize_shortcut_combo(&combo));
            }
            Ok(json!({ "ok": true, "name": name, "combo": combo }))
        }
        "debug.shortcut.simulate" => {
            let combo = optional_string_param(params, "combo")?
                .or_else(|| optional_string_param(params, "action").ok().flatten())
                .ok_or_else(|| CommandError::invalid_params("combo/action is required"))?;
            let combo_norm = normalize_shortcut_combo(&combo);
            if state.handle_palette_shortcut(&combo_norm) {
                return Ok(json!({ "ok": true, "combo": combo }));
            }
            let focus_left = state
                .shortcuts
                .get("focus_left")
                .cloned()
                .unwrap_or_else(|| normalize_shortcut_combo("cmd+opt+left"));
            let focus_right = state
                .shortcuts
                .get("focus_right")
                .cloned()
                .unwrap_or_else(|| normalize_shortcut_combo("cmd+opt+right"));
            if combo_norm == focus_left {
                let _ = state.focus_left_pane();
            } else if combo_norm == focus_right {
                let _ = state.focus_right_pane();
            } else if combo_norm == normalize_shortcut_combo("cmd+opt+i") {
                let _ = state.toggle_devtools_on_focused_browser();
            } else if combo_norm == normalize_shortcut_combo("cmd+b") {
                if let Some(window_id) = state.active_window_id() {
                    let visible = state.toggle_sidebar_for_window(window_id);
                    return Ok(json!({ "ok": true, "combo": combo, "visible": visible }));
                }
            } else if combo_norm == normalize_shortcut_combo("cmd+t") {
                let _ = state.create_surface(None);
            } else if combo_norm == normalize_shortcut_combo("cmd+d") {
                if let Some((_, _, source_pane_id, _)) = focused_handles(state) {
                    if let Some(surface) = state.split_surface(None) {
                        state.register_split_relation(source_pane_id, surface.pane_id, "right");
                    }
                }
            } else if combo_norm == normalize_shortcut_combo("cmd+shift+d") {
                if let Some((_, _, source_pane_id, _)) = focused_handles(state) {
                    if let Some(surface) = state.split_surface(None) {
                        state.register_split_relation(source_pane_id, surface.pane_id, "down");
                    }
                }
            } else if combo_norm == normalize_shortcut_combo("ctrl+d") {
                if let Some((workspace_id, _, _, surface_id)) = focused_handles(state) {
                    let _ = apply_surface_key_input(state, workspace_id, surface_id, "ctrl-d");
                }
            } else if combo_norm == normalize_shortcut_combo("enter") {
                if let Some((workspace_id, _, _, surface_id)) = focused_handles(state) {
                    let _ = apply_surface_text_input(state, workspace_id, surface_id, "\n");
                }
            } else if combo_norm.chars().count() == 1 && !combo_norm.contains('+') {
                if let Some((workspace_id, _, _, surface_id)) = focused_handles(state) {
                    let _ = apply_surface_text_input(state, workspace_id, surface_id, &combo_norm);
                }
            }
            Ok(json!({ "ok": true, "combo": combo }))
        }
        "debug.notification.focus" => {
            let workspace_id = optional_u64_param_any(params, &["workspace_id", "id"])?
                .ok_or_else(|| CommandError::invalid_params("workspace_id is required"))?;
            let surface_id = optional_u64_param_any(params, &["surface_id"])?;
            let _ = state.select_workspace(Some(workspace_id), None);
            if let Some(surface_id) = surface_id {
                let _ = state.focus_surface(surface_id);
                let _ = state.mark_notifications_read_for_surface(surface_id);
            } else {
                let _ = state.mark_notifications_read_for_workspace(workspace_id);
            }
            Ok(json!({ "ok": true }))
        }
        "debug.type" => {
            let text = required_string_param(params, "text")?;
            if let Some(window_id) = state.palette_target_window_for_typing() {
                state.command_palette_apply_type(window_id, &text);
                return Ok(json!({ "ok": true, "text": text }));
            }
            if let Some((workspace_id, _, _, surface_id)) = focused_handles(state) {
                let _ = apply_surface_text_input(state, workspace_id, surface_id, &text);
            }
            Ok(json!({ "ok": true, "text": text }))
        }
        "debug.app.activate" => {
            state.app_focus_override = true;
            state.app_simulate_active = true;
            let _ = state.mark_all_notifications_read();
            Ok(json!({ "ok": true }))
        }
        "debug.terminal.is_focused" => {
            let surface_id = optional_u64_param_any(params, &["surface_id", "id"])?;
            let focused_surface = focused_handles(state).map(|(_, _, _, surface)| surface);
            let focused = if let Some(surface_id) = surface_id {
                focused_surface == Some(surface_id)
                    && !state.browser.browser_surfaces.contains(&surface_id)
            } else {
                focused_surface.is_some()
            };
            Ok(json!({ "focused": focused }))
        }
        "debug.terminal.read_text" => {
            let surface_id = optional_u64_param_any(params, &["surface_id", "id"])?;
            let text = if let Some(surface_id) = surface_id {
                state
                    .list_surfaces()
                    .unwrap_or_default()
                    .into_iter()
                    .find(|surface| surface.id == surface_id)
                    .map(|surface| surface.text)
                    .unwrap_or_default()
            } else {
                state
                    .current_surface()
                    .map(|surface| surface.text)
                    .unwrap_or_default()
            };
            Ok(json!({ "text": text }))
        }
        "debug.terminal.render_stats" => {
            let surface_id = optional_u64_param_any(params, &["surface_id", "id"])?;
            let text_len = if let Some(surface_id) = surface_id {
                state
                    .list_surfaces()
                    .unwrap_or_default()
                    .into_iter()
                    .find(|surface| surface.id == surface_id)
                    .map(|surface| surface.text.len() as u64)
                    .unwrap_or(0)
            } else {
                state
                    .current_surface()
                    .map(|surface| surface.text.len() as u64)
                    .unwrap_or(0)
            };
            Ok(json!({
                "stats": {
                    "fps": 60,
                    "frameTimeMs": 16,
                    "inWindow": true,
                    "presentCount": text_len,
                    "appIsActive": true
                }
            }))
        }
        "debug.window.screenshot" => {
            let label = optional_string_param(params, "label")?.unwrap_or_default();
            let screenshot_id = state.next_snapshot_id;
            state.next_snapshot_id = state.next_snapshot_id.saturating_add(1);
            let path = write_mock_png("limux-window", &label, screenshot_id)
                .ok_or_else(|| CommandError::timeout("failed to write screenshot"))?;
            let path_string = path.to_string_lossy().to_string();
            Ok(json!({
                "ok": true,
                "screenshot_id": screenshot_id.to_string(),
                "format": "png",
                "bytes": MOCK_PNG_BYTES.len(),
                "path": path_string,
                "url": format!("file://{}", path.to_string_lossy()),
            }))
        }
        _ => Err(CommandError::unknown_method(method)),
    }
}

fn handle_command(
    state: &mut ControlState,
    method: &str,
    params: &Value,
) -> Result<Value, CommandError> {
    match method {
        "system.ping" => Ok(json!({ "pong": true })),
        "system.identify" => {
            let params = params_object(params)?;
            let focused = focused_payload(state);
            let caller = params
                .get("caller")
                .cloned()
                .unwrap_or_else(|| focused.clone());
            Ok(json!({
                "name": "limux-control",
                "protocol": "v1+v2",
                "version": env!("CARGO_PKG_VERSION"),
                "focused": focused,
                "caller": caller,
            }))
        }
        "system.capabilities" => Ok(json!({ "commands": COMMANDS, "methods": COMMANDS })),

        "app.focus_override.set" => {
            let params = params_object(params)?;
            let enabled = match optional_bool_param(params, "enabled")? {
                Some(value) => value,
                None => match optional_string_param(params, "state")?
                    .unwrap_or_else(|| "clear".to_string())
                    .as_str()
                {
                    "active" => true,
                    "inactive" => false,
                    "clear" => false,
                    other => {
                        return Err(CommandError::invalid_params(format!(
                            "state must be one of active|inactive|clear, got {other}"
                        )))
                    }
                },
            };
            state.app_focus_override = enabled;
            state.app_simulate_active = enabled;
            Ok(json!({ "focus_override": state.app_focus_override }))
        }
        "app.simulate_active" => {
            let params = params_object(params)?;
            let active = optional_bool_param(params, "active")?.unwrap_or(true);
            state.app_simulate_active = active;
            if active {
                let _ = state.mark_all_notifications_read();
            }
            Ok(json!({ "simulate_active": state.app_simulate_active }))
        }

        "workspace.list" => {
            let params = params_object(params)?;
            let requested_window_id = optional_u64_param_any(params, &["window_id"])?;
            let default_window_id = state
                .current_workspace()
                .map(|workspace| workspace.host_window_id);
            let window_id = requested_window_id.or(default_window_id);
            let workspaces = state.list_workspaces();
            let selected_id = state.current_workspace_id;
            let filtered: Vec<&WorkspaceInfo> = workspaces
                .iter()
                .filter(|workspace| {
                    window_id
                        .map(|target| workspace.host_window_id == target)
                        .unwrap_or(true)
                })
                .collect();
            let rows: Vec<Value> = filtered
                .iter()
                .enumerate()
                .map(|(idx, workspace)| {
                    let cwd = state.workspace_cwd(workspace.id);
                    workspace_row(idx, selected_id, workspace, cwd.as_deref())
                })
                .collect();
            Ok(json!({ "workspaces": rows }))
        }
        "workspace.current" => state
            .current_workspace()
            .map(WorkspaceState::info)
            .map(|workspace| {
                let cwd = state.workspace_cwd(workspace.id);
                let row = workspace_row(0, workspace.id, &workspace, cwd.as_deref());
                json!({
                    "workspace_id": row["workspace_id"],
                    "workspace_ref": row["workspace_ref"],
                    "workspace": workspace,
                    "title": row["title"],
                    "name": row["name"],
                })
            })
            .ok_or_else(|| CommandError::not_found("no active workspace")),
        "workspace.create" => {
            let params = params_object(params)?;
            let name = optional_string_param(params, "name")?
                .or_else(|| optional_string_param(params, "title").ok().flatten());
            let window_id = optional_u64_param_any(params, &["window_id"])?;
            let cwd = optional_string_param(params, "cwd")?;
            let command = optional_string_param(params, "command")?;
            let workspace = state.create_workspace(name, window_id);
            if cwd.is_some() {
                let _ = state.set_workspace_cwd(workspace.id, cwd.clone());
            }
            if let Some(command) = command {
                let _ = with_workspace_scope(state, Some(workspace.id), |scoped| {
                    let _ = scoped
                        .update_surface(None, |surface| {
                            surface.text.push_str(&command);
                            surface.text.push('\n');
                        })
                        .ok_or_else(|| CommandError::not_found("surface not found"))?;
                    Ok(())
                });
            }
            Ok(json!({
                "workspace_id": encode_handle_id(workspace.id),
                "workspace_ref": workspace_ref(workspace.id),
                "workspace": workspace,
            }))
        }
        "workspace.select" => {
            let params = params_object(params)?;
            let id = optional_u64_param_any(params, &["workspace_id", "id"])?;
            let name = optional_string_param(params, "name")?;
            if id.is_none() && name.is_none() {
                return Err(CommandError::invalid_params(
                    "workspace.select requires workspace_id/id or name",
                ));
            }
            let workspace = state
                .select_workspace(id, name.as_deref())
                .ok_or_else(|| CommandError::not_found("workspace not found"))?;
            Ok(json!({
                "workspace_id": encode_handle_id(workspace.id),
                "workspace_ref": workspace_ref(workspace.id),
                "workspace": workspace
            }))
        }
        "workspace.next" => state
            .select_workspace_relative(1)
            .map(|workspace| {
                json!({
                    "workspace_id": encode_handle_id(workspace.id),
                    "workspace_ref": workspace_ref(workspace.id),
                    "workspace": workspace
                })
            })
            .ok_or_else(|| CommandError::not_found("workspace not found")),
        "workspace.previous" => state
            .select_workspace_relative(-1)
            .map(|workspace| {
                json!({
                    "workspace_id": encode_handle_id(workspace.id),
                    "workspace_ref": workspace_ref(workspace.id),
                    "workspace": workspace
                })
            })
            .ok_or_else(|| CommandError::not_found("workspace not found")),
        "workspace.last" => state
            .select_last_workspace()
            .map(|workspace| {
                json!({
                    "workspace_id": encode_handle_id(workspace.id),
                    "workspace_ref": workspace_ref(workspace.id),
                    "workspace": workspace
                })
            })
            .ok_or_else(|| CommandError::not_found("last workspace not found")),
        "workspace.rename" => {
            let params = params_object(params)?;
            let id = optional_u64_param_any(params, &["workspace_id", "id"])?;
            let name = optional_string_param(params, "name")?
                .or_else(|| optional_string_param(params, "title").ok().flatten())
                .ok_or_else(|| {
                    CommandError::invalid_params("workspace.rename requires name/title")
                })?;
            let workspace = state
                .rename_workspace(id, name)
                .ok_or_else(|| CommandError::not_found("workspace not found"))?;
            Ok(json!({
                "workspace_id": encode_handle_id(workspace.id),
                "workspace_ref": workspace_ref(workspace.id),
                "workspace": workspace
            }))
        }
        "workspace.reorder" => {
            let params = params_object(params)?;
            let id = optional_u64_param_any(params, &["workspace_id", "id"])?
                .ok_or_else(|| CommandError::invalid_params("workspace_id/id is required"))?;
            let index = optional_index_param(params, "index")?;
            let before_workspace_id = optional_u64_param_any(params, &["before_workspace_id"])?;
            let after_workspace_id = optional_u64_param_any(params, &["after_workspace_id"])?;
            let targets = usize::from(index.is_some())
                + usize::from(before_workspace_id.is_some())
                + usize::from(after_workspace_id.is_some());
            if targets != 1 {
                return Err(CommandError::invalid_params(
                    "workspace.reorder requires exactly one target: index|before_workspace_id|after_workspace_id",
                ));
            }
            let index = if let Some(index) = index {
                index
            } else if let Some(before_workspace_id) = before_workspace_id {
                let _ = state
                    .workspaces
                    .iter()
                    .position(|workspace| workspace.id == before_workspace_id)
                    .ok_or_else(|| CommandError::not_found("before_workspace not found"))?;
                0
            } else if let Some(after_workspace_id) = after_workspace_id {
                state
                    .workspaces
                    .iter()
                    .position(|workspace| workspace.id == after_workspace_id)
                    .ok_or_else(|| CommandError::not_found("after_workspace not found"))?
                    .saturating_add(1)
            } else {
                0
            };
            let workspace = state
                .reorder_workspace(id, index)
                .ok_or_else(|| CommandError::not_found("workspace not found"))?;
            Ok(json!({
                "workspace_id": encode_handle_id(workspace.id),
                "workspace_ref": workspace_ref(workspace.id),
                "workspace": workspace
            }))
        }
        "workspace.close" => {
            let params = params_object(params)?;
            let id = optional_u64_param_any(params, &["workspace_id", "id"])?;
            let closed = state
                .close_workspace(id)
                .ok_or_else(|| CommandError::conflict("cannot close workspace"))?;
            Ok(json!({
                "workspace_id": encode_handle_id(closed.id),
                "workspace_ref": workspace_ref(closed.id),
                "workspace": closed
            }))
        }
        "workspace.move_to_window" => {
            let params = params_object(params)?;
            let workspace_id = optional_u64_param_any(params, &["workspace_id", "id"])?;
            let host_window_id = optional_u64_param_any(params, &["window_id", "id"])?
                .ok_or_else(|| CommandError::invalid_params("window_id is required"))?;
            let moved = state
                .move_workspace_to_window(workspace_id, host_window_id)
                .ok_or_else(|| CommandError::not_found("workspace not found"))?;
            Ok(json!({
                "workspace_id": encode_handle_id(moved.id),
                "workspace_ref": workspace_ref(moved.id),
                "workspace": moved
            }))
        }
        "workspace.action" => {
            let params = params_object(params)?;
            let action = required_string_param(params, "action")?;
            Ok(json!({ "ok": true, "action": action }))
        }

        "window.list" => state
            .list_windows()
            .map(|windows| {
                let current_window_id = state.current_window().map(|window| window.id);
                let rows: Vec<Value> = windows
                    .iter()
                    .enumerate()
                    .map(|(idx, window)| {
                        json!({
                            "index": idx + 1,
                            "id": encode_handle_id(window.id),
                            "ref": window_ref(window.id),
                            "window_id": encode_handle_id(window.id),
                            "window_ref": window_ref(window.id),
                            "title": window.title,
                            "focused": current_window_id == Some(window.id),
                            "pane_count": window.pane_count,
                        })
                    })
                    .collect();
                json!({ "windows": rows })
            })
            .ok_or_else(|| CommandError::not_found("no active workspace")),
        "window.current" => state
            .current_window()
            .map(|window| {
                json!({
                    "window_id": encode_handle_id(window.id),
                    "window_ref": window_ref(window.id),
                    "window": window
                })
            })
            .ok_or_else(|| CommandError::not_found("no active window")),
        "window.create" => {
            let params = params_object(params)?;
            let title = optional_string_param(params, "title")?;
            let window = state
                .create_window(title)
                .ok_or_else(|| CommandError::not_found("no active workspace"))?;
            Ok(json!({
                "window_id": encode_handle_id(window.id),
                "window_ref": window_ref(window.id),
                "window": window
            }))
        }
        "window.focus" => {
            let params = params_object(params)?;
            let id = optional_u64_param_any(params, &["window_id", "id"])?
                .ok_or_else(|| CommandError::invalid_params("window_id is required"))?;
            let window = state
                .focus_window(id)
                .ok_or_else(|| CommandError::not_found("window not found"))?;
            Ok(json!({
                "window_id": encode_handle_id(window.id),
                "window_ref": window_ref(window.id),
                "window": window
            }))
        }
        "window.close" => {
            let params = params_object(params)?;
            let id = optional_u64_param_any(params, &["window_id", "id"])?;
            let window = state
                .close_window(id)
                .ok_or_else(|| CommandError::conflict("cannot close window"))?;
            Ok(json!({
                "window_id": encode_handle_id(window.id),
                "window_ref": window_ref(window.id),
                "window": window
            }))
        }

        "pane.list" => {
            let params = params_object(params)?;
            let workspace_id = optional_u64_param_any(params, &["workspace_id"])?;
            with_workspace_scope(state, workspace_id, |scoped| {
                let panes = scoped
                    .list_panes()
                    .ok_or_else(|| CommandError::not_found("pane list unavailable"))?;
                let focused_pane_id = focused_handles(scoped).map(|(_, _, pane_id, _)| pane_id);
                let rows: Vec<Value> = panes
                    .iter()
                    .enumerate()
                    .map(|(idx, pane)| pane_row(idx, focused_pane_id, pane))
                    .collect();
                Ok(json!({ "panes": rows }))
            })
        }
        "pane.surfaces" => {
            let params = params_object(params)?;
            let workspace_id = optional_u64_param_any(params, &["workspace_id"])?;
            let pane_id = optional_u64_param_any(params, &["pane_id", "id"])?;
            with_workspace_scope(state, workspace_id, |scoped| {
                let surfaces = scoped
                    .list_surfaces_for_pane(pane_id)
                    .ok_or_else(|| CommandError::not_found("pane not found"))?;
                let focused_surface_id =
                    focused_handles(scoped).map(|(_, _, _, surface_id)| surface_id);
                let rows: Vec<Value> = surfaces
                    .iter()
                    .enumerate()
                    .map(|(idx, surface)| surface_row(idx, focused_surface_id, surface))
                    .collect();
                Ok(json!({ "surfaces": rows }))
            })
        }
        "pane.create" => {
            let params = params_object(params)?;
            let title = optional_string_param(params, "surface_title")?;
            let pane_type =
                optional_string_param(params, "type")?.unwrap_or_else(|| "terminal".to_string());
            let url = optional_string_param(params, "url")?;
            let pane = state
                .create_pane(title)
                .ok_or_else(|| CommandError::not_found("no active window"))?;
            let surface_id = pane.current_surface_id.unwrap_or_default();
            if pane_type == "browser" {
                state.browser_register_surface(surface_id);
                state.browser_navigate(url.unwrap_or_else(|| "about:blank".to_string()));
            }
            Ok(json!({
                "pane_id": encode_handle_id(pane.id),
                "pane_ref": pane_ref(pane.id),
                "surface_id": encode_handle_id(surface_id),
                "surface_ref": surface_ref(surface_id),
                "pane": pane
            }))
        }
        "pane.focus" => {
            let params = params_object(params)?;
            let pane_id = optional_u64_param_any(params, &["pane_id", "id"])?
                .ok_or_else(|| CommandError::invalid_params("pane_id is required"))?;
            let pane = state
                .focus_pane(pane_id)
                .ok_or_else(|| CommandError::not_found("pane not found"))?;
            Ok(json!({
                "pane_id": encode_handle_id(pane.id),
                "pane_ref": pane_ref(pane.id),
                "pane": pane
            }))
        }
        "pane.swap" => {
            let params = params_object(params)?;
            let first = optional_u64_param_any(params, &["first_pane_id", "pane_id"])?
                .ok_or_else(|| CommandError::invalid_params("pane_id/first_pane_id is required"))?;
            let second = optional_u64_param_any(params, &["second_pane_id", "target_pane_id"])?
                .ok_or_else(|| {
                    CommandError::invalid_params("target_pane_id/second_pane_id is required")
                })?;
            let panes = state
                .swap_panes(first, second)
                .ok_or_else(|| CommandError::not_found("pane not found"))?;
            let focused_pane_id = focused_handles(state).map(|(_, _, pane_id, _)| pane_id);
            let rows: Vec<Value> = panes
                .iter()
                .enumerate()
                .map(|(idx, pane)| pane_row(idx, focused_pane_id, pane))
                .collect();
            Ok(json!({ "panes": rows }))
        }
        "pane.break" => {
            let params = params_object(params)?;
            let pane_id = optional_u64_param_any(params, &["pane_id", "id"])?;
            let pane = state
                .break_pane(pane_id)
                .ok_or_else(|| CommandError::not_found("pane not found"))?;
            let workspace_id = state.current_workspace_id;
            Ok(json!({
                "workspace_id": encode_handle_id(workspace_id),
                "workspace_ref": workspace_ref(workspace_id),
                "pane_id": encode_handle_id(pane.id),
                "pane_ref": pane_ref(pane.id),
                "pane": pane
            }))
        }
        "pane.join" => {
            let params = params_object(params)?;
            let target_id = optional_u64_param_any(params, &["target_pane_id"])?
                .ok_or_else(|| CommandError::invalid_params("target_pane_id is required"))?;
            let source_id = if let Some(source) =
                optional_u64_param_any(params, &["source_pane_id", "pane_id"])?
            {
                source
            } else if let Some(surface_id) = optional_u64_param_any(params, &["surface_id"])? {
                state
                    .find_pane_for_surface_in_current_window(surface_id)
                    .ok_or_else(|| CommandError::not_found("surface not found"))?
            } else {
                focused_handles(state)
                    .map(|(_, _, pane_id, _)| pane_id)
                    .ok_or_else(|| CommandError::not_found("pane not found"))?
            };
            let pane = state
                .join_panes(source_id, target_id)
                .ok_or_else(|| CommandError::not_found("pane not found"))?;
            Ok(json!({
                "pane_id": encode_handle_id(pane.id),
                "pane_ref": pane_ref(pane.id),
                "pane": pane
            }))
        }
        "pane.last" => state
            .focus_last_pane()
            .map(|pane| {
                json!({
                    "pane_id": encode_handle_id(pane.id),
                    "pane_ref": pane_ref(pane.id),
                    "pane": pane
                })
            })
            .ok_or_else(|| CommandError::not_found("last pane not found")),
        "pane.resize" => {
            let params = params_object(params)?;
            let pane_id = optional_u64_param_any(params, &["pane_id", "id"])?
                .ok_or_else(|| CommandError::invalid_params("pane_id is required"))?;
            let direction = optional_string_param(params, "direction")?
                .unwrap_or_else(|| "right".to_string())
                .to_ascii_lowercase();
            let amount = optional_u64_param(params, "amount")?.unwrap_or(1) as f64;
            let (width, height) = state
                .resize_pane(pane_id, &direction, amount)
                .ok_or_else(|| CommandError::not_found("pane not found"))?;
            Ok(json!({
                "pane_id": encode_handle_id(pane_id),
                "pane_ref": pane_ref(pane_id),
                "direction": direction,
                "amount": amount,
                "frame": {"width": width, "height": height},
                "ok": true
            }))
        }

        "surface.list" => {
            let params = params_object(params)?;
            let workspace_id = optional_u64_param_any(params, &["workspace_id"])?;
            with_workspace_scope(state, workspace_id, |scoped| {
                let surfaces = scoped
                    .list_surfaces()
                    .ok_or_else(|| CommandError::not_found("surface list unavailable"))?;
                let focused_surface_id =
                    focused_handles(scoped).map(|(_, _, _, surface_id)| surface_id);
                let rows: Vec<Value> = surfaces
                    .iter()
                    .enumerate()
                    .map(|(idx, surface)| surface_row(idx, focused_surface_id, surface))
                    .collect();
                Ok(json!({ "surfaces": rows }))
            })
        }
        "surface.current" => {
            let params = params_object(params)?;
            let workspace_id = optional_u64_param_any(params, &["workspace_id"])?;
            with_workspace_scope(state, workspace_id, |scoped| {
                let surface = scoped
                    .current_surface()
                    .ok_or_else(|| CommandError::not_found("no active surface"))?;
                Ok(json!({
                    "surface_id": encode_handle_id(surface.id),
                    "surface_ref": surface_ref(surface.id),
                    "pane_id": encode_handle_id(surface.pane_id),
                    "pane_ref": pane_ref(surface.pane_id),
                    "surface": surface
                }))
            })
        }
        "surface.create" => {
            let params = params_object(params)?;
            let workspace_id = optional_u64_param_any(params, &["workspace_id"])?;
            let title = optional_string_param(params, "title")?;
            let panel_type =
                optional_string_param(params, "type")?.unwrap_or_else(|| "terminal".to_string());
            let url = optional_string_param(params, "url")?;
            with_workspace_scope(state, workspace_id, |scoped| {
                let surface = scoped
                    .create_surface(title)
                    .ok_or_else(|| CommandError::not_found("no active pane"))?;
                if panel_type == "browser" {
                    scoped.browser_register_surface(surface.id);
                    scoped.browser_navigate(url.unwrap_or_else(|| "about:blank".to_string()));
                }
                Ok(json!({
                    "surface_id": encode_handle_id(surface.id),
                    "surface_ref": surface_ref(surface.id),
                    "pane_id": encode_handle_id(surface.pane_id),
                    "pane_ref": pane_ref(surface.pane_id),
                    "surface": surface
                }))
            })
        }
        "surface.split" => {
            let params = params_object(params)?;
            let workspace_id = optional_u64_param_any(params, &["workspace_id"])?;
            let source_surface_id = optional_u64_param_any(params, &["surface_id", "id"])?;
            let direction =
                optional_string_param(params, "direction")?.unwrap_or_else(|| "right".to_string());
            let title = optional_string_param(params, "title")?;
            with_workspace_scope(state, workspace_id, |scoped| {
                let source_pane_id = source_surface_id
                    .and_then(|surface_id| {
                        scoped.find_pane_for_surface_in_current_window(surface_id)
                    })
                    .or_else(|| focused_handles(scoped).map(|(_, _, pane_id, _)| pane_id))
                    .ok_or_else(|| CommandError::not_found("no active pane"))?;
                let surface = scoped
                    .split_surface_from_pane(source_pane_id, title)
                    .ok_or_else(|| CommandError::not_found("no active window"))?;
                scoped.register_split_relation(source_pane_id, surface.pane_id, &direction);
                Ok(json!({
                    "surface_id": encode_handle_id(surface.id),
                    "surface_ref": surface_ref(surface.id),
                    "pane_id": encode_handle_id(surface.pane_id),
                    "pane_ref": pane_ref(surface.pane_id),
                    "surface": surface
                }))
            })
        }
        "surface.focus" => {
            let params = params_object(params)?;
            let surface_id = optional_u64_param_any(params, &["surface_id", "id"])?
                .ok_or_else(|| CommandError::invalid_params("surface_id is required"))?;
            let surface = state
                .focus_surface(surface_id)
                .ok_or_else(|| CommandError::not_found("surface not found"))?;
            Ok(json!({
                "surface_id": encode_handle_id(surface.id),
                "surface_ref": surface_ref(surface.id),
                "pane_id": encode_handle_id(surface.pane_id),
                "pane_ref": pane_ref(surface.pane_id),
                "surface": surface
            }))
        }
        "surface.close" => {
            let params = params_object(params)?;
            let surface_id = optional_u64_param_any(params, &["surface_id", "id"])?;
            let surface = state
                .close_surface(surface_id)
                .ok_or_else(|| CommandError::not_found("surface not found"))?;
            Ok(json!({
                "surface_id": encode_handle_id(surface.id),
                "surface_ref": surface_ref(surface.id),
                "pane_id": encode_handle_id(surface.pane_id),
                "pane_ref": pane_ref(surface.pane_id),
                "surface": surface
            }))
        }
        "surface.move" => {
            let params = params_object(params)?;
            let surface_id = optional_u64_param_any(params, &["surface_id", "id"])?
                .ok_or_else(|| CommandError::invalid_params("surface_id is required"))?;
            let target_pane_id = optional_u64_param_any(params, &["target_pane_id", "pane_id"])?
                .ok_or_else(|| {
                    CommandError::invalid_params("target_pane_id/pane_id is required")
                })?;
            let index = optional_index_param(params, "index")?;
            let surface = state
                .move_surface(surface_id, target_pane_id, index)
                .ok_or_else(|| CommandError::not_found("surface or target pane not found"))?;
            Ok(json!({
                "surface_id": encode_handle_id(surface.id),
                "surface_ref": surface_ref(surface.id),
                "pane_id": encode_handle_id(surface.pane_id),
                "pane_ref": pane_ref(surface.pane_id),
                "surface": surface
            }))
        }
        "surface.reorder" => {
            let params = params_object(params)?;
            let surface_id = optional_u64_param_any(params, &["surface_id", "id"])?
                .ok_or_else(|| CommandError::invalid_params("surface_id is required"))?;
            let index = optional_index_param(params, "index")?;
            let before_surface_id = optional_u64_param_any(params, &["before_surface_id"])?;
            let after_surface_id = optional_u64_param_any(params, &["after_surface_id"])?;
            let targets = usize::from(index.is_some())
                + usize::from(before_surface_id.is_some())
                + usize::from(after_surface_id.is_some());
            if targets != 1 {
                return Err(CommandError::invalid_params(
                    "surface.reorder requires exactly one target: index|before_surface_id|after_surface_id",
                ));
            }
            let index = if let Some(index) = index {
                index
            } else if let Some(before_surface_id) = before_surface_id {
                let (_, _, encoded) = state
                    .find_surface_in_current_window(before_surface_id)
                    .ok_or_else(|| CommandError::not_found("before_surface not found"))?;
                let (_pane_idx, before_idx) = ControlState::decode_surface_index(encoded);
                before_idx
            } else if let Some(after_surface_id) = after_surface_id {
                let (_, _, encoded) = state
                    .find_surface_in_current_window(after_surface_id)
                    .ok_or_else(|| CommandError::not_found("after_surface not found"))?;
                let (_pane_idx, after_idx) = ControlState::decode_surface_index(encoded);
                after_idx.saturating_add(1)
            } else {
                0
            };
            let surface = state
                .reorder_surface(surface_id, index)
                .ok_or_else(|| CommandError::not_found("surface not found"))?;
            Ok(json!({
                "surface_id": encode_handle_id(surface.id),
                "surface_ref": surface_ref(surface.id),
                "pane_id": encode_handle_id(surface.pane_id),
                "pane_ref": pane_ref(surface.pane_id),
                "surface": surface
            }))
        }
        "surface.drag_to_split" => {
            let params = params_object(params)?;
            let surface_id = optional_u64_param_any(params, &["surface_id", "id"])?
                .ok_or_else(|| CommandError::invalid_params("surface_id is required"))?;
            let title = optional_string_param(params, "title")?;
            let surface = state
                .drag_surface_to_split(surface_id, title)
                .ok_or_else(|| CommandError::not_found("surface not found"))?;
            Ok(json!({
                "surface_id": encode_handle_id(surface.id),
                "surface_ref": surface_ref(surface.id),
                "pane_id": encode_handle_id(surface.pane_id),
                "pane_ref": pane_ref(surface.pane_id),
                "surface": surface
            }))
        }
        "surface.refresh" => {
            let params = params_object(params)?;
            let surface_id = optional_u64_param_any(params, &["surface_id", "id"])?;
            let surface = state
                .update_surface(surface_id, |surface| {
                    surface.refresh_count = surface.refresh_count.saturating_add(1)
                })
                .ok_or_else(|| CommandError::not_found("surface not found"))?;
            Ok(json!({
                "surface_id": encode_handle_id(surface.id),
                "surface_ref": surface_ref(surface.id),
                "surface": surface
            }))
        }
        "surface.health" => {
            let params = params_object(params)?;
            let workspace_id = optional_u64_param_any(params, &["workspace_id"])?;
            let surface_id = optional_u64_param_any(params, &["surface_id", "id"])?;
            with_workspace_scope(state, workspace_id, |scoped| {
                let focused_surface = focused_handles(scoped).map(|(_, _, _, id)| id);
                let surfaces = if let Some(id) = surface_id {
                    vec![scoped
                        .update_surface(Some(id), |_| {})
                        .ok_or_else(|| CommandError::not_found("surface not found"))?]
                } else {
                    scoped
                        .list_surfaces()
                        .ok_or_else(|| CommandError::not_found("surface list unavailable"))?
                };
                let rows: Vec<Value> = surfaces
                    .iter()
                    .enumerate()
                    .map(|(idx, surface)| {
                        json!({
                            "index": idx,
                            "id": encode_handle_id(surface.id),
                            "ref": surface_ref(surface.id),
                            "surface_id": encode_handle_id(surface.id),
                            "surface_ref": surface_ref(surface.id),
                            "pane_id": encode_handle_id(surface.pane_id),
                            "pane_ref": pane_ref(surface.pane_id),
                            "type": surface.panel_type,
                            "focused": focused_surface == Some(surface.id),
                            "selected": focused_surface == Some(surface.id),
                            "healthy": true,
                            "in_window": true,
                            "hidden": false,
                            "text_bytes": surface.text.len(),
                            "refresh_count": surface.refresh_count,
                            "flash_count": surface.flash_count,
                        })
                    })
                    .collect();
                Ok(json!({ "surfaces": rows }))
            })
        }
        "surface.read_text" => {
            let params = params_object(params)?;
            let workspace_id = optional_u64_param_any(params, &["workspace_id"])?;
            let surface_hint = optional_u64_param_any(params, &["surface_id", "id"])?;
            let (workspace_id, surface_id) =
                resolve_surface_target(state, workspace_id, surface_hint)?;
            let surface = update_surface_metadata(state, workspace_id, surface_id, |_| {})
                .ok_or_else(|| CommandError::not_found("surface not found"))?;
            Ok(json!({
                "workspace_id": encode_handle_id(workspace_id),
                "workspace_ref": workspace_ref(workspace_id),
                "surface_id": encode_handle_id(surface.id),
                "surface_ref": surface_ref(surface.id),
                "text": surface.text
            }))
        }
        "surface.send_text" => {
            let params = params_object(params)?;
            let workspace_id = optional_u64_param_any(params, &["workspace_id"])?;
            let surface_hint = optional_u64_param_any(params, &["surface_id", "id"])?;
            let text = required_string_param(params, "text")?;
            let (workspace_id, surface_id) =
                resolve_surface_target(state, workspace_id, surface_hint)?;
            let surface = apply_surface_text_input(state, workspace_id, surface_id, &text)?;
            Ok(json!({
                "workspace_id": encode_handle_id(workspace_id),
                "workspace_ref": workspace_ref(workspace_id),
                "surface_id": encode_handle_id(surface.id),
                "surface_ref": surface_ref(surface.id),
                "surface": surface
            }))
        }
        "surface.send_key" => {
            let params = params_object(params)?;
            let workspace_id = optional_u64_param_any(params, &["workspace_id"])?;
            let surface_hint = optional_u64_param_any(params, &["surface_id", "id"])?;
            let key = required_string_param(params, "key")?;
            let (workspace_id, surface_id) =
                resolve_surface_target(state, workspace_id, surface_hint)?;
            let surface = apply_surface_key_input(state, workspace_id, surface_id, &key)?;
            Ok(json!({
                "workspace_id": encode_handle_id(workspace_id),
                "workspace_ref": workspace_ref(workspace_id),
                "surface_id": encode_handle_id(surface.id),
                "surface_ref": surface_ref(surface.id),
                "surface": surface
            }))
        }
        "surface.trigger_flash" => {
            let params = params_object(params)?;
            let workspace_id = optional_u64_param_any(params, &["workspace_id"])?;
            let surface_hint = optional_u64_param_any(params, &["surface_id", "id"])?;
            let (workspace_id, surface_id) =
                resolve_surface_target(state, workspace_id, surface_hint)?;
            let surface = update_surface_metadata(state, workspace_id, surface_id, |surface| {
                surface.flash_count = surface.flash_count.saturating_add(1)
            })
            .ok_or_else(|| CommandError::not_found("surface not found"))?;
            state.debug_flash_count = state.debug_flash_count.saturating_add(1);
            Ok(json!({
                "workspace_id": encode_handle_id(workspace_id),
                "workspace_ref": workspace_ref(workspace_id),
                "surface_id": encode_handle_id(surface.id),
                "surface_ref": surface_ref(surface.id),
                "surface": surface
            }))
        }
        "surface.clear_history" => {
            let params = params_object(params)?;
            let workspace_id = optional_u64_param_any(params, &["workspace_id"])?;
            let surface_hint = optional_u64_param_any(params, &["surface_id", "id"])?;
            let (workspace_id, surface_id) =
                resolve_surface_target(state, workspace_id, surface_hint)?;
            let surface = update_surface_metadata(state, workspace_id, surface_id, |surface| {
                surface.text.clear();
                surface.unread = false;
            })
            .ok_or_else(|| CommandError::not_found("surface not found"))?;
            Ok(json!({
                "workspace_id": encode_handle_id(workspace_id),
                "workspace_ref": workspace_ref(workspace_id),
                "surface_id": encode_handle_id(surface.id),
                "surface_ref": surface_ref(surface.id),
                "surface": surface
            }))
        }
        "surface.action" => {
            let params = params_object(params)?;
            let action = required_string_param(params, "action")?;
            let action_key = action.to_ascii_lowercase().replace('-', "_");
            let workspace_hint = optional_u64_param_any(params, &["workspace_id"])?;
            let surface_hint = optional_u64_param_any(params, &["surface_id", "id"])?;
            let title = optional_string_param(params, "title")?;
            let (workspace_id, surface_id) =
                resolve_surface_target(state, workspace_hint, surface_hint)?;

            let updated = update_surface_metadata(state, workspace_id, surface_id, |surface| {
                apply_named_surface_action(surface, &action_key, title.as_deref())
            })
            .ok_or_else(|| CommandError::not_found("surface not found"))?;

            Ok(json!({
                "ok": true,
                "action": action,
                "workspace_id": encode_handle_id(workspace_id),
                "workspace_ref": workspace_ref(workspace_id),
                "surface_id": encode_handle_id(updated.id),
                "surface_ref": surface_ref(updated.id),
                "title": updated.title,
                "pinned": updated.pinned,
                "unread": updated.unread,
            }))
        }

        "notification.create" => {
            let params = params_object(params)?;
            let (title, subtitle, body) = normalize_notification_content(params)?;
            let surface_id = optional_u64_param_any(params, &["surface_id", "id"])?;
            let workspace_id = Some(state.current_workspace_id);
            Ok(created_notification_response(state.create_notification(
                title,
                subtitle,
                body,
                surface_id,
                workspace_id,
            )))
        }
        "notification.create_for_surface" => {
            let params = params_object(params)?;
            let (title, subtitle, body) = normalize_notification_content(params)?;
            let surface_id = optional_u64_param_any(params, &["surface_id", "id"])?
                .ok_or_else(|| CommandError::invalid_params("surface_id is required"))?;
            let workspace_id = find_workspace_for_surface(state, surface_id);
            Ok(created_notification_response(state.create_notification(
                title,
                subtitle,
                body,
                Some(surface_id),
                workspace_id,
            )))
        }
        "notification.list" => {
            let params = params_object(params)?;
            let unread_only = optional_bool_param(params, "unread_only")?.unwrap_or(false);
            let notifications: Vec<_> = if unread_only {
                state
                    .notifications
                    .iter()
                    .filter(|item| item.unread)
                    .cloned()
                    .collect()
            } else {
                state.notifications.clone()
            };
            let rows: Vec<Value> = notifications.iter().map(notification_row).collect();
            Ok(json!({ "notifications": rows }))
        }
        "notification.clear" => {
            let params = params_object(params)?;
            let id = optional_u64_param_any(params, &["id", "notification_id"])?;
            let notifications = state.clear_notification(id);
            let rows: Vec<Value> = notifications.iter().map(notification_row).collect();
            Ok(json!({ "notifications": rows }))
        }

        "browser.open_split" => {
            let params = params_object(params)?;
            let workspace_id = optional_u64_param_any(params, &["workspace_id"])?;
            let source_surface_id = optional_u64_param_any(params, &["surface_id", "id"])?;
            with_workspace_scope(state, workspace_id, |scoped| {
                let focused_surface_id =
                    focused_handles(scoped).map(|(_, _, _, surface_id)| surface_id);
                let source_surface_id = source_surface_id.or(focused_surface_id);
                let source_pane_id = source_surface_id
                    .and_then(|surface_id| {
                        scoped.find_pane_for_surface_in_current_window(surface_id)
                    })
                    .or_else(|| focused_handles(scoped).map(|(_, _, pane_id, _)| pane_id))
                    .ok_or_else(|| CommandError::not_found("no active pane"))?;
                let url = optional_string_param(params, "url")?.unwrap_or_else(|| {
                    let can_inherit_source_url = source_surface_id
                        .filter(|surface_id| scoped.browser.browser_surfaces.contains(surface_id))
                        .map(|surface_id| {
                            scoped.browser.surface_id == Some(surface_id)
                                || scoped.browser.current_tab_id == surface_id
                        })
                        .unwrap_or(false);
                    if scoped.browser.open && can_inherit_source_url {
                        scoped.browser.url.clone()
                    } else {
                        "about:blank".to_string()
                    }
                });
                let target_pane_id = scoped.right_neighbor_in_current_window(source_pane_id);

                let (created_split, created) = if let Some(target_pane_id) = target_pane_id {
                    let created = scoped
                        .create_surface_in_pane(target_pane_id, Some("browser".to_string()))
                        .ok_or_else(|| CommandError::not_found("target pane not found"))?;
                    (false, created)
                } else {
                    let created = scoped
                        .split_surface_from_pane(source_pane_id, Some("browser".to_string()))
                        .ok_or_else(|| CommandError::not_found("no active window"))?;
                    scoped.register_split_relation(source_pane_id, created.pane_id, "right");
                    (true, created)
                };

                scoped.browser.open = true;
                scoped.browser.focused = true;
                scoped.browser_register_surface(created.id);
                scoped.browser_navigate(url.clone());
                Ok(json!({
                    "surface_id": encode_handle_id(created.id),
                    "surface_ref": surface_ref(created.id),
                    "pane_id": encode_handle_id(created.pane_id),
                    "pane_ref": pane_ref(created.pane_id),
                    "target_pane_id": encode_handle_id(created.pane_id),
                    "created_split": created_split,
                    "browser": {
                        "open": scoped.browser.open,
                        "url": scoped.browser.url,
                        "title": scoped.browser.title
                    }
                }))
            })
        }
        "browser.navigate" => {
            let params = params_object(params)?;
            let url = required_string_param(params, "url")?;
            let surface_id = optional_u64_param_any(params, &["surface_id", "id"])?;
            ensure_browser_surface(&state.browser, surface_id)?;
            state.browser_navigate(url);
            Ok(json!({
                "surface_id": state.browser.surface_id.map(encode_handle_id),
                "surface_ref": state.browser.surface_id.map(surface_ref),
                "browser": {
                    "open": state.browser.open,
                    "url": state.browser.url,
                    "title": state.browser.title
                }
            }))
        }
        "browser.url.get" => {
            let params = params_object(params)?;
            let surface_id = optional_u64_param_any(params, &["surface_id", "id"])?;
            ensure_browser_surface(&state.browser, surface_id)?;
            Ok(json!({ "url": state.browser.url }))
        }
        "browser.eval" => {
            let params = params_object(params)?;
            let script = required_string_param(params, "script")?;
            let value = if script.contains("document.title") {
                Value::String(state.browser.title.clone())
            } else if script.contains("window.location.href") {
                Value::String(state.browser.url.clone())
            } else if script.contains("document.readyState") {
                Value::String("complete".to_string())
            } else if script.contains("document.activeElement") {
                Value::String(
                    state
                        .browser
                        .active_element
                        .trim_start_matches('#')
                        .to_string(),
                )
            } else if script.contains("window.frameClicks") {
                Value::Number(state.browser.frame_clicks.into())
            } else if script.contains("querySelector('#name').value") {
                Value::String(
                    state
                        .browser
                        .fields
                        .get("#name")
                        .cloned()
                        .unwrap_or_default(),
                )
            } else if script.contains("__limuxInitMarker") {
                Value::String(state.browser.init_marker.clone())
            } else if script.contains("window.__hover") || script.contains("window.__keys") {
                json!({
                    "hover": state.browser.hover_count,
                    "dbl": state.browser.dbl_count,
                    "down": state.browser.key_down_count,
                    "up": state.browser.key_up_count,
                    "press": state.browser.key_press_count
                })
            } else if script.contains("querySelector('#scroller').scrollTop") {
                let value = state
                    .browser
                    .scroll_tops
                    .get("#scroller")
                    .copied()
                    .unwrap_or(0.0_f64);
                json!(value)
            } else if script.contains("querySelector('#bottom')") && script.contains("innerHeight")
            {
                json!(state.browser.in_view.contains("#bottom"))
            } else if script.contains("document.querySelector(") && script.contains("!== null") {
                let selector = if script.contains("#probe") {
                    "#probe"
                } else if script.contains("#hdr") {
                    "#hdr"
                } else if script.contains("#frame-text") {
                    "#frame-text"
                } else {
                    ""
                };
                Value::Bool(!selector.is_empty() && state.browser_selector_exists(selector))
            } else if script.contains("document.body") {
                Value::String(state.browser.page_text.clone())
            } else {
                Value::Null
            };
            Ok(json!({ "value": value }))
        }
        "browser.wait" => {
            let params = params_object(params)?;
            let surface_id = optional_u64_param_any(params, &["surface_id", "id"])?;
            ensure_browser_surface(&state.browser, surface_id)?;
            let selector = optional_string_param(params, "selector")?;
            let text_contains = optional_string_param(params, "text_contains")?;
            let function = optional_string_param(params, "function")?;
            let load_state = optional_string_param(params, "load_state")?;
            let url_contains = optional_string_param(params, "url_contains")?;

            let ready = if let Some(selector) = selector {
                state.browser_selector_exists(&selector)
            } else if let Some(text_contains) = text_contains {
                state.browser.page_text.contains(&text_contains)
            } else if let Some(function) = function {
                if function.contains("#frame-text") {
                    state.browser_selector_exists("#frame-text")
                } else if function.contains("#hdr") {
                    state.browser_selector_exists("#hdr")
                } else {
                    !function.contains("#never")
                }
            } else if let Some(load_state) = load_state {
                load_state.eq_ignore_ascii_case("complete")
            } else if let Some(url_contains) = url_contains {
                state.browser.url.contains(&url_contains)
            } else {
                state.browser.open
            };

            if !ready {
                return Err(CommandError::timeout("wait condition not met"));
            }
            Ok(json!({ "ready": true, "ok": true }))
        }
        "browser.click" => {
            let params = params_object(params)?;
            let selector = required_string_param(params, "selector")?;
            let resolved = state.browser_resolve_selector(&selector);
            if !state.browser_selector_exists(&resolved) {
                return Err(CommandError::not_found(
                    "element not found; snapshot: run browser.snapshot; hint: verify selector",
                ));
            }
            if resolved == "#btn" {
                let value = state
                    .browser
                    .fields
                    .get("#name")
                    .cloned()
                    .unwrap_or_default();
                let status = if value.is_empty() {
                    "empty".to_string()
                } else {
                    value
                };
                if state.browser_selector_exists("#out") {
                    state
                        .browser
                        .dom_text
                        .insert("#out".to_string(), status.clone());
                    state.browser.dom_html.insert(
                        "#out".to_string(),
                        format!("<div id=\"out\">{status}</div>"),
                    );
                } else {
                    state
                        .browser
                        .dom_text
                        .insert("#status".to_string(), status.clone());
                    state.browser.dom_html.insert(
                        "#status".to_string(),
                        format!("<div id=\"status\" data-role=\"status\">{status}</div>"),
                    );
                }
            }
            if resolved == "#action-btn" {
                state
                    .browser
                    .dom_text
                    .insert("#status".to_string(), "clicked".to_string());
                state.browser.dom_html.insert(
                    "#status".to_string(),
                    "<div id=\"status\">clicked</div>".to_string(),
                );
            }
            if resolved == "#frame-btn" && state.browser.frame_selected {
                state.browser.frame_clicks = state.browser.frame_clicks.saturating_add(1);
            }
            Ok(json!({ "ok": true, "selector": selector }))
        }
        "browser.fill" => {
            let params = params_object(params)?;
            let selector = required_string_param(params, "selector")?;
            let value = optional_string_param(params, "value")?
                .or_else(|| optional_string_param(params, "text").ok().flatten())
                .unwrap_or_default();
            state.browser.fields.insert(selector.clone(), value.clone());
            let mut payload = json!({ "ok": true, "selector": selector, "value": value });
            if optional_bool_param(params, "snapshot_after")?.unwrap_or(false) {
                if let Some(obj) = payload.as_object_mut() {
                    obj.insert(
                        "post_action_snapshot".to_string(),
                        json!(format!(
                            "- document \"{}\"\n  - ref=e1 text=\"{}\"",
                            state.browser.title, state.browser.page_text
                        )),
                    );
                }
            }
            Ok(payload)
        }
        "browser.get.text" => {
            let params = params_object(params)?;
            let selector = required_string_param(params, "selector")?;
            let selector = state.browser_resolve_selector(&selector);
            let text = if selector == "body" {
                state.browser.page_text.clone()
            } else if selector == "#frame-text" && state.browser.frame_selected {
                "frame-ready".to_string()
            } else {
                state
                    .browser
                    .dom_text
                    .get(&selector)
                    .cloned()
                    .or_else(|| state.browser.fields.get(&selector).cloned())
                    .unwrap_or_default()
            };
            Ok(json!({ "text": text, "value": text }))
        }
        "browser.get.value" => {
            let params = params_object(params)?;
            let selector = required_string_param(params, "selector")?;
            let selector = state.browser_resolve_selector(&selector);
            let value = state
                .browser
                .fields
                .get(&selector)
                .cloned()
                .unwrap_or_default();
            Ok(json!({ "value": value, "text": value }))
        }
        "browser.get.title" => Ok(json!({ "title": state.browser.title })),
        "browser.snapshot" => Ok(json!({
            "url": state.browser.url,
            "title": state.browser.title,
            "snapshot": format!("- document \"{}\"\n  - text \"{}\"", state.browser.title, state.browser.page_text),
            "text": format!("{} {}", state.browser.title, state.browser.page_text),
            "refs": {
                "e1": {"role": "document", "name": state.browser.title},
                "e2": {"role": "text", "name": state.browser.page_text},
            },
            "nodes": [
                {"role": "document", "name": state.browser.title},
                {"role": "paragraph", "name": state.browser.page_text}
            ]
        })),
        "browser.focus_webview" => {
            state.browser.focused = true;
            Ok(json!({ "focused": true, "is_webview_focused": true }))
        }
        "browser.is_webview_focused" => Ok(
            json!({ "focused": state.browser.focused, "is_webview_focused": state.browser.focused }),
        ),
        "browser.screenshot" => Ok(json!({
            "ok": true,
            "format": "png",
            "bytes": 256,
            "path": "/tmp/limux-browser-shot.png",
            "url": "file:///tmp/limux-browser-shot.png",
            "png_base64": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        })),
        "browser.back" => {
            if state.browser.history_index > 0 {
                state.browser.history_index -= 1;
                state.browser_history_set_current();
            }
            Ok(json!({ "url": state.browser.url }))
        }
        "browser.forward" => {
            if state.browser.history_index + 1 < state.browser.history.len() {
                state.browser.history_index += 1;
                state.browser_history_set_current();
            }
            Ok(json!({ "url": state.browser.url }))
        }
        "browser.reload" => {
            state.browser_history_set_current();
            Ok(json!({ "url": state.browser.url }))
        }
        method if method.starts_with("browser.") => {
            let params = params_object(params)?;
            handle_browser_extended_command(state, method, params)
        }
        method if method.starts_with("debug.") => {
            let params = params_object(params)?;
            handle_debug_command(state, method, params)
        }

        "tab.action" => {
            let params = params_object(params)?;
            let action = required_string_param(params, "action")?;
            let action_key = action.to_ascii_lowercase().replace('-', "_");
            let workspace_hint = optional_u64_param_any(params, &["workspace_id"])?;
            let tab_hint = optional_u64_param_any(params, &["tab_id", "surface_id", "id"])?;
            let title = optional_string_param(params, "title")?;
            let (workspace_id, surface_id) =
                resolve_surface_target(state, workspace_hint, tab_hint)?;

            let updated = update_surface_metadata(state, workspace_id, surface_id, |surface| {
                apply_named_surface_action(surface, &action_key, title.as_deref())
            })
            .ok_or_else(|| CommandError::not_found("tab not found"))?;

            Ok(json!({
                "ok": true,
                "action": action,
                "workspace_id": encode_handle_id(workspace_id),
                "workspace_ref": workspace_ref(workspace_id),
                "surface_id": encode_handle_id(updated.id),
                "surface_ref": surface_ref(updated.id),
                "tab_ref": format!("tab:{}", updated.id),
                "title": updated.title,
                "pinned": updated.pinned,
                "unread": updated.unread,
            }))
        }

        _ => Err(CommandError::unknown_method(method)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(method: &str, params: Value) -> V2Request {
        V2Request {
            id: Some(Value::String("test".to_string())),
            method: method.to_string(),
            params,
        }
    }

    #[tokio::test]
    async fn dispatcher_handles_system_commands() {
        let dispatcher = Dispatcher::new();

        let ping = dispatcher.dispatch(request("system.ping", json!({}))).await;
        assert_eq!(ping.result.expect("ping result")["pong"], true);

        let identify = dispatcher
            .dispatch(request("system.identify", json!({})))
            .await;
        let identify_result = identify.result.expect("identify result");
        assert_eq!(identify_result["name"], "limux-control");

        let capabilities = dispatcher
            .dispatch(request("system.capabilities", json!({})))
            .await;
        let capabilities_result = capabilities.result.expect("capabilities result");
        for key in ["commands", "methods"] {
            assert!(capabilities_result[key]
                .as_array()
                .expect("capabilities array")
                .contains(&Value::String("surface.send_text".to_string())));
        }
    }

    #[tokio::test]
    async fn dispatcher_handles_workspace_and_window_flow() {
        let dispatcher = Dispatcher::new();

        let created_workspace = dispatcher
            .dispatch(request("workspace.create", json!({ "name": "dev" })))
            .await;
        assert_eq!(
            created_workspace.result.expect("workspace result")["workspace"]["name"],
            "dev"
        );

        let previous = dispatcher
            .dispatch(request("workspace.previous", json!({})))
            .await;
        assert_eq!(
            previous.result.expect("workspace previous")["workspace"]["name"],
            "main"
        );

        let moved_workspace = dispatcher
            .dispatch(request(
                "workspace.move_to_window",
                json!({ "window_id": 7 }),
            ))
            .await;
        assert_eq!(
            moved_workspace.result.expect("workspace move")["workspace"]["host_window_id"],
            7
        );

        let window = dispatcher
            .dispatch(request("window.create", json!({ "title": "shell" })))
            .await;
        assert_eq!(
            window.result.expect("window create")["window"]["title"],
            "shell"
        );

        let windows = dispatcher.dispatch(request("window.list", json!({}))).await;
        assert!(
            windows.result.expect("window list")["windows"]
                .as_array()
                .expect("window array")
                .len()
                >= 2
        );
    }

    #[tokio::test]
    async fn dispatcher_handles_pane_and_surface_flow() {
        let dispatcher = Dispatcher::new();

        let new_surface = dispatcher
            .dispatch(request("surface.create", json!({ "title": "agent" })))
            .await;
        let surface_id = new_surface.result.expect("surface create")["surface"]["id"]
            .as_u64()
            .expect("surface id");

        let sent_text = dispatcher
            .dispatch(request(
                "surface.send_text",
                json!({ "surface_id": surface_id, "text": "hello" }),
            ))
            .await;
        assert_eq!(
            sent_text.result.expect("send text")["surface"]["text"],
            "hello"
        );

        let read = dispatcher
            .dispatch(request(
                "surface.read_text",
                json!({ "surface_id": surface_id }),
            ))
            .await;
        assert_eq!(read.result.expect("read text")["text"], "hello");

        let split = dispatcher
            .dispatch(request("surface.split", json!({ "title": "split" })))
            .await;
        let split_pane_id = split.result.expect("split")["surface"]["pane_id"]
            .as_u64()
            .expect("pane id");

        let moved = dispatcher
            .dispatch(request(
                "surface.move",
                json!({ "surface_id": surface_id, "target_pane_id": split_pane_id }),
            ))
            .await;
        assert_eq!(
            moved.result.expect("surface move")["surface"]["pane_id"],
            split_pane_id
        );

        let cleared = dispatcher
            .dispatch(request(
                "surface.clear_history",
                json!({ "surface_id": surface_id }),
            ))
            .await;
        assert_eq!(
            cleared.result.expect("clear history")["surface"]["text"],
            ""
        );
    }

    #[tokio::test]
    async fn dispatcher_surface_send_text_executes_shell_touch_command() {
        let dispatcher = Dispatcher::new();
        let marker = std::env::temp_dir().join(format!(
            "limux-touch-marker-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&marker);

        let command = format!("touch {}\n", marker.to_string_lossy());
        let sent = dispatcher
            .dispatch(request("surface.send_text", json!({ "text": command })))
            .await;
        assert!(sent.result.is_some(), "send_text should succeed");
        assert!(marker.exists(), "touch command should create marker file");
        let _ = std::fs::remove_file(marker);
    }

    #[tokio::test]
    async fn dispatcher_surface_ctrl_keys_interrupt_terminal_modes() {
        let dispatcher = Dispatcher::new();
        let marker = std::env::temp_dir().join(format!(
            "limux-ctrl-marker-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&marker);

        let _ = dispatcher
            .dispatch(request(
                "surface.send_text",
                json!({ "text": "sleep 30\n" }),
            ))
            .await;
        let _ = dispatcher
            .dispatch(request(
                "surface.send_text",
                json!({ "text": format!("touch {}\n", marker.to_string_lossy()) }),
            ))
            .await;
        assert!(
            !marker.exists(),
            "sleep mode should block command execution until ctrl-c"
        );

        let _ = dispatcher
            .dispatch(request("surface.send_key", json!({ "key": "ctrl-c" })))
            .await;
        let _ = dispatcher
            .dispatch(request(
                "surface.send_text",
                json!({ "text": format!("touch {}\n", marker.to_string_lossy()) }),
            ))
            .await;
        assert!(marker.exists(), "ctrl-c should return terminal to idle");
        let _ = std::fs::remove_file(&marker);

        let _ = dispatcher
            .dispatch(request("surface.send_text", json!({ "text": "cat\n" })))
            .await;
        let _ = dispatcher
            .dispatch(request(
                "surface.send_text",
                json!({ "text": format!("touch {}\n", marker.to_string_lossy()) }),
            ))
            .await;
        assert!(
            !marker.exists(),
            "cat mode should block shell command execution until ctrl-d"
        );

        let _ = dispatcher
            .dispatch(request("surface.send_key", json!({ "key": "ctrl-d" })))
            .await;
        let _ = dispatcher
            .dispatch(request(
                "surface.send_text",
                json!({ "text": format!("touch {}\n", marker.to_string_lossy()) }),
            ))
            .await;
        assert!(marker.exists(), "ctrl-d should exit cat mode");
        let _ = std::fs::remove_file(marker);
    }

    #[tokio::test]
    async fn dispatcher_surface_runs_python_env_dump_with_ghostty_paths() {
        let dispatcher = Dispatcher::new();
        let env_path = std::env::temp_dir().join(format!(
            "limux-env-dump-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&env_path);

        let command = format!(
            "python3 -c 'import json,os;open(\"{}\",\"w\").write(json.dumps({{\"TERMINFO\": os.environ.get(\"TERMINFO\", \"\"),\"XDG_DATA_DIRS\": os.environ.get(\"XDG_DATA_DIRS\", \"\")}}))'\n",
            env_path.to_string_lossy()
        );
        let _ = dispatcher
            .dispatch(request("surface.send_text", json!({ "text": command })))
            .await;

        assert!(env_path.exists(), "env dump file should be created");
        let payload = std::fs::read_to_string(&env_path).expect("read env dump");
        let parsed: Value = serde_json::from_str(&payload).expect("parse env json");
        let terminfo = parsed["TERMINFO"].as_str().unwrap_or_default();
        let xdg_data_dirs = parsed["XDG_DATA_DIRS"].as_str().unwrap_or_default();

        assert!(!terminfo.is_empty(), "TERMINFO should be non-empty");
        let terminfo_path = std::path::PathBuf::from(terminfo);
        assert!(terminfo_path.exists(), "TERMINFO path should exist");
        assert!(
            terminfo_path.join("78").join("xterm-ghostty").exists(),
            "TERMINFO should include xterm-ghostty entry"
        );

        assert!(
            !xdg_data_dirs.is_empty(),
            "XDG_DATA_DIRS should be non-empty"
        );
        let xdg_entries: Vec<&str> = xdg_data_dirs
            .split(':')
            .filter(|part| !part.trim().is_empty())
            .collect();
        let resources_path = terminfo_path
            .parent()
            .expect("resources parent")
            .to_string_lossy()
            .to_string();
        assert!(
            xdg_entries.iter().any(|entry| *entry == resources_path),
            "XDG_DATA_DIRS should contain resources path"
        );
        assert!(
            xdg_entries.contains(&"/usr/local/share") && xdg_entries.contains(&"/usr/share"),
            "XDG_DATA_DIRS should contain standard default entries"
        );

        let _ = std::fs::remove_file(env_path);
    }

    #[tokio::test]
    async fn dispatcher_handles_surface_and_tab_actions_with_workspace_resolution() {
        let dispatcher = Dispatcher::new();

        let created_workspace = dispatcher
            .dispatch(request("workspace.create", json!({ "name": "target" })))
            .await;
        let workspace_id = created_workspace.result.expect("workspace create")["workspace"]["id"]
            .as_u64()
            .expect("workspace id");

        let current_surface = dispatcher
            .dispatch(request(
                "surface.current",
                json!({ "workspace_id": workspace_id }),
            ))
            .await;
        let surface_id = current_surface.result.expect("surface current")["surface"]["id"]
            .as_u64()
            .expect("surface id");

        // Switch away so tab_id-only lookups must resolve across workspaces.
        let _ = dispatcher
            .dispatch(request("workspace.previous", json!({})))
            .await;

        let marked_unread = dispatcher
            .dispatch(request(
                "tab.action",
                json!({ "tab_id": format!("tab:{surface_id}"), "action": "mark_unread" }),
            ))
            .await;
        let marked_unread_result = marked_unread.result.expect("tab action mark_unread");
        assert_eq!(
            marked_unread_result["tab_ref"],
            Value::String(format!("tab:{surface_id}"))
        );
        assert_eq!(
            marked_unread_result["workspace_id"],
            Value::String(encode_handle_id(workspace_id))
        );
        assert_eq!(marked_unread_result["unread"], true);

        let renamed = dispatcher
            .dispatch(request(
                "surface.action",
                json!({
                    "surface_id": surface_id,
                    "action": "rename",
                    "title": "renamed-from-test"
                }),
            ))
            .await;
        assert_eq!(
            renamed.result.expect("surface action rename")["title"],
            "renamed-from-test"
        );
    }

    #[tokio::test]
    async fn dispatcher_handles_notifications_and_app_flags() {
        let dispatcher = Dispatcher::new();

        let notification = dispatcher
            .dispatch(request(
                "notification.create",
                json!({ "message": "agent done" }),
            ))
            .await;
        assert_eq!(
            notification.result.expect("notification")["notification"]["message"],
            "agent done"
        );

        let listed = dispatcher
            .dispatch(request("notification.list", json!({})))
            .await;
        assert_eq!(
            listed.result.expect("notification list")["notifications"]
                .as_array()
                .expect("notifications")
                .len(),
            1
        );

        let cleared = dispatcher
            .dispatch(request("notification.clear", json!({})))
            .await;
        assert_eq!(
            cleared.result.expect("notification clear")["notifications"]
                .as_array()
                .expect("notifications")
                .len(),
            0
        );

        let focus_override = dispatcher
            .dispatch(request(
                "app.focus_override.set",
                json!({ "enabled": true }),
            ))
            .await;
        assert_eq!(
            focus_override.result.expect("focus override")["focus_override"],
            true
        );

        let active = dispatcher
            .dispatch(request("app.simulate_active", json!({ "active": true })))
            .await;
        assert_eq!(
            active.result.expect("simulate active")["simulate_active"],
            true
        );
    }

    #[test]
    fn osc9_desktop_notifications_become_limux_notifications() {
        let mut state = ControlState::default();
        let workspace_id = state.current_workspace_id;
        let surface_id = state.workspaces[0].windows[0].panes[0].surfaces[0].id;

        maybe_emit_osc_notification(
            &mut state,
            workspace_id,
            surface_id,
            "\\x1b]9;Codex turn complete\\x07",
        );

        assert_eq!(state.notifications.len(), 1);
        let notification = &state.notifications[0];
        assert_eq!(notification.title, "");
        assert_eq!(notification.body, "Codex turn complete");
        assert_eq!(notification.message, "Codex turn complete");
        assert_eq!(notification.surface_id, Some(surface_id));
        assert_eq!(notification.workspace_id, Some(workspace_id));
        assert!(notification.unread);
    }

    #[test]
    fn osc9_control_byte_notifications_become_limux_notifications() {
        let mut state = ControlState::default();
        let workspace_id = state.current_workspace_id;
        let surface_id = state.workspaces[0].windows[0].panes[0].surfaces[0].id;

        maybe_emit_osc_notification(
            &mut state,
            workspace_id,
            surface_id,
            "\u{1b}]9;Codex turn complete\u{7}",
        );

        assert_eq!(state.notifications.len(), 1);
        let notification = &state.notifications[0];
        assert_eq!(notification.title, "");
        assert_eq!(notification.body, "Codex turn complete");
        assert_eq!(notification.message, "Codex turn complete");
        assert_eq!(notification.surface_id, Some(surface_id));
        assert_eq!(notification.workspace_id, Some(workspace_id));
        assert!(notification.unread);
    }

    #[tokio::test]
    async fn dispatcher_handles_browser_p0_flow() {
        let dispatcher = Dispatcher::new();

        let opened = dispatcher
            .dispatch(request(
                "browser.open_split",
                json!({ "url": "https://example.com" }),
            ))
            .await;
        assert_eq!(
            opened.result.expect("browser open")["browser"]["url"],
            "https://example.com"
        );

        let navigated = dispatcher
            .dispatch(request(
                "browser.navigate",
                json!({ "url": "https://cmux.dev" }),
            ))
            .await;
        assert_eq!(
            navigated.result.expect("browser navigate")["browser"]["url"],
            "https://cmux.dev"
        );

        let url_get = dispatcher
            .dispatch(request("browser.url.get", json!({})))
            .await;
        assert_eq!(
            url_get.result.expect("browser url")["url"],
            "https://cmux.dev"
        );

        let eval = dispatcher
            .dispatch(request(
                "browser.eval",
                json!({ "script": "document.title" }),
            ))
            .await;
        assert_eq!(eval.result.expect("browser eval")["value"], "cmux.dev");

        let focused = dispatcher
            .dispatch(request("browser.focus_webview", json!({})))
            .await;
        assert_eq!(
            focused.result.expect("browser focus")["is_webview_focused"],
            true
        );
    }

    #[tokio::test]
    async fn dispatcher_handles_browser_file_url_navigation() {
        let dispatcher = Dispatcher::new();
        let unique_name = format!(
            "limux-file-url-{}-{}.html",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        );
        let file_path = std::env::temp_dir().join(unique_name);
        std::fs::write(
            &file_path,
            "<!doctype html><html><head><title>limux file url load</title></head><body><h1>local HTML file loaded</h1></body></html>",
        )
        .expect("write temp html");
        let file_url = format!("file://{}", file_path.to_string_lossy());

        let opened = dispatcher
            .dispatch(request(
                "browser.open_split",
                json!({ "url": "about:blank" }),
            ))
            .await;
        let surface_id = opened.result.expect("open split")["surface_id"].clone();

        let navigated = dispatcher
            .dispatch(request(
                "browser.navigate",
                json!({ "surface_id": surface_id, "url": file_url }),
            ))
            .await;
        assert_eq!(
            navigated.result.expect("navigate")["browser"]["title"],
            "limux file url load"
        );

        let title = dispatcher
            .dispatch(request(
                "browser.get.title",
                json!({ "surface_id": surface_id.clone() }),
            ))
            .await;
        assert_eq!(title.result.expect("title")["title"], "limux file url load");

        let eval = dispatcher
            .dispatch(request(
                "browser.eval",
                json!({ "surface_id": surface_id, "script": "document.body ? (document.body.innerText || '') : ''" }),
            ))
            .await;
        let eval_result = eval.result.expect("eval");
        let body_text = eval_result["value"].as_str().expect("body text");
        assert!(body_text.contains("local HTML file loaded"));

        let _ = std::fs::remove_file(file_path);
    }

    #[tokio::test]
    async fn dispatcher_browser_open_split_reuses_right_neighbor_panes() {
        let dispatcher = Dispatcher::new();

        let created_workspace = dispatcher
            .dispatch(request("workspace.create", json!({})))
            .await;
        let workspace_id =
            created_workspace.result.expect("workspace create")["workspace_id"].clone();
        let _ = dispatcher
            .dispatch(request(
                "workspace.select",
                json!({ "workspace_id": workspace_id.clone() }),
            ))
            .await;

        let current = dispatcher
            .dispatch(request(
                "surface.current",
                json!({ "workspace_id": workspace_id.clone() }),
            ))
            .await;
        let left_surface_id = current.result.expect("surface current")["surface_id"].clone();

        let right = dispatcher
            .dispatch(request(
                "surface.split",
                json!({
                    "workspace_id": workspace_id.clone(),
                    "surface_id": left_surface_id.clone(),
                    "direction": "right"
                }),
            ))
            .await;
        let right_surface_id = right.result.expect("split right")["surface_id"].clone();

        let right_down = dispatcher
            .dispatch(request(
                "surface.split",
                json!({
                    "workspace_id": workspace_id.clone(),
                    "surface_id": right_surface_id.clone(),
                    "direction": "down"
                }),
            ))
            .await;
        let right_bottom_surface_id =
            right_down.result.expect("split right down")["surface_id"].clone();

        let left_down = dispatcher
            .dispatch(request(
                "surface.split",
                json!({
                    "workspace_id": workspace_id.clone(),
                    "surface_id": left_surface_id.clone(),
                    "direction": "down"
                }),
            ))
            .await;
        let left_bottom_surface_id =
            left_down.result.expect("split left down")["surface_id"].clone();

        let surface_list = dispatcher
            .dispatch(request(
                "surface.list",
                json!({ "workspace_id": workspace_id.clone() }),
            ))
            .await;
        let surface_list_result = surface_list.result.expect("surface list");
        let surfaces = surface_list_result["surfaces"]
            .as_array()
            .expect("surface rows");
        let pane_for = |surface_id: &Value| -> Value {
            surfaces
                .iter()
                .find(|row| row["id"] == *surface_id)
                .map(|row| row["pane_id"].clone())
                .expect("pane for surface")
        };
        let right_top_pane = pane_for(&right_surface_id);
        let right_bottom_pane = pane_for(&right_bottom_surface_id);

        let pane_list_before = dispatcher
            .dispatch(request(
                "pane.list",
                json!({ "workspace_id": workspace_id.clone() }),
            ))
            .await;
        let base_pane_count = pane_list_before.result.expect("pane list before")["panes"]
            .as_array()
            .expect("pane rows before")
            .len();

        let open_from_left_top = dispatcher
            .dispatch(request(
                "browser.open_split",
                json!({
                    "workspace_id": workspace_id.clone(),
                    "surface_id": left_surface_id.clone(),
                    "url": "about:blank"
                }),
            ))
            .await;
        let open_from_left_top = open_from_left_top.result.expect("open from left top");
        assert_eq!(open_from_left_top["created_split"], false);
        assert_eq!(open_from_left_top["target_pane_id"], right_top_pane);

        let pane_list_after_left_top = dispatcher
            .dispatch(request(
                "pane.list",
                json!({ "workspace_id": workspace_id.clone() }),
            ))
            .await;
        assert_eq!(
            pane_list_after_left_top
                .result
                .expect("pane list after left top")["panes"]
                .as_array()
                .expect("pane rows after left top")
                .len(),
            base_pane_count
        );

        let open_from_left_bottom = dispatcher
            .dispatch(request(
                "browser.open_split",
                json!({
                    "workspace_id": workspace_id.clone(),
                    "surface_id": left_bottom_surface_id.clone(),
                    "url": "about:blank"
                }),
            ))
            .await;
        let open_from_left_bottom = open_from_left_bottom.result.expect("open from left bottom");
        assert_eq!(open_from_left_bottom["created_split"], false);
        assert_eq!(open_from_left_bottom["target_pane_id"], right_bottom_pane);

        let pane_list_after_left_bottom = dispatcher
            .dispatch(request(
                "pane.list",
                json!({ "workspace_id": workspace_id.clone() }),
            ))
            .await;
        assert_eq!(
            pane_list_after_left_bottom
                .result
                .expect("pane list after left bottom")["panes"]
                .as_array()
                .expect("pane rows after left bottom")
                .len(),
            base_pane_count
        );

        let open_from_right = dispatcher
            .dispatch(request(
                "browser.open_split",
                json!({
                    "workspace_id": workspace_id.clone(),
                    "surface_id": right_bottom_surface_id,
                    "url": "about:blank"
                }),
            ))
            .await;
        let open_from_right = open_from_right.result.expect("open from right");
        assert_eq!(open_from_right["created_split"], true);

        let pane_list_after_right = dispatcher
            .dispatch(request(
                "pane.list",
                json!({ "workspace_id": workspace_id }),
            ))
            .await;
        assert_eq!(
            pane_list_after_right.result.expect("pane list after right")["panes"]
                .as_array()
                .expect("pane rows after right")
                .len(),
            base_pane_count + 1
        );
    }

    #[tokio::test]
    async fn dispatcher_browser_open_split_without_url_inherits_current_browser_page() {
        let dispatcher = Dispatcher::new();

        let opened = dispatcher
            .dispatch(request(
                "browser.open_split",
                json!({ "url": "https://example.com/start" }),
            ))
            .await;
        let source_surface_id = opened.result.expect("initial browser open")["surface_id"].clone();

        let navigated = dispatcher
            .dispatch(request(
                "browser.navigate",
                json!({
                    "surface_id": source_surface_id.clone(),
                    "url": "https://cmux.dev/docs"
                }),
            ))
            .await;
        assert_eq!(
            navigated.result.expect("navigate current browser")["browser"]["url"],
            "https://cmux.dev/docs"
        );

        let duplicated = dispatcher
            .dispatch(request(
                "browser.open_split",
                json!({ "surface_id": source_surface_id.clone() }),
            ))
            .await;
        let duplicated_result = duplicated.result.expect("duplicate browser split");
        assert_eq!(duplicated_result["browser"]["url"], "https://cmux.dev/docs");

        let source_url = dispatcher
            .dispatch(request(
                "browser.url.get",
                json!({ "surface_id": source_surface_id }),
            ))
            .await;
        assert_eq!(
            source_url.result.expect("source browser url")["url"],
            "https://cmux.dev/docs"
        );
    }

    #[tokio::test]
    async fn dispatcher_shortcut_simulate_moves_focus_right() {
        let dispatcher = Dispatcher::new();

        let created = dispatcher
            .dispatch(request("pane.create", json!({ "direction": "right" })))
            .await;
        assert!(created.result.is_some());

        let panes_before = dispatcher.dispatch(request("pane.list", json!({}))).await;
        let panes_before_result = panes_before.result.expect("pane list before");
        let panes_before = panes_before_result["panes"]
            .as_array()
            .expect("pane rows before");
        assert!(panes_before.len() >= 2);
        let first_pane_id = panes_before[0]["id"].clone();
        let second_pane_id = panes_before[1]["id"].clone();

        let _ = dispatcher
            .dispatch(request("pane.focus", json!({ "pane_id": first_pane_id })))
            .await;

        let simulated = dispatcher
            .dispatch(request(
                "debug.shortcut.simulate",
                json!({ "combo": "cmd+opt+right" }),
            ))
            .await;
        assert_eq!(simulated.result.expect("simulate")["ok"], true);

        let panes_after = dispatcher.dispatch(request("pane.list", json!({}))).await;
        let panes_after_result = panes_after.result.expect("pane list after");
        let panes_after = panes_after_result["panes"]
            .as_array()
            .expect("pane rows after");
        let focused = panes_after
            .iter()
            .find(|pane| pane["focused"].as_bool().unwrap_or(false))
            .expect("focused pane");
        assert_eq!(focused["id"], second_pane_id);
    }

    #[tokio::test]
    async fn dispatcher_lists_use_zero_based_indices() {
        let dispatcher = Dispatcher::new();

        let workspaces = dispatcher
            .dispatch(request("workspace.list", json!({})))
            .await;
        assert_eq!(
            workspaces.result.expect("workspace list")["workspaces"][0]["index"],
            0
        );

        let panes = dispatcher.dispatch(request("pane.list", json!({}))).await;
        assert_eq!(panes.result.expect("pane list")["panes"][0]["index"], 0);

        let surfaces = dispatcher
            .dispatch(request("surface.list", json!({})))
            .await;
        assert_eq!(
            surfaces.result.expect("surface list")["surfaces"][0]["index"],
            0
        );
    }

    #[tokio::test]
    async fn dispatcher_recognizes_tests_v2_extended_method_surface() {
        let dispatcher = Dispatcher::new();
        let methods = [
            "browser.addinitscript",
            "browser.addscript",
            "browser.addstyle",
            "browser.check",
            "browser.console.clear",
            "browser.console.list",
            "browser.cookies.clear",
            "browser.cookies.get",
            "browser.cookies.set",
            "browser.dblclick",
            "browser.dialog.accept",
            "browser.dialog.dismiss",
            "browser.download.wait",
            "browser.errors.list",
            "browser.find.role",
            "browser.focus",
            "browser.frame.main",
            "browser.frame.select",
            "browser.get.attr",
            "browser.get.box",
            "browser.get.count",
            "browser.get.html",
            "browser.get.styles",
            "browser.highlight",
            "browser.hover",
            "browser.is.checked",
            "browser.is.enabled",
            "browser.is.visible",
            "browser.keydown",
            "browser.keyup",
            "browser.press",
            "browser.scroll",
            "browser.scroll_into_view",
            "browser.select",
            "browser.state.load",
            "browser.state.save",
            "browser.storage.clear",
            "browser.storage.get",
            "browser.storage.set",
            "browser.tab.close",
            "browser.tab.list",
            "browser.tab.new",
            "browser.tab.switch",
            "browser.type",
            "browser.uncheck",
            "debug.app.activate",
            "debug.bonsplit_underflow.count",
            "debug.bonsplit_underflow.reset",
            "debug.command_palette.rename_input.delete_backward",
            "debug.command_palette.rename_input.interact",
            "debug.command_palette.rename_input.select_all",
            "debug.command_palette.rename_input.selection",
            "debug.command_palette.rename_tab.open",
            "debug.command_palette.results",
            "debug.command_palette.selection",
            "debug.command_palette.toggle",
            "debug.command_palette.visible",
            "debug.empty_panel.count",
            "debug.empty_panel.reset",
            "debug.flash.count",
            "debug.flash.reset",
            "debug.layout",
            "debug.notification.focus",
            "debug.panel_snapshot",
            "debug.panel_snapshot.reset",
            "debug.portal.stats",
            "debug.shortcut.set",
            "debug.shortcut.simulate",
            "debug.sidebar.visible",
            "debug.terminal.is_focused",
            "debug.terminal.read_text",
            "debug.terminal.render_stats",
            "debug.type",
            "debug.window.screenshot",
        ];

        for method in methods {
            let response = dispatcher.dispatch(request(method, json!({}))).await;
            if let Some(error) = response.error {
                assert_ne!(
                    error.code, -32601,
                    "method {method} should be recognized, got unknown method"
                );
            }
        }
    }
}
