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

    let unshifted = widget
        .zip(key_event)
        .and_then(|(widget, key_event)| keyval_unicode_unshifted(widget, key_event, keycode))
        .unwrap_or_else(|| fallback_unshifted_codepoint(keyval));

    let consumed = key_event
        .map(translate_consumed_mods)
        .unwrap_or_else(|| fallback_consumed_mods(keyval, modifier));
    let keycode = ghostty_keycode_with_keyval_remap(keyval, keycode);

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
        resolved_key: ghostty_key_for_keyval(keyval),
    }
}

pub(super) fn ghostty_keycode_with_keyval_remap(keyval: gtk::gdk::Key, keycode: u32) -> u32 {
    const XKB_KEYCODE_ESCAPE: u32 = 9;
    const XKB_KEYCODE_CAPS_LOCK: u32 = 66;

    match keyval {
        gtk::gdk::Key::Escape => XKB_KEYCODE_ESCAPE,
        gtk::gdk::Key::Caps_Lock => XKB_KEYCODE_CAPS_LOCK,
        _ => keycode,
    }
}

/// Keypad keyvals as Ghostty keys, matching Ghostty's GTK apprt.
///
/// Embedded Ghostty derives its key from the XKB keycode, so with Num Lock off
/// keypad 1 (`KP_End`) would still resolve to `numpad_1`. Passing the keyval's
/// key keeps keypad identity (`numpad_end`) for `kp_*` keybinds and Kitty
/// keyboard sequences while still producing navigation.
pub(super) fn ghostty_key_for_keyval(keyval: gtk::gdk::Key) -> c_int {
    use gtk::gdk::Key;

    match keyval {
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
        _ => GHOSTTY_KEY_UNIDENTIFIED,
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

pub(super) fn key_event_text(keyval: gtk::gdk::Key) -> Option<CString> {
    let ch = keyval.to_unicode()?;
    if ch.is_control() {
        return None;
    }

    let mut buf = [0u8; 4];
    let s = ch.encode_utf8(&mut buf);
    CString::new(s.as_bytes()).ok()
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

/// Bash-escape a path so it can be safely pasted into the terminal without
/// sending raw control bytes to Ghostty.
pub(super) fn shell_escape_bytes(s: &[u8]) -> Vec<u8> {
    Bash::quote_vec(s)
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
