//! Raw FFI bindings to libghostty's C embedding API.
//!
//! These bindings mirror the types and functions declared in ghostty.h.
//! Only the subset needed by limux is included.

#![allow(non_camel_case_types, non_upper_case_globals)]

use std::os::raw::{c_char, c_int, c_void};

// -------------------------------------------------------------------
// Opaque handles
// -------------------------------------------------------------------

pub type ghostty_app_t = *mut c_void;
pub type ghostty_config_t = *mut c_void;
pub type ghostty_surface_t = *mut c_void;

// -------------------------------------------------------------------
// Enums
// -------------------------------------------------------------------

pub const GHOSTTY_PLATFORM_INVALID: c_int = 0;
pub const GHOSTTY_PLATFORM_MACOS: c_int = 1;
pub const GHOSTTY_PLATFORM_IOS: c_int = 2;
pub const GHOSTTY_PLATFORM_LINUX: c_int = 3;

pub const GHOSTTY_CLIPBOARD_STANDARD: c_int = 0;
pub const GHOSTTY_CLIPBOARD_SELECTION: c_int = 1;

pub const GHOSTTY_CLIPBOARD_REQUEST_PASTE: c_int = 0;
pub const GHOSTTY_CLIPBOARD_REQUEST_OSC_52_READ: c_int = 1;
pub const GHOSTTY_CLIPBOARD_REQUEST_OSC_52_WRITE: c_int = 2;

pub const GHOSTTY_MOUSE_RELEASE: c_int = 0;
pub const GHOSTTY_MOUSE_PRESS: c_int = 1;

pub const GHOSTTY_MOUSE_UNKNOWN: c_int = 0;
pub const GHOSTTY_MOUSE_LEFT: c_int = 1;
pub const GHOSTTY_MOUSE_RIGHT: c_int = 2;
pub const GHOSTTY_MOUSE_MIDDLE: c_int = 3;

pub const GHOSTTY_ACTION_RELEASE: c_int = 0;
pub const GHOSTTY_ACTION_PRESS: c_int = 1;
pub const GHOSTTY_ACTION_REPEAT: c_int = 2;

pub const GHOSTTY_MODS_NONE: c_int = 0;
pub const GHOSTTY_MODS_SHIFT: c_int = 1 << 0;
pub const GHOSTTY_MODS_CTRL: c_int = 1 << 1;
pub const GHOSTTY_MODS_ALT: c_int = 1 << 2;
pub const GHOSTTY_MODS_SUPER: c_int = 1 << 3;

pub const GHOSTTY_SURFACE_CONTEXT_WINDOW: c_int = 0;
pub const GHOSTTY_SURFACE_CONTEXT_TAB: c_int = 1;
pub const GHOSTTY_SURFACE_CONTEXT_SPLIT: c_int = 2;

pub const GHOSTTY_COLOR_SCHEME_LIGHT: c_int = 0;
pub const GHOSTTY_COLOR_SCHEME_DARK: c_int = 1;

// Action tags — values must match ghostty_action_tag_e in ghostty.h
pub const GHOSTTY_ACTION_QUIT: c_int = 0;
pub const GHOSTTY_ACTION_NEW_WINDOW: c_int = 1;
pub const GHOSTTY_ACTION_NEW_TAB: c_int = 2;
pub const GHOSTTY_ACTION_CLOSE_TAB: c_int = 3;
pub const GHOSTTY_ACTION_NEW_SPLIT: c_int = 4;
pub const GHOSTTY_ACTION_RENDER: c_int = 27;
pub const GHOSTTY_ACTION_DESKTOP_NOTIFICATION: c_int = 31;
pub const GHOSTTY_ACTION_SET_TITLE: c_int = 32;
pub const GHOSTTY_ACTION_PWD: c_int = 34;
pub const GHOSTTY_ACTION_MOUSE_SHAPE: c_int = 35;
pub const GHOSTTY_ACTION_COLOR_CHANGE: c_int = 45;
pub const GHOSTTY_ACTION_CLOSE_WINDOW: c_int = 48;
pub const GHOSTTY_ACTION_RING_BELL: c_int = 49;
pub const GHOSTTY_ACTION_SHOW_CHILD_EXITED: c_int = 54;

// Key codes (W3C UIEvents, subset)
pub const GHOSTTY_KEY_UNIDENTIFIED: c_int = 0;
// Writing System Keys
pub const GHOSTTY_KEY_BACKQUOTE: c_int = 1;
pub const GHOSTTY_KEY_BACKSLASH: c_int = 2;
pub const GHOSTTY_KEY_BRACKET_LEFT: c_int = 3;
pub const GHOSTTY_KEY_BRACKET_RIGHT: c_int = 4;
pub const GHOSTTY_KEY_COMMA: c_int = 5;
pub const GHOSTTY_KEY_DIGIT_0: c_int = 6;
pub const GHOSTTY_KEY_DIGIT_1: c_int = 7;
pub const GHOSTTY_KEY_DIGIT_2: c_int = 8;
pub const GHOSTTY_KEY_DIGIT_3: c_int = 9;
pub const GHOSTTY_KEY_DIGIT_4: c_int = 10;
pub const GHOSTTY_KEY_DIGIT_5: c_int = 11;
pub const GHOSTTY_KEY_DIGIT_6: c_int = 12;
pub const GHOSTTY_KEY_DIGIT_7: c_int = 13;
pub const GHOSTTY_KEY_DIGIT_8: c_int = 14;
pub const GHOSTTY_KEY_DIGIT_9: c_int = 15;
pub const GHOSTTY_KEY_EQUAL: c_int = 16;
pub const GHOSTTY_KEY_INTL_BACKSLASH: c_int = 17;
pub const GHOSTTY_KEY_INTL_RO: c_int = 18;
pub const GHOSTTY_KEY_INTL_YEN: c_int = 19;
pub const GHOSTTY_KEY_A: c_int = 20;
pub const GHOSTTY_KEY_B: c_int = 21;
pub const GHOSTTY_KEY_C: c_int = 22;
pub const GHOSTTY_KEY_D: c_int = 23;
pub const GHOSTTY_KEY_E: c_int = 24;
pub const GHOSTTY_KEY_F: c_int = 25;
pub const GHOSTTY_KEY_G: c_int = 26;
pub const GHOSTTY_KEY_H: c_int = 27;
pub const GHOSTTY_KEY_I: c_int = 28;
pub const GHOSTTY_KEY_J: c_int = 29;
pub const GHOSTTY_KEY_K: c_int = 30;
pub const GHOSTTY_KEY_L: c_int = 31;
pub const GHOSTTY_KEY_M: c_int = 32;
pub const GHOSTTY_KEY_N: c_int = 33;
pub const GHOSTTY_KEY_O: c_int = 34;
pub const GHOSTTY_KEY_P: c_int = 35;
pub const GHOSTTY_KEY_Q: c_int = 36;
pub const GHOSTTY_KEY_R: c_int = 37;
pub const GHOSTTY_KEY_S: c_int = 38;
pub const GHOSTTY_KEY_T: c_int = 39;
pub const GHOSTTY_KEY_U: c_int = 40;
pub const GHOSTTY_KEY_V: c_int = 41;
pub const GHOSTTY_KEY_W: c_int = 42;
pub const GHOSTTY_KEY_X: c_int = 43;
pub const GHOSTTY_KEY_Y: c_int = 44;
pub const GHOSTTY_KEY_Z: c_int = 45;
pub const GHOSTTY_KEY_MINUS: c_int = 46;
pub const GHOSTTY_KEY_PERIOD: c_int = 47;
pub const GHOSTTY_KEY_QUOTE: c_int = 48;
pub const GHOSTTY_KEY_SEMICOLON: c_int = 49;
pub const GHOSTTY_KEY_SLASH: c_int = 50;
// Functional Keys
pub const GHOSTTY_KEY_ALT_LEFT: c_int = 51;
pub const GHOSTTY_KEY_ALT_RIGHT: c_int = 52;
pub const GHOSTTY_KEY_BACKSPACE: c_int = 53;
pub const GHOSTTY_KEY_CAPS_LOCK: c_int = 54;
pub const GHOSTTY_KEY_CONTEXT_MENU: c_int = 55;
pub const GHOSTTY_KEY_CONTROL_LEFT: c_int = 56;
pub const GHOSTTY_KEY_CONTROL_RIGHT: c_int = 57;
pub const GHOSTTY_KEY_ENTER: c_int = 58;
pub const GHOSTTY_KEY_META_LEFT: c_int = 59;
pub const GHOSTTY_KEY_META_RIGHT: c_int = 60;
pub const GHOSTTY_KEY_SHIFT_LEFT: c_int = 61;
pub const GHOSTTY_KEY_SHIFT_RIGHT: c_int = 62;
pub const GHOSTTY_KEY_SPACE: c_int = 63;
pub const GHOSTTY_KEY_TAB: c_int = 64;
pub const GHOSTTY_KEY_CONVERT: c_int = 65;
pub const GHOSTTY_KEY_KANA_MODE: c_int = 66;
pub const GHOSTTY_KEY_NON_CONVERT: c_int = 67;
// Control Pad
pub const GHOSTTY_KEY_DELETE: c_int = 68;
pub const GHOSTTY_KEY_END: c_int = 69;
pub const GHOSTTY_KEY_HELP: c_int = 70;
pub const GHOSTTY_KEY_HOME: c_int = 71;
pub const GHOSTTY_KEY_INSERT: c_int = 72;
pub const GHOSTTY_KEY_PAGE_DOWN: c_int = 73;
pub const GHOSTTY_KEY_PAGE_UP: c_int = 74;
// Arrow Pad
pub const GHOSTTY_KEY_ARROW_DOWN: c_int = 75;
pub const GHOSTTY_KEY_ARROW_LEFT: c_int = 76;
pub const GHOSTTY_KEY_ARROW_RIGHT: c_int = 77;
pub const GHOSTTY_KEY_ARROW_UP: c_int = 78;
// Function Keys
pub const GHOSTTY_KEY_ESCAPE: c_int = 120;
pub const GHOSTTY_KEY_F1: c_int = 121;
pub const GHOSTTY_KEY_F2: c_int = 122;
pub const GHOSTTY_KEY_F3: c_int = 123;
pub const GHOSTTY_KEY_F4: c_int = 124;
pub const GHOSTTY_KEY_F5: c_int = 125;
pub const GHOSTTY_KEY_F6: c_int = 126;
pub const GHOSTTY_KEY_F7: c_int = 127;
pub const GHOSTTY_KEY_F8: c_int = 128;
pub const GHOSTTY_KEY_F9: c_int = 129;
pub const GHOSTTY_KEY_F10: c_int = 130;
pub const GHOSTTY_KEY_F11: c_int = 131;
pub const GHOSTTY_KEY_F12: c_int = 132;

// -------------------------------------------------------------------
// Structs
// -------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ghostty_platform_linux_s {
    pub reserved: *mut c_void,
}

#[repr(C)]
pub union ghostty_platform_u {
    pub macos: ghostty_platform_macos_s,
    pub ios: ghostty_platform_ios_s,
    pub linux: ghostty_platform_linux_s,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ghostty_platform_macos_s {
    pub nsview: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ghostty_platform_ios_s {
    pub uiview: *mut c_void,
}

#[repr(C)]
pub struct ghostty_input_key_s {
    pub action: c_int, // ghostty_input_action_e
    pub mods: c_int,   // ghostty_input_mods_e
    pub consumed_mods: c_int,
    pub keycode: u32,
    pub text: *const c_char,
    pub unshifted_codepoint: u32,
    pub composing: bool,
}

pub type ghostty_io_write_cb = unsafe extern "C" fn(*mut c_void, *const c_char, usize);

#[repr(C)]
pub struct ghostty_surface_config_s {
    pub platform_tag: c_int,
    pub platform: ghostty_platform_u,
    pub userdata: *mut c_void,
    pub scale_factor: f64,
    pub font_size: f32,
    pub working_directory: *const c_char,
    pub command: *const c_char,
    pub env_vars: *mut ghostty_env_var_s,
    pub env_var_count: usize,
    pub initial_input: *const c_char,
    pub wait_after_command: bool,
    pub context: c_int, // ghostty_surface_context_e
    pub io_mode: c_int, // ghostty_surface_io_mode_e (0 = exec)
    pub io_write_cb: Option<ghostty_io_write_cb>,
    pub io_write_userdata: *mut c_void,
}

#[repr(C)]
pub struct ghostty_env_var_s {
    pub key: *const c_char,
    pub value: *const c_char,
}

#[repr(C)]
pub struct ghostty_surface_size_s {
    pub columns: u16,
    pub rows: u16,
    pub width_px: u32,
    pub height_px: u32,
    pub cell_width_px: u32,
    pub cell_height_px: u32,
}

#[repr(C)]
pub struct ghostty_clipboard_content_s {
    pub mime: *const c_char,
    pub data: *const c_char,
}

#[repr(C)]
pub struct ghostty_text_s {
    pub tl_px_x: f64,
    pub tl_px_y: f64,
    pub offset_start: u32,
    pub offset_len: u32,
    pub text: *const c_char,
    pub text_len: usize,
}

// Target
#[repr(C)]
pub struct ghostty_target_s {
    pub tag: c_int, // GHOSTTY_TARGET_APP or GHOSTTY_TARGET_SURFACE
    pub target: ghostty_target_u,
}

#[repr(C)]
pub union ghostty_target_u {
    pub surface: ghostty_surface_t,
}

pub const GHOSTTY_TARGET_APP: c_int = 0;
pub const GHOSTTY_TARGET_SURFACE: c_int = 1;

// Action
#[repr(C)]
pub struct ghostty_action_s {
    pub tag: c_int,
    pub action: ghostty_action_u,
}

// We only need a subset of the action union — for matching on tag
// we just access the right field after checking the tag.
// Must be exactly 24 bytes to match the C union.
#[repr(C)]
pub union ghostty_action_u {
    pub set_title: ghostty_action_set_title_s,
    pub pwd: ghostty_action_pwd_s,
    pub child_exited: ghostty_surface_message_childexited_s,
    _padding: [u8; 24],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ghostty_action_set_title_s {
    pub title: *const c_char,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ghostty_action_pwd_s {
    pub pwd: *const c_char,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ghostty_surface_message_childexited_s {
    pub exit_code: u32,
    pub runtime_ms: u64,
}

// Runtime config (callbacks)
pub type ghostty_runtime_wakeup_cb = unsafe extern "C" fn(*mut c_void);
pub type ghostty_runtime_action_cb =
    unsafe extern "C" fn(ghostty_app_t, ghostty_target_s, ghostty_action_s) -> bool;
pub type ghostty_runtime_read_clipboard_cb = unsafe extern "C" fn(*mut c_void, c_int, *mut c_void);
pub type ghostty_runtime_confirm_read_clipboard_cb =
    unsafe extern "C" fn(*mut c_void, *const c_char, *mut c_void, c_int);
pub type ghostty_runtime_write_clipboard_cb =
    unsafe extern "C" fn(*mut c_void, c_int, *const ghostty_clipboard_content_s, usize, bool);
pub type ghostty_runtime_close_surface_cb = unsafe extern "C" fn(*mut c_void, bool);

#[repr(C)]
pub struct ghostty_runtime_config_s {
    pub userdata: *mut c_void,
    pub supports_selection_clipboard: bool,
    pub wakeup_cb: ghostty_runtime_wakeup_cb,
    pub action_cb: ghostty_runtime_action_cb,
    pub read_clipboard_cb: ghostty_runtime_read_clipboard_cb,
    pub confirm_read_clipboard_cb: ghostty_runtime_confirm_read_clipboard_cb,
    pub write_clipboard_cb: ghostty_runtime_write_clipboard_cb,
    pub close_surface_cb: ghostty_runtime_close_surface_cb,
}

// -------------------------------------------------------------------
// Functions
// -------------------------------------------------------------------

// GL functions via libepoxy (used by GTK4 for GL dispatch)
extern "C" {
    #[link_name = "epoxy_glViewport"]
    pub fn glViewport(x: c_int, y: c_int, width: c_int, height: c_int);
}

extern "C" {
    // Init
    pub fn ghostty_init(argc: usize, argv: *mut *mut c_char) -> c_int;

    // Config
    pub fn ghostty_config_new() -> ghostty_config_t;
    pub fn ghostty_config_free(config: ghostty_config_t);
    pub fn ghostty_config_load_default_files(config: ghostty_config_t);
    pub fn ghostty_config_load_recursive_files(config: ghostty_config_t);
    pub fn ghostty_config_finalize(config: ghostty_config_t);

    // App
    pub fn ghostty_app_new(
        config: *const ghostty_runtime_config_s,
        ghostty_config: ghostty_config_t,
    ) -> ghostty_app_t;
    pub fn ghostty_app_free(app: ghostty_app_t);
    pub fn ghostty_app_tick(app: ghostty_app_t);
    pub fn ghostty_app_set_focus(app: ghostty_app_t, focused: bool);
    pub fn ghostty_app_set_color_scheme(app: ghostty_app_t, scheme: c_int);

    // Surface config
    pub fn ghostty_surface_config_new() -> ghostty_surface_config_s;

    // Surface
    pub fn ghostty_surface_new(
        app: ghostty_app_t,
        config: *const ghostty_surface_config_s,
    ) -> ghostty_surface_t;
    pub fn ghostty_surface_free(surface: ghostty_surface_t);
    pub fn ghostty_surface_refresh(surface: ghostty_surface_t);
    pub fn ghostty_surface_display_unrealized(surface: ghostty_surface_t);
    pub fn ghostty_surface_display_realized(surface: ghostty_surface_t);
    pub fn ghostty_surface_draw(surface: ghostty_surface_t);
    pub fn ghostty_surface_set_content_scale(surface: ghostty_surface_t, x: f64, y: f64);
    pub fn ghostty_surface_set_focus(surface: ghostty_surface_t, focused: bool);
    pub fn ghostty_surface_set_size(surface: ghostty_surface_t, width: u32, height: u32);
    pub fn ghostty_surface_size(surface: ghostty_surface_t) -> ghostty_surface_size_s;
    pub fn ghostty_surface_key(surface: ghostty_surface_t, event: ghostty_input_key_s) -> bool;
    pub fn ghostty_surface_text(surface: ghostty_surface_t, text: *const c_char, len: usize);
    pub fn ghostty_surface_mouse_button(
        surface: ghostty_surface_t,
        state: c_int,
        button: c_int,
        mods: c_int,
    ) -> bool;
    pub fn ghostty_surface_mouse_pos(surface: ghostty_surface_t, x: f64, y: f64, mods: c_int);
    pub fn ghostty_surface_mouse_scroll(surface: ghostty_surface_t, x: f64, y: f64, mods: c_int);
    pub fn ghostty_surface_request_close(surface: ghostty_surface_t);
    pub fn ghostty_surface_set_color_scheme(surface: ghostty_surface_t, scheme: c_int);

    // Binding actions
    pub fn ghostty_surface_binding_action(
        surface: ghostty_surface_t,
        action: *const c_char,
        action_len: usize,
    ) -> bool;
    pub fn ghostty_surface_has_selection(surface: ghostty_surface_t) -> bool;
    pub fn ghostty_surface_read_selection(
        surface: ghostty_surface_t,
        text: *mut ghostty_text_s,
    ) -> bool;
    pub fn ghostty_surface_free_text(surface: ghostty_surface_t, text: *mut ghostty_text_s);

    // Clipboard
    pub fn ghostty_surface_complete_clipboard_request(
        surface: ghostty_surface_t,
        data: *const c_char,
        state: *mut c_void,
        confirmed: bool,
    );
}
