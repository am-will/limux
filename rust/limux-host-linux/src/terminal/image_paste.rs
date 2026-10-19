//! Explicit user paste only. OSC 52 reads continue through clipboard.rs.
use super::*;
use crate::ssh_transfer::{Upload, MAX_PIXELS};

#[derive(Clone, Copy, PartialEq, Eq)]
struct Ticket {
    identity: SurfaceIdentity,
    serial: u64,
}

struct Pending {
    ticket: Ticket,
    cancellable: gtk::gio::Cancellable,
    upload: Option<Rc<Upload>>,
    timer: Option<glib::SourceId>,
}

impl Drop for Pending {
    fn drop(&mut self) {
        if let Some(timer) = self.timer.take() {
            timer.remove();
        }
        self.cancellable.cancel();
        if let Some(upload) = self.upload.take() {
            upload.cancel();
        }
    }
}

thread_local! {
    static PENDING: RefCell<HashMap<usize, Pending>> = RefCell::new(HashMap::new());
    static SERIAL: Cell<u64> = const { Cell::new(0) };
}

fn current(ticket: Ticket) -> bool {
    current_surface_identity(ticket.identity.surface_key) == Some(ticket.identity)
        && PENDING.with_borrow(|pending| {
            pending
                .get(&ticket.identity.surface_key)
                .is_some_and(|request| request.ticket == ticket)
        })
}

pub(super) fn cancel_surface(surface_key: usize) {
    // Drop outside the registry borrow: cancellation may finish an upload.
    let request = PENDING.with_borrow_mut(|pending| pending.remove(&surface_key));
    drop(request);
}

fn message(identity: SurfaceIdentity, text: &str) {
    let overlay = SURFACE_MAP.with_borrow(|map| {
        map.get(&identity.surface_key)
            .filter(|entry| entry.identity == identity)
            .map(|entry| entry.toast_overlay.clone())
    });
    if let Some(overlay) = overlay {
        show_terminal_toast(&overlay, text, Duration::from_secs(6));
    }
}

fn complete(ticket: Ticket, result: Result<String, String>) {
    if !current(ticket) {
        return;
    }
    cancel_surface(ticket.identity.surface_key);
    match result {
        Ok(path) => {
            // The validated remote path has no shell metacharacters. Use the
            // same paste API as text input, preserving bracketed-paste mode.
            let text = format!("{path} ");
            unsafe {
                ghostty_surface_text(
                    ticket.identity.surface_key as ghostty_surface_t,
                    text.as_ptr().cast(),
                    text.len(),
                );
            }
            message(ticket.identity, "Image uploaded");
        }
        Err(error) => message(ticket.identity, &error),
    }
}

pub(super) fn try_begin(
    surface: Option<ghostty_surface_t>,
    callbacks: &Rc<RefCell<TerminalCallbacks>>,
) -> bool {
    let Some(surface) = surface else {
        return false;
    };
    let Some(identity) = current_surface_identity(surface as usize) else {
        return false;
    };
    let Some(display) = gtk::gdk::Display::default() else {
        return false;
    };
    let clipboard = display.clipboard();
    let formats = clipboard.formats();
    if !formats.contains_type(gtk::gdk::Texture::static_type())
        && !clipboard_formats_include_image(formats.mime_types().iter().map(|mime| mime.as_str()))
    {
        return false;
    }
    let Some(target) = (callbacks.borrow().image_paste_target)() else {
        message(identity, "Image upload requires a persistent SSH tab");
        return true;
    };
    let busy = PENDING
        .with_borrow(|pending| pending.contains_key(&identity.surface_key) || pending.len() >= 4);
    if busy {
        message(
            identity,
            "An image upload is already pending; try again when it finishes",
        );
        return true;
    }
    let ticket = Ticket {
        identity,
        serial: SERIAL.with(|serial| {
            let next = serial.get().wrapping_add(1);
            serial.set(next);
            next
        }),
    };
    let cancellable = gtk::gio::Cancellable::new();
    let timer = glib::timeout_add_local_once(Duration::from_secs(45), move || {
        PENDING.with_borrow_mut(|pending| {
            if let Some(request) = pending
                .get_mut(&identity.surface_key)
                .filter(|r| r.ticket == ticket)
            {
                request.timer.take();
            }
        });
        complete(ticket, Err("Image paste timed out".into()));
    });
    PENDING.with_borrow_mut(|pending| {
        pending.insert(
            identity.surface_key,
            Pending {
                ticket,
                cancellable: cancellable.clone(),
                upload: None,
                timer: Some(timer),
            },
        )
    });
    message(identity, "Uploading clipboard image…");
    clipboard.read_texture_async(Some(&cancellable), move |result| {
        if !current(ticket) {
            return;
        }
        let texture = match result {
            Ok(Some(texture)) => texture,
            _ => {
                complete(ticket, Err("Could not read the clipboard image".into()));
                return;
            }
        };
        if texture.width() <= 0
            || texture.height() <= 0
            || texture.width() as u64 * texture.height() as u64 > MAX_PIXELS
        {
            complete(ticket, Err("Clipboard image exceeds 16 megapixels".into()));
            return;
        }
        // GdkTexture is immutable and Send+Sync. Encoding large screenshots
        // must not block the GTK event loop.
        glib::MainContext::default().spawn_local(async move {
            let png = gtk::gio::spawn_blocking(move || texture.save_to_png_bytes()).await;
            if !current(ticket) {
                return;
            }
            let Ok(png) = png else {
                complete(ticket, Err("Could not encode clipboard image".into()));
                return;
            };
            match Upload::start(&target, png, flatpak::enabled(), move |result| {
                complete(ticket, result)
            }) {
                Ok(upload) => PENDING.with_borrow_mut(|pending| {
                    if let Some(request) = pending.get_mut(&identity.surface_key) {
                        request.upload = Some(upload);
                    }
                }),
                Err(error) => complete(ticket, Err(error)),
            }
        });
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "opt-in localhost SSH/Codex test; see docs/ssh.md"]
    fn live_ssh_agent_image_attachment() {
        let command =
            std::env::var("LIMUX_IMAGE_TEST_COMMAND").expect("disposable SSH agent command");
        crate::prepare_ghostty_runtime();
        gtk::init().unwrap();
        init_ghostty();
        let mut callbacks = TerminalCallbacks::disconnected();
        callbacks.image_paste_target =
            Box::new(|| Some(crate::ssh_hosts::SshTarget::parse("localhost", "").unwrap()));
        let terminal = create_terminal(
            None,
            TerminalOptions {
                startup_command: Some(command),
                ..Default::default()
            },
            callbacks,
        );
        let window = gtk::Window::builder()
            .default_width(1200)
            .default_height(800)
            .child(&terminal.root)
            .build();
        window.present();
        let handle = terminal.handle;
        let wait = |condition: &dyn Fn() -> bool| {
            let deadline = std::time::Instant::now() + Duration::from_secs(45);
            while !condition() {
                for _ in 0..100 {
                    if !glib::MainContext::default().pending() {
                        break;
                    }
                    glib::MainContext::default().iteration(false);
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "agent timeout: {:?}",
                    handle.read_viewport_text()
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        wait(&|| {
            handle
                .read_viewport_text()
                .is_some_and(|s| s.contains("OpenAI Codex") && s.contains("Ask Codex"))
        });
        let until = std::time::Instant::now() + Duration::from_secs(2);
        wait(&|| std::time::Instant::now() >= until);
        let clipboard = gtk::gdk::Display::default().unwrap().clipboard();
        let texture = gtk::gdk::MemoryTexture::new(
            1,
            1,
            gtk::gdk::MemoryFormat::R8g8b8a8,
            &glib::Bytes::from_static(&[255, 0, 0, 255]),
            4,
        );
        clipboard.set_texture(&texture);
        handle.perform_binding_action("paste_from_clipboard");
        wait(&|| {
            handle
                .read_viewport_text()
                .is_some_and(|s| s.contains("[Image #1]"))
        });
        eprintln!("PASS: real SSH/tmux/Codex accepted the uploaded image attachment");
        clipboard.set_text("image transfer regression text");
        handle.perform_binding_action("paste_from_clipboard");
        wait(&|| {
            handle
                .read_viewport_text()
                .is_some_and(|s| s.contains("image transfer regression text"))
        });
        handle.perform_binding_action("select_all");
        assert!(handle.copy_selection_to_clipboard());
        let copied = Rc::new(RefCell::new(None));
        let result = copied.clone();
        clipboard.read_text_async(gtk::gio::Cancellable::NONE, move |text| {
            *result.borrow_mut() = Some(text.unwrap().unwrap().to_string())
        });
        wait(&|| copied.borrow().is_some());
        assert!(copied
            .borrow()
            .as_ref()
            .unwrap()
            .contains("image transfer regression text"));
        eprintln!("PASS: remote agent text paste and copy; no model turn submitted");
        handle.shutdown();
        window.close();
    }
}
