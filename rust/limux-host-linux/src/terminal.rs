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
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use limux_ghostty_sys::*;

use crate::app_config::LinkOpenDestination;
use crate::link_uri;
use crate::shortcut_config::NormalizedShortcut;

// ---------------------------------------------------------------------------
// Global Ghostty app singleton
// ---------------------------------------------------------------------------

struct GhosttyState {
    app: ghostty_app_t,
    background_opacity: f64,
}

// Safety: ghostty_app_t is thread-safe for the operations we perform
unsafe impl Send for GhosttyState {}
unsafe impl Sync for GhosttyState {}

static GHOSTTY: OnceLock<GhosttyState> = OnceLock::new();
static CURRENT_COLOR_SCHEME: AtomicI32 = AtomicI32::new(GHOSTTY_COLOR_SCHEME_LIGHT);
static CURRENT_SCROLLBAR_ENABLED: AtomicBool = AtomicBool::new(true);
static WAKEUP_IDLE_QUEUED: AtomicBool = AtomicBool::new(false);
static EMPTY_CLIPBOARD_TEXT: [u8; 1] = [0];

type TitleChangedCallback = dyn Fn(&str);
type PwdChangedCallback = dyn Fn(&str);
type DesktopNotificationCallback = dyn Fn(&str, &str, bool);
type BellCallback = dyn Fn(bool);
type OpenUrlCallback = dyn Fn(&str, LinkOpenRequest);
type VoidCallback = dyn Fn();
type WidgetCallback = dyn Fn(&gtk::Widget);
type IdentityCallback = dyn Fn() -> TerminalIdentity;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkOpenRequest {
    Configured,
    Destination(LinkOpenDestination),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalIdentity {
    pub workspace_id: Option<String>,
    pub surface_id: String,
}

/// Vertical pixel gap between the cursor and the bottom edge of the hover
/// URL preview popover. Roughly the height of a large accessibility cursor
/// so the popover stays visible even when the user has a >32 px pointer
/// skin enabled.
const LINK_PREVIEW_CURSOR_Y_GAP: i32 = 14;

/// Per-surface state, stored in a global registry keyed by surface pointer.
struct SurfaceEntry {
    identity: SurfaceIdentity,
    gl_area: gtk::GLArea,
    toast_overlay: gtk::Overlay,
    scrollbar: gtk::Scrollbar,
    scrollbar_adjustment: gtk::Adjustment,
    scrollbar_syncing: Rc<Cell<bool>>,
    on_title_changed: Option<Box<TitleChangedCallback>>,
    on_pwd_changed: Option<Box<PwdChangedCallback>>,
    on_desktop_notification: Option<Box<DesktopNotificationCallback>>,
    on_bell: Option<Box<BellCallback>>,
    on_open_url: Option<Rc<OpenUrlCallback>>,
    on_close: Option<Box<VoidCallback>>,
    clipboard_context: *mut ClipboardContext,
    // Hover URL preview for OSC 8 hyperlinks. The popover is a child of
    // `gl_area` so it inherits libadwaita's popover styling — matching the
    // right-click context menu by construction.
    link_popover: gtk::Popover,
    link_label: gtk::Label,
    cursor_pos: Rc<Cell<(f64, f64)>>,
    mouse_cursor: Cell<MouseCursorState>,
}

struct ClipboardContext {
    surface: Cell<ghostty_surface_t>,
    copy_selection_to_clipboard: Rc<dyn Fn() -> bool>,
    url_probe: RefCell<Option<String>>,
    url_probe_active: Cell<bool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ClipboardWritePolicy {
    write_clipboard: bool,
    write_primary: bool,
    show_toast: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MouseCursorState {
    shape: ghostty_action_mouse_shape_e,
    visible: bool,
}

impl Default for MouseCursorState {
    fn default() -> Self {
        Self {
            shape: GHOSTTY_MOUSE_SHAPE_TEXT,
            visible: true,
        }
    }
}

impl MouseCursorState {
    fn update_shape(self, shape: ghostty_action_mouse_shape_e) -> Self {
        Self { shape, ..self }
    }

    fn update_visibility(self, visibility: ghostty_action_mouse_visibility_e) -> Self {
        Self {
            visible: visibility != GHOSTTY_MOUSE_HIDDEN,
            ..self
        }
    }

    fn gtk_cursor_name(self) -> &'static str {
        if self.visible {
            gtk_cursor_name_for_ghostty_shape(self.shape)
        } else {
            "none"
        }
    }
}

#[derive(Clone, Copy)]
enum MouseCursorUpdate {
    Shape(ghostty_action_mouse_shape_e),
    Visibility(ghostty_action_mouse_visibility_e),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SurfaceIdentity {
    surface_key: usize,
    generation: u64,
}

#[derive(Default)]
struct SurfaceIdentityRegistry {
    next_generation: u64,
    active_generations: HashMap<usize, u64>,
}

impl SurfaceIdentityRegistry {
    fn register(&mut self, surface_key: usize) -> SurfaceIdentity {
        self.next_generation = self.next_generation.wrapping_add(1);
        let identity = SurfaceIdentity {
            surface_key,
            generation: self.next_generation,
        };
        self.active_generations
            .insert(surface_key, identity.generation);
        identity
    }

    fn current(&self, surface_key: usize) -> Option<SurfaceIdentity> {
        self.active_generations
            .get(&surface_key)
            .copied()
            .map(|generation| SurfaceIdentity {
                surface_key,
                generation,
            })
    }

    fn is_current(&self, identity: SurfaceIdentity) -> bool {
        self.current(identity.surface_key) == Some(identity)
    }

    fn unregister(&mut self, identity: SurfaceIdentity) {
        if self.is_current(identity) {
            self.active_generations.remove(&identity.surface_key);
        }
    }
}

thread_local! {
    static SURFACE_MAP: RefCell<HashMap<usize, SurfaceEntry>> = RefCell::new(HashMap::new());
}

static SURFACE_IDENTITIES: OnceLock<Mutex<SurfaceIdentityRegistry>> = OnceLock::new();

fn surface_identity_registry() -> &'static Mutex<SurfaceIdentityRegistry> {
    SURFACE_IDENTITIES.get_or_init(|| Mutex::new(SurfaceIdentityRegistry::default()))
}

fn register_surface_identity(surface_key: usize) -> SurfaceIdentity {
    surface_identity_registry()
        .lock()
        .expect("surface identity registry poisoned")
        .register(surface_key)
}

fn current_surface_identity(surface_key: usize) -> Option<SurfaceIdentity> {
    surface_identity_registry()
        .lock()
        .expect("surface identity registry poisoned")
        .current(surface_key)
}

fn unregister_surface_identity(identity: SurfaceIdentity) {
    surface_identity_registry()
        .lock()
        .expect("surface identity registry poisoned")
        .unregister(identity);
}

#[derive(Clone)]
pub struct TerminalHandle {
    surface_cell: Rc<RefCell<Option<ghostty_surface_t>>>,
    clipboard_context_cell: Rc<Cell<*mut ClipboardContext>>,
    shutting_down: Rc<Cell<bool>>,
    gl_area: gtk::GLArea,
    search_bar: gtk::SearchBar,
    search_entry: gtk::SearchEntry,
    callbacks: Rc<RefCell<TerminalCallbacks>>,
    im_context: gtk::IMMulticontext,
    im_fallback: gtk::IMContextSimple,
    signal_handlers: Rc<RefCell<Vec<(glib::Object, glib::SignalHandlerId)>>>,
    controllers: Rc<RefCell<Vec<gtk::EventController>>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalHealth {
    pub realized: bool,
    pub process_exited: bool,
    pub columns: u16,
    pub rows: u16,
    pub width_px: u32,
    pub height_px: u32,
}

impl TerminalHandle {
    pub fn replace_callbacks(&self, callbacks: TerminalCallbacks) {
        if !self.shutting_down.get() {
            *self.callbacks.borrow_mut() = callbacks;
        }
    }

    pub fn shutdown(&self) {
        if self.shutting_down.replace(true) {
            return;
        }

        for (object, handler) in self.signal_handlers.borrow_mut().drain(..) {
            object.disconnect(handler);
        }
        for controller in self.controllers.borrow_mut().drain(..) {
            self.gl_area.remove_controller(&controller);
        }
        self.im_context.set_client_widget(gtk::Widget::NONE);
        self.im_fallback.set_client_widget(gtk::Widget::NONE);

        if let Some(surface) = self.surface_cell.borrow_mut().take() {
            free_terminal_surface(surface, &self.clipboard_context_cell);
        } else {
            let clipboard_context = self.clipboard_context_cell.replace(ptr::null_mut());
            if !clipboard_context.is_null() {
                unsafe {
                    drop(Box::from_raw(clipboard_context));
                }
            }
        }

        *self.callbacks.borrow_mut() = TerminalCallbacks::disconnected();
    }

    pub fn focus_surface(&self) -> bool {
        self.refresh_display();
        self.gl_area.grab_focus();
        true
    }

    pub fn refresh_display(&self) {
        let Some(surface) = *self.surface_cell.borrow() else {
            self.gl_area.queue_render();
            return;
        };

        refresh_realized_surface_display(surface, &self.gl_area);
    }

    pub fn perform_binding_action(&self, action: &str) -> bool {
        let surface = *self.surface_cell.borrow();
        surface_action(surface, action);
        surface.is_some()
    }

    pub fn copy_selection_to_clipboard(&self) -> bool {
        let Some(surface) = *self.surface_cell.borrow() else {
            return false;
        };
        if !unsafe { ghostty_surface_has_selection(surface) } {
            return false;
        }

        surface_action(Some(surface), "copy_to_clipboard");
        true
    }

    /// Inject text into the terminal surface for control-socket requests and
    /// drag/drop payloads. Ghostty treats this as pasted text, which matches
    /// the current control protocol semantics.
    pub fn send_text(&self, text: &str) -> bool {
        let Some(surface) = *self.surface_cell.borrow() else {
            return false;
        };

        unsafe {
            ghostty_surface_text(surface, text.as_ptr() as *const c_char, text.len());
        }
        true
    }

    pub fn send_key(&self, key: &str) -> bool {
        let Some(surface) = *self.surface_cell.borrow() else {
            return false;
        };

        let Ok(binding) = NormalizedShortcut::parse(key) else {
            return false;
        };
        let Some((keyval, modifier)) = gtk::accelerator_parse(binding.to_config_accel()) else {
            return false;
        };

        // A key needs BOTH halves, and supplying only one drops it silently.
        //
        // `keycode` — ghostty resolves the *physical* key from the hardware keycode
        // and does not fall back to the keyval. With `keycode: 0` it sees an
        // unidentified key and drops the event, so Enter, arrows, F-keys and
        // ctrl-chords never reach the PTY.
        //
        // `text` — ghostty writes ordinary printable input from the text field, not
        // from the keycode (see the comment on the GTK key controller below: "Ghostty
        // uses the text field for actual character input and the keycode for
        // bindings"). With `text` null, `send-key a` is encoded as a keypress that
        // produces no character, so it too vanishes.
        //
        // The GTK path gets `text` from the input method; a socket-injected key has
        // no IM behind it, so derive it here the same way GTK's fallback does.
        // `key_event_text` returns None for control characters, which is what we want:
        // Enter and ctrl-chords must be *encoded* by ghostty from the key + mods, not
        // written as literal bytes.
        let keycode = keycode_for_keyval(self.gl_area.upcast_ref(), keyval);
        let text_keyval =
            translated_keyval_for_keycode(self.gl_area.upcast_ref(), keyval, keycode, modifier);
        let text = key_event_text(text_keyval);

        let mut press = translate_key_event(
            GHOSTTY_ACTION_PRESS,
            Some(self.gl_area.upcast_ref()),
            None,
            keyval,
            keycode,
            modifier,
        );
        // The CString must outlive the ghostty call below; `text` owns it until the
        // end of this function.
        if let Some(text) = text.as_ref() {
            press.text = text.as_ptr();
        }

        // Release carries no text, matching the GTK controller.
        let release = translate_key_event(
            GHOSTTY_ACTION_RELEASE,
            Some(self.gl_area.upcast_ref()),
            None,
            keyval,
            keycode,
            modifier,
        );

        unsafe {
            ghostty_surface_key(surface, press);
            ghostty_surface_key(surface, release);
        }
        drop(text);
        true
    }

    pub fn health(&self) -> TerminalHealth {
        let Some(surface) = *self.surface_cell.borrow() else {
            return TerminalHealth {
                realized: false,
                process_exited: false,
                columns: 0,
                rows: 0,
                width_px: 0,
                height_px: 0,
            };
        };

        let size = unsafe { ghostty_surface_size(surface) };
        TerminalHealth {
            realized: true,
            process_exited: unsafe { ghostty_surface_process_exited(surface) },
            columns: size.columns,
            rows: size.rows,
            width_px: size.width_px,
            height_px: size.height_px,
        }
    }

    pub fn read_viewport_text(&self) -> Option<String> {
        let Some(surface) = *self.surface_cell.borrow() else {
            return Some(String::new());
        };

        let selection = ghostty_selection_s {
            top_left: ghostty_point_s {
                tag: GHOSTTY_POINT_VIEWPORT,
                coord: GHOSTTY_POINT_COORD_TOP_LEFT,
                x: 0,
                y: 0,
            },
            bottom_right: ghostty_point_s {
                tag: GHOSTTY_POINT_VIEWPORT,
                coord: GHOSTTY_POINT_COORD_BOTTOM_RIGHT,
                x: 0,
                y: 0,
            },
            rectangle: false,
        };
        let mut text = empty_ghostty_text();

        let has_text = unsafe { ghostty_surface_read_text(surface, selection, &mut text) };
        if !has_text || text.text.is_null() {
            return Some(String::new());
        }

        let bytes = unsafe { std::slice::from_raw_parts(text.text.cast::<u8>(), text.text_len) };
        let output = String::from_utf8_lossy(bytes).into_owned();
        unsafe { ghostty_surface_free_text(surface, &mut text) };
        Some(output)
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

        let mut text = empty_ghostty_text();

        let has_selection = unsafe { ghostty_surface_read_selection(surface, &mut text) };
        if !has_selection || text.text.is_null() {
            return String::new();
        }

        let bytes = unsafe { std::slice::from_raw_parts(text.text.cast::<u8>(), text.text_len) };
        let selection = String::from_utf8_lossy(bytes).into_owned();
        unsafe { ghostty_surface_free_text(surface, &mut text) };
        selection
    }
}

fn track_signal<T>(
    handlers: &RefCell<Vec<(glib::Object, glib::SignalHandlerId)>>,
    object: &T,
    handler: glib::SignalHandlerId,
) where
    T: IsA<glib::Object> + Clone,
{
    handlers
        .borrow_mut()
        .push((object.clone().upcast(), handler));
}

fn empty_ghostty_text() -> ghostty_text_s {
    ghostty_text_s {
        tl_px_x: 0.0,
        tl_px_y: 0.0,
        offset_start: 0,
        offset_len: 0,
        text: ptr::null(),
        text_len: 0,
    }
}

pub struct TerminalWidget {
    pub root: gtk::Widget,
    pub handle: TerminalHandle,
}

fn terminal_search_action(query: &str) -> String {
    format!("search:{query}")
}

fn request_terminal_focus(gl_area: &gtk::GLArea, had_focus: &Cell<bool>) {
    had_focus.set(true);
    gl_area.grab_focus();
}

fn physical_size_for_allocation(
    logical_width: i32,
    logical_height: i32,
    scale_factor: i32,
) -> Option<(u32, u32, u32)> {
    let logical_width = u32::try_from(logical_width).ok()?;
    let logical_height = u32::try_from(logical_height).ok()?;
    let scale_factor = u32::try_from(scale_factor).ok()?;

    if logical_width == 0 || logical_height == 0 || scale_factor == 0 {
        return None;
    }

    let physical_width = logical_width.checked_mul(scale_factor)?;
    let physical_height = logical_height.checked_mul(scale_factor)?;
    Some((physical_width, physical_height, scale_factor))
}

fn refresh_surface_display(surface: ghostty_surface_t, gl_area: &gtk::GLArea) {
    let alloc = gl_area.allocation();
    if let Some((width, height, scale_factor)) =
        physical_size_for_allocation(alloc.width(), alloc.height(), gl_area.scale_factor())
    {
        let scale = scale_factor as f64;
        unsafe {
            ghostty_surface_set_content_scale(surface, scale, scale);
            ghostty_surface_set_size(surface, width, height);
        }
    }
    unsafe { ghostty_surface_refresh(surface) };
    gl_area.queue_render();
}

fn refresh_realized_surface_display(surface: ghostty_surface_t, gl_area: &gtk::GLArea) {
    if gl_area.is_realized() {
        gl_area.make_current();
        if gl_area.error().is_none() {
            unsafe { ghostty_surface_display_realized(surface) };
        }
    }
    refresh_surface_display(surface, gl_area);
}

fn load_ghostty_config() -> ghostty_config_t {
    unsafe {
        let config = ghostty_config_new();
        ghostty_config_load_default_files(config);
        ghostty_config_load_recursive_files(config);
        ghostty_config_finalize(config);
        config
    }
}

/// Initialize the global Ghostty app. Must be called once before creating surfaces.
pub fn init_ghostty() {
    GHOSTTY.get_or_init(|| {
        unsafe {
            ghostty_init(0, ptr::null_mut());
        }

        let config = load_ghostty_config();
        let background_opacity = load_background_opacity(config);
        CURRENT_SCROLLBAR_ENABLED.store(load_scrollbar_enabled(config), Ordering::Relaxed);

        let runtime_config = ghostty_runtime_config_s {
            userdata: ptr::null_mut(),
            supports_selection_clipboard: true,
            wakeup_cb: ghostty_wakeup_cb,
            action_cb: ghostty_action_cb,
            read_clipboard_cb: ghostty_read_clipboard_cb,
            confirm_read_clipboard_cb: ghostty_confirm_read_clipboard_cb,
            write_clipboard_cb: ghostty_write_clipboard_cb,
            close_surface_cb: ghostty_close_surface_cb,
            tmux_control_cb: None,
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

        GhosttyState {
            app,
            background_opacity,
        }
    });
}

fn ghostty_app() -> ghostty_app_t {
    GHOSTTY.get().expect("ghostty not initialized").app
}

pub fn ghostty_background_opacity() -> f64 {
    init_ghostty();
    GHOSTTY
        .get()
        .map(|state| state.background_opacity)
        .unwrap_or(1.0)
}

fn load_background_opacity(config: ghostty_config_t) -> f64 {
    let mut opacity = 1.0_f64;
    let key = b"background-opacity";
    let loaded = unsafe {
        ghostty_config_get(
            config,
            (&mut opacity as *mut f64).cast::<c_void>(),
            key.as_ptr().cast::<c_char>(),
            key.len(),
        )
    };

    if loaded && opacity.is_finite() {
        opacity.clamp(0.0, 1.0)
    } else {
        1.0
    }
}

fn load_scrollbar_enabled(config: ghostty_config_t) -> bool {
    let mut value: *const c_char = ptr::null();
    let key = b"scrollbar";
    let loaded = unsafe {
        ghostty_config_get(
            config,
            (&mut value as *mut *const c_char).cast::<c_void>(),
            key.as_ptr().cast::<c_char>(),
            key.len(),
        )
    };

    !loaded || value.is_null() || unsafe { std::ffi::CStr::from_ptr(value) }.to_bytes() != b"never"
}

fn ghostty_color_scheme_for_dark_mode(dark: bool) -> c_int {
    if dark {
        GHOSTTY_COLOR_SCHEME_DARK
    } else {
        GHOSTTY_COLOR_SCHEME_LIGHT
    }
}

fn current_ghostty_color_scheme() -> c_int {
    CURRENT_COLOR_SCHEME.load(Ordering::Relaxed)
}

fn gtk_cursor_name_for_ghostty_shape(shape: ghostty_action_mouse_shape_e) -> &'static str {
    match shape {
        GHOSTTY_MOUSE_SHAPE_DEFAULT => "default",
        GHOSTTY_MOUSE_SHAPE_CONTEXT_MENU => "context-menu",
        GHOSTTY_MOUSE_SHAPE_HELP => "help",
        GHOSTTY_MOUSE_SHAPE_POINTER => "pointer",
        GHOSTTY_MOUSE_SHAPE_PROGRESS => "progress",
        GHOSTTY_MOUSE_SHAPE_WAIT => "wait",
        GHOSTTY_MOUSE_SHAPE_CELL => "cell",
        GHOSTTY_MOUSE_SHAPE_CROSSHAIR => "crosshair",
        GHOSTTY_MOUSE_SHAPE_TEXT => "text",
        GHOSTTY_MOUSE_SHAPE_VERTICAL_TEXT => "vertical-text",
        GHOSTTY_MOUSE_SHAPE_ALIAS => "alias",
        GHOSTTY_MOUSE_SHAPE_COPY => "copy",
        GHOSTTY_MOUSE_SHAPE_MOVE => "move",
        GHOSTTY_MOUSE_SHAPE_NO_DROP => "no-drop",
        GHOSTTY_MOUSE_SHAPE_NOT_ALLOWED => "not-allowed",
        GHOSTTY_MOUSE_SHAPE_GRAB => "grab",
        GHOSTTY_MOUSE_SHAPE_GRABBING => "grabbing",
        GHOSTTY_MOUSE_SHAPE_ALL_SCROLL => "all-scroll",
        GHOSTTY_MOUSE_SHAPE_COL_RESIZE => "col-resize",
        GHOSTTY_MOUSE_SHAPE_ROW_RESIZE => "row-resize",
        GHOSTTY_MOUSE_SHAPE_N_RESIZE => "n-resize",
        GHOSTTY_MOUSE_SHAPE_E_RESIZE => "e-resize",
        GHOSTTY_MOUSE_SHAPE_S_RESIZE => "s-resize",
        GHOSTTY_MOUSE_SHAPE_W_RESIZE => "w-resize",
        GHOSTTY_MOUSE_SHAPE_NE_RESIZE => "ne-resize",
        GHOSTTY_MOUSE_SHAPE_NW_RESIZE => "nw-resize",
        GHOSTTY_MOUSE_SHAPE_SE_RESIZE => "se-resize",
        GHOSTTY_MOUSE_SHAPE_SW_RESIZE => "sw-resize",
        GHOSTTY_MOUSE_SHAPE_EW_RESIZE => "ew-resize",
        GHOSTTY_MOUSE_SHAPE_NS_RESIZE => "ns-resize",
        GHOSTTY_MOUSE_SHAPE_NESW_RESIZE => "nesw-resize",
        GHOSTTY_MOUSE_SHAPE_NWSE_RESIZE => "nwse-resize",
        GHOSTTY_MOUSE_SHAPE_ZOOM_IN => "zoom-in",
        GHOSTTY_MOUSE_SHAPE_ZOOM_OUT => "zoom-out",
        _ => "default",
    }
}

fn set_gtk_mouse_cursor(widget: &impl IsA<gtk::Widget>, state: MouseCursorState) {
    widget.set_cursor_from_name(Some(state.gtk_cursor_name()));
}

fn apply_mouse_cursor_update(identity: SurfaceIdentity, update: MouseCursorUpdate) {
    SURFACE_MAP.with(|map| {
        let map = map.borrow();
        let Some(entry) = map.get(&identity.surface_key) else {
            return;
        };
        if entry.identity != identity {
            return;
        }
        let state = match update {
            MouseCursorUpdate::Shape(shape) => entry.mouse_cursor.get().update_shape(shape),
            MouseCursorUpdate::Visibility(visibility) => {
                entry.mouse_cursor.get().update_visibility(visibility)
            }
        };
        entry.mouse_cursor.set(state);
        set_gtk_mouse_cursor(&entry.gl_area, state);
    });
}

fn dispatch_mouse_cursor_update(surface_key: usize, update: MouseCursorUpdate) {
    let Some(identity) = current_surface_identity(surface_key) else {
        return;
    };

    if glib::MainContext::default().is_owner() {
        apply_mouse_cursor_update(identity, update);
    } else {
        // Capture copied payload and surface generation. GTK access happens
        // later, only if this pointer still identifies the same surface.
        glib::idle_add_once(move || apply_mouse_cursor_update(identity, update));
    }
}

pub fn sync_color_scheme(dark: bool) {
    let scheme = ghostty_color_scheme_for_dark_mode(dark);
    CURRENT_COLOR_SCHEME.store(scheme, Ordering::Relaxed);
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

fn claim_wakeup_idle_slot(flag: &AtomicBool) -> bool {
    !flag.swap(true, Ordering::AcqRel)
}

fn release_wakeup_idle_slot(flag: &AtomicBool) {
    flag.store(false, Ordering::Release);
}

unsafe extern "C" fn ghostty_wakeup_cb(_userdata: *mut c_void) {
    // Collapse renderer wakeups to a single pending idle source so text floods
    // do not enqueue unbounded GTK callbacks on the main thread.
    if claim_wakeup_idle_slot(&WAKEUP_IDLE_QUEUED) {
        glib::idle_add_once(|| {
            release_wakeup_idle_slot(&WAKEUP_IDLE_QUEUED);
            let app = ghostty_app();
            unsafe { ghostty_app_tick(app) };
        });
    }
    glib::MainContext::default().wakeup();
}

unsafe extern "C" fn ghostty_action_cb(
    app: ghostty_app_t,
    target: ghostty_target_s,
    action: ghostty_action_s,
) -> bool {
    let tag = action.tag;

    match tag {
        GHOSTTY_ACTION_SCROLLBAR => {
            if target.tag == GHOSTTY_TARGET_SURFACE {
                let surface_key = unsafe { target.target.surface } as usize;
                let scrollbar = unsafe { action.action.scrollbar };
                SURFACE_MAP.with(|map| {
                    if let Some(entry) = map.borrow().get(&surface_key) {
                        entry.scrollbar_syncing.set(true);
                        entry.scrollbar_adjustment.configure(
                            scrollbar.offset as f64,
                            0.0,
                            scrollbar.total as f64,
                            1.0,
                            scrollbar.len as f64,
                            scrollbar.len as f64,
                        );
                        entry.scrollbar_syncing.set(false);
                        entry.scrollbar.set_visible(
                            CURRENT_SCROLLBAR_ENABLED.load(Ordering::Relaxed)
                                && scrollbar.total > scrollbar.len,
                        );
                    }
                });
            }
            true
        }
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
                            cb(&title, &body, entry.gl_area.is_focus());
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
        GHOSTTY_ACTION_MOUSE_SHAPE => {
            if target.tag == GHOSTTY_TARGET_SURFACE {
                let surface_key = unsafe { target.target.surface } as usize;
                let shape = unsafe { action.action.mouse_shape };
                dispatch_mouse_cursor_update(surface_key, MouseCursorUpdate::Shape(shape));
            }
            true
        }
        GHOSTTY_ACTION_MOUSE_VISIBILITY => {
            if target.tag == GHOSTTY_TARGET_SURFACE {
                let surface_key = unsafe { target.target.surface } as usize;
                let visibility = unsafe { action.action.mouse_visibility };
                dispatch_mouse_cursor_update(
                    surface_key,
                    MouseCursorUpdate::Visibility(visibility),
                );
            }
            true
        }
        GHOSTTY_ACTION_MOUSE_OVER_LINK => {
            // Ghostty emits this when the cursor enters / leaves a hyperlink
            // region (only while the link is in its "clickable" state — i.e.
            // Ctrl held). Show the target URL in a libadwaita popover so the
            // user can see where an OSC 8 labelled link actually points
            // before they click — defence against deceptive link-masking.
            if target.tag == GHOSTTY_TARGET_SURFACE {
                let surface_key = unsafe { target.target.surface } as usize;
                let payload = unsafe { action.action.mouse_over_link };
                let url = if payload.url.is_null() || payload.len == 0 {
                    None
                } else {
                    let bytes = unsafe {
                        std::slice::from_raw_parts(payload.url.cast::<u8>(), payload.len)
                    };
                    Some(String::from_utf8_lossy(bytes).to_string())
                };
                SURFACE_MAP.with(|map| {
                    if let Some(entry) = map.borrow().get(&surface_key) {
                        match url {
                            Some(url) if widget_has_native_surface(&entry.gl_area) => {
                                entry.link_label.set_text(&url);
                                let (x, y) = entry.cursor_pos.get();
                                // GTK default: PositionType::Top centers the
                                // popover horizontally on the rectangle and
                                // anchors its bottom edge to the rectangle's
                                // top edge. The Y-gap keeps the popover clear
                                // of large accessibility cursor skins.
                                entry.link_popover.set_pointing_to(Some(
                                    &gtk::gdk::Rectangle::new(
                                        x as i32,
                                        (y as i32).saturating_sub(LINK_PREVIEW_CURSOR_Y_GAP),
                                        1,
                                        1,
                                    ),
                                ));
                                entry.link_popover.popup();
                            }
                            Some(_) | None => entry.link_popover.popdown(),
                        }
                    }
                });
            }
            true
        }
        GHOSTTY_ACTION_OPEN_URL => {
            if target.tag == GHOSTTY_TARGET_SURFACE {
                let surface_key = unsafe { target.target.surface } as usize;
                let open_url = unsafe { action.action.open_url };
                if let Some(url) = ghostty_open_url_to_string(open_url) {
                    if let Some(identity) = current_surface_identity(surface_key) {
                        glib::idle_add_local_once(move || {
                            let callback = SURFACE_MAP.with(|map| {
                                let map = map.borrow();
                                map.get(&surface_key)
                                    .filter(|entry| entry.identity == identity)
                                    .and_then(|entry| entry.on_open_url.clone())
                            });
                            if let Some(callback) = callback {
                                callback(&url, LinkOpenRequest::Configured);
                            }
                        });
                    }
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
                            cb(entry.gl_area.is_focus());
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
        GHOSTTY_ACTION_RELOAD_CONFIG => {
            let config = load_ghostty_config();
            CURRENT_SCROLLBAR_ENABLED.store(load_scrollbar_enabled(config), Ordering::Relaxed);
            match target.tag {
                GHOSTTY_TARGET_APP => unsafe {
                    ghostty_app_update_config(app, config);
                },
                GHOSTTY_TARGET_SURFACE => {
                    let surface = unsafe { target.target.surface };
                    unsafe {
                        ghostty_surface_update_config(surface, config);
                    }
                }
                _ => {}
            }
            unsafe {
                ghostty_config_free(config);
            }
            true
        }
        _ => false,
    }
}

fn ghostty_open_url_to_string(open_url: ghostty_action_open_url_s) -> Option<String> {
    if open_url.url.is_null() || open_url.len == 0 {
        return None;
    }

    let bytes = unsafe { std::slice::from_raw_parts(open_url.url.cast::<u8>(), open_url.len) };
    Some(String::from_utf8_lossy(bytes).to_string())
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

unsafe fn clipboard_context_from_userdata(
    userdata: *mut c_void,
) -> Option<&'static ClipboardContext> {
    if userdata.is_null() {
        return None;
    }
    Some(unsafe { &*(userdata as *const ClipboardContext) })
}

fn clipboard_read_text_cstring(text: Option<&str>) -> CString {
    CString::new(text.unwrap_or_default().replace('\0', ""))
        .expect("clipboard text should not contain NUL bytes")
}

fn clipboard_completion_text_ptr(text: *const c_char) -> *const c_char {
    if text.is_null() {
        EMPTY_CLIPBOARD_TEXT.as_ptr().cast()
    } else {
        text
    }
}

fn surface_is_registered(surface: ghostty_surface_t) -> bool {
    SURFACE_MAP.with(|map| map.borrow().contains_key(&(surface as usize)))
}

unsafe fn complete_clipboard_request(
    surface: ghostty_surface_t,
    text: *const c_char,
    state: *mut c_void,
    confirmed: bool,
) {
    if !surface_is_registered(surface) {
        return;
    }

    unsafe {
        ghostty_surface_complete_clipboard_request(
            surface,
            clipboard_completion_text_ptr(text),
            state,
            confirmed,
        );
    }
}

unsafe extern "C" fn ghostty_read_clipboard_cb(
    userdata: *mut c_void,
    clipboard_type: c_int,
    state: *mut c_void,
) -> bool {
    let surface_ptr = match unsafe { clipboard_surface_from_userdata(userdata) } {
        Some(surface) => surface,
        None => return false,
    };

    let Some(display) = gtk::gdk::Display::default() else {
        return false;
    };
    let clipboard = clipboard_from_type(&display, clipboard_type);
    if !clipboard_has_text(&clipboard) {
        return false;
    }

    clipboard.read_text_async(gtk::gio::Cancellable::NONE, move |result| {
        let text = result.ok().flatten().map(|s| s.to_string());
        let cstr = clipboard_read_text_cstring(text.as_deref());
        unsafe {
            complete_clipboard_request(surface_ptr, cstr.as_ptr(), state, true);
        }
    });

    true
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
        complete_clipboard_request(surface_ptr, text, state, true);
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

    if let Some(context) = unsafe { clipboard_context_from_userdata(userdata) } {
        if context.url_probe_active.get() {
            *context.url_probe.borrow_mut() = Some(text);
            return;
        }
    }

    let display = match gtk::gdk::Display::default() {
        Some(d) => d,
        None => return,
    };

    let copy_selection_to_clipboard = unsafe { clipboard_context_from_userdata(userdata) }
        .map(|context| (context.copy_selection_to_clipboard)())
        .unwrap_or(true);
    let policy = clipboard_write_policy(clipboard_type, copy_selection_to_clipboard);

    if policy.write_clipboard {
        display.clipboard().set_text(&text);
    }
    if policy.write_primary {
        display.primary_clipboard().set_text(&text);
    }
    if !policy.show_toast {
        return;
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

fn clipboard_write_policy(
    clipboard_type: c_int,
    copy_selection_to_clipboard: bool,
) -> ClipboardWritePolicy {
    if clipboard_type == GHOSTTY_CLIPBOARD_SELECTION {
        ClipboardWritePolicy {
            write_clipboard: copy_selection_to_clipboard,
            write_primary: true,
            show_toast: copy_selection_to_clipboard,
        }
    } else {
        ClipboardWritePolicy {
            write_clipboard: true,
            write_primary: true,
            show_toast: true,
        }
    }
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
    pub on_bell: Box<BellCallback>,
    pub on_close: Box<VoidCallback>,
    pub on_open_url: Box<OpenUrlCallback>,
    pub on_open_browser_here: Box<VoidCallback>,
    pub on_split_right: Box<VoidCallback>,
    pub on_split_down: Box<VoidCallback>,
    pub on_open_keybinds: Box<WidgetCallback>,
    pub identity: Box<IdentityCallback>,
}

impl TerminalCallbacks {
    fn disconnected() -> Self {
        Self {
            on_title_changed: Box::new(|_| {}),
            on_pwd_changed: Box::new(|_| {}),
            on_desktop_notification: Box::new(|_, _, _| {}),
            on_bell: Box::new(|_| {}),
            on_close: Box::new(|| {}),
            on_open_url: Box::new(|_, _| {}),
            on_open_browser_here: Box::new(|| {}),
            on_split_right: Box::new(|| {}),
            on_split_down: Box::new(|| {}),
            on_open_keybinds: Box::new(|_| {}),
            identity: Box::new(|| TerminalIdentity {
                workspace_id: None,
                surface_id: String::new(),
            }),
        }
    }
}

pub struct TerminalOptions {
    pub hover_focus: Rc<dyn Fn() -> bool>,
    pub copy_selection_to_clipboard: Rc<dyn Fn() -> bool>,
    pub saved_font_size: Option<f32>,
    pub startup_command: Option<String>,
    /// Extra environment variables to expose to the spawned shell
    /// (e.g. `LIMUX_WORKSPACE_ID`, `LIMUX_SURFACE_ID`, `LIMUX_PANE_ID`, `LIMUX_SOCKET`).
    ///
    /// These are resolved at pane-creation time so scripts running inside the
    /// terminal can discover their own workspace/surface/pane without having
    /// to call `limux identify` first. This is the foundation for the cmux
    /// agent-to-agent communication workflow.
    pub extra_env: Vec<(String, String)>,
}

impl Default for TerminalOptions {
    fn default() -> Self {
        Self {
            hover_focus: Rc::new(|| false),
            copy_selection_to_clipboard: Rc::new(|| true),
            saved_font_size: None,
            startup_command: None,
            extra_env: Vec::new(),
        }
    }
}

/// Default font-size from ghostty config (cached on first access).
pub(crate) fn default_font_size() -> f32 {
    use std::sync::OnceLock;
    static SIZE: OnceLock<f32> = OnceLock::new();
    *SIZE.get_or_init(crate::ghostty_config::read_font_size)
}

fn free_terminal_surface(
    surface: ghostty_surface_t,
    clipboard_context_cell: &Cell<*mut ClipboardContext>,
) {
    let surface_key = surface as usize;
    let entry = SURFACE_MAP.with(|map| map.borrow_mut().remove(&surface_key));
    let clipboard_context = entry
        .as_ref()
        .map(|entry| entry.clipboard_context)
        .unwrap_or_else(|| clipboard_context_cell.get());

    clipboard_context_cell.set(ptr::null_mut());
    if let Some(entry) = entry.as_ref() {
        entry.link_popover.popdown();
        unregister_surface_identity(entry.identity);
    }

    // Ghostty can invoke callbacks while its worker threads stop. Keep callback
    // userdata and widgets alive until deinitialization finishes.
    unsafe { ghostty_surface_free(surface) };
    if !clipboard_context.is_null() {
        unsafe {
            drop(Box::from_raw(clipboard_context));
        }
    }
    drop(entry);
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
    set_gtk_mouse_cursor(&gl_area, MouseCursorState::default());
    let wd = working_directory.map(|s| s.to_string());
    let saved_font_size = options.saved_font_size;
    let startup_command = options.startup_command;
    let hover_focus = options.hover_focus;
    let copy_selection_to_clipboard = options.copy_selection_to_clipboard;
    let extra_env = options.extra_env;
    let callbacks = Rc::new(RefCell::new(callbacks));
    let surface_cell: Rc<RefCell<Option<ghostty_surface_t>>> = Rc::new(RefCell::new(None));
    let shutting_down = Rc::new(Cell::new(false));
    let had_focus = Rc::new(Cell::new(false));
    let scrollbar_syncing = Rc::new(Cell::new(false));
    let clipboard_context_cell: Rc<Cell<*mut ClipboardContext>> =
        Rc::new(Cell::new(ptr::null_mut()));
    let cursor_pos: Rc<Cell<(f64, f64)>> = Rc::new(Cell::new((0.0, 0.0)));
    let signal_handlers = Rc::new(RefCell::new(Vec::new()));
    let controllers = Rc::new(RefCell::new(Vec::new()));

    // Popover used by the OSC 8 hover preview. Built via the same helpers
    // as the right-click context menu so the look matches by construction.
    let link_label = gtk::Label::new(None);
    link_label.set_selectable(false);
    let link_inner = build_popover_inner_box();
    link_inner.append(&link_label);
    let link_popover = build_floating_popover(&gl_area, &link_inner);
    link_popover.set_autohide(false);
    // Above the cursor by default, web-browser style. GTK auto-flips to
    // Bottom if there isn't enough room above the pointing rectangle.
    link_popover.set_position(gtk::PositionType::Top);
    link_popover.set_can_focus(false);

    // Create overlay early so closures can capture it for toast notifications
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&gl_area));
    overlay.set_hexpand(true);
    overlay.set_vexpand(true);

    let scrollbar_adjustment = gtk::Adjustment::new(0.0, 0.0, 0.0, 1.0, 0.0, 0.0);
    let scrollbar = gtk::Scrollbar::new(gtk::Orientation::Vertical, Some(&scrollbar_adjustment));
    scrollbar.set_visible(false);
    scrollbar.set_vexpand(true);
    // The scrollbar must be an overlay child, not a sibling in the box below.
    // Ghostty emits a SCROLLBAR action whenever `total > len` flips, and we
    // toggle visibility in response. As a Box sibling that consumes layout
    // width, showing/hiding it reallocated the GLArea, changed the terminal
    // grid columns, and reflowed the text left/right — the "window bounces"
    // flicker in issue #136. Overlaid, the terminal keeps its full width and
    // the scrollbar just draws (or stops drawing) on top of it.
    scrollbar.set_halign(gtk::Align::End);
    scrollbar.set_valign(gtk::Align::Fill);
    overlay.add_overlay(&scrollbar);

    let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    root.set_hexpand(true);
    root.set_vexpand(true);
    root.append(&overlay);

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

    let pane_ime = Rc::new(crate::ime::create_pane_ime(&gl_area, &surface_cell));
    // Aliases for the focus / lifecycle wiring further down. Cheap
    // GObject clones, not state copies.
    let im_context = pane_ime.primary.clone();
    let im_fallback = pane_ime.fallback.clone();

    let handle = TerminalHandle {
        surface_cell: surface_cell.clone(),
        clipboard_context_cell: clipboard_context_cell.clone(),
        shutting_down: shutting_down.clone(),
        gl_area: gl_area.clone(),
        search_bar: search_bar.clone(),
        search_entry: search_entry.clone(),
        callbacks: callbacks.clone(),
        im_context: im_context.clone(),
        im_fallback: im_fallback.clone(),
        signal_handlers: signal_handlers.clone(),
        controllers: controllers.clone(),
    };

    {
        let surface_cell = surface_cell.clone();
        let handler = gl_area.connect_map(move |gl_area| {
            if let Some(surface) = *surface_cell.borrow() {
                refresh_realized_surface_display(surface, gl_area);
            } else {
                gl_area.queue_render();
            }
        });
        track_signal(&signal_handlers, &gl_area, handler);
    }

    {
        let surface_cell = surface_cell.clone();
        let handler = search_entry.connect_search_changed(move |entry| {
            surface_action(
                *surface_cell.borrow(),
                &terminal_search_action(entry.text().as_str()),
            );
        });
        track_signal(&signal_handlers, &search_entry, handler);
    }
    {
        let surface_cell = surface_cell.clone();
        let search_bar = search_bar.downgrade();
        let gl_area = gl_area.downgrade();
        let handler = search_entry.connect_stop_search(move |_| {
            let Some(search_bar) = search_bar.upgrade() else {
                return;
            };
            if !search_bar.is_search_mode() {
                return;
            }
            surface_action(*surface_cell.borrow(), "end_search");
            search_bar.set_search_mode(false);
            if let Some(gl_area) = gl_area.upgrade() {
                gl_area.grab_focus();
            }
        });
        track_signal(&signal_handlers, &search_entry, handler);
    }
    {
        let surface_cell = surface_cell.clone();
        let scrollbar_syncing = scrollbar_syncing.clone();
        let handler = scrollbar_adjustment.connect_value_changed(move |adj| {
            if scrollbar_syncing.get() {
                return;
            }

            let row = adj.value().round() as usize;
            surface_action(*surface_cell.borrow(), &format!("scroll_to_row:{row}"));
        });
        track_signal(&signal_handlers, &scrollbar_adjustment, handler);
    }

    // On realize: create the Ghostty surface
    {
        let gl = gl_area.clone();
        let overlay_for_map = overlay.clone();
        let scrollbar_for_map = scrollbar.clone();
        let scrollbar_adjustment_for_map = scrollbar_adjustment.clone();
        let link_popover_for_map = link_popover.clone();
        let link_label_for_map = link_label.clone();
        let cursor_pos_for_map = cursor_pos.clone();
        let surface_cell = surface_cell.clone();
        let callbacks = callbacks.clone();
        let had_focus = had_focus.clone();
        let clipboard_context_cell = clipboard_context_cell.clone();
        let scrollbar_syncing = scrollbar_syncing.clone();
        let extra_env = extra_env.clone();
        let shutting_down = shutting_down.clone();
        let handler = gl_area.connect_realize(move |gl_area| {
            if shutting_down.get() {
                return;
            }
            gl_area.make_current();
            if let Some(err) = gl_area.error() {
                eprintln!("limux: GLArea error after make_current: {err}");
                return;
            }

            // If the surface already exists (reparenting from a split),
            // reinitialize the GL renderer with the new GL context while
            // preserving the terminal/pty state.
            if let Some(surface) = *surface_cell.borrow() {
                refresh_realized_surface_display(surface, gl_area);
                let gl_area = gl_area.clone();
                glib::idle_add_local_once(move || {
                    gl_area.queue_render();
                });
                return;
            }

            let app = ghostty_app();
            let mut config = unsafe { ghostty_surface_config_new() };
            let clipboard_context = Box::into_raw(Box::new(ClipboardContext {
                surface: Cell::new(ptr::null_mut()),
                copy_selection_to_clipboard: copy_selection_to_clipboard.clone(),
                url_probe: RefCell::new(None),
                url_probe_active: Cell::new(false),
            }));
            config.platform_tag = GHOSTTY_PLATFORM_LINUX;
            config.platform = ghostty_platform_u {
                linux_platform: ghostty_platform_linux_s {
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

            // Build env_vars array for the spawned shell. Keep the CStrings
            // and the ghostty_env_var_s array alive until after
            // ghostty_surface_new returns — Ghostty dupes the strings into
            // its own arena (see ghostty/src/apprt/embedded.zig:573), so we
            // only need the pointers valid across that single call.
            let mut env_cstrings: Vec<(CString, CString)> = Vec::with_capacity(extra_env.len());
            for (k, v) in extra_env.iter() {
                if let (Ok(k_c), Ok(v_c)) = (CString::new(k.as_str()), CString::new(v.as_str())) {
                    env_cstrings.push((k_c, v_c));
                }
            }
            let mut env_vars_raw: Vec<ghostty_env_var_s> = env_cstrings
                .iter()
                .map(|(k, v)| ghostty_env_var_s {
                    key: k.as_ptr(),
                    value: v.as_ptr(),
                })
                .collect();
            if !env_vars_raw.is_empty() {
                config.env_vars = env_vars_raw.as_mut_ptr();
                config.env_var_count = env_vars_raw.len();
            }

            let c_startup_command = startup_command
                .as_ref()
                .and_then(|command| CString::new(command.as_str()).ok());
            if let Some(ref command) = c_startup_command {
                config.command = command.as_ptr();
                eprintln!(
                    "limux: starting restored terminal command={}",
                    command.to_string_lossy()
                );
            }

            let surface = unsafe { ghostty_surface_new(app, &config) };
            if surface.is_null() {
                unsafe {
                    drop(Box::from_raw(clipboard_context));
                }
                eprintln!("limux: failed to create ghostty surface");
                return;
            }
            let surface_key = surface as usize;
            let surface_identity = register_surface_identity(surface_key);
            unsafe {
                (*clipboard_context).surface.set(surface);
                ghostty_surface_set_color_scheme(surface, current_ghostty_color_scheme());
            }
            clipboard_context_cell.set(clipboard_context);

            // Apply saved font size (if different from ghostty default)
            if let Some(size) = saved_font_size {
                let action = format!("set_font_size:{size}");
                unsafe {
                    ghostty_surface_binding_action(
                        surface,
                        action.as_ptr() as *const c_char,
                        action.len(),
                    );
                }
            }

            // Set initial size in physical pixels. GTK allocation is logical
            // CSS pixels; Ghostty's GL renderer expects the backing FBO size.
            let alloc = gl_area.allocation();
            if physical_size_for_allocation(alloc.width(), alloc.height(), gl_area.scale_factor())
                .is_some()
            {
                refresh_surface_display(surface, gl_area);
            }

            SURFACE_MAP.with(|map| {
                map.borrow_mut().insert(
                    surface_key,
                    SurfaceEntry {
                        identity: surface_identity,
                        gl_area: gl.clone(),
                        toast_overlay: overlay_for_map.clone(),
                        scrollbar: scrollbar_for_map.clone(),
                        scrollbar_adjustment: scrollbar_adjustment_for_map.clone(),
                        scrollbar_syncing: scrollbar_syncing.clone(),
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
                            move |title, body, source_focused| {
                                let callbacks = cb.borrow();
                                (callbacks.on_desktop_notification)(title, body, source_focused);
                            }
                        })),
                        on_bell: Some(Box::new({
                            let cb = callbacks.clone();
                            move |source_focused| {
                                let callbacks = cb.borrow();
                                (callbacks.on_bell)(source_focused);
                            }
                        })),
                        on_open_url: Some(Rc::new({
                            let cb = callbacks.clone();
                            move |url, destination| {
                                let callbacks = cb.borrow();
                                (callbacks.on_open_url)(url, destination);
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
                        link_popover: link_popover_for_map.clone(),
                        link_label: link_label_for_map.clone(),
                        cursor_pos: cursor_pos_for_map.clone(),
                        mouse_cursor: Cell::new(MouseCursorState::default()),
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
        track_signal(&signal_handlers, &gl_area, handler);
    }

    // On render: draw the surface.
    {
        let surface_cell = surface_cell.clone();
        let handler = gl_area.connect_render(move |_gl_area, _context| {
            if let Some(surface) = *surface_cell.borrow() {
                unsafe { ghostty_surface_draw(surface) };
            }
            glib::Propagation::Stop
        });
        track_signal(&signal_handlers, &gl_area, handler);
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
        let handler = gl_area.connect_resize(move |gl_area, width, height| {
            if let Some(surface) = *surface_cell.borrow() {
                let w = width as u32;
                let h = height as u32;
                if w > 0 && h > 0 {
                    refresh_surface_display(surface, gl_area);
                }
            }

            if had_focus.get() {
                let gl_for_focus = gl_for_resize.clone();
                glib::idle_add_local_once(move || {
                    gl_for_focus.grab_focus();
                });
            }
        });
        track_signal(&signal_handlers, &gl_area, handler);
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
        let pane_ime_press = pane_ime.clone();
        let pane_ime_release = pane_ime.clone();
        let key_controller = gtk::EventControllerKey::new();
        key_controller.connect_key_pressed(move |ctrl, keyval, keycode, modifier| {
            if let Some(surface) = *sc_press.borrow() {
                let current_event = ctrl
                    .current_event()
                    .and_then(|event| event.downcast::<gtk::gdk::KeyEvent>().ok());
                let widget = ctrl.widget();
                let fallback_text = key_event_text(keyval);

                if let Some(current_event) = current_event.as_ref() {
                    pane_ime_press.state.borrow_mut().begin_key_event();

                    let filter_outcome =
                        crate::ime::filter_key_event(surface, &pane_ime_press, current_event);
                    if filter_outcome == crate::ime::ImeFilterOutcome::ConsumeForIme {
                        pane_ime_press.state.borrow_mut().finish_key_event();
                        return glib::Propagation::Stop;
                    }
                }

                let mut event = translate_key_event(
                    GHOSTTY_ACTION_PRESS,
                    widget.as_ref(),
                    current_event.as_ref(),
                    keyval,
                    keycode,
                    modifier,
                );
                let c_text = pane_ime_press
                    .state
                    .borrow_mut()
                    .take_event_text(fallback_text);
                if let Some(ref ct) = c_text {
                    event.text = ct.as_ptr();
                }

                let consumed = unsafe { ghostty_surface_key(surface, event) };
                if consumed && pane_ime_press.state.borrow().composing {
                    crate::ime::reset_after_consumed_compose(surface, &pane_ime_press);
                }
                pane_ime_press.state.borrow_mut().finish_key_event();
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

                if let Some(current_event) = current_event.as_ref() {
                    pane_ime_release.state.borrow_mut().begin_key_event();

                    let filter_outcome =
                        crate::ime::filter_key_event(surface, &pane_ime_release, current_event);
                    if filter_outcome == crate::ime::ImeFilterOutcome::ConsumeForIme {
                        pane_ime_release.state.borrow_mut().finish_key_event();
                        return;
                    }
                }

                let event = translate_key_event(
                    GHOSTTY_ACTION_RELEASE,
                    widget.as_ref(),
                    current_event.as_ref(),
                    keyval,
                    keycode,
                    modifier,
                );
                unsafe { ghostty_surface_key(surface, event) };
                pane_ime_release.state.borrow_mut().finish_key_event();
            }
        });

        controllers
            .borrow_mut()
            .push(key_controller.clone().upcast());
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
        controllers.borrow_mut().push(click.clone().upcast());
        gl_area.add_controller(click);
    }

    // Right-click context menu
    {
        let sc = surface_cell.clone();
        let callbacks = callbacks.clone();
        let gl = gl_area.clone();
        let overlay = overlay.clone();
        let right_click = gtk::GestureClick::new();
        right_click.set_button(3);
        right_click.connect_pressed(move |gesture, _n, x, y| {
            let surface = *sc.borrow();
            let mods = translate_mouse_mods(gesture.current_event_state());
            show_terminal_context_menu(&gl, &overlay, surface, &callbacks, x, y, mods);
            gesture.set_state(gtk::EventSequenceState::Claimed);
        });
        controllers.borrow_mut().push(right_click.clone().upcast());
        gl_area.add_controller(right_click);
    }

    // Mouse motion
    {
        let surface_cell = surface_cell.clone();
        let surface_cell_for_enter = surface_cell.clone();
        let gl_for_focus = gl_area.clone();
        let had_focus = had_focus.clone();
        let cursor_pos_enter = cursor_pos.clone();
        let cursor_pos_motion = cursor_pos.clone();
        let link_popover_motion = link_popover.clone();
        let motion = gtk::EventControllerMotion::new();
        motion.connect_enter(move |ctrl, x, y| {
            if (hover_focus)() {
                // Match common Hyprland/Omarchy-style focus-follows-mouse behavior:
                // as soon as the pointer enters a terminal, focus it so typing works
                // immediately without an extra click.
                request_terminal_focus(&gl_for_focus, &had_focus);
            }

            cursor_pos_enter.set((x, y));
            if let Some(surface) = *surface_cell_for_enter.borrow() {
                let mods = translate_mouse_mods(ctrl.current_event_state());
                unsafe { ghostty_surface_mouse_pos(surface, x, y, mods) };
            }
        });
        let surface_cell = surface_cell.clone();
        motion.connect_motion(move |ctrl, x, y| {
            cursor_pos_motion.set((x, y));
            // Keep the URL preview tracking the cursor while it stays inside
            // a clickable link region (Ghostty only emits MOUSE_OVER_LINK
            // on enter/leave, not on every motion).
            if link_popover_motion.is_visible() {
                link_popover_motion.set_pointing_to(Some(&gtk::gdk::Rectangle::new(
                    x as i32,
                    (y as i32).saturating_sub(LINK_PREVIEW_CURSOR_Y_GAP),
                    1,
                    1,
                )));
            }
            if let Some(surface) = *surface_cell.borrow() {
                let mods = translate_mouse_mods(ctrl.current_event_state());
                unsafe { ghostty_surface_mouse_pos(surface, x, y, mods) };
            }
        });
        controllers.borrow_mut().push(motion.clone().upcast());
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
        controllers.borrow_mut().push(scroll.clone().upcast());
        gl_area.add_controller(scroll);
    }

    // Focus
    {
        let surface_cell = surface_cell.clone();
        let had_focus_enter = had_focus.clone();
        let had_focus_leave = had_focus.clone();
        let im_context_enter = im_context.clone();
        let im_context_leave = im_context.clone();
        let im_fallback_enter = im_fallback.clone();
        let im_fallback_leave = im_fallback.clone();
        let focus_ctrl = gtk::EventControllerFocus::new();
        let sc = surface_cell.clone();
        focus_ctrl.connect_enter(move |_| {
            had_focus_enter.set(true);
            im_context_enter.focus_in();
            im_fallback_enter.focus_in();
            if let Some(surface) = *sc.borrow() {
                unsafe { ghostty_surface_set_focus(surface, true) };
            }
        });
        focus_ctrl.connect_leave(move |_| {
            had_focus_leave.set(false);
            im_context_leave.focus_out();
            im_fallback_leave.focus_out();
            if let Some(surface) = *surface_cell.borrow() {
                unsafe { ghostty_surface_set_focus(surface, false) };
            }
        });
        controllers.borrow_mut().push(focus_ctrl.clone().upcast());
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
        controllers.borrow_mut().push(drop_target.clone().upcast());
        gl_area.add_controller(drop_target);
    }

    // On unrealize: deinit GL resources but keep the surface alive.
    // GTK unrealizes widgets during reparenting (splits), and we need
    // the terminal/pty to survive. The GL resources will be recreated
    // in connect_realize when the widget is re-realized.
    {
        let surface_cell = surface_cell.clone();
        let link_popover = link_popover.clone();
        let handler = gl_area.connect_unrealize(move |gl_area| {
            link_popover.popdown();
            if let Some(surface) = *surface_cell.borrow() {
                gl_area.make_current();
                unsafe { ghostty_surface_display_unrealized(surface) };
            }
        });
        track_signal(&signal_handlers, &gl_area, handler);
    }

    // Explicit tab and pane teardown normally shuts down the surface first.
    // Keep widget destruction as a fallback for any future removal path.
    {
        let handle = handle.clone();
        root.connect_destroy(move |_| {
            handle.shutdown();
        });
    }

    TerminalWidget {
        root: root.upcast(),
        handle,
    }
}

// ---------------------------------------------------------------------------
// Context menu
// ---------------------------------------------------------------------------

/// Send a binding action to every live surface.
pub(crate) fn broadcast_binding_action(action: &str) {
    SURFACE_MAP.with(|map| {
        for &key in map.borrow().keys() {
            let surface = key as ghostty_surface_t;
            unsafe {
                ghostty_surface_binding_action(
                    surface,
                    action.as_ptr() as *const c_char,
                    action.len(),
                );
            }
        }
    });
}

fn surface_action(surface: Option<ghostty_surface_t>, action: &str) {
    if let Some(surface) = surface {
        unsafe {
            ghostty_surface_binding_action(surface, action.as_ptr() as *const c_char, action.len());
        }
    }
}

fn url_at_position(
    surface: Option<ghostty_surface_t>,
    x: f64,
    y: f64,
    restore_mods: c_int,
) -> Option<String> {
    let surface = surface?;
    // Mouse-position callbacks become terminal input while a TUI has mouse
    // reporting enabled, so probing there would alter terminal state.
    if unsafe { ghostty_surface_mouse_captured(surface) } {
        return None;
    }
    let clipboard_context = SURFACE_MAP.with(|map| {
        map.borrow()
            .get(&(surface as usize))
            .map(|entry| entry.clipboard_context)
    })?;
    let context = unsafe { clipboard_context.as_ref() }?;

    context.url_probe_active.set(true);

    // Let Ghostty resolve OSC 8 targets and wrapped URLs. Leave and re-enter
    // because embedded surfaces deduplicate same-position moves.
    *context.url_probe.borrow_mut() = None;
    unsafe {
        ghostty_surface_mouse_pos(surface, -1.0, -1.0, GHOSTTY_MODS_CTRL);
        ghostty_surface_mouse_pos(surface, x, y, GHOSTTY_MODS_CTRL);
    }
    surface_action(Some(surface), "copy_url_to_clipboard");
    let url = context.url_probe.borrow_mut().take();
    context.url_probe_active.set(false);

    unsafe {
        ghostty_surface_mouse_pos(surface, -1.0, -1.0, restore_mods);
        ghostty_surface_mouse_pos(surface, x, y, restore_mods);
    }
    SURFACE_MAP.with(|map| {
        if let Some(entry) = map.borrow().get(&(surface as usize)) {
            entry.link_popover.popdown();
        }
    });

    url
}

fn copy_text_to_clipboards(text: &str) {
    if let Some(display) = gtk::gdk::Display::default() {
        display.clipboard().set_text(text);
        display.primary_clipboard().set_text(text);
    }
}

/// Build a libadwaita-styled floating popover (no arrow), parented to
/// `parent`. Shared by the right-click context menu and the OSC 8 hover
/// URL preview so they look identical by construction. The caller is
/// responsible for `set_pointing_to` and `popup`/`popdown`.
fn build_floating_popover(
    parent: &impl IsA<gtk::Widget>,
    child: &impl IsA<gtk::Widget>,
) -> gtk::Popover {
    let popover = gtk::Popover::new();
    popover.set_child(Some(child));
    popover.set_has_arrow(false);
    popover.set_parent(parent);
    popover
}

fn widget_has_native_surface(widget: &impl IsA<gtk::Widget>) -> bool {
    widget.is_mapped()
        && widget
            .native()
            .and_then(|native| native.surface())
            .is_some()
}

/// 4 px box wrapper that matches the inner margin used by the right-click
/// context menu items. Reused for the hover preview so both popovers have
/// the same visual breathing room around their content.
fn build_popover_inner_box() -> gtk::Box {
    let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    menu_box.set_margin_top(4);
    menu_box.set_margin_bottom(4);
    menu_box.set_margin_start(4);
    menu_box.set_margin_end(4);
    menu_box
}

fn build_submenu_button(label: &str, popover: &gtk::Popover) -> gtk::MenuButton {
    let button = gtk::MenuButton::new();
    button.set_label(label);
    button.set_direction(gtk::ArrowType::Right);
    button.set_has_frame(false);
    button.set_halign(gtk::Align::Fill);
    button.set_popover(Some(popover));
    button.add_css_class("flat");

    let weak_button = button.downgrade();
    let motion = gtk::EventControllerMotion::new();
    motion.connect_enter(move |motion, _, _| {
        if motion
            .widget()
            .is_some_and(|widget| widget_has_native_surface(&widget))
        {
            if let Some(button) = weak_button.upgrade() {
                button.popup();
            }
        }
    });
    button.add_controller(motion);
    button
}

fn show_terminal_context_menu(
    gl_area: &gtk::GLArea,
    overlay: &gtk::Overlay,
    surface: Option<ghostty_surface_t>,
    callbacks: &Rc<RefCell<TerminalCallbacks>>,
    x: f64,
    y: f64,
    mods: c_int,
) {
    if !widget_has_native_surface(gl_area) {
        return;
    }

    let menu_box = build_popover_inner_box();
    let url = url_at_position(surface, x, y, mods);

    let has_selection = surface
        .map(|s| unsafe { ghostty_surface_has_selection(s) })
        .unwrap_or(false);

    let mut items: Vec<(&str, bool)> = vec![("Copy", has_selection)];
    if url.is_some() {
        items.push(("Copy URL", true));
    }
    if url.as_deref().is_some_and(link_uri::is_safe_external_url) {
        items.push(("Open in", true));
    }
    items.extend([
        ("Paste", true),
        ("---", false),
        ("IDs", true),
        ("---", false),
        ("Browser", true),
        ("Split Right", true),
        ("Split Down", true),
        ("Keybinds", true),
        ("---", false),
        ("Clear", true),
    ]);

    let identity = (callbacks.borrow().identity)();
    let ids_popover = gtk::Popover::new();
    ids_popover.set_has_arrow(false);
    ids_popover.set_position(gtk::PositionType::Right);
    let ids_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    ids_box.set_margin_top(4);
    ids_box.set_margin_bottom(4);
    ids_box.set_margin_start(4);
    ids_box.set_margin_end(4);
    let copy_workspace_btn = gtk::Button::with_label("Copy Workspace ID");
    copy_workspace_btn.add_css_class("flat");
    copy_workspace_btn.set_sensitive(identity.workspace_id.is_some());
    let copy_surface_btn = gtk::Button::with_label("Copy Surface ID");
    copy_surface_btn.add_css_class("flat");
    for btn in [&copy_workspace_btn, &copy_surface_btn] {
        btn.set_halign(gtk::Align::Fill);
        if let Some(lbl) = btn.child().and_then(|c| c.downcast::<gtk::Label>().ok()) {
            lbl.set_xalign(0.0);
        }
        ids_box.append(btn);
    }
    ids_popover.set_child(Some(&ids_box));
    let ids_menu_button = build_submenu_button("IDs", &ids_popover);

    let open_in_popover = gtk::Popover::new();
    open_in_popover.set_has_arrow(false);
    open_in_popover.set_position(gtk::PositionType::Right);
    let open_in_box = build_popover_inner_box();
    let open_default_browser_btn = gtk::Button::with_label("Default browser");
    let open_browser_tab_btn = gtk::Button::with_label("Browser tab");
    open_browser_tab_btn.set_sensitive(
        cfg!(feature = "webkit")
            && url
                .as_deref()
                .is_some_and(link_uri::is_embedded_browser_url),
    );
    for btn in [&open_default_browser_btn, &open_browser_tab_btn] {
        btn.add_css_class("flat");
        btn.set_halign(gtk::Align::Fill);
        if let Some(label) = btn
            .child()
            .and_then(|child| child.downcast::<gtk::Label>().ok())
        {
            label.set_xalign(0.0);
        }
        open_in_box.append(btn);
    }
    open_in_popover.set_child(Some(&open_in_box));
    let open_in_menu_button = build_submenu_button("Open in…", &open_in_popover);

    for (label, enabled) in &items {
        if *label == "---" {
            let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
            sep.set_margin_top(4);
            sep.set_margin_bottom(4);
            menu_box.append(&sep);
            continue;
        }

        if *label == "IDs" {
            menu_box.append(&ids_menu_button);
            continue;
        }
        if *label == "Open in" {
            menu_box.append(&open_in_menu_button);
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

    let popover = build_floating_popover(gl_area, &menu_box);
    popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));

    // Wire up each button
    let mut child = menu_box.first_child();
    while let Some(widget) = child {
        if let Some(btn) = widget.downcast_ref::<gtk::Button>() {
            let label = btn.label().unwrap_or_default().to_string();
            let pop = popover.downgrade();
            let cb = callbacks.clone();
            let gl_area = gl_area.clone();
            let url = url.clone();
            let overlay = overlay.clone();

            btn.connect_clicked(move |_| {
                let Some(pop) = pop.upgrade() else {
                    return;
                };
                pop.popdown();
                match label.as_str() {
                    "Copy" => surface_action(surface, "copy_to_clipboard"),
                    "Copy URL" => {
                        if let Some(url) = url.as_deref() {
                            copy_text_to_clipboards(url);
                            show_clipboard_toast(&overlay);
                        }
                    }
                    "Paste" => surface_action(surface, "paste_from_clipboard"),
                    "Browser" => {
                        let callbacks = cb.borrow();
                        (callbacks.on_open_browser_here)();
                    }
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

    for (button, destination) in [
        (
            open_default_browser_btn,
            LinkOpenDestination::DefaultBrowser,
        ),
        (open_browser_tab_btn, LinkOpenDestination::BrowserTab),
    ] {
        let pop = popover.downgrade();
        let open_in_pop = open_in_popover.downgrade();
        let callbacks = callbacks.clone();
        let url = url.clone();
        button.connect_clicked(move |_| {
            if let Some(open_in_pop) = open_in_pop.upgrade() {
                open_in_pop.popdown();
            }
            if let Some(pop) = pop.upgrade() {
                pop.popdown();
            }
            if let Some(url) = url.as_deref() {
                (callbacks.borrow().on_open_url)(url, LinkOpenRequest::Destination(destination));
            }
        });
    }

    {
        let pop = popover.downgrade();
        let ids_pop = ids_popover.downgrade();
        let overlay = overlay.clone();
        let workspace_id = identity.workspace_id.clone();
        copy_workspace_btn.connect_clicked(move |_| {
            if let Some(workspace_id) = workspace_id.as_deref() {
                copy_text_to_clipboards(workspace_id);
                show_clipboard_toast(&overlay);
            }
            if let Some(ids_pop) = ids_pop.upgrade() {
                ids_pop.popdown();
            }
            if let Some(pop) = pop.upgrade() {
                pop.popdown();
            }
        });
    }

    {
        let pop = popover.downgrade();
        let ids_pop = ids_popover.downgrade();
        let overlay = overlay.clone();
        let surface_id = identity.surface_id.clone();
        copy_surface_btn.connect_clicked(move |_| {
            copy_text_to_clipboards(&surface_id);
            show_clipboard_toast(&overlay);
            if let Some(ids_pop) = ids_pop.upgrade() {
                ids_pop.popdown();
            }
            if let Some(pop) = pop.upgrade() {
                pop.popdown();
            }
        });
    }

    {
        let ids_menu_button = ids_menu_button.clone();
        let open_in_menu_button = open_in_menu_button.clone();
        popover.connect_closed(move |p| {
            ids_menu_button.popdown();
            ids_menu_button.set_popover(None::<&gtk::Popover>);
            open_in_menu_button.popdown();
            open_in_menu_button.set_popover(None::<&gtk::Popover>);
            p.unparent();
        });
    }

    if widget_has_native_surface(gl_area) {
        popover.popup();
    } else {
        ids_menu_button.set_popover(None::<&gtk::Popover>);
        open_in_menu_button.set_popover(None::<&gtk::Popover>);
        popover.unparent();
    }
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
    let keycode = ghostty_keycode_with_caps_escape_remap(keyval, keycode);

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

fn ghostty_keycode_with_caps_escape_remap(keyval: gtk::gdk::Key, keycode: u32) -> u32 {
    const XKB_KEYCODE_ESCAPE: u32 = 9;
    const XKB_KEYCODE_CAPS_LOCK: u32 = 66;

    // Embedded Ghostty derives its key from the XKB keycode and cannot see GTK's remapped keyval.
    if keyval == gtk::gdk::Key::Escape {
        XKB_KEYCODE_ESCAPE
    } else if keyval == gtk::gdk::Key::Caps_Lock {
        XKB_KEYCODE_CAPS_LOCK
    } else {
        keycode
    }
}

/// Map a keyval back to a hardware keycode via the display's keymap.
///
/// Real key events arrive from GTK with the keycode already filled in. Keys
/// injected through the control socket only have a keyval (parsed from a string
/// like `enter`), so we have to ask the keymap for the physical key that
/// produces it. Returns 0 when the keyval isn't on the current layout, which
/// preserves the previous behaviour rather than inventing a wrong key.
fn keycode_for_keyval(widget: &gtk::Widget, keyval: gtk::gdk::Key) -> u32 {
    widget
        .display()
        .map_keyval(keyval)
        .and_then(|keys| keys.first().map(|key| key.keycode()))
        .unwrap_or(0)
}

fn translated_keyval_for_keycode(
    widget: &gtk::Widget,
    keyval: gtk::gdk::Key,
    keycode: u32,
    modifier: gtk::gdk::ModifierType,
) -> gtk::gdk::Key {
    if keycode == 0 || !modifier.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
        return keyval;
    }

    let display = widget.display();
    let group = display
        .map_keyval(keyval)
        .and_then(|mappings| {
            mappings
                .iter()
                .find(|mapping| mapping.keycode() == keycode)
                .or_else(|| mappings.first())
                .map(|mapping| mapping.group())
        })
        .unwrap_or(0);

    display
        .translate_key(keycode, modifier, group)
        .map(|(translated, _, _, _)| translated)
        .unwrap_or(keyval)
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
    fn physical_size_matches_logical_allocation_times_scale_factor() {
        assert_eq!(
            physical_size_for_allocation(1280, 720, 1),
            Some((1280, 720, 1))
        );
        assert_eq!(
            physical_size_for_allocation(1280, 720, 2),
            Some((2560, 1440, 2))
        );
        assert_eq!(
            physical_size_for_allocation(640, 480, 3),
            Some((1920, 1440, 3))
        );
    }

    #[test]
    fn physical_size_rejects_invalid_allocation_or_scale() {
        assert_eq!(physical_size_for_allocation(0, 720, 2), None);
        assert_eq!(physical_size_for_allocation(1280, 0, 2), None);
        assert_eq!(physical_size_for_allocation(1280, 720, 0), None);
        assert_eq!(physical_size_for_allocation(-1, 720, 2), None);
        assert_eq!(physical_size_for_allocation(1280, -1, 2), None);
        assert_eq!(physical_size_for_allocation(1280, 720, -1), None);
    }

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
    fn maps_ghostty_mouse_shapes_to_gtk_cursor_names() {
        let cases = [
            (GHOSTTY_MOUSE_SHAPE_DEFAULT, "default"),
            (GHOSTTY_MOUSE_SHAPE_CONTEXT_MENU, "context-menu"),
            (GHOSTTY_MOUSE_SHAPE_HELP, "help"),
            (GHOSTTY_MOUSE_SHAPE_POINTER, "pointer"),
            (GHOSTTY_MOUSE_SHAPE_PROGRESS, "progress"),
            (GHOSTTY_MOUSE_SHAPE_WAIT, "wait"),
            (GHOSTTY_MOUSE_SHAPE_CELL, "cell"),
            (GHOSTTY_MOUSE_SHAPE_CROSSHAIR, "crosshair"),
            (GHOSTTY_MOUSE_SHAPE_TEXT, "text"),
            (GHOSTTY_MOUSE_SHAPE_VERTICAL_TEXT, "vertical-text"),
            (GHOSTTY_MOUSE_SHAPE_ALIAS, "alias"),
            (GHOSTTY_MOUSE_SHAPE_COPY, "copy"),
            (GHOSTTY_MOUSE_SHAPE_MOVE, "move"),
            (GHOSTTY_MOUSE_SHAPE_NO_DROP, "no-drop"),
            (GHOSTTY_MOUSE_SHAPE_NOT_ALLOWED, "not-allowed"),
            (GHOSTTY_MOUSE_SHAPE_GRAB, "grab"),
            (GHOSTTY_MOUSE_SHAPE_GRABBING, "grabbing"),
            (GHOSTTY_MOUSE_SHAPE_ALL_SCROLL, "all-scroll"),
            (GHOSTTY_MOUSE_SHAPE_COL_RESIZE, "col-resize"),
            (GHOSTTY_MOUSE_SHAPE_ROW_RESIZE, "row-resize"),
            (GHOSTTY_MOUSE_SHAPE_N_RESIZE, "n-resize"),
            (GHOSTTY_MOUSE_SHAPE_E_RESIZE, "e-resize"),
            (GHOSTTY_MOUSE_SHAPE_S_RESIZE, "s-resize"),
            (GHOSTTY_MOUSE_SHAPE_W_RESIZE, "w-resize"),
            (GHOSTTY_MOUSE_SHAPE_NE_RESIZE, "ne-resize"),
            (GHOSTTY_MOUSE_SHAPE_NW_RESIZE, "nw-resize"),
            (GHOSTTY_MOUSE_SHAPE_SE_RESIZE, "se-resize"),
            (GHOSTTY_MOUSE_SHAPE_SW_RESIZE, "sw-resize"),
            (GHOSTTY_MOUSE_SHAPE_EW_RESIZE, "ew-resize"),
            (GHOSTTY_MOUSE_SHAPE_NS_RESIZE, "ns-resize"),
            (GHOSTTY_MOUSE_SHAPE_NESW_RESIZE, "nesw-resize"),
            (GHOSTTY_MOUSE_SHAPE_NWSE_RESIZE, "nwse-resize"),
            (GHOSTTY_MOUSE_SHAPE_ZOOM_IN, "zoom-in"),
            (GHOSTTY_MOUSE_SHAPE_ZOOM_OUT, "zoom-out"),
        ];

        for (shape, expected) in cases {
            assert_eq!(gtk_cursor_name_for_ghostty_shape(shape), expected);
        }
        assert_eq!(gtk_cursor_name_for_ghostty_shape(c_int::MAX), "default");
    }

    #[test]
    fn mouse_visibility_restores_latest_shape_and_treats_unknown_as_visible() {
        let state = MouseCursorState::default()
            .update_shape(GHOSTTY_MOUSE_SHAPE_POINTER)
            .update_visibility(GHOSTTY_MOUSE_HIDDEN);
        assert_eq!(state.gtk_cursor_name(), "none");

        let state = state
            .update_shape(GHOSTTY_MOUSE_SHAPE_WAIT)
            .update_visibility(GHOSTTY_MOUSE_VISIBLE);
        assert_eq!(state.gtk_cursor_name(), "wait");

        let state = state.update_visibility(c_int::MAX);
        assert!(state.visible);
        assert_eq!(state.gtk_cursor_name(), "wait");
    }

    #[test]
    fn surface_generation_rejects_queued_cursor_update_after_address_reuse() {
        let mut registry = SurfaceIdentityRegistry::default();
        let surface_key = 0x1234;
        let original = registry.register(surface_key);
        let queued_update = registry.current(surface_key).unwrap();

        registry.unregister(original);
        let replacement = registry.register(surface_key);

        assert_eq!(queued_update.surface_key, replacement.surface_key);
        assert_ne!(queued_update.generation, replacement.generation);
        assert!(!registry.is_current(queued_update));
        assert!(registry.is_current(replacement));
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
    fn key_translation_honors_caps_lock_escape_remaps() {
        let modifiers = gtk::gdk::ModifierType::empty();
        let caps_as_escape = translate_key_event(
            GHOSTTY_ACTION_PRESS,
            None,
            None,
            gtk::gdk::Key::Escape,
            66,
            modifiers,
        );
        let escape_as_caps = translate_key_event(
            GHOSTTY_ACTION_PRESS,
            None,
            None,
            gtk::gdk::Key::Caps_Lock,
            9,
            modifiers,
        );
        let writing_key = translate_key_event(
            GHOSTTY_ACTION_PRESS,
            None,
            None,
            gtk::gdk::Key::a,
            38,
            modifiers,
        );

        assert_eq!(caps_as_escape.keycode, 9);
        assert_eq!(escape_as_caps.keycode, 66);
        assert_eq!(writing_key.keycode, 38);
    }

    #[test]
    fn terminal_search_action_formats_queries_for_ghostty() {
        assert_eq!(terminal_search_action(""), "search:");
        assert_eq!(terminal_search_action("needle"), "search:needle");
        assert_eq!(terminal_search_action("two words"), "search:two words");
    }

    #[test]
    fn open_url_action_uses_explicit_byte_len() {
        let url = b"https://example.com/path?x=1";
        let action = ghostty_action_open_url_s {
            kind: 0,
            url: url.as_ptr().cast(),
            len: url.len(),
        };

        assert_eq!(
            ghostty_open_url_to_string(action).as_deref(),
            Some("https://example.com/path?x=1")
        );
    }

    #[test]
    fn open_url_action_rejects_empty_payload() {
        let action = ghostty_action_open_url_s {
            kind: 0,
            url: ptr::null(),
            len: 0,
        };

        assert_eq!(ghostty_open_url_to_string(action), None);
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

    /// The contract `send-key` depends on, in one place.
    ///
    /// A socket-injected key needs BOTH the hardware keycode and, for printable
    /// input, the text field. Supplying only one drops the key silently:
    ///
    ///   * keycode only -> `send-key a` returns OK and writes nothing.
    ///   * text only    -> Enter/arrows/ctrl-chords cannot be encoded at all.
    ///
    /// `key_event_text` is what decides which keys carry text, so pin its behaviour
    /// for each of the three classes.
    #[test]
    fn send_key_text_is_supplied_for_printables_and_withheld_from_control_keys() {
        // Printable: ghostty writes these from the text field.
        for (key, expected) in [
            (gtk::gdk::Key::a, "a"),
            (gtk::gdk::Key::A, "A"),
            (gtk::gdk::Key::_1, "1"),
            (gtk::gdk::Key::space, " "),
        ] {
            let text = key_event_text(key).and_then(|s| s.into_string().ok());
            assert_eq!(
                text.as_deref(),
                Some(expected),
                "printable key must carry text or it is dropped"
            );
        }

        // Control keys: text must be WITHHELD so ghostty encodes them from the
        // key + mods. Writing "\r" as literal text instead of letting ghostty encode
        // Return is how you break the kitty keyboard protocol.
        for key in [
            gtk::gdk::Key::Return,
            gtk::gdk::Key::Tab,
            gtk::gdk::Key::Escape,
            gtk::gdk::Key::BackSpace,
        ] {
            assert!(
                key_event_text(key).is_none(),
                "{key:?} must be encoded by ghostty, not written as literal text"
            );
        }

        // Non-textual keys have no unicode at all.
        assert!(key_event_text(gtk::gdk::Key::Up).is_none());
        assert!(key_event_text(gtk::gdk::Key::F1).is_none());
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
    fn clipboard_read_text_defaults_to_empty_when_missing() {
        let text = clipboard_read_text_cstring(None);

        assert_eq!(text.to_bytes_with_nul(), b"\0");
    }

    #[test]
    fn clipboard_read_text_strips_nul_bytes() {
        let text = clipboard_read_text_cstring(Some("a\0b\0c"));

        assert_eq!(text.to_bytes(), b"abc");
    }

    #[test]
    fn clipboard_completion_text_replaces_null_with_empty_cstr() {
        let text = clipboard_completion_text_ptr(ptr::null());
        let text = unsafe { std::ffi::CStr::from_ptr(text) };

        assert_eq!(text.to_bytes(), b"");
    }

    #[test]
    fn clipboard_completion_text_keeps_non_null_ptr() {
        let text = CString::new("clipboard").unwrap();

        assert_eq!(clipboard_completion_text_ptr(text.as_ptr()), text.as_ptr());
    }

    #[test]
    fn clipboard_write_policy_can_disable_selection_to_regular_clipboard() {
        assert_eq!(
            clipboard_write_policy(GHOSTTY_CLIPBOARD_SELECTION, false),
            ClipboardWritePolicy {
                write_clipboard: false,
                write_primary: true,
                show_toast: false,
            }
        );
        assert_eq!(
            clipboard_write_policy(GHOSTTY_CLIPBOARD_SELECTION, true),
            ClipboardWritePolicy {
                write_clipboard: true,
                write_primary: true,
                show_toast: true,
            }
        );
        assert_eq!(
            clipboard_write_policy(GHOSTTY_CLIPBOARD_STANDARD, false),
            ClipboardWritePolicy {
                write_clipboard: true,
                write_primary: true,
                show_toast: true,
            }
        );
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

    #[test]
    fn wakeup_idle_slot_coalesces_until_released() {
        let flag = AtomicBool::new(false);

        assert!(claim_wakeup_idle_slot(&flag));
        assert!(!claim_wakeup_idle_slot(&flag));

        release_wakeup_idle_slot(&flag);

        assert!(claim_wakeup_idle_slot(&flag));
    }
}
