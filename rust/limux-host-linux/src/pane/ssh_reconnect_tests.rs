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

    fn add_local(&self, pane: usize, command: &str) -> String {
        *self.panes[pane].callbacks.initial_command.borrow_mut() = Some(command.to_string());
        add_terminal_tab_inner(
            &self.panes[pane],
            Some(self.directory.path().to_str().unwrap()),
            Some(TerminalTabOptions {
                id: None,
                custom_name: None,
                pinned: false,
                cwd: Some(self.directory.path().to_str().unwrap()),
                agent: None,
                ssh: None,
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
fn missing_tab_is_not_a_local_image_destination() {
    assert!(image_paste_destination_for_tab("missing-tab").is_err());
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

#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn image_paste_follows_moves_and_cancels_on_reconnect() {
    let fixture = Fixture::new();
    let root = fixture.directory.path();
    std::fs::write(
        root.join("ssh"),
        r#"#!/usr/bin/python3
import os, sys, pathlib, tty, time, signal
root = pathlib.Path(__file__).parent
if '-T' in sys.argv:
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    data = sys.stdin.buffer.read()
    (root / 'uploaded').write_bytes(data)
    with (root / 'calls').open('a') as f: f.write('upload\n')
    while not (root / 'release').exists(): time.sleep(.01)
    if (root / 'fail').exists(): sys.exit(17)
    print('/tmp/limux-paste.ABCDEF1234/image.png')
else:
    tty.setraw(0)
    os.write(1, b'\x1b[?2004hREADY')
    while True:
        data = os.read(0, 4096)
        if not data: break
        with (root / ('input-' + os.environ['LIMUX_TAB_ID'])).open('ab') as f: f.write(data)
"#,
    )
    .unwrap();
    let id = fixture.add_ssh(0);
    let (_, handle) =
        terminal_handle_for_surface(fixture.panes[0].pane_outer.upcast_ref(), Some(&id)).unwrap();
    pump_until(|| {
        handle
            .read_viewport_text()
            .is_some_and(|s| s.contains("READY"))
    });
    let clipboard = gtk::gdk::Display::default().unwrap().clipboard();
    let texture = gtk::gdk::MemoryTexture::new(
        1,
        1,
        gtk::gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from_static(&[12, 34, 56, 255]),
        4,
    );
    clipboard.set_texture(&texture);
    assert!(handle.perform_binding_action("paste_from_clipboard"));
    pump_until(|| root.join("uploaded").exists());
    assert_eq!(
        std::fs::read(root.join("uploaded")).unwrap(),
        texture.save_to_png_bytes().as_ref()
    );
    assert!(handle.perform_binding_action("paste_from_clipboard"));
    pump_for(Duration::from_millis(100));
    assert_eq!(
        std::fs::read_to_string(root.join("calls")).unwrap(),
        "upload\n"
    );
    assert!(move_tab_to_pane(
        fixture.panes[0].pane_outer.upcast_ref(),
        &id,
        fixture.panes[1].pane_outer.upcast_ref()
    ));
    std::fs::write(root.join("release"), "").unwrap();
    let input = root.join(format!("input-{id}"));
    pump_until(|| input.exists());
    assert_eq!(
        std::fs::read(&input).unwrap(),
        b"\x1b[200~/tmp/limux-paste.ABCDEF1234/image.png \x1b[201~"
    );

    // Text still follows Ghostty's ordinary clipboard path.
    clipboard.set_text("ordinary text");
    handle.perform_binding_action("paste_from_clipboard");
    pump_until(|| {
        std::fs::read(&input)
            .unwrap()
            .ends_with(b"ordinary text\x1b[201~")
    });
    let before = std::fs::read(&input).unwrap();
    clipboard.set_texture(&texture);
    std::fs::write(root.join("fail"), "").unwrap();
    fn paste_button(widget: &gtk::Widget) -> Option<gtk::Button> {
        if let Some(button) = widget.downcast_ref::<gtk::Button>() {
            if button.label().as_deref() == Some("Paste") {
                return Some(button.clone());
            }
        }
        let mut child = widget.first_child();
        while let Some(widget) = child {
            if let Some(button) = paste_button(&widget) {
                return Some(button);
            }
            child = widget.next_sibling();
        }
        None
    }
    let menu = handle.build_context_menu_for_test();
    paste_button(menu.upcast_ref())
        .expect("Paste menu action")
        .emit_clicked();
    menu.unparent();
    pump_until(|| {
        std::fs::read_to_string(root.join("calls"))
            .unwrap()
            .lines()
            .count()
            == 2
    });
    pump_for(Duration::from_millis(150));
    assert_eq!(
        std::fs::read(&input).unwrap(),
        before,
        "failed upload injected input"
    );
    std::fs::remove_file(root.join("fail")).unwrap();
    std::fs::remove_file(root.join("release")).unwrap();
    handle.perform_binding_action("paste_from_clipboard");
    pump_until(|| {
        std::fs::read_to_string(root.join("calls"))
            .unwrap()
            .lines()
            .count()
            == 3
    });
    let generation = fixture.generation(1, &id);
    respawn_terminal_tab(&id, &generation);
    pump_until(|| !Rc::ptr_eq(&fixture.generation(1, &id), &generation));
    std::fs::write(root.join("release"), "").unwrap();
    pump_for(Duration::from_millis(250));
    assert_eq!(
        std::fs::read(&input).unwrap(),
        before,
        "obsolete upload reached replacement terminal"
    );
    let (_, replacement) =
        terminal_handle_for_surface(fixture.panes[1].pane_outer.upcast_ref(), Some(&id)).unwrap();
    pump_until(|| {
        replacement
            .read_viewport_text()
            .is_some_and(|s| s.contains("READY"))
    });
    std::fs::remove_file(root.join("release")).unwrap();
    replacement.perform_binding_action("paste_from_clipboard");
    pump_until(|| {
        std::fs::read_to_string(root.join("calls"))
            .unwrap()
            .lines()
            .count()
            == 4
    });
    remove_terminal_tab(&fixture.panes[1], &id);
    std::fs::write(root.join("release"), "").unwrap();
    pump_for(Duration::from_millis(250));
    assert_eq!(
        std::fs::read(&input).unwrap(),
        before,
        "closed terminal received late input"
    );
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn image_payloads(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8(bytes.to_vec()).expect("pty input is utf-8");
    let mut payloads = Vec::new();
    let mut rest = text.as_str();
    while let Some(start) = rest.find("\u{1b}[200~") {
        rest = &rest[start + "\u{1b}[200~".len()..];
        let end = rest
            .find("\u{1b}[201~")
            .expect("bracketed paste terminator");
        let payload = &rest[..end];
        if let Some(path) = payload.strip_suffix(' ') {
            if path.starts_with("/tmp/limux-paste.") {
                assert!(!path.ends_with(' '), "{payload:?}");
                assert!(!path.contains('\n') && !path.contains('\r'), "{payload:?}");
                payloads.push(path.to_string());
            }
        }
        rest = &rest[end + "\u{1b}[201~".len()..];
    }
    payloads
}

fn click_context_paste(handle: &terminal::TerminalHandle) {
    fn paste_button(widget: &gtk::Widget) -> Option<gtk::Button> {
        if let Some(button) = widget.downcast_ref::<gtk::Button>() {
            if button.label().as_deref() == Some("Paste") {
                return Some(button.clone());
            }
        }
        let mut child = widget.first_child();
        while let Some(widget) = child {
            if let Some(button) = paste_button(&widget) {
                return Some(button);
            }
            child = widget.next_sibling();
        }
        None
    }
    let menu = handle.build_context_menu_for_test();
    paste_button(menu.upcast_ref())
        .expect("Paste menu action")
        .emit_clicked();
    menu.unparent();
}

fn assert_private_png(path: &str, png: &[u8]) {
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::read(path).unwrap(), png);
    assert_eq!(
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(std::path::Path::new(path).parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}

#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn local_image_paste_inserts_one_private_path() {
    let fixture = Fixture::new();
    let root = fixture.directory.path();
    let script_path = root.join("record-local.py");
    std::fs::write(
        &script_path,
        r#"#!/usr/bin/python3
import os, sys, tty
tty.setraw(0)
os.write(1, b'\x1b[?2004hREADY')
path = os.path.join(sys.argv[1], 'input-' + os.environ['LIMUX_TAB_ID'])
while True:
    data = os.read(0, 4096)
    if not data:
        break
    with open(path, 'ab') as handle:
        handle.write(data)
"#,
    )
    .unwrap();
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let command = format!(
        "{} {}",
        shell_quote(script_path.to_str().unwrap()),
        shell_quote(root.to_str().unwrap())
    );
    let id = fixture.add_local(0, &command);
    assert_eq!(
        image_paste_destination_for_tab(&id),
        Ok(terminal::ImagePasteDestination::Local)
    );
    let (_, handle) =
        terminal_handle_for_surface(fixture.panes[0].pane_outer.upcast_ref(), Some(&id)).unwrap();
    pump_until(|| {
        handle
            .read_viewport_text()
            .is_some_and(|text| text.contains("READY"))
    });
    let clipboard = gtk::gdk::Display::default().unwrap().clipboard();
    let texture = gtk::gdk::MemoryTexture::new(
        1,
        1,
        gtk::gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from_static(&[12, 34, 56, 255]),
        4,
    );
    let png = texture.save_to_png_bytes();
    clipboard.set_texture(&texture);
    assert!(handle.perform_binding_action("paste_from_clipboard"));
    assert!(handle.perform_binding_action("paste_from_clipboard"));
    let input = root.join(format!("input-{id}"));
    pump_until(|| input.exists());
    pump_for(Duration::from_millis(200));
    let pasted = std::fs::read(&input).unwrap();
    assert!(!pasted.contains(&b'\r'), "paste inserted Enter");
    let paths = image_payloads(&pasted);
    assert_eq!(paths.len(), 1, "duplicate paste inserted {pasted:?}");
    assert_private_png(&paths[0], png.as_ref());
    let viewport = handle.read_viewport_text().unwrap_or_default();
    assert!(
        !viewport.contains("persistent SSH") && !viewport.contains("Image upload requires"),
        "{viewport}"
    );

    assert!(move_tab_to_pane(
        fixture.panes[0].pane_outer.upcast_ref(),
        &id,
        fixture.panes[1].pane_outer.upcast_ref()
    ));
    assert_eq!(
        image_paste_destination_for_tab(&id),
        Ok(terminal::ImagePasteDestination::Local)
    );
    clipboard.set_text("ordinary text");
    let (_, handle) =
        terminal_handle_for_surface(fixture.panes[1].pane_outer.upcast_ref(), Some(&id)).unwrap();
    handle.perform_binding_action("paste_from_clipboard");
    pump_until(|| {
        std::fs::read(&input)
            .unwrap()
            .ends_with(b"ordinary text\x1b[201~")
    });

    // A GValue texture does not advertise image MIME types. Offer the PNG
    // bytes beside text, which is how a copied screenshot arrives.
    let beside = "should-not-appear".to_value();
    let provider = gtk::gdk::ContentProvider::new_union(&[
        gtk::gdk::ContentProvider::for_bytes("image/png", &png),
        gtk::gdk::ContentProvider::for_value(&beside),
    ]);
    clipboard.set_content(Some(&provider)).unwrap();
    handle.perform_binding_action("paste_from_clipboard");
    pump_until(|| image_payloads(&std::fs::read(&input).unwrap()).len() == 2);
    let mixed = std::fs::read(&input).unwrap();
    assert!(!String::from_utf8_lossy(&mixed).contains("should-not-appear"));
    assert_private_png(&image_payloads(&mixed)[1], png.as_ref());

    let rejected = std::fs::read(&input).unwrap();
    let wide = 4096_i32;
    let tall = 4097_i32;
    let stride = (wide * 4) as usize;
    let pixels = glib::Bytes::from_owned(vec![0_u8; stride * tall as usize]);
    let oversized = gtk::gdk::MemoryTexture::new(
        wide,
        tall,
        gtk::gdk::MemoryFormat::R8g8b8a8,
        &pixels,
        stride,
    );
    clipboard.set_texture(&oversized);
    handle.perform_binding_action("paste_from_clipboard");
    pump_for(Duration::from_millis(400));
    assert_eq!(
        std::fs::read(&input).unwrap(),
        rejected,
        "oversized image inserted a path"
    );

    clipboard.set_texture(&texture);
    click_context_paste(&handle);
    pump_until(|| image_payloads(&std::fs::read(&input).unwrap()).len() == 3);
    let final_input = std::fs::read(&input).unwrap();
    assert!(!final_input.contains(&b'\r'));
    let paths = image_payloads(&final_input);
    assert_eq!(paths.len(), 3);
    for path in &paths {
        assert_private_png(path, png.as_ref());
    }
    remove_terminal_tab(&fixture.panes[1], &id);
    pump_for(Duration::from_millis(200));
    assert!(image_paste_destination_for_tab(&id).is_err());
    for path in &paths {
        assert_eq!(std::fs::read(path).unwrap(), png.as_ref());
        let directory = std::path::Path::new(path).parent().unwrap();
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
