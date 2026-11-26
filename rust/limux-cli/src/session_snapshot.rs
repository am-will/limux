use anyhow::Result;
#[cfg(any(target_os = "linux", test))]
use limux_control::agent_sessions::normalize_captured_process_args;
#[cfg(any(target_os = "linux", test))]
use limux_control::agent_sessions::proc_matches_agent;
use limux_control::agent_sessions::AgentKind;
use serde::Serialize;
#[cfg(any(target_os = "linux", test))]
use std::collections::{BTreeMap, HashMap, HashSet};
#[cfg(any(target_os = "linux", test))]
use std::fs;
#[cfg(target_os = "linux")]
use std::io::Read;
#[cfg(target_os = "linux")]
use std::os::unix::fs::MetadataExt;
#[cfg(any(target_os = "linux", test))]
use std::path::Path;
use std::path::PathBuf;

#[cfg(any(target_os = "linux", test))]
use crate::agent_hooks::{
    filter_allowed_environment, launch_record_from_process, now_seconds, tab_identity,
    AgentHookSessionRecord, AgentHookSessionStore,
};

#[cfg(any(target_os = "linux", test))]
const MAX_SESSION_FILE_PREFIX_BYTES: u64 = 64 * 1024;

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

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Debug)]
struct ProcessProbe {
    pid: u32,
    args: Vec<String>,
    comm: Option<String>,
    cwd: Option<String>,
    environ: Vec<u8>,
    open_paths: Vec<PathBuf>,
}

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Debug)]
struct MatchedCandidate {
    kind: AgentKind,
    probe: ProcessProbe,
    workspace_id: String,
    surface_id: String,
}

#[cfg(any(target_os = "linux", test))]
struct ResolvedCandidate {
    candidate: MatchedCandidate,
    discovered: Option<(String, SessionIdSource)>,
}

pub(crate) fn snapshot_running_agents(options: &SnapshotOptions) -> Result<SnapshotReport> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = options;
        anyhow::bail!("live agent process snapshotting is currently supported on Linux only");
    }

    #[cfg(target_os = "linux")]
    {
        let (probes, ppid_map) = scan_linux_proc_agents()?;
        snapshot_from_probes(
            probes,
            &ppid_map,
            options,
            AgentHookSessionStore::new,
            read_file_prefix_bounded,
        )
    }
}

#[cfg(any(target_os = "linux", test))]
fn source_priority(source: SessionIdSource) -> u8 {
    match source {
        SessionIdSource::OpenFd => 4,
        SessionIdSource::ExistingHookStore => 3,
        SessionIdSource::Env => 2,
        SessionIdSource::Cmdline => 1,
    }
}

#[cfg(any(target_os = "linux", test))]
fn snapshot_from_probes(
    probes: Vec<ProcessProbe>,
    ppid_map: &HashMap<u32, u32>,
    options: &SnapshotOptions,
    store_for: impl Fn(AgentKind) -> AgentHookSessionStore,
    read_file_prefix: impl Fn(&Path) -> Option<String>,
) -> Result<SnapshotReport> {
    let target_socket = options
        .target_socket
        .as_deref()
        .map(canonical_or_original_path);

    // Keep all agent kinds while building the surface process tree so cross-agent
    // nesting (e.g. Claude invoking Codex on the same surface) is still deduplicated
    // when `--agent` filters the final output.
    let mut resolved = Vec::new();
    for probe in probes {
        let Some(kind) = detect_agent_kind(&probe.args, probe.comm.as_deref()) else {
            continue;
        };
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
        let store = store_for(kind);
        let extracted = extract_session_id(
            kind,
            &probe.args,
            &probe.environ,
            &probe.open_paths,
            &read_file_prefix,
        );
        // The environment and arguments are fixed at launch, while hooks follow the
        // process when it starts, forks or switches to another session.
        let discovered = match extracted {
            Some((_, SessionIdSource::OpenFd)) => extracted,
            _ => store
                .session_for_live_pid(probe.pid, &workspace_id, &surface_id)?
                .map(|session_id| (session_id, SessionIdSource::ExistingHookStore))
                .or(extracted),
        };
        resolved.push(ResolvedCandidate {
            candidate: MatchedCandidate {
                kind,
                probe,
                workspace_id,
                surface_id,
            },
            discovered,
        });
    }

    let relaunchers = relaunching_parents(&resolved, ppid_map);
    for entry in &mut resolved {
        if relaunchers.contains(&entry.candidate.probe.pid) {
            entry.discovered = None;
        }
    }

    // Resolve session IDs before parent/child deduplication so launcher wrappers
    // (such as npm `codex.js` or relaunched `gemini`) without an open session FD
    // yield to their session-bearing child, while an outer agent with a session
    // still suppresses nested subagents on the same surface.
    let candidate_info: HashMap<u32, (&str, &str, bool)> = resolved
        .iter()
        .map(|entry| {
            (
                entry.candidate.probe.pid,
                (
                    entry.candidate.workspace_id.as_str(),
                    entry.candidate.surface_id.as_str(),
                    entry.discovered.is_some(),
                ),
            )
        })
        .collect();

    let mut wrappers_with_session_descendant = HashSet::new();
    for entry in &resolved {
        if entry.discovered.is_none() {
            continue;
        }
        let mut visited = HashSet::new();
        let mut current = entry.candidate.probe.pid;
        while visited.insert(current) {
            let Some(&parent) = ppid_map.get(&current) else {
                break;
            };
            if parent <= 1 || parent == current {
                break;
            }
            if let Some(&(ws, surf, has_session)) = candidate_info.get(&parent) {
                if ws == entry.candidate.workspace_id
                    && surf == entry.candidate.surface_id
                    && !has_session
                {
                    wrappers_with_session_descendant.insert(parent);
                }
            }
            current = parent;
        }
    }

    let keep_pids: HashSet<u32> = resolved
        .iter()
        .filter(|entry| {
            let pid = entry.candidate.probe.pid;
            let ws = entry.candidate.workspace_id.as_str();
            let surf = entry.candidate.surface_id.as_str();
            if has_ancestor_on_same_surface(pid, ws, surf, ppid_map, &candidate_info, true) {
                return false;
            }
            if entry.discovered.is_none() {
                if wrappers_with_session_descendant.contains(&pid) {
                    return false;
                }
                if has_ancestor_on_same_surface(pid, ws, surf, ppid_map, &candidate_info, false) {
                    return false;
                }
            }
            true
        })
        .filter(|entry| {
            options
                .agent
                .is_none_or(|filter| filter == entry.candidate.kind)
        })
        .map(|entry| entry.candidate.probe.pid)
        .collect();
    drop(candidate_info);

    let mut surviving: Vec<ResolvedCandidate> = resolved
        .into_iter()
        .filter(|entry| keep_pids.contains(&entry.candidate.probe.pid))
        .collect();

    surviving.sort_by(|a, b| {
        a.candidate
            .workspace_id
            .cmp(&b.candidate.workspace_id)
            .then_with(|| a.candidate.surface_id.cmp(&b.candidate.surface_id))
            .then_with(|| a.candidate.probe.pid.cmp(&b.candidate.probe.pid))
    });

    // If two independent (non-ancestor) session-bearing processes of the same agent
    // share one surface/tab, pick one winner so `report.captured` matches the single
    // active binding kept by `upsert_snapshot`.
    let mut best_by_agent_surface: HashMap<(AgentKind, String, String), usize> = HashMap::new();
    for (idx, entry) in surviving.iter().enumerate() {
        let Some((_, source)) = entry.discovered else {
            continue;
        };
        let tab = tab_identity(&entry.candidate.surface_id);
        let surface_key = if tab.is_empty() {
            entry.candidate.surface_id.clone()
        } else {
            tab.to_string()
        };
        let key = (
            entry.candidate.kind,
            entry.candidate.workspace_id.clone(),
            surface_key,
        );
        let replace = match best_by_agent_surface.get(&key) {
            None => true,
            Some(&prev_idx) => {
                let prev = &surviving[prev_idx];
                let prev_prio = prev
                    .discovered
                    .as_ref()
                    .map(|(_, s)| source_priority(*s))
                    .unwrap_or(0);
                let next_prio = source_priority(source);
                (next_prio, entry.candidate.probe.pid) > (prev_prio, prev.candidate.probe.pid)
            }
        };
        if replace {
            best_by_agent_surface.insert(key, idx);
        }
    }
    let winning_indices: HashSet<usize> = best_by_agent_surface.into_values().collect();

    let mut captured = Vec::new();
    let mut skipped = Vec::new();

    for (idx, entry) in surviving.into_iter().enumerate() {
        let candidate = entry.candidate;
        let Some((session_id, source)) = entry.discovered else {
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
        if !winning_indices.contains(&idx) {
            continue;
        }

        let store = store_for(candidate.kind);
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

/// Launchers that run the agent again with their own arguments, such as npm
/// `codex.js` or `gemini`. When all they know is how they were launched, the
/// child knows that too and may have moved on to another session.
#[cfg(any(target_os = "linux", test))]
fn relaunching_parents(
    resolved: &[ResolvedCandidate],
    ppid_map: &HashMap<u32, u32>,
) -> HashSet<u32> {
    fn options(candidate: &MatchedCandidate) -> Vec<String> {
        let args = &candidate.probe.args;
        normalize_captured_process_args(candidate.kind, args)
            .unwrap_or_else(|| args.clone())
            .into_iter()
            .skip(1)
            .collect()
    }
    resolved
        .iter()
        .filter(|entry| entry.discovered.is_some())
        .filter_map(|entry| {
            let child = &entry.candidate;
            let parent_pid = ppid_map.get(&child.probe.pid)?;
            let parent = resolved
                .iter()
                .find(|entry| entry.candidate.probe.pid == *parent_pid)?;
            let launch_only = matches!(
                parent.discovered,
                Some((_, SessionIdSource::Env | SessionIdSource::Cmdline))
            );
            let parent = &parent.candidate;
            (launch_only
                && parent.kind == child.kind
                && parent.workspace_id == child.workspace_id
                && parent.surface_id == child.surface_id
                && options(parent) == options(child))
            .then_some(*parent_pid)
        })
        .collect()
}

#[cfg(any(target_os = "linux", test))]
fn has_ancestor_on_same_surface(
    pid: u32,
    workspace_id: &str,
    surface_id: &str,
    ppid_map: &HashMap<u32, u32>,
    candidate_info: &HashMap<u32, (&str, &str, bool)>,
    require_session: bool,
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
        if let Some(&(ws, surf, has_session)) = candidate_info.get(&parent) {
            if ws == workspace_id && surf == surface_id && (!require_session || has_session) {
                return true;
            }
        }
        current = parent;
    }
    false
}

#[cfg(any(target_os = "linux", test))]
fn detect_agent_kind(args: &[String], comm: Option<&str>) -> Option<AgentKind> {
    // Check Agy before Gemini because Antigravity installs under `~/.gemini/antigravity-cli`.
    const DETECTION_ORDER: [AgentKind; 5] = [
        AgentKind::Agy,
        AgentKind::Claude,
        AgentKind::Codex,
        AgentKind::OpenCode,
        AgentKind::Gemini,
    ];
    DETECTION_ORDER
        .into_iter()
        .find(|&kind| proc_matches_agent(args, comm, kind))
}

#[cfg(any(target_os = "linux", test))]
fn extract_session_id(
    kind: AgentKind,
    args: &[String],
    environ: &[u8],
    open_paths: &[PathBuf],
    read_file_prefix: &impl Fn(&Path) -> Option<String>,
) -> Option<(String, SessionIdSource)> {
    // Open file descriptors reflect the currently active session even if a
    // resumed process later switched conversations inside the same TUI.
    if let Some(session_id) = extract_session_id_from_open_paths(kind, open_paths, read_file_prefix)
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

#[cfg(any(target_os = "linux", test))]
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

#[cfg(any(target_os = "linux", test))]
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

#[cfg(any(target_os = "linux", test))]
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

#[cfg(any(target_os = "linux", test))]
fn codex_option_takes_value(arg: &str) -> bool {
    matches!(
        arg,
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
            | "-i"
            | "--image"
    )
}

#[cfg(any(target_os = "linux", test))]
fn extract_codex_resume_arg(args: &[String]) -> Option<String> {
    // Locate the first positional subcommand token while skipping top-level options
    // and their values (so `codex -p resume "prompt"` is not misread as `resume`).
    let mut idx = 1;
    let mut resume_idx = None;
    while idx < args.len() {
        let arg = &args[idx];
        if arg == "--" {
            break;
        }
        if codex_option_takes_value(arg) {
            idx += 2;
            continue;
        }
        if arg.starts_with('-') {
            idx += 1;
            continue;
        }
        if arg == "resume" {
            resume_idx = Some(idx);
        }
        break;
    }
    let resume_idx = resume_idx?;

    let mut idx = resume_idx + 1;
    let mut candidate: Option<String> = None;
    while idx < args.len() {
        let arg = &args[idx];
        if arg == "--" {
            if candidate.is_none() {
                candidate = args.get(idx + 1).and_then(|next| non_empty(next));
            }
            break;
        }
        if matches!(arg.as_str(), "--last" | "--all") {
            // `--last` / `--all` select a session without a positional session id, so any
            // positional argument is a user prompt rather than a session identifier.
            return None;
        }
        if codex_option_takes_value(arg) {
            idx += 2;
            continue;
        }
        if arg.starts_with('-') {
            idx += 1;
            continue;
        }
        if candidate.is_none() {
            candidate = non_empty(arg);
        }
        idx += 1;
    }
    candidate
}

#[cfg(any(target_os = "linux", test))]
fn extract_session_id_from_open_paths(
    kind: AgentKind,
    open_paths: &[PathBuf],
    read_file_prefix: &impl Fn(&Path) -> Option<String>,
) -> Option<String> {
    let live_paths: Vec<&Path> = open_paths
        .iter()
        .map(PathBuf::as_path)
        .filter(|path| path.to_str().is_none_or(|raw| !raw.ends_with(" (deleted)")))
        .collect();

    // `read_open_fd_paths` sorts lexicographically ascending, so iterate in reverse
    // to prefer the newest timestamped session file when more than one is open.
    match kind {
        AgentKind::Agy => {
            // Prefer presence/<id>.lock (held for the entire attached conversation),
            // falling back to conversations/<id>.db.
            live_paths
                .iter()
                .rev()
                .find_map(|path| extract_agy_presence_id(path))
                .or_else(|| {
                    live_paths
                        .iter()
                        .rev()
                        .find_map(|path| extract_agy_conversation_db_id(path))
                })
        }
        AgentKind::Claude => live_paths
            .iter()
            .rev()
            .find_map(|path| extract_claude_project_jsonl_id(path)),
        AgentKind::Codex => live_paths
            .iter()
            .rev()
            .find_map(|path| extract_codex_rollout_jsonl_id(path)),
        AgentKind::Gemini => live_paths
            .iter()
            .rev()
            .find_map(|path| extract_gemini_chat_session_id(path, read_file_prefix)),
        AgentKind::OpenCode => None,
    }
}

#[cfg(any(target_os = "linux", test))]
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

#[cfg(any(target_os = "linux", test))]
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

#[cfg(any(target_os = "linux", test))]
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

#[cfg(any(target_os = "linux", test))]
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

#[cfg(any(target_os = "linux", test))]
fn extract_gemini_chat_session_id(
    path: &Path,
    read_file_prefix: &impl Fn(&Path) -> Option<String>,
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
    let content = read_file_prefix(path)?;
    if ext == "jsonl" {
        let first_line = content
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())?;
        return extract_json_top_level_string_field(first_line, &["sessionId", "session_id"]);
    }
    extract_json_top_level_string_field(&content, &["sessionId", "session_id"])
}

#[cfg(any(target_os = "linux", test))]
fn extract_json_top_level_string_field(content: &str, keys: &[&str]) -> Option<String> {
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(content.trim()) {
        return keys
            .iter()
            .find_map(|key| parsed.get(*key).and_then(serde_json::Value::as_str))
            .and_then(non_empty);
    }

    // Fallback for truncated large `.json` files: scan top-level object pairs
    // (brace depth == 1, outside arrays) without parsing the whole conversation.
    let bytes = content.as_bytes();
    let mut i = 0;
    let mut depth = 0usize;
    let mut array_depth = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            b'[' => {
                array_depth += 1;
                i += 1;
            }
            b']' => {
                array_depth = array_depth.saturating_sub(1);
                i += 1;
            }
            b'"' => {
                let (token, next_i) = scan_json_string(content, i)?;
                i = next_i;
                if depth == 1 && array_depth == 0 {
                    let mut j = i;
                    while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    if bytes.get(j) == Some(&b':') {
                        j += 1;
                        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                            j += 1;
                        }
                        if keys.contains(&token.as_str()) && bytes.get(j) == Some(&b'"') {
                            let (value, _) = scan_json_string(content, j)?;
                            if let Some(value) = non_empty(&value) {
                                return Some(value);
                            }
                        }
                        i = j;
                    }
                }
            }
            _ => i += 1,
        }
    }
    None
}

#[cfg(any(target_os = "linux", test))]
fn scan_json_string(content: &str, start: usize) -> Option<(String, usize)> {
    let bytes = content.as_bytes();
    if bytes.get(start) != Some(&b'"') {
        return None;
    }
    let mut i = start + 1;
    let mut escaped = false;
    while i < bytes.len() {
        let byte = bytes[i];
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            let decoded: String = serde_json::from_str(&content[start..=i]).ok()?;
            return Some((decoded, i + 1));
        }
        i += 1;
    }
    None
}

#[cfg(any(target_os = "linux", test))]
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

#[cfg(any(target_os = "linux", test))]
fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(any(target_os = "linux", test))]
fn env_value(environ: &[u8], name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    environ
        .split(|byte| *byte == 0)
        .filter_map(|part| std::str::from_utf8(part).ok())
        .find_map(|entry| entry.strip_prefix(&prefix).and_then(non_empty))
}

#[cfg(any(target_os = "linux", test))]
fn allowed_env_from_environ(environ: &[u8]) -> BTreeMap<String, String> {
    let pairs = environ
        .split(|byte| *byte == 0)
        .filter_map(|part| std::str::from_utf8(part).ok())
        .filter_map(|entry| entry.split_once('='));
    filter_allowed_environment(pairs)
}

#[cfg(any(target_os = "linux", test))]
fn canonical_or_original_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(target_os = "linux")]
fn read_file_prefix_bounded(path: &Path) -> Option<String> {
    let file = limux_control::socket_path::open_regular_file_readonly(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_SESSION_FILE_PREFIX_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.is_empty() {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(target_os = "linux")]
fn scan_linux_proc_agents() -> Result<(Vec<ProcessProbe>, HashMap<u32, u32>)> {
    let expected_uid = limux_control::socket_path::current_euid();
    let mut ppid_map = HashMap::new();
    let mut probes = Vec::new();
    let entries = fs::read_dir("/proc")?;

    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|raw| raw.parse::<u32>().ok()) else {
            continue;
        };
        let proc_dir = entry.path();
        let Ok(meta) = fs::metadata(&proc_dir) else {
            continue;
        };
        if meta.uid() != expected_uid {
            continue;
        }
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
        if detect_agent_kind(&args, comm.as_deref()).is_none() {
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

    #[test]
    fn wrapper_parent_yields_to_session_bearing_child_and_cross_agent_filter_still_dedups() {
        let dir = tempdir().expect("tempdir");
        let codex_uuid = "01a20861-8ccf-7250-9a82-dffd64a43e58";
        let probes = vec![
            // npm wrapper `node .../bin/codex.js` (pid 200) spawns native `codex` (pid 201)
            ProcessProbe {
                pid: 200,
                args: vec_str(&[
                    "/usr/bin/node",
                    "/usr/lib/node_modules/@openai/codex/bin/codex.js",
                    "--full-auto",
                ]),
                comm: Some("node".to_string()),
                cwd: Some("/repo/codex".to_string()),
                environ: encode_env(&[
                    ("LIMUX_WORKSPACE_ID", "ws-1"),
                    ("LIMUX_SURFACE_ID", "1:tab-codex"),
                ]),
                open_paths: vec![],
            },
            ProcessProbe {
                pid: 201,
                args: vec_str(&[
                    "/usr/lib/node_modules/@openai/codex/vendor/x86_64/codex",
                    "--full-auto",
                ]),
                comm: Some("codex".to_string()),
                cwd: Some("/repo/codex".to_string()),
                environ: encode_env(&[
                    ("LIMUX_WORKSPACE_ID", "ws-1"),
                    ("LIMUX_SURFACE_ID", "1:tab-codex"),
                ]),
                open_paths: vec![PathBuf::from(format!(
                    "/home/u/.codex/sessions/2026/11/23/rollout-2026-11-23T07-29-01-{codex_uuid}.jsonl"
                ))],
            },
            // Outer claude (pid 300) with a live session spawns nested codex (pid 301) on same surface
            ProcessProbe {
                pid: 300,
                args: vec_str(&["claude", "--dangerously-skip-permissions"]),
                comm: Some("claude".to_string()),
                cwd: Some("/repo/claude".to_string()),
                environ: encode_env(&[
                    ("LIMUX_WORKSPACE_ID", "ws-2"),
                    ("LIMUX_SURFACE_ID", "2:tab-claude"),
                    ("CLAUDE_CODE_SESSION_ID", "outer-claude-sess"),
                ]),
                open_paths: vec![],
            },
            ProcessProbe {
                pid: 301,
                args: vec_str(&["codex", "resume", "nested-codex-sess"]),
                comm: Some("codex".to_string()),
                cwd: Some("/repo/claude".to_string()),
                environ: encode_env(&[
                    ("LIMUX_WORKSPACE_ID", "ws-2"),
                    ("LIMUX_SURFACE_ID", "2:tab-claude"),
                ]),
                open_paths: vec![],
            },
        ];
        let ppid_map: HashMap<u32, u32> = [(200, 100), (201, 200), (300, 101), (301, 300)]
            .into_iter()
            .collect();
        let options = SnapshotOptions {
            agent: Some(AgentKind::Codex),
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
        .expect("wrapper and cross-agent snapshot");

        // Wrapper pid 200 yielded to session-bearing child pid 201; nested codex pid 301
        // under session-bearing claude pid 300 was suppressed despite `--agent codex`.
        assert_eq!(report.captured.len(), 1);
        assert_eq!(report.captured[0].pid, 201);
        assert_eq!(report.captured[0].session_id, codex_uuid);
        assert!(report.skipped.is_empty());
    }

    #[test]
    fn recorded_session_of_a_live_process_outranks_its_stale_resume_argument() {
        let dir = tempdir().expect("tempdir");
        let store_for =
            |kind: AgentKind| AgentHookSessionStore::new_for_dir(kind.store_name(), dir.path());
        let record = |kind: AgentKind, session_id: &str, surface_id: &str, pid: u32| {
            store_for(kind)
                .update(session_id, |_| AgentHookSessionRecord {
                    session_id: session_id.to_string(),
                    workspace_id: "ws".to_string(),
                    surface_id: surface_id.to_string(),
                    cwd: None,
                    pid: Some(pid),
                    launch_command: None,
                    updated_at: 100.0,
                })
                .expect("seed record");
        };
        let probe = |pid: u32, surface_id: &str, args: &[&str]| ProcessProbe {
            pid,
            args: vec_str(args),
            comm: None,
            cwd: None,
            environ: encode_env(&[
                ("LIMUX_WORKSPACE_ID", "ws"),
                ("LIMUX_SURFACE_ID", surface_id),
            ]),
            open_paths: vec![],
        };
        // Both agents were resumed and then moved to a new session.
        record(AgentKind::Claude, "claude-now", "1:tab-a", 200);
        record(AgentKind::Gemini, "gemini-now", "1:tab-b", 301);
        let probes = vec![
            probe(200, "1:tab-a", &["claude", "--resume", "claude-old"]),
            probe(
                300,
                "1:tab-b",
                &["node", "/usr/bin/gemini", "--resume", "gemini-old"],
            ),
            probe(
                301,
                "1:tab-b",
                &[
                    "node",
                    "--max-old-space-size=8192",
                    "/usr/bin/gemini",
                    "--resume",
                    "gemini-old",
                ],
            ),
        ];
        let options = SnapshotOptions {
            agent: None,
            workspace_id: None,
            surface_id: None,
            target_socket: None,
            all_sockets: true,
            dry_run: false,
        };

        let report = snapshot_from_probes(
            probes,
            &HashMap::from([(200, 100), (300, 100), (301, 300)]),
            &options,
            store_for,
            no_file_read,
        )
        .expect("snapshot");

        let captured: Vec<_> = report
            .captured
            .iter()
            .map(|item| (item.pid, item.session_id.as_str()))
            .collect();
        assert_eq!(captured, [(200, "claude-now"), (301, "gemini-now")]);
        assert!(report
            .captured
            .iter()
            .all(|item| item.superseded_session_ids.is_empty()));
    }

    #[test]
    fn codex_resume_ignores_last_prompt_and_option_value_named_resume() {
        assert_eq!(
            extract_session_id(
                AgentKind::Codex,
                &vec_str(&["codex", "resume", "--last", "fix the failing test"]),
                &[],
                &[],
                &no_file_read,
            ),
            None
        );
        assert_eq!(
            extract_session_id(
                AgentKind::Codex,
                &vec_str(&["codex", "-p", "resume", "do some work"]),
                &[],
                &[],
                &no_file_read,
            ),
            None
        );
        assert_eq!(
            extract_session_id(
                AgentKind::Codex,
                &vec_str(&["codex", "-m", "gpt-5.4", "resume", "thread-456"]),
                &[],
                &[],
                &no_file_read,
            ),
            Some(("thread-456".to_string(), SessionIdSource::Cmdline))
        );
    }

    #[test]
    fn gemini_json_extractor_handles_pretty_printed_large_json_and_prefers_newest_fd() {
        let dir = tempdir().expect("tempdir");
        let chats = dir.path().join("chats");
        fs::create_dir_all(&chats).expect("create chats dir");

        // Pretty-printed multi-line JSON larger than 64 KiB (prefix truncated mid-messages).
        let large_json = chats.join("session-2026-11-23-big.json");
        let mut body = String::from(
            "{\n  \"meta\": {\"nested\": {\"sessionId\": \"wrong-nested\"}},\n  \"sessionId\": \"gemini-pretty-123\",\n  \"messages\": [\n",
        );
        for i in 0..2000 {
            body.push_str(&format!(
                "    {{\"role\": \"user\", \"text\": \"padding line {i:04} 0123456789abcdef0123456789abcdef\"}},\n"
            ));
        }
        body.push_str("    {\"role\": \"user\", \"text\": \"end\"}\n  ]\n}\n");
        assert!(body.len() > MAX_SESSION_FILE_PREFIX_BYTES as usize);
        fs::write(&large_json, &body).expect("write large json");

        let read_prefix = |path: &Path| -> Option<String> {
            let raw = fs::read(path).ok()?;
            let len = raw.len().min(MAX_SESSION_FILE_PREFIX_BYTES as usize);
            Some(String::from_utf8_lossy(&raw[..len]).into_owned())
        };

        assert_eq!(
            extract_session_id(
                AgentKind::Gemini,
                &vec_str(&["gemini"]),
                &[],
                &[large_json],
                &read_prefix,
            ),
            Some(("gemini-pretty-123".to_string(), SessionIdSource::OpenFd))
        );

        // Multiple Codex rollout FDs sorted lexicographically: newest timestamp wins.
        let older_uuid = "11111111-1111-1111-1111-111111111111";
        let newer_uuid = "22222222-2222-2222-2222-222222222222";
        let open_paths = vec![
            PathBuf::from(format!(
                "/home/u/.codex/sessions/2026/11/23/rollout-2026-11-23T07-00-00-{older_uuid}.jsonl"
            )),
            PathBuf::from(format!(
                "/home/u/.codex/sessions/2026/11/23/rollout-2026-11-23T09-00-00-{newer_uuid}.jsonl"
            )),
        ];
        assert_eq!(
            extract_session_id(
                AgentKind::Codex,
                &vec_str(&["codex"]),
                &[],
                &open_paths,
                &no_file_read,
            ),
            Some((newer_uuid.to_string(), SessionIdSource::OpenFd))
        );
    }

    #[test]
    fn detect_agent_kind_prefers_agy_under_gemini_dir_and_rejects_unrelated_scripts() {
        assert_eq!(
            detect_agent_kind(
                &vec_str(&[
                    "/usr/bin/node",
                    "/home/u/.gemini/antigravity-cli/dist/cli.js"
                ]),
                Some("node"),
            ),
            Some(AgentKind::Agy)
        );
        assert_eq!(
            detect_agent_kind(
                &vec_str(&[
                    "/usr/bin/node",
                    "/home/u/claude-demo/server.js",
                    "-r",
                    "foo"
                ]),
                Some("node"),
            ),
            None
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn read_file_prefix_bounded_rejects_non_regular_files_without_blocking() {
        let dir = tempdir().expect("tempdir");
        let fifo = dir.path().join("session-fifo.json");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo");
        assert!(status.success());
        assert_eq!(read_file_prefix_bounded(&fifo), None);
    }
}
