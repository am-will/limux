use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;
use gtk4 as gtk;

use crate::shortcut_config::{
    self, NormalizedShortcut, ResolvedShortcutConfig, ShortcutConfigError, ShortcutId,
};

enum CaptureOutcome {
    ContinueListening,
    CancelListening,
    CommitBinding(Option<NormalizedShortcut>),
    Error(String),
}

pub const KEYBIND_EDITOR_LISTENING_CSS: &str = "limux-keybind-editor-listening";

pub const KEYBIND_EDITOR_CSS: &str = r#"
.limux-keybind-editor {
    background-color: @window_bg_color;
    color: @window_fg_color;
    padding: 14px;
}
.limux-keybind-header {
    margin-bottom: 8px;
}
.limux-keybind-title {
    font-weight: 700;
}
.limux-keybind-hint {
    font-size: 12px;
    margin-bottom: 10px;
    opacity: 0.7;
}
.limux-keybind-scroll viewport {
    background: transparent;
}
.limux-keybind-row {
    padding: 10px 12px;
    margin-bottom: 8px;
}
.limux-keybind-action {
    font-weight: 600;
}
.limux-keybind-default {
    font-size: 12px;
    opacity: 0.7;
}
.limux-keybind-capture {
    min-width: 168px;
    padding: 8px 12px;
}
.limux-keybind-capture-listening {
    border-color: @accent_bg_color;
    box-shadow: inset 0 0 0 1px @accent_bg_color;
}
.limux-keybind-error {
    color: @error_color;
    font-size: 12px;
    margin-top: 6px;
}
.limux-keybind-row-hint {
    font-size: 12px;
    margin-top: 6px;
    opacity: 0.7;
}
"#;

#[derive(Clone)]
struct RowWidgets {
    id: ShortcutId,
    binding_button: gtk::Button,
    hint_label: gtk::Label,
    error_label: gtk::Label,
}

pub fn build_keybind_editor(
    shortcuts: &ResolvedShortcutConfig,
    on_capture: Rc<
        dyn Fn(ShortcutId, Option<NormalizedShortcut>) -> Result<ResolvedShortcutConfig, String>,
    >,
) -> gtk::Widget {
    let state = Rc::new(RefCell::new(shortcuts.clone()));
    let listening = Rc::new(RefCell::new(None::<ShortcutId>));
    let errors = Rc::new(RefCell::new(HashMap::<ShortcutId, String>::new()));
    let rows = Rc::new(RefCell::new(Vec::<RowWidgets>::new()));

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    outer.add_css_class("limux-keybind-editor");
    outer.set_width_request(540);
    outer.set_hexpand(true);
    outer.set_vexpand(true);
    outer.set_focusable(true);
    outer.set_can_focus(true);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.add_css_class("limux-keybind-header");

    let title = gtk::Label::builder()
        .label("Keybinds")
        .xalign(0.0)
        .hexpand(true)
        .build();
    title.add_css_class("limux-keybind-title");
    header.append(&title);

    let hint = gtk::Label::builder()
        .label(
            "Click a shortcut field, then press a Ctrl, Alt, or Cmd combo. Shift is allowed as an additional modifier. Press Del to unbind. Press Esc to cancel.",
        )
        .wrap(true)
        .xalign(0.0)
        .build();
    hint.add_css_class("limux-keybind-hint");

    let rows_box = gtk::Box::new(gtk::Orientation::Vertical, 0);

    for definition in shortcut_config::definitions() {
        let shortcut_id = definition.id;

        let row = gtk::Box::new(gtk::Orientation::Vertical, 0);
        row.add_css_class("card");
        row.add_css_class("limux-keybind-row");

        let top = gtk::Box::new(gtk::Orientation::Horizontal, 12);

        let meta = gtk::Box::new(gtk::Orientation::Vertical, 4);
        meta.set_hexpand(true);

        let action_label = gtk::Label::builder()
            .label(definition.label)
            .xalign(0.0)
            .hexpand(true)
            .build();
        action_label.add_css_class("limux-keybind-action");

        let default_label = gtk::Label::builder()
            .label(format!(
                "Default: {}",
                shortcuts
                    .default_display_label_for_id(definition.id)
                    .unwrap_or_else(|| definition.default_display_label())
            ))
            .xalign(0.0)
            .wrap(true)
            .build();
        default_label.add_css_class("limux-keybind-default");
        default_label.set_opacity(0.7);

        meta.append(&action_label);
        meta.append(&default_label);

        let binding_button =
            gtk::Button::with_label(&binding_button_label(shortcuts, definition.id, false));
        binding_button.add_css_class("limux-keybind-capture");
        binding_button.set_focusable(true);
        binding_button.set_can_focus(true);
        binding_button.set_focus_on_click(true);
        binding_button.set_halign(gtk::Align::End);

        let error_label = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .visible(false)
            .build();
        error_label.add_css_class("limux-keybind-error");

        let hint_label = gtk::Label::builder()
            .label("Press Del to unbind. Esc cancels.")
            .xalign(0.0)
            .wrap(true)
            .visible(false)
            .build();
        hint_label.add_css_class("limux-keybind-row-hint");
        hint_label.set_opacity(0.7);

        top.append(&meta);
        top.append(&binding_button);
        row.append(&top);
        row.append(&hint_label);
        row.append(&error_label);
        rows_box.append(&row);

        rows.borrow_mut().push(RowWidgets {
            id: definition.id,
            binding_button: binding_button.clone(),
            hint_label: hint_label.clone(),
            error_label: error_label.clone(),
        });

        {
            let listening = listening.clone();
            let errors = errors.clone();
            let rows = rows.clone();
            let state = state.clone();
            let outer = outer.clone();
            binding_button.connect_clicked(move |button| {
                *listening.borrow_mut() = Some(shortcut_id);
                errors.borrow_mut().remove(&shortcut_id);
                sync_editor_listening_class(&outer, true);
                refresh_rows(
                    &rows.borrow(),
                    &state.borrow(),
                    *listening.borrow(),
                    &errors.borrow(),
                );
                button.grab_focus();
            });
        }
    }

    {
        let listening = listening.clone();
        let errors = errors.clone();
        let rows = rows.clone();
        let state = state.clone();
        let on_capture = on_capture.clone();
        let outer_for_controller = outer.clone();
        let key_controller = gtk::EventControllerKey::new();
        key_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        key_controller.connect_key_pressed(move |controller, keyval, keycode, modifier| {
            let Some(shortcut_id) = *listening.borrow() else {
                return gtk::glib::Propagation::Proceed;
            };
            let Some(definition) = shortcut_config::definitions()
                .iter()
                .find(|definition| definition.id == shortcut_id)
            else {
                return gtk::glib::Propagation::Proceed;
            };
            let display = controller.widget().map(|widget| widget.display());

            match capture_outcome_for_key_press(
                display.as_ref(),
                keyval,
                keycode,
                modifier,
                definition.config_key,
            ) {
                CaptureOutcome::ContinueListening => {
                    return gtk::glib::Propagation::Stop;
                }
                CaptureOutcome::CancelListening => {
                    *listening.borrow_mut() = None;
                    errors.borrow_mut().remove(&shortcut_id);
                    sync_editor_listening_class(&outer_for_controller, false);
                }
                CaptureOutcome::CommitBinding(binding) => match on_capture(shortcut_id, binding) {
                    Ok(updated) => {
                        *state.borrow_mut() = updated;
                        *listening.borrow_mut() = None;
                        errors.borrow_mut().remove(&shortcut_id);
                        sync_editor_listening_class(&outer_for_controller, false);
                    }
                    Err(err) => {
                        *listening.borrow_mut() = None;
                        errors.borrow_mut().insert(shortcut_id, err);
                        sync_editor_listening_class(&outer_for_controller, false);
                    }
                },
                CaptureOutcome::Error(message) => {
                    *listening.borrow_mut() = None;
                    errors.borrow_mut().insert(shortcut_id, message);
                    sync_editor_listening_class(&outer_for_controller, false);
                }
            }

            refresh_rows(
                &rows.borrow(),
                &state.borrow(),
                *listening.borrow(),
                &errors.borrow(),
            );
            gtk::glib::Propagation::Stop
        });
        outer.add_controller(key_controller);
    }

    refresh_rows(&rows.borrow(), shortcuts, None, &HashMap::new());

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&rows_box)
        .build();
    scroller.add_css_class("limux-keybind-scroll");
    scroller.set_hexpand(true);
    scroller.set_vexpand(true);

    outer.append(&header);
    outer.append(&hint);
    outer.append(&scroller);
    outer.upcast()
}

fn sync_editor_listening_class(editor: &gtk::Box, listening: bool) {
    if listening {
        editor.add_css_class(KEYBIND_EDITOR_LISTENING_CSS);
    } else {
        editor.remove_css_class(KEYBIND_EDITOR_LISTENING_CSS);
    }
}

fn binding_button_label(
    shortcuts: &ResolvedShortcutConfig,
    id: ShortcutId,
    listening: bool,
) -> String {
    if listening {
        return "Press shortcut…".to_string();
    }

    shortcuts
        .display_label_for_id(id)
        .unwrap_or_else(|| "Unbound".to_string())
}

fn refresh_rows(
    rows: &[RowWidgets],
    shortcuts: &ResolvedShortcutConfig,
    listening: Option<ShortcutId>,
    errors: &HashMap<ShortcutId, String>,
) {
    for row in rows {
        let is_listening = listening == Some(row.id);
        row.binding_button
            .set_label(&binding_button_label(shortcuts, row.id, is_listening));
        row.hint_label.set_visible(is_listening);
        if is_listening {
            row.binding_button
                .add_css_class("limux-keybind-capture-listening");
        } else {
            row.binding_button
                .remove_css_class("limux-keybind-capture-listening");
        }

        if let Some(error) = errors.get(&row.id) {
            row.error_label.set_label(error);
            row.error_label.set_visible(true);
        } else {
            row.error_label.set_visible(false);
        }
    }
}

#[cfg(test)]
fn capture_outcome_for_key_event(
    keyval: gtk::gdk::Key,
    modifier: gtk::gdk::ModifierType,
    config_key: &str,
) -> CaptureOutcome {
    capture_outcome_for_key_press(None, keyval, 0, modifier, config_key)
}

fn capture_outcome_for_key_press(
    display: Option<&gtk::gdk::Display>,
    keyval: gtk::gdk::Key,
    keycode: u32,
    modifier: gtk::gdk::ModifierType,
    config_key: &str,
) -> CaptureOutcome {
    if keyval == gtk::gdk::Key::Escape {
        return CaptureOutcome::CancelListening;
    }

    let unbind_modifiers = gtk::gdk::ModifierType::SHIFT_MASK
        | gtk::gdk::ModifierType::CONTROL_MASK
        | gtk::gdk::ModifierType::ALT_MASK
        | gtk::gdk::ModifierType::META_MASK
        | gtk::gdk::ModifierType::SUPER_MASK;
    if matches!(keyval, gtk::gdk::Key::Delete | gtk::gdk::Key::KP_Delete)
        && !modifier.intersects(unbind_modifiers)
    {
        return CaptureOutcome::CommitBinding(None);
    }

    let Some(binding) = NormalizedShortcut::from_gdk_key_event(display, keyval, keycode, modifier)
    else {
        return CaptureOutcome::ContinueListening;
    };

    let Some(definition) = shortcut_config::definition_by_config_key(config_key) else {
        return CaptureOutcome::Error("That shortcut is not valid.".to_string());
    };

    match binding.validate_host_binding(definition) {
        Ok(()) => CaptureOutcome::CommitBinding(Some(binding)),
        Err(err) => CaptureOutcome::Error(validation_error_message(&err)),
    }
}

fn validation_error_message(err: &ShortcutConfigError) -> String {
    match err {
        ShortcutConfigError::BaseModifierRequired { .. } => {
            "Use Ctrl, Alt, or Cmd together with another key.".to_string()
        }
        ShortcutConfigError::ModifierOnlyBinding { .. } => {
            "Choose a non-modifier key for this shortcut.".to_string()
        }
        ShortcutConfigError::DuplicateBinding { .. } => {
            "That shortcut is already assigned to another action.".to_string()
        }
        _ => "That shortcut is not valid.".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        binding_button_label, capture_outcome_for_key_event, validation_error_message,
        CaptureOutcome,
    };
    use crate::shortcut_config::{
        default_shortcuts, resolve_shortcuts_from_str, ShortcutConfigError, ShortcutId,
    };
    use gtk4::gdk;

    #[test]
    fn binding_button_label_prefers_current_binding_and_listening_state() {
        let defaults = default_shortcuts();
        assert_eq!(
            binding_button_label(&defaults, ShortcutId::SplitRight, false),
            "Ctrl+D"
        );
        assert_eq!(
            binding_button_label(&defaults, ShortcutId::SplitRight, true),
            "Press shortcut…"
        );

        let remapped = resolve_shortcuts_from_str(
            r#"{
                "shortcuts": {
                    "split_right": "<Ctrl><Alt>h"
                }
            }"#,
        )
        .unwrap();
        assert_eq!(
            binding_button_label(&remapped, ShortcutId::SplitRight, false),
            "Ctrl+Alt+H"
        );
    }

    #[test]
    fn validation_error_message_is_user_facing() {
        let err = ShortcutConfigError::BaseModifierRequired {
            shortcut_id: "split_right".to_string(),
            input: "<Shift>h".to_string(),
        };
        assert_eq!(
            validation_error_message(&err),
            "Use Ctrl, Alt, or Cmd together with another key."
        );
    }

    #[test]
    fn capture_outcome_keeps_listening_for_modifier_only_press() {
        assert!(matches!(
            capture_outcome_for_key_event(
                gdk::Key::Control_L,
                gdk::ModifierType::empty(),
                "split_right"
            ),
            CaptureOutcome::ContinueListening
        ));
    }

    #[test]
    fn capture_outcome_commits_first_non_modifier_with_current_modifiers() {
        match capture_outcome_for_key_event(
            gdk::Key::_0,
            gdk::ModifierType::CONTROL_MASK,
            "split_right",
        ) {
            CaptureOutcome::CommitBinding(Some(binding)) => {
                assert_eq!(binding.to_display_label(), "Ctrl+0");
            }
            _ => panic!("expected capture"),
        }
    }

    #[test]
    fn capture_outcome_supports_delete_to_unbind() {
        assert!(matches!(
            capture_outcome_for_key_event(
                gdk::Key::Delete,
                gdk::ModifierType::empty(),
                "split_right"
            ),
            CaptureOutcome::CommitBinding(None)
        ));
    }

    #[test]
    fn capture_outcome_keeps_modified_delete_available_for_binding() {
        assert!(matches!(
            capture_outcome_for_key_event(
                gdk::Key::Delete,
                gdk::ModifierType::CONTROL_MASK,
                "split_right"
            ),
            CaptureOutcome::CommitBinding(Some(_))
        ));
    }
}
