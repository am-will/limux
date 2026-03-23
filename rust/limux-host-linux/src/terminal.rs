use gtk::glib;
use gtk::glib::signal::Inhibit;
use gtk::prelude::*;
use gtk4 as gtk;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;
use std::rc::Rc;
use std::sync::OnceLock;

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
type VoidCallback = dyn Fn();

/// Per-surface state, stored in a global registry keyed by surface pointer.
struct SurfaceEntry {
    gl_area: gtk::GLArea,
    toast_overlay: gtk::Overlay,
    on_title_changed: Option<Box<TitleChangedCallback>>,
    on_pwd_changed: Option<Box<PwdChangedCallback>>,
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

/// Initialize the global Ghostty app. Must be called once before creating surfaces.
pub fn init_ghostty() {
    GHOSTTY.get_or_init(|| {
        let rc = unsafe { ghostty_init(0, ptr::null_mut()) };
        if rc != 0 {
            eprintln!("limux: ghostty_init failed with code {rc}");
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
            read_clipboard_cb: ghostty_read_clipboard_cb,
            confirm_read_clipboard_cb: ghostty_confirm_read_clipboard_cb,
            write_clipboard_cb: ghostty_write_clipboard_cb,
            close_surface_cb: ghostty_close_surface_cb,
        };

        let app = unsafe { ghostty_app_new(&runtime_config, config) };
        if app.is_null() {
            eprintln!("limux: ghostty_app_new returned null — terminals will not work");
        }

        // Ghostty's GTK apprt calls core_app.tick() on every GLib main
        // loop iteration to drain the app mailbox (which includes
        // redraw_surface messages from the renderer thread). The renderer
        // thread pushes these messages but doesn't wake the app.
        // We replicate this with a high-frequency timer (~8ms ≈ 120Hz).
        glib::timeout_add_local(std::time::Duration::from_millis(8), move || {
            unsafe { ghostty_app_tick(app) };
            glib::Continue(true)
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
    let clipboard = if clipboard_type == GHOSTTY_CLIPBOARD_SELECTION {
        display.primary_clipboard()
    } else {
        display.clipboard()
    };

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
    pub on_bell: Box<VoidCallback>,
    pub on_close: Box<VoidCallback>,
    pub on_split_right: Box<VoidCallback>,
    pub on_split_down: Box<VoidCallback>,
}

/// Create a new Ghostty-powered terminal widget.
/// Returns an Overlay (GLArea + toast layer) for embedding in the pane.
pub fn create_terminal(
    working_directory: Option<&str>,
    callbacks: TerminalCallbacks,
) -> gtk::Overlay {
    let gl_area = gtk::GLArea::new();
    gl_area.set_hexpand(true);
    gl_area.set_vexpand(true);
    // auto_render=true ensures GTK continuously redraws the GLArea,
    // which forces its internal FBO to match the current allocation.
    // With auto_render=false, the FBO may stay at the initial size.
    gl_area.set_auto_render(true);
    gl_area.set_focusable(true);
    gl_area.set_can_focus(true);
    // Ghostty requires desktop OpenGL 4.3+ (not OpenGL ES).
    // GTK 4.6 on Ubuntu 22.04 always creates a 3.2 core context regardless
    // of set_required_version(4,3). We work around this by creating a raw
    // GLX 4.3 context in connect_realize after GTK's context is current.
    gl_area.set_use_es(false);
    gl_area.set_required_version(4, 3);

    let wd = working_directory.map(|s| s.to_string());
    let callbacks = Rc::new(callbacks);
    let surface_cell: Rc<RefCell<Option<ghostty_surface_t>>> = Rc::new(RefCell::new(None));
    let had_focus = Rc::new(Cell::new(false));
    let clipboard_context_cell: Rc<Cell<*mut ClipboardContext>> =
        Rc::new(Cell::new(ptr::null_mut()));
    let glx_ctx_cell: Rc<RefCell<Option<glx_compat::Glx43Ctx>>> =
        Rc::new(RefCell::new(None));

    // Create overlay early so closures can capture it for toast notifications
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&gl_area));
    overlay.set_hexpand(true);
    overlay.set_vexpand(true);

    // On realize: create the Ghostty surface
    {
        let gl = gl_area.clone();
        let overlay_for_map = overlay.clone();
        let surface_cell = surface_cell.clone();
        let callbacks = callbacks.clone();
        let had_focus = had_focus.clone();
        let clipboard_context_cell = clipboard_context_cell.clone();
        let glx_ctx_cell = glx_ctx_cell.clone();
        gl_area.connect_realize(move |gl_area| {
            gl_area.make_current();
            if let Some(err) = gl_area.error() {
                eprintln!("limux: GLArea error after make_current: {err}");
                return;
            }

            // Create (or reuse) the OpenGL 4.3 context. GTK's 3.2 context is
            // current here, so EGL/GLX can query the current display/drawable.
            if glx_ctx_cell.borrow().is_none() {
                match glx_compat::create_glx43_context() {
                    Ok(ctx) => {
                        *glx_ctx_cell.borrow_mut() = Some(ctx);
                    }
                    Err(e) => {
                        // OpenGL 4.3 is required by Ghostty. Without it the
                        // terminal cannot render. Show a persistent in-app error
                        // so the failure is visible even without a terminal open.
                        let hint = "Hint: set LIMUX_GL_BACKEND=egl or LIMUX_GL_BACKEND=glx \
                                    to force a specific backend.";
                        let msg = format!(
                            "limux: OpenGL 4.3 context creation failed — \
                             terminal cannot start.\n{e}\n{hint}"
                        );
                        eprintln!("{msg}");
                        show_gl_error_toast(&overlay_for_map, &msg);
                        return;
                    }
                }
            }
            // Make the 4.3 context current (falls back gracefully if absent)
            if let Some(ctx) = glx_ctx_cell.borrow().as_ref() {
                if let Err(e) = ctx.make_current() {
                    eprintln!("limux: GLX make_current failed: {e}");
                }
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

            // Set LIMUX_PANE_ID so processes inside the terminal can identify their pane.
            let pane_id = uuid::Uuid::new_v4().to_string();
            let c_pane_id_key = CString::new("LIMUX_PANE_ID").expect("static key");
            let c_pane_id_val = CString::new(pane_id.as_str()).expect("uuid is valid CString");
            let mut env_var_list = vec![ghostty_env_var_s {
                key: c_pane_id_key.as_ptr(),
                value: c_pane_id_val.as_ptr(),
            }];
            config.env_vars = env_var_list.as_mut_ptr();
            config.env_var_count = env_var_list.len();

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
                            move |title| (cb.on_title_changed)(title)
                        })),
                        on_pwd_changed: Some(Box::new({
                            let cb = callbacks.clone();
                            move |pwd| (cb.on_pwd_changed)(pwd)
                        })),
                        on_bell: Some(Box::new({
                            let cb = callbacks.clone();
                            move || (cb.on_bell)()
                        })),
                        on_close: Some(Box::new({
                            let cb = callbacks.clone();
                            move || (cb.on_close)()
                        })),
                        clipboard_context,
                    },
                );
            });

            *surface_cell.borrow_mut() = Some(surface);

            unsafe {
                ghostty_surface_set_focus(surface, true);
            }

            // Grab GTK focus so key events reach this widget
            had_focus.set(true);
            gl_area.grab_focus();

            // Request an immediate render so PTY output from the initial
            // shell prompt is displayed without waiting for the 8ms tick.
            gl_area.queue_render();
        });
    }

    // On render: draw the surface.
    // GTK has already made its 3.2 context current when this fires.
    // We switch to our 4.3 context (EGL or GLX) before drawing so that
    // Ghostty's OpenGL 4.3 calls succeed. If the context switch fails we
    // skip the draw rather than letting Ghostty invoke 4.3 APIs on a 3.2
    // context (which would produce GL errors or a crash).
    {
        let surface_cell = surface_cell.clone();
        let glx_ctx_cell = glx_ctx_cell.clone();
        gl_area.connect_render(move |_gl_area, _context| {
            if let Some(surface) = *surface_cell.borrow() {
                // Require the 4.3 context to be available and current.
                // GTK's 3.2 context is already current here; make_current()
                // fetches the EGL/GLX drawable from it and switches to 4.3.
                let ctx_ok = if let Some(ctx) = glx_ctx_cell.borrow().as_ref() {
                    match ctx.make_current() {
                        Ok(()) => true,
                        Err(e) => {
                            eprintln!("limux: render: context switch failed: {e}");
                            false
                        }
                    }
                } else {
                    // No 4.3 context was created (driver limitation).
                    // Drawing with a 3.2 context would cause GL errors.
                    false
                };

                if ctx_ok {
                    unsafe { ghostty_surface_draw(surface) };
                }
            }
            Inhibit(true)
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
                    return Inhibit(true);
                }
            }
            Inhibit(false)
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
            had_focus.set(true);
            gl_for_focus.grab_focus();
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
        let motion = gtk::EventControllerMotion::new();
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
            Inhibit(true)
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
        let glx_ctx_cell = glx_ctx_cell.clone();
        overlay.connect_destroy(move |_| {
            // Destroy the raw GLX 4.3 context we created
            if let Some(ctx) = glx_ctx_cell.borrow_mut().take() {
                ctx.destroy();
            }
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

    overlay
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
    callbacks: &Rc<TerminalCallbacks>,
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

            btn.connect_clicked(move |_| {
                pop.popdown();
                match label.as_str() {
                    "Copy" => surface_action(surface, "copy_to_clipboard"),
                    "Paste" => surface_action(surface, "paste_from_clipboard"),
                    "Split Right" => (cb.on_split_right)(),
                    "Split Down" => (cb.on_split_down)(),
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

/// Show a persistent OpenGL error toast centered in the terminal.
/// The toast must be manually dismissed via the × button; there is no auto-dismiss
/// because an OpenGL 4.3 failure is fatal for the terminal and must not be missed.
fn show_gl_error_toast(overlay: &gtk::Overlay, message: &str) {
    let toast = gtk::Box::new(gtk::Orientation::Vertical, 6);
    toast.set_halign(gtk::Align::Center);
    toast.set_valign(gtk::Align::Center);
    toast.set_margin_start(24);
    toast.set_margin_end(24);

    let provider = gtk::CssProvider::new();
    provider.load_from_data(
        "box.limux-gl-error { \
            background: rgba(160, 30, 30, 0.95); \
            color: white; \
            border-radius: 8px; \
            padding: 14px 20px; \
            font-size: 12px; \
        } \
        box.limux-gl-error label { color: white; } \
        box.limux-gl-error button { \
            color: rgba(255,255,255,0.7); \
            border: none; \
            background: none; \
            min-height: 0; min-width: 0; \
            padding: 0 4px; \
        } \
        box.limux-gl-error button:hover { color: white; }",
    );
    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("display"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    toast.add_css_class("limux-gl-error");
    let label = gtk::Label::new(Some(message));
    label.set_wrap(true);
    label.set_max_width_chars(60);
    label.set_selectable(true); // allow copying the error text
    let close_btn = gtk::Button::with_label("\u{00D7}"); // ×
    close_btn.set_halign(gtk::Align::End);
    toast.append(&label);
    toast.append(&close_btn);
    toast.set_can_target(false);

    overlay.add_overlay(&toast);

    // Close button dismisses
    {
        let t = toast.clone();
        let o = overlay.clone();
        close_btn.set_can_target(true);
        close_btn.connect_clicked(move |_| {
            o.remove_overlay(&t);
        });
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

// ---------------------------------------------------------------------------
// OpenGL 4.3 context creation (EGL-first, GLX fallback)
// GTK 4.6 uses EGL even on X11, so we try EGL before GLX.
// ---------------------------------------------------------------------------

mod glx_compat {
    use std::ffi::CString;
    use std::os::raw::{c_char, c_int, c_void};

    extern "C" {
        fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }

    const RTLD_NOW: c_int = 0x2;
    const RTLD_NOLOAD: c_int = 0x4;

    // GLX attribute constants
    const GLX_CONTEXT_MAJOR_VERSION_ARB: c_int = 0x2091;
    const GLX_CONTEXT_MINOR_VERSION_ARB: c_int = 0x2092;
    const GLX_CONTEXT_PROFILE_MASK_ARB: c_int = 0x9126;
    const GLX_CONTEXT_CORE_PROFILE_BIT_ARB: c_int = 0x0001;
    const GLX_FBCONFIG_ID: c_int = 0x8013;

    // EGL attribute constants
    const EGL_CONFIG_ID: c_int = 0x3028;
    const EGL_CONTEXT_MAJOR_VERSION: c_int = 0x3098;
    const EGL_CONTEXT_MINOR_VERSION: c_int = 0x30FB;
    const EGL_CONTEXT_OPENGL_PROFILE_MASK: c_int = 0x30FD;
    const EGL_CONTEXT_OPENGL_CORE_PROFILE_BIT: c_int = 0x00000001;
    const EGL_OPENGL_API: c_int = 0x30A2;
    const EGL_DRAW: c_int = 0x3059;
    const EGL_READ: c_int = 0x305A;
    const EGL_NONE: c_int = 0x3038;

    // ----- GLX function pointer types -----
    type FnGetProcAddress = unsafe extern "C" fn(*const c_char) -> *mut c_void;
    type FnGlxGetCurrentDisplay = unsafe extern "C" fn() -> *mut c_void;
    type FnGlxGetCurrentDrawable = unsafe extern "C" fn() -> usize;
    type FnGlxGetCurrentContext = unsafe extern "C" fn() -> *mut c_void;
    type FnGlxQueryContext =
        unsafe extern "C" fn(*mut c_void, *mut c_void, c_int, *mut c_int) -> c_int;
    type FnGlxChooseFBConfig =
        unsafe extern "C" fn(*mut c_void, c_int, *const c_int, *mut c_int) -> *mut *mut c_void;
    type FnGlxCreateContextAttribsARB =
        unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, c_int, *const c_int)
            -> *mut c_void;
    type FnGlxMakeCurrent = unsafe extern "C" fn(*mut c_void, usize, *mut c_void) -> c_int;
    type FnGlxDestroyContext = unsafe extern "C" fn(*mut c_void, *mut c_void);
    type FnGlxGetFBConfigAttrib =
        unsafe extern "C" fn(*mut c_void, *mut c_void, c_int, *mut c_int) -> c_int;

    // ----- EGL function pointer types -----
    type FnEglGetCurrentDisplay = unsafe extern "C" fn() -> *mut c_void;
    type FnEglGetCurrentContext = unsafe extern "C" fn() -> *mut c_void;
    type FnEglGetCurrentSurface = unsafe extern "C" fn(c_int) -> *mut c_void;
    type FnEglQueryContext =
        unsafe extern "C" fn(*mut c_void, *mut c_void, c_int, *mut c_int) -> c_int;
    type FnEglGetConfigs =
        unsafe extern "C" fn(*mut c_void, *mut *mut c_void, c_int, *mut c_int) -> c_int;
    type FnEglGetConfigAttrib =
        unsafe extern "C" fn(*mut c_void, *mut c_void, c_int, *mut c_int) -> c_int;
    type FnEglBindAPI = unsafe extern "C" fn(c_int) -> c_int;
    type FnEglCreateContext =
        unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *const c_int) -> *mut c_void;
    type FnEglMakeCurrent =
        unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void) -> c_int;
    type FnEglDestroyContext = unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int;

    enum CtxImpl {
        Glx {
            display: *mut c_void,
            ctx: *mut c_void,
            fn_make_current: FnGlxMakeCurrent,
            fn_get_drawable: FnGlxGetCurrentDrawable,
            fn_destroy: FnGlxDestroyContext,
        },
        Egl {
            display: *mut c_void,
            ctx: *mut c_void,
            fn_make_current: FnEglMakeCurrent,
            fn_get_surface: FnEglGetCurrentSurface,
            fn_destroy: FnEglDestroyContext,
        },
    }

    /// OpenGL 4.3 context created via raw GLX or EGL (GTK 4.6 uses EGL on X11).
    pub struct Glx43Ctx {
        inner: CtxImpl,
    }

    impl Glx43Ctx {
        /// Make this context current. Fetches drawable/surface fresh each call.
        pub fn make_current(&self) -> Result<(), String> {
            match &self.inner {
                CtxImpl::Glx { display, ctx, fn_make_current, fn_get_drawable, .. } => {
                    let drawable = unsafe { fn_get_drawable() };
                    if drawable == 0 {
                        return Err("glXGetCurrentDrawable returned 0".into());
                    }
                    let ok = unsafe { fn_make_current(*display, drawable, *ctx) };
                    if ok == 0 {
                        Err("glXMakeCurrent failed".into())
                    } else {
                        Ok(())
                    }
                }
                CtxImpl::Egl { display, ctx, fn_make_current, fn_get_surface, .. } => {
                    let draw = unsafe { fn_get_surface(EGL_DRAW) };
                    let read = unsafe { fn_get_surface(EGL_READ) };
                    let ok = unsafe { fn_make_current(*display, draw, read, *ctx) };
                    if ok == 0 {
                        Err("eglMakeCurrent failed".into())
                    } else {
                        Ok(())
                    }
                }
            }
        }

        pub fn destroy(self) {
            match self.inner {
                CtxImpl::Glx { display, ctx, fn_destroy, .. } => unsafe {
                    fn_destroy(display, ctx)
                },
                CtxImpl::Egl { display, ctx, fn_destroy, .. } => unsafe {
                    fn_destroy(display, ctx);
                },
            }
        }
    }

    unsafe fn transmute_fn<T: Copy>(ptr: *mut c_void) -> T {
        unsafe { *(&ptr as *const *mut c_void as *const T) }
    }

    fn dlsym_load(handle: *mut c_void, name: &str) -> Result<*mut c_void, String> {
        let cname = CString::new(name).unwrap();
        let ptr = unsafe { dlsym(handle, cname.as_ptr()) };
        if ptr.is_null() {
            Err(format!("dlsym could not load '{name}'"))
        } else {
            Ok(ptr)
        }
    }

    /// Try EGL path (GTK 4.6 default on X11).
    fn try_egl() -> Result<Glx43Ctx, String> {
        let libegl_name = CString::new("libEGL.so.1").unwrap();
        let libegl = unsafe { dlopen(libegl_name.as_ptr(), RTLD_NOW | RTLD_NOLOAD) };
        if libegl.is_null() {
            return Err("libEGL.so.1 not loaded".into());
        }
        let fn_get_display: FnEglGetCurrentDisplay =
            unsafe { transmute_fn(dlsym_load(libegl, "eglGetCurrentDisplay")?) };
        let fn_get_context: FnEglGetCurrentContext =
            unsafe { transmute_fn(dlsym_load(libegl, "eglGetCurrentContext")?) };
        let fn_get_surface: FnEglGetCurrentSurface =
            unsafe { transmute_fn(dlsym_load(libegl, "eglGetCurrentSurface")?) };
        let fn_query_context: FnEglQueryContext =
            unsafe { transmute_fn(dlsym_load(libegl, "eglQueryContext")?) };
        let fn_get_configs: FnEglGetConfigs =
            unsafe { transmute_fn(dlsym_load(libegl, "eglGetConfigs")?) };
        let fn_get_config_attrib: FnEglGetConfigAttrib =
            unsafe { transmute_fn(dlsym_load(libegl, "eglGetConfigAttrib")?) };
        let fn_bind_api: FnEglBindAPI =
            unsafe { transmute_fn(dlsym_load(libegl, "eglBindAPI")?) };
        let fn_create_context: FnEglCreateContext =
            unsafe { transmute_fn(dlsym_load(libegl, "eglCreateContext")?) };
        let fn_make_current: FnEglMakeCurrent =
            unsafe { transmute_fn(dlsym_load(libegl, "eglMakeCurrent")?) };
        let fn_destroy: FnEglDestroyContext =
            unsafe { transmute_fn(dlsym_load(libegl, "eglDestroyContext")?) };

        let display = unsafe { fn_get_display() };
        if display.is_null() {
            return Err("eglGetCurrentDisplay returned NULL — no current EGL context".into());
        }
        let share_ctx = unsafe { fn_get_context() };
        if share_ctx.is_null() {
            return Err("eglGetCurrentContext returned NULL".into());
        }

        // Find EGL config matching the current context's config
        let mut config_id: c_int = 0;
        unsafe { fn_query_context(display, share_ctx, EGL_CONFIG_ID, &mut config_id) };

        let mut n_configs: c_int = 0;
        unsafe { fn_get_configs(display, std::ptr::null_mut(), 0, &mut n_configs) };
        let mut configs: Vec<*mut c_void> = vec![std::ptr::null_mut(); n_configs as usize];
        if n_configs > 0 {
            unsafe { fn_get_configs(display, configs.as_mut_ptr(), n_configs, &mut n_configs) };
        }
        let mut chosen: *mut c_void = std::ptr::null_mut();
        for &cfg in &configs {
            let mut id: c_int = 0;
            unsafe { fn_get_config_attrib(display, cfg, EGL_CONFIG_ID, &mut id) };
            if id == config_id {
                chosen = cfg;
                break;
            }
        }
        if chosen.is_null() && !configs.is_empty() {
            chosen = configs[0];
        }
        if chosen.is_null() {
            return Err("could not find matching EGL config".into());
        }

        // Bind desktop OpenGL API and create 4.3 core context
        unsafe { fn_bind_api(EGL_OPENGL_API) };
        let attribs: [c_int; 7] = [
            EGL_CONTEXT_MAJOR_VERSION, 4,
            EGL_CONTEXT_MINOR_VERSION, 3,
            EGL_CONTEXT_OPENGL_PROFILE_MASK, EGL_CONTEXT_OPENGL_CORE_PROFILE_BIT,
            EGL_NONE,
        ];
        let ctx_43 = unsafe { fn_create_context(display, chosen, share_ctx, attribs.as_ptr()) };
        if ctx_43.is_null() {
            return Err("eglCreateContext(4.3) returned NULL".into());
        }
        let draw = unsafe { fn_get_surface(EGL_DRAW) };
        let read = unsafe { fn_get_surface(EGL_READ) };
        let ok = unsafe { fn_make_current(display, draw, read, ctx_43) };
        if ok == 0 {
            unsafe { fn_destroy(display, ctx_43) };
            return Err("eglMakeCurrent(4.3 ctx) failed".into());
        }
        eprintln!("limux: loaded OpenGL 4.3 (EGL backend)");
        Ok(Glx43Ctx {
            inner: CtxImpl::Egl { display, ctx: ctx_43, fn_make_current, fn_get_surface, fn_destroy },
        })
    }

    /// Try GLX path (fallback for GTK+GLX setups).
    fn load_glx_proc(get_proc: FnGetProcAddress, name: &str) -> Result<*mut c_void, String> {
        let cname = CString::new(name).unwrap();
        let ptr = unsafe { get_proc(cname.as_ptr()) };
        if ptr.is_null() {
            Err(format!("glXGetProcAddressARB could not load '{name}'"))
        } else {
            Ok(ptr)
        }
    }

    fn try_glx() -> Result<Glx43Ctx, String> {
        let libgl_name = CString::new("libGL.so.1").unwrap();
        let libgl = unsafe { dlopen(libgl_name.as_ptr(), RTLD_NOW | RTLD_NOLOAD) };
        if libgl.is_null() {
            return Err("libGL.so.1 not loaded".into());
        }
        let gpa_ptr = unsafe { dlsym(libgl, b"glXGetProcAddressARB\0".as_ptr() as *const c_char) };
        if gpa_ptr.is_null() {
            return Err("glXGetProcAddressARB not found".into());
        }
        let get_proc: FnGetProcAddress = unsafe { transmute_fn(gpa_ptr) };

        let fn_get_display: FnGlxGetCurrentDisplay =
            unsafe { transmute_fn(load_glx_proc(get_proc, "glXGetCurrentDisplay")?) };
        let fn_get_drawable: FnGlxGetCurrentDrawable =
            unsafe { transmute_fn(load_glx_proc(get_proc, "glXGetCurrentDrawable")?) };
        let fn_get_context: FnGlxGetCurrentContext =
            unsafe { transmute_fn(load_glx_proc(get_proc, "glXGetCurrentContext")?) };
        let fn_query_context: FnGlxQueryContext =
            unsafe { transmute_fn(load_glx_proc(get_proc, "glXQueryContext")?) };
        let fn_choose_fbconfig: FnGlxChooseFBConfig =
            unsafe { transmute_fn(load_glx_proc(get_proc, "glXChooseFBConfig")?) };
        let fn_create_ctx: FnGlxCreateContextAttribsARB =
            unsafe { transmute_fn(load_glx_proc(get_proc, "glXCreateContextAttribsARB")?) };
        let fn_make_current: FnGlxMakeCurrent =
            unsafe { transmute_fn(load_glx_proc(get_proc, "glXMakeCurrent")?) };
        let fn_destroy: FnGlxDestroyContext =
            unsafe { transmute_fn(load_glx_proc(get_proc, "glXDestroyContext")?) };
        let fn_get_fbconfig_attrib: FnGlxGetFBConfigAttrib =
            unsafe { transmute_fn(load_glx_proc(get_proc, "glXGetFBConfigAttrib")?) };

        // 4. Query current display / drawable / context from GTK's 3.2 ctx
        let display = unsafe { fn_get_display() };
        if display.is_null() {
            return Err("glXGetCurrentDisplay returned NULL".into());
        }
        let drawable = unsafe { fn_get_drawable() };
        if drawable == 0 {
            return Err("glXGetCurrentDrawable returned 0 — no current drawable".into());
        }
        let share_ctx = unsafe { fn_get_context() };
        if share_ctx.is_null() {
            return Err("glXGetCurrentContext returned NULL — no current context".into());
        }

        // 5. Get the FBConfig ID from the current context, then retrieve the FBConfig
        let mut fbconfig_id: c_int = 0;
        let qc_result =
            unsafe { fn_query_context(display, share_ctx, GLX_FBCONFIG_ID, &mut fbconfig_id) };
        if qc_result != 0 {
            return Err(format!(
                "glXQueryContext(GLX_FBCONFIG_ID) failed with {qc_result}"
            ));
        }

        // Get screen number from context
        // GLX_SCREEN = 0x800C
        let mut screen: c_int = 0;
        unsafe { fn_query_context(display, share_ctx, 0x800C, &mut screen) };

        // Choose FBConfig matching the ID from the current context
        let mut n_configs: c_int = 0;
        let fbconfigs_ptr =
            unsafe { fn_choose_fbconfig(display, screen, std::ptr::null(), &mut n_configs) };
        if fbconfigs_ptr.is_null() || n_configs == 0 {
            return Err("glXChooseFBConfig returned no configs".into());
        }
        let fbconfigs = unsafe { std::slice::from_raw_parts(fbconfigs_ptr, n_configs as usize) };

        // Find the FBConfig whose ID matches
        let mut chosen_fbconfig: *mut c_void = std::ptr::null_mut();
        for &fb in fbconfigs {
            let mut id: c_int = 0;
            unsafe { fn_get_fbconfig_attrib(display, fb, GLX_FBCONFIG_ID, &mut id) };
            if id == fbconfig_id {
                chosen_fbconfig = fb;
                break;
            }
        }

        // Free the list returned by glXChooseFBConfig (it's an Xlib-allocated array)
        // We need XFree for this; use dlsym on libX11.
        {
            let libx11_name = CString::new("libX11.so.6").unwrap();
            let libx11 = unsafe { dlopen(libx11_name.as_ptr(), RTLD_NOW | RTLD_NOLOAD) };
            if !libx11.is_null() {
                let xfree_name = CString::new("XFree").unwrap();
                let xfree_ptr = unsafe { dlsym(libx11, xfree_name.as_ptr()) };
                if !xfree_ptr.is_null() {
                    let xfree: unsafe extern "C" fn(*mut c_void) =
                        unsafe { transmute_fn(xfree_ptr) };
                    unsafe { xfree(fbconfigs_ptr as *mut c_void) };
                }
            }
        }

        if chosen_fbconfig.is_null() {
            // Fall back to first config if no exact match
            // Re-query since we freed the list above
            let mut n2: c_int = 0;
            let fb2 =
                unsafe { fn_choose_fbconfig(display, screen, std::ptr::null(), &mut n2) };
            if fb2.is_null() || n2 == 0 {
                return Err("glXChooseFBConfig fallback returned no configs".into());
            }
            chosen_fbconfig = unsafe { *fb2 };
            // Leak fb2 list (minor; only happens once on mismatch)
        }

        // 6. Create the 4.3 core context sharing objects with GTK's 3.2 ctx
        let attribs: [c_int; 7] = [
            GLX_CONTEXT_MAJOR_VERSION_ARB,
            4,
            GLX_CONTEXT_MINOR_VERSION_ARB,
            3,
            GLX_CONTEXT_PROFILE_MASK_ARB,
            GLX_CONTEXT_CORE_PROFILE_BIT_ARB,
            0, // terminator
        ];
        let ctx_43 = unsafe {
            fn_create_ctx(display, chosen_fbconfig, share_ctx, 1, attribs.as_ptr())
        };
        if ctx_43.is_null() {
            return Err(
                "glXCreateContextAttribsARB returned NULL — driver may not support OpenGL 4.3"
                    .into(),
            );
        }

        // 7. Make the 4.3 context current
        let ok = unsafe { fn_make_current(display, drawable, ctx_43) };
        if ok == 0 {
            unsafe { fn_destroy(display, ctx_43) };
            return Err("glXMakeCurrent(4.3 ctx) returned False".into());
        }

        eprintln!("limux: loaded OpenGL 4.3 (GLX backend)");
        Ok(Glx43Ctx {
            inner: CtxImpl::Glx {
                display,
                ctx: ctx_43,
                fn_make_current,
                fn_get_drawable,
                fn_destroy,
            },
        })
    }

    /// Public entry point: select backend via LIMUX_GL_BACKEND env var.
    ///
    /// LIMUX_GL_BACKEND=auto  (default) — try EGL first, then GLX
    /// LIMUX_GL_BACKEND=egl            — force EGL only
    /// LIMUX_GL_BACKEND=glx            — force GLX only
    ///
    /// EGL is tried first by default because GTK 4.6 on Ubuntu 22.04 uses
    /// EGL even on X11 (not GLX), so glXGetCurrentDisplay() returns NULL.
    pub fn create_glx43_context() -> Result<Glx43Ctx, String> {
        let backend = std::env::var("LIMUX_GL_BACKEND")
            .unwrap_or_else(|_| "auto".to_string());

        match backend.to_lowercase().as_str() {
            "egl" => {
                eprintln!("limux: LIMUX_GL_BACKEND=egl — using EGL backend");
                try_egl()
            }
            "glx" => {
                eprintln!("limux: LIMUX_GL_BACKEND=glx — using GLX backend");
                try_glx()
            }
            _ => {
                // auto (default): EGL first (GTK 4.6 on X11 uses EGL), then GLX
                match try_egl() {
                    Ok(ctx) => Ok(ctx),
                    Err(egl_err) => {
                        eprintln!("limux: EGL 4.3 failed ({egl_err}), trying GLX...");
                        try_glx()
                    }
                }
            }
        }
    }
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
    fn key_event_text_preserves_printable_chords() {
        let ctrl_shift_h = key_event_text(gtk::gdk::Key::H).and_then(|s| s.into_string().ok());
        let alt_shift_gt =
            key_event_text(gtk::gdk::Key::greater).and_then(|s| s.into_string().ok());

        assert_eq!(ctrl_shift_h.as_deref(), Some("H"));
        assert_eq!(alt_shift_gt.as_deref(), Some(">"));
        assert!(key_event_text(gtk::gdk::Key::BackSpace).is_none());
    }
}
