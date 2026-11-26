use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use limux_control::socket_path::{resolve_socket_path, SocketMode};
use limux_protocol::{V2Request, V2Response};
use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;

mod agent_hooks;
mod agent_team_file;
mod session_snapshot;
mod terminal_notifications;

const CLI_STATE_LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const CLI_STATE_LOCK_RETRY: Duration = Duration::from_millis(25);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdFormat {
    Refs,
    Both,
    Uuids,
}

impl IdFormat {
    fn parse(raw: &str) -> Result<Self> {
        match raw {
            "refs" => Ok(Self::Refs),
            "both" => Ok(Self::Both),
            "uuids" => Ok(Self::Uuids),
            _ => bail!("--id-format must be one of refs|both|uuids"),
        }
    }
}

#[derive(Debug, Clone)]
struct GlobalOptions {
    socket: Option<PathBuf>,
    socket_mode: SocketMode,
    json_output: bool,
    id_format: IdFormat,
    request: Option<String>,
    pretty: bool,
    command_args: Vec<String>,
}

#[derive(Debug)]
enum CommandOutput {
    Text(String),
    Json(Value),
}

struct ClientConnection {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
}

struct Client {
    socket: PathBuf,
    seq: u64,
    conn: Option<ClientConnection>,
}

impl Client {
    fn new(socket: PathBuf) -> Self {
        Self {
            socket,
            seq: 0,
            conn: None,
        }
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.seq = self.seq.saturating_add(1);
        let request = V2Request {
            id: Some(Value::String(format!("cli-{}", self.seq))),
            method: method.to_string(),
            params,
        };
        self.send_request(request).await
    }

    async fn connect(&self) -> Result<ClientConnection> {
        let stream = UnixStream::connect(&self.socket)
            .await
            .with_context(|| format!("failed to connect to socket {}", self.socket.display()))?;
        limux_control::auth::verify_server_peer(
            &stream,
            limux_control::auth::SocketControlMode::from_env(),
        )
        .with_context(|| format!("untrusted server on socket {}", self.socket.display()))?;
        let (reader_half, writer) = stream.into_split();
        Ok(ClientConnection {
            reader: BufReader::new(reader_half),
            writer,
        })
    }

    async fn send_request(&mut self, request: V2Request) -> Result<Value> {
        let mut payload = serde_json::to_string(&request).context("failed to encode request")?;
        payload.push('\n');

        let mut reused = self.conn.is_some();
        let line = loop {
            if self.conn.is_none() {
                self.conn = Some(self.connect().await?);
                reused = false;
            }
            let conn = self.conn.as_mut().expect("client connection present");
            let write_result = async {
                conn.writer.write_all(payload.as_bytes()).await?;
                conn.writer.flush().await
            }
            .await;
            if let Err(err) = write_result {
                self.conn = None;
                if reused {
                    reused = false;
                    continue;
                }
                return Err(err).context("failed to write request");
            }

            let mut line = String::new();
            match conn.reader.read_line(&mut line).await {
                Ok(0) => {
                    self.conn = None;
                    if reused {
                        reused = false;
                        continue;
                    }
                    bail!("server returned an empty response");
                }
                Ok(_) => break line,
                Err(err) => {
                    self.conn = None;
                    if reused {
                        reused = false;
                        continue;
                    }
                    return Err(err).context("failed to read response");
                }
            }
        };

        if line.trim().is_empty() {
            bail!("server returned an empty response");
        }

        let response: V2Response =
            serde_json::from_str(line.trim()).context("response was not valid v2 JSON")?;

        if response.ok {
            Ok(response.result.unwrap_or_else(|| json!({})))
        } else {
            let err = response
                .error
                .ok_or_else(|| anyhow!("server returned !ok without error payload"))?;
            if err.code == -32004 {
                bail!("not_found: {}", err.message);
            }
            bail!("{}: {}", err.code, err.message);
        }
    }
}

fn parse_global_args() -> Result<GlobalOptions> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    let mut socket: Option<PathBuf> = None;
    let mut socket_mode = SocketMode::Runtime;
    let mut json_output = false;
    let mut id_format = IdFormat::Refs;
    let mut request: Option<String> = None;
    let mut pretty = false;

    let mut command_start = 0usize;
    while command_start < args.len() {
        let arg = args[command_start].clone();
        if !arg.starts_with('-') {
            break;
        }
        match arg.as_str() {
            "--socket" => {
                let value = args
                    .get(command_start + 1)
                    .ok_or_else(|| anyhow!("--socket requires a value"))?;
                socket = Some(PathBuf::from(value));
                command_start += 2;
            }
            "--socket-mode" => {
                let value = args
                    .get(command_start + 1)
                    .ok_or_else(|| anyhow!("--socket-mode requires runtime|debug"))?;
                socket_mode = match value.as_str() {
                    "runtime" => SocketMode::Runtime,
                    "debug" => SocketMode::Debug,
                    _ => bail!("--socket-mode must be runtime or debug"),
                };
                command_start += 2;
            }
            "--json" => {
                json_output = true;
                command_start += 1;
            }
            "--id-format" => {
                let value = args
                    .get(command_start + 1)
                    .ok_or_else(|| anyhow!("--id-format requires refs|both|uuids"))?;
                id_format = IdFormat::parse(value)?;
                command_start += 2;
            }
            "--request" => {
                let value = args
                    .get(command_start + 1)
                    .ok_or_else(|| anyhow!("--request requires a JSON value"))?;
                request = Some(value.clone());
                command_start += 2;
            }
            "--pretty" => {
                pretty = true;
                command_start += 1;
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            _ => break,
        }
    }

    let command_args = args.split_off(command_start);

    Ok(GlobalOptions {
        socket,
        socket_mode,
        json_output,
        id_format,
        request,
        pretty,
        command_args,
    })
}

fn print_help() {
    const HELP: &str = r#"limux CLI

Usage: limux [--socket <path>] [--json] [--id-format refs|both|uuids] <command> [args...]
       limux --version
       limux

Running `limux` with no arguments launches the GTK app.

Common commands:
  activate (present the existing window; never launch a new instance)
  identify [--workspace <id|ref>] [--surface <id|ref>]
  list-panels [--workspace <id|ref>]
  list-panes [--workspace <id|ref>]
  list-workspaces
  surface-health [--workspace <id|ref>]
  send [--workspace <id|ref>] [--surface <id|ref>] <text>
  send-key [--workspace <id|ref>] [--surface <id|ref>] <key>
  new-workspace [--cwd <path>] [--command <text>]
  select-workspace --workspace <id|ref>
  close-workspace --workspace <id|ref>
  sidebar-state --workspace <id|ref>
  new-surface [--workspace <id|ref>] [--pane <id|ref>] [--type terminal|browser] [--url <url>]
  close-surface [--workspace <id|ref>] [--surface <id|ref>]
  focus-surface [--workspace <id|ref>] [--surface <id|ref>]
  focus-pane [--workspace <id|ref>] [--pane <id|ref>]
  new-pane [--workspace <id|ref>] [--pane <id|ref>] [--surface <id|ref>] [--direction <left|right|up|down>] [--type <terminal|browser>] [--command <text>] [--url <url>]
      Supports terminal and browser panes on both the live GTK host and standalone server.
  rename-workspace [--workspace <id|ref>] <title>
  set-workspace-color [--workspace <id|ref>] <red|orange|amber|green|teal|blue|purple|pink|none>
  list-groups
  create-group [--color <color>] [--workspace <id|ref>]... [<name>]
  rename-group --group <id|ref|name> <name>
  set-group-color --group <id|ref|name> <color>
  collapse-group --group <id|ref|name>
  expand-group --group <id|ref|name>
  delete-group --group <id|ref|name>   (only an empty group can be deleted)
  set-workspace-group [--workspace <id|ref>] <group id|ref|name|none>
  rename-window [--workspace <id|ref>] <title>
  gui-hooks list
  gui-hooks set <event> [--command <text>] [--enable|--disable]
      [--show-terminal|--no-show-terminal] [--suppress-default|--no-suppress-default]
      <event>: on-workspace-create, after-workspace-create, on-workspace-delete
      or after-workspace-delete. Runs <text> in bash when that action is done in
      the GUI (not over the socket): an on-* hook before Limux does it, an
      after-* hook once it is done.
      --suppress-default (on-* hooks only) leaves the create/delete to the hook,
      so Limux skips its folder prompt as well; the matching after-* hook then
      does not run, since requests made through the CLI or socket never run hooks.
      --disable keeps the hook's settings but stops it from running.
  gui-hooks enable|disable <event>
  gui-hooks clear <event>
  rename-tab [--workspace <id|ref>] [--tab <id|ref>] <title>
  read-screen [--workspace <id|ref>] [--surface <id|ref>] [--scrollback] [--lines <n>]
  capture-pane (alias of read-screen)
  tab-action --action <name> [--workspace <id|ref>] [--tab <id|ref>] [--title <text>] [--url <url>]
  browser [--surface <id|ref>|<surface>] <subcommand> ...
  save-session
      Flush the running window's session.json with attached restorable agent sessions.

Agent integrations:
  notify [--workspace <id|ref>] [--surface <id|ref>] [--subtitle <text>] [--body <text>] [--osc] <title>
  hooks setup [agent] [--osc] | hooks uninstall [agent] | hooks snapshot [agent] [--dry-run] [--save-session] [--all-sockets] | hooks <agent> <event>
  claude-hook | opencode-hook | gemini-hook | agy-hook --event <name> [--subtitle <text>] [--body <text>] [--title <text>]
  agent-team [--agents codex,claude[,opencode,gemini,agy]] [--cwd <path>] [--no-launch] [--dry-run]
      Splits the active workspace into one pane per agent (caller's pane stays
      as the orchestrator on the left, peers stack down the right), launches
      each CLI in its pane, and writes AGENTS.md describing the <agent-msg>
      XML protocol so peers can talk via
      `limux send --surface <peer-surface-id> <envelope>`."#;

    println!("{HELP}\n");
}

fn should_launch_host(opts: &GlobalOptions) -> bool {
    opts.command_args.is_empty()
        && opts.request.is_none()
        && opts.socket.is_none()
        && opts.socket_mode == SocketMode::Runtime
        && !opts.json_output
        && !opts.pretty
        && opts.id_format == IdFormat::Refs
}

fn host_binary_candidates(exe: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(bin_dir) = exe.parent() {
        if let Some(prefix) = bin_dir.parent() {
            candidates.push(prefix.join("libexec/limux/limux-host"));
        }

        let sibling_host = bin_dir.join("limux-host");
        if sibling_host != exe {
            candidates.push(sibling_host);
        }

        let sibling_dev_host = bin_dir.join("limux");
        if sibling_dev_host != exe {
            candidates.push(sibling_dev_host);
        }
    }

    candidates
}

fn resolve_host_binary() -> Result<PathBuf> {
    if let Ok(raw) = env::var("LIMUX_HOST_BIN") {
        let path = PathBuf::from(raw);
        if path.is_file() {
            return Ok(path);
        }
    }

    let exe = env::current_exe().context("failed to resolve current executable")?;
    host_binary_candidates(&exe)
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            anyhow!(
                "could not find limux host binary; expected limux-host next to the installed CLI"
            )
        })
}

fn launch_host() -> Result<()> {
    let host = resolve_host_binary()?;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        let error = Command::new(&host).exec();
        Err(error).with_context(|| format!("failed to launch {}", host.display()))
    }
    #[cfg(not(unix))]
    {
        let status = Command::new(&host)
            .spawn()
            .with_context(|| format!("failed to launch {}", host.display()))?
            .wait()
            .with_context(|| format!("failed to wait for {}", host.display()))?;
        if status.success() {
            Ok(())
        } else {
            bail!("{} exited with {}", host.display(), status)
        }
    }
}

fn get_string(value: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(raw) = value.get(*key) {
            match raw {
                Value::String(s) if !s.is_empty() => return Some(s.clone()),
                Value::Number(n) => return Some(n.to_string()),
                _ => {}
            }
        }
    }
    None
}

fn handle_from_payload(value: &Value, id_key: &str, ref_key: &str) -> String {
    get_string(value, &[ref_key])
        .or_else(|| get_string(value, &[id_key]))
        .unwrap_or_default()
}

fn apply_id_format(value: &mut Value, id_format: IdFormat) {
    match value {
        Value::Object(map) => {
            let keys: Vec<String> = map.keys().cloned().collect();
            for key in &keys {
                if key.ends_with("_id") {
                    let prefix = key.trim_end_matches("_id");
                    let ref_key = format!("{}_ref", prefix);
                    match id_format {
                        IdFormat::Refs => {
                            if map.contains_key(&ref_key) {
                                map.remove(key);
                            }
                        }
                        IdFormat::Uuids => {
                            if map.contains_key(key) {
                                map.remove(&ref_key);
                            }
                        }
                        IdFormat::Both => {}
                    }
                }
            }

            match id_format {
                IdFormat::Refs => {
                    if map.contains_key("ref") {
                        map.remove("id");
                    }
                }
                IdFormat::Uuids => {
                    if map.contains_key("id") {
                        map.remove("ref");
                    }
                }
                IdFormat::Both => {}
            }

            let child_keys: Vec<String> = map.keys().cloned().collect();
            for key in child_keys {
                if let Some(child) = map.get_mut(&key) {
                    apply_id_format(child, id_format);
                }
            }
        }
        Value::Array(list) => {
            for item in list {
                apply_id_format(item, id_format);
            }
        }
        _ => {}
    }
}

fn parse_opt(args: &[String], name: &str) -> Option<String> {
    args.windows(2).find_map(|w| {
        if w[0] == name {
            Some(w[1].clone())
        } else {
            None
        }
    })
}

fn parse_terminal_surface(args: &[String]) -> Result<Option<String>> {
    let Some(surface) = parse_opt(args, "--surface") else {
        if parse_flag(args, "--surface") {
            bail!("--surface requires a non-empty target");
        }
        return Ok(None);
    };
    let normalized = surface.trim();
    let normalized = normalized.strip_prefix("surface:").unwrap_or(normalized);
    if normalized.trim().is_empty() {
        bail!("--surface requires a non-empty target");
    }
    Ok(Some(surface))
}

fn parse_flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn positional_arg(args: &[String], index: usize) -> Option<String> {
    let mut position = 0usize;
    let mut skip = false;
    for arg in args {
        if skip {
            skip = false;
            continue;
        }
        if arg == "--agent" {
            skip = true;
            continue;
        }
        if arg.starts_with('-') {
            continue;
        }
        if position == index {
            return Some(arg.clone());
        }
        position += 1;
    }
    None
}

fn trailing_title(args: &[String]) -> Option<String> {
    let mut filtered: Vec<String> = Vec::new();
    let mut skip = false;
    for arg in args {
        if skip {
            skip = false;
            continue;
        }
        if arg == "--workspace"
            || arg == "--tab"
            || arg == "--surface"
            || arg == "--pane"
            || arg == "--target-pane"
            || arg == "--action"
            || arg == "--title"
            || arg == "--url"
            || arg == "--cwd"
            || arg == "--command"
            || arg == "--direction"
            || arg == "--type"
            || arg == "--lines"
            || arg == "--timeout"
            || arg == "--timeout-ms"
            || arg == "--name"
            || arg == "--out"
            || arg == "--subtitle"
            || arg == "--body"
            || arg == "--message"
            || arg == "--event"
            || arg == "--agents"
            || arg == "--selector"
            || arg == "--text"
            || arg == "--attr"
            || arg == "--property"
            || arg == "--value"
            || arg == "--amount"
            || arg == "--unset"
            || arg == "--group"
            || arg == "--color"
        {
            skip = true;
            continue;
        }
        if arg.starts_with('-') {
            continue;
        }
        filtered.push(arg.clone());
    }
    if filtered.is_empty() {
        None
    } else {
        Some(filtered.join(" "))
    }
}

fn wait_signal_path(name: &str) -> Result<PathBuf> {
    let sanitized: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let dir = limux_control::socket_path::ensure_private_runtime_subdir("wait-for")
        .context("failed to prepare private wait-for directory")?;
    Ok(dir.join(format!("wait-for-{sanitized}.sig")))
}

fn read_json_map(path: &str) -> BTreeMap<String, String> {
    let raw = fs::read_to_string(path).unwrap_or_default();
    serde_json::from_str::<BTreeMap<String, String>>(&raw).unwrap_or_default()
}

fn write_json_map(path: &Path, map: &BTreeMap<String, String>) -> Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    let encoded = serde_json::to_string_pretty(map).context("failed to encode json map")?;
    if let Some(parent) = path.parent() {
        limux_control::socket_path::ensure_private_dir(parent)
            .with_context(|| format!("failed to prepare {}", parent.display()))?;
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = path.with_extension(format!("tmp-{}-{}", std::process::id(), nonce));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .with_context(|| format!("failed to create {}", tmp.display()))?;
    file.write_all(encoded.as_bytes())
        .with_context(|| format!("failed to write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("failed to replace {}", path.display()))?;
    Ok(())
}

fn socket_state_namespace(socket: &Path) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    socket.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn cli_state_dir(socket: &Path) -> PathBuf {
    limux_control::socket_path::default_runtime_dir()
        .join("cli")
        .join(socket_state_namespace(socket))
}

fn ensure_cli_state_dir(socket: &Path) -> Result<PathBuf> {
    let base = limux_control::socket_path::ensure_private_runtime_subdir("cli")
        .context("failed to prepare private CLI state directory")?;
    let dir = base.join(socket_state_namespace(socket));
    limux_control::socket_path::ensure_private_dir(&dir)
        .with_context(|| format!("failed to prepare {}", dir.display()))?;
    Ok(dir)
}

fn cli_state_path(socket: &Path, kind: &str) -> PathBuf {
    cli_state_dir(socket).join(format!("{kind}.json"))
}

fn cli_state_lock_path(socket: &Path, kind: &str) -> PathBuf {
    cli_state_dir(socket).join(format!("{kind}.lock"))
}

struct CliStateLock {
    path: PathBuf,
}

impl Drop for CliStateLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn acquire_cli_state_lock(socket: &Path, kind: &str) -> Result<CliStateLock> {
    use std::os::unix::fs::OpenOptionsExt as _;

    let _dir = ensure_cli_state_dir(socket)?;
    let lock_path = cli_state_lock_path(socket, kind);
    let deadline = Instant::now() + CLI_STATE_LOCK_TIMEOUT;
    loop {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&lock_path)
        {
            Ok(_) => return Ok(CliStateLock { path: lock_path }),
            Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                if Instant::now() >= deadline {
                    bail!("timed out acquiring CLI state lock {}", lock_path.display());
                }
                std::thread::sleep(CLI_STATE_LOCK_RETRY);
            }
            Err(err) => {
                return Err(err).with_context(|| {
                    format!("failed to create CLI state lock {}", lock_path.display())
                });
            }
        }
    }
}

fn with_locked_json_map<T, F>(socket: &Path, kind: &str, update: F) -> Result<T>
where
    F: FnOnce(&mut BTreeMap<String, String>, &Path) -> Result<T>,
{
    let _lock = acquire_cli_state_lock(socket, kind)?;
    let path = cli_state_path(socket, kind);
    let path_str = path.to_string_lossy().to_string();
    let mut map = read_json_map(&path_str);
    update(&mut map, &path)
}

async fn resolve_current_workspace(client: &mut Client) -> Result<String> {
    let current = client.call("workspace.current", json!({})).await?;
    get_string(&current, &["workspace_id", "workspace_ref"])
        .ok_or_else(|| anyhow!("workspace.current returned no workspace handle"))
}

async fn call_in_workspace_scope(
    client: &mut Client,
    workspace: Option<String>,
    method: &str,
    params: Value,
) -> Result<Value> {
    if let Some(target) = workspace {
        let mut map = match params {
            Value::Object(map) => map,
            Value::Null => Map::new(),
            _ => bail!("{method} requires object params for workspace-scoped calls"),
        };
        map.entry("workspace_id".to_string())
            .or_insert(Value::String(target));
        return client.call(method, Value::Object(map)).await;
    }
    client.call(method, params).await
}

async fn browser_call(
    client: &mut Client,
    workspace: Option<&str>,
    surface: Option<&str>,
    method: &str,
    mut params: Map<String, Value>,
) -> Result<Value> {
    if let Some(surface) = surface {
        params.insert("surface_id".to_string(), Value::String(surface.to_string()));
    }
    call_in_workspace_scope(
        client,
        workspace.map(str::to_string),
        method,
        Value::Object(params),
    )
    .await
}

fn window_activation_params(activation_token: Option<&str>, startup_id: Option<&str>) -> Value {
    match activation_token
        .filter(|token| !token.is_empty())
        .or_else(|| startup_id.filter(|token| !token.is_empty()))
    {
        Some(token) => json!({ "activation_token": token }),
        None => json!({}),
    }
}

async fn run_activate(client: &mut Client, args: &[String]) -> Result<Value> {
    if !args.is_empty() {
        bail!("activate takes no arguments; use --socket <path> before activate to target an instance");
    }
    let activation_token = env::var("XDG_ACTIVATION_TOKEN").ok();
    let startup_id = env::var("DESKTOP_STARTUP_ID").ok();
    client
        .call(
            "window.activate",
            window_activation_params(activation_token.as_deref(), startup_id.as_deref()),
        )
        .await
}

async fn run_save_session(client: &mut Client, args: &[String]) -> Result<Value> {
    if !args.is_empty() {
        bail!("save-session takes no arguments; use --socket <path> before save-session to target an instance");
    }
    client.call("session.save", json!({})).await
}

async fn run_identify(client: &mut Client, args: &[String]) -> Result<Value> {
    let workspace = parse_opt(args, "--workspace");
    let surface = parse_opt(args, "--surface");
    let no_caller = parse_flag(args, "--no-caller");

    let mut params = Map::new();
    if workspace.is_some() || surface.is_some() {
        let mut caller = Map::new();
        if let Some(workspace) = workspace {
            caller.insert("workspace_id".to_string(), Value::String(workspace));
        }
        if let Some(surface) = surface {
            caller.insert("surface_id".to_string(), Value::String(surface));
        }
        params.insert("caller".to_string(), Value::Object(caller));
    }

    let mut payload = client
        .call("system.identify", Value::Object(params))
        .await?;
    if no_caller {
        if let Some(map) = payload.as_object_mut() {
            map.remove("caller");
        }
    }
    Ok(payload)
}

async fn run_list(client: &mut Client, command: &str, args: &[String]) -> Result<Value> {
    let workspace = parse_opt(args, "--workspace")
        .or_else(|| env::var("LIMUX_WORKSPACE_ID").ok())
        .filter(|value| !value.trim().is_empty());
    let params = if let Some(workspace) = workspace.as_ref() {
        json!({ "workspace_id": workspace })
    } else {
        json!({})
    };
    let method = match command {
        "list-panels" => "surface.list",
        "list-panes" => "pane.list",
        "list-workspaces" => "workspace.list",
        "surface-health" => "surface.health",
        _ => bail!("unsupported list command"),
    };
    let mut payload = client.call(method, params).await?;
    if let Some(workspace) = workspace.as_ref() {
        if let Some(map) = payload.as_object_mut() {
            if workspace.contains(':') {
                map.entry("workspace_ref".to_string())
                    .or_insert_with(|| Value::String(workspace.clone()));
            } else {
                map.entry("workspace_id".to_string())
                    .or_insert_with(|| Value::String(workspace.clone()));
            }
        }
    }
    Ok(payload)
}

fn render_list_text(command: &str, payload: &Value) -> String {
    match command {
        "list-panels" => {
            let rows = payload
                .get("surfaces")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if rows.is_empty() {
                return "No surfaces".to_string();
            }
            rows.iter()
                .map(|row| {
                    let handle = handle_from_payload(row, "surface_id", "surface_ref");
                    let title = get_string(row, &["title"]).unwrap_or_default();
                    format!("{} {}", handle, title)
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        "list-panes" => {
            let rows = payload
                .get("panes")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if rows.is_empty() {
                return "No panes".to_string();
            }
            rows.iter()
                .map(|row| {
                    let handle = handle_from_payload(row, "pane_id", "pane_ref");
                    let count = row
                        .get("surface_count")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    format!("{} surfaces={}", handle, count)
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        "list-workspaces" => {
            let rows = payload
                .get("workspaces")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if rows.is_empty() {
                return "No workspaces".to_string();
            }
            rows.iter()
                .map(|row| {
                    let handle = handle_from_payload(row, "workspace_id", "workspace_ref");
                    let title = get_string(row, &["title", "name"]).unwrap_or_default();
                    let selected = row
                        .get("selected")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    if selected {
                        format!("* {} {}", handle, title)
                    } else {
                        format!("  {} {}", handle, title)
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        "surface-health" => {
            let rows = payload
                .get("surfaces")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if rows.is_empty() {
                return "No surfaces".to_string();
            }
            rows.iter()
                .map(|row| {
                    let handle = handle_from_payload(row, "surface_id", "surface_ref");
                    let healthy = row.get("healthy").and_then(Value::as_bool).unwrap_or(true);
                    format!("{} healthy={}", handle, healthy)
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        _ => "".to_string(),
    }
}

async fn run_send(client: &mut Client, args: &[String]) -> Result<Value> {
    let surface = parse_terminal_surface(args)?;
    let workspace = request_workspace(parse_opt(args, "--workspace"), surface.as_deref(), env_opt);

    let text = trailing_title(args).ok_or_else(|| anyhow!("send requires text"))?;

    let mut params = Map::new();
    params.insert("text".to_string(), Value::String(text));
    if let Some(surface) = surface {
        params.insert("surface_id".to_string(), Value::String(surface));
    }

    call_in_workspace_scope(
        client,
        workspace,
        "surface.send_text",
        Value::Object(params),
    )
    .await
}

async fn run_send_key(client: &mut Client, args: &[String]) -> Result<Value> {
    let surface = parse_terminal_surface(args)?;
    let workspace = request_workspace(parse_opt(args, "--workspace"), surface.as_deref(), env_opt);
    let key = trailing_title(args).ok_or_else(|| anyhow!("send-key requires key"))?;

    let mut params = Map::new();
    params.insert("key".to_string(), Value::String(key));
    if let Some(surface) = surface {
        params.insert("surface_id".to_string(), Value::String(surface));
    }

    call_in_workspace_scope(client, workspace, "surface.send_key", Value::Object(params)).await
}

/// `limux notify` — post a notification into the sidebar + toast overlay.
///
/// Usage:
///   limux notify [--workspace <id|ref>] [--surface <id|ref>] [--subtitle <text>] [--body <text>] [--osc] <title>
///   limux notify --title "..." --subtitle "..." --body "..."
///
/// Mirrors the `cmux notify` shape (title / subtitle / body). Title is
/// required; subtitle and body are optional. Falls back to the current
/// workspace via LIMUX_WORKSPACE_ID when --workspace isn't given.
fn notification_surface_target(
    args: &[String],
    mut env_value: impl FnMut(&str) -> Option<String>,
) -> Option<String> {
    parse_opt(args, "--surface")
        .or_else(|| env_value("LIMUX_TAB_ID"))
        .or_else(|| env_value("LIMUX_SURFACE_ID"))
        .filter(|value| !value.is_empty())
}

/// The workspace and surface a notification names, as `request_workspace` scopes them.
fn notification_scope(
    args: &[String],
    env_value: impl Fn(&str) -> Option<String>,
) -> (Option<String>, Option<String>) {
    let surface = notification_surface_target(args, &env_value);
    let workspace = request_workspace(
        parse_opt(args, "--workspace"),
        surface.as_deref(),
        env_value,
    );
    (workspace, surface)
}

/// Keep local socket delivery unchanged. Remote fallback is restricted to an
/// unavailable socket; permission/protocol errors are not bypassed via OSC.
async fn deliver_notification(
    client: &mut Client,
    workspace: Option<String>,
    params: Map<String, Value>,
    force: bool,
) -> Result<Value> {
    let force = force || terminal_notifications::forced();
    let title = params
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let subtitle = params
        .get("subtitle")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let body = params
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let body = if subtitle.is_empty() {
        body.to_owned()
    } else {
        format!("{subtitle} — {body}")
    };
    if !force {
        let result = call_in_workspace_scope(
            client,
            workspace,
            "notification.create",
            Value::Object(params),
        )
        .await;
        let unavailable = result
            .as_ref()
            .err()
            .is_some_and(terminal_notifications::socket_unavailable);
        if !terminal_notifications::remote() || !unavailable {
            return result;
        }
    }
    tokio::task::spawn_blocking(move || terminal_notifications::emit(&title, &body)).await??;
    Ok(json!({"delivered": "osc"}))
}

async fn run_notify(client: &mut Client, args: &[String]) -> Result<Value> {
    let (workspace, surface) = notification_scope(args, env_opt);

    // Title can be provided either via --title or as the trailing positional
    // (matching `limux send`'s ergonomics).
    let title = parse_opt(args, "--title")
        .or_else(|| trailing_title(args))
        .ok_or_else(|| anyhow!("notify requires a title"))?;

    let subtitle = parse_opt(args, "--subtitle").unwrap_or_default();
    let body = parse_opt(args, "--body")
        .or_else(|| parse_opt(args, "--message"))
        .unwrap_or_default();

    let mut params = Map::new();
    params.insert("title".to_string(), Value::String(title));
    if !subtitle.is_empty() {
        params.insert("subtitle".to_string(), Value::String(subtitle));
    }
    if !body.is_empty() {
        params.insert("body".to_string(), Value::String(body));
    }
    if let Some(surface) = surface {
        params.insert("surface_id".to_string(), Value::String(surface));
    }

    deliver_notification(client, workspace, params, parse_flag(args, "--osc")).await
}

// ---------------------------------------------------------------------------
// Agent hooks (claude-hook / opencode-hook / gemini-hook)
// ---------------------------------------------------------------------------
//
// These subcommands read a JSON hook event from stdin and translate it into
// a `notify` (and, eventually, log / progress) call so the GUI reflects
// agent activity in real time. Designed for direct wiring into Claude Code,
// OpenCode, and Gemini CLI's hook settings.
//
// Claude Code stdin schema (what we rely on):
//   {
//     "session_id": "...",
//     "transcript_path": "...",
//     "cwd": "...",
//     "hook_event_name": "Notification" | "Stop" | "SessionStart" | ...,
//     "message": "agent is waiting for input",     // Notification only
//     "tool_name": "...", "tool_input": {...},     // PreToolUse/PostToolUse
//     "tool_response": {...},                       // PostToolUse
//     "prompt": "..."                               // UserPromptSubmit
//   }
//
// OpenCode and Gemini use slightly different names; we fall back gracefully
// when fields are missing.

/// Pull a string field from the hook JSON, trying multiple keys.
fn hook_str<'a>(payload: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|k| payload.get(*k).and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn parse_hook_event(args: &[String], payload: &Value) -> String {
    parse_opt(args, "--event")
        .or_else(|| trailing_title(args))
        .or_else(|| {
            hook_str(payload, &["hook_event_name", "hookEventName", "event"]).map(str::to_owned)
        })
        .unwrap_or_else(|| "event".to_string())
}

fn agy_stop_suppresses_notification(payload: &Value) -> bool {
    payload.get("fullyIdle").and_then(Value::as_bool) == Some(false)
        || hook_str(payload, &["terminationReason"]) == Some("USER_CANCELED")
}

fn agy_stop_notification(payload: &Value) -> Option<(String, String)> {
    if agy_stop_suppresses_notification(payload) {
        return None;
    }
    if let Some(error) = hook_str(payload, &["error"]) {
        return Some(("Antigravity failed".to_string(), error.to_owned()));
    }
    let reason = hook_str(payload, &["terminationReason"]);
    let body = match reason {
        Some("ERROR") => "stopped with an error",
        Some("MAX_INVOCATIONS") => "hit maximum invocations",
        Some("MAX_FORCED_INVOCATIONS") => "hit maximum forced invocations",
        Some("MAX_TOKEN_BUDGET_EXCEEDED") => "exceeded token budget",
        Some("TERMINAL_CUSTOM_HOOK") => "stopped by custom hook",
        _ => hook_str(payload, &["message", "reason"]).unwrap_or("task complete"),
    };
    let title = if reason == Some("ERROR") {
        "Antigravity failed"
    } else {
        "Antigravity finished"
    };
    Some((title.to_string(), body.to_owned()))
}

fn agent_hook_notification(
    agent: agent_hooks::AgentKind,
    event: &str,
    payload: &Value,
) -> Option<(String, String)> {
    let agent_label = agent.label();
    match event {
        "Notification" | "notification" => Some((
            format!("{agent_label} needs you"),
            hook_str(payload, &["message", "notification"])
                .unwrap_or("waiting for input")
                .to_owned(),
        )),
        "Stop" | "stop" | "finished" => {
            if agent == agent_hooks::AgentKind::Agy {
                agy_stop_notification(payload)
            } else {
                Some((
                    format!("{agent_label} finished"),
                    hook_str(payload, &["message", "reason"])
                        .unwrap_or("task complete")
                        .to_owned(),
                ))
            }
        }
        "SessionStart" | "session-start" | "SessionEnd" | "session-end" | "UserPromptSubmit"
        | "prompt-submit" | "PreInvocation" | "pre-invocation" | "PostInvocation"
        | "post-invocation" | "PreToolUse" | "PostToolUse" | "SubagentStop" | "subagent-stop"
        | "Cleanup" | "cleanup" | "restore-exit" => None,
        other => Some((
            format!("{agent_label}: {other}"),
            hook_str(payload, &["message", "summary"])
                .unwrap_or("")
                .to_owned(),
        )),
    }
}

/// Run an agent hook: read JSON from stdin, synthesize a notification.
///
/// Args:
///   [event_name] — optional positional, e.g. "Notification", "Stop".
///                  If omitted, we read `hook_event_name` from the JSON.
async fn run_agent_hook(
    client: &mut Client,
    agent: agent_hooks::AgentKind,
    args: &[String],
) -> Result<Value> {
    use std::io::Read;

    // Read stdin (hook JSON). If stdin is empty or not JSON, treat as
    // minimal event so we still post *something*.
    let mut raw = String::new();
    let _ = std::io::stdin().read_to_string(&mut raw);
    let raw = raw.trim();
    let payload: Value = if raw.is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_str(raw).unwrap_or_else(|_| json!({ "raw": raw }))
    };

    let mut event = parse_hook_event(args, &payload);
    // Older installers routed Claude Notification through positional stop.
    // Preserve that payload's identity while honoring an explicit --event.
    if agent == agent_hooks::AgentKind::Claude
        && event == "stop"
        && parse_opt(args, "--event").is_none()
        && hook_str(&payload, &["hook_event_name"]) == Some("Notification")
    {
        event = "Notification".to_string();
    }

    // Build a human-friendly title + body depending on event + agent.
    persist_agent_hook_session(agent, args, &payload, &event)?;
    let notification_title_body = agent_hook_notification(agent, &event, &payload);

    if let Some((title, body)) = notification_title_body {
        let subtitle = hook_str(
            &payload,
            &["session_id", "conversationId", "sessionId", "sessionID"],
        )
        .map(|s| {
            // Show only a short prefix of the session id to keep sidebar tidy.
            s.chars().take(8).collect::<String>()
        })
        .unwrap_or_default();

        let (workspace, surface) = notification_scope(args, env_opt);

        let mut params = Map::new();
        params.insert("title".to_string(), Value::String(title));
        if !subtitle.is_empty() {
            params.insert("subtitle".to_string(), Value::String(subtitle));
        }
        if !body.is_empty() {
            params.insert("body".to_string(), Value::String(body));
        }
        if let Some(surface) = surface {
            params.insert("surface_id".to_string(), Value::String(surface));
        }

        if let Err(error) = deliver_notification(client, workspace, params, false).await {
            if terminal_notifications::forced() || terminal_notifications::remote() {
                eprintln!("limux: notification not delivered: {error:#}");
            }
        }
    }

    Ok(agent_hook_output(agent, &event, &payload))
}

fn agent_hook_output(agent: agent_hooks::AgentKind, event: &str, payload: &Value) -> Value {
    let canonical_event = canonical_hook_event_name(event);
    if agent == agent_hooks::AgentKind::Agy {
        return match canonical_event {
            Some("Stop") => json!({ "decision": "allow" }),
            Some("SessionStart" | "PreInvocation") => match hook_additional_context(payload) {
                Some(context) => json!({
                    "injectSteps": [{
                        "systemMessage": {
                            "systemMessage": context
                        }
                    }]
                }),
                None => json!({}),
            },
            _ => json!({}),
        };
    }

    let mut output = Map::new();
    output.insert("continue".to_string(), Value::Bool(true));
    output.insert("suppressOutput".to_string(), Value::Bool(false));

    if matches!(canonical_event, Some("SessionStart" | "UserPromptSubmit")) {
        let mut specific = Map::new();
        specific.insert(
            "hookEventName".to_string(),
            Value::String(
                canonical_event
                    .expect("matched canonical event")
                    .to_string(),
            ),
        );
        if let Some(context) = hook_additional_context(payload) {
            specific.insert("additionalContext".to_string(), Value::String(context));
        }
        output.insert("hookSpecificOutput".to_string(), Value::Object(specific));
    }

    Value::Object(output)
}

fn canonical_hook_event_name(event: &str) -> Option<&'static str> {
    match event {
        "SessionStart" | "session-start" => Some("SessionStart"),
        "PreInvocation" | "pre-invocation" => Some("PreInvocation"),
        "UserPromptSubmit" | "prompt-submit" => Some("UserPromptSubmit"),
        "Stop" | "stop" | "Notification" => Some("Stop"),
        "SessionEnd" | "session-end" => None,
        "Cleanup" | "cleanup" | "restore-exit" => None,
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentHookPersistenceAction {
    Upsert,
    Preserve,
    Remove,
}

fn agent_hook_persistence_action(event: &str) -> AgentHookPersistenceAction {
    match event {
        "Cleanup" | "cleanup" | "restore-exit" => AgentHookPersistenceAction::Remove,
        "SessionEnd" | "session-end" => AgentHookPersistenceAction::Preserve,
        _ => AgentHookPersistenceAction::Upsert,
    }
}

fn hook_additional_context(payload: &Value) -> Option<String> {
    hook_str(payload, &["additional_context", "additionalContext"])
        .map(str::to_owned)
        .filter(|value| !value.trim().is_empty())
}

fn hook_cwd(payload: &Value) -> Option<String> {
    hook_str(payload, &["cwd", "working_directory", "directory"])
        .map(str::to_string)
        .or_else(|| {
            payload
                .get("workspacePaths")
                .and_then(Value::as_array)
                .and_then(|paths| {
                    paths
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::trim)
                        .find(|path| !path.is_empty())
                        .map(str::to_string)
                })
        })
}

fn persist_agent_hook_session(
    agent: agent_hooks::AgentKind,
    args: &[String],
    payload: &Value,
    event: &str,
) -> Result<()> {
    let Some(session_id) = hook_session_id(agent, payload) else {
        write_agent_hook_debug(agent, event, "skip_missing_session_id", || {
            json!({
                "payload_keys": payload_keys(payload),
                "has_claude_code_session_env": limux_env_value("CLAUDE_CODE_SESSION_ID").is_some(),
                "has_claude_session_env": limux_env_value("CLAUDE_SESSION_ID").is_some(),
            })
        });
        return Ok(());
    };

    let store = agent_hooks::AgentHookSessionStore::new(agent);
    match agent_hook_persistence_action(event) {
        AgentHookPersistenceAction::Remove => {
            let (workspace_id, surface_id) =
                agent_hook_cleanup_scope(args, payload, limux_env_value);
            let result = store.remove(&session_id, workspace_id.as_deref(), surface_id.as_deref());
            if result.is_ok() {
                write_agent_hook_debug(agent, event, "removed", || {
                    json!({
                        "session_id": session_id,
                        "payload_keys": payload_keys(payload),
                    })
                });
            }
            return result;
        }
        AgentHookPersistenceAction::Preserve => {
            write_agent_hook_debug(agent, event, "preserved", || {
                json!({
                    "session_id": session_id,
                    "payload_keys": payload_keys(payload),
                })
            });
            return Ok(());
        }
        AgentHookPersistenceAction::Upsert => {}
    }

    let workspace_arg = parse_opt(args, "--workspace");
    let surface_arg = parse_opt(args, "--surface");
    let workspace_id = workspace_arg
        .clone()
        .or_else(|| limux_env_value("LIMUX_WORKSPACE_ID"))
        .filter(|value| !value.trim().is_empty());
    let surface_id = if workspace_id.is_some() {
        surface_arg
            .clone()
            .or_else(|| limux_env_value("LIMUX_SURFACE_ID"))
            .filter(|value| !value.trim().is_empty())
    } else {
        None
    };
    let (Some(workspace_id), Some(surface_id)) = (workspace_id, surface_id) else {
        write_agent_hook_debug(agent, event, "skip_missing_limux_target", || {
            json!({
                "session_id": session_id,
                "has_workspace_arg": workspace_arg.is_some(),
                "has_surface_arg": surface_arg.is_some(),
                "has_workspace_env": limux_env_value("LIMUX_WORKSPACE_ID").is_some(),
                "has_surface_env": limux_env_value("LIMUX_SURFACE_ID").is_some(),
                "payload_keys": payload_keys(payload),
            })
        });
        return Ok(());
    };

    let ancestor = agent_ancestor_process(agent);
    let ancestor_pid = ancestor.as_ref().map(|(pid, _)| *pid);
    let proc_args = ancestor.and_then(|(_, args)| (!args.is_empty()).then_some(args));
    let proc_cwd = ancestor_pid.and_then(proc_cwd_path);
    let proc_env = ancestor_pid.and_then(proc_allowed_environment);

    let result = store.update(&session_id, |existing| {
        let cwd = hook_cwd(payload)
            .or_else(|| proc_cwd.clone())
            .or_else(|| existing.and_then(|record| record.cwd.clone()));
        let pid = hook_str(payload, &["pid"])
            .and_then(|value| value.parse::<u32>().ok())
            .or(ancestor_pid)
            .or_else(|| existing.and_then(|record| record.pid));
        let launch_command = agent_hooks::launch_record_from_context(
            agent,
            cwd.as_deref(),
            proc_args.as_deref(),
            proc_cwd.as_deref(),
            proc_env.as_ref(),
        )
        .or_else(|| existing.and_then(|record| record.launch_command.clone()));

        agent_hooks::AgentHookSessionRecord {
            session_id: session_id.clone(),
            workspace_id,
            surface_id,
            cwd,
            pid,
            launch_command,
            updated_at: agent_hooks::now_seconds(),
        }
    });
    if result.is_ok() {
        write_agent_hook_debug(agent, event, "upserted", || {
            json!({
                "payload_keys": payload_keys(payload),
            })
        });
    }
    result
}

fn agent_hook_cleanup_scope(
    args: &[String],
    payload: &Value,
    env_value: impl Fn(&str) -> Option<String>,
) -> (Option<String>, Option<String>) {
    let workspace = parse_opt(args, "--workspace")
        .or_else(|| env_value("LIMUX_WORKSPACE_ID"))
        .or_else(|| hook_str(payload, &["workspace_id"]).map(str::to_string));
    let surface = parse_opt(args, "--surface")
        .or_else(|| env_value("LIMUX_SURFACE_ID"))
        .or_else(|| env_value("LIMUX_TAB_ID"))
        .or_else(|| hook_str(payload, &["surface_id", "tab_id"]).map(str::to_string));
    (workspace, surface)
}

fn hook_session_id(agent: agent_hooks::AgentKind, payload: &Value) -> Option<String> {
    hook_session_id_with_env(agent, payload, limux_env_value)
}

fn hook_session_id_with_env(
    agent: agent_hooks::AgentKind,
    payload: &Value,
    env_value: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    hook_str(
        payload,
        &["session_id", "sessionId", "sessionID", "conversationId"],
    )
    .map(str::to_string)
    .or_else(|| {
        (agent == agent_hooks::AgentKind::Claude)
            .then(|| env_value("CLAUDE_CODE_SESSION_ID").or_else(|| env_value("CLAUDE_SESSION_ID")))
            .flatten()
    })
    .or_else(|| hook_session_id_from_transcript(payload))
    .filter(|value| !value.trim().is_empty())
}

fn hook_session_id_from_transcript(payload: &Value) -> Option<String> {
    let transcript = hook_str(
        payload,
        &["transcript_path", "transcriptPath", "transcript"],
    )?;
    let path = Path::new(transcript);
    let stem = path.file_stem().and_then(|stem| stem.to_str())?;
    if matches!(stem, "transcript" | "transcript_full") {
        let components: Vec<&str> = path
            .components()
            .filter_map(|component| component.as_os_str().to_str())
            .collect();
        if let Some(idx) = components.iter().rposition(|part| *part == "brain") {
            return components
                .get(idx + 1)
                .map(|value| (*value).to_string())
                .filter(|value| !value.trim().is_empty() && !value.starts_with('.'));
        }
        return None;
    }
    Some(stem.to_string()).filter(|value| !value.trim().is_empty())
}

fn payload_keys(payload: &Value) -> Vec<String> {
    payload
        .as_object()
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default()
}

fn agent_hook_debug_enabled() -> bool {
    env::var_os("LIMUX_HOOK_DEBUG")
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

fn write_agent_hook_debug(
    agent: agent_hooks::AgentKind,
    event: &str,
    outcome: &str,
    details: impl FnOnce() -> Value,
) {
    if !agent_hook_debug_enabled() {
        return;
    }
    let details = details();
    let Some(dir) = agent_hook_debug_dir() else {
        return;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        if fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .is_err()
        {
            return;
        }
    }
    #[cfg(not(unix))]
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join("agent-hook-debug.jsonl");
    let line = json!({
        "time": agent_hooks::now_seconds(),
        "agent": agent.store_name(),
        "event": event,
        "outcome": outcome,
        "details": details,
    });
    if let Ok(mut encoded) = serde_json::to_vec(&line) {
        encoded.push(b'\n');
        let _ = append_debug_line(&path, &encoded);
    }
}

fn agent_hook_debug_dir() -> Option<PathBuf> {
    if let Some(dir) = env::var_os("LIMUX_AGENT_HOOK_STATE_DIR").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    dirs::state_dir()
        .map(|dir| dir.join("limux"))
        .or_else(|| dirs::home_dir().map(|home| home.join(".local/state/limux")))
}

fn append_debug_line(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut opts = fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("failed to append {}", path.display()))
}

fn limux_env_value(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| ancestor_env_value(name))
}

#[cfg(target_os = "linux")]
fn agent_ancestor_process(agent: agent_hooks::AgentKind) -> Option<(u32, Vec<String>)> {
    let mut pid = std::process::id();
    for _ in 0..8 {
        let parent = proc_parent_pid(pid)?;
        if parent <= 1 || parent == pid {
            return None;
        }
        let args = proc_cmdline_args(parent).unwrap_or_default();
        if limux_control::agent_sessions::proc_matches_agent(&args, None, agent) {
            return Some((parent, args));
        }
        let comm = fs::read_to_string(format!("/proc/{parent}/comm")).ok();
        if limux_control::agent_sessions::proc_matches_agent(&args, comm.as_deref(), agent) {
            return Some((parent, args));
        }
        pid = parent;
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn agent_ancestor_process(_agent: agent_hooks::AgentKind) -> Option<(u32, Vec<String>)> {
    None
}

#[cfg(target_os = "linux")]
fn proc_cmdline_args(pid: u32) -> Option<Vec<String>> {
    let raw = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let parts = raw
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .filter_map(|part| std::str::from_utf8(part).ok().map(str::to_string))
        .collect::<Vec<_>>();
    (!parts.is_empty()).then_some(parts)
}

#[cfg(not(target_os = "linux"))]
fn proc_cmdline_args(_pid: u32) -> Option<Vec<String>> {
    None
}

#[cfg(target_os = "linux")]
fn proc_cwd_path(pid: u32) -> Option<String> {
    fs::read_link(format!("/proc/{pid}/cwd"))
        .ok()
        .and_then(|path| path.to_str().map(str::to_string))
        .filter(|value| !value.trim().is_empty())
}

#[cfg(not(target_os = "linux"))]
fn proc_cwd_path(_pid: u32) -> Option<String> {
    None
}

#[cfg(target_os = "linux")]
fn proc_allowed_environment(pid: u32) -> Option<BTreeMap<String, String>> {
    let environ = fs::read(format!("/proc/{pid}/environ")).ok()?;
    Some(allowed_environment_from_environ(&environ))
}

#[cfg(not(target_os = "linux"))]
fn proc_allowed_environment(_pid: u32) -> Option<BTreeMap<String, String>> {
    None
}

fn allowed_environment_from_environ(environ: &[u8]) -> BTreeMap<String, String> {
    let pairs = environ
        .split(|byte| *byte == 0)
        .filter_map(|part| std::str::from_utf8(part).ok())
        .filter_map(|entry| entry.split_once('='));
    agent_hooks::filter_allowed_environment(pairs)
}

#[cfg(target_os = "linux")]
fn ancestor_env_value(name: &str) -> Option<String> {
    let mut pid = std::process::id();
    for _ in 0..8 {
        let parent = proc_parent_pid(pid)?;
        if parent <= 1 || parent == pid {
            return None;
        }
        if let Some(value) = proc_env_value(parent, name) {
            return Some(value);
        }
        pid = parent;
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn ancestor_env_value(_name: &str) -> Option<String> {
    None
}

#[cfg(target_os = "linux")]
fn proc_parent_pid(pid: u32) -> Option<u32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_proc_stat_parent_pid(&stat)
}

#[cfg(target_os = "linux")]
fn parse_proc_stat_parent_pid(stat: &str) -> Option<u32> {
    let close = stat.rfind(')')?;
    let mut fields = stat.get(close + 2..)?.split_whitespace();
    fields.next()?;
    fields.next()?.parse().ok()
}

#[cfg(target_os = "linux")]
fn proc_env_value(pid: u32, name: &str) -> Option<String> {
    let environ = fs::read(format!("/proc/{pid}/environ")).ok()?;
    env_value_from_environ(&environ, name)
}

fn env_value_from_environ(environ: &[u8], name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    environ
        .split(|byte| *byte == 0)
        .filter_map(|part| std::str::from_utf8(part).ok())
        .find_map(|entry| entry.strip_prefix(&prefix).map(str::to_string))
        .filter(|value| !value.trim().is_empty())
}

async fn run_hooks_command(
    client: &mut Client,
    args: &[String],
    json_output: bool,
) -> Result<CommandOutput> {
    let Some(first) = args.first().map(String::as_str) else {
        bail!(
            "Usage: limux hooks setup [agent]|uninstall [agent]|snapshot [agent] [--dry-run] [--save-session] [--all-sockets]|<agent> install|uninstall|<event>"
        );
    };

    match first {
        "setup" | "install" => {
            let target = parse_opt(args, "--agent").or_else(|| positional_arg(args, 1));
            let installed = install_hook_targets(target.as_deref(), parse_flag(args, "--osc"))?;
            return hooks_summary_output("installed", installed, json_output);
        }
        "uninstall" => {
            let target = parse_opt(args, "--agent").or_else(|| positional_arg(args, 1));
            let changed = uninstall_hook_targets(target.as_deref())?;
            return hooks_summary_output("uninstalled", changed, json_output);
        }
        "snapshot" => {
            return run_hooks_snapshot(client, &args[1..], json_output).await;
        }
        _ => {}
    }

    let agent = agent_hooks::AgentKind::from_hook_name(first)
        .ok_or_else(|| anyhow!("unknown hooks target: {first}"))?;
    let rest = &args[1..];
    match rest.first().map(String::as_str) {
        Some("install") => {
            install_hook_target(agent, parse_flag(rest, "--osc"))?;
            hooks_summary_output(
                "installed",
                vec![agent.store_name().to_string()],
                json_output,
            )
        }
        Some("uninstall") => {
            uninstall_hook_target(agent)?;
            hooks_summary_output(
                "uninstalled",
                vec![agent.store_name().to_string()],
                json_output,
            )
        }
        _ => {
            let payload = run_agent_hook(client, agent, rest).await?;
            if json_output {
                Ok(CommandOutput::Json(payload))
            } else {
                Ok(CommandOutput::Text("OK".to_string()))
            }
        }
    }
}

fn hooks_summary_output(
    action: &str,
    agents: Vec<String>,
    json_output: bool,
) -> Result<CommandOutput> {
    if json_output {
        Ok(CommandOutput::Json(json!({
            "action": action,
            "agents": agents,
        })))
    } else {
        Ok(CommandOutput::Text(format!(
            "OK {action}: {}",
            if agents.is_empty() {
                "none".to_string()
            } else {
                agents.join(", ")
            }
        )))
    }
}

fn parse_hooks_snapshot_args(
    args: &[String],
    target_socket: PathBuf,
) -> Result<(session_snapshot::SnapshotOptions, bool)> {
    let mut agent_name: Option<String> = None;
    let mut workspace_id: Option<String> = None;
    let mut surface_id: Option<String> = None;
    let mut all_sockets = false;
    let mut dry_run = false;
    let mut save_session = false;

    let mut set_agent = |value: String| -> Result<()> {
        if agent_name.is_some() {
            bail!("duplicate hooks snapshot agent argument: {value}");
        }
        agent_name = Some(value);
        Ok(())
    };

    let mut idx = 0;
    while idx < args.len() {
        let arg = &args[idx];
        match arg.as_str() {
            "--dry-run" => {
                dry_run = true;
                idx += 1;
            }
            "--save-session" => {
                save_session = true;
                idx += 1;
            }
            "--all-sockets" => {
                all_sockets = true;
                idx += 1;
            }
            "--agent" | "--workspace" | "--surface" => {
                let value = args
                    .get(idx + 1)
                    .filter(|next| !next.starts_with('-') && !next.trim().is_empty())
                    .ok_or_else(|| anyhow!("{arg} requires a value"))?;
                match arg.as_str() {
                    "--agent" => set_agent(value.clone())?,
                    "--workspace" => workspace_id = Some(value.clone()),
                    "--surface" => surface_id = Some(value.clone()),
                    _ => unreachable!(),
                }
                idx += 2;
            }
            other if other.starts_with("--agent=") => {
                let value = other.trim_start_matches("--agent=").trim();
                if value.is_empty() {
                    bail!("--agent requires a value");
                }
                set_agent(value.to_string())?;
                idx += 1;
            }
            other if other.starts_with("--workspace=") => {
                let value = other.trim_start_matches("--workspace=").trim();
                if value.is_empty() {
                    bail!("--workspace requires a value");
                }
                workspace_id = Some(value.to_string());
                idx += 1;
            }
            other if other.starts_with("--surface=") => {
                let value = other.trim_start_matches("--surface=").trim();
                if value.is_empty() {
                    bail!("--surface requires a value");
                }
                surface_id = Some(value.to_string());
                idx += 1;
            }
            other if other.starts_with('-') => {
                bail!("unknown hooks snapshot option: {other}");
            }
            positional => {
                set_agent(positional.to_string())?;
                idx += 1;
            }
        }
    }

    let agent = agent_name
        .as_deref()
        .map(|name| {
            agent_hooks::AgentKind::from_hook_name(name)
                .ok_or_else(|| anyhow!("unknown hooks target: {name}"))
        })
        .transpose()?;

    Ok((
        session_snapshot::SnapshotOptions {
            agent,
            workspace_id,
            surface_id,
            target_socket: Some(target_socket),
            all_sockets,
            dry_run,
        },
        save_session,
    ))
}

async fn run_hooks_snapshot(
    client: &mut Client,
    args: &[String],
    json_output: bool,
) -> Result<CommandOutput> {
    let (options, save_session) = parse_hooks_snapshot_args(args, client.socket.clone())?;
    let dry_run = options.dry_run;

    let report = session_snapshot::snapshot_running_agents(&options)?;
    let session_save = if save_session && !dry_run {
        Some(client.call("session.save", json!({})).await?)
    } else {
        None
    };

    if json_output {
        let mut payload = serde_json::to_value(&report)?;
        if let (Some(map), Some(saved)) = (payload.as_object_mut(), session_save) {
            map.insert("session_save".to_string(), saved);
        }
        return Ok(CommandOutput::Json(payload));
    }

    let mut lines = Vec::new();
    let verb = if report.dry_run {
        "would capture"
    } else {
        "captured"
    };
    lines.push(format!(
        "OK {verb} {} running agent session(s) ({} skipped)",
        report.captured.len(),
        report.skipped.len()
    ));
    for item in &report.captured {
        let cwd = item.cwd.as_deref().unwrap_or("-");
        lines.push(format!(
            "  {} {} pid={} surface={} cwd={}",
            item.agent, item.session_id, item.pid, item.surface_id, cwd
        ));
    }
    for item in &report.skipped {
        let cwd = item.cwd.as_deref().unwrap_or("-");
        lines.push(format!(
            "  skipped {} pid={} surface={} cwd={} ({})",
            item.agent, item.pid, item.surface_id, cwd, item.reason
        ));
    }
    if let Some(saved) = session_save {
        if let Some(path) = saved.get("path").and_then(Value::as_str) {
            let workspaces = saved.get("workspaces").and_then(Value::as_u64).unwrap_or(0);
            let tabs = saved.get("tabs").and_then(Value::as_u64).unwrap_or(0);
            let agents = saved
                .get("restorable_agents")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            lines.push(format!(
                "OK saved session to {path} ({workspaces} workspaces, {tabs} tabs, {agents} restorable agents)"
            ));
        } else {
            lines.push("OK saved session".to_string());
        }
    }
    Ok(CommandOutput::Text(lines.join("\n")))
}

fn install_hook_targets(target: Option<&str>, osc: bool) -> Result<Vec<String>> {
    let agents = target
        .map(|name| {
            agent_hooks::AgentKind::from_hook_name(name)
                .ok_or_else(|| anyhow!("unknown hooks target: {name}"))
                .map(|agent| vec![agent])
        })
        .transpose()?
        .unwrap_or_else(default_hook_targets);

    let mut installed = Vec::new();
    for agent in agents {
        install_hook_target(agent, osc)?;
        installed.push(agent.store_name().to_string());
    }
    Ok(installed)
}

fn uninstall_hook_targets(target: Option<&str>) -> Result<Vec<String>> {
    let agents = target
        .map(|name| {
            agent_hooks::AgentKind::from_hook_name(name)
                .ok_or_else(|| anyhow!("unknown hooks target: {name}"))
                .map(|agent| vec![agent])
        })
        .transpose()?
        .unwrap_or_else(default_hook_targets);

    let mut changed = Vec::new();
    for agent in agents {
        uninstall_hook_target(agent)?;
        changed.push(agent.store_name().to_string());
    }
    Ok(changed)
}

fn default_hook_targets() -> Vec<agent_hooks::AgentKind> {
    vec![
        agent_hooks::AgentKind::Codex,
        agent_hooks::AgentKind::Claude,
        agent_hooks::AgentKind::Gemini,
        agent_hooks::AgentKind::Agy,
    ]
}

fn install_hook_target(agent: agent_hooks::AgentKind, osc: bool) -> Result<()> {
    match agent {
        agent_hooks::AgentKind::Codex => install_json_hooks(
            &codex_hooks_path(),
            agent,
            osc,
            &[
                ("SessionStart", "session-start"),
                ("UserPromptSubmit", "prompt-submit"),
                ("Stop", "stop"),
            ],
        ),
        agent_hooks::AgentKind::Claude => install_json_hooks(
            &claude_settings_path(),
            agent,
            osc,
            &[
                ("SessionStart", "session-start"),
                ("UserPromptSubmit", "prompt-submit"),
                ("Stop", "stop"),
                ("Notification", "notification"),
                ("SessionEnd", "session-end"),
            ],
        ),
        agent_hooks::AgentKind::OpenCode if osc => {
            bail!("OSC hook setup supports Codex, Claude, Gemini, and Antigravity")
        }
        agent_hooks::AgentKind::OpenCode => install_opencode_plugin(),
        agent_hooks::AgentKind::Gemini => install_json_hooks(
            &gemini_settings_path(),
            agent,
            osc,
            &[
                ("SessionStart", "session-start"),
                ("BeforeAgent", "prompt-submit"),
                ("AfterAgent", "stop"),
                ("SessionEnd", "session-end"),
            ],
        ),
        agent_hooks::AgentKind::Agy => install_agy_hooks(&agy_hooks_path(), osc),
    }
}

fn uninstall_hook_target(agent: agent_hooks::AgentKind) -> Result<()> {
    match agent {
        agent_hooks::AgentKind::Codex => uninstall_json_hooks(&codex_hooks_path(), agent),
        agent_hooks::AgentKind::Claude => uninstall_json_hooks(&claude_settings_path(), agent),
        agent_hooks::AgentKind::OpenCode => {
            let path = opencode_plugin_path();
            if path.exists() {
                fs::remove_file(&path)
                    .with_context(|| format!("failed to remove {}", path.display()))?;
            }
            opencode_config_unregister_plugin()
        }
        agent_hooks::AgentKind::Gemini => uninstall_json_hooks(&gemini_settings_path(), agent),
        agent_hooks::AgentKind::Agy => uninstall_agy_hooks(&agy_hooks_path()),
    }
}

fn install_agy_hooks(path: &Path, osc: bool) -> Result<()> {
    let mut root = read_json_object(path)?;
    let marker = hook_marker(agent_hooks::AgentKind::Agy);
    prune_agy_limux_entries(&mut root, marker);
    root.insert(
        "limux".to_string(),
        json!({
            "SessionStart": [{
                "type": "command",
                "command": hook_command_with_fallback(
                    agent_hooks::AgentKind::Agy,
                    "session-start",
                    osc,
                    "{}",
                )?,
                "timeout": hook_timeout(agent_hooks::AgentKind::Agy)
            }],
            "PreInvocation": [{
                "type": "command",
                "command": hook_command_with_fallback(
                    agent_hooks::AgentKind::Agy,
                    "pre-invocation",
                    osc,
                    "{}",
                )?,
                "timeout": hook_timeout(agent_hooks::AgentKind::Agy)
            }],
            "Stop": [{
                "type": "command",
                "command": hook_command_with_fallback(
                    agent_hooks::AgentKind::Agy,
                    "stop",
                    osc,
                    "{\"decision\":\"allow\"}",
                )?,
                "timeout": hook_timeout(agent_hooks::AgentKind::Agy)
            }]
        }),
    );
    write_json_object(path, &root)
}

fn uninstall_agy_hooks(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let mut root = read_json_object(path)?;
    let marker = hook_marker(agent_hooks::AgentKind::Agy);
    prune_agy_limux_entries(&mut root, marker);
    write_json_object(path, &root)
}

fn prune_agy_limux_entries(root: &mut Map<String, Value>, marker: &str) {
    for group in root.values_mut() {
        if let Some(events) = group.as_object_mut() {
            for handlers in events.values_mut() {
                if let Some(entries) = handlers.as_array_mut() {
                    entries.retain(|entry| !json_value_contains(entry, marker));
                }
            }
            events.retain(|_, handlers| {
                handlers
                    .as_array()
                    .map(|entries| !entries.is_empty())
                    .unwrap_or(true)
            });
        }
    }
    root.retain(|name, group| {
        if name == "limux" && json_value_contains(group, marker) {
            return false;
        }
        group
            .as_object()
            .map(|events| !events.is_empty())
            .unwrap_or(true)
    });
}

fn install_json_hooks(
    path: &Path,
    agent: agent_hooks::AgentKind,
    osc: bool,
    events: &[(&str, &str)],
) -> Result<()> {
    let mut root = read_json_object(path)?;
    let hooks = root
        .entry("hooks".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let hooks = hooks
        .as_object_mut()
        .ok_or_else(|| anyhow!("{} has non-object hooks field", path.display()))?;
    let marker = hook_marker(agent);
    for value in hooks.values_mut() {
        if let Some(entries) = value.as_array_mut() {
            entries.retain(|entry| !json_value_contains(entry, marker));
        }
    }
    hooks.retain(|_, value| {
        value
            .as_array()
            .map(|entries| !entries.is_empty())
            .unwrap_or(true)
    });

    for (agent_event, limux_event) in events {
        let entries = hooks
            .entry((*agent_event).to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        let entries = entries
            .as_array_mut()
            .ok_or_else(|| anyhow!("{} hook {agent_event} is not an array", path.display()))?;
        entries.retain(|entry| !json_value_contains(entry, marker));
        let mut entry = json!({
            "hooks": [{
                "type": "command",
                "command": hook_command(agent, limux_event, osc)?,
                "statusMessage": format!("Limux {} session restore", agent.label()),
                "timeout": hook_timeout(agent)
            }]
        });
        if matches!(agent, agent_hooks::AgentKind::Claude) {
            entry["matcher"] = Value::String("*".to_string());
        }
        entries.push(entry);
    }

    write_json_object(path, &root)
}

fn hook_timeout(agent: agent_hooks::AgentKind) -> u64 {
    match agent {
        // Gemini reads milliseconds. The others read seconds.
        agent_hooks::AgentKind::Claude
        | agent_hooks::AgentKind::Codex
        | agent_hooks::AgentKind::Agy => 5,
        agent_hooks::AgentKind::Gemini => 5000,
        agent_hooks::AgentKind::OpenCode => 0,
    }
}

fn uninstall_json_hooks(path: &Path, agent: agent_hooks::AgentKind) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let mut root = read_json_object(path)?;
    if let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) {
        let marker = hook_marker(agent);
        for value in hooks.values_mut() {
            if let Some(entries) = value.as_array_mut() {
                entries.retain(|entry| !json_value_contains(entry, marker));
            }
        }
        hooks.retain(|_, value| {
            value
                .as_array()
                .map(|entries| !entries.is_empty())
                .unwrap_or(true)
        });
    }
    write_json_object(path, &root)
}

fn install_opencode_plugin() -> Result<()> {
    let path = opencode_plugin_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(&path, opencode_plugin_source()?).context("failed to write OpenCode plugin")?;
    opencode_config_register_plugin(&path)
}

fn opencode_config_register_plugin(plugin_path: &Path) -> Result<()> {
    let config_path = opencode_config_path();
    let mut root = read_json_object(&config_path)?;
    let plugins = root
        .entry("plugin".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let plugins = plugins
        .as_array_mut()
        .ok_or_else(|| anyhow!("{} has non-array plugin field", config_path.display()))?;
    let plugin_str = plugin_path.to_string_lossy().into_owned();
    if !plugins.iter().any(|v| v.as_str() == Some(&plugin_str)) {
        plugins.push(Value::String(plugin_str));
    }
    write_json_object(&config_path, &root)
}

fn opencode_config_unregister_plugin() -> Result<()> {
    let config_path = opencode_config_path();
    if !config_path.exists() {
        return Ok(());
    }
    let plugin_path = opencode_plugin_path();
    let plugin_str = plugin_path.to_string_lossy().into_owned();
    let mut root = read_json_object(&config_path)?;
    if let Some(plugins) = root.get_mut("plugin").and_then(Value::as_array_mut) {
        plugins.retain(|v| v.as_str() != Some(&plugin_str));
    }
    write_json_object(&config_path, &root)
}

fn hook_command(agent: agent_hooks::AgentKind, event: &str, osc: bool) -> Result<String> {
    hook_command_with_fallback(
        agent,
        event,
        osc,
        "{\"continue\":true,\"suppressOutput\":false}",
    )
}

fn hook_command_with_fallback(
    agent: agent_hooks::AgentKind,
    event: &str,
    osc: bool,
    fallback_json: &str,
) -> Result<String> {
    let disable_var = format!(
        "LIMUX_{}_HOOKS_DISABLED",
        agent.store_name().to_ascii_uppercase()
    );
    let limux_command = hook_cli_command()?;
    let limux_command = if osc {
        format!("env LIMUX_NOTIFY_OSC=1 {limux_command}")
    } else {
        limux_command
    };
    Ok(format!(
        "[ \"${{{disable_var}:-}}\" != \"1\" ] && {limux_command} --json hooks {} {} || echo {}",
        agent.store_name(),
        event,
        shell_single_quote(fallback_json)
    ))
}

fn hook_cli_command() -> Result<String> {
    if let Some(path) = limux_control::flatpak::host_cli_executable() {
        return Ok(shell_single_quote(&path.to_string_lossy()));
    }
    let exe = env::current_exe().context("failed to resolve current executable")?;
    let file_name = exe
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    if file_name == "limux-cli" {
        return Ok(shell_single_quote(&exe.to_string_lossy()));
    }
    Ok("limux".to_string())
}

fn opencode_plugin_cli_command() -> Result<String> {
    if let Some(path) = limux_control::flatpak::host_cli_executable() {
        return Ok(path.to_string_lossy().into_owned());
    }
    let exe = env::current_exe().context("failed to resolve current executable")?;
    let file_name = exe
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    if file_name == "limux-cli" {
        return Ok(exe.to_string_lossy().to_string());
    }
    Ok("limux".to_string())
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn hook_marker(agent: agent_hooks::AgentKind) -> &'static str {
    match agent {
        agent_hooks::AgentKind::Claude => "hooks claude",
        agent_hooks::AgentKind::Codex => "hooks codex",
        agent_hooks::AgentKind::OpenCode => "hooks opencode",
        agent_hooks::AgentKind::Gemini => "hooks gemini",
        agent_hooks::AgentKind::Agy => "hooks agy",
    }
}

fn read_json_object(path: &Path) -> Result<Map<String, Value>> {
    if !path.exists() {
        return Ok(Map::new());
    }
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(Map::new());
    }
    let value: Value = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| anyhow!("{} must contain a JSON object", path.display()))
}

fn write_json_object(path: &Path, object: &Map<String, Value>) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let temp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let encoded = serde_json::to_vec_pretty(object).context("failed to encode hook config")?;
    fs::write(&temp, encoded).with_context(|| format!("failed to write {}", temp.display()))?;
    fs::rename(&temp, path).with_context(|| format!("failed to replace {}", path.display()))
}

fn json_value_contains(value: &Value, needle: &str) -> bool {
    match value {
        Value::String(value) => value.contains(needle),
        Value::Array(values) => values
            .iter()
            .any(|value| json_value_contains(value, needle)),
        Value::Object(map) => map.values().any(|value| json_value_contains(value, needle)),
        _ => false,
    }
}

fn non_empty_env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn codex_hooks_path() -> PathBuf {
    non_empty_env_path("CODEX_HOME")
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))
        .unwrap_or_else(|| PathBuf::from(".codex"))
        .join("hooks.json")
}

fn claude_settings_path() -> PathBuf {
    non_empty_env_path("CLAUDE_CONFIG_DIR")
        .or_else(|| dirs::home_dir().map(|home| home.join(".claude")))
        .unwrap_or_else(|| PathBuf::from(".claude"))
        .join("settings.json")
}

fn gemini_settings_path_from(gemini_cli_home: Option<PathBuf>, home: Option<PathBuf>) -> PathBuf {
    gemini_cli_home
        .or(home)
        .map(|base| base.join(".gemini/settings.json"))
        .unwrap_or_else(|| PathBuf::from(".gemini/settings.json"))
}

fn gemini_settings_path() -> PathBuf {
    gemini_settings_path_from(non_empty_env_path("GEMINI_CLI_HOME"), dirs::home_dir())
}

fn agy_hooks_path_from(home: Option<PathBuf>) -> PathBuf {
    home.map(|home| home.join(".gemini/config/hooks.json"))
        .unwrap_or_else(|| PathBuf::from(".gemini/config/hooks.json"))
}

fn agy_hooks_path() -> PathBuf {
    agy_hooks_path_from(dirs::home_dir())
}

fn opencode_config_dir() -> PathBuf {
    non_empty_env_path("OPENCODE_CONFIG_DIR")
        .or_else(|| dirs::home_dir().map(|home| home.join(".config/opencode")))
        .unwrap_or_else(|| PathBuf::from(".config/opencode"))
}

fn opencode_plugin_path() -> PathBuf {
    opencode_config_dir().join("plugins/limux-session.js")
}

fn opencode_config_path() -> PathBuf {
    opencode_config_dir().join("config.json")
}

fn opencode_plugin_source() -> Result<String> {
    opencode_plugin_source_with_command(&opencode_plugin_cli_command()?)
}

fn opencode_plugin_source_with_command(limux_command: &str) -> Result<String> {
    let limux_command_json =
        serde_json::to_string(limux_command).context("failed to encode OpenCode hook command")?;
    Ok(
        r#"// limux-opencode-session-plugin v2
// Installed by `limux hooks opencode install`. Do not edit manually.

import { spawnSync } from "node:child_process";
import { appendFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";

const LIMUX_COMMAND = __LIMUX_COMMAND__;

function debug(outcome, details = {}) {
  if (process.env.LIMUX_OPENCODE_HOOK_DEBUG !== "1" && outcome !== "spawn_failed") return;
  try {
    const dir = process.env.LIMUX_AGENT_HOOK_STATE_DIR || (process.env.XDG_STATE_HOME ? join(process.env.XDG_STATE_HOME, "limux") : join(process.env.HOME || ".", ".local/state/limux"));
    mkdirSync(dir, { recursive: true });
    appendFileSync(join(dir, "opencode-plugin-debug.jsonl"), JSON.stringify({
      time: Date.now() / 1000,
      outcome,
      details
    }) + "\n");
  } catch (_) {}
}

function firstString(...values) {
  for (const value of values) {
    if (typeof value === "string" && value.trim().length > 0) return value.trim();
  }
  return null;
}

function props(event) {
  return (event && typeof event === "object" && event.properties) || {};
}

function data(event) {
  return (event && typeof event === "object" && event.data) || {};
}

function info(event) {
  const p = props(event);
  const d = data(event);
  return (p.info && typeof p.info === "object" && p.info) || (d.info && typeof d.info === "object" && d.info) || {};
}

function eventType(event) {
  const raw = firstString(event && event.type, event && event.name);
  if (!raw) return null;
  if (raw === "sync") return firstString(event && event.name);
  return raw.endsWith(".1") ? raw.slice(0, -2) : raw;
}

function sessionId(event) {
  const p = props(event);
  const d = data(event);
  const i = info(event);
  return firstString(p.sessionID, p.sessionId, p.session_id, d.sessionID, d.sessionId, d.session_id, i.id, event && event.sessionID, event && event.sessionId);
}

function cwd(ctx, event) {
  const p = props(event);
  const d = data(event);
  const i = info(event);
  return firstString(p.cwd, p.directory, d.cwd, d.directory, i.directory, i.path, ctx && ctx.directory, process.cwd());
}

function launchExecutable() {
  return firstString(process.env.LIMUX_OPENCODE_EXECUTABLE, "opencode");
}

function send(kind, ctx, event) {
  if (process.env.LIMUX_OPENCODE_HOOKS_DISABLED === "1") {
    debug("skip_disabled", { kind });
    return;
  }
  if (!process.env.LIMUX_SURFACE_ID) {
    debug("skip_missing_surface", { kind, type: eventType(event), hasWorkspace: !!process.env.LIMUX_WORKSPACE_ID });
    return;
  }
  const sid = sessionId(event);
  if (!sid) {
    debug("skip_missing_session", { kind, type: eventType(event), keys: Object.keys(event || {}) });
    return;
  }
  const type = eventType(event);
  const payload = {
    session_id: sid,
    cwd: cwd(ctx, event),
    hook_event_name: type,
    event: type
  };
  try {
    const command = process.env.LIMUX_BIN || LIMUX_COMMAND;
    const result = spawnSync(command, ["hooks", "opencode", kind], {
      input: JSON.stringify(payload),
      encoding: "utf8",
      stdio: ["pipe", "ignore", "ignore"],
      timeout: 5000,
      env: {
        ...process.env,
        LIMUX_AGENT_LAUNCH_ARGV: launchExecutable(),
        LIMUX_AGENT_LAUNCH_EXECUTABLE: launchExecutable(),
        LIMUX_AGENT_LAUNCH_CWD: cwd(ctx, event)
      }
    });
    debug("spawned", { kind, type, status: result.status, error: result.error && String(result.error), command });
  } catch (error) {
    debug("spawn_failed", { kind, type, error: String(error) });
  }
}

const limuxSessionRestore = async (ctx) => {
  debug("plugin_started", { directory: ctx && ctx.directory, hasSurface: !!process.env.LIMUX_SURFACE_ID, hasWorkspace: !!process.env.LIMUX_WORKSPACE_ID });
  return {
    event: async ({ event }) => {
    const type = eventType(event);
    debug("event", { type, rawType: event && event.type, rawName: event && event.name });
    if (!type) return;
    if (type === "session.created") send("session-start", ctx, event);
    if (type === "session.idle" || type === "session.updated" || type === "session.status" || type === "session.compacted") send("prompt-submit", ctx, event);
    if (type === "session.error") send("session-end", ctx, event);
    if (type === "session.deleted") send("cleanup", ctx, event);
    }
  };
};

export const LimuxSessionRestore = limuxSessionRestore;
export default limuxSessionRestore;
"#
        .replace("__LIMUX_COMMAND__", &limux_command_json),
    )
}

async fn run_new_workspace(client: &mut Client, args: &[String]) -> Result<Value> {
    let cwd = parse_opt(args, "--cwd");
    let command = parse_opt(args, "--command");
    let original = resolve_current_workspace(client).await?;

    let mut params = Map::new();
    if let Some(cwd_value) = cwd.as_ref() {
        params.insert("cwd".to_string(), Value::String(cwd_value.clone()));
    }
    if let Some(command) = command.clone() {
        params.insert("command".to_string(), Value::String(command));
    }

    let created = client
        .call("workspace.create", Value::Object(params))
        .await
        .context("workspace.create failed")?;

    let _ = client
        .call("workspace.select", json!({ "workspace_id": original }))
        .await;

    Ok(created)
}

// ---------------------------------------------------------------------------
// `limux agent-team` — spin up a multi-agent collaboration workspace.
// ---------------------------------------------------------------------------
//
// Creates ONE workspace and one pane per requested agent (codex / claude /
// opencode / gemini), launches each agent's CLI in its pane, captures the
// pane/surface IDs, and seeds an AGENTS.md in the shared cwd describing the
// XML-tagged message protocol and the peer directory so agents can message
// each other.
//
// The protocol (codified in AGENTS.md):
//   To send a message to a peer, run from any terminal:
//     limux send --surface <peer-surface-id> \\
//       $'<agent-msg from="<me>" to="<peer>" ts="<iso-8601>">\\n...\\n</agent-msg>\\n'
//
// Peers read their own terminals normally — the text appears at the prompt.
// Each agent should watch for <agent-msg from="..."> blocks and reply with
// the same envelope targeted back.

/// Built-in agent launcher commands. Chosen to match the CLIs the user
/// actually has installed (see README); the launch command is what gets
/// typed into the new workspace's terminal, so it also works as a fallback
/// shell command if the CLI isn't in PATH yet.
fn agent_launch_command(agent: &str) -> Option<(&'static str, String)> {
    match agent.to_lowercase().as_str() {
        "codex" => Some(("codex", "codex".to_string())),
        "claude" | "claude-code" => Some(("claude", "claude".to_string())),
        "opencode" => Some(("opencode", "opencode".to_string())),
        "gemini" | "gemini-cli" => Some(("gemini", "gemini".to_string())),
        "agy" | "antigravity" | "antigravity-cli" => Some(("agy", "agy".to_string())),
        _ => None,
    }
}

async fn run_agent_team(client: &mut Client, args: &[String]) -> Result<Value> {
    // Parse --agents codex,claude (default: codex,claude).
    let agents_raw = parse_opt(args, "--agents").unwrap_or_else(|| "codex,claude".to_string());
    let agents: Vec<String> = agents_raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if agents.is_empty() {
        bail!("agent-team: --agents is empty");
    }

    let requested_cwd = parse_opt(args, "--cwd")
        .or_else(|| {
            env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().to_string())
        })
        .ok_or_else(|| anyhow!("agent-team: could not resolve --cwd"))?;

    // Optional: skip launching the CLIs (useful when the user wants to open
    // the agents manually) — still splits the panes + writes AGENTS.md.
    let no_launch = args.iter().any(|a| a == "--no-launch");
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let cwd = if dry_run {
        std::path::absolute(&requested_cwd)
    } else {
        fs::canonicalize(&requested_cwd)
    }
    .context("agent-team: could not resolve cwd")?;
    if !dry_run && !cwd.is_dir() {
        bail!("agent-team: cwd must be a directory");
    }
    let cwd = cwd.to_string_lossy().into_owned();

    // Resolve the agent list up front so --dry-run can build a deterministic
    // peer table without touching the host.
    let resolved: Vec<(String, &'static str, String)> = agents
        .iter()
        .filter_map(|agent| {
            agent_launch_command(agent).map(|(name, launch)| (agent.clone(), name, launch))
        })
        .collect();
    for agent in &agents {
        if agent_launch_command(agent).is_none() {
            eprintln!("agent-team: unknown agent '{agent}', skipping");
        }
    }
    if resolved.is_empty() {
        bail!("agent-team: no valid agents spawned");
    }

    let agents_md_path = std::path::Path::new(&cwd).join("AGENTS.md");

    if dry_run {
        let peers: Vec<(String, String, String, String)> = resolved
            .iter()
            .enumerate()
            .map(|(i, (_, name, launch))| {
                (
                    name.to_string(),
                    format!("<dry-run-pane-{i}>"),
                    format!("<dry-run-surface-{name}>"),
                    launch.clone(),
                )
            })
            .collect();
        let body = build_agents_md(
            &peers,
            &cwd,
            "<active-workspace>",
            "<dry-run-workspace>",
            "<dry-run-orchestrator>",
        );
        return Ok(json!({
            "ok": true,
            "cwd": cwd,
            "workspace_name": "<active-workspace>",
            "workspace_id": Value::Null,
            "orchestrator_surface_id": Value::Null,
            "agents_md": agents_md_path.to_string_lossy(),
            "agents_md_preview": body,
            "dry_run": true,
            "no_launch": no_launch,
            "peers": peers
                .iter()
                .map(|(name, pane, surface, launch)| {
                    json!({
                        "agent": name,
                        "pane_id": pane,
                        "surface_id": surface,
                        "launch_command": launch,
                    })
                })
                .collect::<Vec<_>>(),
        }));
    }

    // Reject existing instructions and check the destination before creating panes.
    let instructions = agent_team_file::PendingInstructions::new(&agents_md_path)?;

    // 1. Resolve the orchestrator's workspace + pane. Prefer LIMUX_* env (set
    //    in every limux-spawned terminal) and fall back to the host's active
    //    focus so callers from a regular shell still work.
    let orchestrator_workspace = env::var("LIMUX_WORKSPACE_ID")
        .ok()
        .filter(|s| !s.is_empty());
    let orchestrator_surface_env = env::var("LIMUX_SURFACE_ID").ok().filter(|s| !s.is_empty());
    let orchestrator_pane_env = env::var("LIMUX_PANE_ID").ok().filter(|s| !s.is_empty());

    let workspace_id = match orchestrator_workspace.clone() {
        Some(id) => id,
        None => resolve_current_workspace(client)
            .await
            .context("agent-team: could not resolve active workspace; run from inside a limux pane or pass --workspace")?,
    };

    // 2. Discover the orchestrator pane's surface_id. If env didn't tell us,
    //    use the focused/first surface in the workspace.
    let surfaces = client
        .call(
            "surface.list",
            json!({ "workspace_id": workspace_id.clone() }),
        )
        .await
        .context("surface.list failed for active workspace")?;
    let surface_rows = surfaces
        .get("surfaces")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if surface_rows.is_empty() {
        bail!("agent-team: active workspace has no surfaces");
    }
    let orchestrator_surface = orchestrator_surface_env.clone().unwrap_or_else(|| {
        surface_rows
            .iter()
            .find(|row| row.get("focused").and_then(Value::as_bool) == Some(true))
            .and_then(|row| get_string(row, &["surface_id"]))
            .or_else(|| get_string(&surface_rows[0], &["surface_id"]))
            .unwrap_or_default()
    });
    if orchestrator_surface.is_empty() {
        bail!("agent-team: could not determine orchestrator surface");
    }
    let orchestrator_pane = orchestrator_pane_env.unwrap_or_else(|| {
        surface_rows
            .iter()
            .find(|row| {
                get_string(row, &["surface_id"]).as_deref() == Some(orchestrator_surface.as_str())
            })
            .and_then(|row| get_string(row, &["pane_id"]))
            .unwrap_or_default()
    });

    // 3. Workspace name (for AGENTS.md header) — best-effort lookup.
    let workspace_name = client
        .call("workspace.list", json!({}))
        .await
        .ok()
        .and_then(|v| v.get("workspaces").and_then(Value::as_array).cloned())
        .and_then(|rows| {
            rows.into_iter().find(|row| {
                get_string(row, &["workspace_id", "id"]).as_deref() == Some(workspace_id.as_str())
            })
        })
        .and_then(|row| get_string(&row, &["name", "title"]))
        .unwrap_or_else(|| "active workspace".to_string());

    // 4. Split a pane per agent. Layout: agent[0] splits RIGHT of orchestrator,
    //    each subsequent agent splits DOWN of the previous agent — orchestrator
    //    keeps its full height on the left, peers stack top-to-bottom on the right.
    let mut peers: Vec<(String, String, String, String)> = Vec::new();
    let mut parent_surface = orchestrator_surface.clone();

    for (i, (_alias, name, launch)) in resolved.iter().enumerate() {
        let direction = if i == 0 { "right" } else { "down" };

        let mut params = Map::new();
        params.insert(
            "workspace_id".to_string(),
            Value::String(workspace_id.clone()),
        );
        params.insert(
            "surface_id".to_string(),
            Value::String(parent_surface.clone()),
        );
        params.insert(
            "direction".to_string(),
            Value::String(direction.to_string()),
        );
        params.insert("type".to_string(), Value::String("terminal".to_string()));
        params.insert(
            "command".to_string(),
            Value::String(instructions.pane_command((!no_launch).then_some(launch.as_str()))),
        );

        let created = client
            .call("pane.create", Value::Object(params))
            .await
            .with_context(|| format!("pane.create failed for agent '{name}'"))?;
        let pane_id = get_string(&created, &["pane_id"])
            .ok_or_else(|| anyhow!("agent-team: pane.create for '{name}' returned no pane_id"))?;
        let surface_id = get_string(&created, &["surface_id"]).ok_or_else(|| {
            anyhow!("agent-team: pane.create for '{name}' returned no surface_id")
        })?;

        parent_surface = surface_id.clone();
        peers.push((name.to_string(), pane_id, surface_id, launch.clone()));
    }

    // 5. Publish all peer IDs before releasing the gated startup commands.
    // Dropping the stage on any earlier error prevents every agent from launching.
    let body = build_agents_md(
        &peers,
        &cwd,
        &workspace_name,
        &workspace_id,
        &orchestrator_surface,
    );
    instructions.publish(&body)?;

    Ok(json!({
        "ok": true,
        "cwd": cwd,
        "workspace_name": workspace_name,
        "workspace_id": workspace_id,
        "orchestrator_pane_id": orchestrator_pane,
        "orchestrator_surface_id": orchestrator_surface,
        "agents_md": agents_md_path.to_string_lossy(),
        "dry_run": false,
        "no_launch": no_launch,
        "peers": peers
            .iter()
            .map(|(name, pane, surface, launch)| {
                json!({
                    "agent": name,
                    "pane_id": pane,
                    "surface_id": surface,
                    "launch_command": launch,
                })
            })
            .collect::<Vec<_>>(),
    }))
}

fn build_agents_md(
    peers: &[(String, String, String, String)],
    cwd: &str,
    workspace_name: &str,
    workspace_id: &str,
    orchestrator_surface: &str,
) -> String {
    let mut out = String::new();
    out.push_str("# AGENTS.md — agent-to-agent message protocol\n\n");
    out.push_str(
        "This file is auto-generated by `limux agent-team`. It defines how the\n\
         agents running in this workspace team communicate with each other via\n\
         the limux control socket. Humans should feel free to edit the\n\
         'Policies' section below; everything else is mechanical.\n\n",
    );

    out.push_str(&format!(
        "## Team workspace\n\n\
         The orchestrator (the pane that ran `limux agent-team`) and all\n\
         spawned peers share one workspace:\n\n\
         - Workspace name: `{workspace_name}`\n\
         - Workspace ID: `{workspace_id}`\n\
         - Orchestrator surface: `{orchestrator_surface}`\n\
         - Shared cwd: `{cwd}`\n\n",
    ));

    out.push_str("## Peers in this team\n\n");
    out.push_str("| Agent | Pane | Surface | Launch command |\n");
    out.push_str("|-------|------|---------|----------------|\n");
    for (name, pane_id, surface_id, launch) in peers {
        out.push_str(&format!(
            "| `{name}` | `{pane_id}` | `{surface_id}` | `{launch}` |\n"
        ));
    }
    out.push('\n');
    out.push_str(
        "The orchestrator is not in the table — message it back using its\n\
         `Orchestrator surface` from the block above.\n\n",
    );

    out.push_str("## How to send a message\n\n");
    out.push_str(
        "Messages use the `<agent-msg>` XML envelope so they're easy to\n\
         extract from the terminal scrollback. To send a message to a peer,\n\
         look up their `Surface` in the peers table above and run (from any\n\
         shell, including the agent's own terminal — `limux` is on PATH):\n\n",
    );
    out.push_str("```bash\n");
    out.push_str("limux send --surface <peer-surface-id> $'<agent-msg from=\"<me>\" to=\"<peer>\" id=\"<uuid>\" ts=\"<iso8601>\">\\n<body/>\\n</agent-msg>\\n'\n");
    out.push_str("```\n\n");
    out.push_str(
        "The message appears at the peer's prompt as plain stdin, so the\n\
         peer's agent CLI picks it up like a normal user message. Trailing\n\
         newline is required so the agent's read-line actually fires.\n\n",
    );

    out.push_str("### Envelope format\n\n");
    out.push_str("```xml\n");
    out.push_str("<agent-msg from=\"codex\" to=\"claude\" id=\"<uuid>\" ts=\"2026-04-19T16:48:00Z\" reply-to=\"<parent-uuid>\">\n");
    out.push_str(
        "  <context>optional: one or two sentences about what the request is for</context>\n",
    );
    out.push_str("  <request>the actual ask, in prose or code</request>\n");
    out.push_str("  <expect>how you want the peer to reply (\"inline code diff\" / \"short summary\" / etc.)</expect>\n");
    out.push_str("</agent-msg>\n");
    out.push_str("```\n\n");

    out.push_str("Rules:\n");
    out.push_str("- `from` / `to` MUST be one of the agent names in the peers table.\n");
    out.push_str("- `id` is a fresh UUID (e.g. `uuidgen`); peers echo it in `reply-to`.\n");
    out.push_str("- `ts` is ISO-8601 UTC (`date -u +%Y-%m-%dT%H:%M:%SZ`).\n");
    out.push_str("- Inner tags are guidance, not required — `<request>` alone is fine.\n");
    out.push_str("- Keep bodies short; link to files in the shared cwd for anything long.\n\n");

    out.push_str("### Replying\n\n");
    out.push_str("Reply with the envelope reversed and `reply-to` set to the original `id`:\n\n");
    out.push_str("```bash\n");
    out.push_str("limux send --surface <orig-sender-surface-id> $'<agent-msg from=\"claude\" to=\"codex\" id=\"<new-uuid>\" reply-to=\"<orig-uuid>\" ts=\"<iso8601>\">\\n<response>...</response>\\n</agent-msg>\\n'\n");
    out.push_str("```\n\n");

    out.push_str("## Pinging the human\n\n");
    out.push_str(
        "When you need human input, use `limux notify` — it pops a toast\n\
         and lights up the workspace in the sidebar. Example:\n\n",
    );
    out.push_str("```bash\n");
    out.push_str("limux notify --subtitle 'needs review' --body 'Claude blocked on auth choice' 'Input needed'\n");
    out.push_str("```\n\n");

    out.push_str("## Environment contract\n\n");
    out.push_str(
        "Every pane spawned by limux inherits:\n\
         - `LIMUX_WORKSPACE_ID` — the team workspace's UUID\n\
         - `LIMUX_SURFACE_ID` — this pane's surface id (this is your `from`)\n\
         - `LIMUX_PANE_ID`, `LIMUX_TAB_ID`\n\
         - `LIMUX_SOCKET` — the control socket path\n\n\
         This means `limux identify`, `limux send` (with `--surface`), and\n\
         `limux notify` all auto-target the right thing with no flags needed\n\
         from inside the agent's own terminal.\n\n",
    );

    out.push_str("## Splitting your own pane\n\n");
    out.push_str("If you need a scratch terminal next to you, split your own pane:\n\n");
    out.push_str("```bash\n");
    out.push_str("limux new-pane --direction right --command bash\n");
    out.push_str("```\n\n");
    out.push_str(
        "`new-pane` reads `LIMUX_WORKSPACE_ID`, `LIMUX_SURFACE_ID`, and\n\
         `LIMUX_PANE_ID`, so it splits your current pane even if GTK focus has\n\
         moved elsewhere. Live GTK self-spawn supports both terminal and browser\n\
         panes (`--type browser --url <url>`).\n\n",
    );

    out.push_str("## Policies (edit these freely)\n\n");
    out.push_str(
        "- If a peer is silent for more than 60 seconds, re-send with `reply-to` = your last id.\n",
    );
    out.push_str(
        "- Never send more than 200 lines at once; write to a file and send the path instead.\n",
    );
    out.push_str("- If two agents disagree on an approach, both message the human via `limux notify` and stop.\n");
    out.push_str("- Before taking destructive actions (rm, git push, kubectl apply), ask the human via `limux notify`.\n\n");

    out.push_str("---\n");
    out.push_str(
        "_Generated by `limux agent-team`. Safe to edit the Policies\n\
         section; keep or move this file before creating another team._\n",
    );

    out
}

async fn run_close_workspace(client: &mut Client, args: &[String]) -> Result<Value> {
    let workspace = parse_opt(args, "--workspace")
        .or_else(|| env::var("LIMUX_WORKSPACE_ID").ok())
        .ok_or_else(|| anyhow!("close-workspace requires --workspace <id|ref>"))?;
    client
        .call("workspace.close", json!({ "workspace_id": workspace }))
        .await
}

async fn run_close_surface(client: &mut Client, args: &[String]) -> Result<Value> {
    let params = lifecycle_scope(args, Some(("--surface", "surface_id", "LIMUX_SURFACE_ID")))?;
    client.call("surface.close", Value::Object(params)).await
}

async fn run_focus_surface(client: &mut Client, args: &[String]) -> Result<Value> {
    let params = lifecycle_scope(args, Some(("--surface", "surface_id", "LIMUX_SURFACE_ID")))?;
    client.call("surface.focus", Value::Object(params)).await
}

async fn run_focus_pane(client: &mut Client, args: &[String]) -> Result<Value> {
    let params = lifecycle_scope(args, Some(("--pane", "pane_id", "LIMUX_PANE_ID")))?;
    client.call("pane.focus", Value::Object(params)).await
}

async fn run_new_surface(client: &mut Client, args: &[String]) -> Result<Value> {
    let mut params = lifecycle_scope(args, Some(("--pane", "pane_id", "LIMUX_PANE_ID")))?;
    for (flag, key) in [("--type", "type"), ("--url", "url")] {
        if let Some(value) = lifecycle_option(args, flag)? {
            params.insert(key.to_string(), Value::String(value));
        }
    }
    client.call("surface.create", Value::Object(params)).await
}

fn lifecycle_option(args: &[String], flag: &str) -> Result<Option<String>> {
    let value = parse_opt(args, flag);
    if parse_flag(args, flag)
        && value
            .as_ref()
            .is_none_or(|value| value.trim().is_empty() || value.starts_with("--"))
    {
        bail!("{flag} requires a non-empty value");
    }
    Ok(value)
}

fn lifecycle_scope(
    args: &[String],
    target: Option<(&str, &str, &str)>,
) -> Result<Map<String, Value>> {
    let mut params = Map::new();
    let explicit_workspace = lifecycle_option(args, "--workspace")?;
    let target = match target {
        Some((flag, key, env_key)) => {
            // An explicit workspace must not inherit the caller's target from another workspace.
            let value = lifecycle_option(args, flag)?.or_else(|| {
                (!parse_flag(args, "--workspace"))
                    .then(|| env::var(env_key).ok().filter(|value| !value.is_empty()))
                    .flatten()
            });
            value.map(|value| (key, value))
        }
        None => None,
    };
    let surface = target
        .as_ref()
        .filter(|(key, _)| *key == "surface_id")
        .map(|(_, value)| value.as_str());
    if let Some(workspace) = request_workspace(explicit_workspace, surface, env_opt) {
        params.insert("workspace_id".to_string(), Value::String(workspace));
    }
    if let Some((key, value)) = target {
        params.insert(key.to_string(), Value::String(value));
    }
    Ok(params)
}

/// `limux select-workspace --workspace <id|ref>` — bring a workspace to the front.
///
/// The host has always implemented `workspace.select`; it just wasn't reachable
/// from the CLI. It needs to be, because a pane only realizes its ghostty surface
/// once its workspace is displayed. Splits made in a background workspace stay
/// unrealized — `surface-health` reports `healthy=false` and `send-key` fails —
/// until something selects it. Without this verb a purely CLI-driven fleet cannot
/// bring its own workspace forward, so it can never start.
async fn run_select_workspace(client: &mut Client, args: &[String]) -> Result<Value> {
    let workspace = parse_opt(args, "--workspace")
        .or_else(|| env::var("LIMUX_WORKSPACE_ID").ok())
        .ok_or_else(|| anyhow!("select-workspace requires --workspace <id|ref>"))?;
    client
        .call("workspace.select", json!({ "workspace_id": workspace }))
        .await
}

async fn run_sidebar_state(client: &mut Client, args: &[String]) -> Result<Value> {
    let workspace = parse_opt(args, "--workspace")
        .or_else(|| env::var("LIMUX_WORKSPACE_ID").ok())
        .ok_or_else(|| anyhow!("sidebar-state requires --workspace <id|ref>"))?;

    let listed = client.call("workspace.list", json!({})).await?;
    let rows = listed
        .get("workspaces")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let matched = rows.into_iter().find(|row| {
        let id = get_string(row, &["workspace_id", "id"]).unwrap_or_default();
        let rf = get_string(row, &["workspace_ref", "ref"]).unwrap_or_default();
        workspace == id || workspace == rf
    });

    let cwd = matched
        .as_ref()
        .and_then(|row| get_string(row, &["cwd"]))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "none".to_string());

    let git_branch = if cwd != "none" {
        let output = Command::new("git")
            .arg("-C")
            .arg(&cwd)
            .arg("rev-parse")
            .arg("--abbrev-ref")
            .arg("HEAD")
            .output();
        match output {
            Ok(out) if out.status.success() => {
                String::from_utf8_lossy(&out.stdout).trim().to_string()
            }
            _ => "none".to_string(),
        }
    } else {
        "none".to_string()
    };

    Ok(json!({
        "workspace": workspace,
        "cwd": cwd,
        "git_branch": git_branch,
    }))
}

fn env_opt(name: &str) -> Option<String> {
    env::var(name).ok()
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.trim().is_empty())
}

/// The workspace a request is scoped to: an explicit `--workspace`, where an
/// empty one means unscoped, else the caller's LIMUX_WORKSPACE_ID unless the
/// request names a surface. A shell keeps that variable after its tab moves to
/// another workspace, so a named surface is sent unscoped and the host looks
/// for it in every workspace.
fn request_workspace(
    explicit: Option<String>,
    surface: Option<&str>,
    env_lookup: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    match explicit {
        Some(explicit) => nonempty(Some(explicit)),
        None => surface
            .is_none()
            .then(|| nonempty(env_lookup("LIMUX_WORKSPACE_ID")))
            .flatten(),
    }
}

fn build_new_pane_request(
    args: &[String],
    env_lookup: impl Fn(&str) -> Option<String>,
) -> (Option<String>, Value) {
    let surface = nonempty(parse_opt(args, "--surface").or_else(|| env_lookup("LIMUX_SURFACE_ID")));
    let workspace = request_workspace(
        parse_opt(args, "--workspace"),
        surface.as_deref(),
        &env_lookup,
    );
    let pane = nonempty(parse_opt(args, "--pane").or_else(|| env_lookup("LIMUX_PANE_ID")));
    let direction = parse_opt(args, "--direction").unwrap_or_else(|| "right".to_string());
    let pane_type = parse_opt(args, "--type").unwrap_or_else(|| "terminal".to_string());
    let command = nonempty(parse_opt(args, "--command"));
    let url = nonempty(parse_opt(args, "--url"));

    let mut params = Map::new();
    params.insert("direction".to_string(), Value::String(direction));
    params.insert("type".to_string(), Value::String(pane_type));
    if let Some(surface) = surface {
        params.insert("surface_id".to_string(), Value::String(surface));
    }
    if let Some(pane) = pane {
        params.insert("pane_id".to_string(), Value::String(pane));
    }
    if let Some(command) = command {
        params.insert("command".to_string(), Value::String(command));
    }
    if let Some(url) = url {
        params.insert("url".to_string(), Value::String(url));
    }

    (workspace, Value::Object(params))
}

async fn run_new_pane(client: &mut Client, args: &[String]) -> Result<Value> {
    // `pane.create` contract shared with the core dispatcher and live GTK host:
    // direction/type are validated by the server, and responses keep
    // pane_id/pane_ref/surface_id/surface_ref. Inside a Limux terminal,
    // LIMUX_* defaults make `limux new-pane --command claude` split the
    // caller's pane; outside Limux, omitting workspace preserves active-focus
    // server behavior.
    let (workspace, params) = build_new_pane_request(args, env_opt);
    call_in_workspace_scope(client, workspace, "pane.create", params).await
}

fn build_read_screen_params(args: &[String]) -> Result<Value> {
    let mut params = Map::new();
    if let Some(raw_lines) = parse_opt(args, "--lines") {
        let lines = raw_lines
            .parse::<u64>()
            .ok()
            .filter(|&lines| lines > 0)
            .ok_or_else(|| anyhow!("--lines must be greater than 0"))?;
        params.insert("lines".to_string(), Value::Number(lines.into()));
    } else if parse_flag(args, "--lines") {
        bail!("--lines must be greater than 0");
    }

    if parse_flag(args, "--scrollback") {
        params.insert("scrollback".to_string(), Value::Bool(true));
    }
    if let Some(workspace) = parse_opt(args, "--workspace") {
        params.insert("workspace_id".to_string(), Value::String(workspace));
    }
    if let Some(surface) = parse_terminal_surface(args)? {
        params.insert("surface_id".to_string(), Value::String(surface));
    }
    Ok(Value::Object(params))
}

async fn run_read_screen(client: &mut Client, args: &[String]) -> Result<Value> {
    let params = build_read_screen_params(args)?;
    client.call("surface.read_text", params).await
}

/// Build the `workspace.set_color` params; the host validates the colour name.
fn build_set_workspace_color_params(
    args: &[String],
    env_value: impl Fn(&str) -> Option<String>,
) -> Result<Value> {
    let color = trailing_title(args)
        .ok_or_else(|| anyhow!("set-workspace-color requires a color name or \"none\""))?;

    let mut params = Map::new();
    params.insert("color".to_string(), Value::String(color));
    if let Some(workspace) =
        parse_opt(args, "--workspace").or_else(|| env_value("LIMUX_WORKSPACE_ID"))
    {
        params.insert("workspace_id".to_string(), Value::String(workspace));
    }
    Ok(Value::Object(params))
}

/// The socket method and params of a sidebar workspace-group command.
fn build_group_request(
    command: &str,
    args: &[String],
    env_value: impl Fn(&str) -> Option<String>,
) -> Result<(&'static str, Value)> {
    let mut params = Map::new();
    let group = || {
        parse_opt(args, "--group")
            .filter(|group| !group.trim().is_empty())
            .ok_or_else(|| anyhow!("{command} requires --group <id|ref|name>"))
    };
    let method = match command {
        "list-groups" => "workspace.group_list",
        "create-group" => {
            if let Some(name) = trailing_title(args) {
                params.insert("name".to_string(), Value::String(name));
            }
            if let Some(color) = parse_opt(args, "--color") {
                params.insert("color".to_string(), Value::String(color));
            }
            let workspaces: Vec<Value> = args
                .windows(2)
                .filter(|pair| pair[0] == "--workspace")
                .map(|pair| Value::String(pair[1].clone()))
                .collect();
            if !workspaces.is_empty() {
                params.insert("workspace_ids".to_string(), Value::Array(workspaces));
            }
            "workspace.group_create"
        }
        "rename-group" => {
            params.insert("group_id".to_string(), Value::String(group()?));
            let name =
                trailing_title(args).ok_or_else(|| anyhow!("rename-group requires a name"))?;
            params.insert("name".to_string(), Value::String(name));
            "workspace.group_rename"
        }
        "set-group-color" => {
            params.insert("group_id".to_string(), Value::String(group()?));
            let color = trailing_title(args)
                .ok_or_else(|| anyhow!("set-group-color requires a color name"))?;
            params.insert("color".to_string(), Value::String(color));
            "workspace.group_set_color"
        }
        "collapse-group" | "expand-group" => {
            params.insert("group_id".to_string(), Value::String(group()?));
            params.insert(
                "collapsed".to_string(),
                Value::Bool(command == "collapse-group"),
            );
            "workspace.group_set_collapsed"
        }
        "delete-group" => {
            params.insert("group_id".to_string(), Value::String(group()?));
            "workspace.group_delete"
        }
        "set-workspace-group" => {
            let group = trailing_title(args).ok_or_else(|| {
                anyhow!("set-workspace-group requires a group, or \"none\" to ungroup")
            })?;
            let group = if group.eq_ignore_ascii_case("none") {
                Value::Null
            } else {
                Value::String(group)
            };
            params.insert("group_id".to_string(), group);
            if let Some(workspace) =
                parse_opt(args, "--workspace").or_else(|| env_value("LIMUX_WORKSPACE_ID"))
            {
                params.insert("workspace_id".to_string(), Value::String(workspace));
            }
            "workspace.set_group"
        }
        _ => bail!("unsupported group command {command}"),
    };
    Ok((method, Value::Object(params)))
}

/// The socket method and params of a `gui-hooks` subcommand.
fn build_gui_hooks_request(args: &[String]) -> Result<(&'static str, Value)> {
    const USAGE: &str = "gui-hooks list | set <event> [--command <text>] [--enable|--disable] [--show-terminal|--no-show-terminal] [--suppress-default|--no-suppress-default] | enable <event> | disable <event> | clear <event>, where <event> is on-workspace-create, after-workspace-create, on-workspace-delete or after-workspace-delete";
    let (subcommand, rest) = args
        .split_first()
        .ok_or_else(|| anyhow!("usage: limux {USAGE}"))?;
    let event = || {
        rest.first()
            .filter(|event| !event.starts_with('-'))
            .cloned()
            .ok_or_else(|| anyhow!("gui-hooks {subcommand} requires an event: on-workspace-create, after-workspace-create, on-workspace-delete or after-workspace-delete"))
    };
    let toggle = |on: &str, off: &str| -> Result<Option<bool>> {
        match (parse_flag(rest, on), parse_flag(rest, off)) {
            (true, true) => bail!("{on} and {off} cannot be combined"),
            (true, false) => Ok(Some(true)),
            (false, true) => Ok(Some(false)),
            (false, false) => Ok(None),
        }
    };
    match subcommand.as_str() {
        "list" => Ok(("gui_hook.list", json!({}))),
        "set" => {
            let mut params = Map::new();
            params.insert("event".to_string(), Value::String(event()?));
            if let Some(command) = parse_opt(rest, "--command") {
                params.insert("command".to_string(), Value::String(command));
            } else if parse_flag(rest, "--command") {
                bail!("--command requires the command to run");
            }
            for (key, on, off) in [
                ("enabled", "--enable", "--disable"),
                ("show_terminal", "--show-terminal", "--no-show-terminal"),
                (
                    "suppress_default",
                    "--suppress-default",
                    "--no-suppress-default",
                ),
            ] {
                if let Some(value) = toggle(on, off)? {
                    params.insert(key.to_string(), Value::Bool(value));
                }
            }
            if params.len() == 1 {
                bail!("gui-hooks set requires --command, --enable/--disable, --[no-]show-terminal or --[no-]suppress-default");
            }
            Ok(("gui_hook.set", Value::Object(params)))
        }
        "enable" | "disable" => Ok((
            "gui_hook.set",
            json!({ "event": event()?, "enabled": subcommand == "enable" }),
        )),
        "clear" => Ok(("gui_hook.clear", json!({ "event": event()? }))),
        other => bail!("unknown gui-hooks subcommand `{other}`; usage: limux {USAGE}"),
    }
}

fn render_gui_hook(hook: &Value) -> String {
    let event = get_string(hook, &["event"]).unwrap_or_default();
    if hook.get("configured").and_then(Value::as_bool) != Some(true) {
        return format!("{event}: not set");
    }
    let command = get_string(hook, &["command"]).unwrap_or_default();
    let flag = |key| hook.get(key).and_then(Value::as_bool) == Some(true);
    let mut notes = Vec::new();
    // Hosts from before the switch existed do not report it.
    if hook.get("enabled").and_then(Value::as_bool) == Some(false) {
        notes.push("disabled");
    }
    notes.push(if flag("show_terminal") {
        "in a terminal"
    } else {
        "in the background"
    });
    if flag("suppress_default") {
        notes.push("default action skipped");
    }
    format!("{event}: {command} ({})", notes.join(", "))
}

fn render_gui_hooks(payload: &Value) -> String {
    match payload.get("hooks").and_then(Value::as_array) {
        Some(hooks) => hooks
            .iter()
            .map(render_gui_hook)
            .collect::<Vec<_>>()
            .join("\n"),
        None => render_gui_hook(payload),
    }
}

fn render_group_list(payload: &Value) -> String {
    let groups = payload
        .get("groups")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if groups.is_empty() {
        return "No groups".to_string();
    }
    groups
        .iter()
        .map(|group| {
            let handle = get_string(group, &["ref", "id"]).unwrap_or_default();
            let name = get_string(group, &["name"]).unwrap_or_default();
            let color = get_string(group, &["color"]).unwrap_or_default();
            let count = group
                .get("workspace_ids")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let collapsed = if group.get("collapsed").and_then(Value::as_bool) == Some(true) {
                ", collapsed"
            } else {
                ""
            };
            format!(
                "{handle} {name} ({color}, {count} workspace{}{collapsed})",
                if count == 1 { "" } else { "s" }
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn run_rename_workspace_like(
    client: &mut Client,
    command: &str,
    args: &[String],
) -> Result<Value> {
    let workspace = parse_opt(args, "--workspace").or_else(|| env::var("LIMUX_WORKSPACE_ID").ok());
    let title = trailing_title(args).ok_or_else(|| {
        if command == "rename-window" {
            anyhow!("rename-window requires a title")
        } else {
            anyhow!("rename-workspace requires a title")
        }
    })?;

    let mut params = Map::new();
    params.insert("title".to_string(), Value::String(title));
    if let Some(workspace) = workspace {
        params.insert("workspace_id".to_string(), Value::String(workspace));
    }

    client.call("workspace.rename", Value::Object(params)).await
}

async fn run_rename_tab(client: &mut Client, args: &[String]) -> Result<Value> {
    let mut params = lifecycle_scope(args, Some(("--tab", "surface_id", "LIMUX_TAB_ID")))?;
    let title = trailing_title(args).ok_or_else(|| anyhow!("rename-tab requires a title"))?;

    params.insert("action".to_string(), Value::String("rename".to_string()));
    params.insert("title".to_string(), Value::String(title));

    client.call("tab.action", Value::Object(params)).await
}

async fn run_tab_action(client: &mut Client, args: &[String]) -> Result<Value> {
    if parse_flag(args, "--help") {
        return Ok(json!({
            "help": "Usage: limux tab-action --action <name> [--workspace <id|ref>] [--tab <id|ref>] [--title <text>] [--url <url>]\nLive GTK actions: rename, set-title, pin, unpin, select, activate, focus, close.\nTargets accept a raw tab ID, tab:<tab-id>, or surface:<pane-id>:<tab-id>. Omitted targets use the caller's tab or the selected workspace's focused tab.\nPass --title '' with rename to clear a custom name."
        }));
    }

    let action = parse_opt(args, "--action")
        .ok_or_else(|| anyhow!("tab-action requires --action <name>"))?;
    let mut params = lifecycle_scope(args, Some(("--tab", "surface_id", "LIMUX_TAB_ID")))?;
    let workspace = params
        .get("workspace_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let tab = params
        .get("surface_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let title = parse_opt(args, "--title").or_else(|| trailing_title(args));
    let url = parse_opt(args, "--url");

    if action == "new-terminal-right" || action == "new-browser-right" {
        let pane_type = if action == "new-browser-right" {
            "browser"
        } else {
            "terminal"
        };
        let mut params = vec![
            "--direction".to_string(),
            "right".to_string(),
            "--type".to_string(),
            pane_type.to_string(),
        ];
        if let Some(workspace) = workspace.clone() {
            params.push("--workspace".to_string());
            params.push(workspace);
        }
        if let Some(url) = url {
            params.push("--url".to_string());
            params.push(url);
        }
        let created = run_new_pane(client, &params).await?;
        let tab_ref = tab.unwrap_or_else(|| "tab:1".to_string());
        return Ok(json!({
            "tab_ref": tab_ref,
            "surface_id": created.get("surface_id").cloned().unwrap_or(Value::Null),
            "surface_ref": created.get("surface_ref").cloned().unwrap_or(Value::Null),
        }));
    }

    params.insert("action".to_string(), Value::String(action.clone()));
    if let Some(title) = title {
        params.insert("title".to_string(), Value::String(title));
    }

    let mut payload = client.call("tab.action", Value::Object(params)).await?;
    if let Some(obj) = payload.as_object_mut() {
        if !obj.contains_key("tab_ref") {
            obj.insert(
                "tab_ref".to_string(),
                Value::String(tab.unwrap_or_else(|| "tab:1".to_string())),
            );
        }
        if action == "pin" {
            obj.insert("pinned".to_string(), Value::Bool(true));
        }
        if action == "unpin" {
            obj.insert("pinned".to_string(), Value::Bool(false));
        }
    }
    Ok(payload)
}

fn ensure_placeholder_screenshot_file(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            bail!("refusing symlinked screenshot target {}", path.display());
        }
        Ok(_) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => {
            use std::os::unix::fs::OpenOptionsExt as _;

            if let Some(parent) = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                fs::create_dir_all(parent).with_context(|| {
                    format!("failed to create screenshot directory {}", parent.display())
                })?;
            }
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)
            {
                Ok(_) => Ok(()),
                Err(open_err) if open_err.kind() == ErrorKind::AlreadyExists => {
                    let meta = fs::symlink_metadata(path).with_context(|| {
                        format!("failed to inspect screenshot {}", path.display())
                    })?;
                    if meta.file_type().is_symlink() {
                        bail!("refusing symlinked screenshot target {}", path.display());
                    }
                    Ok(())
                }
                Err(open_err) => Err(open_err)
                    .with_context(|| format!("failed to create screenshot {}", path.display())),
            }
        }
        Err(err) => {
            Err(err).with_context(|| format!("failed to inspect screenshot {}", path.display()))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BrowserInvocation {
    browser_args: Vec<String>,
    local_json: bool,
    workspace: Option<String>,
    surface: Option<String>,
    subcommand: String,
    rest: Vec<String>,
}

fn is_browser_subcommand(token: &str) -> bool {
    matches!(
        token,
        "open"
            | "open-split"
            | "new"
            | "identify"
            | "url"
            | "get-url"
            | "goto"
            | "navigate"
            | "back"
            | "forward"
            | "reload"
            | "wait"
            | "snapshot"
            | "screenshot"
            | "find"
            | "frame"
            | "click"
            | "dblclick"
            | "hover"
            | "focus"
            | "focus-webview"
            | "is-webview-focused"
            | "check"
            | "uncheck"
            | "scroll-into-view"
            | "type"
            | "fill"
            | "press"
            | "keydown"
            | "keyup"
            | "select"
            | "scroll"
            | "eval"
            | "get"
            | "is"
            | "cookies"
            | "storage"
            | "tab"
            | "addscript"
            | "addinitscript"
            | "addstyle"
            | "console"
            | "errors"
            | "highlight"
            | "state"
            | "viewport"
            | "dialog"
            | "download"
            | "input-mouse"
            | "input-keyboard"
            | "input-touch"
    )
}

fn parse_browser_invocation(args: &[String], json_output: bool) -> Result<BrowserInvocation> {
    let mut browser_args = args.to_vec();
    let mut local_json = json_output;

    while browser_args.last().map(|s| s.as_str()) == Some("--json") {
        local_json = true;
        browser_args.pop();
    }

    let workspace = parse_opt(&browser_args, "--workspace");
    if let Some(ref ws) = workspace {
        if ws.trim().is_empty() {
            bail!("--workspace requires a non-empty target");
        }
    } else if parse_flag(&browser_args, "--workspace") {
        bail!("--workspace requires a non-empty target");
    }
    let mut surface = parse_terminal_surface(&browser_args)?;

    let mut positional: Vec<String> = Vec::new();
    let mut skip = false;
    for (idx, arg) in browser_args.iter().enumerate() {
        if skip {
            skip = false;
            continue;
        }
        match arg.as_str() {
            "--workspace" | "--surface" | "--id-format" | "--timeout-ms" | "--load-state"
            | "--out" | "--selector" | "--text" | "--attr" | "--property" | "--name"
            | "--value" => {
                if idx + 1 < browser_args.len() {
                    skip = true;
                }
            }
            value if value.starts_with('-') => {}
            _ => positional.push(arg.clone()),
        }
    }

    if positional.is_empty() {
        bail!("browser requires a subcommand");
    }

    let mut pos_idx = 0usize;
    let first = positional[0].clone();
    if surface.is_none()
        && !is_browser_subcommand(&first)
        && (first.contains(':') || first.contains('-'))
    {
        surface = Some(first);
        pos_idx = 1;
    }

    if pos_idx >= positional.len() {
        bail!("browser requires a subcommand");
    }

    Ok(BrowserInvocation {
        browser_args,
        local_json,
        workspace,
        surface,
        subcommand: positional[pos_idx].clone(),
        rest: positional[(pos_idx + 1)..].to_vec(),
    })
}

async fn run_browser(
    client: &mut Client,
    args: &[String],
    json_output: bool,
) -> Result<CommandOutput> {
    let BrowserInvocation {
        browser_args,
        local_json,
        workspace,
        surface,
        subcommand: sub,
        rest,
    } = parse_browser_invocation(args, json_output)?;
    let ws = workspace.as_deref();

    let output = match sub.as_str() {
        "open" | "open-split" | "new" => {
            let url = rest
                .first()
                .cloned()
                .unwrap_or_else(|| "about:blank".to_string());
            if let Some(surface) = surface.as_deref() {
                let payload = browser_call(client, ws, Some(surface), "browser.navigate", {
                    let mut p = Map::new();
                    p.insert("url".to_string(), Value::String(url));
                    p
                })
                .await?;
                CommandOutput::Json(payload)
            } else {
                let payload = call_in_workspace_scope(
                    client,
                    workspace.clone(),
                    "browser.open_split",
                    json!({ "url": url }),
                )
                .await?;
                CommandOutput::Json(payload)
            }
        }
        "url" | "get-url" => {
            let payload = browser_call(
                client,
                ws,
                surface.as_deref(),
                "browser.url.get",
                Map::new(),
            )
            .await?;
            if local_json {
                CommandOutput::Json(payload)
            } else {
                CommandOutput::Text(get_string(&payload, &["url"]).unwrap_or_default())
            }
        }
        "goto" | "navigate" => {
            let url = rest
                .first()
                .cloned()
                .ok_or_else(|| anyhow!("browser navigate requires a URL"))?;
            let payload = browser_call(client, ws, surface.as_deref(), "browser.navigate", {
                let mut p = Map::new();
                p.insert("url".to_string(), Value::String(url));
                p
            })
            .await?;
            if parse_flag(&browser_args, "--snapshot-after") {
                let sid = surface.as_deref().ok_or_else(|| {
                    anyhow!("browser navigate --snapshot-after requires a surface")
                })?;
                let snap =
                    browser_call(client, ws, Some(sid), "browser.snapshot", Map::new()).await?;
                if local_json {
                    let mut merged = payload;
                    if let Some(obj) = merged.as_object_mut() {
                        obj.insert("post_action_snapshot".to_string(), snap);
                    }
                    CommandOutput::Json(merged)
                } else {
                    CommandOutput::Text(
                        get_string(&snap, &["snapshot", "text"])
                            .unwrap_or_else(|| "OK".to_string()),
                    )
                }
            } else {
                CommandOutput::Json(payload)
            }
        }
        "back" | "forward" | "reload" => {
            let method = format!("browser.{}", sub);
            let payload = browser_call(client, ws, surface.as_deref(), &method, Map::new()).await?;
            CommandOutput::Json(payload)
        }
        "wait" => {
            let sid = surface
                .as_deref()
                .ok_or_else(|| anyhow!("browser wait requires a surface"))?;
            let mut p = Map::new();
            if let Some(selector) = parse_opt(&browser_args, "--selector") {
                p.insert("selector".to_string(), Value::String(selector));
            }
            if let Some(timeout_ms) = parse_opt(&browser_args, "--timeout-ms") {
                if let Ok(ms) = timeout_ms.parse::<u64>() {
                    p.insert("timeout_ms".to_string(), Value::Number(ms.into()));
                }
            }
            let payload = browser_call(client, ws, Some(sid), "browser.wait", p).await?;
            if local_json {
                CommandOutput::Json(payload)
            } else {
                CommandOutput::Text("OK".to_string())
            }
        }
        "snapshot" => {
            let sid = surface
                .as_deref()
                .ok_or_else(|| anyhow!("browser snapshot requires a surface"))?;
            let payload =
                browser_call(client, ws, Some(sid), "browser.snapshot", Map::new()).await?;
            if local_json {
                CommandOutput::Json(payload)
            } else {
                let url = get_string(&payload, &["url"]).unwrap_or_default();
                if parse_flag(&browser_args, "--interactive") && url == "about:blank" {
                    CommandOutput::Text("about:blank\nNo interactive elements found; try `browser <surface> get url`.".to_string())
                } else if parse_flag(&browser_args, "--interactive") {
                    let mut text = get_string(&payload, &["snapshot", "text"])
                        .unwrap_or_else(|| "OK".to_string());
                    if let Some(refs) = payload.get("refs").and_then(Value::as_object) {
                        for key in refs.keys() {
                            text.push_str(&format!("\nref={}", key));
                        }
                    }
                    CommandOutput::Text(text)
                } else {
                    CommandOutput::Text(
                        get_string(&payload, &["snapshot", "text"])
                            .unwrap_or_else(|| "OK".to_string()),
                    )
                }
            }
        }
        "screenshot" => {
            let sid = surface
                .as_deref()
                .ok_or_else(|| anyhow!("browser screenshot requires a surface"))?;
            let mut payload =
                browser_call(client, ws, Some(sid), "browser.screenshot", Map::new()).await?;
            let out = parse_opt(&browser_args, "--out");
            let mut path = match get_string(&payload, &["path"]) {
                Some(path) => path,
                None => limux_control::socket_path::ensure_private_runtime_dir()
                    .context("failed to prepare private screenshot directory")?
                    .join("limux-browser-shot.png")
                    .to_string_lossy()
                    .into_owned(),
            };
            if let Some(out_path) = out {
                path = out_path;
            }
            ensure_placeholder_screenshot_file(Path::new(&path))?;
            let url = format!("file://{}", path);
            if let Some(obj) = payload.as_object_mut() {
                obj.insert("path".to_string(), Value::String(path.clone()));
                obj.insert("url".to_string(), Value::String(url.clone()));
                obj.remove("png_base64");
            }
            if parse_opt(&browser_args, "--out").is_some() {
                CommandOutput::Text(format!("OK {}", path))
            } else if local_json {
                CommandOutput::Json(payload)
            } else {
                CommandOutput::Text(path)
            }
        }
        "find" => {
            let sid = surface
                .as_deref()
                .ok_or_else(|| anyhow!("browser find requires a surface"))?;
            let locator = rest.first().cloned().unwrap_or_else(|| "text".to_string());
            let value = rest.get(1).cloned().unwrap_or_default();
            let method = format!("browser.find.{}", locator);
            let mut params = Map::new();
            match locator.as_str() {
                "role" => {
                    params.insert("role".to_string(), Value::String(value));
                }
                "nth" => {
                    params.insert(
                        "selector".to_string(),
                        Value::String(rest.get(1).cloned().unwrap_or_default()),
                    );
                    let index = rest.get(2).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
                    params.insert("index".to_string(), Value::Number(index.into()));
                }
                "first" | "last" => {
                    params.insert("selector".to_string(), Value::String(value));
                }
                _ => {
                    params.insert(locator.clone(), Value::String(value));
                }
            }
            let payload = browser_call(client, ws, Some(sid), &method, params).await?;
            if local_json {
                CommandOutput::Json(payload)
            } else {
                CommandOutput::Text(
                    get_string(&payload, &["element_ref"]).unwrap_or_else(|| "@e1".to_string()),
                )
            }
        }
        "frame" => {
            let sid = surface
                .as_deref()
                .ok_or_else(|| anyhow!("browser frame requires a surface"))?;
            let target = rest.first().cloned().unwrap_or_else(|| "main".to_string());
            let payload = if target == "main" {
                browser_call(client, ws, Some(sid), "browser.frame.main", Map::new()).await?
            } else {
                browser_call(client, ws, Some(sid), "browser.frame.select", {
                    let mut p = Map::new();
                    p.insert("selector".to_string(), Value::String(target));
                    p
                })
                .await?
            };
            CommandOutput::Json(payload)
        }
        "click" => {
            let sid = surface
                .as_deref()
                .ok_or_else(|| anyhow!("browser click requires a surface"))?;
            let selector = parse_opt(&browser_args, "--selector")
                .or_else(|| rest.first().cloned())
                .ok_or_else(|| anyhow!("browser click requires a selector"))?;
            let payload = browser_call(client, ws, Some(sid), "browser.click", {
                let mut p = Map::new();
                p.insert("selector".to_string(), Value::String(selector));
                p
            })
            .await?;
            CommandOutput::Json(payload)
        }
        "fill" => {
            let sid = surface
                .as_deref()
                .ok_or_else(|| anyhow!("browser fill requires a surface"))?;
            let selector = parse_opt(&browser_args, "--selector")
                .or_else(|| rest.first().cloned())
                .unwrap_or_default();
            let text = parse_opt(&browser_args, "--text")
                .or_else(|| rest.get(1).cloned())
                .unwrap_or_default();
            let payload = browser_call(client, ws, Some(sid), "browser.fill", {
                let mut p = Map::new();
                p.insert("selector".to_string(), Value::String(selector));
                p.insert("text".to_string(), Value::String(text));
                p
            })
            .await?;
            if parse_flag(&browser_args, "--snapshot-after") {
                let snap =
                    browser_call(client, ws, Some(sid), "browser.snapshot", Map::new()).await?;
                if local_json {
                    let mut merged = payload;
                    if let Some(obj) = merged.as_object_mut() {
                        obj.insert("post_action_snapshot".to_string(), snap);
                    }
                    CommandOutput::Json(merged)
                } else {
                    CommandOutput::Text(
                        get_string(&snap, &["snapshot", "text"])
                            .unwrap_or_else(|| "OK".to_string()),
                    )
                }
            } else {
                CommandOutput::Json(payload)
            }
        }
        "get" => {
            let sid = surface
                .as_deref()
                .ok_or_else(|| anyhow!("browser get requires a surface"))?;
            let get_verb = rest.first().cloned().unwrap_or_else(|| "url".to_string());
            let method = match get_verb.as_str() {
                "url" => "browser.url.get".to_string(),
                "title" => "browser.get.title".to_string(),
                "text" => "browser.get.text".to_string(),
                "html" => "browser.get.html".to_string(),
                "value" => "browser.get.value".to_string(),
                "attr" => "browser.get.attr".to_string(),
                "count" => "browser.get.count".to_string(),
                "box" => "browser.get.box".to_string(),
                "styles" => "browser.get.styles".to_string(),
                other => bail!("Unsupported browser get subcommand: {}", other),
            };
            let selector = rest
                .get(1)
                .cloned()
                .or_else(|| parse_opt(&browser_args, "--selector"));
            let mut p = Map::new();
            if let Some(selector) = selector {
                p.insert("selector".to_string(), Value::String(selector));
            }
            if let Some(attr) = parse_opt(&browser_args, "--attr") {
                p.insert("name".to_string(), Value::String(attr));
            }
            if let Some(property) = parse_opt(&browser_args, "--property") {
                p.insert("property".to_string(), Value::String(property));
            }
            let payload = browser_call(client, ws, Some(sid), &method, p).await?;
            if local_json {
                CommandOutput::Json(payload)
            } else {
                let text = get_string(&payload, &["url", "title", "text", "value", "html"])
                    .unwrap_or_else(|| "OK".to_string());
                CommandOutput::Text(text)
            }
        }
        "cookies" => {
            let sid = surface
                .as_deref()
                .ok_or_else(|| anyhow!("browser cookies requires a surface"))?;
            let op = rest.first().cloned().unwrap_or_else(|| "get".to_string());
            let method = match op.as_str() {
                "get" => "browser.cookies.get",
                "set" => "browser.cookies.set",
                "clear" => "browser.cookies.clear",
                _ => bail!("Unsupported browser cookies subcommand: {}", op),
            };
            let mut p = Map::new();
            if let Some(name) = rest
                .get(1)
                .cloned()
                .or_else(|| parse_opt(&browser_args, "--name"))
            {
                p.insert("name".to_string(), Value::String(name));
            }
            if let Some(value) = rest
                .get(2)
                .cloned()
                .or_else(|| parse_opt(&browser_args, "--value"))
            {
                p.insert("value".to_string(), Value::String(value));
            }
            let payload = browser_call(client, ws, Some(sid), method, p).await?;
            CommandOutput::Json(payload)
        }
        "storage" => {
            let sid = surface
                .as_deref()
                .ok_or_else(|| anyhow!("browser storage requires a surface"))?;
            if rest.len() < 2 {
                bail!("browser storage requires <local|session> <get|set|clear>");
            }
            let storage_type = rest[0].clone();
            let op = rest[1].clone();
            let method = match op.as_str() {
                "get" => "browser.storage.get",
                "set" => "browser.storage.set",
                "clear" => "browser.storage.clear",
                _ => bail!("Unsupported browser storage subcommand: {}", op),
            };
            let mut p = Map::new();
            p.insert("type".to_string(), Value::String(storage_type));
            if let Some(key) = rest.get(2) {
                p.insert("key".to_string(), Value::String(key.clone()));
            }
            if let Some(value) = rest.get(3) {
                p.insert("value".to_string(), Value::String(value.clone()));
            }
            let payload = browser_call(client, ws, Some(sid), method, p).await?;
            CommandOutput::Json(payload)
        }
        "tab" => {
            let sid = surface
                .as_deref()
                .ok_or_else(|| anyhow!("browser tab requires a surface"))?;
            let tab_verb = rest.first().cloned().unwrap_or_else(|| "list".to_string());
            let (method, p) = match tab_verb.as_str() {
                "list" => ("browser.tab.list", Map::new()),
                "new" => {
                    let mut p = Map::new();
                    if let Some(url) = rest.get(1) {
                        p.insert("url".to_string(), Value::String(url.clone()));
                    }
                    ("browser.tab.new", p)
                }
                "switch" => {
                    let mut p = Map::new();
                    if let Some(target) = rest.get(1) {
                        p.insert(
                            "target_surface_id".to_string(),
                            Value::String(target.clone()),
                        );
                    }
                    ("browser.tab.switch", p)
                }
                "close" => {
                    let mut p = Map::new();
                    if let Some(target) = rest.get(1) {
                        p.insert(
                            "target_surface_id".to_string(),
                            Value::String(target.clone()),
                        );
                    }
                    ("browser.tab.close", p)
                }
                _ => bail!("Unsupported browser tab subcommand: {}", tab_verb),
            };
            let payload = browser_call(client, ws, Some(sid), method, p).await?;
            CommandOutput::Json(payload)
        }
        "addscript" | "addinitscript" | "addstyle" => {
            let sid = surface
                .as_deref()
                .ok_or_else(|| anyhow!("browser {} requires a surface", sub))?;
            let content = rest.join(" ");
            if content.trim().is_empty() {
                bail!("browser {} requires content", sub);
            }
            let field = if sub == "addstyle" { "css" } else { "script" };
            let method = format!("browser.{}", sub);
            let mut p = Map::new();
            p.insert(field.to_string(), Value::String(content));
            let payload = browser_call(client, ws, Some(sid), &method, p).await?;
            CommandOutput::Json(payload)
        }
        "console" | "errors" => {
            let sid = surface
                .as_deref()
                .ok_or_else(|| anyhow!("browser {} requires a surface", sub))?;
            let op = rest.first().cloned().unwrap_or_else(|| "list".to_string());
            let method = format!("browser.{}.{}", sub, op);
            let payload = browser_call(client, ws, Some(sid), &method, Map::new()).await?;
            CommandOutput::Json(payload)
        }
        "highlight" => {
            let sid = surface
                .as_deref()
                .ok_or_else(|| anyhow!("browser highlight requires a surface"))?;
            let selector = rest.first().cloned().unwrap_or_default();
            let payload = browser_call(client, ws, Some(sid), "browser.highlight", {
                let mut p = Map::new();
                p.insert("selector".to_string(), Value::String(selector));
                p
            })
            .await?;
            CommandOutput::Json(payload)
        }
        "state" => {
            let sid = surface
                .as_deref()
                .ok_or_else(|| anyhow!("browser state requires a surface"))?;
            let op = rest.first().cloned().unwrap_or_else(|| "save".to_string());
            let path = rest
                .get(1)
                .cloned()
                .ok_or_else(|| anyhow!("browser state {} requires a file path", op))?;
            let method = match op.as_str() {
                "save" => "browser.state.save",
                "load" => "browser.state.load",
                _ => bail!("Unsupported browser state subcommand: {}", op),
            };
            let payload = browser_call(client, ws, Some(sid), method, {
                let mut p = Map::new();
                p.insert("path".to_string(), Value::String(path));
                p
            })
            .await?;
            CommandOutput::Json(payload)
        }
        "viewport" => {
            bail!("not_supported: browser viewport is not supported in linux mock");
        }
        _ => {
            // Generic passthrough to browser.<sub>
            let sid = surface
                .as_deref()
                .ok_or_else(|| anyhow!("browser {} requires a surface", sub))?;
            let method = format!("browser.{}", sub.replace('-', "_"));
            let payload = browser_call(client, ws, Some(sid), &method, Map::new()).await?;
            CommandOutput::Json(payload)
        }
    };

    Ok(output)
}

fn is_unsupported_tmux_cmd(cmd: &str) -> bool {
    matches!(cmd, "popup" | "bind-key" | "unbind-key" | "copy-mode")
}

fn is_gui_hook_event_name(name: &str) -> bool {
    matches!(
        name,
        "on-workspace-create"
            | "after-workspace-create"
            | "on-workspace-delete"
            | "after-workspace-delete"
    )
}

fn parse_set_hook_options(args: &[String]) -> (bool, Option<String>) {
    let mut list_mode = false;
    let mut unset = None;
    let mut idx = 0;
    while idx < args.len() {
        let arg = &args[idx];
        if arg == "--" {
            break;
        }
        if arg == "--list" {
            list_mode = true;
            idx += 1;
            continue;
        }
        if arg == "--unset" {
            unset = args.get(idx + 1).cloned();
            idx += 2;
            continue;
        }
        if let Some(val) = arg.strip_prefix("--unset=") {
            unset = Some(val.to_string());
            idx += 1;
            continue;
        }
        if matches!(
            arg.as_str(),
            "--workspace" | "--surface" | "--pane" | "--tab" | "-t"
        ) {
            idx += 2;
            continue;
        }
        if arg.starts_with('-') {
            idx += 1;
            continue;
        }
        break;
    }
    (list_mode, unset)
}

fn parse_set_hook_positional(args: &[String]) -> Result<(String, String)> {
    let mut idx = 0;
    let mut name: Option<String> = None;
    let mut options_ended = false;
    while idx < args.len() {
        let arg = &args[idx];
        if !options_ended {
            if arg == "--" {
                options_ended = true;
                idx += 1;
                continue;
            }
            if matches!(
                arg.as_str(),
                "--workspace" | "--unset" | "--surface" | "--pane" | "--tab" | "-t"
            ) {
                idx += 2;
                continue;
            }
            if arg == "--list" || arg.starts_with('-') {
                idx += 1;
                continue;
            }
        }
        name = Some(arg.clone());
        idx += 1;
        break;
    }
    let Some(name) = name else {
        bail!("set-hook requires <name> <command>");
    };
    if !options_ended && args.get(idx).map(String::as_str) == Some("--") {
        idx += 1;
    }
    let body = args.get(idx..).unwrap_or_default().join(" ");
    if name.trim().is_empty() || body.trim().is_empty() {
        bail!("set-hook requires <name> <command>");
    }
    Ok((name, body))
}

async fn run_tmux_compat(client: &mut Client, command: &str, args: &[String]) -> Result<Value> {
    if is_unsupported_tmux_cmd(command) {
        bail!("not supported");
    }

    match command {
        "capture-pane" => run_read_screen(client, args).await,
        "pipe-pane" => {
            let capture = run_read_screen(client, args).await?;
            let text = get_string(&capture, &["text"]).unwrap_or_default();
            let shell_cmd = parse_opt(args, "--command")
                .ok_or_else(|| anyhow!("pipe-pane requires --command"))?;
            let mut child = Command::new("bash")
                .arg("-lc")
                .arg(shell_cmd)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .context("failed to spawn pipe-pane command")?;
            if let Some(stdin) = child.stdin.as_mut() {
                use std::io::Write;
                stdin
                    .write_all(text.as_bytes())
                    .context("failed to write pipe-pane stdin")?;
            }
            let status = child
                .wait()
                .context("failed waiting for pipe-pane command")?;
            if !status.success() {
                bail!("pipe-pane command failed");
            }
            Ok(json!({"ok": true}))
        }
        "wait-for" => {
            let signal = parse_flag(args, "-S") || parse_flag(args, "--signal");
            let name = trailing_title(args).ok_or_else(|| anyhow!("wait-for requires a name"))?;
            let timeout = parse_opt(args, "--timeout")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(5);
            let path = wait_signal_path(&name)?;
            if signal {
                use std::io::Write as _;
                use std::os::unix::fs::OpenOptionsExt as _;

                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(0o600)
                    .open(&path)
                    .context("failed to create wait-for signal")?;
                file.write_all(b"1")
                    .context("failed to write wait-for signal")?;
                Ok(json!({"ok": true, "name": name}))
            } else {
                let deadline = Instant::now() + Duration::from_secs(timeout);
                loop {
                    if path.exists() {
                        let _ = fs::remove_file(&path);
                        return Ok(json!({"ok": true, "name": name}));
                    }
                    if Instant::now() >= deadline {
                        bail!("wait-for timed out waiting for '{}'", name);
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
        "find-window" => {
            let needle = trailing_title(args).unwrap_or_default();
            let listed = client.call("workspace.list", json!({})).await?;
            let rows = listed
                .get("workspaces")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let mut out = String::new();
            for row in rows {
                let title = get_string(&row, &["title", "name"]).unwrap_or_default();
                if title.contains(&needle) {
                    let handle = handle_from_payload(&row, "workspace_id", "workspace_ref");
                    out = format!("{} {}", handle, title);
                    break;
                }
            }
            Ok(json!({"text": out}))
        }
        "last-window" => client.call("workspace.last", json!({})).await,
        "next-window" => client.call("workspace.next", json!({})).await,
        "previous-window" => client.call("workspace.previous", json!({})).await,
        "swap-pane" => {
            let workspace = parse_opt(args, "--workspace");
            let pane =
                parse_opt(args, "--pane").ok_or_else(|| anyhow!("swap-pane requires --pane"))?;
            let target = parse_opt(args, "--target-pane")
                .ok_or_else(|| anyhow!("swap-pane requires --target-pane"))?;

            call_in_workspace_scope(
                client,
                workspace,
                "pane.swap",
                json!({"pane_id": pane, "target_pane_id": target}),
            )
            .await
        }
        "break-pane" => {
            let workspace = parse_opt(args, "--workspace");
            let pane = parse_opt(args, "--pane");
            let surface = parse_opt(args, "--surface");
            let mut p = Map::new();
            if let Some(pane) = pane {
                p.insert("pane_id".to_string(), Value::String(pane));
            }
            if let Some(surface) = surface {
                p.insert("surface_id".to_string(), Value::String(surface));
            }
            call_in_workspace_scope(client, workspace, "pane.break", Value::Object(p)).await
        }
        "join-pane" => {
            let workspace = parse_opt(args, "--workspace");
            let pane = parse_opt(args, "--pane");
            let surface = parse_opt(args, "--surface");
            let target = parse_opt(args, "--target-pane")
                .ok_or_else(|| anyhow!("join-pane requires --target-pane"))?;
            let mut p = Map::new();
            p.insert("target_pane_id".to_string(), Value::String(target));
            if let Some(pane) = pane {
                p.insert("pane_id".to_string(), Value::String(pane));
            }
            if let Some(surface) = surface {
                p.insert("surface_id".to_string(), Value::String(surface));
            }
            call_in_workspace_scope(client, workspace, "pane.join", Value::Object(p)).await
        }
        "last-pane" => {
            let workspace = parse_opt(args, "--workspace");
            call_in_workspace_scope(client, workspace, "pane.last", json!({})).await
        }
        "clear-history" => {
            let workspace = parse_opt(args, "--workspace");
            let surface = parse_opt(args, "--surface");
            let mut p = Map::new();
            if let Some(surface) = surface {
                p.insert("surface_id".to_string(), Value::String(surface));
            }
            call_in_workspace_scope(client, workspace, "surface.clear_history", Value::Object(p))
                .await
        }
        "set-hook" => {
            let (list_mode, unset) = parse_set_hook_options(args);
            if list_mode {
                let gui_entries = client
                    .call("gui_hook.list", json!({}))
                    .await
                    .ok()
                    .and_then(|payload| {
                        payload.get("hooks").and_then(Value::as_array).map(|hooks| {
                            hooks
                                .iter()
                                .filter(|hook| {
                                    hook.get("configured").and_then(Value::as_bool) == Some(true)
                                })
                                .filter_map(|hook| {
                                    let event = get_string(hook, &["event"])?;
                                    let command = get_string(hook, &["command"])?;
                                    Some((event, command))
                                })
                                .collect::<Vec<_>>()
                        })
                    })
                    .unwrap_or_default();
                return with_locked_json_map(&client.socket, "hooks", |hooks, path| {
                    let mut merged = hooks.clone();
                    for (event, command) in gui_entries {
                        merged.insert(event, command);
                    }
                    let text = merged
                        .iter()
                        .map(|(k, v)| format!("{}={}", k, v))
                        .collect::<Vec<_>>()
                        .join("\n");
                    Ok(json!({
                        "text": text,
                        "path": path.display().to_string(),
                    }))
                });
            }
            if let Some(name) = unset {
                if is_gui_hook_event_name(&name)
                    && client
                        .call("gui_hook.clear", json!({ "event": name }))
                        .await
                        .is_ok()
                {
                    let _ = with_locked_json_map(&client.socket, "hooks", |hooks, path| {
                        if hooks.remove(&name).is_some() {
                            write_json_map(path, hooks)?;
                        }
                        Ok(json!({"ok": true}))
                    });
                    return Ok(json!({"ok": true}));
                }
                return with_locked_json_map(&client.socket, "hooks", |hooks, path| {
                    hooks.remove(&name);
                    write_json_map(path, hooks)?;
                    Ok(json!({"ok": true}))
                });
            }
            let (name, body) = parse_set_hook_positional(args)?;
            if is_gui_hook_event_name(&name)
                && client
                    .call(
                        "gui_hook.set",
                        json!({ "event": name, "command": body, "enabled": true }),
                    )
                    .await
                    .is_ok()
            {
                return Ok(json!({"ok": true}));
            }
            with_locked_json_map(&client.socket, "hooks", |hooks, path| {
                hooks.insert(name, body);
                write_json_map(path, hooks)?;
                Ok(json!({"ok": true}))
            })
        }
        "resize-pane" => {
            let workspace = parse_opt(args, "--workspace");
            let pane =
                parse_opt(args, "--pane").ok_or_else(|| anyhow!("resize-pane requires --pane"))?;
            let direction = if parse_flag(args, "-R") {
                "right"
            } else if parse_flag(args, "-L") {
                "left"
            } else if parse_flag(args, "-D") {
                "down"
            } else if parse_flag(args, "-U") {
                "up"
            } else {
                "right"
            };
            let amount = parse_opt(args, "--amount")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(1);
            call_in_workspace_scope(
                client,
                workspace,
                "pane.resize",
                json!({"pane_id": pane, "direction": direction, "amount": amount}),
            )
            .await
        }
        "set-buffer" => {
            let name =
                parse_opt(args, "--name").ok_or_else(|| anyhow!("set-buffer requires --name"))?;
            let body = trailing_title(args).unwrap_or_default();
            with_locked_json_map(&client.socket, "buffers", |buffers, path| {
                buffers.insert(name, body);
                write_json_map(path, buffers)?;
                Ok(json!({"ok": true}))
            })
        }
        "list-buffers" => with_locked_json_map(&client.socket, "buffers", |buffers, _path| {
            let text = buffers.keys().cloned().collect::<Vec<_>>().join("\n");
            Ok(json!({"text": text}))
        }),
        "paste-buffer" => {
            let name =
                parse_opt(args, "--name").ok_or_else(|| anyhow!("paste-buffer requires --name"))?;
            let workspace = parse_opt(args, "--workspace");
            let surface = parse_opt(args, "--surface");
            let text = with_locked_json_map(&client.socket, "buffers", |buffers, _path| {
                Ok(buffers.get(&name).cloned().unwrap_or_default())
            })?;
            let mut p = Map::new();
            if let Some(surface) = surface {
                p.insert("surface_id".to_string(), Value::String(surface));
            }
            p.insert("text".to_string(), Value::String(text));
            call_in_workspace_scope(client, workspace, "surface.send_text", Value::Object(p)).await
        }
        "respawn-pane" => {
            let workspace = parse_opt(args, "--workspace");
            let surface = parse_opt(args, "--surface");
            let command = parse_opt(args, "--command").unwrap_or_default();
            let mut p = Map::new();
            if let Some(surface) = surface {
                p.insert("surface_id".to_string(), Value::String(surface));
            }
            p.insert("text".to_string(), Value::String(format!("{}\n", command)));
            call_in_workspace_scope(client, workspace, "surface.send_text", Value::Object(p)).await
        }
        "display-message" => {
            let msg = trailing_title(args).unwrap_or_default();
            Ok(json!({"text": msg}))
        }
        _ => bail!("unknown tmux command"),
    }
}

async fn execute_command(client: &mut Client, opts: &GlobalOptions) -> Result<CommandOutput> {
    if let Some(raw_request) = &opts.request {
        let request: V2Request =
            serde_json::from_str(raw_request).context("request must be a valid v2 JSON object")?;
        let mut payload = client.send_request(request).await?;
        apply_id_format(&mut payload, opts.id_format);
        return Ok(CommandOutput::Json(payload));
    }

    if opts.command_args.is_empty() {
        print_help();
        bail!("missing command");
    }

    let command = opts.command_args[0].as_str();
    let args = &opts.command_args[1..];
    let mut effective_id_format = opts.id_format;
    if command == "browser" {
        if let Some(raw) = parse_opt(args, "--id-format") {
            effective_id_format = IdFormat::parse(&raw)?;
        }
    }

    let mut out = match command {
        "--version" | "-V" => CommandOutput::Text(format!("limux {}", env!("CARGO_PKG_VERSION"))),
        "activate" => {
            let payload = run_activate(client, args).await?;
            if opts.json_output {
                CommandOutput::Json(payload)
            } else {
                CommandOutput::Text("OK".to_string())
            }
        }
        "save-session" => {
            let payload = run_save_session(client, args).await?;
            if opts.json_output {
                CommandOutput::Json(payload)
            } else if let Some(path) = payload.get("path").and_then(Value::as_str) {
                let workspaces = payload
                    .get("workspaces")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let tabs = payload.get("tabs").and_then(Value::as_u64).unwrap_or(0);
                let agents = payload
                    .get("restorable_agents")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                CommandOutput::Text(format!(
                    "OK saved {path} ({workspaces} workspaces, {tabs} tabs, {agents} restorable agents)"
                ))
            } else {
                CommandOutput::Text("OK".to_string())
            }
        }
        "identify" => CommandOutput::Json(run_identify(client, args).await?),
        "list-panels" | "list-panes" | "list-workspaces" | "surface-health" => {
            let payload = run_list(client, command, args).await?;
            if opts.json_output {
                CommandOutput::Json(payload)
            } else {
                CommandOutput::Text(render_list_text(command, &payload))
            }
        }
        "send" => {
            let payload = run_send(client, args).await?;
            if opts.json_output {
                CommandOutput::Json(payload)
            } else {
                let handle = handle_from_payload(&payload, "surface_id", "surface_ref");
                CommandOutput::Text(format!("OK {}", handle.trim()))
            }
        }
        "send-key" => {
            let payload = run_send_key(client, args).await?;
            if opts.json_output {
                CommandOutput::Json(payload)
            } else {
                let handle = handle_from_payload(&payload, "surface_id", "surface_ref");
                CommandOutput::Text(format!("OK {}", handle.trim()))
            }
        }
        "notify" => {
            let payload = run_notify(client, args).await?;
            if opts.json_output {
                CommandOutput::Json(payload)
            } else {
                CommandOutput::Text("OK".to_string())
            }
        }
        "claude-hook" | "opencode-hook" | "gemini-hook" | "agy-hook" => {
            let agent = match command {
                "claude-hook" => agent_hooks::AgentKind::Claude,
                "opencode-hook" => agent_hooks::AgentKind::OpenCode,
                "gemini-hook" => agent_hooks::AgentKind::Gemini,
                "agy-hook" => agent_hooks::AgentKind::Agy,
                _ => unreachable!(),
            };
            let payload = run_agent_hook(client, agent, args).await?;
            if opts.json_output {
                CommandOutput::Json(payload)
            } else {
                CommandOutput::Text("OK".to_string())
            }
        }
        "hooks" => return run_hooks_command(client, args, opts.json_output).await,
        "new-workspace" => {
            let payload = run_new_workspace(client, args).await?;
            if opts.json_output {
                CommandOutput::Json(payload)
            } else {
                let handle = handle_from_payload(&payload, "workspace_id", "workspace_ref");
                CommandOutput::Text(format!("OK {}", handle))
            }
        }
        "close-surface" => {
            let payload = run_close_surface(client, args).await?;
            if opts.json_output {
                CommandOutput::Json(payload)
            } else {
                CommandOutput::Text("OK".to_string())
            }
        }
        "focus-surface" => {
            let payload = run_focus_surface(client, args).await?;
            if opts.json_output {
                CommandOutput::Json(payload)
            } else {
                CommandOutput::Text("OK".to_string())
            }
        }
        "focus-pane" => {
            let payload = run_focus_pane(client, args).await?;
            if opts.json_output {
                CommandOutput::Json(payload)
            } else {
                CommandOutput::Text("OK".to_string())
            }
        }
        "select-workspace" => {
            let payload = run_select_workspace(client, args).await?;
            if opts.json_output {
                CommandOutput::Json(payload)
            } else {
                CommandOutput::Text("OK".to_string())
            }
        }
        "close-workspace" => {
            let payload = run_close_workspace(client, args).await?;
            if opts.json_output {
                CommandOutput::Json(payload)
            } else {
                CommandOutput::Text("OK".to_string())
            }
        }
        "agent-team" => {
            let payload = run_agent_team(client, args).await?;
            if opts.json_output {
                CommandOutput::Json(payload)
            } else {
                let agents_md = payload
                    .get("agents_md")
                    .and_then(|v| v.as_str())
                    .unwrap_or("AGENTS.md");
                let workspace = payload
                    .get("workspace_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let peers = payload
                    .get("peers")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|p| p.get("agent").and_then(|v| v.as_str()))
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                CommandOutput::Text(format!(
                    "OK agent-team workspace={workspace} peers=[{peers}] agents_md={agents_md}"
                ))
            }
        }
        "sidebar-state" => {
            let payload = run_sidebar_state(client, args).await?;
            if opts.json_output {
                CommandOutput::Json(payload)
            } else {
                let workspace =
                    get_string(&payload, &["workspace"]).unwrap_or_else(|| "none".to_string());
                let cwd = get_string(&payload, &["cwd"]).unwrap_or_else(|| "none".to_string());
                let git_branch =
                    get_string(&payload, &["git_branch"]).unwrap_or_else(|| "none".to_string());
                CommandOutput::Text(format!(
                    "workspace={}\ncwd={}\ngit_branch={}",
                    workspace, cwd, git_branch
                ))
            }
        }
        "new-surface" => {
            let payload = run_new_surface(client, args).await?;
            if opts.json_output {
                CommandOutput::Json(payload)
            } else {
                let handle = handle_from_payload(&payload, "surface_id", "surface_ref");
                CommandOutput::Text(format!("OK {}", handle))
            }
        }
        "new-pane" => {
            let payload = run_new_pane(client, args).await?;
            if opts.json_output {
                CommandOutput::Json(payload)
            } else {
                let handle = handle_from_payload(&payload, "surface_id", "surface_ref");
                CommandOutput::Text(format!("OK {}", handle))
            }
        }
        "tab-action" => {
            let payload = run_tab_action(client, args).await?;
            if opts.json_output {
                CommandOutput::Json(payload)
            } else if let Some(help) = get_string(&payload, &["help"]) {
                CommandOutput::Text(help)
            } else {
                CommandOutput::Text("OK".to_string())
            }
        }
        "rename-workspace" | "rename-window" => {
            let payload = run_rename_workspace_like(client, command, args).await?;
            if opts.json_output {
                CommandOutput::Json(payload)
            } else {
                CommandOutput::Text("OK".to_string())
            }
        }
        "set-workspace-color" => {
            let params = build_set_workspace_color_params(args, |name| env::var(name).ok())?;
            let payload = client.call("workspace.set_color", params).await?;
            if opts.json_output {
                CommandOutput::Json(payload)
            } else {
                CommandOutput::Text("OK".to_string())
            }
        }
        "list-groups"
        | "create-group"
        | "rename-group"
        | "set-group-color"
        | "collapse-group"
        | "expand-group"
        | "delete-group"
        | "set-workspace-group" => {
            let (method, params) = build_group_request(command, args, |name| env::var(name).ok())?;
            let payload = client.call(method, params).await?;
            if opts.json_output {
                CommandOutput::Json(payload)
            } else if command == "list-groups" {
                CommandOutput::Text(render_group_list(&payload))
            } else {
                CommandOutput::Text("OK".to_string())
            }
        }
        "gui-hooks" => {
            let (method, params) = build_gui_hooks_request(args)?;
            let payload = client.call(method, params).await?;
            if opts.json_output {
                CommandOutput::Json(payload)
            } else {
                CommandOutput::Text(render_gui_hooks(&payload))
            }
        }
        "rename-tab" => {
            let payload = run_rename_tab(client, args).await?;
            if opts.json_output {
                CommandOutput::Json(payload)
            } else {
                CommandOutput::Text("OK".to_string())
            }
        }
        "read-screen" | "capture-pane" => {
            let payload = run_read_screen(client, args).await?;
            if opts.json_output {
                CommandOutput::Json(payload)
            } else {
                CommandOutput::Text(get_string(&payload, &["text"]).unwrap_or_default())
            }
        }
        "browser" => run_browser(client, args, opts.json_output).await?,
        "open-browser" => {
            let mut bridged = vec!["open".to_string()];
            bridged.extend(args.iter().cloned());
            run_browser(client, &bridged, opts.json_output).await?
        }
        "navigate-browser" => {
            let mut bridged = vec!["navigate".to_string()];
            bridged.extend(args.iter().cloned());
            run_browser(client, &bridged, opts.json_output).await?
        }
        "browser-back" => {
            let mut bridged = vec!["back".to_string()];
            bridged.extend(args.iter().cloned());
            run_browser(client, &bridged, opts.json_output).await?
        }
        "browser-forward" => {
            let mut bridged = vec!["forward".to_string()];
            bridged.extend(args.iter().cloned());
            run_browser(client, &bridged, opts.json_output).await?
        }
        "browser-reload" => {
            let mut bridged = vec!["reload".to_string()];
            bridged.extend(args.iter().cloned());
            run_browser(client, &bridged, opts.json_output).await?
        }
        "pipe-pane" | "wait-for" | "find-window" | "last-window" | "next-window"
        | "previous-window" | "swap-pane" | "break-pane" | "join-pane" | "last-pane"
        | "clear-history" | "set-hook" | "resize-pane" | "set-buffer" | "list-buffers"
        | "paste-buffer" | "respawn-pane" | "display-message" | "popup" | "bind-key"
        | "unbind-key" | "copy-mode" => {
            let payload = run_tmux_compat(client, command, args).await?;
            if opts.json_output {
                CommandOutput::Json(payload)
            } else if let Some(text) = get_string(&payload, &["text"]) {
                CommandOutput::Text(text)
            } else {
                CommandOutput::Text("OK".to_string())
            }
        }
        _ => bail!("unknown command: {}", command),
    };

    if let CommandOutput::Json(ref mut payload) = out {
        apply_id_format(payload, effective_id_format);
    }

    Ok(out)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let opts = parse_global_args()?;
    if should_launch_host(&opts) {
        return launch_host();
    }

    let socket = resolve_socket_path(opts.socket.clone(), opts.socket_mode);

    let mut client = Client::new(socket);
    let output = execute_command(&mut client, &opts).await;

    match output {
        Ok(CommandOutput::Text(text)) => {
            println!("{}", text);
            Ok(())
        }
        Ok(CommandOutput::Json(value)) => {
            if opts.pretty {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&value)
                        .context("failed to pretty print response")?
                );
            } else {
                println!(
                    "{}",
                    serde_json::to_string(&value).context("failed to encode json output")?
                );
            }
            Ok(())
        }
        Err(err) => {
            eprintln!("{}", err);
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod cli_arg_tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn default_opts(command_args: Vec<String>) -> GlobalOptions {
        GlobalOptions {
            socket: None,
            socket_mode: SocketMode::Runtime,
            json_output: false,
            id_format: IdFormat::Refs,
            request: None,
            pretty: false,
            command_args,
        }
    }

    #[test]
    fn no_args_launches_host_but_cli_flags_do_not() {
        assert!(should_launch_host(&default_opts(Vec::new())));

        let mut json_only = default_opts(Vec::new());
        json_only.json_output = true;
        assert!(!should_launch_host(&json_only));

        assert!(!should_launch_host(&default_opts(args(&[
            "list-workspaces"
        ]))));
    }

    #[tokio::test]
    async fn version_does_not_require_a_control_socket() {
        let mut client = Client::new(PathBuf::from("/does/not/exist.sock"));

        for flag in ["--version", "-V"] {
            let output = execute_command(&mut client, &default_opts(args(&[flag])))
                .await
                .expect("version output");
            match output {
                CommandOutput::Text(text) => {
                    assert_eq!(text, format!("limux {}", env!("CARGO_PKG_VERSION")));
                }
                CommandOutput::Json(_) => panic!("version should be plain text"),
            }
        }
    }

    #[test]
    fn activation_prefers_xdg_token_and_preserves_opaque_values() {
        assert_eq!(window_activation_params(None, None), json!({}));
        assert_eq!(window_activation_params(Some(""), Some("")), json!({}));
        assert_eq!(
            window_activation_params(Some(""), Some("launcher_TIME123")),
            json!({ "activation_token": "launcher_TIME123" })
        );
        assert_eq!(
            window_activation_params(None, Some("launcher_TIME123")),
            json!({ "activation_token": "launcher_TIME123" })
        );
        assert_eq!(
            window_activation_params(Some(" opaque token "), Some("launcher_TIME123")),
            json!({ "activation_token": " opaque token " })
        );
    }

    #[tokio::test]
    async fn activate_requires_an_existing_socket_and_rejects_arguments() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("absent.sock");
        let mut client = Client::new(socket.clone());
        let opts = default_opts(args(&["activate"]));
        assert!(!should_launch_host(&opts));
        let error = execute_command(&mut client, &opts).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("failed to connect to socket {}", socket.display())
        );

        let error = execute_command(
            &mut client,
            &default_opts(args(&["activate", "--workspace", "other"])),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().starts_with("activate takes no arguments"));
    }

    #[tokio::test]
    async fn terminal_commands_reject_empty_explicit_surface_before_connecting() {
        for command in ["send", "send-key", "read-screen"] {
            for target in ["", "   ", "surface:", " surface:   "] {
                let mut client = Client::new(PathBuf::from("/does/not/exist.sock"));
                let mut command_args = args(&[command, "--surface", target]);
                if command == "send" {
                    command_args.push("must not be sent".into());
                }
                if command == "send-key" {
                    command_args.push("Enter".into());
                }
                let error = execute_command(&mut client, &default_opts(command_args))
                    .await
                    .expect_err("invalid target must fail locally");
                assert_eq!(
                    error.to_string(),
                    "--surface requires a non-empty target",
                    "{command} {target:?}"
                );
            }
        }
    }

    #[test]
    fn terminal_surface_option_preserves_omission_and_valid_handles() {
        assert_eq!(parse_terminal_surface(&args(&["payload"])).unwrap(), None);
        assert!(parse_terminal_surface(&args(&["--surface"])).is_err());
        for target in ["tab-id", "4:tab-id", "surface:4:tab-id"] {
            assert_eq!(
                parse_terminal_surface(&args(&["--surface", target])).unwrap(),
                Some(target.to_string())
            );
        }
    }

    #[test]
    fn lifecycle_options_reject_explicit_empty_or_missing_values() {
        for flag in ["--workspace", "--pane", "--surface", "--type", "--url"] {
            for values in [
                vec![flag],
                vec![flag, ""],
                vec![flag, " "],
                vec![flag, "--other"],
            ] {
                assert!(
                    lifecycle_option(&args(&values), flag).is_err(),
                    "{values:?}"
                );
            }
            assert_eq!(lifecycle_option(&args(&[]), flag).unwrap(), None);
            assert_eq!(
                lifecycle_option(&args(&[flag, "value"]), flag).unwrap(),
                Some("value".to_string())
            );
        }
    }

    #[tokio::test]
    async fn lifecycle_commands_reject_empty_targets_before_connecting() {
        for (command, target_flag) in [
            ("new-surface", "--pane"),
            ("focus-pane", "--pane"),
            ("focus-surface", "--surface"),
            ("close-surface", "--surface"),
            ("rename-tab", "--tab"),
            ("tab-action", "--tab"),
        ] {
            for flag in [target_flag, "--workspace"] {
                let mut client = Client::new(PathBuf::from("/does/not/exist.sock"));
                let command_args = args(&[command, flag, "", "--action", "focus", "title"]);
                let error = execute_command(&mut client, &default_opts(command_args))
                    .await
                    .unwrap_err();
                assert_eq!(
                    error.to_string(),
                    format!("{flag} requires a non-empty value"),
                    "{command}"
                );
            }
        }
    }

    #[test]
    fn host_binary_candidates_cover_installed_and_dev_layouts() {
        let installed = Path::new("/usr/bin/limux");
        let candidates = host_binary_candidates(installed);
        assert!(candidates.contains(&PathBuf::from("/usr/libexec/limux/limux-host")));
        assert!(!candidates.contains(&PathBuf::from("/usr/bin/limux")));

        let dev = Path::new("/repo/target/debug/limux-cli");
        let candidates = host_binary_candidates(dev);
        assert!(candidates.contains(&PathBuf::from("/repo/target/debug/limux")));
    }

    #[test]
    fn notify_positional_title_skips_option_values() {
        let args = args(&[
            "--subtitle",
            "needs review",
            "--body",
            "blocked",
            "Input needed",
        ]);

        assert_eq!(trailing_title(&args).as_deref(), Some("Input needed"));
    }

    #[test]
    fn notification_target_prefers_explicit_surface_then_stable_tab_id() {
        let env_value = |key: &str| match key {
            "LIMUX_TAB_ID" => Some("tab-stable".to_string()),
            "LIMUX_SURFACE_ID" => Some("1:tab-stable".to_string()),
            _ => None,
        };

        assert_eq!(
            notification_surface_target(&args(&["--surface", "2:explicit"]), env_value),
            Some("2:explicit".to_string())
        );
        assert_eq!(
            notification_surface_target(&[], env_value),
            Some("tab-stable".to_string())
        );
    }

    #[test]
    fn hook_event_comes_from_json_after_option_values() {
        let args = args(&["--workspace", "codex"]);
        let payload = json!({ "hook_event_name": "Notification" });

        assert_eq!(parse_hook_event(&args, &payload), "Notification");
    }

    #[test]
    fn hook_event_prefers_explicit_event_flag() {
        let args = args(&["--workspace", "codex", "--event", "Stop"]);
        let payload = json!({ "hook_event_name": "Notification" });

        assert_eq!(parse_hook_event(&args, &payload), "Stop");
    }

    #[test]
    fn hook_event_accepts_positional_event_after_options() {
        let args = args(&["--workspace", "codex", "Stop"]);
        let payload = json!({ "hook_event_name": "Notification" });

        assert_eq!(parse_hook_event(&args, &payload), "Stop");
    }

    #[test]
    fn external_session_end_preserves_restorable_hook_session() {
        assert_eq!(
            agent_hook_persistence_action("SessionEnd"),
            AgentHookPersistenceAction::Preserve
        );
        assert_eq!(
            agent_hook_persistence_action("session-end"),
            AgentHookPersistenceAction::Preserve
        );
    }

    #[test]
    fn internal_cleanup_removes_restorable_hook_session() {
        assert_eq!(
            agent_hook_persistence_action("cleanup"),
            AgentHookPersistenceAction::Remove
        );
        assert_eq!(
            agent_hook_persistence_action("restore-exit"),
            AgentHookPersistenceAction::Remove
        );
    }

    #[test]
    fn cleanup_scope_uses_current_target_and_can_fall_back_to_stable_tab_or_payload() {
        let payload = json!({"workspace_id": "payload-workspace", "surface_id": "5:payload-tab"});
        let current = |key: &str| match key {
            "LIMUX_WORKSPACE_ID" => Some("current-workspace".into()),
            "LIMUX_SURFACE_ID" => Some("7:current-tab".into()),
            "LIMUX_TAB_ID" => Some("stable-tab".into()),
            _ => None,
        };
        assert_eq!(
            agent_hook_cleanup_scope(
                &args(&["--workspace", "explicit", "--surface", "8:explicit-tab"]),
                &payload,
                current
            ),
            (Some("explicit".into()), Some("8:explicit-tab".into()))
        );
        assert_eq!(
            agent_hook_cleanup_scope(&[], &payload, current),
            (
                Some("current-workspace".into()),
                Some("7:current-tab".into())
            )
        );
        assert_eq!(
            agent_hook_cleanup_scope(&[], &json!({}), |key| (key == "LIMUX_TAB_ID")
                .then(|| "stable-tab".into())),
            (None, Some("stable-tab".into()))
        );
        assert_eq!(
            agent_hook_cleanup_scope(&[], &payload, |_| None),
            (
                Some("payload-workspace".into()),
                Some("5:payload-tab".into())
            )
        );
    }

    #[test]
    fn default_hook_setup_omits_opencode_until_supported() {
        assert_eq!(
            default_hook_targets(),
            vec![
                agent_hooks::AgentKind::Codex,
                agent_hooks::AgentKind::Claude,
                agent_hooks::AgentKind::Gemini,
                agent_hooks::AgentKind::Agy,
            ]
        );
        assert!(!default_hook_targets().contains(&agent_hooks::AgentKind::OpenCode));
    }

    #[test]
    fn opencode_plugin_embeds_installer_cli_command() {
        let source = opencode_plugin_source_with_command("/tmp/limux-cli").expect("plugin source");

        assert!(source.contains("const LIMUX_COMMAND = \"/tmp/limux-cli\";"));
        assert!(source.contains("process.env.LIMUX_BIN || LIMUX_COMMAND"));
        assert!(!source.contains("process.env.LIMUX_BIN || \"limux\""));
    }

    #[test]
    fn opencode_plugin_removes_only_deleted_sessions() {
        let source = opencode_plugin_source_with_command("/tmp/limux-cli").expect("plugin source");

        assert!(
            source.contains("if (type === \"session.error\") send(\"session-end\", ctx, event);")
        );
        assert!(source.contains("if (type === \"session.deleted\") send(\"cleanup\", ctx, event);"));
        assert!(source.contains("type === \"session.status\""));
        assert!(source.contains("type === \"session.compacted\""));
    }

    #[test]
    fn stop_hook_output_matches_codex_schema_shape() {
        let output = agent_hook_output(
            agent_hooks::AgentKind::Codex,
            "stop",
            &json!({ "session_id": "session-a" }),
        );

        assert_eq!(
            output,
            json!({
                "continue": true,
                "suppressOutput": false
            })
        );
    }

    #[test]
    fn session_start_hook_output_uses_camel_case_specific_output() {
        let output = agent_hook_output(
            agent_hooks::AgentKind::Claude,
            "session-start",
            &json!({ "additionalContext": "Limux session restore tracking active." }),
        );

        assert_eq!(
            output,
            json!({
                "continue": true,
                "suppressOutput": false,
                "hookSpecificOutput": {
                    "hookEventName": "SessionStart",
                    "additionalContext": "Limux session restore tracking active."
                }
            })
        );
    }

    #[test]
    fn agy_hook_output_matches_antigravity_schema() {
        assert_eq!(
            agent_hook_output(
                agent_hooks::AgentKind::Agy,
                "stop",
                &json!({ "conversationId": "conv-1" }),
            ),
            json!({ "decision": "allow" })
        );
        assert_eq!(
            agent_hook_output(
                agent_hooks::AgentKind::Agy,
                "session-start",
                &json!({ "conversationId": "conv-1" }),
            ),
            json!({})
        );
        assert_eq!(
            agent_hook_output(
                agent_hooks::AgentKind::Agy,
                "pre-invocation",
                &json!({ "conversationId": "conv-1", "invocationNum": 2 }),
            ),
            json!({})
        );
        assert_eq!(
            agent_hook_output(
                agent_hooks::AgentKind::Agy,
                "session-start",
                &json!({
                    "conversationId": "conv-1",
                    "additionalContext": "Injected context"
                }),
            ),
            json!({
                "injectSteps": [{
                    "systemMessage": {
                        "systemMessage": "Injected context"
                    }
                }]
            })
        );
        assert_eq!(
            agent_hook_output(
                agent_hooks::AgentKind::Agy,
                "PreInvocation",
                &json!({
                    "conversationId": "conv-1",
                    "additionalContext": "Turn context"
                }),
            ),
            json!({
                "injectSteps": [{
                    "systemMessage": {
                        "systemMessage": "Turn context"
                    }
                }]
            })
        );
        assert!(agy_stop_suppresses_notification(
            &json!({ "terminationReason": "USER_CANCELED" })
        ));
        assert!(agy_stop_suppresses_notification(
            &json!({ "fullyIdle": false })
        ));
        assert!(!agy_stop_suppresses_notification(
            &json!({ "terminationReason": "NO_TOOL_CALL", "fullyIdle": true })
        ));
        assert_eq!(
            agent_hook_notification(
                agent_hooks::AgentKind::Agy,
                "PreInvocation",
                &json!({ "conversationId": "conv-1" }),
            ),
            None
        );
        assert_eq!(
            agent_hook_notification(
                agent_hooks::AgentKind::Agy,
                "PostInvocation",
                &json!({ "conversationId": "conv-1" }),
            ),
            None
        );
        assert_eq!(
            agent_hook_notification(
                agent_hooks::AgentKind::Agy,
                "stop",
                &json!({ "terminationReason": "ERROR", "error": "rate limit exceeded", "fullyIdle": true }),
            ),
            Some((
                "Antigravity failed".to_string(),
                "rate limit exceeded".to_string(),
            ))
        );
        assert_eq!(
            agent_hook_notification(
                agent_hooks::AgentKind::Agy,
                "stop",
                &json!({ "terminationReason": "MAX_TOKEN_BUDGET_EXCEEDED", "fullyIdle": true }),
            ),
            Some((
                "Antigravity finished".to_string(),
                "exceeded token budget".to_string(),
            ))
        );
        assert_eq!(
            agent_hook_notification(
                agent_hooks::AgentKind::Agy,
                "stop",
                &json!({ "terminationReason": "NO_TOOL_CALL", "fullyIdle": true }),
            ),
            Some((
                "Antigravity finished".to_string(),
                "task complete".to_string(),
            ))
        );
        assert_eq!(
            hook_cwd(&json!({ "workspacePaths": ["", "/tmp/agy-ws", "/tmp/other"] })),
            Some("/tmp/agy-ws".to_string())
        );
        assert_eq!(
            hook_cwd(&json!({ "cwd": "/tmp/explicit", "workspacePaths": ["/tmp/agy-ws"] })),
            Some("/tmp/explicit".to_string())
        );
    }

    #[test]
    fn agy_hook_install_and_uninstall_preserve_other_named_hooks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("hooks.json");
        let original = json!({
            "my-linter": {
                "PostToolUse": [{
                    "matcher": "write_file",
                    "type": "command",
                    "command": "echo lint"
                }]
            }
        });
        fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();

        install_agy_hooks(&path, true).expect("install agy hooks");
        let installed: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(installed["my-linter"], original["my-linter"]);
        assert_eq!(installed["limux"]["SessionStart"][0]["timeout"], 5);
        assert!(installed["limux"]["SessionStart"][0]["command"]
            .as_str()
            .unwrap()
            .contains("env LIMUX_NOTIFY_OSC=1"));
        assert!(installed["limux"]["SessionStart"][0]["command"]
            .as_str()
            .unwrap()
            .contains("hooks agy session-start"));
        assert_eq!(installed["limux"]["PreInvocation"][0]["timeout"], 5);
        assert!(installed["limux"]["PreInvocation"][0]["command"]
            .as_str()
            .unwrap()
            .contains("hooks agy pre-invocation"));
        assert!(installed["limux"]["PreInvocation"][0]["command"]
            .as_str()
            .unwrap()
            .contains("echo '{}'"));
        assert!(installed["limux"]["Stop"][0]["command"]
            .as_str()
            .unwrap()
            .contains("{\"decision\":\"allow\"}"));

        uninstall_agy_hooks(&path).expect("uninstall agy hooks");
        let after: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(after, original);
    }

    #[test]
    fn gemini_and_agy_config_paths_resolve_canonical_locations() {
        assert_eq!(
            gemini_settings_path_from(
                Some(PathBuf::from("/custom/gemini-home")),
                Some(PathBuf::from("/home/u")),
            ),
            PathBuf::from("/custom/gemini-home/.gemini/settings.json")
        );
        assert_eq!(
            gemini_settings_path_from(None, Some(PathBuf::from("/home/u"))),
            PathBuf::from("/home/u/.gemini/settings.json")
        );
        assert_eq!(
            gemini_settings_path_from(None, None),
            PathBuf::from(".gemini/settings.json")
        );
        assert_eq!(
            agy_hooks_path_from(Some(PathBuf::from("/home/u"))),
            PathBuf::from("/home/u/.gemini/config/hooks.json")
        );
        assert_eq!(
            agy_hooks_path_from(None),
            PathBuf::from(".gemini/config/hooks.json")
        );
    }

    #[test]
    fn claude_hook_install_writes_required_matcher() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");

        install_json_hooks(
            &path,
            agent_hooks::AgentKind::Claude,
            false,
            &[("SessionStart", "session-start")],
        )
        .expect("install hooks");

        let root: Value =
            serde_json::from_slice(&fs::read(&path).expect("read settings")).expect("json");
        let entry = &root["hooks"]["SessionStart"][0];
        assert_eq!(entry["matcher"], "*");
        assert_eq!(entry["hooks"][0]["timeout"], 5);
        assert!(entry["hooks"][0]["command"]
            .as_str()
            .expect("command")
            .contains("hooks claude session-start"));
    }

    #[test]
    fn osc_hook_install_is_explicit_idempotent_and_preserves_other_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hooks.json");
        let original = json!({"custom": 42, "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "echo independent"}]}]}});
        fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
        for _ in 0..2 {
            install_json_hooks(
                &path,
                agent_hooks::AgentKind::Codex,
                true,
                &[("Stop", "stop")],
            )
            .unwrap();
        }
        let config: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(config["custom"], 42);
        let stop = config["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0], original["hooks"]["Stop"][0]);
        assert!(stop[1]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("env LIMUX_NOTIFY_OSC=1"));
        install_json_hooks(
            &path,
            agent_hooks::AgentKind::Codex,
            false,
            &[("Stop", "stop")],
        )
        .unwrap();
        assert!(!fs::read_to_string(&path)
            .unwrap()
            .contains("LIMUX_NOTIFY_OSC"));
        uninstall_json_hooks(&path, agent_hooks::AgentKind::Codex).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(path).unwrap()).unwrap(),
            original
        );
    }

    #[test]
    fn codex_hook_install_keeps_codex_schema_without_matcher() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("hooks.json");

        install_json_hooks(
            &path,
            agent_hooks::AgentKind::Codex,
            false,
            &[("SessionStart", "session-start")],
        )
        .expect("install hooks");

        let root: Value =
            serde_json::from_slice(&fs::read(&path).expect("read hooks")).expect("json");
        let entry = &root["hooks"]["SessionStart"][0];
        assert!(entry.get("matcher").is_none());
        assert_eq!(entry["hooks"][0]["timeout"], 5);
        assert!(entry["hooks"][0]["command"]
            .as_str()
            .expect("command")
            .contains("hooks codex session-start"));
    }

    #[test]
    fn environ_parser_reads_requested_limux_value() {
        let environ = b"PATH=/bin\0LIMUX_WORKSPACE_ID=ws-1\0LIMUX_SURFACE_ID=7:tab-a\0";

        assert_eq!(
            env_value_from_environ(environ, "LIMUX_WORKSPACE_ID").as_deref(),
            Some("ws-1")
        );
        assert_eq!(
            env_value_from_environ(environ, "LIMUX_SURFACE_ID").as_deref(),
            Some("7:tab-a")
        );
        assert_eq!(env_value_from_environ(environ, "LIMUX_PANE_ID"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn proc_stat_parser_handles_process_names_with_spaces() {
        let stat = "1234 (claude hook sh) S 987 1 1 0 -1 4194560";

        assert_eq!(parse_proc_stat_parent_pid(stat), Some(987));
    }

    #[test]
    fn hook_session_id_falls_back_to_transcript_stem() {
        let payload = json!({
            "transcript_path": "/home/amwill/.claude/projects/-home-amwill-Applications-limux/268746f1-5a8f-471c-85db-dc50649c2f9c.jsonl"
        });

        assert_eq!(
            hook_session_id_with_env(agent_hooks::AgentKind::Claude, &payload, |_| None).as_deref(),
            Some("268746f1-5a8f-471c-85db-dc50649c2f9c")
        );
    }

    #[test]
    fn hook_session_id_prefers_explicit_session_id() {
        let payload = json!({
            "session_id": "explicit-session",
            "transcript_path": "/tmp/transcript-session.jsonl"
        });

        assert_eq!(
            hook_session_id_with_env(agent_hooks::AgentKind::Claude, &payload, |_| None).as_deref(),
            Some("explicit-session")
        );
    }

    #[tokio::test]
    async fn protocol_cli_subcommands_execute_and_call_canonical_rpc_method() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("mock.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind mock socket");
        let recorded = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let recorded_server = std::sync::Arc::clone(&recorded);

        let server_task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let recorded_conn = std::sync::Arc::clone(&recorded_server);
                tokio::spawn(async move {
                    let (reader_half, mut writer_half) = stream.into_split();
                    let mut reader = BufReader::new(reader_half);
                    let mut line = String::new();
                    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let req: V2Request = serde_json::from_str(line.trim()).expect("valid request");
                    recorded_conn
                        .lock()
                        .expect("recorded lock")
                        .push(req.method.clone());
                    let resp = V2Response::success(
                        req.id,
                        json!({
                            "workspace_id": "ws-1",
                            "workspace_ref": "workspace:1",
                            "surface_id": "1:tab-1",
                            "surface_ref": "surface:1:tab-1",
                            "pane_id": "1",
                            "pane_ref": "pane:1",
                            "url": "https://example.com",
                        }),
                    );
                    let mut encoded = serde_json::to_string(&resp).expect("encode response");
                    encoded.push('\n');
                    let _ = writer_half.write_all(encoded.as_bytes()).await;
                    let _ = writer_half.flush().await;
                });
            }
        });

        let mut client = Client::new(socket_path.clone());
        let mut tested_count = 0usize;

        for spec in &limux_protocol::methods::ALL_METHOD_SPECS {
            for &sub in spec.cli_subcommands {
                tested_count += 1;
                assert_eq!(
                    limux_protocol::methods::resolve_cli_subcommand_spec(sub).map(|s| s.name),
                    Some(spec.name),
                    "CLI subcommand {sub} should resolve to {}",
                    spec.name
                );

                if sub == "notify" && terminal_notifications::forced() {
                    continue;
                }

                recorded.lock().expect("recorded lock").clear();
                let command_args = match sub {
                    "send" => args(&["send", "hello"]),
                    "send-key" => args(&["send-key", "Enter"]),
                    "notify" => args(&["notify", "--workspace", "ws-1", "hello"]),
                    "select-workspace" | "close-workspace" => args(&[sub, "--workspace", "ws-1"]),
                    "rename-workspace" | "rename-window" => {
                        args(&[sub, "--workspace", "ws-1", "Title"])
                    }
                    "set-workspace-color" => {
                        args(&["set-workspace-color", "--workspace", "ws-1", "blue"])
                    }
                    "set-workspace-group" => {
                        args(&["set-workspace-group", "--workspace", "ws-1", "Ops"])
                    }
                    "rename-group" => args(&["rename-group", "--group", "g1", "Ops"]),
                    "set-group-color" => args(&["set-group-color", "--group", "g1", "blue"]),
                    "collapse-group" | "expand-group" | "delete-group" => {
                        args(&[sub, "--group", "g1"])
                    }
                    "tab-action" => args(&["tab-action", "--action", "pin"]),
                    "rename-tab" => args(&["rename-tab", "Title"]),
                    "swap-pane" => args(&["swap-pane", "--pane", "1", "--target-pane", "2"]),
                    "join-pane" => args(&["join-pane", "--target-pane", "2"]),
                    "resize-pane" => args(&["resize-pane", "--pane", "1"]),
                    "open-browser" => args(&["open-browser", "https://example.com"]),
                    "navigate-browser" => args(&[
                        "navigate-browser",
                        "--surface",
                        "1:tab-1",
                        "https://example.com",
                    ]),
                    "browser-back" | "browser-forward" | "browser-reload" => {
                        args(&[sub, "--surface", "1:tab-1"])
                    }
                    _ => args(&[sub]),
                };

                execute_command(&mut client, &default_opts(command_args))
                    .await
                    .unwrap_or_else(|err| panic!("CLI subcommand {sub} failed: {err:#}"));

                let calls = recorded.lock().expect("recorded lock").clone();
                assert!(
                    calls.iter().any(|method| method == spec.name),
                    "CLI subcommand {sub} expected RPC {}, got {calls:?}",
                    spec.name
                );
            }
        }
        assert!(tested_count >= 40, "expected full CLI subcommand coverage");

        for non_cli in [
            "ping",
            "send-text",
            "workspace.activate",
            "activate-workspace",
            "capabilities",
            "list-windows",
            "new-window",
            "focus-window",
            "close-window",
            "next-workspace",
            "previous-workspace",
            "last-workspace",
            "new-split",
            "browser.open-split",
        ] {
            assert!(
                limux_protocol::methods::resolve_cli_subcommand_spec(non_cli).is_none(),
                "{non_cli} must not be registered as a CLI subcommand"
            );
            let err = execute_command(&mut client, &default_opts(args(&[non_cli])))
                .await
                .expect_err("non-CLI name must fail");
            assert!(
                err.to_string().contains("unknown command:"),
                "unexpected error for {non_cli}: {err:#}"
            );
        }

        server_task.abort();
    }

    #[test]
    fn hook_session_id_isolates_claude_env_and_supports_agy_conversation_id() {
        let claude_env = |name: &str| {
            (name == "CLAUDE_CODE_SESSION_ID").then(|| "claude-parent-session".to_string())
        };
        assert_eq!(
            hook_session_id_with_env(agent_hooks::AgentKind::Claude, &json!({}), claude_env)
                .as_deref(),
            Some("claude-parent-session")
        );
        assert_eq!(
            hook_session_id_with_env(agent_hooks::AgentKind::Codex, &json!({}), claude_env),
            None
        );
        assert_eq!(
            hook_session_id_with_env(
                agent_hooks::AgentKind::Agy,
                &json!({ "conversationId": "agy-conv-42" }),
                claude_env
            )
            .as_deref(),
            Some("agy-conv-42")
        );
        assert_eq!(
            hook_session_id_with_env(
                agent_hooks::AgentKind::Agy,
                &json!({
                    "transcriptPath": "/home/u/.gemini/antigravity-cli/brain/fe490dbf-794d-4c6d-8aa0-71d50673e523/.system_generated/logs/transcript.jsonl"
                }),
                |_| None
            )
            .as_deref(),
            Some("fe490dbf-794d-4c6d-8aa0-71d50673e523")
        );
    }
}

#[cfg(test)]
mod agent_team_tests {
    use super::*;

    #[tokio::test]
    async fn dry_run_previews_instructions_without_creating_or_overwriting_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("AGENTS.md");
        let args = vec![
            "--dry-run".to_string(),
            "--cwd".to_string(),
            dir.path().to_string_lossy().into_owned(),
        ];
        let mut client = Client::new(dir.path().join("absent.sock"));
        let preview = run_agent_team(&mut client, &args).await.unwrap();
        assert!(preview["agents_md_preview"]
            .as_str()
            .unwrap()
            .contains("<agent-msg"));
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);

        let original = "# Existing project instructions\nNever discard these.\n";
        fs::write(&path, original).unwrap();
        run_agent_team(&mut client, &args).await.unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), original);
    }

    #[tokio::test]
    async fn existing_instructions_fail_before_contacting_host() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("AGENTS.md");
        fs::write(&path, "User policies").unwrap();
        let args = vec![
            "--cwd".to_string(),
            dir.path().to_string_lossy().into_owned(),
        ];
        let mut client = Client::new(dir.path().join("absent.sock"));
        let error = run_agent_team(&mut client, &args).await.unwrap_err();
        assert!(error.to_string().contains("refusing to replace"));
        assert_eq!(client.seq, 0);
        assert_eq!(fs::read_to_string(path).unwrap(), "User policies");
    }

    #[tokio::test]
    async fn invalid_destination_fails_before_contacting_host() {
        let dir = tempfile::tempdir().unwrap();
        let args = vec![
            "--cwd".to_string(),
            dir.path()
                .join("missing-directory")
                .to_string_lossy()
                .into_owned(),
        ];
        let mut client = Client::new(dir.path().join("absent.sock"));
        let error = run_agent_team(&mut client, &args).await.unwrap_err();
        assert!(error.to_string().contains("could not resolve cwd"));
        assert_eq!(client.seq, 0);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn agent_launch_known() {
        for agent in [
            "codex",
            "claude",
            "claude-code",
            "opencode",
            "gemini",
            "gemini-cli",
            "agy",
            "antigravity",
            "antigravity-cli",
        ] {
            assert!(
                agent_launch_command(agent).is_some(),
                "expected '{agent}' to be a known agent"
            );
        }
    }

    #[test]
    fn agent_launch_unknown_returns_none() {
        assert!(agent_launch_command("nonsense-cli").is_none());
    }

    #[test]
    fn agents_md_contains_protocol_and_peers() {
        let peers = vec![
            (
                "codex".to_string(),
                "10".to_string(),
                "10:tab-a".to_string(),
                "codex".to_string(),
            ),
            (
                "claude".to_string(),
                "11".to_string(),
                "11:tab-a".to_string(),
                "claude".to_string(),
            ),
        ];
        let md = build_agents_md(
            &peers,
            "/tmp/team",
            "active-ws",
            "ws-uuid-123",
            "9:terminal-orch",
        );

        // Header & generation marker
        assert!(md.contains("AGENTS.md — agent-to-agent message protocol"));
        assert!(md.contains("Generated by `limux agent-team`"));

        // Team workspace block
        assert!(md.contains("Workspace name: `active-ws`"));
        assert!(md.contains("Workspace ID: `ws-uuid-123`"));
        assert!(md.contains("Orchestrator surface: `9:terminal-orch`"));
        assert!(md.contains("Shared cwd: `/tmp/team`"));

        // Peer table rows (Agent | Pane | Surface | Launch)
        assert!(md.contains("| `codex` | `10` | `10:tab-a` | `codex` |"));
        assert!(md.contains("| `claude` | `11` | `11:tab-a` | `claude` |"));

        // Protocol envelope spec uses --surface, not --workspace
        assert!(md.contains("<agent-msg from=\"codex\" to=\"claude\""));
        assert!(md.contains("limux send --surface"));
        assert!(!md.contains("limux send --workspace"));
        assert!(md.contains("reply-to"));

        // Notify + env contract
        assert!(md.contains("limux notify"));
        assert!(md.contains("LIMUX_WORKSPACE_ID"));
        assert!(md.contains("LIMUX_SURFACE_ID"));
        assert!(md.contains("limux new-pane --direction right --command bash"));
        assert!(md.contains("Live GTK self-spawn supports both terminal and browser"));
    }
}

#[cfg(test)]
mod request_workspace_tests {
    use super::request_workspace;

    fn env(name: &str) -> Option<String> {
        (name == "LIMUX_WORKSPACE_ID").then(|| "ws-env".to_string())
    }

    #[test]
    fn env_workspace_scopes_only_requests_without_a_surface() {
        assert_eq!(
            request_workspace(None, None, env).as_deref(),
            Some("ws-env")
        );
        assert_eq!(request_workspace(None, Some("7:tab-a"), env), None);
        assert_eq!(
            request_workspace(Some("ws-flag".to_string()), Some("7:tab-a"), env).as_deref(),
            Some("ws-flag")
        );
        assert_eq!(request_workspace(None, None, |_| None), None);
    }

    #[test]
    fn an_empty_explicit_workspace_means_unscoped() {
        for explicit in ["", " "] {
            assert_eq!(
                request_workspace(Some(explicit.to_string()), None, env),
                None
            );
        }
    }

    #[test]
    fn a_notification_from_a_surface_is_unscoped() {
        let env = |name: &str| match name {
            "LIMUX_WORKSPACE_ID" => Some("ws-env".to_string()),
            "LIMUX_TAB_ID" => Some("tab-a".to_string()),
            _ => None,
        };
        assert_eq!(
            super::notification_scope(&[], env),
            (None, Some("tab-a".to_string()))
        );
        assert_eq!(
            super::notification_scope(&["--workspace".to_string(), "ws-flag".to_string()], env),
            (Some("ws-flag".to_string()), Some("tab-a".to_string()))
        );
        assert_eq!(
            super::notification_scope(&[], |name: &str| {
                (name == "LIMUX_WORKSPACE_ID").then(|| "ws-env".to_string())
            }),
            (Some("ws-env".to_string()), None)
        );
    }
}

#[cfg(test)]
mod new_pane_tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn test_env(name: &str) -> Option<String> {
        match name {
            "LIMUX_WORKSPACE_ID" => Some("workspace:agent".to_string()),
            "LIMUX_SURFACE_ID" => Some("surface:11:tab-a".to_string()),
            "LIMUX_PANE_ID" => Some("pane:11".to_string()),
            _ => None,
        }
    }

    #[test]
    fn set_workspace_color_uses_env_workspace_and_passes_color_through() {
        let params = build_set_workspace_color_params(&args(&["blue"]), test_env).unwrap();

        assert_eq!(
            params,
            json!({ "color": "blue", "workspace_id": "workspace:agent" })
        );
    }

    #[test]
    fn set_workspace_color_flag_overrides_env_workspace() {
        let params = build_set_workspace_color_params(
            &args(&["--workspace", "raw-workspace", "none"]),
            test_env,
        )
        .unwrap();

        assert_eq!(
            params,
            json!({ "color": "none", "workspace_id": "raw-workspace" })
        );
    }

    #[test]
    fn set_workspace_color_without_env_targets_the_active_workspace() {
        let params = build_set_workspace_color_params(&args(&["teal"]), |_| None).unwrap();

        assert_eq!(params, json!({ "color": "teal" }));
    }

    #[test]
    fn set_workspace_color_requires_a_color() {
        let error =
            build_set_workspace_color_params(&args(&["--workspace", "raw-workspace"]), test_env)
                .unwrap_err();

        assert!(error.to_string().contains("requires a color"));
    }

    #[test]
    fn group_commands_build_their_socket_requests() {
        let cases = [
            (vec!["list-groups"], "workspace.group_list", json!({})),
            (
                vec![
                    "create-group",
                    "--color",
                    "teal",
                    "--workspace",
                    "workspace:1",
                    "--workspace",
                    "2",
                    "Back",
                    "end",
                ],
                "workspace.group_create",
                json!({ "name": "Back end", "color": "teal", "workspace_ids": ["workspace:1", "2"] }),
            ),
            (vec!["create-group"], "workspace.group_create", json!({})),
            (
                vec!["rename-group", "--group", "group:abc", "Ops"],
                "workspace.group_rename",
                json!({ "group_id": "group:abc", "name": "Ops" }),
            ),
            (
                vec!["set-group-color", "--group", "Ops", "red"],
                "workspace.group_set_color",
                json!({ "group_id": "Ops", "color": "red" }),
            ),
            (
                vec!["collapse-group", "--group", "abc"],
                "workspace.group_set_collapsed",
                json!({ "group_id": "abc", "collapsed": true }),
            ),
            (
                vec!["expand-group", "--group", "abc"],
                "workspace.group_set_collapsed",
                json!({ "group_id": "abc", "collapsed": false }),
            ),
            (
                vec!["delete-group", "--group", "abc"],
                "workspace.group_delete",
                json!({ "group_id": "abc" }),
            ),
            (
                vec!["set-workspace-group", "Ops"],
                "workspace.set_group",
                json!({ "group_id": "Ops", "workspace_id": "workspace:agent" }),
            ),
            (
                vec!["set-workspace-group", "--workspace", "raw", "None"],
                "workspace.set_group",
                json!({ "group_id": null, "workspace_id": "raw" }),
            ),
        ];
        for (argv, method, params) in cases {
            let (command, rest) = argv.split_first().unwrap();
            assert_eq!(
                build_group_request(command, &args(rest), test_env).unwrap(),
                (method, params),
                "{argv:?}"
            );
        }
    }

    #[test]
    fn group_commands_require_their_arguments() {
        for (argv, message) in [
            (vec!["rename-group", "Ops"], "requires --group"),
            (vec!["rename-group", "--group", "abc"], "requires a name"),
            (
                vec!["set-group-color", "--group", "abc"],
                "requires a color",
            ),
            (vec!["delete-group"], "requires --group"),
            (vec!["collapse-group", "--group", " "], "requires --group"),
            (vec!["set-workspace-group"], "or \"none\" to ungroup"),
        ] {
            let (command, rest) = argv.split_first().unwrap();
            let error = build_group_request(command, &args(rest), test_env).unwrap_err();
            assert!(error.to_string().contains(message), "{argv:?}: {error}");
        }
    }

    #[test]
    fn gui_hooks_commands_build_their_socket_requests() {
        let cases = [
            (vec!["list"], "gui_hook.list", json!({})),
            (
                vec![
                    "set",
                    "on-workspace-create",
                    "--command",
                    "~/bin/new.sh --force",
                    "--show-terminal",
                    "--no-suppress-default",
                ],
                "gui_hook.set",
                json!({
                    "event": "on-workspace-create",
                    "command": "~/bin/new.sh --force",
                    "show_terminal": true,
                    "suppress_default": false,
                }),
            ),
            (
                vec!["set", "on-workspace-delete", "--suppress-default"],
                "gui_hook.set",
                json!({ "event": "on-workspace-delete", "suppress_default": true }),
            ),
            (
                vec!["set", "on-workspace-delete", "--disable"],
                "gui_hook.set",
                json!({ "event": "on-workspace-delete", "enabled": false }),
            ),
            (
                vec![
                    "set",
                    "after-workspace-create",
                    "--command",
                    "notify-send created",
                    "--show-terminal",
                ],
                "gui_hook.set",
                json!({
                    "event": "after-workspace-create",
                    "command": "notify-send created",
                    "show_terminal": true,
                }),
            ),
            (
                vec!["disable", "after-workspace-delete"],
                "gui_hook.set",
                json!({ "event": "after-workspace-delete", "enabled": false }),
            ),
            (
                vec!["disable", "on-workspace-create"],
                "gui_hook.set",
                json!({ "event": "on-workspace-create", "enabled": false }),
            ),
            (
                vec!["enable", "on-workspace-create"],
                "gui_hook.set",
                json!({ "event": "on-workspace-create", "enabled": true }),
            ),
            (
                vec!["clear", "on-workspace-delete"],
                "gui_hook.clear",
                json!({ "event": "on-workspace-delete" }),
            ),
        ];
        for (argv, method, params) in cases {
            assert_eq!(
                build_gui_hooks_request(&args(&argv)).unwrap(),
                (method, params),
                "{argv:?}"
            );
        }
    }

    #[test]
    fn gui_hooks_commands_reject_incomplete_arguments() {
        for (argv, message) in [
            (vec![], "usage"),
            (vec!["remove"], "unknown gui-hooks subcommand"),
            (vec!["set", "--show-terminal"], "requires an event"),
            (vec!["set", "on-workspace-create"], "requires --command"),
            (
                vec!["set", "on-workspace-create", "--command"],
                "requires the command",
            ),
            (
                vec![
                    "set",
                    "on-workspace-create",
                    "--show-terminal",
                    "--no-show-terminal",
                ],
                "cannot be combined",
            ),
            (
                vec!["set", "on-workspace-create", "--enable", "--disable"],
                "cannot be combined",
            ),
            (vec!["disable"], "requires an event"),
            (vec!["clear"], "requires an event"),
        ] {
            let error = build_gui_hooks_request(&args(&argv)).unwrap_err();
            assert!(error.to_string().contains(message), "{argv:?}: {error}");
        }
    }

    #[test]
    fn gui_hooks_render_one_line_per_hook() {
        let payload = json!({ "hooks": [
            { "event": "on-workspace-create", "configured": true, "command": "new.sh -y", "show_terminal": true, "suppress_default": true },
            { "event": "after-workspace-create", "configured": true, "enabled": true, "command": "notify-send created", "show_terminal": false, "suppress_default": false },
            { "event": "on-workspace-delete", "configured": false, "command": "", "show_terminal": false, "suppress_default": false },
        ]});
        assert_eq!(
            render_gui_hooks(&payload),
            "on-workspace-create: new.sh -y (in a terminal, default action skipped)\n\
             after-workspace-create: notify-send created (in the background)\n\
             on-workspace-delete: not set"
        );
        let disabled = json!({ "event": "on-workspace-delete", "configured": true, "enabled": false, "command": "rm.sh", "show_terminal": false, "suppress_default": false });
        assert_eq!(
            render_gui_hooks(&disabled),
            "on-workspace-delete: rm.sh (disabled, in the background)"
        );
    }

    #[test]
    fn group_list_renders_one_line_per_group() {
        let payload = json!({ "groups": [
            { "ref": "group:a", "name": "Backend", "color": "blue", "collapsed": false, "workspace_ids": ["1", "2"] },
            { "ref": "group:b", "name": "Docs", "color": "teal", "collapsed": true, "workspace_ids": ["3"] },
        ]});
        assert_eq!(
            render_group_list(&payload),
            "group:a Backend (blue, 2 workspaces)\ngroup:b Docs (teal, 1 workspace, collapsed)"
        );
        assert_eq!(render_group_list(&json!({ "groups": [] })), "No groups");
    }

    #[test]
    fn read_screen_params_forward_scrollback_and_lines() {
        assert_eq!(
            build_read_screen_params(&args(&[
                "--workspace",
                "ws-1",
                "--surface",
                "4:tab-a",
                "--scrollback",
                "--lines",
                "80",
            ]))
            .unwrap(),
            json!({
                "workspace_id": "ws-1",
                "surface_id": "4:tab-a",
                "scrollback": true,
                "lines": 80,
            })
        );
        assert_eq!(build_read_screen_params(&args(&[])).unwrap(), json!({}));
        assert!(build_read_screen_params(&args(&["--lines", "0"])).is_err());
        assert!(build_read_screen_params(&args(&["--lines"])).is_err());
    }

    #[test]
    fn set_hook_positional_separates_event_name_from_command_body() {
        assert_eq!(
            parse_set_hook_positional(&args(&["after-workspace-create", "echo", "ready"])).unwrap(),
            (
                "after-workspace-create".to_string(),
                "echo ready".to_string()
            )
        );
        assert_eq!(
            parse_set_hook_positional(&args(&[
                "after-workspace-create",
                "bash",
                "-lc",
                "echo ready",
            ]))
            .unwrap(),
            (
                "after-workspace-create".to_string(),
                "bash -lc echo ready".to_string()
            )
        );
        assert_eq!(
            parse_set_hook_positional(&args(&[
                "after-workspace-create",
                "notify-send",
                "-u",
                "critical",
                "Done",
            ]))
            .unwrap(),
            (
                "after-workspace-create".to_string(),
                "notify-send -u critical Done".to_string()
            )
        );
        assert_eq!(
            parse_set_hook_positional(&args(&[
                "--workspace",
                "ws1",
                "on-workspace-create",
                "echo",
                "-n",
                "ready",
            ]))
            .unwrap(),
            (
                "on-workspace-create".to_string(),
                "echo -n ready".to_string()
            )
        );
        assert_eq!(
            parse_set_hook_positional(&args(&[
                "--workspace=ws1",
                "-g",
                "on-workspace-create",
                "echo",
                "-n",
                "ready",
            ]))
            .unwrap(),
            (
                "on-workspace-create".to_string(),
                "echo -n ready".to_string()
            )
        );
        assert_eq!(
            parse_set_hook_positional(&args(
                &["on-workspace-create", "--", "echo", "-n", "ready",]
            ))
            .unwrap(),
            (
                "on-workspace-create".to_string(),
                "echo -n ready".to_string()
            )
        );
        assert_eq!(
            parse_set_hook_options(&args(&[
                "on-workspace-create",
                "cmd",
                "--list",
                "--unset",
                "x"
            ])),
            (false, None)
        );
        assert_eq!(
            parse_set_hook_options(&args(&["--workspace", "ws1", "--list"])),
            (true, None)
        );
        assert_eq!(
            parse_set_hook_options(&args(&["--unset", "on-workspace-create"])),
            (false, Some("on-workspace-create".to_string()))
        );
        assert!(parse_set_hook_positional(&args(&["only-name"])).is_err());
        assert!(parse_set_hook_positional(&args(&["on-workspace-create", "--"])).is_err());
        assert!(is_gui_hook_event_name("on-workspace-create"));
        assert!(!is_gui_hook_event_name("pane-focus-in"));
    }

    #[test]
    fn new_pane_serializes_browser_type_and_url() {
        let (_, params) = build_new_pane_request(
            &args(&["--type", "browser", "--url", "https://example.com"]),
            |_| None,
        );
        assert_eq!(
            params,
            json!({
                "direction": "right",
                "type": "browser",
                "url": "https://example.com"
            })
        );
    }

    #[test]
    fn new_pane_serializes_env_defaults_and_command() {
        let (workspace, params) = build_new_pane_request(&args(&["--command", "claude"]), test_env);

        // The caller's surface leads; its workspace may be stale after a move.
        assert_eq!(workspace, None);
        assert_eq!(
            params,
            json!({
                "direction": "right",
                "type": "terminal",
                "surface_id": "surface:11:tab-a",
                "pane_id": "pane:11",
                "command": "claude"
            })
        );
    }

    #[test]
    fn new_pane_flags_override_env_and_preserve_raw_refs() {
        let (workspace, params) = build_new_pane_request(
            &args(&[
                "--workspace",
                "raw-workspace",
                "--surface",
                "7:tab-b",
                "--pane",
                "7",
                "--direction",
                "down",
                "--type",
                "terminal",
                "--command",
                "codex --ask-for-approval never",
            ]),
            test_env,
        );

        assert_eq!(workspace.as_deref(), Some("raw-workspace"));
        assert_eq!(
            params,
            json!({
                "direction": "down",
                "type": "terminal",
                "surface_id": "7:tab-b",
                "pane_id": "7",
                "command": "codex --ask-for-approval never"
            })
        );
    }

    #[test]
    fn new_pane_without_env_preserves_active_workspace_fallback() {
        let (workspace, params) = build_new_pane_request(&args(&[]), |_| None);

        assert_eq!(workspace, None);
        assert_eq!(
            params,
            json!({
                "direction": "right",
                "type": "terminal"
            })
        );
    }

    static CLI_ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct TestEnvGuard {
        key: &'static str,
        old: Option<std::ffi::OsString>,
    }

    impl TestEnvGuard {
        fn set(key: &'static str, value: Option<&std::ffi::OsStr>) -> Self {
            let old = std::env::var_os(key);
            match value {
                Some(val) => unsafe { std::env::set_var(key, val) },
                None => unsafe { std::env::remove_var(key) },
            }
            Self { key, old }
        }
    }

    impl Drop for TestEnvGuard {
        fn drop(&mut self) {
            match &self.old {
                Some(val) => unsafe { std::env::set_var(self.key, val) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    #[test]
    fn cli_runtime_paths_avoid_shared_tmp_and_enforce_private_modes() {
        use std::os::unix::fs::PermissionsExt as _;

        let _lock = CLI_ENV_TEST_LOCK.lock().expect("cli env test lock");
        let temp_xdg = tempfile::tempdir().expect("temp xdg runtime dir");
        std::fs::set_permissions(temp_xdg.path(), std::fs::Permissions::from_mode(0o700))
            .expect("chmod xdg 0700");
        let _xdg = TestEnvGuard::set("XDG_RUNTIME_DIR", Some(temp_xdg.path().as_os_str()));
        let expected_runtime_dir = temp_xdg.path().join("limux");

        let signal_path = super::wait_signal_path("agent/ready").expect("wait_signal_path");
        assert!(
            signal_path.starts_with(&expected_runtime_dir),
            "unexpected signal path: {}",
            signal_path.display()
        );
        assert_ne!(
            signal_path.parent(),
            Some(std::path::Path::new("/tmp")),
            "signal path must not live directly in /tmp"
        );
        let signal_parent_mode = std::fs::symlink_metadata(signal_path.parent().unwrap())
            .expect("signal dir metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(signal_parent_mode, 0o700);

        let fake_socket = expected_runtime_dir.join("test-cli.sock");
        super::with_locked_json_map(&fake_socket, "hooks-test", |map, path| {
            map.insert("pane-focus-in".to_string(), "echo ok".to_string());
            super::write_json_map(path, map)
        })
        .expect("with_locked_json_map");
        let state_path = super::cli_state_path(&fake_socket, "hooks-test");
        assert!(state_path.starts_with(&expected_runtime_dir));
        let state_mode = std::fs::symlink_metadata(&state_path)
            .expect("state file metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(state_mode, 0o600);
        let state_dir_mode = std::fs::symlink_metadata(state_path.parent().unwrap())
            .expect("state dir metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(state_dir_mode, 0o700);
    }

    #[tokio::test]
    async fn browser_screenshot_refuses_dangling_and_existing_symlink_out_targets() {
        use std::os::unix::fs::PermissionsExt as _;
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let temp = tempfile::tempdir().expect("tempdir");

        let dangling_target = temp.path().join("victim-new.png");
        let dangling_link = temp.path().join("dangling-out.png");
        std::os::unix::fs::symlink(&dangling_target, &dangling_link).expect("dangling symlink");

        let err = super::ensure_placeholder_screenshot_file(&dangling_link)
            .expect_err("dangling symlink must be refused");
        assert!(err
            .to_string()
            .contains("refusing symlinked screenshot target"));
        assert!(!dangling_target.exists());

        let existing_target = temp.path().join("victim-existing.png");
        std::fs::write(&existing_target, b"original-bytes").expect("write existing");
        let existing_link = temp.path().join("existing-out.png");
        std::os::unix::fs::symlink(&existing_target, &existing_link).expect("existing symlink");

        let err = super::ensure_placeholder_screenshot_file(&existing_link)
            .expect_err("existing symlink must be refused");
        assert!(err
            .to_string()
            .contains("refusing symlinked screenshot target"));
        assert_eq!(std::fs::read(&existing_target).unwrap(), b"original-bytes");

        let socket_path = temp.path().join("mock-browser.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind mock socket");
        let server_shot = temp.path().join("server-missing.png");
        let server_shot_str = server_shot.to_string_lossy().into_owned();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let (reader, mut writer) = stream.into_split();
            let mut line = String::new();
            let mut buf = BufReader::new(reader);
            buf.read_line(&mut line).await.expect("read request");
            let req: serde_json::Value = serde_json::from_str(&line).expect("parse request");
            let resp = json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "ok": true,
                    "format": "png",
                    "path": server_shot_str,
                }
            });
            let mut encoded = serde_json::to_string(&resp).unwrap();
            encoded.push('\n');
            writer
                .write_all(encoded.as_bytes())
                .await
                .expect("write response");
        });

        let mut client = super::Client {
            socket: socket_path,
            seq: 1,
            conn: None,
        };
        let cli_args = vec![
            "--surface".to_string(),
            "surface:1".to_string(),
            "screenshot".to_string(),
            "--out".to_string(),
            dangling_link.to_string_lossy().into_owned(),
        ];
        let cli_err = match super::run_browser(&mut client, &cli_args, false).await {
            Ok(_) => panic!("browser screenshot --out dangling symlink must fail"),
            Err(err) => err,
        };
        assert!(
            cli_err
                .to_string()
                .contains("refusing symlinked screenshot target"),
            "unexpected error: {cli_err}"
        );
        assert!(!dangling_target.exists());
        server.await.expect("mock server task");

        let created_shot = temp.path().join("nested/shot.png");
        super::ensure_placeholder_screenshot_file(&created_shot).expect("create placeholder");
        let mode = std::fs::symlink_metadata(&created_shot)
            .expect("created metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn browser_invocation_preserves_hyphenated_subcommands_and_positional_surfaces() {
        let flagged = parse_browser_invocation(
            &args(&[
                "--surface",
                "15:36580cc0-ce6a-47a7-9a02-aeea222d4d38",
                "get-url",
            ]),
            false,
        )
        .expect("flagged get-url");
        assert_eq!(
            flagged.surface.as_deref(),
            Some("15:36580cc0-ce6a-47a7-9a02-aeea222d4d38")
        );
        assert_eq!(flagged.subcommand, "get-url");
        assert!(flagged.rest.is_empty());

        let positional_surface = parse_browser_invocation(
            &args(&[
                "15:36580cc0-ce6a-47a7-9a02-aeea222d4d38",
                "get-url",
                "--json",
            ]),
            false,
        )
        .expect("positional surface get-url");
        assert_eq!(
            positional_surface.surface.as_deref(),
            Some("15:36580cc0-ce6a-47a7-9a02-aeea222d4d38")
        );
        assert_eq!(positional_surface.subcommand, "get-url");
        assert!(positional_surface.local_json);

        let workspace_scoped =
            parse_browser_invocation(&args(&["--workspace", "ws-1", "focus-webview"]), false)
                .expect("workspace focus-webview");
        assert_eq!(workspace_scoped.workspace.as_deref(), Some("ws-1"));
        assert_eq!(workspace_scoped.surface, None);
        assert_eq!(workspace_scoped.subcommand, "focus-webview");

        let click = parse_browser_invocation(
            &args(&["--surface", "surface:3", "click", "--selector", "#go"]),
            false,
        )
        .expect("click with --selector");
        assert_eq!(click.surface.as_deref(), Some("surface:3"));
        assert_eq!(click.subcommand, "click");
        assert!(click.rest.is_empty());
    }

    #[test]
    fn hooks_snapshot_args_validate_flags_equals_forms_and_duplicates() {
        let sock = PathBuf::from("/tmp/limux.sock");
        let (opts, save) = parse_hooks_snapshot_args(
            &args(&[
                "agy",
                "--workspace=ws-1",
                "--surface",
                "1:tab-a",
                "--dry-run",
                "--save-session",
                "--all-sockets",
            ]),
            sock.clone(),
        )
        .expect("valid snapshot args");
        assert_eq!(opts.agent, Some(agent_hooks::AgentKind::Agy));
        assert_eq!(opts.workspace_id.as_deref(), Some("ws-1"));
        assert_eq!(opts.surface_id.as_deref(), Some("1:tab-a"));
        assert!(opts.dry_run);
        assert!(opts.all_sockets);
        assert!(save);

        assert!(parse_hooks_snapshot_args(&args(&["--dryrun"]), sock.clone()).is_err());
        assert!(parse_hooks_snapshot_args(&args(&["--save"]), sock.clone()).is_err());
        assert!(parse_hooks_snapshot_args(&args(&["--workspace"]), sock.clone()).is_err());
        assert!(
            parse_hooks_snapshot_args(&args(&["--workspace", "--dry-run"]), sock.clone()).is_err()
        );
        assert!(parse_hooks_snapshot_args(&args(&["claude", "codex"]), sock.clone()).is_err());
        assert!(
            parse_hooks_snapshot_args(&args(&["--agent=claude", "codex"]), sock.clone()).is_err()
        );
    }
}
