//! Explicit user paste only. OSC 52 reads continue through clipboard.rs.
use super::image_staging::StagedImage;
use super::*;
use crate::ssh_transfer::{Upload, MAX_PIXELS};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ImagePasteDestination {
    Local,
    Ssh(crate::ssh_hosts::SshTarget),
}

enum PasteOutput {
    Local(String),
    Uploaded(String),
}

impl PasteOutput {
    fn path(&self) -> &str {
        match self {
            Self::Local(path) | Self::Uploaded(path) => path,
        }
    }

    fn announcement(&self) -> &'static str {
        match self {
            Self::Local(_) => "Image ready",
            Self::Uploaded(_) => "Image uploaded",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Ticket {
    identity: SurfaceIdentity,
    serial: u64,
}

struct Pending {
    ticket: Ticket,
    cancellable: gtk::gio::Cancellable,
    upload: Option<Rc<Upload>>,
    staged: Option<StagedImage>,
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

/// Mark a staged file as kept before pending-operation teardown can drop it.
fn settle_staged(staged: &mut Option<StagedImage>, deliver: bool) {
    if deliver {
        if let Some(staged) = staged {
            staged.retain();
        }
    }
}

fn complete(ticket: Ticket, result: Result<PasteOutput, String>) {
    if !current(ticket) {
        return;
    }
    if result.is_ok() {
        PENDING.with_borrow_mut(|pending| {
            if let Some(request) = pending
                .get_mut(&ticket.identity.surface_key)
                .filter(|request| request.ticket == ticket)
            {
                settle_staged(&mut request.staged, true);
            }
        });
    }
    // Retain first. Teardown drops the guard, and a retained file stays.
    cancel_surface(ticket.identity.surface_key);
    match result {
        Ok(output) => {
            // The validated path has no shell metacharacters. Use the same
            // paste API as text input, preserving bracketed-paste mode.
            let text = format!("{} ", output.path());
            unsafe {
                ghostty_surface_text(
                    ticket.identity.surface_key as ghostty_surface_t,
                    text.as_ptr().cast(),
                    text.len(),
                );
            }
            message(ticket.identity, output.announcement());
        }
        Err(error) => message(ticket.identity, &error),
    }
}

fn store_upload(ticket: Ticket, upload: Rc<Upload>) {
    let orphan = PENDING.with_borrow_mut(|pending| {
        if let Some(request) = pending
            .get_mut(&ticket.identity.surface_key)
            .filter(|request| request.ticket == ticket)
        {
            request.upload = Some(upload);
            None
        } else {
            Some(upload)
        }
    });
    if let Some(upload) = orphan {
        upload.cancel();
    }
}

fn deliver_staged(ticket: Ticket, staged: StagedImage, path: String) {
    let stored = PENDING.with_borrow_mut(|pending| {
        if let Some(request) = pending
            .get_mut(&ticket.identity.surface_key)
            .filter(|request| request.ticket == ticket)
        {
            request.staged = Some(staged);
            true
        } else {
            drop(staged);
            false
        }
    });
    if stored {
        complete(ticket, Ok(PasteOutput::Local(path)));
    }
}

pub(super) fn image_is_too_large(width: i32, height: i32) -> bool {
    width <= 0 || height <= 0 || width as u64 * height as u64 > MAX_PIXELS
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
    // Captured for this request. A later tab selection must not retarget it.
    let destination = match (callbacks.borrow().image_paste_destination)() {
        Ok(destination) => destination,
        Err(()) => {
            message(identity, "This terminal is no longer available");
            return true;
        }
    };
    let busy = PENDING
        .with_borrow(|pending| pending.contains_key(&identity.surface_key) || pending.len() >= 4);
    if busy {
        message(
            identity,
            "An image paste is already pending; try again when it finishes",
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
                .filter(|request| request.ticket == ticket)
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
                staged: None,
                timer: Some(timer),
            },
        )
    });
    let flatpak = flatpak::enabled();
    message(
        identity,
        match destination {
            ImagePasteDestination::Local => "Saving clipboard image…",
            ImagePasteDestination::Ssh(_) => "Uploading clipboard image…",
        },
    );
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
        if image_is_too_large(texture.width(), texture.height()) {
            complete(ticket, Err("Clipboard image exceeds 16 megapixels".into()));
            return;
        }
        // GdkTexture is immutable and Send+Sync. Encoding and local writes
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
            match destination {
                ImagePasteDestination::Ssh(target) => {
                    match Upload::start(&target, png, flatpak, move |result| {
                        complete(ticket, result.map(PasteOutput::Uploaded))
                    }) {
                        Ok(upload) => store_upload(ticket, upload),
                        Err(error) => complete(ticket, Err(error)),
                    }
                }
                ImagePasteDestination::Local if flatpak => {
                    match Upload::stage_on_host(png, move |result| match result {
                        Ok(path) => match StagedImage::host(path.clone()) {
                            Ok(staged) => deliver_staged(ticket, staged, path),
                            Err(error) => complete(ticket, Err(error)),
                        },
                        Err(error) => complete(ticket, Err(error)),
                    }) {
                        Ok(upload) => store_upload(ticket, upload),
                        Err(error) => complete(ticket, Err(error)),
                    }
                }
                ImagePasteDestination::Local => {
                    let bytes = png.as_ref().to_vec();
                    let staged =
                        gtk::gio::spawn_blocking(move || StagedImage::native(&bytes)).await;
                    if !current(ticket) {
                        return;
                    }
                    match staged {
                        Ok(Ok(staged)) => {
                            let path = staged.path().to_string();
                            deliver_staged(ticket, staged, path);
                        }
                        Ok(Err(error)) => complete(ticket, Err(error)),
                        Err(_) => {
                            complete(ticket, Err("Could not save the clipboard image".into()))
                        }
                    }
                }
            }
        });
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_pixel_limit_is_16777216() {
        assert!(!image_is_too_large(4096, 4096));
        assert!(image_is_too_large(4096, 4097));
        assert!(image_is_too_large(0, 1));
        assert!(image_is_too_large(-1, 10));
    }

    #[test]
    fn delivery_retains_the_file_before_the_guard_is_dropped() {
        let png = b"\x89PNG\r\n\x1a\n\0delivery";
        let staged = StagedImage::native(png).unwrap();
        let path = staged.path().to_string();
        let directory = std::path::Path::new(&path).parent().unwrap().to_path_buf();
        let mut slot = Some(staged);
        settle_staged(&mut slot, true);
        drop(slot);
        assert_eq!(std::fs::read(&path).unwrap(), png);
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(directory).unwrap();

        let staged = StagedImage::native(png).unwrap();
        let path = staged.path().to_string();
        let directory = std::path::Path::new(&path).parent().unwrap().to_path_buf();
        let mut slot = Some(staged);
        settle_staged(&mut slot, false);
        drop(slot);
        assert!(!std::path::Path::new(&path).exists());
        assert!(!directory.exists());
    }

    #[test]
    #[ignore = "opt-in localhost SSH/Codex test; see docs/ssh.md"]
    fn live_ssh_agent_image_attachment() {
        let command =
            std::env::var("LIMUX_IMAGE_TEST_COMMAND").expect("disposable SSH agent command");
        crate::prepare_ghostty_runtime();
        gtk::init().unwrap();
        init_ghostty();
        let mut callbacks = TerminalCallbacks::disconnected();
        callbacks.image_paste_destination = Box::new(|| {
            Ok(ImagePasteDestination::Ssh(
                crate::ssh_hosts::SshTarget::parse("localhost", "").unwrap(),
            ))
        });
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
