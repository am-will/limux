use super::*;
use crate::gui_hooks::{GuiHook, GuiHookEvent};
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

fn pump_for(duration: Duration) {
    let deadline = std::time::Instant::now() + duration;
    while std::time::Instant::now() < deadline {
        while glib::MainContext::default().pending() {
            glib::MainContext::default().iteration(false);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// What one hook run reported, from the line its command appends.
#[derive(Debug)]
struct HookRun {
    default_action: String,
    name: String,
    id: String,
    folder: String,
    command: String,
}

/// The log every test hook appends to. Background hooks finish in any order,
/// so runs are keyed by hook name.
struct HookLog {
    path: std::path::PathBuf,
    seen: usize,
}

impl HookLog {
    fn lines(&self) -> Vec<String> {
        std::fs::read_to_string(&self.path)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Wait for exactly the hooks in `events` to run, once each, and nothing else.
    fn expect(&mut self, events: &[GuiHookEvent]) -> BTreeMap<&'static str, HookRun> {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while self.lines().len() < self.seen + events.len() {
            assert!(
                std::time::Instant::now() < deadline,
                "expected {events:?}, got {:?}",
                &self.lines()[self.seen..]
            );
            pump_for(Duration::from_millis(10));
        }
        // Give an unexpected extra hook the time to show up.
        pump_for(Duration::from_millis(300));
        let lines = self.lines();
        let new = &lines[self.seen..];
        assert_eq!(new.len(), events.len(), "expected {events:?}, got {new:?}");
        self.seen = lines.len();
        let runs: BTreeMap<_, _> = new
            .iter()
            .map(|line| {
                let fields: Vec<&str> = line.split('|').collect();
                let event = GuiHookEvent::parse(fields[0]).expect("hook name");
                let run = HookRun {
                    default_action: fields[1].to_string(),
                    name: fields[2].to_string(),
                    id: fields[3].to_string(),
                    folder: fields[4].to_string(),
                    command: fields[5].to_string(),
                };
                (event.as_str(), run)
            })
            .collect();
        let mut expected: Vec<&str> = events.iter().map(|event| event.as_str()).collect();
        expected.sort_unstable();
        assert_eq!(
            runs.keys().copied().collect::<Vec<_>>(),
            expected,
            "{new:?}"
        );
        runs
    }

    fn expect_none(&mut self) {
        self.expect(&[]);
    }
}

fn workspace_ids(state: &State) -> Vec<String> {
    state
        .borrow()
        .workspaces
        .iter()
        .map(|workspace| workspace.id.clone())
        .collect()
}

/// The one workspace in `after` that is not in `before`.
fn new_workspace(before: &[String], after: &[String]) -> String {
    let new: Vec<&String> = after.iter().filter(|id| !before.contains(id)).collect();
    assert_eq!(new.len(), 1, "{before:?} -> {after:?}");
    new[0].clone()
}

fn active_pane(state: &State) -> (String, gtk::Widget) {
    let s = state.borrow();
    let workspace = s.active_workspace().unwrap();
    let pane = find_leaf_pane(&workspace.root, gtk::Orientation::Horizontal, true);
    (workspace.id.clone(), pane)
}

fn find_descendant<T: IsA<gtk::Widget>>(
    widget: &gtk::Widget,
    matches: &impl Fn(&T) -> bool,
) -> Option<T> {
    let mut child = widget.first_child();
    while let Some(widget) = child {
        if let Some(found) = widget
            .clone()
            .downcast::<T>()
            .ok()
            .filter(matches)
            .or_else(|| find_descendant(&widget, matches))
        {
            return Some(found);
        }
        child = widget.next_sibling();
    }
    None
}

fn folder_prompt() -> gtk::Window {
    gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|widget| widget.downcast::<gtk::Window>().ok())
        .find(|window| {
            window.is_visible() && window.title().as_deref() == Some("Open Folder as Workspace")
        })
        .expect("folder prompt is shown")
}

#[test]
#[ignore = "requires GTK and Ghostty resources; exercised by xvfb-smoke-test.sh"]
fn gui_actions_run_their_hooks_and_socket_requests_do_not() {
    use GuiHookEvent::{
        AfterWorkspaceCreate, AfterWorkspaceDelete, OnWorkspaceCreate, OnWorkspaceDelete,
    };

    let temp = tempfile::tempdir().unwrap();
    for key in [
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
    ] {
        let path = temp.path().join(key);
        std::fs::create_dir_all(&path).unwrap();
        std::env::set_var(key, path);
    }
    let ghostty_config = temp.path().join("XDG_CONFIG_HOME/ghostty");
    std::fs::create_dir_all(&ghostty_config).unwrap();
    std::fs::write(
        ghostty_config.join("config"),
        "command = /bin/sh\nshell-integration = none\n",
    )
    .unwrap();
    let socket = temp.path().join("limux.sock");
    std::env::set_var("LIMUX_SOCKET", &socket);
    std::env::set_var("LIMUX_SOCKET_PATH", &socket);
    // SSH workspaces that do get created connect to a local shell.
    let ssh = temp.path().join("ssh");
    std::fs::write(&ssh, "#!/bin/sh\nexec /bin/sh\n").unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::env::set_var(
        "PATH",
        format!(
            "{}:{}",
            temp.path().display(),
            std::env::var("PATH").unwrap()
        ),
    );

    crate::prepare_ghostty_runtime();
    adw::init().unwrap();
    crate::terminal::init_ghostty();
    let app = adw::Application::builder()
        .application_id("dev.limux.GuiHookTest")
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    build_window(&app);
    let state = CONTROL_STATE.with(|slot| slot.borrow().as_ref().unwrap().clone());
    let window = state.borrow().window.clone();
    pump_for(Duration::from_millis(200));
    let config = state.borrow().config.clone();
    config.borrow_mut().workspace.prompt_for_folder_on_create = false;

    let mut log = HookLog {
        path: temp.path().join("hooks.log"),
        seen: 0,
    };
    let command = format!(
        "printf '%s|%s|%s|%s|%s|%s\\n' \"$LIMUX_HOOK\" \"$LIMUX_HOOK_DEFAULT_ACTION\" \
         \"$LIMUX_WORKSPACE_NAME\" \"$LIMUX_WORKSPACE_ID\" \"$LIMUX_WORKSPACE_FOLDER\" \
         \"$LIMUX_WORKSPACE_COMMAND\" >> {}",
        log.path.display()
    );
    let hook = |enabled, suppress_default| GuiHook {
        enabled,
        command: command.clone(),
        show_terminal: false,
        suppress_default,
    };
    let set_hook = |event: GuiHookEvent, enabled: bool, suppress_default: bool| {
        *config.borrow_mut().hooks.get_mut(event) = hook(enabled, suppress_default);
    };

    // Disabled hooks keep their settings but do not run.
    for event in GuiHookEvent::ALL {
        set_hook(event, false, false);
    }
    let before = workspace_ids(&state);
    add_workspace_from_gui(&state);
    new_workspace(&before, &workspace_ids(&state));
    log.expect_none();

    // The on-create hook runs before Limux creates the workspace, the
    // after-create hook once it exists, and is told its id.
    set_hook(OnWorkspaceCreate, true, false);
    set_hook(AfterWorkspaceCreate, true, false);
    let before = workspace_ids(&state);
    add_workspace_from_gui(&state);
    let created = new_workspace(&before, &workspace_ids(&state));
    let runs = log.expect(&[OnWorkspaceCreate, AfterWorkspaceCreate]);
    let on = &runs["on-workspace-create"];
    let after = &runs["after-workspace-create"];
    assert_eq!((on.default_action.as_str(), on.id.as_str()), ("run", ""));
    assert_eq!(after.id, created);
    assert_eq!((&after.name, &after.folder), (&on.name, &on.folder));
    assert!(!after.folder.is_empty());

    // A skipped default leaves the create to the hook. The after-create hook
    // follows only Limux's own create, so it does not run.
    set_hook(OnWorkspaceCreate, true, true);
    let before = workspace_ids(&state);
    add_workspace_from_gui(&state);
    assert_eq!(workspace_ids(&state), before);
    let runs = log.expect(&[OnWorkspaceCreate]);
    assert_eq!(runs["on-workspace-create"].default_action, "suppressed");

    // The SSH button creates workspaces too, and names their command.
    let target = || crate::ssh_hosts::SshTarget::parse("alice@example", "2222").unwrap();
    let ssh_command = "'env' 'TERM=xterm-256color' 'ssh' '-t' '-p' '2222' '--' 'alice@example'";
    connect_ssh_target(&state, target(), None);
    assert_eq!(workspace_ids(&state), before);
    let runs = log.expect(&[OnWorkspaceCreate]);
    assert_eq!(runs["on-workspace-create"].name, "SSH: alice@example");
    assert_eq!(runs["on-workspace-create"].command, ssh_command);
    set_hook(OnWorkspaceCreate, true, false);
    connect_ssh_target(&state, target(), None);
    let created = new_workspace(&before, &workspace_ids(&state));
    let runs = log.expect(&[OnWorkspaceCreate, AfterWorkspaceCreate]);
    assert_eq!(runs["after-workspace-create"].id, created);
    assert_eq!(runs["after-workspace-create"].command, ssh_command);
    // A persistent SSH workspace names the tmux command it runs instead.
    connect_ssh_target(&state, target(), Some(false));
    let runs = log.expect(&[OnWorkspaceCreate, AfterWorkspaceCreate]);
    for run in runs.values() {
        assert!(
            run.command
                .contains("'exec tmux new-session -A -s '\\''limux-"),
            "{}",
            run.command
        );
    }

    // So does dropping a tab on the new-workspace button.
    let (_, pane) = active_pane(&state);
    pane::add_terminal_tab_to_pane(&pane);
    let surface = pane::active_surface_summary(&pane).unwrap();
    let payload = format!("{}:{}", surface.pane_id, surface.tab_id);
    set_hook(OnWorkspaceCreate, true, true);
    let before = workspace_ids(&state);
    assert!(!create_workspace_for_tab(&state, &payload));
    assert_eq!(workspace_ids(&state), before);
    log.expect(&[OnWorkspaceCreate]);
    set_hook(OnWorkspaceCreate, true, false);
    assert!(create_workspace_for_tab(&state, &payload));
    let created = new_workspace(&before, &workspace_ids(&state));
    let runs = log.expect(&[OnWorkspaceCreate, AfterWorkspaceCreate]);
    assert_eq!(runs["after-workspace-create"].id, created);

    // With the folder prompt on, the hooks run only once a folder is chosen.
    config.borrow_mut().workspace.prompt_for_folder_on_create = true;
    add_workspace_from_gui(&state);
    folder_prompt().close();
    pump_for(Duration::from_millis(100));
    log.expect_none();
    let folder = temp.path().join("chosen");
    std::fs::create_dir_all(&folder).unwrap();
    let before = workspace_ids(&state);
    add_workspace_from_gui(&state);
    let prompt = folder_prompt();
    let entry = find_descendant::<gtk::Entry>(prompt.upcast_ref(), &|_| true).unwrap();
    entry.set_text(&folder.to_string_lossy());
    find_descendant::<gtk::Button>(prompt.upcast_ref(), &|button| {
        button.label().as_deref() == Some("Open")
    })
    .expect("Open button")
    .emit_clicked();
    let created = new_workspace(&before, &workspace_ids(&state));
    let runs = log.expect(&[OnWorkspaceCreate, AfterWorkspaceCreate]);
    for run in runs.values() {
        assert_eq!(
            (run.name.as_str(), run.folder.as_str()),
            ("chosen", folder.to_str().unwrap())
        );
    }
    assert_eq!(runs["after-workspace-create"].id, created);
    config.borrow_mut().workspace.prompt_for_folder_on_create = false;

    // Socket requests never run hooks, before or after.
    set_hook(OnWorkspaceDelete, true, false);
    set_hook(AfterWorkspaceDelete, true, false);
    let before = workspace_ids(&state);
    gui_hooks::handling_socket_command(|| connect_ssh_target(&state, target(), None));
    let created = new_workspace(&before, &workspace_ids(&state));
    gui_hooks::handling_socket_command(|| close_workspace_from_gui(&state, &created));
    assert_eq!(workspace_ids(&state), before);
    log.expect_none();

    // A skipped delete keeps the workspace; the after-delete hook follows only
    // Limux's own delete, so it does not run.
    let target_id = workspace_ids(&state).last().unwrap().clone();
    set_hook(OnWorkspaceDelete, true, true);
    close_workspace_from_gui(&state, &target_id);
    assert!(workspace_ids(&state).contains(&target_id));
    let runs = log.expect(&[OnWorkspaceDelete]);
    assert_eq!(runs["on-workspace-delete"].default_action, "suppressed");
    assert_eq!(runs["on-workspace-delete"].id, target_id);

    // A delete Limux carries out runs both hooks, with the closed workspace.
    set_hook(OnWorkspaceDelete, true, false);
    close_workspace_from_gui(&state, &target_id);
    assert!(!workspace_ids(&state).contains(&target_id));
    let runs = log.expect(&[OnWorkspaceDelete, AfterWorkspaceDelete]);
    for run in runs.values() {
        assert_eq!(
            (run.default_action.as_str(), run.id.as_str()),
            ("run", target_id.as_str())
        );
    }

    // Closing a workspace's last pane closes it anyway: both delete hooks
    // run, and the on-delete hook is told it cannot skip the close.
    set_hook(OnWorkspaceDelete, true, true);
    let target_id = workspace_ids(&state).last().unwrap().clone();
    let index = workspace_ids(&state)
        .iter()
        .position(|id| *id == target_id)
        .unwrap();
    switch_workspace(&state, index);
    let (workspace_id, pane) = active_pane(&state);
    assert_eq!(workspace_id, target_id);
    remove_pane(&state, &workspace_id, &pane);
    assert!(!workspace_ids(&state).contains(&target_id));
    let runs = log.expect(&[OnWorkspaceDelete, AfterWorkspaceDelete]);
    for run in runs.values() {
        assert_eq!(
            (run.default_action.as_str(), run.id.as_str()),
            ("run", target_id.as_str())
        );
    }

    window.close();
    pump_for(Duration::from_millis(300));
}
