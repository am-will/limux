use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;
use shell_quote::Bash;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};
use std::os::unix::ffi::OsStringExt;
use std::ptr;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::Duration;

use limux_ghostty_sys::*;

// ---------------------------------------------------------------------------
// Global Ghostty app singleton
// ---------------------------------------------------------------------------

struct GhosttyState {
    app: ghostty_app_t,
}

// Safety: ghostty_app_t is thread-safe for the operations we perform
unsafe impl Send for GhosttyState {}
unsafe impl Sync for GhosttyState {}

static GHOSTTY: OnceLock<GhosttyState> = OnceLock::new();

type TitleChangedCallback = dyn Fn(&str);
type PwdChangedCallback = dyn Fn(&str);
type DesktopNotificationCallback = dyn Fn(&str, &str);
type VoidCallback = dyn Fn();
type WidgetCallback = dyn Fn(&gtk::Widget);

/// Per-surface state, stored in a global registry keyed by surface pointer.
struct SurfaceEntry {
    gl_area: gtk::GLArea,
    toast_overlay: gtk::Overlay,
    on_title_changed: Option<Box<TitleChangedCallback>>,
    on_pwd_changed: Option<Box<PwdChangedCallback>>,
    on_desktop_notification: Option<Box<DesktopNotificationCallback>>,
    on_bell: Option<Box<VoidCallback>>,
    on_close: Option<Box<VoidCallback>>,
    clipboard_context: *mut ClipboardContext,
}

struct ClipboardContext {
    surface: Cell<ghostty_surface_t>,
}

thread_local! {
    static SURFACE_MAP: RefCell<HashMap<usize, SurfaceEntry>> = RefCell::new(HashMap::new());
}

#[derive(Clone)]
pub struct TerminalHandle {
    surface_cell: Rc<RefCell<Option<ghostty_surface_t>>>,
    gl_area: gtk::GLArea,
    search_bar: gtk::SearchBar,
    search_entry: gtk::SearchEntry,
    callbacks: Rc<RefCell<TerminalCallbacks>>,
}

impl TerminalHandle {
    pub fn replace_callbacks(&self, callbacks: TerminalCallbacks) {
        *self.callbacks.borrow_mut() = callbacks;
    }

    pub fn perform_binding_action(&self, action: &str) -> bool {
        let surface = *self.surface_cell.borrow();
        surface_action(surface, action);
        surface.is_some()
    }

    pub fn show_find(&self) -> bool {
        self.search_bar.set_search_mode(true);
        self.search_entry.grab_focus();
        self.search_entry.select_region(0, -1);
        if !self.search_entry.text().is_empty() {
            self.apply_search_query(self.search_entry.text().as_str());
        }
        true
    }

    pub fn find_next(&self) -> bool {
        if !self.search_bar.is_search_mode() || self.search_entry.text().is_empty() {
            return false;
        }
        self.perform_binding_action("navigate_search:next")
    }

    pub fn find_previous(&self) -> bool {
        if !self.search_bar.is_search_mode() || self.search_entry.text().is_empty() {
            return false;
        }
        self.perform_binding_action("navigate_search:previous")
    }

    pub fn hide_find(&self) -> bool {
        if !self.search_bar.is_search_mode() {
            return false;
        }
        self.perform_binding_action("end_search");
        self.search_bar.set_search_mode(false);
        self.gl_area.grab_focus();
        true
    }

    pub fn use_selection_for_find(&self) -> bool {
        let selection = self.read_selection_text();
        if selection.is_empty() {
            return false;
        }

        self.search_bar.set_search_mode(true);
        self.search_entry.set_text(&selection);
        self.search_entry.grab_focus();
        self.search_entry.select_region(0, -1);
        self.apply_search_query(&selection);
        true
    }

    fn apply_search_query(&self, query: &str) -> bool {
        let surface = *self.surface_cell.borrow();
        surface_action(surface, &terminal_search_action(query));
        surface.is_some()
    }

    fn read_selection_text(&self) -> String {
        let Some(surface) = *self.surface_cell.borrow() else {
            return String::new();
        };

        let mut text = ghostty_text_s {
            tl_px_x: 0.0,
            tl_px_y: 0.0,
            offset_start: 0,
            offset_len: 0,
            text: ptr::null(),
            text_len: 0,
        };

        let has_selection = unsafe { ghostty_surface_read_selection(surface, &mut text) };
        if !has_selection || text.text.is_null() || text.text_len == 0 {
            return String::new();
        }

        let bytes = unsafe { std::slice::from_raw_parts(text.text as *const u8, text.text_len) };
        let selection = String::from_utf8_lossy(bytes).into_owned();
        unsafe { ghostty_surface_free_text(surface, &mut text) };
        selection
    }
}

pub struct TerminalWidget {
    pub overlay: gtk::Overlay,
    pub handle: TerminalHandle,
}

fn terminal_search_action(query: &str) -> String {
    format!("search:{query}")
}

fn request_terminal_focus(gl_area: &gtk::GLArea, had_focus: &Cell<bool>) {
    had_focus.set(true);
    gl_area.grab_focus();
}

/// Initialize the global Ghostty app. Must be called once before creating surfaces.
pub fn init_ghostty() {
    GHOSTTY.get_or_init(|| {
        unsafe {
            ghostty_init(0, ptr::null_mut());
        }

        let config = unsafe {
            let c = ghostty_config_new();
            ghostty_config_load_default_files(c);
            ghostty_config_load_recursive_files(c);
            ghostty_config_finalize(c);
            c
        };

        let runtime_config = ghostty_runtime_config_s {
            userdata: ptr::null_mut(),
            supports_selection_clipboard: true,
            wakeup_cb: ghostty_wakeup_cb,
            action_cb: ghostty_action_cb,
            clipboard_has_text_cb: ghostty_clipboard_has_text_cb,
            read_clipboard_cb: ghostty_read_clipboard_cb,
            confirm_read_clipboard_cb: ghostty_confirm_read_clipboard_cb,
            write_clipboard_cb: ghostty_write_clipboard_cb,
            close_surface_cb: ghostty_close_surface_cb,
        };

        let app = unsafe { ghostty_app_new(&runtime_config, config) };

        // Ghostty's GTK apprt calls core_app.tick() on every GLib main
        // loop iteration to drain the app mailbox (which includes
        // redraw_surface messages from the renderer thread). The renderer
        // thread pushes these messages but doesn't wake the app.
        // We replicate this with a high-frequency timer (~8ms ≈ 120Hz).
        glib::timeout_add_local(std::time::Duration::from_millis(8), move || {
            unsafe { ghostty_app_tick(app) };
            glib::ControlFlow::Continue
        });

        GhosttyState { app }
    });
}

fn ghostty_app() -> ghostty_app_t {
    GHOSTTY.get().expect("ghostty not initialized").app
}

fn ghostty_color_scheme_for_dark_mode(dark: bool) -> c_int {
    if dark {
        GHOSTTY_COLOR_SCHEME_DARK
    } else {
        GHOSTTY_COLOR_SCHEME_LIGHT
    }
}

pub fn sync_color_scheme(dark: bool) {
    let scheme = ghostty_color_scheme_for_dark_mode(dark);
    let app = ghostty_app();

    unsafe {
        ghostty_app_set_color_scheme(app, scheme);
    }

    SURFACE_MAP.with(|map| {
        for surface_key in map.borrow().keys() {
            let surface = *surface_key as ghostty_surface_t;
            unsafe {
                ghostty_surface_set_color_scheme(surface, scheme);
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Runtime callbacks (C ABI)
// ---------------------------------------------------------------------------

unsafe extern "C" fn ghostty_wakeup_cb(_userdata: *mut c_void) {
    glib::idle_add_once(|| {
        let app = ghostty_app();
        unsafe { ghostty_app_tick(app) };
    });
}

unsafe extern "C" fn ghostty_action_cb(
    _app: ghostty_app_t,
    target: ghostty_target_s,
    action: ghostty_action_s,
) -> bool {
    let tag = action.tag;

    match tag {
        GHOSTTY_ACTION_RENDER => {
            if target.tag == GHOSTTY_TARGET_SURFACE {
                let surface_key = unsafe { target.target.surface } as usize;
                SURFACE_MAP.with(|map| {
                    if let Some(entry) = map.borrow().get(&surface_key) {
                        entry.gl_area.queue_render();
                    }
                });
            }
            true
        }
        GHOSTTY_ACTION_SET_TITLE => {
            if target.tag == GHOSTTY_TARGET_SURFACE {
                let surface_key = unsafe { target.target.surface } as usize;
                let title_ptr = unsafe { action.action.set_title.title };
                if !title_ptr.is_null() {
                    let title = unsafe { std::ffi::CStr::from_ptr(title_ptr) }
                        .to_str()
                        .unwrap_or("")
                        .to_string();
                    SURFACE_MAP.with(|map| {
                        if let Some(entry) = map.borrow().get(&surface_key) {
                            if let Some(cb) = &entry.on_title_changed {
                                cb(&title);
                            }
                        }
                    });
                }
            }
            true
        }
        GHOSTTY_ACTION_DESKTOP_NOTIFICATION => {
            if target.tag == GHOSTTY_TARGET_SURFACE {
                let surface_key = unsafe { target.target.surface } as usize;
                let title_ptr = unsafe { action.action.desktop_notification.title };
                let body_ptr = unsafe { action.action.desktop_notification.body };
                let title = if title_ptr.is_null() {
                    String::new()
                } else {
                    unsafe { std::ffi::CStr::from_ptr(title_ptr) }
                        .to_str()
                        .unwrap_or("")
                        .to_string()
                };
                let body = if body_ptr.is_null() {
                    String::new()
                } else {
                    unsafe { std::ffi::CStr::from_ptr(body_ptr) }
                        .to_str()
                        .unwrap_or("")
                        .to_string()
                };
                SURFACE_MAP.with(|map| {
                    if let Some(entry) = map.borrow().get(&surface_key) {
                        if let Some(cb) = &entry.on_desktop_notification {
                            cb(&title, &body);
                        }
                    }
                });
            }
            true
        }
        GHOSTTY_ACTION_PWD => {
            if target.tag == GHOSTTY_TARGET_SURFACE {
                let surface_key = unsafe { target.target.surface } as usize;
                let pwd_ptr = unsafe { action.action.pwd.pwd };
                if !pwd_ptr.is_null() {
                    let pwd = unsafe { std::ffi::CStr::from_ptr(pwd_ptr) }
                        .to_str()
                        .unwrap_or("")
                        .to_string();
                    SURFACE_MAP.with(|map| {
                        if let Some(entry) = map.borrow().get(&surface_key) {
                            if let Some(cb) = &entry.on_pwd_changed {
                                cb(&pwd);
                            }
                        }
                    });
                }
            }
            true
        }
        GHOSTTY_ACTION_RING_BELL => {
            if target.tag == GHOSTTY_TARGET_SURFACE {
                let surface_key = unsafe { target.target.surface } as usize;
                SURFACE_MAP.with(|map| {
                    if let Some(entry) = map.borrow().get(&surface_key) {
                        if let Some(cb) = &entry.on_bell {
                            cb();
                        }
                    }
                });
            }
            true
        }
        GHOSTTY_ACTION_SHOW_CHILD_EXITED => {
            if target.tag == GHOSTTY_TARGET_SURFACE {
                let surface_key = unsafe { target.target.surface } as usize;
                glib::idle_add_local_once(move || {
                    SURFACE_MAP.with(|map| {
                        if let Some(entry) = map.borrow().get(&surface_key) {
                            if let Some(cb) = &entry.on_close {
                                cb();
                            }
                        }
                    });
                });
            }
            true
        }
        _ => false,
    }
}

unsafe fn clipboard_surface_from_userdata(userdata: *mut c_void) -> Option<ghostty_surface_t> {
    if userdata.is_null() {
        return None;
    }
    let context = unsafe { &*(userdata as *const ClipboardContext) };
    let surface = context.surface.get();
    if surface.is_null() {
        None
    } else {
        Some(surface)
    }
}

unsafe extern "C" fn ghostty_read_clipboard_cb(
    userdata: *mut c_void,
    clipboard_type: c_int,
    state: *mut c_void,
) {
    let surface_ptr = match unsafe { clipboard_surface_from_userdata(userdata) } {
        Some(surface) => surface,
        None => return,
    };

    let display = match gtk::gdk::Display::default() {
        Some(d) => d,
        None => return,
    };
    let clipboard = clipboard_from_type(&display, clipboard_type);

    clipboard.read_text_async(gtk::gio::Cancellable::NONE, move |result| {
        // Get clipboard text, defaulting to empty string on failure
        let text = result
            .ok()
            .flatten()
            .map(|s| s.to_string())
            .unwrap_or_default();
        // Replace interior null bytes so CString doesn't fail
        let clean = text.replace('\0', "");
        if let Ok(cstr) = CString::new(clean) {
            unsafe {
                ghostty_surface_complete_clipboard_request(surface_ptr, cstr.as_ptr(), state, true);
            }
        }
    });
}

fn clipboard_from_type(display: &gtk::gdk::Display, clipboard_type: c_int) -> gtk::gdk::Clipboard {
    if clipboard_type == GHOSTTY_CLIPBOARD_SELECTION {
        display.primary_clipboard()
    } else {
        display.clipboard()
    }
}

fn clipboard_has_text(clipboard: &gtk::gdk::Clipboard) -> bool {
    let formats = clipboard.formats();
    let mime_types = formats.mime_types();
    if clipboard_formats_include_image(mime_types.iter().map(|mime| mime.as_str())) {
        return false;
    }

    clipboard_formats_include_text(
        formats.contains_type(String::static_type()),
        mime_types.iter().map(|mime| mime.as_str()),
    )
}

fn clipboard_formats_include_image<'a>(mime_types: impl IntoIterator<Item = &'a str>) -> bool {
    mime_types
        .into_iter()
        .any(|mime| mime.starts_with("image/"))
}

fn clipboard_formats_include_text<'a>(
    has_string_type: bool,
    mime_types: impl IntoIterator<Item = &'a str>,
) -> bool {
    if !has_string_type {
        return false;
    }

    mime_types.into_iter().any(|mime| {
        mime.eq_ignore_ascii_case("text/plain")
            || mime.eq_ignore_ascii_case("text/plain;charset=utf-8")
    })
}

unsafe extern "C" fn ghostty_clipboard_has_text_cb(
    _userdata: *mut c_void,
    clipboard_type: c_int,
) -> bool {
    let Some(display) = gtk::gdk::Display::default() else {
        return false;
    };
    let clipboard = clipboard_from_type(&display, clipboard_type);
    clipboard_has_text(&clipboard)
}

unsafe extern "C" fn ghostty_confirm_read_clipboard_cb(
    userdata: *mut c_void,
    text: *const c_char,
    state: *mut c_void,
    _request_type: c_int,
) {
    let surface_ptr = match unsafe { clipboard_surface_from_userdata(userdata) } {
        Some(surface) => surface,
        None => return,
    };
    unsafe {
        ghostty_surface_complete_clipboard_request(surface_ptr, text, state, true);
    }
}

unsafe extern "C" fn ghostty_write_clipboard_cb(
    userdata: *mut c_void,
    clipboard_type: c_int,
    contents: *const ghostty_clipboard_content_s,
    count: usize,
    _confirm: bool,
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

    let display = match gtk::gdk::Display::default() {
        Some(d) => d,
        None => return,
    };

    // Write to the requested clipboard
    let clipboard = if clipboard_type == GHOSTTY_CLIPBOARD_SELECTION {
        display.primary_clipboard()
    } else {
        display.clipboard()
    };
    clipboard.set_text(&text);

    // Also set the other clipboard for convenience
    if clipboard_type == GHOSTTY_CLIPBOARD_SELECTION {
        display.clipboard().set_text(&text);
    } else {
        display.primary_clipboard().set_text(&text);
    }

    // Show "Copied to clipboard" toast on the surface's overlay
    let surface_key = match unsafe { clipboard_surface_from_userdata(userdata) } {
        Some(surface) => surface as usize,
        None => return,
    };
    SURFACE_MAP.with(|map| {
        if let Some(entry) = map.borrow().get(&surface_key) {
            show_clipboard_toast(&entry.toast_overlay);
        }
    });
}

unsafe extern "C" fn ghostty_close_surface_cb(userdata: *mut c_void, _process_alive: bool) {
    let Some(surface_key) =
        (unsafe { clipboard_surface_from_userdata(userdata) }).map(|surface| surface as usize)
    else {
        return;
    };
    glib::idle_add_local_once(move || {
        SURFACE_MAP.with(|map| {
            if let Some(entry) = map.borrow().get(&surface_key) {
                if let Some(cb) = &entry.on_close {
                    cb();
                }
            }
        });
    });
}

// ---------------------------------------------------------------------------
// Surface creation
// ---------------------------------------------------------------------------

pub struct TerminalCallbacks {
    pub on_title_changed: Box<TitleChangedCallback>,
    pub on_pwd_changed: Box<PwdChangedCallback>,
    pub on_desktop_notification: Box<DesktopNotificationCallback>,
    pub on_bell: Box<VoidCallback>,
    pub on_close: Box<VoidCallback>,
    pub on_split_right: Box<VoidCallback>,
    pub on_split_down: Box<VoidCallback>,
    pub on_open_keybinds: Box<WidgetCallback>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TerminalOptions {
    pub hover_focus: bool,
}

/// Create a new Ghostty-powered terminal widget.
/// Returns an Overlay (GLArea + toast layer) for embedding in the pane.
pub fn create_terminal(
    working_directory: Option<&str>,
    options: TerminalOptions,
    callbacks: TerminalCallbacks,
) -> TerminalWidget {
    let gl_area = gtk::GLArea::new();
    gl_area.set_hexpand(true);
    gl_area.set_vexpand(true);
    // auto_render=true ensures GTK continuously redraws the GLArea,
    // which forces its internal FBO to match the current allocation.
    // With auto_render=false, the FBO may stay at the initial size.
    gl_area.set_auto_render(true);
    gl_area.set_focusable(true);
    gl_area.set_can_focus(true);

    let wd = working_directory.map(|s| s.to_string());
    let hover_focus = options.hover_focus;
    let callbacks = Rc::new(RefCell::new(callbacks));
    let surface_cell: Rc<RefCell<Option<ghostty_surface_t>>> = Rc::new(RefCell::new(None));
    let had_focus = Rc::new(Cell::new(false));
    let clipboard_context_cell: Rc<Cell<*mut ClipboardContext>> =
        Rc::new(Cell::new(ptr::null_mut()));

    // Create overlay early so closures can capture it for toast notifications
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&gl_area));
    overlay.set_hexpand(true);
    overlay.set_vexpand(true);

    let search_entry = gtk::SearchEntry::builder()
        .hexpand(true)
        .placeholder_text("Find in terminal")
        .build();
    let search_bar = gtk::SearchBar::new();
    search_bar.set_show_close_button(true);
    search_bar.connect_entry(&search_entry);
    search_bar.set_child(Some(&search_entry));
    search_bar.set_valign(gtk::Align::Start);
    search_bar.set_halign(gtk::Align::Fill);
    search_bar.set_margin_top(8);
    search_bar.set_margin_start(8);
    search_bar.set_margin_end(8);
    overlay.add_overlay(&search_bar);

    let handle = TerminalHandle {
        surface_cell: surface_cell.clone(),
        gl_area: gl_area.clone(),
        search_bar: search_bar.clone(),
        search_entry: search_entry.clone(),
        callbacks: callbacks.clone(),
    };

    {
        let handle = handle.clone();
        search_entry.connect_search_changed(move |entry| {
            handle.apply_search_query(entry.text().as_str());
        });
    }
    {
        let handle = handle.clone();
        search_entry.connect_stop_search(move |_| {
            handle.hide_find();
        });
    }

    // On realize: create the Ghostty surface
    {
        let gl = gl_area.clone();
        let overlay_for_map = overlay.clone();
        let surface_cell = surface_cell.clone();
        let callbacks = callbacks.clone();
        let had_focus = had_focus.clone();
        let clipboard_context_cell = clipboard_context_cell.clone();
        gl_area.connect_realize(move |gl_area| {
            gl_area.make_current();
            if let Some(err) = gl_area.error() {
                eprintln!("limux: GLArea error after make_current: {err}");
                return;
            }

            // If the surface already exists (reparenting from a split),
            // reinitialize the GL renderer with the new GL context while
            // preserving the terminal/pty state.
            if let Some(surface) = *surface_cell.borrow() {
                unsafe { ghostty_surface_display_realized(surface) };
                gl_area.queue_render();
                return;
            }

            let app = ghostty_app();
            let mut config = unsafe { ghostty_surface_config_new() };
            let clipboard_context = Box::into_raw(Box::new(ClipboardContext {
                surface: Cell::new(ptr::null_mut()),
            }));
            config.platform_tag = GHOSTTY_PLATFORM_LINUX;
            config.platform = ghostty_platform_u {
                linux: ghostty_platform_linux_s {
                    reserved: ptr::null_mut(),
                },
            };
            config.userdata = clipboard_context.cast();

            let scale = gl_area.scale_factor() as f64;
            config.scale_factor = scale;
            config.context = GHOSTTY_SURFACE_CONTEXT_WINDOW;

            let c_wd = wd.as_ref().and_then(|s| CString::new(s.as_str()).ok());
            if let Some(ref cwd) = c_wd {
                config.working_directory = cwd.as_ptr();
            }

            let surface = unsafe { ghostty_surface_new(app, &config) };
            if surface.is_null() {
                unsafe {
                    drop(Box::from_raw(clipboard_context));
                }
                eprintln!("limux: failed to create ghostty surface");
                return;
            }
            unsafe {
                (*clipboard_context).surface.set(surface);
            }
            clipboard_context_cell.set(clipboard_context);

            // Set initial size — GLArea gives unscaled CSS pixels,
            // Ghostty handles scaling internally via content_scale.
            let alloc = gl_area.allocation();
            let w = alloc.width() as u32;
            let h = alloc.height() as u32;
            if w > 0 && h > 0 {
                unsafe {
                    ghostty_surface_set_content_scale(surface, scale, scale);
                    ghostty_surface_set_size(surface, w, h);
                }
            }

            let surface_key = surface as usize;
            SURFACE_MAP.with(|map| {
                map.borrow_mut().insert(
                    surface_key,
                    SurfaceEntry {
                        gl_area: gl.clone(),
                        toast_overlay: overlay_for_map.clone(),
                        on_title_changed: Some(Box::new({
                            let cb = callbacks.clone();
                            move |title| {
                                let callbacks = cb.borrow();
                                (callbacks.on_title_changed)(title);
                            }
                        })),
                        on_pwd_changed: Some(Box::new({
                            let cb = callbacks.clone();
                            move |pwd| {
                                let callbacks = cb.borrow();
                                (callbacks.on_pwd_changed)(pwd);
                            }
                        })),
                        on_desktop_notification: Some(Box::new({
                            let cb = callbacks.clone();
                            move |title, body| {
                                let callbacks = cb.borrow();
                                (callbacks.on_desktop_notification)(title, body);
                            }
                        })),
                        on_bell: Some(Box::new({
                            let cb = callbacks.clone();
                            move || {
                                let callbacks = cb.borrow();
                                (callbacks.on_bell)();
                            }
                        })),
                        on_close: Some(Box::new({
                            let cb = callbacks.clone();
                            move || {
                                let callbacks = cb.borrow();
                                (callbacks.on_close)();
                            }
                        })),
                        clipboard_context,
                    },
                );
            });

            *surface_cell.borrow_mut() = Some(surface);

            unsafe {
                ghostty_surface_set_focus(surface, true);
            }

            // Grab GTK focus so key events reach this widget.
            request_terminal_focus(gl_area, &had_focus);
        });
    }

    // On render: draw the surface.
    {
        let surface_cell = surface_cell.clone();
        gl_area.connect_render(move |_gl_area, _context| {
            if let Some(surface) = *surface_cell.borrow() {
                unsafe { ghostty_surface_draw(surface) };
            }
            glib::Propagation::Stop
        });
    }

    // On resize: update Ghostty's terminal grid size and queue a redraw.
    // The actual GL viewport is set by GTK when the render signal fires,
    // so we must NOT call ghostty_surface_draw here — the viewport would
    // still be the old size. Instead we queue_render() and let the render
    // callback draw with the correct viewport.
    {
        let surface_cell = surface_cell.clone();
        let gl_for_resize = gl_area.clone();
        let had_focus = had_focus.clone();
        gl_area.connect_resize(move |gl_area, width, height| {
            if let Some(surface) = *surface_cell.borrow() {
                let w = width as u32;
                let h = height as u32;
                if w > 0 && h > 0 {
                    let scale = gl_area.scale_factor() as f64;
                    unsafe {
                        ghostty_surface_set_content_scale(surface, scale, scale);
                        ghostty_surface_set_size(surface, w, h);
                    }
                    gl_area.queue_render();
                }
            }

            if had_focus.get() {
                let gl_for_focus = gl_for_resize.clone();
                glib::idle_add_local_once(move || {
                    gl_for_focus.grab_focus();
                });
            }
        });
    }

    // Keyboard input
    //
    // Send key events with the text field populated. Ghostty uses the
    // text field for actual character input and the keycode for bindings.
    // Do NOT use ghostty_surface_text() for regular typing — Ghostty
    // treats that as a paste, causing "pasting..." indicators in apps.
    {
        let sc_press = surface_cell.clone();
        let sc_release = surface_cell.clone();
        let key_controller = gtk::EventControllerKey::new();
        key_controller.connect_key_pressed(move |ctrl, keyval, keycode, modifier| {
            if let Some(surface) = *sc_press.borrow() {
                let c_text = key_event_text(keyval);

                let current_event = ctrl
                    .current_event()
                    .and_then(|event| event.downcast::<gtk::gdk::KeyEvent>().ok());
                let widget = ctrl.widget();

                let mut event = translate_key_event(
                    GHOSTTY_ACTION_PRESS,
                    widget.as_ref(),
                    current_event.as_ref(),
                    keyval,
                    keycode,
                    modifier,
                );
                if let Some(ref ct) = c_text {
                    event.text = ct.as_ptr();
                }

                let consumed = unsafe { ghostty_surface_key(surface, event) };
                if consumed {
                    return glib::Propagation::Stop;
                }
            }
            glib::Propagation::Proceed
        });

        key_controller.connect_key_released(move |ctrl, keyval, keycode, modifier| {
            if let Some(surface) = *sc_release.borrow() {
                let current_event = ctrl
                    .current_event()
                    .and_then(|event| event.downcast::<gtk::gdk::KeyEvent>().ok());
                let widget = ctrl.widget();
                let event = translate_key_event(
                    GHOSTTY_ACTION_RELEASE,
                    widget.as_ref(),
                    current_event.as_ref(),
                    keyval,
                    keycode,
                    modifier,
                );
                unsafe { ghostty_surface_key(surface, event) };
            }
        });

        gl_area.add_controller(key_controller);
    }

    // Mouse buttons (also handles click-to-focus) — skip right-click (handled below)
    {
        let surface_cell = surface_cell.clone();
        let click = gtk::GestureClick::new();
        click.set_button(0); // all buttons
        let sc = surface_cell.clone();
        let gl_for_focus = gl_area.clone();
        let had_focus = had_focus.clone();
        click.connect_pressed(move |gesture, _n, x, y| {
            let btn = gesture.current_button();
            // Grab keyboard focus on any click
            request_terminal_focus(&gl_for_focus, &had_focus);
            // Skip right-click — context menu handles it
            if btn == 3 {
                return;
            }
            if let Some(surface) = *sc.borrow() {
                let button = match btn {
                    1 => GHOSTTY_MOUSE_LEFT,
                    2 => GHOSTTY_MOUSE_MIDDLE,
                    _ => GHOSTTY_MOUSE_UNKNOWN,
                };
                let mods = translate_mouse_mods(gesture.current_event_state());
                unsafe {
                    ghostty_surface_mouse_pos(surface, x, y, mods);
                    ghostty_surface_mouse_button(surface, GHOSTTY_MOUSE_PRESS, button, mods);
                }
            }
        });
        let sc2 = surface_cell.clone();
        click.connect_released(move |gesture, _n, x, y| {
            let btn = gesture.current_button();
            if btn == 3 {
                return;
            }
            if let Some(surface) = *sc2.borrow() {
                let button = match btn {
                    1 => GHOSTTY_MOUSE_LEFT,
                    2 => GHOSTTY_MOUSE_MIDDLE,
                    _ => GHOSTTY_MOUSE_UNKNOWN,
                };
                let mods = translate_mouse_mods(gesture.current_event_state());
                unsafe {
                    ghostty_surface_mouse_pos(surface, x, y, mods);
                    ghostty_surface_mouse_button(surface, GHOSTTY_MOUSE_RELEASE, button, mods);
                }
            }
        });
        gl_area.add_controller(click);
    }

    // Right-click context menu
    {
        let sc = surface_cell.clone();
        let callbacks = callbacks.clone();
        let gl = gl_area.clone();
        let right_click = gtk::GestureClick::new();
        right_click.set_button(3);
        right_click.connect_pressed(move |gesture, _n, x, y| {
            let surface = *sc.borrow();
            show_terminal_context_menu(&gl, surface, &callbacks, x, y);
            gesture.set_state(gtk::EventSequenceState::Claimed);
        });
        gl_area.add_controller(right_click);
    }

    // Mouse motion
    {
        let surface_cell = surface_cell.clone();
        let surface_cell_for_enter = surface_cell.clone();
        let gl_for_focus = gl_area.clone();
        let had_focus = had_focus.clone();
        let motion = gtk::EventControllerMotion::new();
        motion.connect_enter(move |ctrl, x, y| {
            if hover_focus {
                // Match common Hyprland/Omarchy-style focus-follows-mouse behavior:
                // as soon as the pointer enters a terminal, focus it so typing works
                // immediately without an extra click.
                request_terminal_focus(&gl_for_focus, &had_focus);
            }

            if let Some(surface) = *surface_cell_for_enter.borrow() {
                let mods = translate_mouse_mods(ctrl.current_event_state());
                unsafe { ghostty_surface_mouse_pos(surface, x, y, mods) };
            }
        });
        let surface_cell = surface_cell.clone();
        motion.connect_motion(move |ctrl, x, y| {
            if let Some(surface) = *surface_cell.borrow() {
                let mods = translate_mouse_mods(ctrl.current_event_state());
                unsafe { ghostty_surface_mouse_pos(surface, x, y, mods) };
            }
        });
        gl_area.add_controller(motion);
    }

    // Mouse scroll
    {
        let surface_cell = surface_cell.clone();
        let scroll = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::BOTH_AXES | gtk::EventControllerScrollFlags::DISCRETE,
        );
        scroll.connect_scroll(move |ctrl, dx, dy| {
            if let Some(surface) = *surface_cell.borrow() {
                let mods = translate_mouse_mods(ctrl.current_event_state());
                // GTK and Ghostty use opposite scroll conventions — negate both axes
                unsafe { ghostty_surface_mouse_scroll(surface, -dx, -dy, mods) };
            }
            glib::Propagation::Stop
        });
        gl_area.add_controller(scroll);
    }

    // Focus
    {
        let surface_cell = surface_cell.clone();
        let had_focus_enter = had_focus.clone();
        let had_focus_leave = had_focus.clone();
        let focus_ctrl = gtk::EventControllerFocus::new();
        let sc = surface_cell.clone();
        focus_ctrl.connect_enter(move |_| {
            had_focus_enter.set(true);
            if let Some(surface) = *sc.borrow() {
                unsafe { ghostty_surface_set_focus(surface, true) };
            }
        });
        focus_ctrl.connect_leave(move |_| {
            had_focus_leave.set(false);
            if let Some(surface) = *surface_cell.borrow() {
                unsafe { ghostty_surface_set_focus(surface, false) };
            }
        });
        gl_area.add_controller(focus_ctrl);
    }

    // File drop: accept files dragged from a file manager and paste their
    // shell-escaped paths into the terminal.
    {
        let surface_cell = surface_cell.clone();
        let drop_target = gtk::DropTarget::new(
            gtk::gdk::FileList::static_type(),
            gtk::gdk::DragAction::COPY,
        );
        drop_target.connect_drop(move |_target, value, _x, _y| {
            let Some(surface) = *surface_cell.borrow() else {
                return false;
            };
            let Ok(file_list) = value.get::<gtk::gdk::FileList>() else {
                return false;
            };
            let Some(text) = dropped_file_text(&file_list) else {
                return false;
            };

            unsafe {
                ghostty_surface_text(surface, text.as_ptr(), text.as_bytes().len());
            }
            true
        });
        gl_area.add_controller(drop_target);
    }

    // On unrealize: deinit GL resources but keep the surface alive.
    // GTK unrealizes widgets during reparenting (splits), and we need
    // the terminal/pty to survive. The GL resources will be recreated
    // in connect_realize when the widget is re-realized.
    {
        let surface_cell = surface_cell.clone();
        gl_area.connect_unrealize(move |gl_area| {
            if let Some(surface) = *surface_cell.borrow() {
                gl_area.make_current();
                unsafe { ghostty_surface_display_unrealized(surface) };
            }
        });
    }

    // Clean up only when the widget is actually destroyed.
    {
        let surface_cell = surface_cell.clone();
        let clipboard_context_cell = clipboard_context_cell.clone();
        overlay.connect_destroy(move |_| {
            if let Some(surface) = surface_cell.borrow_mut().take() {
                let surface_key = surface as usize;
                SURFACE_MAP.with(|map| {
                    if let Some(entry) = map.borrow_mut().remove(&surface_key) {
                        unsafe {
                            drop(Box::from_raw(entry.clipboard_context));
                        }
                    }
                });
                unsafe { ghostty_surface_free(surface) };
            } else {
                let clipboard_context = clipboard_context_cell.replace(ptr::null_mut());
                if !clipboard_context.is_null() {
                    unsafe {
                        drop(Box::from_raw(clipboard_context));
                    }
                }
            }
        });
    }

    TerminalWidget { overlay, handle }
}

// ---------------------------------------------------------------------------
// Context menu
// ---------------------------------------------------------------------------

fn surface_action(surface: Option<ghostty_surface_t>, action: &str) {
    if let Some(surface) = surface {
        unsafe {
            ghostty_surface_binding_action(surface, action.as_ptr() as *const c_char, action.len());
        }
    }
}

fn show_terminal_context_menu(
    gl_area: &gtk::GLArea,
    surface: Option<ghostty_surface_t>,
    callbacks: &Rc<RefCell<TerminalCallbacks>>,
    x: f64,
    y: f64,
) {
    let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    menu_box.set_margin_top(4);
    menu_box.set_margin_bottom(4);
    menu_box.set_margin_start(4);
    menu_box.set_margin_end(4);

    let has_selection = surface
        .map(|s| unsafe { ghostty_surface_has_selection(s) })
        .unwrap_or(false);

    let items: Vec<(&str, bool)> = vec![
        ("Copy", has_selection),
        ("Paste", true),
        ("---", false),
        ("Split Right", true),
        ("Split Down", true),
        ("Keybinds", true),
        ("---", false),
        ("Clear", true),
    ];

    for (label, enabled) in &items {
        if *label == "---" {
            let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
            sep.set_margin_top(4);
            sep.set_margin_bottom(4);
            menu_box.append(&sep);
            continue;
        }

        let btn = gtk::Button::with_label(label);
        btn.add_css_class("flat");
        btn.set_sensitive(*enabled);
        btn.set_halign(gtk::Align::Fill);
        if let Some(lbl) = btn.child().and_then(|c| c.downcast::<gtk::Label>().ok()) {
            lbl.set_xalign(0.0);
        }
        menu_box.append(&btn);
    }

    let popover = gtk::Popover::new();
    popover.set_child(Some(&menu_box));
    popover.set_parent(gl_area);
    popover.set_has_arrow(false);
    popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));

    // Wire up each button
    let mut child = menu_box.first_child();
    while let Some(widget) = child {
        if let Some(btn) = widget.downcast_ref::<gtk::Button>() {
            let label = btn.label().unwrap_or_default().to_string();
            let pop = popover.clone();
            let cb = callbacks.clone();
            let gl_area = gl_area.clone();

            btn.connect_clicked(move |_| {
                pop.popdown();
                match label.as_str() {
                    "Copy" => surface_action(surface, "copy_to_clipboard"),
                    "Paste" => surface_action(surface, "paste_from_clipboard"),
                    "Split Right" => {
                        let callbacks = cb.borrow();
                        (callbacks.on_split_right)();
                    }
                    "Split Down" => {
                        let callbacks = cb.borrow();
                        (callbacks.on_split_down)();
                    }
                    "Keybinds" => {
                        let anchor: gtk::Widget = gl_area.clone().upcast();
                        let cb = cb.clone();
                        glib::timeout_add_local_once(Duration::from_millis(80), move || {
                            let callbacks = cb.borrow();
                            (callbacks.on_open_keybinds)(&anchor);
                        });
                    }
                    "Clear" => surface_action(surface, "clear_screen"),
                    _ => {}
                }
            });
        }
        child = widget.next_sibling();
    }

    {
        popover.connect_closed(move |p| {
            p.unparent();
        });
    }

    popover.popup();
}

// ---------------------------------------------------------------------------
// Key translation
// ---------------------------------------------------------------------------

fn translate_key_event(
    action: c_int,
    widget: Option<&gtk::Widget>,
    key_event: Option<&gtk::gdk::KeyEvent>,
    keyval: gtk::gdk::Key,
    keycode: u32,
    modifier: gtk::gdk::ModifierType,
) -> ghostty_input_key_s {
    let mut mods: c_int = GHOSTTY_MODS_NONE;
    if modifier.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
        mods |= GHOSTTY_MODS_SHIFT;
    }
    if modifier.contains(gtk::gdk::ModifierType::CONTROL_MASK) {
        mods |= GHOSTTY_MODS_CTRL;
    }
    if modifier.contains(gtk::gdk::ModifierType::ALT_MASK) {
        mods |= GHOSTTY_MODS_ALT;
    }
    if modifier.contains(gtk::gdk::ModifierType::SUPER_MASK) {
        mods |= GHOSTTY_MODS_SUPER;
    }

    let unshifted = widget
        .zip(key_event)
        .and_then(|(widget, key_event)| keyval_unicode_unshifted(widget, key_event, keycode))
        .unwrap_or_else(|| fallback_unshifted_codepoint(keyval));

    let consumed = key_event
        .map(translate_consumed_mods)
        .unwrap_or_else(|| fallback_consumed_mods(keyval, modifier));

    ghostty_input_key_s {
        action,
        mods,
        consumed_mods: consumed,
        keycode,
        text: ptr::null(),
        unshifted_codepoint: unshifted,
        composing: false,
    }
}

fn key_event_text(keyval: gtk::gdk::Key) -> Option<CString> {
    let ch = keyval.to_unicode()?;
    if ch.is_control() {
        return None;
    }

    let mut buf = [0u8; 4];
    let s = ch.encode_utf8(&mut buf);
    CString::new(s.as_bytes()).ok()
}

fn keyval_unicode_unshifted(
    widget: &gtk::Widget,
    key_event: &gtk::gdk::KeyEvent,
    keycode: u32,
) -> Option<u32> {
    widget
        .display()
        .map_keycode(keycode)
        .and_then(|entries| {
            entries
                .into_iter()
                .find(|(keymap_key, _)| {
                    keymap_key.group() == key_event.layout() as i32 && keymap_key.level() == 0
                })
                .and_then(|(_, key)| key.to_unicode())
        })
        .map(|ch| ch as u32)
        .filter(|codepoint| *codepoint != 0)
}

fn translate_consumed_mods(key_event: &gtk::gdk::KeyEvent) -> c_int {
    let consumed = key_event.consumed_modifiers() & gtk::gdk::MODIFIER_MASK;
    translate_mouse_mods(consumed)
}

fn fallback_consumed_mods(keyval: gtk::gdk::Key, modifier: gtk::gdk::ModifierType) -> c_int {
    let mut consumed: c_int = GHOSTTY_MODS_NONE;
    if modifier.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
        let shifted = keyval.to_unicode().map(|c| c as u32).unwrap_or(0);
        let unshifted = fallback_unshifted_codepoint(keyval);
        if shifted != 0 && shifted != unshifted {
            consumed |= GHOSTTY_MODS_SHIFT;
        }
    }
    consumed
}

fn fallback_unshifted_codepoint(keyval: gtk::gdk::Key) -> u32 {
    match keyval.to_unicode() {
        Some('!') => '1' as u32,
        Some('@') => '2' as u32,
        Some('#') => '3' as u32,
        Some('$') => '4' as u32,
        Some('%') => '5' as u32,
        Some('^') => '6' as u32,
        Some('&') => '7' as u32,
        Some('*') => '8' as u32,
        Some('(') => '9' as u32,
        Some(')') => '0' as u32,
        Some('_') => '-' as u32,
        Some('+') => '=' as u32,
        Some('{') => '[' as u32,
        Some('}') => ']' as u32,
        Some('|') => '\\' as u32,
        Some(':') => ';' as u32,
        Some('"') => '\'' as u32,
        Some('<') => ',' as u32,
        Some('>') => '.' as u32,
        Some('?') => '/' as u32,
        Some('~') => '`' as u32,
        Some(ch) => ch.to_lowercase().next().map(|c| c as u32).unwrap_or(0),
        None => 0,
    }
}

/// Show a brief "Copied to clipboard" toast at the bottom of the terminal.
fn show_clipboard_toast(overlay: &gtk::Overlay) {
    let toast = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    toast.set_halign(gtk::Align::Center);
    toast.set_valign(gtk::Align::End);
    toast.set_margin_bottom(12);

    let provider = gtk::CssProvider::new();
    provider.load_from_data(
        "box.limux-toast { \
            background: rgba(45, 45, 45, 0.95); \
            color: white; \
            border-radius: 6px; \
            padding: 6px 14px; \
            font-size: 12px; \
        } \
        box.limux-toast label { color: white; } \
        box.limux-toast button { \
            color: rgba(255,255,255,0.5); \
            border: none; \
            background: none; \
            min-height: 0; min-width: 0; \
            padding: 0 2px; \
        } \
        box.limux-toast button:hover { color: white; }",
    );
    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("display"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    toast.add_css_class("limux-toast");
    let label = gtk::Label::new(Some("Copied to clipboard"));
    let close_btn = gtk::Button::with_label("\u{00D7}"); // ×
    toast.append(&label);
    toast.append(&close_btn);
    toast.set_can_target(false);

    overlay.add_overlay(&toast);

    // Close button dismisses immediately
    {
        let t = toast.clone();
        let o = overlay.clone();
        close_btn.set_can_target(true);
        close_btn.connect_clicked(move |_| {
            o.remove_overlay(&t);
        });
    }

    // Auto-dismiss after 2 seconds
    {
        let t = toast.clone();
        let o = overlay.clone();
        glib::timeout_add_local_once(std::time::Duration::from_secs(2), move || {
            if t.parent().is_some() {
                o.remove_overlay(&t);
            }
        });
    }
}

fn dropped_file_text(file_list: &gtk::gdk::FileList) -> Option<CString> {
    shell_escape_joined_bytes(
        file_list
            .files()
            .iter()
            .filter_map(|file| file.path())
            .map(|path| path.into_os_string().into_vec()),
    )
}

/// Bash-escape a path so it can be safely pasted into the terminal without
/// sending raw control bytes to Ghostty.
fn shell_escape_bytes(s: &[u8]) -> Vec<u8> {
    Bash::quote_vec(s)
}

fn shell_escape_joined_bytes<I, B>(paths: I) -> Option<CString>
where
    I: IntoIterator<Item = B>,
    B: AsRef<[u8]>,
{
    let mut text = Vec::new();

    for path in paths {
        if !text.is_empty() {
            text.push(b' ');
        }
        text.extend(shell_escape_bytes(path.as_ref()));
    }

    if text.is_empty() {
        return None;
    }

    CString::new(text).ok()
}

fn translate_mouse_mods(state: gtk::gdk::ModifierType) -> c_int {
    let mut mods: c_int = GHOSTTY_MODS_NONE;
    if state.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
        mods |= GHOSTTY_MODS_SHIFT;
    }
    if state.contains(gtk::gdk::ModifierType::CONTROL_MASK) {
        mods |= GHOSTTY_MODS_CTRL;
    }
    if state.contains(gtk::gdk::ModifierType::ALT_MASK) {
        mods |= GHOSTTY_MODS_ALT;
    }
    if state.contains(gtk::gdk::ModifierType::SUPER_MASK) {
        mods |= GHOSTTY_MODS_SUPER;
    }
    mods
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_dark_mode_to_ghostty_color_scheme() {
        assert_eq!(
            ghostty_color_scheme_for_dark_mode(true),
            GHOSTTY_COLOR_SCHEME_DARK
        );
        assert_eq!(
            ghostty_color_scheme_for_dark_mode(false),
            GHOSTTY_COLOR_SCHEME_LIGHT
        );
    }

    #[test]
    fn fallback_unshifted_codepoint_maps_shifted_symbols() {
        assert_eq!(
            fallback_unshifted_codepoint(gtk::gdk::Key::exclam),
            '1' as u32
        );
        assert_eq!(
            fallback_unshifted_codepoint(gtk::gdk::Key::plus),
            '=' as u32
        );
        assert_eq!(
            fallback_unshifted_codepoint(gtk::gdk::Key::underscore),
            '-' as u32
        );
        assert_eq!(fallback_unshifted_codepoint(gtk::gdk::Key::A), 'a' as u32);
    }

    #[test]
    fn terminal_search_action_formats_queries_for_ghostty() {
        assert_eq!(terminal_search_action(""), "search:");
        assert_eq!(terminal_search_action("needle"), "search:needle");
        assert_eq!(terminal_search_action("two words"), "search:two words");
    }

    #[test]
    fn key_event_text_preserves_printable_chords() {
        let ctrl_shift_h = key_event_text(gtk::gdk::Key::H).and_then(|s| s.into_string().ok());
        let alt_shift_gt =
            key_event_text(gtk::gdk::Key::greater).and_then(|s| s.into_string().ok());

        assert_eq!(ctrl_shift_h.as_deref(), Some("H"));
        assert_eq!(alt_shift_gt.as_deref(), Some(">"));
        assert!(key_event_text(gtk::gdk::Key::BackSpace).is_none());
    }

    #[test]
    fn shell_escape_preserves_simple_paths() {
        assert_eq!(
            shell_escape_bytes(b"/home/user/file.txt"),
            b"/home/user/file.txt"
        );
        assert_eq!(shell_escape_bytes(b"/tmp/a-b_c.rs"), b"/tmp/a-b_c.rs");
    }

    #[test]
    fn shell_escape_quotes_paths_with_spaces() {
        assert_eq!(
            shell_escape_bytes(b"/home/user/my file.txt"),
            b"$'/home/user/my file.txt'"
        );
    }

    #[test]
    fn shell_escape_handles_single_quotes() {
        assert_eq!(
            shell_escape_bytes(b"/tmp/it's a file"),
            b"$'/tmp/it\\'s a file'"
        );
    }

    #[test]
    fn shell_escape_preserves_non_utf8_bytes() {
        let path = b"/home/user/\xff\xfefile.txt";
        assert_eq!(
            shell_escape_bytes(path),
            b"$'/home/user/\\xFF\\xFEfile.txt'"
        );
    }

    #[test]
    fn shell_escape_hex_escapes_terminal_control_bytes() {
        let path = b"/tmp/line\nbreak\tand\x03escape\x1b";
        assert_eq!(
            shell_escape_bytes(path),
            b"$'/tmp/line\\nbreak\\tand\\x03escape\\e'"
        );
    }

    #[test]
    fn clipboard_formats_include_text_rejects_image_clipboards() {
        assert!(clipboard_formats_include_text(
            true,
            ["text/plain", "text/plain;charset=utf-8"]
        ));
        assert!(clipboard_formats_include_image(["image/png", "text/plain"]));
    }

    #[test]
    fn shell_escape_joins_multiple_paths_for_terminal_drop() {
        let text = shell_escape_joined_bytes([
            b"/tmp/plain".as_slice(),
            b"/tmp/space name".as_slice(),
            b"/tmp/it's".as_slice(),
            b"/tmp/\xff\xfe".as_slice(),
            b"/tmp/line\nbreak".as_slice(),
        ])
        .expect("drop payload must be NUL-free");

        assert_eq!(
            text.as_bytes(),
            b"/tmp/plain $'/tmp/space name' $'/tmp/it\\'s' $'/tmp/\\xFF\\xFE' $'/tmp/line\\nbreak'"
        );
    }

    #[test]
    fn shell_escape_joined_bytes_rejects_empty_input() {
        assert!(shell_escape_joined_bytes(std::iter::empty::<&[u8]>()).is_none());
    }
}
