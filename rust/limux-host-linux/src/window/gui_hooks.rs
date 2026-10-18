//! Running GUI hooks for sidebar and shortcut actions, and the socket
//! handler that reads and changes them. The hook model lives in
//! `crate::gui_hooks`.

use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitStatus, Stdio};

use serde_json::{json, Value};

use super::*;
use crate::control_bridge::GuiHookRequest;
use crate::gui_hooks::{
    hook_environment, terminal_command, GuiHook, GuiHookContext, GuiHookEvent,
    INHERITED_IDENTITY_VARS,
};
use crate::terminal::{TerminalCallbacks, TerminalOptions};

/// Run the hook configured for `event`, if any. Returns whether Limux should
/// still run its own action.
pub(super) fn run_gui_hook(state: &State, event: GuiHookEvent, context: &GuiHookContext) -> bool {
    let hook = {
        let s = state.borrow();
        let hook = s.config.borrow().hooks.configured(event).cloned();
        hook
    };
    let Some(hook) = hook else {
        return true;
    };

    let mut env = hook_environment(event, &hook, context);
    env.extend(crate::terminal_child_environment_overrides());
    if let Some(socket) = limux_control::socket_path::resolve_socket_path(
        None,
        limux_control::socket_path::SocketMode::Runtime,
    )
    .to_str()
    {
        env.push(("LIMUX_SOCKET".to_string(), socket.to_string()));
    }
    let unset: Vec<&str> = INHERITED_IDENTITY_VARS
        .iter()
        .copied()
        .filter(|name| !env.iter().any(|(set, _)| set == name))
        .collect();
    let cwd = context
        .folder
        .as_deref()
        .map(PathBuf::from)
        .filter(|folder| folder.is_dir())
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("/"));

    if hook.show_terminal {
        open_hook_terminal(state, event, &hook, env, &unset, &cwd);
    } else {
        spawn_hook(state, event, &hook, env, &unset, &cwd);
    }
    !hook.suppress_default
}

fn spawn_hook(
    state: &State,
    event: GuiHookEvent,
    hook: &GuiHook,
    env: Vec<(String, String)>,
    unset: &[&str],
    cwd: &Path,
) {
    let mut command = Command::new("bash");
    command
        .arg("-c")
        .arg(&hook.command)
        .current_dir(cwd)
        .stdin(Stdio::null());
    for name in unset {
        command.env_remove(name);
    }
    command.envs(env);

    let child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            let detail = format!("Failed to run the {} hook: {err}", event.as_str());
            eprintln!("limux: {detail}");
            show_runtime_error(state, "Failed to run GUI hook", &detail);
            return;
        }
    };

    // The watch reaps the child on the main loop, so a failure can be shown.
    let state = state.clone();
    let pid = glib::Pid(child.id() as i32);
    glib::child_watch_add_local(pid, move |_, wait_status| {
        let status = ExitStatus::from_raw(wait_status);
        if !status.success() {
            let detail = format!("The {} hook {status}.", event.as_str());
            eprintln!("limux: {detail}");
            show_runtime_error(&state, "GUI hook failed", &detail);
        }
    });
}

/// Show the hook in its own terminal window, which closes when the hook's
/// shell exits.
fn open_hook_terminal(
    state: &State,
    event: GuiHookEvent,
    hook: &GuiHook,
    env: Vec<(String, String)>,
    unset: &[&str],
    cwd: &Path,
) {
    let parent = state.borrow().window.clone();
    let window = gtk::Window::builder()
        .title(format!("Limux hook: {}", event.as_str()))
        .default_width(820)
        .default_height(460)
        .transient_for(&parent)
        .build();

    let on_close = {
        let window = window.downgrade();
        // Ghostty reports the exit from inside its surface map; closing the
        // window frees the surface, so leave that callback first.
        Box::new(move |_| {
            let window = window.clone();
            glib::idle_add_local_once(move || {
                if let Some(window) = window.upgrade() {
                    window.close();
                }
            });
        })
    };
    let terminal = crate::terminal::create_terminal(
        cwd.to_str(),
        TerminalOptions {
            startup_command: Some(terminal_command(&hook.command, unset)),
            extra_env: env,
            ..TerminalOptions::default()
        },
        TerminalCallbacks {
            on_close,
            ..TerminalCallbacks::disconnected()
        },
    );
    window.set_child(Some(&terminal.root));
    {
        let handle = terminal.handle.clone();
        window.connect_close_request(move |_| {
            handle.shutdown();
            glib::Propagation::Proceed
        });
    }
    window.present();
    let handle = terminal.handle.clone();
    glib::idle_add_local_once(move || {
        handle.focus_surface();
    });
}

/// What a delete hook is told about the workspace it is about to close.
pub(super) fn workspace_hook_context(state: &State, workspace_id: &str) -> GuiHookContext {
    let s = state.borrow();
    let Some(workspace) = s.workspaces.iter().find(|w| w.id == workspace_id) else {
        return GuiHookContext::default();
    };
    GuiHookContext {
        workspace_id: Some(workspace.id.clone()),
        workspace_name: Some(workspace.name.clone()),
        folder: workspace
            .folder_path
            .clone()
            .or_else(|| workspace.cwd.borrow().clone()),
        group_id: s
            .sidebar
            .group_of(workspace_id)
            .map(|group| group.id.clone()),
    }
}

pub(super) fn handle_gui_hook_request(
    state: &State,
    request: GuiHookRequest,
) -> Result<Value, BridgeError> {
    let config = state.borrow().config.clone();
    let mut updated = config.borrow().clone();
    let event = match request {
        GuiHookRequest::List => {
            let hooks: Vec<Value> = GuiHookEvent::ALL
                .into_iter()
                .map(|event| updated.hooks.describe(event))
                .collect();
            return Ok(json!({ "hooks": hooks }));
        }
        GuiHookRequest::Set { event, update } => {
            let hook = updated.hooks.get_mut(event);
            update.apply(hook);
            if !hook.is_configured() {
                return Err(BridgeError::invalid_params(format!(
                    "the {} hook has no command; set one with command",
                    event.as_str()
                )));
            }
            event
        }
        GuiHookRequest::Clear { event } => {
            *updated.hooks.get_mut(event) = GuiHook::default();
            event
        }
    };
    app_config::save(&updated).map_err(BridgeError::internal)?;
    let payload = updated.hooks.describe(event);
    *config.borrow_mut() = updated;
    Ok(payload)
}
