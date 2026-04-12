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

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "tab_kind", rename_all = "snake_case")]
pub enum TabContentState {
    Terminal {
        #[serde(default)]
        cwd: Option<String>,
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
            active_tab_id: Some(tab.id.clone()),
            tabs: vec![tab],
        }
    }

    pub fn browser_only(uri: Option<&str>) -> Self {
        let tab = TabState::browser(default_tab_id("browser"), uri);
        Self {
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
                    name: workspace.name,
                    favorite: workspace.favorite,
                    cwd: workspace.cwd,
                    folder_path: workspace.folder_path,
                    // Legacy files only knew "workspace exists"; rehydrate a fresh terminal at the
                    // last known directory instead of pretending process state can be restored.
                    layout: LayoutNodeState::Pane(PaneState {
                        active_tab_id: Some(tab.id.clone()),
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
            TabContentState::Terminal { cwd } => {
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
        assert_eq!(decoded.workspaces[0].name, "workspace");
    }

    #[test]
    fn normalize_layout_falls_back_to_first_tab_when_active_tab_is_stale() {
        let mut layout = LayoutNodeState::Pane(PaneState {
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
            active_tab_id: None,
            tabs: Vec::new(),
        });

        normalize_layout(&mut layout, Some("/tmp/project"));

        let LayoutNodeState::Pane(pane) = layout else {
            panic!("expected pane");
        };
        assert_eq!(pane.tabs.len(), 1);
        match &pane.tabs[0].content {
            TabContentState::Terminal { cwd } => {
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
                name: "workspace".to_string(),
                favorite: false,
                cwd: None,
                folder_path: None,
                layout: LayoutNodeState::Pane(PaneState {
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
