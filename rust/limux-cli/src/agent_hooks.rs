use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AgentKind {
    Claude,
    Codex,
    OpenCode,
    Gemini,
}

impl AgentKind {
    pub(crate) fn from_hook_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "claude" | "claude-code" | "claudecode" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "opencode" | "open-code" => Some(Self::OpenCode),
            "gemini" => Some(Self::Gemini),
            _ => None,
        }
    }

    pub(crate) fn store_name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::Gemini => "gemini",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::OpenCode => "OpenCode",
            Self::Gemini => "Gemini",
        }
    }

    fn fallback_executable(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::Gemini => "gemini",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct AgentLaunchCommandRecord {
    pub(crate) executable: String,
    pub(crate) arguments: Vec<String>,
    #[serde(default)]
    pub(crate) cwd: Option<String>,
    #[serde(default)]
    pub(crate) environment: BTreeMap<String, String>,
    pub(crate) captured_at: f64,
}

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
}

pub(crate) struct AgentHookSessionStore {
    path: PathBuf,
}

impl AgentHookSessionStore {
    pub(crate) fn new(agent: AgentKind) -> Self {
        Self::new_for_agent_name(agent.store_name())
    }

    pub(crate) fn new_for_agent_name(agent: &str) -> Self {
        let filename = format!("{}-hook-sessions.json", safe_store_name(agent));
        if let Some(dir) = std::env::var_os("LIMUX_AGENT_HOOK_STATE_DIR") {
            let dir = PathBuf::from(dir);
            return Self {
                path: dir.join(filename),
            };
        }
        Self {
            path: state_dir().join(filename),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_for_dir(agent: &str, dir: &Path) -> Self {
        Self {
            path: dir.join(format!("{}-hook-sessions.json", safe_store_name(agent))),
        }
    }

    pub(crate) fn lookup(&self, session_id: &str) -> Result<Option<AgentHookSessionRecord>> {
        let session_id = normalized(session_id);
        if session_id.is_none() {
            return Ok(None);
        }
        let file = self.load()?;
        Ok(file.sessions.get(session_id.as_deref().unwrap()).cloned())
    }

    pub(crate) fn upsert(&self, record: AgentHookSessionRecord) -> Result<()> {
        let Some(session_id) = normalized(&record.session_id) else {
            return Ok(());
        };
        let mut file = self.load()?;
        file.version = 1;
        file.sessions.insert(session_id, record);
        self.save(&file)
    }

    pub(crate) fn remove(&self, session_id: &str) -> Result<()> {
        let Some(session_id) = normalized(session_id) else {
            return Ok(());
        };
        let mut file = self.load()?;
        file.sessions.remove(&session_id);
        self.save(&file)
    }

    fn load(&self) -> Result<AgentHookSessionFile> {
        if !self.path.exists() {
            return Ok(AgentHookSessionFile {
                version: 1,
                sessions: BTreeMap::new(),
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
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let temp = self
            .path
            .with_extension(format!("json.{}.tmp", std::process::id()));
        let json = serde_json::to_vec_pretty(file).context("failed to encode hook store")?;
        fs::write(&temp, json).with_context(|| format!("failed to write {}", temp.display()))?;
        fs::rename(&temp, &self.path)
            .with_context(|| format!("failed to replace {}", self.path.display()))?;
        Ok(())
    }
}

pub(crate) fn sanitize_launch_arguments(kind: AgentKind, arguments: &[String]) -> Vec<String> {
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
        if is_resume_selector(kind, arg) {
            index += 1;
            if index < arguments.len() && !arguments[index].starts_with('-') {
                index += 1;
            }
            continue;
        }
        if option_takes_secret_value(arg) {
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

    if result.is_empty() {
        vec![kind.fallback_executable().to_string()]
    } else {
        result
    }
}

#[cfg(test)]
pub(crate) fn build_resume_command(
    kind: AgentKind,
    session_id: &str,
    launch: Option<&AgentLaunchCommandRecord>,
    cwd: Option<&str>,
) -> Option<String> {
    let session_id = normalized(session_id)?;
    let fallback = kind.fallback_executable().to_string();
    let raw_args = launch
        .map(|launch| launch.arguments.clone())
        .filter(|args| !args.is_empty())
        .unwrap_or_else(|| vec![fallback.clone()]);
    let sanitized = sanitize_launch_arguments(kind, &raw_args);
    let executable = launch
        .and_then(|launch| normalized(&launch.executable))
        .or_else(|| sanitized.first().cloned())
        .unwrap_or(fallback);
    let preserved_tail = sanitized
        .get(1..)
        .map(|tail| tail.to_vec())
        .unwrap_or_default();

    let mut parts = vec![executable];
    match kind {
        AgentKind::Codex => {
            parts.push("resume".to_string());
            parts.extend(preserved_tail);
            parts.push(session_id);
        }
        AgentKind::OpenCode => {
            parts.push("--session".to_string());
            parts.push(session_id);
            parts.extend(preserved_tail);
        }
        AgentKind::Claude | AgentKind::Gemini => {
            parts.push("--resume".to_string());
            parts.push(session_id);
            parts.extend(preserved_tail);
        }
    }

    let command = parts
        .iter()
        .map(|part| shell_single_quote(part))
        .collect::<Vec<_>>()
        .join(" ");
    let cwd = cwd.and_then(normalized).or_else(|| {
        launch
            .and_then(|launch| launch.cwd.as_deref())
            .and_then(normalized)
    });
    Some(match cwd {
        Some(cwd) => format!("cd {} && {command}", shell_single_quote(&cwd)),
        None => command,
    })
}

pub(crate) fn launch_record_from_env(
    kind: AgentKind,
    payload_cwd: Option<&str>,
) -> Option<AgentLaunchCommandRecord> {
    let args = std::env::var("LIMUX_AGENT_LAUNCH_ARGV")
        .ok()
        .map(split_nul_or_space_separated)
        .filter(|args| !args.is_empty())
        .unwrap_or_else(|| vec![kind.fallback_executable().to_string()]);
    let sanitized = sanitize_launch_arguments(kind, &args);
    let executable = std::env::var("LIMUX_AGENT_LAUNCH_EXECUTABLE")
        .ok()
        .and_then(|value| normalized(&value))
        .or_else(|| sanitized.first().cloned())?;
    Some(AgentLaunchCommandRecord {
        executable,
        arguments: sanitized,
        cwd: std::env::var("LIMUX_AGENT_LAUNCH_CWD")
            .ok()
            .and_then(|value| normalized(&value))
            .or_else(|| payload_cwd.and_then(normalized))
            .or_else(|| {
                std::env::var("PWD")
                    .ok()
                    .and_then(|value| normalized(&value))
            }),
        environment: selected_environment(),
        captured_at: now_seconds(),
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

#[cfg(test)]
fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn is_resume_selector(kind: AgentKind, arg: &str) -> bool {
    match kind {
        AgentKind::Codex => arg == "resume" || arg == "--resume" || arg.starts_with("--resume="),
        AgentKind::OpenCode => arg == "--session" || arg.starts_with("--session="),
        AgentKind::Claude | AgentKind::Gemini => {
            arg == "--resume" || arg.starts_with("--resume=") || arg == "--continue"
        }
    }
}

fn option_takes_secret_value(arg: &str) -> bool {
    matches!(
        arg,
        "--api-key"
            | "--apikey"
            | "--token"
            | "--auth-token"
            | "--password"
            | "--credential"
            | "--credentials"
    )
}

fn option_is_secret_assignment(arg: &str) -> bool {
    let lower = arg.to_ascii_lowercase();
    lower.starts_with("--api-key=")
        || lower.starts_with("--apikey=")
        || lower.starts_with("--token=")
        || lower.starts_with("--auth-token=")
        || lower.starts_with("--password=")
        || lower.starts_with("--credential=")
        || lower.starts_with("--credentials=")
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
            | "--config-dir"
            | "--home"
    )
}

fn option_is_safe_flag_or_assignment(arg: &str) -> bool {
    if matches!(
        arg,
        "--dangerously-bypass-approvals-and-sandbox"
            | "--dangerously-skip-permissions"
            | "--full-auto"
            | "--search"
            | "--no-search"
            | "--yolo"
    ) {
        return true;
    }

    let Some((name, _)) = arg.split_once('=') else {
        return false;
    };
    option_takes_safe_value(name)
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

fn selected_environment() -> BTreeMap<String, String> {
    let allowlist: BTreeSet<&'static str> = [
        "CODEX_HOME",
        "CLAUDE_CONFIG_DIR",
        "OPENCODE_CONFIG_DIR",
        "GEMINI_CONFIG_DIR",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_MODEL",
        "ANTHROPIC_SMALL_FAST_MODEL",
    ]
    .into_iter()
    .collect();

    std::env::vars()
        .filter(|(key, value)| allowlist.contains(key.as_str()) && !value.trim().is_empty())
        .collect()
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
                captured_at: 10.0,
            }),
            updated_at: 11.0,
        };

        store.upsert(record.clone()).expect("upsert");

        assert_eq!(
            store.lookup("codex-session-1").expect("lookup"),
            Some(record)
        );
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
            captured_at: 20.0,
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
