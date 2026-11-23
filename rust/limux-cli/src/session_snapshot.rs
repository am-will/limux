use anyhow::Result;
use limux_control::agent_sessions::{
    normalize_captured_process_args, proc_matches_agent, AgentKind,
};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
#[cfg(target_os = "linux")]
use std::fs::File;
#[cfg(target_os = "linux")]
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use crate::agent_hooks::{
    filter_allowed_environment, launch_record_from_process, now_seconds, AgentHookSessionRecord,
    AgentHookSessionStore,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SessionIdSource {
    OpenFd,
    Env,
    Cmdline,
    ExistingHookStore,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct SnapshottedSession {
    pub(crate) agent: String,
    pub(crate) session_id: String,
    pub(crate) source: SessionIdSource,
    pub(crate) workspace_id: String,
    pub(crate) surface_id: String,
    pub(crate) cwd: Option<String>,
    pub(crate) pid: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) superseded_session_ids: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct SkippedProcess {
    pub(crate) agent: String,
    pub(crate) pid: u32,
    pub(crate) workspace_id: String,
    pub(crate) surface_id: String,
    pub(crate) cwd: Option<String>,
    pub(crate) reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct SnapshotReport {
    pub(crate) dry_run: bool,
    pub(crate) captured: Vec<SnapshottedSession>,
    pub(crate) skipped: Vec<SkippedProcess>,
}

#[derive(Clone, Debug)]
pub(crate) struct SnapshotOptions {
    pub(crate) agent: Option<AgentKind>,
    pub(crate) workspace_id: Option<String>,
    pub(crate) surface_id: Option<String>,
    pub(crate) target_socket: Option<PathBuf>,
    pub(crate) all_sockets: bool,
    pub(crate) dry_run: bool,
}

#[derive(Clone, Debug)]
struct ProcessProbe {
    pid: u32,
    args: Vec<String>,
    comm: Option<String>,
    cwd: Option<String>,
    environ: Vec<u8>,
    open_paths: Vec<PathBuf>,
}

#[derive(Clone, Debug)]
struct MatchedCandidate {
    kind: AgentKind,
    probe: ProcessProbe,
    workspace_id: String,
    surface_id: String,
}

pub(crate) fn snapshot_running_agents(options: &SnapshotOptions) -> Result<SnapshotReport> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = options;
        anyhow::bail!("live agent process snapshotting is currently supported on Linux only");
    }

    #[cfg(target_os = "linux")]
    {
        let (probes, ppid_map) = scan_linux_proc_agents(options)?;
        snapshot_from_probes(
            probes,
            &ppid_map,
            options,
            AgentHookSessionStore::new,
            read_first_line_bounded,
        )
    }
}

fn snapshot_from_probes(
    probes: Vec<ProcessProbe>,
    ppid_map: &HashMap<u32, u32>,
    options: &SnapshotOptions,
    store_for: impl Fn(AgentKind) -> AgentHookSessionStore,
    read_first_line: impl Fn(&Path) -> Option<String>,
) -> Result<SnapshotReport> {
    let target_socket = options
        .target_socket
        .as_deref()
        .map(canonical_or_original_path);

    let mut candidates = Vec::new();
    for probe in probes {
        let Some(kind) = detect_agent_kind(&probe.args, probe.comm.as_deref()) else {
            continue;
        };
        if options.agent.is_some_and(|filter| filter != kind) {
            continue;
        }
        let Some(workspace_id) = env_value(&probe.environ, "LIMUX_WORKSPACE_ID") else {
            continue;
        };
        let Some(surface_id) = env_value(&probe.environ, "LIMUX_SURFACE_ID") else {
            continue;
        };
        if options
            .workspace_id
            .as_deref()
            .is_some_and(|filter| filter != workspace_id)
        {
            continue;
        }
        if options
            .surface_id
            .as_deref()
            .is_some_and(|filter| filter != surface_id)
        {
            continue;
        }
        if !options.all_sockets {
            if let (Some(expected), Some(actual_raw)) = (
                target_socket.as_deref(),
                env_value(&probe.environ, "LIMUX_SOCKET"),
            ) {
                let actual = canonical_or_original_path(Path::new(&actual_raw));
                if actual != expected {
                    continue;
                }
            }
        }
        candidates.push(MatchedCandidate {
            kind,
            probe,
            workspace_id,
            surface_id,
        });
    }

    let candidate_surfaces: HashMap<u32, (&str, &str)> = candidates
        .iter()
        .map(|candidate| {
            (
                candidate.probe.pid,
                (
                    candidate.workspace_id.as_str(),
                    candidate.surface_id.as_str(),
                ),
            )
        })
        .collect();

    let mut filtered: Vec<MatchedCandidate> = candidates
        .iter()
        .filter(|candidate| {
            !has_matched_ancestor_on_same_surface(
                candidate.probe.pid,
                &candidate.workspace_id,
                &candidate.surface_id,
                ppid_map,
                &candidate_surfaces,
            )
        })
        .cloned()
        .collect();

    filtered.sort_by(|a, b| {
        a.workspace_id
            .cmp(&b.workspace_id)
            .then_with(|| a.surface_id.cmp(&b.surface_id))
            .then_with(|| a.probe.pid.cmp(&b.probe.pid))
    });

    let mut captured = Vec::new();
    let mut skipped = Vec::new();

    for candidate in filtered {
        let store = store_for(candidate.kind);
        let extracted = extract_session_id(
            candidate.kind,
            &candidate.probe.args,
            &candidate.probe.environ,
            &candidate.probe.open_paths,
            &read_first_line,
        );
        let discovered = match extracted {
            Some(found) => Some(found),
            None => store
                .session_for_live_pid(
                    candidate.probe.pid,
                    &candidate.workspace_id,
                    &candidate.surface_id,
                )?
                .map(|session_id| (session_id, SessionIdSource::ExistingHookStore)),
        };

        let Some((session_id, source)) = discovered else {
            skipped.push(SkippedProcess {
                agent: candidate.kind.store_name().to_string(),
                pid: candidate.probe.pid,
                workspace_id: candidate.workspace_id,
                surface_id: candidate.surface_id,
                cwd: candidate.probe.cwd,
                reason: "no_session_id".to_string(),
            });
            continue;
        };

        let proc_env = allowed_env_from_environ(&candidate.probe.environ);
        let launch_command = launch_record_from_process(
            candidate.kind,
            Some(&candidate.probe.args),
            candidate.probe.cwd.as_deref(),
            Some(&proc_env),
        );

        let superseded = if options.dry_run {
            Vec::new()
        } else {
            store.upsert_snapshot(AgentHookSessionRecord {
                session_id: session_id.clone(),
                workspace_id: candidate.workspace_id.clone(),
                surface_id: candidate.surface_id.clone(),
                cwd: candidate.probe.cwd.clone(),
                pid: Some(candidate.probe.pid),
                launch_command,
                updated_at: now_seconds(),
            })?
        };

        captured.push(SnapshottedSession {
            agent: candidate.kind.store_name().to_string(),
            session_id,
            source,
            workspace_id: candidate.workspace_id,
            surface_id: candidate.surface_id,
            cwd: candidate.probe.cwd,
            pid: candidate.probe.pid,
            superseded_session_ids: superseded,
        });
    }

    Ok(SnapshotReport {
        dry_run: options.dry_run,
        captured,
        skipped,
    })
}

fn has_matched_ancestor_on_same_surface(
    pid: u32,
    workspace_id: &str,
    surface_id: &str,
    ppid_map: &HashMap<u32, u32>,
    candidate_surfaces: &HashMap<u32, (&str, &str)>,
) -> bool {
    let mut visited = HashSet::new();
    let mut current = pid;
    while visited.insert(current) {
        let Some(&parent) = ppid_map.get(&current) else {
            break;
        };
        if parent <= 1 || parent == current {
            break;
        }
        if candidate_surfaces.get(&parent) == Some(&(workspace_id, surface_id)) {
            return true;
        }
        current = parent;
    }
    false
}

fn detect_agent_kind(args: &[String], comm: Option<&str>) -> Option<AgentKind> {
    AgentKind::ALL
        .into_iter()
        .find(|&kind| proc_matches_agent(args, comm, kind))
}

fn extract_session_id(
    kind: AgentKind,
    args: &[String],
    environ: &[u8],
    open_paths: &[PathBuf],
    read_first_line: &impl Fn(&Path) -> Option<String>,
) -> Option<(String, SessionIdSource)> {
    // Open file descriptors reflect the currently active session even if a
    // resumed process later switched conversations inside the same TUI.
    if let Some(session_id) = extract_session_id_from_open_paths(kind, open_paths, read_first_line)
    {
        return Some((session_id, SessionIdSource::OpenFd));
    }
    if let Some(session_id) = extract_session_id_from_env(kind, environ) {
        return Some((session_id, SessionIdSource::Env));
    }
    if let Some(session_id) = extract_session_id_from_cmdline(kind, args) {
        return Some((session_id, SessionIdSource::Cmdline));
    }
    None
}

fn extract_session_id_from_env(kind: AgentKind, environ: &[u8]) -> Option<String> {
    let vars: &[&str] = match kind {
        AgentKind::Claude => &["CLAUDE_CODE_SESSION_ID", "CLAUDE_SESSION_ID"],
        AgentKind::Codex => &["CODEX_THREAD_ID", "CODEX_SESSION_ID"],
        AgentKind::Gemini => &["GEMINI_SESSION_ID"],
        AgentKind::OpenCode => &["OPENCODE_SESSION_ID"],
        AgentKind::Agy => &[],
    };
    vars.iter().find_map(|name| env_value(environ, name))
}

fn extract_session_id_from_cmdline(kind: AgentKind, raw_args: &[String]) -> Option<String> {
    let normalized = normalize_captured_process_args(kind, raw_args);
    let args = normalized.as_deref().unwrap_or(raw_args);
    if args.len() <= 1 {
        return None;
    }

    match kind {
        AgentKind::Agy => extract_flag_value(args, &["--conversation"]),
        AgentKind::Claude => extract_flag_value(args, &["--resume", "-r", "--session-id"]),
        AgentKind::Gemini => extract_flag_value(args, &["--resume", "-r"])
            .filter(|value| !value.eq_ignore_ascii_case("latest")),
        AgentKind::OpenCode => extract_flag_value(args, &["--session", "-s"]),
        AgentKind::Codex => {
            extract_flag_value(args, &["--resume"]).or_else(|| extract_codex_resume_arg(args))
        }
    }
}

fn extract_flag_value(args: &[String], flags: &[&str]) -> Option<String> {
    let mut idx = 1;
    while idx < args.len() {
        let arg = &args[idx];
        for flag in flags {
            if arg == flag {
                if let Some(next) = args
                    .get(idx + 1)
                    .filter(|next| !next.starts_with('-'))
                    .and_then(|next| non_empty(next))
                {
                    return Some(next);
                }
            }
            if flag.starts_with("--") {
                if let Some(rest) = arg
                    .strip_prefix(flag)
                    .and_then(|rest| rest.strip_prefix('='))
                {
                    if let Some(value) = non_empty(rest) {
                        return Some(value);
                    }
                }
            }
        }
        idx += 1;
    }
    None
}

fn extract_codex_resume_arg(args: &[String]) -> Option<String> {
    let resume_idx = args.iter().skip(1).position(|arg| arg == "resume")? + 1;
    let mut idx = resume_idx + 1;
    while idx < args.len() {
        let arg = &args[idx];
        if arg == "--" {
            return args.get(idx + 1).and_then(|next| non_empty(next));
        }
        if matches!(
            arg.as_str(),
            "-m" | "--model"
                | "-c"
                | "--config"
                | "-p"
                | "--profile"
                | "-s"
                | "--sandbox"
                | "-a"
                | "--ask-for-approval"
                | "--approval-policy"
                | "-C"
                | "--cd"
                | "--cwd"
                | "--working-directory"
                | "--config-dir"
                | "--home"
                | "--effort"
                | "--add-dir"
        ) {
            idx += 2;
            continue;
        }
        if arg.starts_with('-') {
            idx += 1;
            continue;
        }
        return non_empty(arg);
    }
    None
}

fn extract_session_id_from_open_paths(
    kind: AgentKind,
    open_paths: &[PathBuf],
    read_first_line: &impl Fn(&Path) -> Option<String>,
) -> Option<String> {
    let live_paths: Vec<&Path> = open_paths
        .iter()
        .map(PathBuf::as_path)
        .filter(|path| path.to_str().is_none_or(|raw| !raw.ends_with(" (deleted)")))
        .collect();

    match kind {
        AgentKind::Agy => {
            // Prefer presence/<id>.lock (held for the entire attached conversation),
            // falling back to conversations/<id>.db.
            live_paths
                .iter()
                .find_map(|path| extract_agy_presence_id(path))
                .or_else(|| {
                    live_paths
                        .iter()
                        .find_map(|path| extract_agy_conversation_db_id(path))
                })
        }
        AgentKind::Claude => live_paths
            .iter()
            .find_map(|path| extract_claude_project_jsonl_id(path)),
        AgentKind::Codex => live_paths
            .iter()
            .find_map(|path| extract_codex_rollout_jsonl_id(path)),
        AgentKind::Gemini => live_paths
            .iter()
            .find_map(|path| extract_gemini_chat_session_id(path, read_first_line)),
        AgentKind::OpenCode => None,
    }
}

fn extract_agy_presence_id(path: &Path) -> Option<String> {
    let parent = path.parent()?.file_name()?.to_str()?;
    if parent != "presence" {
        return None;
    }
    if path.extension().and_then(|ext| ext.to_str()) != Some("lock") {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    (!stem.starts_with('.')).then(|| non_empty(stem)).flatten()
}

fn extract_agy_conversation_db_id(path: &Path) -> Option<String> {
    let parent = path.parent()?.file_name()?.to_str()?;
    if parent != "conversations" {
        return None;
    }
    if path.extension().and_then(|ext| ext.to_str()) != Some("db") {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    (!stem.starts_with('.')).then(|| non_empty(stem)).flatten()
}

fn extract_claude_project_jsonl_id(path: &Path) -> Option<String> {
    if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
        return None;
    }
    let grandparent = path.parent()?.parent()?.file_name()?.to_str()?;
    if grandparent != "projects" {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    is_uuid_like(stem).then(|| stem.to_string())
}

fn extract_codex_rollout_jsonl_id(path: &Path) -> Option<String> {
    if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
        return None;
    }
    let has_sessions_ancestor = path
        .ancestors()
        .skip(1)
        .filter_map(|ancestor| ancestor.file_name().and_then(|name| name.to_str()))
        .any(|name| name == "sessions");
    if !has_sessions_ancestor {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    if !stem.starts_with("rollout-") || stem.len() <= 37 {
        return None;
    }
    let split_at = stem.len() - 36;
    if stem.as_bytes().get(split_at - 1) != Some(&b'-') {
        return None;
    }
    let candidate = &stem[split_at..];
    is_uuid_like(candidate).then(|| candidate.to_string())
}

fn extract_gemini_chat_session_id(
    path: &Path,
    read_first_line: &impl Fn(&Path) -> Option<String>,
) -> Option<String> {
    let parent = path.parent()?.file_name()?.to_str()?;
    if parent != "chats" {
        return None;
    }
    let ext = path.extension().and_then(|ext| ext.to_str())?;
    if !matches!(ext, "jsonl" | "json") {
        return None;
    }
    let file_name = path.file_name()?.to_str()?;
    if !file_name.starts_with("session-") {
        return None;
    }
    let first_line = read_first_line(path)?;
    let parsed: serde_json::Value = serde_json::from_str(first_line.trim()).ok()?;
    parsed
        .get("sessionId")
        .or_else(|| parsed.get("session_id"))
        .and_then(serde_json::Value::as_str)
        .and_then(non_empty)
}

fn is_uuid_like(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    bytes.iter().enumerate().all(|(idx, byte)| {
        if matches!(idx, 8 | 13 | 18 | 23) {
            *byte == b'-'
        } else {
            byte.is_ascii_hexdigit()
        }
    })
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn env_value(environ: &[u8], name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    environ
        .split(|byte| *byte == 0)
        .filter_map(|part| std::str::from_utf8(part).ok())
        .find_map(|entry| entry.strip_prefix(&prefix).and_then(non_empty))
}

fn allowed_env_from_environ(environ: &[u8]) -> BTreeMap<String, String> {
    let pairs = environ
        .split(|byte| *byte == 0)
        .filter_map(|part| std::str::from_utf8(part).ok())
        .filter_map(|entry| entry.split_once('='));
    filter_allowed_environment(pairs)
}

fn canonical_or_original_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(target_os = "linux")]
fn read_first_line_bounded(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file.take(16 * 1024));
    let mut line = String::new();
    let read = reader.read_line(&mut line).ok()?;
    (read > 0).then_some(line)
}

#[cfg(target_os = "linux")]
fn scan_linux_proc_agents(
    options: &SnapshotOptions,
) -> Result<(Vec<ProcessProbe>, HashMap<u32, u32>)> {
    let mut ppid_map = HashMap::new();
    let mut probes = Vec::new();
    let entries = fs::read_dir("/proc")?;

    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|raw| raw.parse::<u32>().ok()) else {
            continue;
        };
        let proc_dir = entry.path();
        if let Some(ppid) = fs::read_to_string(proc_dir.join("stat"))
            .ok()
            .and_then(|stat| parse_stat_ppid(&stat))
        {
            ppid_map.insert(pid, ppid);
        }

        let Some(args) = read_cmdline(&proc_dir.join("cmdline")) else {
            continue;
        };
        let comm = fs::read_to_string(proc_dir.join("comm"))
            .ok()
            .map(|raw| raw.trim().to_string());
        let Some(kind) = detect_agent_kind(&args, comm.as_deref()) else {
            continue;
        };
        if options.agent.is_some_and(|filter| filter != kind) {
            continue;
        }
        let Ok(environ) = fs::read(proc_dir.join("environ")) else {
            continue;
        };
        if env_value(&environ, "LIMUX_WORKSPACE_ID").is_none()
            || env_value(&environ, "LIMUX_SURFACE_ID").is_none()
        {
            continue;
        }
        let cwd = fs::read_link(proc_dir.join("cwd"))
            .ok()
            .and_then(|path| path.to_str().and_then(non_empty));
        let open_paths = read_open_fd_paths(&proc_dir.join("fd"));

        probes.push(ProcessProbe {
            pid,
            args,
            comm,
            cwd,
            environ,
            open_paths,
        });
    }

    Ok((probes, ppid_map))
}

#[cfg(target_os = "linux")]
fn parse_stat_ppid(stat: &str) -> Option<u32> {
    let close = stat.rfind(')')?;
    let mut fields = stat.get(close + 2..)?.split_whitespace();
    fields.next()?;
    fields.next()?.parse().ok()
}

#[cfg(target_os = "linux")]
fn read_cmdline(path: &Path) -> Option<Vec<String>> {
    let raw = fs::read(path).ok()?;
    let parts: Vec<String> = raw
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .filter_map(|part| std::str::from_utf8(part).ok().map(str::to_string))
        .collect();
    (!parts.is_empty()).then_some(parts)
}

#[cfg(target_os = "linux")]
fn read_open_fd_paths(fd_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(fd_dir) else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    for entry in entries.flatten() {
        if let Ok(target) = fs::read_link(entry.path()) {
            paths.push(target);
        }
    }
    paths.sort();
    paths.dedup();
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn vec_str(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_string()).collect()
    }

    fn encode_env(pairs: &[(&str, &str)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (key, value) in pairs {
            out.extend_from_slice(key.as_bytes());
            out.push(b'=');
            out.extend_from_slice(value.as_bytes());
            out.push(0);
        }
        out
    }

    fn no_file_read(_path: &Path) -> Option<String> {
        None
    }

    #[test]
    fn agy_extraction_prefers_live_presence_lock_over_cmdline_and_db() {
        let args = vec_str(&[
            "agy",
            "--conversation",
            "old-cmdline-conv",
            "--dangerously-skip-permissions",
        ]);
        let open_paths = vec![
            PathBuf::from("/home/u/.gemini/antigravity-cli/conversation_summaries.db"),
            PathBuf::from("/home/u/.gemini/antigravity-cli/presence/deleted-id.lock (deleted)"),
            PathBuf::from("/home/u/.gemini/antigravity-cli/conversations/db-conv-id.db"),
            PathBuf::from("/home/u/.gemini/antigravity-cli/presence/live-presence-id.lock"),
        ];

        assert_eq!(
            extract_session_id(AgentKind::Agy, &args, &[], &open_paths, &no_file_read),
            Some(("live-presence-id".to_string(), SessionIdSource::OpenFd))
        );

        let db_only = vec![
            PathBuf::from("/home/u/.gemini/antigravity-cli/conversation_summaries.db"),
            PathBuf::from("/home/u/.gemini/antigravity-cli/conversations/db-conv-id.db-wal"),
            PathBuf::from("/home/u/.gemini/antigravity-cli/conversations/db-conv-id.db"),
        ];
        assert_eq!(
            extract_session_id(AgentKind::Agy, &args, &[], &db_only, &no_file_read),
            Some(("db-conv-id".to_string(), SessionIdSource::OpenFd))
        );

        assert_eq!(
            extract_session_id(AgentKind::Agy, &args, &[], &[], &no_file_read),
            Some(("old-cmdline-conv".to_string(), SessionIdSource::Cmdline))
        );
    }

    #[test]
    fn claude_codex_gemini_and_opencode_extractors_handle_fds_env_and_cmdline() {
        let claude_uuid = "79254d1f-6861-4eb4-add9-b96802294ec0";
        let claude_paths = vec![
            PathBuf::from("/home/u/.claude/projects/-tmp-repo/sessions-index.jsonl"),
            PathBuf::from("/home/u/.claude/projects/-tmp-repo/agent-1234.jsonl"),
            PathBuf::from(format!(
                "/home/u/.claude/projects/-tmp-repo/{claude_uuid}.jsonl"
            )),
        ];
        assert_eq!(
            extract_session_id(
                AgentKind::Claude,
                &vec_str(&["claude"]),
                &[],
                &claude_paths,
                &no_file_read,
            ),
            Some((claude_uuid.to_string(), SessionIdSource::OpenFd))
        );
        assert_eq!(
            extract_session_id(
                AgentKind::Claude,
                &vec_str(&["node", "/usr/lib/claude/cli.js", "-r", "sess-claude"]),
                &[],
                &[],
                &no_file_read,
            ),
            Some(("sess-claude".to_string(), SessionIdSource::Cmdline))
        );
        assert_eq!(
            extract_session_id(
                AgentKind::Claude,
                &vec_str(&["claude"]),
                &encode_env(&[("CLAUDE_CODE_SESSION_ID", "env-claude")]),
                &[],
                &no_file_read,
            ),
            Some(("env-claude".to_string(), SessionIdSource::Env))
        );

        let codex_uuid = "01a20861-8ccf-7250-9a82-dffd64a43e58";
        let codex_paths = vec![PathBuf::from(format!(
            "/home/u/.codex/sessions/2026/11/23/rollout-2026-11-23T07-29-01-{codex_uuid}.jsonl"
        ))];
        assert_eq!(
            extract_session_id(
                AgentKind::Codex,
                &vec_str(&["codex"]),
                &[],
                &codex_paths,
                &no_file_read,
            ),
            Some((codex_uuid.to_string(), SessionIdSource::OpenFd))
        );
        assert_eq!(
            extract_session_id(
                AgentKind::Codex,
                &vec_str(&["codex", "resume", "-m", "gpt-5.4", "thread-123"]),
                &[],
                &[],
                &no_file_read,
            ),
            Some(("thread-123".to_string(), SessionIdSource::Cmdline))
        );
        assert_eq!(
            extract_session_id(
                AgentKind::Codex,
                &vec_str(&["codex"]),
                &encode_env(&[("CODEX_THREAD_ID", "env-codex")]),
                &[],
                &no_file_read,
            ),
            Some(("env-codex".to_string(), SessionIdSource::Env))
        );

        let gemini_path =
            PathBuf::from("/home/u/.gemini/tmp/repo/chats/session-2026-04-12-9082.jsonl");
        assert_eq!(
            extract_session_id(
                AgentKind::Gemini,
                &vec_str(&["gemini", "--resume", "latest"]),
                &[],
                &[gemini_path],
                &|_| Some(r#"{"sessionId":"9082d221-7993-4214-8882-78c3696f974f"}"#.to_string()),
            ),
            Some((
                "9082d221-7993-4214-8882-78c3696f974f".to_string(),
                SessionIdSource::OpenFd,
            ))
        );
        assert_eq!(
            extract_session_id(
                AgentKind::Gemini,
                &vec_str(&["gemini", "--resume", "latest"]),
                &[],
                &[],
                &no_file_read,
            ),
            None
        );

        assert_eq!(
            extract_session_id(
                AgentKind::OpenCode,
                &vec_str(&["opencode", "-s", "oc-42"]),
                &[],
                &[],
                &no_file_read,
            ),
            Some(("oc-42".to_string(), SessionIdSource::Cmdline))
        );
    }

    #[test]
    fn snapshot_deduplicates_nested_agents_supersedes_stale_records_and_respects_filters() {
        let dir = tempdir().expect("tempdir");
        let agy_store = AgentHookSessionStore::new_for_dir("agy", dir.path());
        // Seed a stale dead record on the same surface as live pid 200.
        agy_store
            .update("stale-dead-session", |_| AgentHookSessionRecord {
                session_id: "stale-dead-session".to_string(),
                workspace_id: "ws-1".to_string(),
                surface_id: "1:tab-a".to_string(),
                cwd: Some("/repo/a".to_string()),
                pid: Some(99999),
                launch_command: None,
                updated_at: 100.0,
            })
            .expect("seed stale");
        // Seed an already-tracked live Codex PID with no open FD or cmdline session.
        let codex_store = AgentHookSessionStore::new_for_dir("codex", dir.path());
        codex_store
            .update("codex-existing-1", |_| AgentHookSessionRecord {
                session_id: "codex-existing-1".to_string(),
                workspace_id: "ws-2".to_string(),
                surface_id: "2:tab-b".to_string(),
                cwd: Some("/repo/b".to_string()),
                pid: Some(300),
                launch_command: None,
                updated_at: 100.0,
            })
            .expect("seed codex");

        let probes = vec![
            // Interactive outer agy on ws-1 / 1:tab-a
            ProcessProbe {
                pid: 200,
                args: vec_str(&["agy", "--dangerously-skip-permissions"]),
                comm: Some("agy".to_string()),
                cwd: Some("/repo/a".to_string()),
                environ: encode_env(&[
                    ("LIMUX_WORKSPACE_ID", "ws-1"),
                    ("LIMUX_SURFACE_ID", "1:tab-a"),
                    ("LIMUX_SOCKET", "/tmp/target.sock"),
                ]),
                open_paths: vec![PathBuf::from(
                    "/home/u/.gemini/antigravity-cli/presence/live-agy-1.lock",
                )],
            },
            // Nested child agy spawned inside pid 200 on the same surface (must be ignored)
            ProcessProbe {
                pid: 201,
                args: vec_str(&["agy", "--conversation", "nested-child-conv"]),
                comm: Some("agy".to_string()),
                cwd: Some("/repo/a".to_string()),
                environ: encode_env(&[
                    ("LIMUX_WORKSPACE_ID", "ws-1"),
                    ("LIMUX_SURFACE_ID", "1:tab-a"),
                    ("LIMUX_SOCKET", "/tmp/target.sock"),
                ]),
                open_paths: vec![],
            },
            // Live codex with session only known from existing hook store
            ProcessProbe {
                pid: 300,
                args: vec_str(&["codex", "--full-auto"]),
                comm: Some("codex".to_string()),
                cwd: Some("/repo/b".to_string()),
                environ: encode_env(&[
                    ("LIMUX_WORKSPACE_ID", "ws-2"),
                    ("LIMUX_SURFACE_ID", "2:tab-b"),
                    ("LIMUX_SOCKET", "/tmp/target.sock"),
                    ("CODEX_HOME", "/tmp/custom-codex"),
                ]),
                open_paths: vec![],
            },
            // Live agy with no conversation started yet (must appear in skipped)
            ProcessProbe {
                pid: 400,
                args: vec_str(&["agy", "--dangerously-skip-permissions"]),
                comm: Some("agy".to_string()),
                cwd: Some("/repo/c".to_string()),
                environ: encode_env(&[
                    ("LIMUX_WORKSPACE_ID", "ws-3"),
                    ("LIMUX_SURFACE_ID", "3:tab-c"),
                    ("LIMUX_SOCKET", "/tmp/target.sock"),
                ]),
                open_paths: vec![],
            },
            // Agent on a different Limux socket (must be excluded unless all_sockets)
            ProcessProbe {
                pid: 500,
                args: vec_str(&["agy", "--conversation", "other-sock-conv"]),
                comm: Some("agy".to_string()),
                cwd: Some("/repo/other".to_string()),
                environ: encode_env(&[
                    ("LIMUX_WORKSPACE_ID", "ws-9"),
                    ("LIMUX_SURFACE_ID", "9:tab-z"),
                    ("LIMUX_SOCKET", "/tmp/other.sock"),
                ]),
                open_paths: vec![],
            },
        ];

        let ppid_map: HashMap<u32, u32> =
            [(200, 100), (201, 200), (300, 101), (400, 102), (500, 103)]
                .into_iter()
                .collect();
        let options = SnapshotOptions {
            agent: None,
            workspace_id: None,
            surface_id: None,
            target_socket: Some(PathBuf::from("/tmp/target.sock")),
            all_sockets: false,
            dry_run: false,
        };

        let report = snapshot_from_probes(
            probes,
            &ppid_map,
            &options,
            |kind| AgentHookSessionStore::new_for_dir(kind.store_name(), dir.path()),
            no_file_read,
        )
        .expect("snapshot");

        assert_eq!(report.captured.len(), 2);
        assert_eq!(report.captured[0].agent, "agy");
        assert_eq!(report.captured[0].session_id, "live-agy-1");
        assert_eq!(report.captured[0].source, SessionIdSource::OpenFd);
        assert_eq!(
            report.captured[0].superseded_session_ids,
            vec!["stale-dead-session".to_string()]
        );

        assert_eq!(report.captured[1].agent, "codex");
        assert_eq!(report.captured[1].session_id, "codex-existing-1");
        assert_eq!(
            report.captured[1].source,
            SessionIdSource::ExistingHookStore
        );

        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].pid, 400);
        assert_eq!(report.skipped[0].reason, "no_session_id");

        // Verify stale record was removed and new live record persisted cleanly.
        let agy_raw: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.path().join("agy-hook-sessions.json")).unwrap())
                .unwrap();
        let agy_sessions = agy_raw["sessions"].as_object().unwrap();
        assert_eq!(agy_sessions.len(), 1);
        assert!(agy_sessions.contains_key("live-agy-1"));
        assert!(!agy_sessions.contains_key("stale-dead-session"));
        assert_eq!(
            agy_sessions["live-agy-1"]["launch_command"]["arguments"],
            serde_json::json!(["agy", "--dangerously-skip-permissions"])
        );

        let codex_raw: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.path().join("codex-hook-sessions.json")).unwrap())
                .unwrap();
        assert_eq!(
            codex_raw["sessions"]["codex-existing-1"]["launch_command"]["environment"]
                ["CODEX_HOME"],
            "/tmp/custom-codex"
        );
    }

    #[test]
    fn dry_run_does_not_write_hook_store() {
        let dir = tempdir().expect("tempdir");
        let probes = vec![ProcessProbe {
            pid: 200,
            args: vec_str(&["agy", "--dangerously-skip-permissions"]),
            comm: Some("agy".to_string()),
            cwd: Some("/repo/a".to_string()),
            environ: encode_env(&[
                ("LIMUX_WORKSPACE_ID", "ws-1"),
                ("LIMUX_SURFACE_ID", "1:tab-a"),
            ]),
            open_paths: vec![PathBuf::from(
                "/home/u/.gemini/antigravity-cli/presence/dry-run-conv.lock",
            )],
        }];
        let ppid_map: HashMap<u32, u32> = [(200, 100)].into_iter().collect();
        let options = SnapshotOptions {
            agent: None,
            workspace_id: None,
            surface_id: None,
            target_socket: None,
            all_sockets: true,
            dry_run: true,
        };

        let report = snapshot_from_probes(
            probes,
            &ppid_map,
            &options,
            |kind| AgentHookSessionStore::new_for_dir(kind.store_name(), dir.path()),
            no_file_read,
        )
        .expect("dry-run snapshot");

        assert!(report.dry_run);
        assert_eq!(report.captured.len(), 1);
        assert!(!dir.path().join("agy-hook-sessions.json").exists());
    }
}
