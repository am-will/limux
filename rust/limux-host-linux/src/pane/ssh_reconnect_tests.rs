use super::*;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

struct Fixture {
    window: gtk::Window,
    panes: Vec<Rc<PaneInternals>>,
    directory: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let ssh = directory.path().join("ssh");
        std::fs::write(&ssh, "#!/bin/sh\nexec /bin/sh\n").unwrap();
        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::env::set_var(
            "PATH",
            format!(
                "{}:{}",
                directory.path().display(),
                std::env::var("PATH").unwrap()
            ),
        );
        crate::prepare_ghostty_runtime();
        gtk::init().expect("GTK display required");
        terminal::init_ghostty();
        let shortcuts = Rc::new(crate::shortcut_config::default_shortcuts());
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let panes = (0..3)
            .map(|_| {
                let shortcuts_for_callback = shortcuts.clone();
                let callbacks = Rc::new(PaneCallbacks {
                    workspace_id: "ssh-regression".into(),
                    autostart_command: Rc::default(),
                    suppress_next_autostart: Cell::new(false),
                    initial_command: RefCell::new(None),
                    on_split: Box::new(|_, _| {}),
                    on_close_pane: Box::new(|_| {}),
                    on_bell: Box::new(|_, _, _| {}),
                    on_desktop_notification: Box::new(|_, _, _, _, _| {}),
                    on_open_browser_here: Box::new(|_| {}),
                    on_open_url_in_browser: Box::new(|_, _| {}),
                    on_open_keybinds: Box::new(|_| {}),
                    current_shortcuts: Box::new(move || shortcuts_for_callback.clone()),
                    on_capture_shortcut: Rc::new(|_, _| Err(String::new())),
                    on_pwd_changed: Box::new(|_| {}),
                    on_empty: Box::new(|_, _| {}),
                    on_state_changed: Box::new(|| {}),
                    on_unread_changed: Box::new(|| {}),
                    is_pane_visible: Box::new(|_| true),
                    on_split_with_tab: Box::new(|_, _, _, _, _| {}),
                    current_config: Box::new(|| Rc::new(RefCell::new(AppConfig::default()))),
                    workspace_for_pane: Box::new(|_| None),
                });
                let pane = create_pane(callbacks, shortcuts.clone(), None, None, true);
                row.append(&pane);
                find_pane_internals(pane.upcast_ref()).unwrap()
            })
            .collect();
        let window = gtk::Window::builder()
            .title("Limux SSH reconnect regression")
            .default_width(1200)
            .default_height(700)
            .child(&row)
            .build();
        window.present();
        Self {
            window,
            panes,
            directory,
        }
    }

    fn add_ssh(&self, pane: usize) -> String {
        let connection = crate::ssh_session::SshConnection::new(
            crate::ssh_hosts::SshTarget::parse("fixture.invalid", "").unwrap(),
            true,
        );
        add_terminal_tab_inner(
            &self.panes[pane],
            Some(self.directory.path().to_str().unwrap()),
            Some(TerminalTabOptions {
                id: None,
                custom_name: None,
                pinned: false,
                cwd: Some(self.directory.path().to_str().unwrap()),
                agent: None,
                ssh: Some(connection),
            }),
        );
        let id = self.panes[pane]
            .tab_state
            .borrow()
            .active_tab
            .clone()
            .unwrap();
        pump_until(|| {
            let (_, handle) =
                terminal_handle_for_surface(self.panes[pane].pane_outer.upcast_ref(), Some(&id))
                    .unwrap();
            handle.health().realized && !handle.health().process_exited
        });
        id
    }

    fn generation(&self, pane: usize, id: &str) -> Rc<()> {
        let tabs = self.panes[pane].tab_state.borrow();
        let entry = tabs.tabs.iter().find(|entry| entry.id == id).unwrap();
        let TabKind::Terminal { state } = &entry.kind else {
            unreachable!()
        };
        state.generation.clone()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for pane in &self.panes {
            retire_pane(pane.pane_outer.upcast_ref());
        }
        pump_for(Duration::from_millis(150));
        self.window.close();
    }
}

fn pump_for(duration: Duration) {
    let until = Instant::now() + duration;
    while Instant::now() < until {
        for _ in 0..100 {
            if !glib::MainContext::default().pending() {
                break;
            }
            glib::MainContext::default().iteration(false);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn pump_until(condition: impl Fn() -> bool) {
    let until = Instant::now() + Duration::from_secs(10);
    while !condition() && Instant::now() < until {
        pump_for(Duration::from_millis(10));
    }
    assert!(condition(), "GTK reconnect condition timed out");
}

#[test]
#[ignore = "requires X11, xdotool, and Ghostty resources"]
fn reconnect_preserves_neighbor_keyboard_focus_and_restores_its_own() {
    let fixture = Fixture::new();
    fixture.add_ssh(0);
    let ssh = fixture.add_ssh(0);
    let neighbor = fixture.add_ssh(1);
    let (_, neighbor_handle) =
        terminal_handle_for_surface(fixture.panes[1].pane_outer.upcast_ref(), Some(&neighbor))
            .unwrap();
    let output = std::process::Command::new("xdotool")
        .args(["search", "--name", "^Limux SSH reconnect regression$"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let window_id = String::from_utf8(output.stdout).unwrap();
    assert!(std::process::Command::new("xdotool")
        .args(["windowfocus", "--sync", window_id.lines().next().unwrap()])
        .status()
        .unwrap()
        .success());
    neighbor_handle.focus_surface();
    pump_for(Duration::from_millis(100));
    let generation = fixture.generation(0, &ssh);
    respawn_terminal_tab(&ssh, &generation);
    pump_until(|| !Rc::ptr_eq(&fixture.generation(0, &ssh), &generation));
    pump_for(Duration::from_millis(150));
    let neighbor_content = fixture.panes[1].tab_state.borrow().tabs[0].content.clone();
    assert!(
        terminal::focus_is_within(&neighbor_content),
        "reconnecting a neighboring pane stole keyboard focus"
    );
    let proof = fixture.directory.path().join("typed-in-neighbor");
    let mut typing = std::process::Command::new("xdotool")
        .args([
            "type",
            "--clearmodifiers",
            "--delay",
            "5",
            &format!("printf neighbor > '{}'", proof.display()),
        ])
        .spawn()
        .unwrap();
    while typing.try_wait().unwrap().is_none() {
        pump_for(Duration::from_millis(10));
    }
    assert!(typing.wait().unwrap().success());
    assert!(std::process::Command::new("xdotool")
        .args(["key", "--clearmodifiers", "Return"])
        .status()
        .unwrap()
        .success());
    pump_until(|| proof.exists());
    assert_eq!(std::fs::read_to_string(proof).unwrap(), "neighbor");

    let (_, handle) =
        terminal_handle_for_surface(fixture.panes[0].pane_outer.upcast_ref(), Some(&ssh)).unwrap();
    handle.focus_surface();
    pump_for(Duration::from_millis(50));
    let generation = fixture.generation(0, &ssh);
    respawn_terminal_tab(&ssh, &generation);
    pump_until(|| !Rc::ptr_eq(&fixture.generation(0, &ssh), &generation));
    let current = fixture.panes[0]
        .tab_state
        .borrow()
        .tabs
        .iter()
        .find(|entry| entry.id == ssh)
        .unwrap()
        .content
        .clone();
    assert!(
        terminal::focus_is_within(&current),
        "focused SSH reconnect lost keyboard focus"
    );

    let generation = fixture.generation(0, &ssh);
    respawn_terminal_tab(&ssh, &generation);
    neighbor_handle.focus_surface();
    pump_until(|| !Rc::ptr_eq(&fixture.generation(0, &ssh), &generation));
    assert!(
        terminal::focus_is_within(&neighbor_content),
        "a deliberate focus change while reconnecting must take precedence"
    );
}

#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn reconnect_and_deferred_tab_moves_keep_one_current_widget() {
    let fixture = Fixture::new();
    for respawn_first in [true, false] {
        let id = fixture.add_ssh(0);
        let old = fixture.panes[0].tab_state.borrow().tabs[0].content.clone();
        let generation = fixture.generation(0, &id);
        if respawn_first {
            respawn_terminal_tab(&id, &generation);
        }
        assert!(move_tab_to_pane(
            fixture.panes[0].pane_outer.upcast_ref(),
            &id,
            fixture.panes[1].pane_outer.upcast_ref()
        ));
        if !respawn_first {
            respawn_terminal_tab(&id, &generation);
        }
        assert!(move_tab_to_pane(
            fixture.panes[1].pane_outer.upcast_ref(),
            &id,
            fixture.panes[2].pane_outer.upcast_ref()
        ));
        pump_until(|| !Rc::ptr_eq(&fixture.generation(2, &id), &generation));
        pump_for(Duration::from_millis(200));
        let current = fixture.panes[2].tab_state.borrow().tabs[0].content.clone();
        assert_ne!(current, old);
        assert_eq!(
            old.parent(),
            None,
            "obsolete terminal remained parented after reconnect and move"
        );
        assert_eq!(
            fixture.panes[2].content_stack.child_by_name(&id),
            Some(current.clone())
        );
        assert_eq!(
            current.parent(),
            Some(fixture.panes[2].content_stack.clone().upcast())
        );
        assert_eq!(fixture.panes[2].content_stack.pages().n_items(), 1);
        for pane in &fixture.panes[..2] {
            assert_eq!(pane.content_stack.pages().n_items(), 0);
        }
        remove_terminal_tab(&fixture.panes[2], &id);
        pump_for(Duration::from_millis(150));
    }
}
