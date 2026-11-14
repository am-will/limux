use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const ALLOWED_AGENT_ENV_VARS: &[&str] = &[
    "CODEX_HOME",
    "CLAUDE_CONFIG_DIR",
    "OPENCODE_CONFIG_DIR",
    "GEMINI_CONFIG_DIR",
    "AGY_CONFIG_DIR",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_MODEL",
    "ANTHROPIC_SMALL_FAST_MODEL",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    Claude,
    Codex,
    OpenCode,
    Gemini,
    Agy,
}

impl AgentKind {
    pub const ALL: [Self; 5] = [
        Self::Claude,
        Self::Codex,
        Self::OpenCode,
        Self::Gemini,
        Self::Agy,
    ];

    pub fn from_hook_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "claude" | "claude-code" | "claudecode" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "opencode" | "open-code" => Some(Self::OpenCode),
            "gemini" | "gemini-cli" => Some(Self::Gemini),
            "agy" | "antigravity" | "antigravity-cli" => Some(Self::Agy),
            _ => None,
        }
    }

    pub fn store_name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::Gemini => "gemini",
            Self::Agy => "agy",
        }
    }

    pub fn hook_session_file_name(self) -> &'static str {
        match self {
            Self::Claude => "claude-hook-sessions.json",
            Self::Codex => "codex-hook-sessions.json",
            Self::OpenCode => "opencode-hook-sessions.json",
            Self::Gemini => "gemini-hook-sessions.json",
            Self::Agy => "agy-hook-sessions.json",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::OpenCode => "OpenCode",
            Self::Gemini => "Gemini",
            Self::Agy => "Antigravity",
        }
    }

    pub fn fallback_executable(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::Gemini => "gemini",
            Self::Agy => "agy",
        }
    }

    pub fn process_needles(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &["claude"],
            Self::Codex => &["codex"],
            Self::OpenCode => &["opencode"],
            Self::Gemini => &["gemini"],
            Self::Agy => &["agy", "antigravity"],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentLaunchCommand {
    pub executable: String,
    #[serde(default)]
    pub arguments: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_at: Option<f64>,
}

pub fn filter_allowed_environment<I, K, V>(vars: I) -> BTreeMap<String, String>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: AsRef<str>,
{
    let allowlist: BTreeSet<&'static str> = ALLOWED_AGENT_ENV_VARS.iter().copied().collect();
    vars.into_iter()
        .filter_map(|(key, value)| {
            let key = key.as_ref();
            let value = value.as_ref().trim();
            (allowlist.contains(key) && !value.is_empty())
                .then(|| (key.to_string(), value.to_string()))
        })
        .collect()
}

pub fn proc_matches_agent(args: &[String], comm: Option<&str>, kind: AgentKind) -> bool {
    if let Some(first) = args.first() {
        let exe_name = executable_basename(first);
        if is_hook_wrapper_binary(&exe_name) {
            return false;
        }
        if matches_agent_needle(&exe_name, kind) {
            return true;
        }
        if is_script_interpreter(&exe_name) {
            if let Some(script_idx) = interpreter_script_index(args) {
                let script_lower = args[script_idx].to_ascii_lowercase();
                if kind
                    .process_needles()
                    .iter()
                    .any(|needle| script_lower.contains(needle))
                {
                    return true;
                }
            }
            return false;
        }
    }

    let Some(comm) = comm.map(str::trim).filter(|value| !value.is_empty()) else {
        return false;
    };
    let comm_lower = comm.to_ascii_lowercase();
    if is_hook_wrapper_binary(&comm_lower) || is_script_interpreter(&comm_lower) {
        return false;
    }
    matches_agent_needle(&comm_lower, kind)
}

pub fn normalize_captured_process_args(
    kind: AgentKind,
    raw_args: &[String],
) -> Option<Vec<String>> {
    let first = raw_args.first()?;
    let exe_name = executable_basename(first);
    if is_hook_wrapper_binary(&exe_name) {
        return None;
    }
    if is_script_interpreter(&exe_name) {
        let script_idx = interpreter_script_index(raw_args)?;
        let script = &raw_args[script_idx];
        let script_lower = script.to_ascii_lowercase();
        if !kind
            .process_needles()
            .iter()
            .any(|needle| script_lower.contains(needle))
        {
            return None;
        }
        let entrypoint = if is_script_source_path(&script_lower) {
            kind.fallback_executable().to_string()
        } else {
            script.clone()
        };
        let mut normalized_args =
            Vec::with_capacity(1 + raw_args.len().saturating_sub(script_idx + 1));
        normalized_args.push(entrypoint);
        normalized_args.extend_from_slice(&raw_args[script_idx + 1..]);
        return Some(normalized_args);
    }

    if matches_agent_needle(&exe_name, kind) {
        return Some(raw_args.to_vec());
    }
    None
}

pub fn sanitize_launch_arguments(kind: AgentKind, arguments: &[String]) -> Vec<String> {
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
        if resume_selector_takes_value(kind, arg) {
            index += 1;
            if index < arguments.len() && !arguments[index].starts_with('-') {
                index += 1;
            }
            continue;
        }
        if resume_selector_flag_or_assignment(kind, arg) {
            index += 1;
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
        if option_takes_safe_value(kind, arg) {
            result.push(arg.clone());
            if index + 1 < arguments.len() {
                result.push(arguments[index + 1].clone());
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        if option_is_safe_flag_or_assignment(kind, arg) {
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

pub fn build_resume_command(
    kind: AgentKind,
    session_id: &str,
    launch: Option<&AgentLaunchCommand>,
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

    let mut parts = Vec::new();
    if let Some(launch) = launch {
        let allowed_env = filter_allowed_environment(&launch.environment);
        if !allowed_env.is_empty() {
            parts.push("env".to_string());
            for (key, value) in allowed_env {
                parts.push(format!("{key}={value}"));
            }
        }
    }

    parts.push(executable);
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
        AgentKind::Agy => {
            parts.push("--conversation".to_string());
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

pub fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn normalized(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn executable_basename(arg: &str) -> String {
    Path::new(arg.trim())
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(arg.trim())
        .to_ascii_lowercase()
}

fn matches_agent_needle(exe_name: &str, kind: AgentKind) -> bool {
    kind.process_needles().iter().any(|needle| {
        exe_name == *needle
            || exe_name.starts_with(&format!("{needle}-"))
            || exe_name.starts_with(&format!("{needle}_"))
    })
}

fn is_hook_wrapper_binary(exe_name: &str) -> bool {
    matches!(
        exe_name,
        "sh" | "bash" | "dash" | "zsh" | "fish" | "nu" | "env" | "limux" | "limux-cli"
    )
}

fn is_script_interpreter(exe_name: &str) -> bool {
    exe_name == "node"
        || exe_name == "nodejs"
        || exe_name == "bun"
        || exe_name == "deno"
        || exe_name == "python"
        || exe_name.starts_with("python3")
}

fn interpreter_script_index(args: &[String]) -> Option<usize> {
    let mut idx = 1;
    while idx < args.len() {
        let arg = &args[idx];
        if !arg.starts_with('-') {
            return Some(idx);
        }
        if matches!(arg.as_str(), "-r" | "--require" | "--import" | "--loader") {
            idx += 2;
            continue;
        }
        idx += 1;
    }
    None
}

fn is_script_source_path(path_lower: &str) -> bool {
    path_lower.ends_with(".js")
        || path_lower.ends_with(".mjs")
        || path_lower.ends_with(".cjs")
        || path_lower.ends_with(".ts")
        || path_lower.ends_with(".py")
}

fn resume_selector_takes_value(kind: AgentKind, arg: &str) -> bool {
    match kind {
        AgentKind::Codex => arg == "resume" || arg == "--resume",
        AgentKind::OpenCode => arg == "--session",
        AgentKind::Claude | AgentKind::Gemini => arg == "--resume" || arg == "-r",
        AgentKind::Agy => arg == "--conversation",
    }
}

fn resume_selector_flag_or_assignment(kind: AgentKind, arg: &str) -> bool {
    match kind {
        AgentKind::Codex => arg.starts_with("--resume=") || arg == "--last",
        AgentKind::OpenCode => arg.starts_with("--session=") || arg == "--continue" || arg == "-c",
        AgentKind::Claude | AgentKind::Gemini => {
            arg.starts_with("--resume=") || arg == "--continue" || arg == "-c"
        }
        AgentKind::Agy => arg.starts_with("--conversation=") || arg == "--continue" || arg == "-c",
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

fn option_takes_safe_value(kind: AgentKind, arg: &str) -> bool {
    if arg == "-c" {
        return kind == AgentKind::Codex;
    }
    matches!(
        arg,
        "--model"
            | "-m"
            | "--config"
            | "--profile"
            | "--sandbox"
            | "--approval-policy"
            | "--ask-for-approval"
            | "--cwd"
            | "--cd"
            | "--working-directory"
            | "--config-dir"
            | "--home"
            | "--effort"
            | "--agent"
            | "--add-dir"
            | "--mode"
            | "--project"
            | "--permission-mode"
    )
}

fn option_is_safe_flag_or_assignment(kind: AgentKind, arg: &str) -> bool {
    if matches!(
        arg,
        "--dangerously-bypass-approvals-and-sandbox"
            | "--dangerously-skip-permissions"
            | "--full-auto"
            | "--search"
            | "--no-search"
            | "--yolo"
            | "--remote-agent"
    ) {
        return true;
    }

    let Some((name, _)) = arg.split_once('=') else {
        return false;
    };
    option_takes_safe_value(kind, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vec_str(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn sanitizer_preserves_config_dir_home_and_safe_flags_without_truncating() {
        let args = vec_str(&[
            "gemini",
            "--config-dir",
            "/tmp/gemini-work",
            "--yolo",
            "--model",
            "gemini-2.5-pro",
            "explain this repo",
        ]);
        assert_eq!(
            sanitize_launch_arguments(AgentKind::Gemini, &args),
            vec_str(&[
                "gemini",
                "--config-dir",
                "/tmp/gemini-work",
                "--yolo",
                "--model",
                "gemini-2.5-pro",
            ])
        );
    }

    #[test]
    fn sanitizer_strips_agy_resume_selectors_and_secrets_while_preserving_flags() {
        let args = vec_str(&[
            "agy",
            "-c",
            "--conversation",
            "old-conv-id",
            "--api-key=secret-123",
            "--credential",
            "secret-cred",
            "--model",
            "gemini-2.5-pro",
            "--effort",
            "high",
            "--dangerously-skip-permissions",
            "fix the bug",
        ]);
        assert_eq!(
            sanitize_launch_arguments(AgentKind::Agy, &args),
            vec_str(&[
                "agy",
                "--model",
                "gemini-2.5-pro",
                "--effort",
                "high",
                "--dangerously-skip-permissions",
            ])
        );
    }

    #[test]
    fn build_resume_command_applies_allowlisted_environment_and_ignores_disallowed_vars() {
        let mut environment = BTreeMap::new();
        environment.insert("CODEX_HOME".to_string(), "/tmp/codex home".to_string());
        environment.insert("LD_PRELOAD".to_string(), "/tmp/evil.so".to_string());
        let launch = AgentLaunchCommand {
            executable: "codex".to_string(),
            arguments: vec_str(&["codex", "-c", "profile=work", "--full-auto"]),
            cwd: Some("/tmp/project".to_string()),
            environment,
            captured_at: Some(42.0),
        };

        let command = build_resume_command(AgentKind::Codex, "sess-99", Some(&launch), None)
            .expect("resume command");
        assert_eq!(
            command,
            "cd '/tmp/project' && 'env' 'CODEX_HOME=/tmp/codex home' 'codex' 'resume' '-c' 'profile=work' '--full-auto' 'sess-99'"
        );
    }

    #[test]
    fn build_resume_command_supports_agy_conversation_flag() {
        let launch = AgentLaunchCommand {
            executable: "/opt/bin/agy".to_string(),
            arguments: vec_str(&["/opt/bin/agy", "--model", "gemini-2.5-pro"]),
            cwd: Some("/tmp/repo".to_string()),
            environment: BTreeMap::new(),
            captured_at: None,
        };
        let command = build_resume_command(AgentKind::Agy, "conv-abc", Some(&launch), None)
            .expect("agy resume command");
        assert_eq!(
            command,
            "cd '/tmp/repo' && '/opt/bin/agy' '--conversation' 'conv-abc' '--model' 'gemini-2.5-pro'"
        );
    }

    #[test]
    fn proc_matching_skips_hook_shell_wrappers_and_normalizes_node_shebangs() {
        let hook_shell = vec_str(&[
            "/bin/sh",
            "-c",
            "[ \"${LIMUX_CLAUDE_HOOKS_DISABLED:-}\" != \"1\" ] && limux --json hooks claude session-start",
        ]);
        assert!(!proc_matches_agent(
            &hook_shell,
            Some("sh"),
            AgentKind::Claude
        ));

        let node_claude = vec_str(&[
            "/usr/bin/node",
            "/usr/lib/node_modules/@anthropic-ai/claude-code/cli.js",
            "--model",
            "opus",
            "--dangerously-skip-permissions",
        ]);
        assert!(proc_matches_agent(
            &node_claude,
            Some("node"),
            AgentKind::Claude
        ));
        assert_eq!(
            normalize_captured_process_args(AgentKind::Claude, &node_claude),
            Some(vec_str(&[
                "claude",
                "--model",
                "opus",
                "--dangerously-skip-permissions",
            ]))
        );
    }
}
