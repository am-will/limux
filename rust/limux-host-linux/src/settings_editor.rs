use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;

use crate::app_config::{
    AppConfig, ColorScheme, NotificationSound, UiScale, WindowControlsSide, DEFAULT_UI_SCALE,
};
use crate::keybind_editor;
use crate::shortcut_config::{NormalizedShortcut, ResolvedShortcutConfig, ShortcutId};

pub const SETTINGS_CSS: &str = r#"
.limux-settings-window {
    background-color: @window_bg_color;
    color: @window_fg_color;
}
"#;

const MIN_UI_SCALE_PERCENT: f64 = 80.0;
const MAX_UI_SCALE_PERCENT: f64 = 200.0;
const UI_SCALE_STEP_PERCENT: f64 = 5.0;

#[derive(Clone, Copy, Debug)]
struct UiScaleDescriptor {
    selector: &'static str,
    property: &'static str,
    base_size: f32,
}

const UI_SCALE_DESCRIPTORS: &[UiScaleDescriptor] = &[
    UiScaleDescriptor {
        selector: ".limux-ws-name",
        property: "font-size",
        base_size: 15.0,
    },
    UiScaleDescriptor {
        selector: ".limux-ws-star-btn",
        property: "font-size",
        base_size: 22.0,
    },
    UiScaleDescriptor {
        selector: ".limux-notify-dot, .limux-notify-dot-hidden",
        property: "font-size",
        base_size: 10.0,
    },
    UiScaleDescriptor {
        selector: ".limux-notify-msg, .limux-notify-msg-unread",
        property: "font-size",
        base_size: 11.0,
    },
    UiScaleDescriptor {
        selector: ".limux-sidebar-title",
        property: "font-size",
        base_size: 11.0,
    },
    UiScaleDescriptor {
        selector: ".limux-ws-path",
        property: "font-size",
        base_size: 12.0,
    },
    UiScaleDescriptor {
        selector: ".limux-tab",
        property: "font-size",
        base_size: 12.0,
    },
    UiScaleDescriptor {
        selector: ".limux-pin-icon",
        property: "font-size",
        base_size: 9.0,
    },
    UiScaleDescriptor {
        selector: ".limux-tab-rename-entry",
        property: "font-size",
        base_size: 12.0,
    },
    UiScaleDescriptor {
        selector: ".limux-browser-url-entry, .limux-browser-search-entry",
        property: "font-size",
        base_size: 12.0,
    },
    UiScaleDescriptor {
        selector: ".limux-keybind-hint, .limux-keybind-default, .limux-keybind-error, .limux-keybind-row-hint",
        property: "font-size",
        base_size: 12.0,
    },
    UiScaleDescriptor {
        selector: ".limux-pane-action image",
        property: "-gtk-icon-size",
        base_size: 16.0,
    },
    UiScaleDescriptor {
        selector: ".limux-tab-close image",
        property: "-gtk-icon-size",
        base_size: 12.0,
    },
];

pub fn ui_scale_css(config: &AppConfig) -> String {
    let scale = config.appearance.ui_scale.get();
    UI_SCALE_DESCRIPTORS
        .iter()
        .map(|descriptor| {
            format!(
                "{} {{ {}: {:.3}px; }}",
                descriptor.selector,
                descriptor.property,
                descriptor.base_size * scale
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

type OnConfigChanged = dyn Fn(&AppConfig, &AppConfig);

pub struct SettingsEditorInput {
    pub config: Rc<RefCell<AppConfig>>,
    pub shortcuts: Rc<ResolvedShortcutConfig>,
    pub on_capture: Rc<
        dyn Fn(ShortcutId, Option<NormalizedShortcut>) -> Result<ResolvedShortcutConfig, String>,
    >,
    pub on_config_changed: Rc<OnConfigChanged>,
}

pub fn present_settings_dialog(parent: &impl IsA<gtk::Widget>, input: SettingsEditorInput) {
    let window = adw::Window::new();
    window.set_title(Some("Settings"));
    window.set_default_size(760, 680);
    window.set_modal(true);

    if let Some(parent_window) = parent
        .root()
        .and_then(|root| root.downcast::<gtk::Window>().ok())
    {
        window.set_transient_for(Some(&parent_window));
        if let Some(app) = parent_window.application() {
            window.set_application(Some(&app));
        }
    }

    let content = build_settings_window_content(&window, input);
    window.set_content(Some(&content));
    window.present();
}

fn apply_config_change<F, G>(
    config: &Rc<RefCell<AppConfig>>,
    on_changed: &F,
    update: G,
) -> AppConfig
where
    F: Fn(&AppConfig, &AppConfig) + ?Sized,
    G: FnOnce(&mut AppConfig),
{
    let (previous, updated) = {
        let mut config_ref = config.borrow_mut();
        let previous = config_ref.clone();
        update(&mut config_ref);
        let updated = config_ref.clone();
        (previous, updated)
    };
    on_changed(&previous, &updated);
    config.borrow().clone()
}

fn build_settings_window_content(window: &adw::Window, input: SettingsEditorInput) -> gtk::Widget {
    let stack = adw::ViewStack::new();
    stack.set_hexpand(true);
    stack.set_vexpand(true);

    let general_page = build_general_page(&input);
    let general_stack_page = stack.add_titled(&general_page, Some("general"), "General");
    general_stack_page.set_icon_name(Some("preferences-system-symbolic"));

    let notifications_page = build_notifications_page(&input);
    let notifications_stack_page =
        stack.add_titled(&notifications_page, Some("notifications"), "Notifications");
    notifications_stack_page.set_icon_name(Some("preferences-system-notifications-symbolic"));

    let keybinds_page = keybind_editor::build_keybind_editor(&input.shortcuts, input.on_capture);
    let keybinds_stack_page = stack.add_titled(&keybinds_page, Some("keybindings"), "Keybindings");
    keybinds_stack_page.set_icon_name(Some("input-keyboard-symbolic"));

    let switcher = adw::ViewSwitcher::builder()
        .stack(&stack)
        .policy(adw::ViewSwitcherPolicy::Wide)
        .build();

    let close_button = gtk::Button::builder()
        .icon_name("window-close-symbolic")
        .tooltip_text("Close settings")
        .valign(gtk::Align::Center)
        .build();
    close_button.add_css_class("flat");

    {
        let window = window.clone();
        close_button.connect_clicked(move |_| {
            window.close();
        });
    }

    let header_bar = adw::HeaderBar::new();
    header_bar.set_show_start_title_buttons(false);
    header_bar.set_show_end_title_buttons(false);
    header_bar.set_title_widget(Some(&switcher));
    header_bar.pack_end(&close_button);

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    outer.add_css_class("limux-settings-window");
    outer.append(&header_bar);
    outer.append(&stack);
    outer.upcast()
}

fn build_general_page(input: &SettingsEditorInput) -> gtk::Widget {
    let page = adw::PreferencesPage::new();
    page.set_title("General");
    page.set_name(Some("general"));
    page.set_icon_name(Some("preferences-system-symbolic"));
    page.set_hexpand(true);
    page.set_vexpand(true);

    let group = adw::PreferencesGroup::new();

    let color_row = adw::ActionRow::builder()
        .title("GTK color scheme")
        .subtitle("Choose whether the GTK interface follows system, dark, or light")
        .build();
    color_row.set_title_lines(1);
    color_row.set_subtitle_lines(2);
    let color_dropdown = gtk::DropDown::from_strings(&["System", "Dark", "Light"]);
    let initial_scheme = input.config.borrow().appearance.color_scheme;
    color_dropdown.set_selected(match initial_scheme {
        ColorScheme::System => 0,
        ColorScheme::Dark => 1,
        ColorScheme::Light => 2,
    });
    color_dropdown.set_valign(gtk::Align::Center);
    color_row.add_suffix(&color_dropdown);
    color_row.set_activatable_widget(Some(&color_dropdown));
    group.add(&color_row);

    let ghostty_row = adw::ActionRow::builder()
        .title("Ghostty color scheme")
        .subtitle("Choose whether terminal surfaces follow system, dark, or light")
        .build();
    ghostty_row.set_title_lines(1);
    ghostty_row.set_subtitle_lines(2);
    let ghostty_dropdown = gtk::DropDown::from_strings(&["System", "Dark", "Light"]);
    let initial_ghostty_scheme = input.config.borrow().appearance.ghostty_color_scheme;
    ghostty_dropdown.set_selected(match initial_ghostty_scheme {
        ColorScheme::System => 0,
        ColorScheme::Dark => 1,
        ColorScheme::Light => 2,
    });
    ghostty_dropdown.set_valign(gtk::Align::Center);
    ghostty_row.add_suffix(&ghostty_dropdown);
    ghostty_row.set_activatable_widget(Some(&ghostty_dropdown));
    group.add(&ghostty_row);

    let hover_row = adw::ActionRow::builder()
        .title("Hover terminal focus")
        .subtitle("Focus terminal panes when the mouse pointer enters them")
        .build();
    hover_row.set_title_lines(1);
    hover_row.set_subtitle_lines(2);
    let hover_switch = gtk::Switch::new();
    hover_switch.set_active(input.config.borrow().focus.hover_terminal_focus);
    hover_switch.set_valign(gtk::Align::Center);
    hover_row.add_suffix(&hover_switch);
    hover_row.set_activatable_widget(Some(&hover_switch));
    group.add(&hover_row);

    let workspace_path_row = adw::ActionRow::builder()
        .title("Show workspace path")
        .subtitle("Display workspace folder below its name")
        .build();
    workspace_path_row.set_title_lines(1);
    workspace_path_row.set_subtitle_lines(2);
    let workspace_path_switch = gtk::Switch::new();
    workspace_path_switch.set_active(input.config.borrow().appearance.show_workspace_path);
    workspace_path_switch.set_valign(gtk::Align::Center);
    workspace_path_row.add_suffix(&workspace_path_switch);
    workspace_path_row.set_activatable_widget(Some(&workspace_path_switch));
    group.add(&workspace_path_row);

    let keep_workspace_open_row = adw::ActionRow::builder()
        .title("Keep workspace open")
        .subtitle("Keep the workspace available after its final terminal exits")
        .build();
    keep_workspace_open_row.set_title_lines(1);
    keep_workspace_open_row.set_subtitle_lines(2);
    let keep_workspace_open_switch = gtk::Switch::new();
    keep_workspace_open_switch.set_active(
        input
            .config
            .borrow()
            .workspace
            .keep_open_after_last_terminal_closes,
    );
    keep_workspace_open_switch.set_valign(gtk::Align::Center);
    keep_workspace_open_row.add_suffix(&keep_workspace_open_switch);
    keep_workspace_open_row.set_activatable_widget(Some(&keep_workspace_open_switch));
    group.add(&keep_workspace_open_row);

    let auto_copy_row = adw::ActionRow::builder()
        .title("Copy selection automatically")
        .subtitle("Copy selected terminal text to the regular clipboard")
        .build();
    auto_copy_row.set_title_lines(1);
    auto_copy_row.set_subtitle_lines(2);
    let auto_copy_switch = gtk::Switch::new();
    auto_copy_switch.set_active(input.config.borrow().clipboard.copy_selection_to_clipboard);
    auto_copy_switch.set_valign(gtk::Align::Center);
    auto_copy_row.add_suffix(&auto_copy_switch);
    auto_copy_row.set_activatable_widget(Some(&auto_copy_switch));
    group.add(&auto_copy_row);

    let scale_row = adw::ActionRow::builder()
        .title("Interface scale")
        .subtitle("Scale Limux sidebar, pane header, and settings text")
        .build();
    scale_row.set_title_lines(1);
    scale_row.set_subtitle_lines(2);
    let scale = input.config.borrow().appearance.ui_scale.get();
    let scale_adjustment = gtk::Adjustment::new(
        f64::from(scale) * 100.0,
        MIN_UI_SCALE_PERCENT,
        MAX_UI_SCALE_PERCENT,
        UI_SCALE_STEP_PERCENT,
        UI_SCALE_STEP_PERCENT * 2.0,
        0.0,
    );
    let scale_spin = gtk::SpinButton::builder()
        .adjustment(&scale_adjustment)
        .digits(0)
        .numeric(true)
        .valign(gtk::Align::Center)
        .width_chars(4)
        .build();
    let scale_reset_button = gtk::Button::builder()
        .label("Default")
        .tooltip_text("Reset interface scale")
        .valign(gtk::Align::Center)
        .build();
    scale_row.add_suffix(&scale_spin);
    scale_row.add_suffix(&scale_reset_button);
    scale_row.set_activatable_widget(Some(&scale_spin));
    group.add(&scale_row);

    let top_bar_row = adw::ActionRow::builder()
        .title("Top bar")
        .subtitle("Show a top bar with workspace indicators. When off, the dock toggle, settings, new workspace, and window controls move into the sidebar header (or the leading pane when the sidebar is collapsed).")
        .build();
    top_bar_row.set_title_lines(1);
    top_bar_row.set_subtitle_lines(4);
    let top_bar_switch = gtk::Switch::new();
    top_bar_switch.set_active(input.config.borrow().interface.show_top_bar);
    top_bar_switch.set_valign(gtk::Align::Center);
    top_bar_row.add_suffix(&top_bar_switch);
    top_bar_row.set_activatable_widget(Some(&top_bar_switch));
    group.add(&top_bar_row);

    let indicators_row = adw::ActionRow::builder()
        .title("Workspace indicators on the top bar")
        .subtitle("Show a clickable pill for each workspace in the top bar")
        .build();
    indicators_row.set_title_lines(1);
    indicators_row.set_subtitle_lines(2);
    let indicators_switch = gtk::Switch::new();
    indicators_switch.set_active(input.config.borrow().interface.show_workspace_indicators);
    indicators_switch.set_valign(gtk::Align::Center);
    indicators_row.add_suffix(&indicators_switch);
    indicators_row.set_activatable_widget(Some(&indicators_switch));
    group.add(&indicators_row);

    let controls_row = adw::ActionRow::builder()
        .title("Window controls side")
        .subtitle("Place close, minimize, and maximize on the left or right of the top bar (or of the sidebar header when the top bar is off)")
        .build();
    controls_row.set_title_lines(1);
    controls_row.set_subtitle_lines(3);
    let controls_dropdown = gtk::DropDown::from_strings(&["Left", "Right"]);
    let initial_side = input.config.borrow().interface.window_controls_side;
    controls_dropdown.set_selected(match initial_side {
        WindowControlsSide::Left => 0,
        WindowControlsSide::Right => 1,
    });
    controls_dropdown.set_valign(gtk::Align::Center);
    controls_row.add_suffix(&controls_dropdown);
    controls_row.set_activatable_widget(Some(&controls_dropdown));
    group.add(&controls_row);

    page.add(&group);

    {
        let config = input.config.clone();
        let on_changed = input.on_config_changed.clone();
        color_dropdown.connect_selected_notify(move |dropdown| {
            let scheme = match dropdown.selected() {
                1 => ColorScheme::Dark,
                2 => ColorScheme::Light,
                _ => ColorScheme::System,
            };
            apply_config_change(&config, &*on_changed, move |c| {
                c.appearance.color_scheme = scheme;
            });
        });
    }
    {
        let config = input.config.clone();
        let on_changed = input.on_config_changed.clone();
        ghostty_dropdown.connect_selected_notify(move |dropdown| {
            let scheme = match dropdown.selected() {
                1 => ColorScheme::Dark,
                2 => ColorScheme::Light,
                _ => ColorScheme::System,
            };
            apply_config_change(&config, &*on_changed, move |c| {
                c.appearance.ghostty_color_scheme = scheme;
            });
        });
    }
    {
        let config = input.config.clone();
        let on_changed = input.on_config_changed.clone();
        hover_switch.connect_active_notify(move |switch| {
            let hover_terminal_focus = switch.is_active();
            apply_config_change(&config, &*on_changed, move |c| {
                c.focus.hover_terminal_focus = hover_terminal_focus;
            });
        });
    }
    {
        let config = input.config.clone();
        let on_changed = input.on_config_changed.clone();
        let syncing = Rc::new(Cell::new(false));
        workspace_path_switch.connect_active_notify(move |switch| {
            if syncing.get() {
                return;
            }
            let show_workspace_path = switch.is_active();
            let effective = apply_config_change(&config, &*on_changed, move |c| {
                c.appearance.show_workspace_path = show_workspace_path;
            })
            .appearance
            .show_workspace_path;
            if switch.is_active() != effective {
                syncing.set(true);
                switch.set_active(effective);
                syncing.set(false);
            }
        });
    }
    {
        let config = input.config.clone();
        let on_changed = input.on_config_changed.clone();
        keep_workspace_open_switch.connect_active_notify(move |switch| {
            let keep_open = switch.is_active();
            apply_config_change(&config, &*on_changed, move |c| {
                c.workspace.keep_open_after_last_terminal_closes = keep_open;
            });
        });
    }
    {
        let config = input.config.clone();
        let on_changed = input.on_config_changed.clone();
        auto_copy_switch.connect_active_notify(move |switch| {
            let copy_selection_to_clipboard = switch.is_active();
            apply_config_change(&config, &*on_changed, move |c| {
                c.clipboard.copy_selection_to_clipboard = copy_selection_to_clipboard;
            });
        });
    }
    {
        let config = input.config.clone();
        let on_changed = input.on_config_changed.clone();
        top_bar_switch.connect_active_notify(move |switch| {
            let show_top_bar = switch.is_active();
            apply_config_change(&config, &*on_changed, move |c| {
                c.interface.show_top_bar = show_top_bar;
            });
        });
    }
    {
        let config = input.config.clone();
        let on_changed = input.on_config_changed.clone();
        let updating_spin = Rc::new(Cell::new(false));
        let updating_spin_for_reset = updating_spin.clone();
        scale_spin.connect_value_changed(move |spin| {
            if updating_spin.get() {
                return;
            }
            let scale = UiScale::new((spin.value() / 100.0) as f32).unwrap_or_default();
            apply_config_change(&config, &*on_changed, move |c| {
                c.appearance.ui_scale = scale;
            });
        });

        let config = input.config.clone();
        let on_changed = input.on_config_changed.clone();
        let scale_spin = scale_spin.clone();
        scale_reset_button.connect_clicked(move |_| {
            apply_config_change(&config, &*on_changed, move |c| {
                c.appearance.ui_scale = UiScale::default();
            });
            updating_spin_for_reset.set(true);
            scale_spin.set_value(f64::from(DEFAULT_UI_SCALE) * 100.0);
            updating_spin_for_reset.set(false);
        });
    }
    {
        let config = input.config.clone();
        let on_changed = input.on_config_changed.clone();
        indicators_switch.connect_active_notify(move |switch| {
            let show_workspace_indicators = switch.is_active();
            apply_config_change(&config, &*on_changed, move |c| {
                c.interface.show_workspace_indicators = show_workspace_indicators;
            });
        });
    }
    {
        let config = input.config.clone();
        let on_changed = input.on_config_changed.clone();
        controls_dropdown.connect_selected_notify(move |dropdown| {
            let side = match dropdown.selected() {
                0 => WindowControlsSide::Left,
                _ => WindowControlsSide::Right,
            };
            apply_config_change(&config, &*on_changed, move |c| {
                c.interface.window_controls_side = side;
            });
        });
    }

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&page)
        .build();
    scroller.set_hexpand(true);
    scroller.set_vexpand(true);

    scroller.upcast()
}

fn build_notifications_page(input: &SettingsEditorInput) -> gtk::Widget {
    let page = adw::PreferencesPage::new();
    page.set_title("Notifications");
    page.set_name(Some("notifications"));
    page.set_icon_name(Some("preferences-system-notifications-symbolic"));
    page.set_hexpand(true);
    page.set_vexpand(true);

    let group = adw::PreferencesGroup::new();

    let enabled_row = adw::ActionRow::builder()
        .title("Desktop notifications")
        .subtitle("Show desktop alerts when background workspaces need attention")
        .build();
    enabled_row.set_title_lines(1);
    enabled_row.set_subtitle_lines(2);
    let notifications = input.config.borrow().notifications;
    let enabled_switch = gtk::Switch::new();
    enabled_switch.set_active(notifications.enabled);
    enabled_switch.set_valign(gtk::Align::Center);
    enabled_row.add_suffix(&enabled_switch);
    enabled_row.set_activatable_widget(Some(&enabled_switch));
    group.add(&enabled_row);

    let sound_row = adw::ActionRow::builder()
        .title("Notification sound")
        .subtitle("Choose sound hint sent with desktop alerts. Support depends on your desktop notification service")
        .build();
    sound_row.set_title_lines(1);
    sound_row.set_subtitle_lines(3);
    sound_row.set_sensitive(notifications.enabled);
    let sound_dropdown = gtk::DropDown::from_strings(NotificationSound::labels());
    sound_dropdown.set_selected(notifications.sound.dropdown_index());
    sound_dropdown.set_valign(gtk::Align::Center);
    sound_row.add_suffix(&sound_dropdown);
    sound_row.set_activatable_widget(Some(&sound_dropdown));
    group.add(&sound_row);

    page.add(&group);

    {
        let config = input.config.clone();
        let on_changed = input.on_config_changed.clone();
        let sound_row = sound_row.clone();
        enabled_switch.connect_active_notify(move |switch| {
            let enabled = switch.is_active();
            sound_row.set_sensitive(enabled);
            apply_config_change(&config, &*on_changed, move |c| {
                c.notifications.enabled = enabled;
            });
        });
    }
    {
        let config = input.config.clone();
        let on_changed = input.on_config_changed.clone();
        sound_dropdown.connect_selected_notify(move |dropdown| {
            let sound = NotificationSound::from_dropdown_index(dropdown.selected());
            apply_config_change(&config, &*on_changed, move |c| {
                c.notifications.sound = sound;
            });
        });
    }

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&page)
        .build();
    scroller.set_hexpand(true);
    scroller.set_vexpand(true);

    scroller.upcast()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_config_change_returns_reentrant_config_state() {
        let config = Rc::new(RefCell::new(AppConfig::default()));

        let effective = apply_config_change(
            &config,
            &|previous, _updated| {
                config.borrow_mut().clone_from(previous);
            },
            |current| {
                current.focus.hover_terminal_focus = true;
            },
        );

        assert!(!effective.focus.hover_terminal_focus);
        assert!(!config.borrow().focus.hover_terminal_focus);
    }

    #[test]
    fn ui_scale_css_scales_sidebar_and_pane_chrome() {
        let mut config = AppConfig::default();
        config.appearance.ui_scale = UiScale::new(1.5).expect("valid scale");

        let css = ui_scale_css(&config);

        assert!(css.contains(".limux-ws-name { font-size: 22.500px; }"));
        assert!(css.contains(".limux-tab { font-size: 18.000px; }"));
        assert!(css.contains(".limux-pane-action image { -gtk-icon-size: 24.000px; }"));
    }
}
