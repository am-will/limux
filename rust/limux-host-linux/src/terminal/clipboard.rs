//! Clipboard consent and the lifetime of Ghostty's pending C requests.

use super::*;

struct PendingRequest {
    identity: SurfaceIdentity,
    state: Option<*mut c_void>,
    cancellable: gtk::gio::Cancellable,
}

#[derive(Default)]
struct PendingRequests {
    next_id: u64,
    requests: HashMap<u64, PendingRequest>,
}

impl PendingRequests {
    fn insert(&mut self, request: PendingRequest) -> u64 {
        self.next_id += 1;
        self.requests.insert(self.next_id, request);
        self.next_id
    }

    fn take(&mut self, id: u64) -> Option<PendingRequest> {
        self.requests.remove(&id)
    }

    fn drain_surface(&mut self, surface_key: usize) -> Vec<PendingRequest> {
        let ids: Vec<_> = self
            .requests
            .iter()
            .filter(|(_, request)| request.identity.surface_key == surface_key)
            .map(|(id, _)| *id)
            .collect();
        ids.into_iter().filter_map(|id| self.take(id)).collect()
    }
}

thread_local! {
    static PENDING: RefCell<PendingRequests> = RefCell::new(PendingRequests::default());
}

fn track(identity: SurfaceIdentity, state: Option<*mut c_void>) -> (u64, gtk::gio::Cancellable) {
    let cancellable = gtk::gio::Cancellable::new();
    let id = PENDING.with(|pending| {
        pending.borrow_mut().insert(PendingRequest {
            identity,
            state,
            cancellable: cancellable.clone(),
        })
    });
    (id, cancellable)
}

fn take_current(id: u64) -> Option<PendingRequest> {
    let request = PENDING.with(|pending| pending.borrow_mut().take(id))?;
    (current_surface_identity(request.identity.surface_key) == Some(request.identity))
        .then_some(request)
}

// A confirmed empty completion is Ghostty's available cancellation mechanism:
// paste does nothing, and OSC 52 receives an empty reply. It also releases the
// C request allocation without prompting again. Never free that allocation here.
fn complete(request: PendingRequest, text: &std::ffi::CStr, confirmed: bool) {
    if let Some(state) = request.state {
        unsafe {
            ghostty_surface_complete_clipboard_request(
                request.identity.surface_key as ghostty_surface_t,
                text.as_ptr(),
                state,
                confirmed,
            );
        }
    }
}

pub(super) fn cancel_surface_requests(surface_key: usize) {
    // Remove every request before cancellation can schedule its async callback.
    // The caller still owns the live Ghostty surface until this returns.
    let requests = PENDING.with(|pending| pending.borrow_mut().drain_surface(surface_key));
    for request in requests {
        request.cancellable.cancel();
        complete(request, c"", true);
    }
}

unsafe fn identity_from_userdata(userdata: *mut c_void) -> Option<SurfaceIdentity> {
    let surface = unsafe { clipboard_surface_from_userdata(userdata) }?;
    current_surface_identity(surface as usize)
}

pub(super) unsafe extern "C" fn ghostty_read_clipboard_cb(
    userdata: *mut c_void,
    clipboard_type: c_int,
    state: *mut c_void,
) -> bool {
    let Some(identity) = (unsafe { identity_from_userdata(userdata) }) else {
        return false;
    };
    let Some(display) = gtk::gdk::Display::default() else {
        return false;
    };
    let clipboard = clipboard_from_type(&display, clipboard_type);
    if !clipboard_has_text(&clipboard) {
        return false;
    }

    let (id, cancellable) = track(identity, Some(state));
    clipboard.read_text_async(Some(&cancellable), move |result| {
        let Some(request) = take_current(id) else {
            return;
        };
        match result {
            Ok(Some(text)) => {
                let text = clipboard_read_text_cstring(Some(&text));
                // Let Ghostty enforce clipboard-read and paste-protection policy.
                // It may synchronously transfer this state to the confirm callback.
                complete(request, &text, false);
            }
            _ => complete(request, c"", true),
        }
    });
    true
}

fn consent_text(text: &std::ffi::CStr, choice: Option<i32>) -> &std::ffi::CStr {
    if choice == Some(1) {
        text
    } else {
        c""
    }
}

fn ask_consent(
    identity: SurfaceIdentity,
    state: Option<*mut c_void>,
    message: &str,
    detail: &str,
    allow_label: &str,
    on_choice: impl FnOnce(PendingRequest, Option<i32>) + 'static,
) {
    let parent = SURFACE_MAP.with(|map| {
        map.borrow()
            .get(&identity.surface_key)
            .filter(|entry| entry.identity == identity)
            .and_then(|entry| entry.gl_area.root())
            .and_then(|root| root.downcast::<gtk::Window>().ok())
    });
    let (id, cancellable) = track(identity, state);
    let Some(parent) = parent else {
        if let Some(request) = take_current(id) {
            on_choice(request, None);
        }
        return;
    };
    let dialog = gtk::AlertDialog::builder()
        .message(message)
        .detail(detail)
        .buttons(["Cancel", allow_label])
        .default_button(0)
        .cancel_button(0)
        .modal(true)
        .build();
    dialog.choose(Some(&parent), Some(&cancellable), move |result| {
        if let Some(request) = take_current(id) {
            on_choice(request, result.ok());
        }
    });
}

pub(super) unsafe extern "C" fn ghostty_confirm_read_clipboard_cb(
    userdata: *mut c_void,
    text: *const c_char,
    state: *mut c_void,
    request_type: c_int,
) {
    let Some(identity) = (unsafe { identity_from_userdata(userdata) }) else {
        return;
    };
    // Ghostty only borrows this string for the duration of this callback.
    let text = unsafe { std::ffi::CStr::from_ptr(clipboard_completion_text_ptr(text)) }.to_owned();
    let (message, detail, allow_label) = match request_type {
        GHOSTTY_CLIPBOARD_REQUEST_PASTE => (
            "Paste potentially unsafe text?",
            "The clipboard contains text that may execute commands in this terminal.",
            "Paste",
        ),
        GHOSTTY_CLIPBOARD_REQUEST_OSC_52_READ => (
            "Allow this terminal to read the clipboard?",
            "A program in this terminal wants to read your clipboard contents.",
            "Allow Read",
        ),
        _ => {
            let (id, _) = track(identity, Some(state));
            if let Some(request) = take_current(id) {
                complete(request, c"", true);
            }
            return;
        }
    };
    ask_consent(
        identity,
        Some(state),
        message,
        detail,
        allow_label,
        move |request, choice| complete(request, consent_text(&text, choice), true),
    );
}

pub(super) unsafe extern "C" fn ghostty_write_clipboard_cb(
    userdata: *mut c_void,
    clipboard_type: c_int,
    contents: *const ghostty_clipboard_content_s,
    count: usize,
    confirm: bool,
) {
    if count == 0 || contents.is_null() {
        return;
    }
    let content = unsafe { &*contents };
    if content.data.is_null() {
        return;
    }
    let text = unsafe { std::ffi::CStr::from_ptr(content.data) }
        .to_str()
        .unwrap_or("")
        .to_string();
    let Some(context) = (unsafe { clipboard_context_from_userdata(userdata) }) else {
        return;
    };
    if context.url_probe_active.get() {
        *context.url_probe.borrow_mut() = Some(text);
        return;
    }
    let Some(identity) = (unsafe { identity_from_userdata(userdata) }) else {
        return;
    };
    let policy = clipboard_write_policy(clipboard_type, (context.copy_selection_to_clipboard)());
    if confirm {
        ask_consent(
            identity,
            None,
            "Allow this terminal to replace the clipboard?",
            "A program in this terminal wants to overwrite your clipboard contents.",
            "Allow Write",
            move |request, choice| {
                if choice == Some(1) {
                    write_text(request.identity, &text, policy);
                }
            },
        );
    } else {
        write_text(identity, &text, policy);
    }
}

fn write_text(identity: SurfaceIdentity, text: &str, policy: ClipboardWritePolicy) {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    if policy.write_clipboard {
        display.clipboard().set_text(text);
    }
    if policy.write_primary {
        display.primary_clipboard().set_text(text);
    }
    if policy.show_toast {
        SURFACE_MAP.with(|map| {
            if let Some(entry) = map.borrow().get(&identity.surface_key) {
                show_clipboard_toast(&entry.toast_overlay);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_explicit_consent_releases_clipboard_text() {
        for choice in [None, Some(0), Some(-1), Some(2)] {
            assert_eq!(consent_text(c"secret\ncommand", choice), c"");
        }
        assert_eq!(
            consent_text(c"secret\ncommand", Some(1)),
            c"secret\ncommand"
        );
    }

    fn request(identity: SurfaceIdentity) -> PendingRequest {
        PendingRequest {
            identity,
            state: Some(std::ptr::dangling_mut()),
            cancellable: gtk::gio::Cancellable::new(),
        }
    }

    #[test]
    fn teardown_takes_pending_requests_once_and_preserves_other_surfaces() {
        let mut identities = SurfaceIdentityRegistry::default();
        let old_surface = identities.register(10);
        let other_surface = identities.register(20);
        let mut pending = PendingRequests::default();
        let read = pending.insert(request(old_surface));
        let prompt = pending.insert(request(old_surface));
        let other = pending.insert(request(other_surface));

        assert_eq!(pending.drain_surface(10).len(), 2);
        assert!(pending.take(read).is_none());
        assert!(pending.take(prompt).is_none());
        assert!(pending.drain_surface(10).is_empty());
        assert!(pending.take(other).is_some());
        assert!(pending.take(other).is_none());

        identities.unregister(old_surface);
        let replacement = identities.register(10);
        assert_ne!(old_surface, replacement);
        let new_request = pending.insert(request(replacement));
        assert_ne!(read, new_request);
        assert!(pending.take(read).is_none());
        assert_eq!(pending.take(new_request).unwrap().identity, replacement);
    }

    #[test]
    fn cancelled_prompt_cannot_complete_later() {
        let identity = register_surface_identity(0xc11b_0001);
        let (id, cancellable) = track(identity, None);
        cancel_surface_requests(identity.surface_key);
        assert!(cancellable.is_cancelled());
        assert!(take_current(id).is_none());
        cancel_surface_requests(identity.surface_key);
        unregister_surface_identity(identity);
    }

    #[test]
    fn late_callback_cannot_target_a_reused_surface_address() {
        let old = register_surface_identity(0xc11b_0002);
        let (id, _) = track(old, None);
        unregister_surface_identity(old);
        let replacement = register_surface_identity(old.surface_key);

        assert!(take_current(id).is_none());
        let (new_id, _) = track(replacement, None);
        assert_eq!(take_current(new_id).unwrap().identity, replacement);
        assert!(take_current(new_id).is_none());
        unregister_surface_identity(replacement);
    }

    #[test]
    fn read_can_transfer_c_state_to_confirmation_without_double_completion() {
        let identity = register_surface_identity(0xc11b_0003);
        let state = std::ptr::dangling_mut();
        let (read_id, _) = track(identity, Some(state));

        // An unconfirmed completion removes the read before Ghostty calls the
        // confirmation callback synchronously with the same opaque C state.
        let read = take_current(read_id).unwrap();
        let (prompt_id, _) = track(read.identity, read.state);
        assert!(take_current(read_id).is_none());

        // Closing the surface must reclaim exactly the confirmation's state.
        let drained =
            PENDING.with(|pending| pending.borrow_mut().drain_surface(identity.surface_key));
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].state, Some(state));
        assert!(take_current(prompt_id).is_none());
        assert!(take_current(read_id).is_none());
        unregister_surface_identity(identity);
    }
}
