use anyhow::{bail, Context, Result};
#[cfg(test)]
pub(crate) use limux_control::agent_sessions::build_resume_command;
pub(crate) use limux_control::agent_sessions::{
    filter_allowed_environment, normalize_captured_process_args, sanitize_launch_arguments,
    AgentKind, AgentLaunchCommand as AgentLaunchCommandRecord,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, TryLockError};
use std::io::{ErrorKind, Write};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const STORE_LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const STORE_LOCK_RETRY: Duration = Duration::from_millis(10);
const MAX_ENDED_SESSIONS: usize = 256;
const ENDED_SESSION_RETENTION_SECS: f64 = 30.0 * 86_400.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct AgentHookSessionRecord {
    pub(crate) session_id: String,
    pub(crate) workspace_id: String,
    pub(crate) surface_id: String,
    #[serde(default)]
    pub(crate) cwd: Option<String>,
    #[serde(default)]
    pub(crate) pid: Option<u32>,
    #[serde(default)]
    pub(crate) launch_command: Option<AgentLaunchCommandRecord>,
    pub(crate) updated_at: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct AgentHookSessionFile {
    version: u32,
    #[serde(default)]
    sessions: BTreeMap<String, AgentHookSessionRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    ended_sessions: Vec<EndedAgentHookSession>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct EndedAgentHookSession {
    session_id: String,
    workspace_id: String,
    surface_id: String,
    updated_at: f64,
}

pub(crate) struct AgentHookSessionStore {
    path: PathBuf,
    session_dir: PathBuf,
}

impl AgentHookSessionStore {
    pub(crate) fn new(agent: AgentKind) -> Self {
        Self::new_for_agent_name(agent.store_name())
    }

    pub(crate) fn new_for_agent_name(agent: &str) -> Self {
        let filename = format!("{}-hook-sessions.json", safe_store_name(agent));
        let session_dir = session_persistence_dir();
        if let Some(dir) =
            std::env::var_os("LIMUX_AGENT_HOOK_STATE_DIR").filter(|value| !value.is_empty())
        {
            let dir = PathBuf::from(dir);
            return Self {
                path: dir.join(filename),
                session_dir,
            };
        }
        Self {
            path: state_dir().join(filename),
            session_dir,
        }
    }

    #[cfg(test)]
    pub(crate) fn new_for_dir(agent: &str, dir: &Path) -> Self {
        Self {
            path: dir.join(format!("{}-hook-sessions.json", safe_store_name(agent))),
            session_dir: dir.to_path_buf(),
        }
    }

    #[cfg(test)]
    fn lookup(&self, session_id: &str) -> Result<Option<AgentHookSessionRecord>> {
        let session_id = normalized(session_id);
        if session_id.is_none() {
            return Ok(None);
        }
        let file = self.load()?;
        Ok(file.sessions.get(session_id.as_deref().unwrap()).cloned())
    }

    pub(crate) fn update(
        &self,
        session_id: &str,
        update: impl FnOnce(Option<&AgentHookSessionRecord>) -> AgentHookSessionRecord,
    ) -> Result<()> {
        let Some(session_id) = normalized(session_id) else {
            return Ok(());
        };
        let _lock = self.lock()?;
        let mut file = self.load()?;
        let record = update(file.sessions.get(&session_id));
        file.ended_sessions.retain(|ended| {
            ended.session_id != session_id
                || tab_identity(&ended.surface_id) != tab_identity(&record.surface_id)
        });
        let session_refs = SessionReferences::load(&self.session_dir);
        prune_ended_sessions(&mut file.ended_sessions, now_seconds(), &session_refs);
        file.version = 1;
        file.sessions.insert(session_id, record);
        self.save(&file)
    }

    pub(crate) fn remove(
        &self,
        session_id: &str,
        workspace_id: Option<&str>,
        surface_id: Option<&str>,
    ) -> Result<()> {
        let Some(session_id) = normalized(session_id) else {
            return Ok(());
        };
        let _lock = self.lock()?;
        let mut file = self.load()?;
        let removed = file.sessions.remove(&session_id);
        let now = now_seconds();
        let surface_id = surface_id.and_then(normalized).or_else(|| {
            removed
                .as_ref()
                .and_then(|record| normalized(&record.surface_id))
        });
        if let Some(surface_id) = surface_id {
            file.ended_sessions.retain(|ended| {
                ended.session_id != session_id
                    || tab_identity(&ended.surface_id) != tab_identity(&surface_id)
            });
            file.ended_sessions.push(EndedAgentHookSession {
                session_id,
                workspace_id: workspace_id
                    .and_then(normalized)
                    .or_else(|| {
                        removed
                            .as_ref()
                            .and_then(|record| normalized(&record.workspace_id))
                    })
                    .unwrap_or_default(),
                surface_id,
                updated_at: now,
            });
        }
        let session_refs = SessionReferences::load(&self.session_dir);
        prune_ended_sessions(&mut file.ended_sessions, now, &session_refs);
        self.save(&file)
    }

    fn lock(&self) -> Result<File> {
        if let Some(parent) = self.path.parent() {
            ensure_private_dir(parent)?;
        }
        // Keep a stable sibling inode: the JSON is replaced by rename, and
        // unlinking the lock file would let another writer lock a different inode.
        let lock_path = self.path.with_extension("lock");
        let mut opts = File::options();
        opts.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        opts.mode(0o600);
        let lock = opts
            .open(&lock_path)
            .with_context(|| format!("failed to open {}", lock_path.display()))?;
        let deadline = Instant::now() + STORE_LOCK_TIMEOUT;
        loop {
            match lock.try_lock() {
                Ok(()) => return Ok(lock),
                Err(TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        bail!(
                            "timed out acquiring hook store lock {}",
                            lock_path.display()
                        );
                    }
                    std::thread::sleep(STORE_LOCK_RETRY);
                }
                Err(TryLockError::Error(error)) => {
                    return Err(error)
                        .with_context(|| format!("failed to lock {}", lock_path.display()));
                }
            }
        }
    }

    fn load(&self) -> Result<AgentHookSessionFile> {
        if !self.path.exists() {
            return Ok(AgentHookSessionFile {
                version: 1,
                sessions: BTreeMap::new(),
                ended_sessions: Vec::new(),
            });
        }
        let raw = fs::read_to_string(&self.path)
            .with_context(|| format!("failed to read {}", self.path.display()))?;
        let mut file: AgentHookSessionFile = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse {}", self.path.display()))?;
        if file.version == 0 {
            file.version = 1;
        }
        Ok(file)
    }

    fn save(&self, file: &AgentHookSessionFile) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            ensure_private_dir(parent)?;
        }
        let temp = self
            .path
            .with_extension(format!("json.{}.tmp", std::process::id()));
        let json = serde_json::to_vec_pretty(file).context("failed to encode hook store")?;
        write_private_file(&temp, &json)?;
        fs::rename(&temp, &self.path)
            .with_context(|| format!("failed to replace {}", self.path.display()))?;
        Ok(())
    }
}

fn ensure_private_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .with_context(|| format!("failed to create {}", dir.display()))
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))
    }
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut opts = File::options();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    opts.mode(0o600);
    let mut file = opts
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("failed to write {}", path.display()))
}

enum SessionReferences {
    Known {
        tab_ids: BTreeSet<String>,
        session_ids: BTreeSet<String>,
    },
    Unknown,
}

impl SessionReferences {
    fn load(session_dir: &Path) -> Self {
        let entries = match fs::read_dir(session_dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == ErrorKind::NotFound => {
                return Self::Known {
                    tab_ids: BTreeSet::new(),
                    session_ids: BTreeSet::new(),
                };
            }
            Err(_) => return Self::Unknown,
        };

        let mut candidate_paths = Vec::new();
        for entry in entries {
            let Ok(entry) = entry else {
                return Self::Unknown;
            };
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name == "session.json"
                || (name.starts_with("session-recovery-") && name.ends_with(".json"))
            {
                candidate_paths.push(entry.path());
            }
        }

        let mut tab_ids = BTreeSet::new();
        let mut session_ids = BTreeSet::new();
        for path in candidate_paths {
            let raw = match fs::read_to_string(&path) {
                Ok(raw) => raw,
                Err(err) if err.kind() == ErrorKind::NotFound => continue,
                Err(_) => return Self::Unknown,
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
                return Self::Unknown;
            };
            if collect_session_value_references(&value, &mut tab_ids, &mut session_ids).is_err() {
                return Self::Unknown;
            }
        }

        Self::Known {
            tab_ids,
            session_ids,
        }
    }

    fn is_referenced(&self, entry: &EndedAgentHookSession) -> bool {
        match self {
            Self::Unknown => true,
            Self::Known {
                tab_ids,
                session_ids,
            } => {
                let tab_id = tab_identity(&entry.surface_id);
                (!tab_id.is_empty() && tab_ids.contains(tab_id))
                    || session_ids.contains(entry.session_id.trim())
            }
        }
    }
}

fn collect_session_value_references(
    value: &serde_json::Value,
    tab_ids: &mut BTreeSet<String>,
    session_ids: &mut BTreeSet<String>,
) -> std::result::Result<(), ()> {
    let root = value.as_object().ok_or(())?;
    let Some(workspaces) = root.get("workspaces") else {
        return Ok(());
    };
    for workspace in workspaces.as_array().ok_or(())? {
        let workspace_obj = workspace.as_object().ok_or(())?;
        let layout = workspace_obj.get("layout").ok_or(())?;
        collect_layout_value_references(layout, tab_ids, session_ids)?;
    }
    Ok(())
}

fn collect_layout_value_references(
    layout: &serde_json::Value,
    tab_ids: &mut BTreeSet<String>,
    session_ids: &mut BTreeSet<String>,
) -> std::result::Result<(), ()> {
    let layout_obj = layout.as_object().ok_or(())?;
    match layout_obj.get("kind").and_then(serde_json::Value::as_str) {
        Some("pane") => {
            if let Some(tabs) = layout_obj.get("tabs") {
                for tab in tabs.as_array().ok_or(())? {
                    let tab_obj = tab.as_object().ok_or(())?;
                    if let Some(id) = tab_obj
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .and_then(normalized)
                    {
                        tab_ids.insert(id);
                    }
                    if let Some(agent) = tab_obj.get("agent").filter(|value| !value.is_null()) {
                        let agent_obj = agent.as_object().ok_or(())?;
                        if let Some(session_id) = agent_obj
                            .get("session_id")
                            .and_then(serde_json::Value::as_str)
                            .and_then(normalized)
                        {
                            session_ids.insert(session_id);
                        }
                    }
                }
            }
            Ok(())
        }
        Some("split") => {
            collect_layout_value_references(
                layout_obj.get("start").ok_or(())?,
                tab_ids,
                session_ids,
            )?;
            collect_layout_value_references(layout_obj.get("end").ok_or(())?, tab_ids, session_ids)
        }
        _ => Err(()),
    }
}

fn prune_ended_sessions(
    ended: &mut Vec<EndedAgentHookSession>,
    now: f64,
    refs: &SessionReferences,
) {
    if now > 0.0 {
        let cutoff = now - ENDED_SESSION_RETENTION_SECS;
        ended.retain(|entry| {
            refs.is_referenced(entry) || entry.updated_at <= 0.0 || entry.updated_at >= cutoff
        });
    }
    if ended.len() > MAX_ENDED_SESSIONS {
        let mut drop_budget = ended.len() - MAX_ENDED_SESSIONS;
        ended.retain(|entry| {
            if drop_budget > 0 && !refs.is_referenced(entry) {
                drop_budget -= 1;
                false
            } else {
                true
            }
        });
    }
}

fn tab_identity(surface_id: &str) -> &str {
    let surface_id = surface_id.trim();
    surface_id.rsplit(':').next().unwrap_or(surface_id)
}

pub(crate) fn launch_record_from_context(
    kind: AgentKind,
    payload_cwd: Option<&str>,
    proc_args: Option<&[String]>,
    proc_cwd: Option<&str>,
    proc_env: Option<&BTreeMap<String, String>>,
) -> Option<AgentLaunchCommandRecord> {
    let args = std::env::var("LIMUX_AGENT_LAUNCH_ARGV")
        .ok()
        .map(split_nul_or_space_separated)
        .filter(|args| !args.is_empty())
        .or_else(|| proc_args.and_then(|raw| normalize_captured_process_args(kind, raw)))
        .unwrap_or_else(|| vec![kind.fallback_executable().to_string()]);
    let sanitized = sanitize_launch_arguments(kind, &args);
    let executable = std::env::var("LIMUX_AGENT_LAUNCH_EXECUTABLE")
        .ok()
        .and_then(|value| normalized(&value))
        .or_else(|| sanitized.first().cloned())?;
    let mut environment = proc_env.cloned().unwrap_or_default();
    environment.extend(filter_allowed_environment(std::env::vars()));

    Some(AgentLaunchCommandRecord {
        executable,
        arguments: sanitized,
        cwd: std::env::var("LIMUX_AGENT_LAUNCH_CWD")
            .ok()
            .and_then(|value| normalized(&value))
            .or_else(|| payload_cwd.and_then(normalized))
            .or_else(|| proc_cwd.and_then(normalized))
            .or_else(|| {
                std::env::var("PWD")
                    .ok()
                    .and_then(|value| normalized(&value))
            }),
        environment,
        captured_at: Some(now_seconds()),
    })
}

pub(crate) fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .unwrap_or_default()
}

fn state_dir() -> PathBuf {
    if let Some(dir) = dirs::state_dir() {
        return dir.join("limux");
    }
    if let Some(home) = dirs::home_dir() {
        return home.join(".local/state/limux");
    }
    PathBuf::from(".limux")
}

fn session_persistence_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("LIMUX_SESSION_DIR").filter(|value| !value.is_empty()) {
        return PathBuf::from(dir);
    }
    if let Some(data_dir) = dirs::data_dir() {
        return data_dir.join("limux");
    }
    if let Some(home) = dirs::home_dir() {
        return home.join(".local/share/limux");
    }
    PathBuf::from(".limux")
}

fn safe_store_name(agent: &str) -> String {
    agent
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
        .collect::<String>()
}

fn normalized(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn split_nul_or_space_separated(raw: String) -> Vec<String> {
    if raw.contains('\0') {
        raw.split('\0').filter_map(normalized).collect::<Vec<_>>()
    } else {
        raw.split_whitespace()
            .filter_map(normalized)
            .collect::<Vec<_>>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn hook_store_round_trips_session_records() {
        let dir = tempdir().expect("tempdir");
        let store = AgentHookSessionStore::new_for_dir("codex", dir.path());
        let record = AgentHookSessionRecord {
            session_id: "codex-session-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            surface_id: "7:tab-a".to_string(),
            cwd: Some("/tmp/project".to_string()),
            pid: Some(1234),
            launch_command: Some(AgentLaunchCommandRecord {
                executable: "codex".to_string(),
                arguments: vec![
                    "codex".to_string(),
                    "--model".to_string(),
                    "gpt-5.5".to_string(),
                ],
                cwd: Some("/tmp/project".to_string()),
                environment: Default::default(),
                captured_at: Some(10.0),
            }),
            updated_at: 11.0,
        };

        store
            .update(&record.session_id, |_| record.clone())
            .expect("update");

        assert_eq!(
            store.lookup("codex-session-1").expect("lookup"),
            Some(record)
        );
    }

    fn test_record(session_id: &str) -> AgentHookSessionRecord {
        AgentHookSessionRecord {
            session_id: session_id.to_string(),
            workspace_id: "workspace".to_string(),
            surface_id: format!("1:{session_id}"),
            cwd: Some("/fresh-cwd".to_string()),
            pid: None,
            launch_command: None,
            updated_at: now_seconds(),
        }
    }

    #[test]
    fn cleanup_durably_invalidates_the_removed_binding() {
        let dir = tempdir().unwrap();
        let store = AgentHookSessionStore::new_for_dir("codex", dir.path());
        store.update("ended", |_| test_record("ended")).unwrap();
        store.remove("ended", None, None).unwrap();
        assert!(store.lookup("ended").unwrap().is_none());
        let json: serde_json::Value =
            serde_json::from_slice(&fs::read(&store.path).unwrap()).unwrap();
        assert_eq!(json["ended_sessions"][0]["session_id"], "ended");
        assert_eq!(json["ended_sessions"][0]["surface_id"], "1:ended");
    }

    #[test]
    fn ended_sessions_are_bounded_to_recent_entries() {
        let dir = tempdir().unwrap();
        let store = AgentHookSessionStore::new_for_dir("codex", dir.path());
        for i in 0..(MAX_ENDED_SESSIONS + 20) {
            let sid = format!("sess-{i}");
            let surf = format!("1:tab-{i}");
            store.remove(&sid, Some("ws"), Some(&surf)).unwrap();
        }
        let persisted = store.load().unwrap();
        assert_eq!(persisted.ended_sessions.len(), MAX_ENDED_SESSIONS);
        assert_eq!(persisted.ended_sessions[0].session_id, "sess-20");
    }

    #[test]
    fn ended_sessions_retain_tombstones_for_open_session_tabs_across_cap_and_cutoff() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("session.json"),
            serde_json::json!({
                "version": 1,
                "workspaces": [{
                    "id": "ws",
                    "name": "Workspace 1",
                    "layout": {
                        "kind": "split",
                        "orientation": "horizontal",
                        "ratio": 0.5,
                        "start": {
                            "kind": "pane",
                            "pane_id": 1,
                            "active_tab_id": "tab-open",
                            "tabs": [{
                                "id": "tab-open",
                                "tab_kind": "terminal",
                                "cwd": "/tmp"
                            }]
                        },
                        "end": {
                            "kind": "pane",
                            "pane_id": 2,
                            "active_tab_id": "tab-other",
                            "tabs": [{
                                "id": "tab-other",
                                "tab_kind": "terminal",
                                "agent": {
                                    "kind": "codex",
                                    "session_id": "sess-persisted",
                                    "restore_on_startup": true
                                }
                            }]
                        }
                    }
                }]
            })
            .to_string(),
        )
        .unwrap();

        let store = AgentHookSessionStore::new_for_dir("codex", dir.path());
        let stale_file = AgentHookSessionFile {
            version: 1,
            sessions: BTreeMap::new(),
            ended_sessions: vec![
                EndedAgentHookSession {
                    session_id: "sess-open".to_string(),
                    workspace_id: "ws".to_string(),
                    surface_id: "1:tab-open".to_string(),
                    updated_at: 10.0,
                },
                EndedAgentHookSession {
                    session_id: "sess-persisted".to_string(),
                    workspace_id: "ws".to_string(),
                    surface_id: "2:closed-tab".to_string(),
                    updated_at: 10.0,
                },
                EndedAgentHookSession {
                    session_id: "sess-stale-unreferenced".to_string(),
                    workspace_id: "ws".to_string(),
                    surface_id: "3:tab-closed".to_string(),
                    updated_at: 10.0,
                },
            ],
        };
        store.save(&stale_file).unwrap();

        for i in 0..(MAX_ENDED_SESSIONS + 20) {
            let sid = format!("sess-extra-{i}");
            let surf = format!("9:tab-extra-{i}");
            store.remove(&sid, Some("ws"), Some(&surf)).unwrap();
        }

        let persisted = store.load().unwrap();
        assert_eq!(persisted.ended_sessions.len(), MAX_ENDED_SESSIONS);
        assert!(persisted
            .ended_sessions
            .iter()
            .any(|entry| entry.session_id == "sess-open"));
        assert!(persisted
            .ended_sessions
            .iter()
            .any(|entry| entry.session_id == "sess-persisted"));
        assert!(!persisted
            .ended_sessions
            .iter()
            .any(|entry| entry.session_id == "sess-stale-unreferenced"));

        // Once the open tabs are closed and session.json no longer references them,
        // subsequent updates prune those stale tombstones normally.
        fs::write(
            dir.path().join("session.json"),
            r#"{"version":1,"workspaces":[]}"#,
        )
        .unwrap();
        store
            .remove("sess-final", Some("ws"), Some("9:tab-final"))
            .unwrap();
        let after_close = store.load().unwrap();
        assert!(!after_close
            .ended_sessions
            .iter()
            .any(|entry| entry.session_id == "sess-open"));
        assert!(!after_close
            .ended_sessions
            .iter()
            .any(|entry| entry.session_id == "sess-persisted"));
    }

    #[test]
    fn ended_sessions_fail_closed_when_session_file_is_unparseable() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("session.json"), "{corrupt").unwrap();
        let store = AgentHookSessionStore::new_for_dir("codex", dir.path());
        store
            .save(&AgentHookSessionFile {
                version: 1,
                sessions: BTreeMap::new(),
                ended_sessions: vec![EndedAgentHookSession {
                    session_id: "sess-keep".to_string(),
                    workspace_id: "ws".to_string(),
                    surface_id: "1:tab-keep".to_string(),
                    updated_at: 10.0,
                }],
            })
            .unwrap();
        store
            .remove("sess-new", Some("ws"), Some("1:tab-new"))
            .unwrap();
        let persisted = store.load().unwrap();
        assert!(persisted
            .ended_sessions
            .iter()
            .any(|entry| entry.session_id == "sess-keep"));
    }

    #[test]
    fn cleanup_without_a_record_keeps_scoped_invalidations_until_matching_upsert() {
        let dir = tempdir().unwrap();
        let store = AgentHookSessionStore::new_for_dir("codex", dir.path());
        store
            .remove("ended", Some("old-workspace"), Some("1:tab-a"))
            .unwrap();
        store
            .remove("ended", Some("other-workspace"), Some("2:tab-b"))
            .unwrap();
        store
            .remove("other", Some("old-workspace"), Some("1:tab-a"))
            .unwrap();
        let persisted = AgentHookSessionStore::new_for_dir("codex", dir.path());
        assert_eq!(persisted.load().unwrap().ended_sessions.len(), 3);

        // Restarting the same tab clears its cleanup after a pane/workspace
        // change, while other sessions and tabs keep their cleanup markers.
        persisted
            .update("ended", |_| {
                let mut record = test_record("ended");
                record.workspace_id = "new-workspace".into();
                record.surface_id = "99:tab-a".into();
                record
            })
            .unwrap();
        let ended = persisted.load().unwrap().ended_sessions;
        assert_eq!(ended.len(), 2);
        assert!(ended
            .iter()
            .any(|record| record.session_id == "ended" && record.surface_id == "2:tab-b"));
        assert!(ended.iter().any(|record| record.session_id == "other"));
    }

    #[test]
    fn launch_record_captures_ancestor_proc_args_cwd_and_env() {
        let proc_args = vec![
            "/usr/local/bin/agy".to_string(),
            "--model".to_string(),
            "gemini-2.5-pro".to_string(),
            "--effort".to_string(),
            "high".to_string(),
            "--api-key".to_string(),
            "super-secret".to_string(),
            "initial prompt".to_string(),
        ];
        let mut proc_env = BTreeMap::new();
        proc_env.insert("AGY_CONFIG_DIR".to_string(), "/tmp/agy-cfg".to_string());

        let record = launch_record_from_context(
            AgentKind::Agy,
            None,
            Some(&proc_args),
            Some("/tmp/proc-cwd"),
            Some(&proc_env),
        )
        .expect("launch record");

        assert_eq!(record.executable, "/usr/local/bin/agy");
        assert_eq!(
            record.arguments,
            vec![
                "/usr/local/bin/agy",
                "--model",
                "gemini-2.5-pro",
                "--effort",
                "high",
            ]
        );
        assert_eq!(record.cwd.as_deref(), Some("/tmp/proc-cwd"));
        assert_eq!(
            record.environment.get("AGY_CONFIG_DIR").map(String::as_str),
            Some("/tmp/agy-cfg")
        );
    }

    fn await_marker(path: &Path) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !path.exists() {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {}",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    struct Worker(std::process::Child);

    impl Worker {
        fn start(dir: &Path, action: &str) -> Self {
            Self(
                std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "agent_hooks::tests::hook_store_process_worker",
                        "--nocapture",
                    ])
                    .env("LIMUX_HOOK_TEST_DIR", dir)
                    .env("LIMUX_HOOK_TEST_ACTION", action)
                    .stdout(std::process::Stdio::null())
                    .spawn()
                    .unwrap(),
            )
        }

        fn finish(&mut self) {
            assert!(self.0.wait().unwrap().success());
        }
    }

    impl Drop for Worker {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    // Run in separate processes so these tests exercise OS lock semantics,
    // including descriptor release on termination, rather than a thread mutex.
    #[test]
    fn hook_store_process_worker() {
        let Ok(dir) = std::env::var("LIMUX_HOOK_TEST_DIR") else {
            return;
        };
        let dir = Path::new(&dir);
        let action = std::env::var("LIMUX_HOOK_TEST_ACTION").unwrap();
        let store = AgentHookSessionStore::new_for_dir("codex", dir);
        if action == "hold" {
            store
                .update("first", |_| {
                    fs::write(dir.join("held"), "").unwrap();
                    await_marker(&dir.join("release"));
                    test_record("first")
                })
                .unwrap();
            return;
        }

        // The first worker has entered its merge closure. Verify it actually
        // holds the kernel lock before allowing the parent to release it.
        let probe = File::options()
            .read(true)
            .write(true)
            .open(store.path.with_extension("lock"))
            .unwrap();
        assert!(matches!(probe.try_lock(), Err(TryLockError::WouldBlock)));
        fs::write(dir.join("contended"), "").unwrap();
        match action.as_str() {
            "insert" => store.update("second", |_| test_record("second")).unwrap(),
            "merge" => store
                .update("first", |existing| {
                    let mut record = existing.expect("must see committed first update").clone();
                    assert_eq!(record.cwd.as_deref(), Some("/fresh-cwd"));
                    record.pid = Some(42);
                    record
                })
                .unwrap(),
            "remove" => store.remove("remove-me", None, None).unwrap(),
            _ => panic!("unknown worker action"),
        }
    }

    #[test]
    fn concurrent_process_transactions_preserve_sessions_merges_and_cleanup() {
        for action in ["insert", "merge", "remove"] {
            let dir = tempdir().unwrap();
            let store = AgentHookSessionStore::new_for_dir("codex", dir.path());
            store
                .update("remove-me", |_| test_record("remove-me"))
                .unwrap();
            let mut first = Worker::start(dir.path(), "hold");
            await_marker(&dir.path().join("held"));
            let mut second = Worker::start(dir.path(), action);
            await_marker(&dir.path().join("contended"));
            fs::write(dir.path().join("release"), "").unwrap();
            first.finish();
            second.finish();

            let first = store.lookup("first").unwrap().unwrap();
            assert_eq!(first.cwd.as_deref(), Some("/fresh-cwd"));
            match action {
                "insert" => assert!(store.lookup("second").unwrap().is_some()),
                "merge" => assert_eq!(first.pid, Some(42)),
                "remove" => assert!(store.lookup("remove-me").unwrap().is_none()),
                _ => unreachable!(),
            }
            if action != "remove" {
                assert!(store.lookup("remove-me").unwrap().is_some());
            }
        }
    }

    #[test]
    fn killed_writer_releases_lock_without_deleting_lock_file() {
        let dir = tempdir().unwrap();
        let store = AgentHookSessionStore::new_for_dir("codex", dir.path());
        let mut holder = Worker::start(dir.path(), "hold");
        await_marker(&dir.path().join("held"));
        holder.0.kill().unwrap();
        holder.0.wait().unwrap();
        assert!(store.path.with_extension("lock").exists());
        store.update("next", |_| test_record("next")).unwrap();
        assert!(store.lookup("next").unwrap().is_some());
    }

    #[test]
    fn lock_timeout_does_not_modify_session_store() {
        let dir = tempdir().unwrap();
        let store = AgentHookSessionStore::new_for_dir("codex", dir.path());
        store.update("saved", |_| test_record("saved")).unwrap();
        let before = fs::read(&store.path).unwrap();
        let _holder = Worker::start(dir.path(), "hold");
        await_marker(&dir.path().join("held"));
        let result = store.update("blocked", |_| panic!("must not merge without lock"));
        assert!(result.unwrap_err().to_string().contains("timed out"));
        assert_eq!(fs::read(&store.path).unwrap(), before);
    }

    #[test]
    fn sanitizer_drops_prompts_credentials_and_existing_resume_selectors() {
        let args = vec![
            "codex".to_string(),
            "--model".to_string(),
            "gpt-5.5".to_string(),
            "--api-key".to_string(),
            "secret".to_string(),
            "resume".to_string(),
            "old-session".to_string(),
            "--dangerously-bypass-approvals-and-sandbox".to_string(),
            "write a prompt".to_string(),
        ];

        let sanitized = sanitize_launch_arguments(AgentKind::Codex, &args);

        assert_eq!(
            sanitized,
            vec![
                "codex".to_string(),
                "--model".to_string(),
                "gpt-5.5".to_string(),
                "--dangerously-bypass-approvals-and-sandbox".to_string(),
            ]
        );
    }

    #[test]
    fn resume_command_preserves_safe_launch_flags_and_cwd() {
        let launch = AgentLaunchCommandRecord {
            executable: "codex".to_string(),
            arguments: vec![
                "codex".to_string(),
                "--model".to_string(),
                "gpt-5.5".to_string(),
                "--config".to_string(),
                "profile=work".to_string(),
            ],
            cwd: Some("/tmp/project one".to_string()),
            environment: Default::default(),
            captured_at: Some(20.0),
        };

        let command = build_resume_command(
            AgentKind::Codex,
            "sess-123",
            Some(&launch),
            Some("/tmp/project one"),
        )
        .expect("resume command");

        assert_eq!(
            command,
            "cd '/tmp/project one' && 'codex' 'resume' '--model' 'gpt-5.5' '--config' 'profile=work' 'sess-123'"
        );
    }
}
