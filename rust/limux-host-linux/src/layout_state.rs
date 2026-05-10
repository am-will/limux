use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const SESSION_VERSION: u32 = 1;
pub const PERSISTENCE_DIR_NAME: &str = "limux";
pub const SESSION_FILE_NAME: &str = "session.json";
pub const LEGACY_WORKSPACES_FILE_NAME: &str = "workspaces.json";
pub const DEFAULT_SIDEBAR_WIDTH: i32 = 220;
pub const DEFAULT_SPLIT_RATIO: f64 = 0.5;
const MIN_SPLIT_RATIO: f64 = 0.02;
const MAX_SPLIT_RATIO: f64 = 0.98;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionLoadSource {
    Canonical,
    Legacy,
    Empty,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedSession {
    pub state: AppSessionState,
    pub source: SessionLoadSource,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SidebarState {
    #[serde(default = "default_sidebar_visible")]
    pub visible: bool,
    #[serde(default = "default_sidebar_width")]
    pub width: i32,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub struct AppSessionState {
    #[serde(default = "default_session_version")]
    pub version: u32,
    #[serde(default)]
    pub active_workspace_index: usize,
    #[serde(default = "default_top_bar_visible")]
    pub top_bar_visible: bool,
    #[serde(default)]
    pub sidebar: SidebarState,
    #[serde(default)]
    pub workspaces: Vec<WorkspaceState>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub struct WorkspaceState {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub favorite: bool,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub folder_path: Option<String>,
    pub layout: LayoutNodeState,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LayoutNodeState {
    Pane(PaneState),
    Split(SplitState),
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub struct SplitState {
    pub orientation: SplitOrientation,
    #[serde(default = "default_split_ratio")]
    pub ratio: f64,
    pub start: Box<LayoutNodeState>,
    pub end: Box<LayoutNodeState>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SplitOrientation {
    Horizontal,
    Vertical,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub struct PaneState {
    #[serde(default)]
    pub pane_id: Option<u32>,
    #[serde(default)]
    pub active_tab_id: Option<String>,
    #[serde(default)]
    pub tabs: Vec<TabState>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub struct TabState {
    pub id: String,
    #[serde(default)]
    pub custom_name: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(flatten)]
    pub content: TabContentState,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RestorableAgentKind {
    Claude,
    Codex,
    OpenCode,
    Gemini,
}

impl RestorableAgentKind {
    pub fn resume_command(
        self,
        session_id: &str,
        launch_command: Option<&AgentLaunchCommandState>,
        cwd: Option<&str>,
    ) -> Option<String> {
        build_resume_command(self, session_id, launch_command, cwd)
    }

    fn fallback_executable(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::Gemini => "gemini",
        }
    }

    fn store_name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::Gemini => "gemini",
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub struct AgentLaunchCommandState {
    pub executable: String,
    #[serde(default)]
    pub arguments: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    #[serde(default)]
    pub captured_at: Option<f64>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub struct RestorableAgentState {
    pub kind: RestorableAgentKind,
    pub session_id: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub launch_command: Option<AgentLaunchCommandState>,
    #[serde(default)]
    pub restore_on_startup: bool,
}

impl RestorableAgentState {
    pub fn resume_command(&self) -> Option<String> {
        if !self.restore_on_startup {
            return None;
        }
        self.kind.resume_command(
            &self.session_id,
            self.launch_command.as_ref(),
            self.cwd.as_deref(),
        )
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "tab_kind", rename_all = "snake_case")]
pub enum TabContentState {
    Terminal {
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        agent: Option<RestorableAgentState>,
    },
    Browser {
        #[serde(default)]
        uri: Option<String>,
    },
    Keybinds {},
    Settings {},
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct LegacySavedWorkspace {
    pub name: String,
    pub favorite: bool,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub folder_path: Option<String>,
}

impl Default for SidebarState {
    fn default() -> Self {
        Self {
            visible: default_sidebar_visible(),
            width: default_sidebar_width(),
        }
    }
}

impl Default for AppSessionState {
    fn default() -> Self {
        Self {
            version: default_session_version(),
            active_workspace_index: 0,
            top_bar_visible: default_top_bar_visible(),
            sidebar: SidebarState::default(),
            workspaces: Vec::new(),
        }
    }
}

impl PaneState {
    pub fn fallback(working_directory: Option<&str>) -> Self {
        let tab = TabState::terminal(default_tab_id("terminal"), working_directory);
        Self {
            pane_id: None,
            active_tab_id: Some(tab.id.clone()),
            tabs: vec![tab],
        }
    }

    pub fn browser_only(uri: Option<&str>) -> Self {
        let tab = TabState::browser(default_tab_id("browser"), uri);
        Self {
            pane_id: None,
            active_tab_id: Some(tab.id.clone()),
            tabs: vec![tab],
        }
    }
}

impl TabState {
    pub fn terminal(id: impl Into<String>, cwd: Option<&str>) -> Self {
        Self {
            id: id.into(),
            custom_name: None,
            pinned: false,
            content: TabContentState::Terminal {
                cwd: cwd.map(|value| value.to_string()),
                agent: None,
            },
        }
    }

    pub fn browser(id: impl Into<String>, uri: Option<&str>) -> Self {
        Self {
            id: id.into(),
            custom_name: None,
            pinned: false,
            content: TabContentState::Browser {
                uri: uri.map(|value| value.to_string()),
            },
        }
    }
}

pub fn persistence_dir() -> PathBuf {
    if let Some(data_dir) = dirs::data_dir() {
        return data_dir.join(PERSISTENCE_DIR_NAME);
    }

    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".local/share").join(PERSISTENCE_DIR_NAME)
}

pub fn canonical_session_path_in(dir: &Path) -> PathBuf {
    dir.join(SESSION_FILE_NAME)
}

pub fn legacy_workspaces_path_in(dir: &Path) -> PathBuf {
    dir.join(LEGACY_WORKSPACES_FILE_NAME)
}

pub fn load_session() -> LoadedSession {
    load_session_from_dir(&persistence_dir())
}

pub fn load_session_from_dir(dir: &Path) -> LoadedSession {
    let canonical_path = canonical_session_path_in(dir);
    if canonical_path.exists() {
        let state = fs::read_to_string(&canonical_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<AppSessionState>(&raw).ok())
            .map(normalize_session)
            .unwrap_or_default();
        return LoadedSession {
            state,
            source: SessionLoadSource::Canonical,
        };
    }

    let legacy_path = legacy_workspaces_path_in(dir);
    if legacy_path.exists() {
        let state = fs::read_to_string(&legacy_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<LegacySavedWorkspace>>(&raw).ok())
            .map(AppSessionState::from_legacy)
            .unwrap_or_default();
        return LoadedSession {
            state,
            source: SessionLoadSource::Legacy,
        };
    }

    LoadedSession {
        state: AppSessionState::default(),
        source: SessionLoadSource::Empty,
    }
}

pub fn save_session_atomic(state: &AppSessionState) -> io::Result<PathBuf> {
    save_session_atomic_in(&persistence_dir(), state)
}

pub fn save_session_atomic_in(dir: &Path, state: &AppSessionState) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = canonical_session_path_in(dir);
    // Write to a sibling temp file first so a crash never leaves a truncated canonical session.
    let temp_path = temp_session_path(&path);
    let normalized = normalize_session(state.clone());
    let json = serde_json::to_vec_pretty(&normalized)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    fs::write(&temp_path, json)?;
    fs::rename(&temp_path, &path)?;
    Ok(path)
}

pub fn clamp_split_ratio(ratio: f64) -> f64 {
    if !ratio.is_finite() {
        return DEFAULT_SPLIT_RATIO;
    }
    ratio.clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO)
}

pub fn split_ratio_from_position(position: i32, total_size: i32) -> f64 {
    if total_size <= 0 {
        return DEFAULT_SPLIT_RATIO;
    }
    clamp_split_ratio(position as f64 / total_size as f64)
}

pub fn snapshot_split_ratio(position: i32, total_size: i32, stored_ratio: Option<f64>) -> f64 {
    if total_size <= 0 {
        return stored_ratio
            .map(clamp_split_ratio)
            .unwrap_or(DEFAULT_SPLIT_RATIO);
    }
    split_ratio_from_position(position, total_size)
}

pub fn split_position_from_ratio(ratio: f64, total_size: i32) -> i32 {
    if total_size <= 0 {
        return 0;
    }
    (clamp_split_ratio(ratio) * total_size as f64).round() as i32
}

pub fn normalize_session(mut state: AppSessionState) -> AppSessionState {
    state.version = SESSION_VERSION;
    state.sidebar.width = state.sidebar.width.max(DEFAULT_SIDEBAR_WIDTH);
    if state.workspaces.is_empty() {
        state.active_workspace_index = 0;
    } else if state.active_workspace_index >= state.workspaces.len() {
        state.active_workspace_index = state.workspaces.len() - 1;
    }
    for workspace in &mut state.workspaces {
        normalize_layout(
            &mut workspace.layout,
            workspace
                .folder_path
                .as_deref()
                .or(workspace.cwd.as_deref()),
        );
    }
    state
}

pub fn normalize_layout(layout: &mut LayoutNodeState, working_directory: Option<&str>) {
    match layout {
        LayoutNodeState::Pane(pane) => {
            if pane.tabs.is_empty() {
                *pane = PaneState::fallback(working_directory);
                return;
            }
            let mut active_exists = false;
            for tab in &pane.tabs {
                if pane.active_tab_id.as_deref() == Some(tab.id.as_str()) {
                    active_exists = true;
                    break;
                }
            }
            if !active_exists {
                pane.active_tab_id = pane.tabs.first().map(|tab| tab.id.clone());
            }
        }
        LayoutNodeState::Split(split) => {
            split.ratio = clamp_split_ratio(split.ratio);
            normalize_layout(&mut split.start, working_directory);
            normalize_layout(&mut split.end, working_directory);
        }
    }
}

impl AppSessionState {
    pub fn from_legacy(workspaces: Vec<LegacySavedWorkspace>) -> Self {
        let workspaces = workspaces
            .into_iter()
            .map(|workspace| {
                let working_directory = workspace
                    .folder_path
                    .as_deref()
                    .or(workspace.cwd.as_deref());
                let tab = TabState::terminal(default_tab_id("legacy-terminal"), working_directory);
                WorkspaceState {
                    id: None,
                    name: workspace.name,
                    favorite: workspace.favorite,
                    cwd: workspace.cwd,
                    folder_path: workspace.folder_path,
                    // Legacy files only knew "workspace exists"; rehydrate a fresh terminal at the
                    // last known directory instead of pretending process state can be restored.
                    layout: LayoutNodeState::Pane(PaneState {
                        active_tab_id: Some(tab.id.clone()),
                        pane_id: None,
                        tabs: vec![tab],
                    }),
                }
            })
            .collect();
        normalize_session(Self {
            workspaces,
            ..Self::default()
        })
    }
}

#[derive(serde::Deserialize)]
struct HookSessionRecord {
    session_id: String,
    workspace_id: String,
    surface_id: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    launch_command: Option<AgentLaunchCommandState>,
    updated_at: f64,
}

#[derive(serde::Deserialize)]
struct HookSessionFile {
    #[serde(default)]
    sessions: BTreeMap<String, HookSessionRecord>,
}

#[derive(Clone, Debug, Default)]
pub struct RestorableAgentIndex {
    by_surface: HashMap<(String, String), (RestorableAgentState, f64)>,
    by_any_workspace_surface: HashMap<String, (RestorableAgentState, f64)>,
    by_tab_id: HashMap<String, (RestorableAgentState, f64)>,
}

impl RestorableAgentIndex {
    pub fn load() -> Self {
        Self::load_from_dir(&agent_hook_state_dir())
    }

    pub fn load_from_dir(dir: &Path) -> Self {
        let mut index = Self::default();
        for (kind, file_name) in [
            (RestorableAgentKind::Claude, "claude-hook-sessions.json"),
            (RestorableAgentKind::Codex, "codex-hook-sessions.json"),
            (RestorableAgentKind::OpenCode, "opencode-hook-sessions.json"),
            (RestorableAgentKind::Gemini, "gemini-hook-sessions.json"),
        ] {
            let path = dir.join(file_name);
            let Ok(raw) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(file) = serde_json::from_str::<HookSessionFile>(&raw) else {
                continue;
            };
            for record in file.sessions.values() {
                let Some(session_id) = normalized_str(&record.session_id) else {
                    continue;
                };
                let Some(workspace_id) = normalized_str(&record.workspace_id) else {
                    continue;
                };
                let Some(surface_id) = normalized_str(&record.surface_id) else {
                    continue;
                };
                let tab_id = surface_id
                    .rsplit_once(':')
                    .map(|(_, tab_id)| tab_id.to_string());
                let key = (workspace_id, surface_id);
                if index
                    .by_surface
                    .get(&key)
                    .is_some_and(|(_, updated_at)| *updated_at > record.updated_at)
                {
                    continue;
                }
                index.by_surface.insert(
                    key.clone(),
                    (
                        RestorableAgentState {
                            kind,
                            session_id: session_id.clone(),
                            cwd: record.cwd.clone(),
                            launch_command: record.launch_command.clone(),
                            restore_on_startup: true,
                        },
                        record.updated_at,
                    ),
                );
                let latest_for_surface = index
                    .by_any_workspace_surface
                    .get(&key.1)
                    .is_some_and(|(_, updated_at)| *updated_at > record.updated_at);
                if !latest_for_surface {
                    index.by_any_workspace_surface.insert(
                        key.1,
                        (
                            RestorableAgentState {
                                kind,
                                session_id: session_id.clone(),
                                cwd: record.cwd.clone(),
                                launch_command: record.launch_command.clone(),
                                restore_on_startup: true,
                            },
                            record.updated_at,
                        ),
                    );
                }
                if let Some(tab_id) = tab_id {
                    let latest_for_tab = index
                        .by_tab_id
                        .get(&tab_id)
                        .is_some_and(|(_, updated_at)| *updated_at > record.updated_at);
                    if !latest_for_tab {
                        index.by_tab_id.insert(
                            tab_id,
                            (
                                RestorableAgentState {
                                    kind,
                                    session_id: session_id.clone(),
                                    cwd: record.cwd.clone(),
                                    launch_command: record.launch_command.clone(),
                                    restore_on_startup: true,
                                },
                                record.updated_at,
                            ),
                        );
                    }
                }
            }
        }
        index
    }

    pub fn agent_for_surface(
        &self,
        workspace_id: &str,
        pane_id: Option<u32>,
        tab_id: &str,
    ) -> Option<RestorableAgentState> {
        let surface_id = pane_id.map(|pane_id| format!("{pane_id}:{tab_id}"));
        surface_id
            .as_ref()
            .and_then(|surface_id| {
                self.by_surface
                    .get(&(workspace_id.to_string(), surface_id.clone()))
                    .or_else(|| self.by_any_workspace_surface.get(surface_id))
            })
            .or_else(|| self.by_tab_id.get(tab_id))
            .map(|(agent, _)| agent.clone())
    }
}

pub fn attach_restorable_agents_to_layout(
    layout: &mut LayoutNodeState,
    workspace_id: &str,
    index: &RestorableAgentIndex,
) {
    match layout {
        LayoutNodeState::Pane(pane) => {
            for tab in &mut pane.tabs {
                if let TabContentState::Terminal { agent, .. } = &mut tab.content {
                    if agent
                        .as_ref()
                        .is_some_and(|agent| !agent.restore_on_startup)
                    {
                        continue;
                    }
                    if let Some(restored_agent) =
                        index.agent_for_surface(workspace_id, pane.pane_id, &tab.id)
                    {
                        *agent = Some(restored_agent);
                    }
                }
            }
        }
        LayoutNodeState::Split(split) => {
            attach_restorable_agents_to_layout(&mut split.start, workspace_id, index);
            attach_restorable_agents_to_layout(&mut split.end, workspace_id, index);
        }
    }
}

fn agent_hook_state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("LIMUX_AGENT_HOOK_STATE_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(dir) = dirs::state_dir() {
        return dir.join(PERSISTENCE_DIR_NAME);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local/state")
        .join(PERSISTENCE_DIR_NAME)
}

fn temp_session_path(path: &Path) -> PathBuf {
    let temp_name = format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or(SESSION_FILE_NAME),
        std::process::id()
    );
    path.with_file_name(temp_name)
}

fn default_session_version() -> u32 {
    SESSION_VERSION
}

fn default_sidebar_visible() -> bool {
    true
}

fn default_top_bar_visible() -> bool {
    true
}

fn default_sidebar_width() -> i32 {
    DEFAULT_SIDEBAR_WIDTH
}

fn default_split_ratio() -> f64 {
    DEFAULT_SPLIT_RATIO
}

fn default_tab_id(prefix: &str) -> String {
    format!("{prefix}-0")
}

fn build_resume_command(
    kind: RestorableAgentKind,
    session_id: &str,
    launch: Option<&AgentLaunchCommandState>,
    cwd: Option<&str>,
) -> Option<String> {
    let session_id = normalized_str(session_id)?;
    let fallback = kind.fallback_executable().to_string();
    let args = launch
        .map(|launch| launch.arguments.clone())
        .filter(|args| !args.is_empty())
        .unwrap_or_else(|| vec![fallback.clone()]);
    let sanitized = sanitize_launch_arguments(kind, &args);
    let executable = launch
        .and_then(|launch| normalized_str(&launch.executable))
        .or_else(|| sanitized.first().cloned())
        .unwrap_or(fallback);
    let preserved_tail = sanitized
        .get(1..)
        .map(|tail| tail.to_vec())
        .unwrap_or_default();

    let mut parts = vec![executable];
    match kind {
        RestorableAgentKind::Codex => {
            parts.push("resume".to_string());
            parts.extend(preserved_tail);
            parts.push(session_id.clone());
        }
        RestorableAgentKind::OpenCode => {
            parts.push("--session".to_string());
            parts.push(session_id.clone());
            parts.extend(preserved_tail);
        }
        RestorableAgentKind::Claude | RestorableAgentKind::Gemini => {
            parts.push("--resume".to_string());
            parts.push(session_id.clone());
            parts.extend(preserved_tail);
        }
    }

    let command = parts
        .iter()
        .map(|part| shell_single_quote(part))
        .collect::<Vec<_>>()
        .join(" ");
    let cwd = cwd.and_then(normalized_str).or_else(|| {
        launch
            .and_then(|launch| launch.cwd.as_deref())
            .and_then(normalized_str)
    });
    let run_command = match cwd {
        Some(cwd) => format!("cd {} && {command}", shell_single_quote(&cwd)),
        None => command,
    };
    Some(wrap_restored_agent_command(kind, &session_id, &run_command))
}

fn wrap_restored_agent_command(
    kind: RestorableAgentKind,
    session_id: &str,
    run_command: &str,
) -> String {
    let payload = format!(
        "{{\"session_id\":{},\"hook_event_name\":\"Cleanup\"}}",
        serde_json::to_string(session_id).unwrap_or_else(|_| "\"\"".to_string())
    );
    let cleanup = format!(
        "printf %s {} | {} --json hooks {} cleanup >/dev/null 2>&1 || true",
        shell_single_quote(&payload),
        shell_single_quote(&limux_cli_executable()),
        kind.store_name()
    );
    format!("{run_command}; limux_agent_status=$?; {cleanup}; exec \"${{SHELL:-/bin/sh}}\" -l")
}

fn limux_cli_executable() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| {
            let candidate = path.with_file_name("limux-cli");
            candidate.exists().then_some(candidate)
        })
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|| "limux-cli".to_string())
}

fn sanitize_launch_arguments(kind: RestorableAgentKind, arguments: &[String]) -> Vec<String> {
    if arguments.is_empty() {
        return vec![kind.fallback_executable().to_string()];
    }
    let mut result = Vec::new();
    let mut index = 0;
    while index < arguments.len() {
        let arg = &arguments[index];
        if index == 0 {
            result.push(arg.clone());
            index += 1;
            continue;
        }
        if is_resume_selector(kind, arg) || option_takes_secret_value(arg) {
            index += 1;
            if index < arguments.len() && !arguments[index].starts_with('-') {
                index += 1;
            }
            continue;
        }
        if option_is_secret_assignment(arg) {
            index += 1;
            continue;
        }
        if option_takes_safe_value(arg) {
            result.push(arg.clone());
            if index + 1 < arguments.len() {
                result.push(arguments[index + 1].clone());
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        if option_is_safe_flag_or_assignment(arg) {
            result.push(arg.clone());
            index += 1;
            continue;
        }
        if arg.starts_with('-') {
            index += 1;
            continue;
        }
        break;
    }
    result
}

fn is_resume_selector(kind: RestorableAgentKind, arg: &str) -> bool {
    match kind {
        RestorableAgentKind::Codex => {
            arg == "resume" || arg == "--resume" || arg.starts_with("--resume=")
        }
        RestorableAgentKind::OpenCode => arg == "--session" || arg.starts_with("--session="),
        RestorableAgentKind::Claude | RestorableAgentKind::Gemini => {
            arg == "--resume" || arg.starts_with("--resume=") || arg == "--continue"
        }
    }
}

fn option_takes_secret_value(arg: &str) -> bool {
    matches!(
        arg,
        "--api-key" | "--apikey" | "--token" | "--auth-token" | "--password"
    )
}

fn option_is_secret_assignment(arg: &str) -> bool {
    let lower = arg.to_ascii_lowercase();
    lower.starts_with("--api-key=")
        || lower.starts_with("--apikey=")
        || lower.starts_with("--token=")
        || lower.starts_with("--auth-token=")
        || lower.starts_with("--password=")
}

fn option_takes_safe_value(arg: &str) -> bool {
    matches!(
        arg,
        "--model"
            | "-m"
            | "--config"
            | "-c"
            | "--profile"
            | "--sandbox"
            | "--approval-policy"
            | "--cwd"
            | "--cd"
            | "--working-directory"
    )
}

fn option_is_safe_flag_or_assignment(arg: &str) -> bool {
    if matches!(
        arg,
        "--dangerously-bypass-approvals-and-sandbox" | "--dangerously-skip-permissions"
    ) {
        return true;
    }
    let Some((name, _)) = arg.split_once('=') else {
        return false;
    };
    option_takes_safe_value(name)
}

fn normalized_str(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::tempdir;

    static ENV_TEST_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        key: &'static str,
        old: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: Option<&str>) -> Self {
            let old = std::env::var_os(key);
            match value {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
            Self { key, old }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.old {
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    #[test]
    fn persistence_dir_uses_xdg_data_home_directly() {
        let _lock = ENV_TEST_LOCK.lock().expect("env test lock");
        let _xdg = EnvGuard::set("XDG_DATA_HOME", Some("/tmp/limux-xdg-data"));
        let _home = EnvGuard::set("HOME", Some("/tmp/limux-home"));

        assert_eq!(
            persistence_dir(),
            PathBuf::from("/tmp/limux-xdg-data").join(PERSISTENCE_DIR_NAME)
        );
    }

    #[test]
    fn persistence_dir_falls_back_to_home_local_share_when_data_dir_missing() {
        let _lock = ENV_TEST_LOCK.lock().expect("env test lock");
        let _xdg = EnvGuard::set("XDG_DATA_HOME", None);
        let _home = EnvGuard::set("HOME", Some("/tmp/limux-home"));

        assert_eq!(
            persistence_dir(),
            PathBuf::from("/tmp/limux-home/.local/share").join(PERSISTENCE_DIR_NAME)
        );
    }
    #[test]
    fn load_prefers_canonical_session_over_legacy() {
        let dir = tempdir().expect("tempdir");
        let canonical_path = canonical_session_path_in(dir.path());
        let legacy_path = legacy_workspaces_path_in(dir.path());

        let canonical = AppSessionState {
            workspaces: vec![WorkspaceState {
                id: Some("11111111-1111-4111-8111-111111111111".to_string()),
                name: "canonical".to_string(),
                favorite: true,
                cwd: Some("/canonical".to_string()),
                folder_path: Some("/canonical".to_string()),
                layout: LayoutNodeState::Pane(PaneState::fallback(Some("/canonical"))),
            }],
            ..AppSessionState::default()
        };
        fs::write(
            &canonical_path,
            serde_json::to_string_pretty(&canonical).expect("canonical json"),
        )
        .expect("write canonical");
        fs::write(
            &legacy_path,
            serde_json::to_string_pretty(&vec![LegacySavedWorkspace {
                name: "legacy".to_string(),
                favorite: false,
                cwd: Some("/legacy".to_string()),
                folder_path: None,
            }])
            .expect("legacy json"),
        )
        .expect("write legacy");

        let loaded = load_session_from_dir(dir.path());
        assert_eq!(loaded.source, SessionLoadSource::Canonical);
        assert_eq!(loaded.state.workspaces[0].name, "canonical");
    }

    #[test]
    fn load_migrates_legacy_workspaces_when_canonical_missing() {
        let dir = tempdir().expect("tempdir");
        let legacy_path = legacy_workspaces_path_in(dir.path());
        fs::write(
            &legacy_path,
            serde_json::to_string_pretty(&vec![LegacySavedWorkspace {
                name: "legacy".to_string(),
                favorite: true,
                cwd: Some("/tmp/project".to_string()),
                folder_path: None,
            }])
            .expect("legacy json"),
        )
        .expect("write legacy");

        let loaded = load_session_from_dir(dir.path());
        assert_eq!(loaded.source, SessionLoadSource::Legacy);
        assert_eq!(loaded.state.workspaces.len(), 1);
        assert_eq!(loaded.state.workspaces[0].name, "legacy");
        let LayoutNodeState::Pane(pane) = &loaded.state.workspaces[0].layout else {
            panic!("legacy migration should create a pane layout");
        };
        assert_eq!(pane.tabs.len(), 1);
        match &pane.tabs[0].content {
            TabContentState::Terminal { cwd, .. } => {
                assert_eq!(cwd.as_deref(), Some("/tmp/project"));
            }
            other => panic!("expected terminal tab, got {other:?}"),
        }
    }

    #[test]
    fn load_returns_empty_state_for_corrupt_canonical_file() {
        let dir = tempdir().expect("tempdir");
        let canonical_path = canonical_session_path_in(dir.path());
        fs::write(&canonical_path, "{not-json").expect("write corrupt canonical");

        let loaded = load_session_from_dir(dir.path());
        assert_eq!(loaded.source, SessionLoadSource::Canonical);
        assert_eq!(loaded.state, AppSessionState::default());
    }

    #[test]
    fn load_defaults_top_bar_visible_when_omitted_from_session_json() {
        let dir = tempdir().expect("tempdir");
        let canonical_path = canonical_session_path_in(dir.path());
        fs::write(
            &canonical_path,
            r#"{
                "version": 1,
                "active_workspace_index": 0,
                "sidebar": {
                    "visible": true,
                    "width": 220
                },
                "workspaces": []
            }"#,
        )
        .expect("write canonical");

        let loaded = load_session_from_dir(dir.path());
        assert!(loaded.state.top_bar_visible);
    }

    #[test]
    fn save_session_atomic_writes_canonical_file() {
        let dir = tempdir().expect("tempdir");
        let state = AppSessionState {
            workspaces: vec![WorkspaceState {
                id: Some("22222222-2222-4222-8222-222222222222".to_string()),
                name: "workspace".to_string(),
                favorite: false,
                cwd: Some("/tmp".to_string()),
                folder_path: Some("/tmp".to_string()),
                layout: LayoutNodeState::Pane(PaneState::fallback(Some("/tmp"))),
            }],
            ..AppSessionState::default()
        };

        let path = save_session_atomic_in(dir.path(), &state).expect("save canonical session");
        assert_eq!(path, canonical_session_path_in(dir.path()));
        let raw = fs::read_to_string(path).expect("read canonical session");
        let decoded: AppSessionState =
            serde_json::from_str(&raw).expect("decode canonical session");
        assert_eq!(decoded.version, SESSION_VERSION);
        assert_eq!(
            decoded.workspaces[0].id.as_deref(),
            Some("22222222-2222-4222-8222-222222222222")
        );
        assert_eq!(decoded.workspaces[0].name, "workspace");
    }

    #[test]
    fn workspace_id_defaults_for_legacy_session_json() {
        let raw = r#"{
            "version": 1,
            "workspaces": [{
                "name": "legacy-shape",
                "favorite": false,
                "layout": {
                    "kind": "pane",
                    "active_tab_id": "terminal-0",
                    "tabs": [{
                        "id": "terminal-0",
                        "tab_kind": "terminal",
                        "cwd": "/tmp/project"
                    }]
                }
            }]
        }"#;

        let decoded: AppSessionState = serde_json::from_str(raw).expect("decode legacy shape");
        assert_eq!(decoded.workspaces[0].id, None);
    }

    #[test]
    fn hook_index_attaches_agent_to_matching_workspace_surface() {
        let dir = tempdir().expect("tempdir");
        fs::write(
            dir.path().join("codex-hook-sessions.json"),
            r#"{
                "version": 1,
                "sessions": {
                    "session-a": {
                        "session_id": "session-a",
                        "workspace_id": "workspace-a",
                        "surface_id": "42:tab-a",
                        "cwd": "/tmp/project",
                        "pid": 1,
                        "launch_command": {
                            "executable": "codex",
                            "arguments": ["codex"],
                            "cwd": "/tmp/project",
                            "environment": {},
                            "captured_at": 10.0
                        },
                        "updated_at": 10.0
                    }
                }
            }"#,
        )
        .expect("write hook state");
        let index = RestorableAgentIndex::load_from_dir(dir.path());
        let mut layout = LayoutNodeState::Pane(PaneState {
            pane_id: Some(42),
            active_tab_id: Some("tab-a".to_string()),
            tabs: vec![TabState::terminal("tab-a", Some("/tmp/project"))],
        });

        attach_restorable_agents_to_layout(&mut layout, "workspace-a", &index);

        let LayoutNodeState::Pane(pane) = layout else {
            panic!("expected pane");
        };
        match &pane.tabs[0].content {
            TabContentState::Terminal { agent, .. } => {
                let agent = agent.as_ref().expect("agent metadata");
                assert_eq!(agent.kind, RestorableAgentKind::Codex);
                assert_eq!(agent.session_id, "session-a");
                let command = agent.resume_command().expect("resume command");
                assert!(command.contains("cd '/tmp/project' && 'codex' 'resume' 'session-a'"));
                assert!(command.contains("hooks codex cleanup"));
            }
            other => panic!("expected terminal tab, got {other:?}"),
        }
    }

    #[test]
    fn hook_index_falls_back_to_surface_when_workspace_id_drifted() {
        let dir = tempdir().expect("tempdir");
        fs::write(
            dir.path().join("codex-hook-sessions.json"),
            r#"{
                "version": 1,
                "sessions": {
                    "session-a": {
                        "session_id": "session-a",
                        "workspace_id": "old-workspace",
                        "surface_id": "42:tab-a",
                        "cwd": "/tmp/project",
                        "pid": 1,
                        "launch_command": {
                            "executable": "codex",
                            "arguments": ["codex"],
                            "cwd": "/tmp/project",
                            "environment": {},
                            "captured_at": 10.0
                        },
                        "updated_at": 10.0
                    }
                }
            }"#,
        )
        .expect("write hook state");
        let index = RestorableAgentIndex::load_from_dir(dir.path());

        let agent = index
            .agent_for_surface("new-workspace", Some(42), "tab-a")
            .expect("agent by surface fallback");
        assert_eq!(agent.kind, RestorableAgentKind::Codex);
        assert_eq!(agent.session_id, "session-a");
    }

    #[test]
    fn hook_index_falls_back_to_tab_id_when_pane_id_is_missing() {
        let dir = tempdir().expect("tempdir");
        fs::write(
            dir.path().join("codex-hook-sessions.json"),
            r#"{
                "version": 1,
                "sessions": {
                    "session-a": {
                        "session_id": "session-a",
                        "workspace_id": "old-workspace",
                        "surface_id": "42:tab-a",
                        "cwd": "/tmp/project",
                        "pid": 1,
                        "launch_command": {
                            "executable": "codex",
                            "arguments": ["codex"],
                            "cwd": "/tmp/project",
                            "environment": {},
                            "captured_at": 10.0
                        },
                        "updated_at": 10.0
                    }
                }
            }"#,
        )
        .expect("write hook state");
        let index = RestorableAgentIndex::load_from_dir(dir.path());

        let agent = index
            .agent_for_surface("new-workspace", None, "tab-a")
            .expect("agent by tab id fallback");
        assert_eq!(agent.kind, RestorableAgentKind::Codex);
        assert_eq!(agent.session_id, "session-a");
    }

    #[test]
    fn hook_merge_preserves_persisted_agent_when_index_misses() {
        let index = RestorableAgentIndex::default();
        let mut layout = LayoutNodeState::Pane(PaneState {
            pane_id: Some(42),
            active_tab_id: Some("tab-a".to_string()),
            tabs: vec![TabState {
                id: "tab-a".to_string(),
                custom_name: None,
                pinned: false,
                content: TabContentState::Terminal {
                    cwd: Some("/tmp/project".to_string()),
                    agent: Some(RestorableAgentState {
                        kind: RestorableAgentKind::Codex,
                        session_id: "persisted-session".to_string(),
                        cwd: Some("/tmp/project".to_string()),
                        launch_command: None,
                        restore_on_startup: true,
                    }),
                },
            }],
        });

        attach_restorable_agents_to_layout(&mut layout, "workspace-a", &index);

        let LayoutNodeState::Pane(pane) = layout else {
            panic!("expected pane");
        };
        match &pane.tabs[0].content {
            TabContentState::Terminal { agent, .. } => {
                assert_eq!(
                    agent.as_ref().map(|agent| agent.session_id.as_str()),
                    Some("persisted-session")
                );
            }
            other => panic!("expected terminal tab, got {other:?}"),
        }
    }

    #[test]
    fn hook_merge_recovers_agent_without_workspace_or_pane_id() {
        let dir = tempdir().expect("tempdir");
        fs::write(
            dir.path().join("codex-hook-sessions.json"),
            r#"{
                "version": 1,
                "sessions": {
                    "session-a": {
                        "session_id": "session-a",
                        "workspace_id": "old-workspace",
                        "surface_id": "42:tab-a",
                        "cwd": "/tmp/project",
                        "pid": 1,
                        "launch_command": {
                            "executable": "codex",
                            "arguments": ["codex"],
                            "cwd": "/tmp/project",
                            "environment": {},
                            "captured_at": 10.0
                        },
                        "updated_at": 10.0
                    }
                }
            }"#,
        )
        .expect("write hook state");
        let index = RestorableAgentIndex::load_from_dir(dir.path());
        let mut layout = LayoutNodeState::Pane(PaneState {
            pane_id: None,
            active_tab_id: Some("tab-a".to_string()),
            tabs: vec![TabState::terminal("tab-a", Some("/tmp/project"))],
        });

        attach_restorable_agents_to_layout(&mut layout, "", &index);

        let LayoutNodeState::Pane(pane) = layout else {
            panic!("expected pane");
        };
        match &pane.tabs[0].content {
            TabContentState::Terminal { agent, .. } => {
                assert_eq!(
                    agent.as_ref().map(|agent| agent.session_id.as_str()),
                    Some("session-a")
                );
            }
            other => panic!("expected terminal tab, got {other:?}"),
        }
    }

    #[test]
    fn terminal_tab_state_round_trips_restorable_agent_metadata() {
        let tab = TabState {
            id: "tab-a".to_string(),
            custom_name: None,
            pinned: false,
            content: TabContentState::Terminal {
                cwd: Some("/tmp/project".to_string()),
                agent: Some(RestorableAgentState {
                    kind: RestorableAgentKind::Codex,
                    session_id: "sess-123".to_string(),
                    cwd: Some("/tmp/project".to_string()),
                    launch_command: Some(AgentLaunchCommandState {
                        executable: "codex".to_string(),
                        arguments: vec![
                            "codex".to_string(),
                            "--model".to_string(),
                            "gpt-5.5".to_string(),
                        ],
                        cwd: Some("/tmp/project".to_string()),
                        environment: Default::default(),
                        captured_at: Some(12.0),
                    }),
                    restore_on_startup: true,
                }),
            },
        };

        let raw = serde_json::to_string(&tab).expect("encode tab");
        let decoded: TabState = serde_json::from_str(&raw).expect("decode tab");

        match decoded.content {
            TabContentState::Terminal { agent, .. } => {
                let agent = agent.expect("agent metadata");
                assert_eq!(agent.kind, RestorableAgentKind::Codex);
                assert_eq!(agent.session_id, "sess-123");
                assert_eq!(
                    agent
                        .launch_command
                        .expect("launch command")
                        .arguments
                        .as_slice(),
                    ["codex", "--model", "gpt-5.5"]
                );
            }
            other => panic!("expected terminal tab, got {other:?}"),
        }
    }

    #[test]
    fn restorable_agent_resume_command_runs_from_cwd() {
        let agent = RestorableAgentState {
            kind: RestorableAgentKind::Codex,
            session_id: "sess-123".to_string(),
            cwd: Some("/tmp/project".to_string()),
            launch_command: Some(AgentLaunchCommandState {
                executable: "codex".to_string(),
                arguments: vec!["codex".to_string()],
                cwd: Some("/tmp/project".to_string()),
                environment: Default::default(),
                captured_at: Some(12.0),
            }),
            restore_on_startup: true,
        };

        let command = agent.resume_command().expect("resume command");
        assert!(command.contains("cd '/tmp/project' && 'codex' 'resume' 'sess-123'"));
        assert!(command.contains("hooks codex cleanup"));
        assert!(command.contains("exec \"${SHELL:-/bin/sh}\" -l"));
    }

    #[test]
    fn legacy_restorable_agent_without_restore_marker_does_not_resume() {
        let agent = RestorableAgentState {
            kind: RestorableAgentKind::Codex,
            session_id: "old-stale-session".to_string(),
            cwd: Some("/tmp/project".to_string()),
            launch_command: None,
            restore_on_startup: false,
        };

        assert_eq!(agent.resume_command(), None);
    }

    #[test]
    fn normalize_layout_falls_back_to_first_tab_when_active_tab_is_stale() {
        let mut layout = LayoutNodeState::Pane(PaneState {
            pane_id: None,
            active_tab_id: Some("missing".to_string()),
            tabs: vec![TabState {
                id: "browser-1".to_string(),
                custom_name: None,
                pinned: false,
                content: TabContentState::Browser {
                    uri: Some("https://example.com".to_string()),
                },
            }],
        });

        normalize_layout(&mut layout, None);

        let LayoutNodeState::Pane(pane) = layout else {
            panic!("expected pane");
        };
        assert_eq!(pane.active_tab_id.as_deref(), Some("browser-1"));
    }

    #[test]
    fn normalize_layout_rebuilds_empty_pane_from_working_directory() {
        let mut layout = LayoutNodeState::Pane(PaneState {
            pane_id: None,
            active_tab_id: None,
            tabs: Vec::new(),
        });

        normalize_layout(&mut layout, Some("/tmp/project"));

        let LayoutNodeState::Pane(pane) = layout else {
            panic!("expected pane");
        };
        assert_eq!(pane.tabs.len(), 1);
        match &pane.tabs[0].content {
            TabContentState::Terminal { cwd, .. } => {
                assert_eq!(cwd.as_deref(), Some("/tmp/project"));
            }
            other => panic!("expected terminal fallback, got {other:?}"),
        }
    }

    #[test]
    fn browser_only_pane_creates_a_single_browser_tab() {
        let pane = PaneState::browser_only(Some("https://example.com"));

        assert_eq!(pane.tabs.len(), 1);
        assert_eq!(pane.active_tab_id.as_deref(), Some("browser-0"));
        match &pane.tabs[0].content {
            TabContentState::Browser { uri } => {
                assert_eq!(uri.as_deref(), Some("https://example.com"));
            }
            other => panic!("expected browser tab, got {other:?}"),
        }
    }

    #[test]
    fn keybind_tab_round_trips_through_session_json() {
        let state = AppSessionState {
            top_bar_visible: false,
            workspaces: vec![WorkspaceState {
                id: Some("33333333-3333-4333-8333-333333333333".to_string()),
                name: "workspace".to_string(),
                favorite: false,
                cwd: None,
                folder_path: None,
                layout: LayoutNodeState::Pane(PaneState {
                    pane_id: None,
                    active_tab_id: Some("keybinds-1".to_string()),
                    tabs: vec![TabState {
                        id: "keybinds-1".to_string(),
                        custom_name: None,
                        pinned: false,
                        content: TabContentState::Keybinds {},
                    }],
                }),
            }],
            ..AppSessionState::default()
        };

        let raw = serde_json::to_string(&state).expect("serialize session");
        let decoded: AppSessionState = serde_json::from_str(&raw).expect("deserialize session");

        assert!(!decoded.top_bar_visible);
        let LayoutNodeState::Pane(pane) = &decoded.workspaces[0].layout else {
            panic!("expected pane");
        };
        assert_eq!(pane.active_tab_id.as_deref(), Some("keybinds-1"));
        assert!(matches!(pane.tabs[0].content, TabContentState::Keybinds {}));
    }

    #[test]
    fn split_ratio_helpers_clamp_invalid_values() {
        assert_eq!(clamp_split_ratio(f64::NAN), DEFAULT_SPLIT_RATIO);
        assert_eq!(split_ratio_from_position(0, 0), DEFAULT_SPLIT_RATIO);
        assert!(split_ratio_from_position(9999, 10) <= MAX_SPLIT_RATIO);
        assert_eq!(split_position_from_ratio(f64::INFINITY, 200), 100);
    }

    #[test]
    fn snapshot_split_ratio_preserves_stored_ratio_when_unallocated() {
        assert_eq!(snapshot_split_ratio(0, 0, Some(0.73)), 0.73);
        assert_eq!(
            snapshot_split_ratio(0, 0, Some(f64::INFINITY)),
            DEFAULT_SPLIT_RATIO
        );
        assert_eq!(snapshot_split_ratio(0, 0, None), DEFAULT_SPLIT_RATIO);
    }
}
