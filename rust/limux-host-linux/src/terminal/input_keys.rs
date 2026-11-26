use super::*;

// ---------------------------------------------------------------------------
// Key translation
// ---------------------------------------------------------------------------

pub(super) struct TranslatedKeyEvent {
    pub(super) input: ghostty_input_key_s,
    pub(super) resolved_key: c_int,
}

pub(super) fn translate_key_event(
    action: c_int,
    widget: Option<&gtk::Widget>,
    key_event: Option<&gtk::gdk::KeyEvent>,
    keyval: gtk::gdk::Key,
    keycode: u32,
    modifier: gtk::gdk::ModifierType,
) -> TranslatedKeyEvent {
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
    let resolved_key = ghostty_key_for_keyval(keyval);
    // GDK reports the modifiers from before the event, so a modifier key's own
    // press is missing from them and its release is still in them.
    let own_modifier = modifier_for_ghostty_key(resolved_key);
    if action == GHOSTTY_ACTION_RELEASE {
        mods &= !own_modifier;
    } else {
        mods |= own_modifier;
    }

    let unshifted = widget
        .zip(key_event)
        .and_then(|(widget, key_event)| keyval_unicode_unshifted(widget, key_event, keycode))
        .unwrap_or_else(|| fallback_unshifted_codepoint(keyval));

    let consumed = key_event
        .map(translate_consumed_mods)
        .unwrap_or_else(|| fallback_consumed_mods(keyval, modifier));

    TranslatedKeyEvent {
        input: ghostty_input_key_s {
            action,
            mods,
            consumed_mods: consumed,
            keycode,
            text: ptr::null(),
            unshifted_codepoint: unshifted,
            composing: false,
        },
        resolved_key,
    }
}

/// Keyvals as Ghostty keys, matching Ghostty's GTK apprt without the
/// writing-system keys, which keep their physical key.
///
/// Embedded Ghostty derives its key from the XKB keycode, so a key the layout
/// remaps would keep its physical identity: Caps Lock remapped to BackSpace
/// would type nothing. Keypad 1 (`KP_End` with Num Lock off) also keeps keypad
/// identity (`numpad_end`) for `kp_*` keybinds and Kitty keyboard sequences.
pub(super) fn ghostty_key_for_keyval(keyval: gtk::gdk::Key) -> c_int {
    use gtk::gdk::Key;

    match keyval {
        Key::space => GHOSTTY_KEY_SPACE,
        Key::Up => GHOSTTY_KEY_ARROW_UP,
        Key::Down => GHOSTTY_KEY_ARROW_DOWN,
        Key::Right => GHOSTTY_KEY_ARROW_RIGHT,
        Key::Left => GHOSTTY_KEY_ARROW_LEFT,
        Key::Home => GHOSTTY_KEY_HOME,
        Key::End => GHOSTTY_KEY_END,
        Key::Insert => GHOSTTY_KEY_INSERT,
        Key::Delete => GHOSTTY_KEY_DELETE,
        Key::Caps_Lock => GHOSTTY_KEY_CAPS_LOCK,
        Key::Scroll_Lock => GHOSTTY_KEY_SCROLL_LOCK,
        Key::Num_Lock => GHOSTTY_KEY_NUM_LOCK,
        Key::Page_Up => GHOSTTY_KEY_PAGE_UP,
        Key::Page_Down => GHOSTTY_KEY_PAGE_DOWN,
        Key::Escape => GHOSTTY_KEY_ESCAPE,
        Key::Return => GHOSTTY_KEY_ENTER,
        Key::Tab => GHOSTTY_KEY_TAB,
        Key::BackSpace => GHOSTTY_KEY_BACKSPACE,
        Key::Print => GHOSTTY_KEY_PRINT_SCREEN,
        Key::Pause => GHOSTTY_KEY_PAUSE,
        Key::F1 => GHOSTTY_KEY_F1,
        Key::F2 => GHOSTTY_KEY_F2,
        Key::F3 => GHOSTTY_KEY_F3,
        Key::F4 => GHOSTTY_KEY_F4,
        Key::F5 => GHOSTTY_KEY_F5,
        Key::F6 => GHOSTTY_KEY_F6,
        Key::F7 => GHOSTTY_KEY_F7,
        Key::F8 => GHOSTTY_KEY_F8,
        Key::F9 => GHOSTTY_KEY_F9,
        Key::F10 => GHOSTTY_KEY_F10,
        Key::F11 => GHOSTTY_KEY_F11,
        Key::F12 => GHOSTTY_KEY_F12,
        Key::F13 => GHOSTTY_KEY_F13,
        Key::F14 => GHOSTTY_KEY_F14,
        Key::F15 => GHOSTTY_KEY_F15,
        Key::F16 => GHOSTTY_KEY_F16,
        Key::F17 => GHOSTTY_KEY_F17,
        Key::F18 => GHOSTTY_KEY_F18,
        Key::F19 => GHOSTTY_KEY_F19,
        Key::F20 => GHOSTTY_KEY_F20,
        Key::F21 => GHOSTTY_KEY_F21,
        Key::F22 => GHOSTTY_KEY_F22,
        Key::F23 => GHOSTTY_KEY_F23,
        Key::F24 => GHOSTTY_KEY_F24,
        Key::F25 => GHOSTTY_KEY_F25,
        Key::KP_0 => GHOSTTY_KEY_NUMPAD_0,
        Key::KP_1 => GHOSTTY_KEY_NUMPAD_1,
        Key::KP_2 => GHOSTTY_KEY_NUMPAD_2,
        Key::KP_3 => GHOSTTY_KEY_NUMPAD_3,
        Key::KP_4 => GHOSTTY_KEY_NUMPAD_4,
        Key::KP_5 => GHOSTTY_KEY_NUMPAD_5,
        Key::KP_6 => GHOSTTY_KEY_NUMPAD_6,
        Key::KP_7 => GHOSTTY_KEY_NUMPAD_7,
        Key::KP_8 => GHOSTTY_KEY_NUMPAD_8,
        Key::KP_9 => GHOSTTY_KEY_NUMPAD_9,
        Key::KP_Decimal => GHOSTTY_KEY_NUMPAD_DECIMAL,
        Key::KP_Divide => GHOSTTY_KEY_NUMPAD_DIVIDE,
        Key::KP_Multiply => GHOSTTY_KEY_NUMPAD_MULTIPLY,
        Key::KP_Subtract => GHOSTTY_KEY_NUMPAD_SUBTRACT,
        Key::KP_Add => GHOSTTY_KEY_NUMPAD_ADD,
        Key::KP_Enter => GHOSTTY_KEY_NUMPAD_ENTER,
        Key::KP_Equal => GHOSTTY_KEY_NUMPAD_EQUAL,
        Key::KP_Separator => GHOSTTY_KEY_NUMPAD_SEPARATOR,
        Key::KP_Left => GHOSTTY_KEY_NUMPAD_LEFT,
        Key::KP_Right => GHOSTTY_KEY_NUMPAD_RIGHT,
        Key::KP_Up => GHOSTTY_KEY_NUMPAD_UP,
        Key::KP_Down => GHOSTTY_KEY_NUMPAD_DOWN,
        Key::KP_Page_Up => GHOSTTY_KEY_NUMPAD_PAGE_UP,
        Key::KP_Page_Down => GHOSTTY_KEY_NUMPAD_PAGE_DOWN,
        Key::KP_Home => GHOSTTY_KEY_NUMPAD_HOME,
        Key::KP_End => GHOSTTY_KEY_NUMPAD_END,
        Key::KP_Insert => GHOSTTY_KEY_NUMPAD_INSERT,
        Key::KP_Delete => GHOSTTY_KEY_NUMPAD_DELETE,
        Key::KP_Begin => GHOSTTY_KEY_NUMPAD_BEGIN,
        Key::Copy => GHOSTTY_KEY_COPY,
        Key::Cut => GHOSTTY_KEY_CUT,
        Key::Paste => GHOSTTY_KEY_PASTE,
        Key::Shift_L => GHOSTTY_KEY_SHIFT_LEFT,
        Key::Control_L => GHOSTTY_KEY_CONTROL_LEFT,
        Key::Alt_L => GHOSTTY_KEY_ALT_LEFT,
        Key::Super_L => GHOSTTY_KEY_META_LEFT,
        Key::Shift_R => GHOSTTY_KEY_SHIFT_RIGHT,
        Key::Control_R => GHOSTTY_KEY_CONTROL_RIGHT,
        Key::Alt_R => GHOSTTY_KEY_ALT_RIGHT,
        Key::Super_R => GHOSTTY_KEY_META_RIGHT,
        _ => GHOSTTY_KEY_UNIDENTIFIED,
    }
}

fn modifier_for_ghostty_key(key: c_int) -> c_int {
    match key {
        GHOSTTY_KEY_SHIFT_LEFT | GHOSTTY_KEY_SHIFT_RIGHT => GHOSTTY_MODS_SHIFT,
        GHOSTTY_KEY_CONTROL_LEFT | GHOSTTY_KEY_CONTROL_RIGHT => GHOSTTY_MODS_CTRL,
        GHOSTTY_KEY_ALT_LEFT | GHOSTTY_KEY_ALT_RIGHT => GHOSTTY_MODS_ALT,
        GHOSTTY_KEY_META_LEFT | GHOSTTY_KEY_META_RIGHT => GHOSTTY_MODS_SUPER,
        _ => GHOSTTY_MODS_NONE,
    }
}

/// Map a keyval back to a hardware keycode via the display's keymap.
///
/// Real key events arrive from GTK with the keycode already filled in. Keys
/// injected through the control socket only have a keyval (parsed from a string
/// like `enter`), so we have to ask the keymap for the physical key that
/// produces it. Returns 0 when the keyval isn't on the current layout, which
/// preserves the previous behaviour rather than inventing a wrong key.
pub(super) fn keycode_for_keyval(widget: &gtk::Widget, keyval: gtk::gdk::Key) -> u32 {
    widget
        .display()
        .map_keyval(keyval)
        .and_then(|keys| keys.first().map(|key| key.keycode()))
        .unwrap_or(0)
}

pub(super) fn translated_keyval_for_keycode(
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

pub(super) struct StackKeyText {
    buf: [u8; 5],
}

impl StackKeyText {
    pub(super) fn from_keyval(keyval: gtk::gdk::Key) -> Option<Self> {
        let ch = keyval.to_unicode()?;
        if ch.is_control() {
            return None;
        }
        let mut buf = [0u8; 5];
        let encoded = ch.encode_utf8(&mut buf[..4]);
        buf[encoded.len()] = 0;
        Some(Self { buf })
    }

    pub(super) fn as_ptr(&self) -> *const c_char {
        self.buf.as_ptr().cast::<c_char>()
    }
}

#[cfg(test)]
pub(super) fn key_event_text(keyval: gtk::gdk::Key) -> Option<CString> {
    let stack = StackKeyText::from_keyval(keyval)?;
    let cstr = unsafe { std::ffi::CStr::from_ptr(stack.as_ptr()) };
    Some(cstr.to_owned())
}

pub(super) fn encode_terminal_scroll(
    dx: f64,
    dy: f64,
    scale_factor: i32,
    unit: gtk::gdk::ScrollUnit,
    precision_scroll: bool,
) -> (f64, f64, c_int) {
    // Ghostty decodes `scroll_mods` as a packed bitfield (bit 0 = precision,
    // bits 1..3 = momentum), NOT keyboard modifiers.
    //
    // - `ScrollUnit::Surface` (GTK 4.8+ Wayland touchpads): deltas are already
    //   in surface logical pixels; convert 1:1 to physical pixels via
    //   `scale_factor` and set the precision bit (1).
    // - `ScrollUnit::Wheel` with `precision_scroll` (X11 smooth touchpads
    //   without surface units): apply Ghostty's legacy `* 10.0` heuristic with
    //   the precision bit (1).
    // - Discrete `ScrollUnit::Wheel`: pass negated wheel ticks with
    //   `scroll_mods = 0` so Ghostty multiplies by its physical cell size.
    match unit {
        gtk::gdk::ScrollUnit::Surface => {
            let factor = -(scale_factor.max(1) as f64);
            (dx * factor, dy * factor, 1)
        }
        gtk::gdk::ScrollUnit::Wheel if precision_scroll => {
            let factor = -(scale_factor.max(1) as f64) * 10.0;
            (dx * factor, dy * factor, 1)
        }
        _ => (-dx, -dy, 0),
    }
}

pub(super) fn keyval_unicode_unshifted(
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

pub(super) fn translate_consumed_mods(key_event: &gtk::gdk::KeyEvent) -> c_int {
    let consumed = key_event.consumed_modifiers() & gtk::gdk::MODIFIER_MASK;
    translate_mouse_mods(consumed)
}

pub(super) fn fallback_consumed_mods(
    keyval: gtk::gdk::Key,
    modifier: gtk::gdk::ModifierType,
) -> c_int {
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

pub(super) fn fallback_unshifted_codepoint(keyval: gtk::gdk::Key) -> u32 {
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

/// Styles for the clipboard toast, installed once with the app stylesheet.
pub(crate) const CLIPBOARD_TOAST_CSS: &str = r#"
label.limux-toast {
    background: rgba(45, 45, 45, 0.95);
    color: white;
    border-radius: 6px;
    padding: 6px 14px;
    font-size: 12px;
}
"#;

/// Intercept images only for an explicit user paste action.
pub(super) fn paste_clipboard(
    surface: Option<ghostty_surface_t>,
    callbacks: &Rc<RefCell<TerminalCallbacks>>,
) {
    if !image_paste::try_begin(surface, callbacks) {
        surface_action(surface, "paste_from_clipboard");
    }
}

pub(super) fn show_clipboard_toast(overlay: &gtk::Overlay) {
    show_terminal_toast(overlay, "Copied to clipboard", Duration::from_secs(2));
}

pub(super) fn show_terminal_toast(overlay: &gtk::Overlay, message: &str, duration: Duration) {
    // Replace the previous toast instead of stacking status messages.
    let mut child = overlay.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        if widget.has_css_class("limux-toast") {
            overlay.remove_overlay(&widget);
        }
    }

    let toast = gtk::Label::new(Some(message));
    toast.set_wrap(true);
    toast.set_max_width_chars(60);
    toast.add_css_class("limux-toast");
    toast.set_halign(gtk::Align::Center);
    toast.set_valign(gtk::Align::End);
    toast.set_margin_bottom(12);
    toast.set_can_target(false);
    overlay.add_overlay(&toast);

    // Weak, so a toast whose terminal has closed is freed at once.
    let toast = toast.downgrade();
    glib::timeout_add_local_once(duration, move || {
        if let Some(toast) = toast.upgrade() {
            if let Some(overlay) = toast.parent().and_downcast::<gtk::Overlay>() {
                overlay.remove_overlay(&toast);
            }
        }
    });
}

pub(super) fn dropped_file_text(file_list: &gtk::gdk::FileList) -> Option<CString> {
    shell_escape_joined_bytes(
        file_list
            .files()
            .iter()
            .filter_map(|file| file.path())
            .map(|path| path.into_os_string().into_vec()),
    )
}

/// Escape a path so it can be safely pasted into the terminal without sending
/// raw control bytes to Ghostty. Only Bash-like shells read the `$'...'` form
/// that those bytes need, so other paths get plain single quotes.
pub(super) fn shell_escape_bytes(s: &[u8]) -> Vec<u8> {
    match std::str::from_utf8(s) {
        Ok(text) if !text.chars().any(char::is_control) => {
            crate::pane::shell_quote(text).into_bytes()
        }
        _ => Bash::quote_vec(s),
    }
}

pub(super) fn shell_escape_joined_bytes<I, B>(paths: I) -> Option<CString>
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

pub(super) fn translate_mouse_mods(state: gtk::gdk::ModifierType) -> c_int {
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
