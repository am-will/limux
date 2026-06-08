use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;

use crate::app_config::{AppConfig, ColorScheme, NotificationSound};
use crate::keybind_editor;
use crate::shortcut_config::{NormalizedShortcut, ResolvedShortcutConfig, ShortcutId};

pub const SETTINGS_CSS: &str = r#"
.limux-settings-window {
    background-color: @window_bg_color;
    color: @window_fg_color;
}
"#;

const MIN_FONT_SIZE: f64 = 8.0;
const MAX_FONT_SIZE: f64 = 255.0;
const FONT_SIZE_STEP: f64 = 1.0;

#[derive(Clone, Copy, Debug)]
pub struct UiFontDescriptor {
    pub id: &'static str,
    pub label: &'static str,
    pub subtitle: &'static str,
    pub selector: &'static str,
    pub default_size: f32,
}

impl UiFontDescriptor {
    fn css_property(self) -> &'static str {
        match self.id {
            "pane_action_icon" | "pane_tab_close_icon" => "-gtk-icon-size",
            _ => "font-size",
        }
    }
}

pub const UI_FONT_DESCRIPTORS: &[UiFontDescriptor] = &[
    UiFontDescriptor {
        id: "sidebar_workspace_name",
        label: "Sidebar workspace name",
        subtitle: "Workspace names in the left sidebar",
        selector: ".limux-ws-name",
        default_size: 12.5,
    },
    UiFontDescriptor {
        id: "sidebar_favorite_star",
        label: "Sidebar favorite star",
        subtitle: "Pinned workspace star icon in sidebar rows",
        selector: ".limux-ws-star-btn",
        default_size: 9.0,
    },
    UiFontDescriptor {
        id: "sidebar_notification_dot",
        label: "Sidebar notification dot",
        subtitle: "Unread notification marker in workspace rows",
        selector: ".limux-notify-dot, .limux-notify-dot-hidden",
        default_size: 9.0,
    },
    UiFontDescriptor {
        id: "sidebar_notification_message",
        label: "Sidebar notification message",
        subtitle: "Notification preview text below workspace names",
        selector: ".limux-notify-msg, .limux-notify-msg-unread",
        default_size: 10.0,
    },
    UiFontDescriptor {
        id: "sidebar_section_title",
        label: "Sidebar section title",
        subtitle: "The WORKSPACES heading above the sidebar list",
        selector: ".limux-sidebar-title",
        default_size: 11.0,
    },
    UiFontDescriptor {
        id: "sidebar_workspace_path",
        label: "Sidebar workspace path",
        subtitle: "Folder path text below workspace names",
        selector: ".limux-ws-path",
        default_size: 10.0,
    },
    UiFontDescriptor {
        id: "sidebar_git_branch",
        label: "Sidebar git branch",
        subtitle: "Git branch pill in workspace rows",
        selector: ".limux-ws-branch",
        default_size: 10.0,
    },
    UiFontDescriptor {
        id: "sidebar_ports",
        label: "Sidebar ports",
        subtitle: "Localhost port pill in workspace rows",
        selector: ".limux-ws-ports",
        default_size: 10.0,
    },
    UiFontDescriptor {
        id: "pane_tab_title",
        label: "Pane tab title",
        subtitle: "Terminal and browser tab labels in pane headers",
        selector: ".limux-tab",
        default_size: 11.0,
    },
    UiFontDescriptor {
        id: "pane_tab_status_icon",
        label: "Pane tab status icon",
        subtitle: "Attention and finished marker shown inside pane tabs",
        selector: ".limux-tab-status",
        default_size: 8.0,
    },
    UiFontDescriptor {
        id: "pane_pin_icon",
        label: "Pane pinned-tab icon",
        subtitle: "Pin indicator shown inside pane tabs",
        selector: ".limux-pin-icon",
        default_size: 9.0,
    },
    UiFontDescriptor {
        id: "pane_tab_rename_entry",
        label: "Pane tab rename entry",
        subtitle: "Inline text field used while renaming a tab",
        selector: ".limux-tab-rename-entry",
        default_size: 11.0,
    },
    UiFontDescriptor {
        id: "pane_action_icon",
        label: "Pane action icons",
        subtitle: "New tab, split, settings, close, and browser navigation icons in pane headers",
        selector: ".limux-pane-action image",
        default_size: 12.0,
    },
    UiFontDescriptor {
        id: "pane_tab_close_icon",
        label: "Pane tab close icon",
        subtitle: "Close button icon inside each pane tab",
        selector: ".limux-tab-close image",
        default_size: 12.0,
    },
    UiFontDescriptor {
        id: "notification_panel_title",
        label: "Notification panel title",
        subtitle: "Header text in the notification panel",
        selector: ".limux-notification-panel-title",
        default_size: 18.0,
    },
    UiFontDescriptor {
        id: "notification_panel_empty",
        label: "Notification empty state",
        subtitle: "Empty-state text in the notification panel",
        selector: ".limux-notification-empty",
        default_size: 12.0,
    },
    UiFontDescriptor {
        id: "notification_panel_status",
        label: "Notification status dot",
        subtitle: "Attention and finished status marker in notification rows",
        selector: ".limux-notification-status",
        default_size: 8.0,
    },
    UiFontDescriptor {
        id: "notification_panel_workspace",
        label: "Notification workspace",
        subtitle: "Workspace label in notification rows",
        selector: ".limux-notification-workspace",
        default_size: 10.0,
    },
    UiFontDescriptor {
        id: "notification_panel_message",
        label: "Notification message",
        subtitle: "Primary message text in notification rows",
        selector: ".limux-notification-message",
        default_size: 13.0,
    },
    UiFontDescriptor {
        id: "notification_panel_detail",
        label: "Notification detail",
        subtitle: "Secondary detail text in notification rows",
        selector: ".limux-notification-detail",
        default_size: 11.0,
    },
    UiFontDescriptor {
        id: "browser_url_entry",
        label: "Browser URL entry",
        subtitle: "Address field in browser panes",
        selector: ".limux-browser-url-entry",
        default_size: 12.0,
    },
    UiFontDescriptor {
        id: "browser_search_entry",
        label: "Browser search entry",
        subtitle: "Find-in-page entry in browser panes",
        selector: ".limux-browser-search-entry",
        default_size: 12.0,
    },
    UiFontDescriptor {
        id: "keybind_hint",
        label: "Keybinding editor hint",
        subtitle: "Explanatory hint text in the keybinding editor",
        selector: ".limux-keybind-hint",
        default_size: 12.0,
    },
    UiFontDescriptor {
        id: "keybind_default",
        label: "Keybinding default label",
        subtitle: "Default binding text in keybinding rows",
        selector: ".limux-keybind-default",
        default_size: 12.0,
    },
    UiFontDescriptor {
        id: "keybind_error",
        label: "Keybinding error text",
        subtitle: "Validation error text in the keybinding editor",
        selector: ".limux-keybind-error",
        default_size: 12.0,
    },
    UiFontDescriptor {
        id: "keybind_row_hint",
        label: "Keybinding row hint",
        subtitle: "Per-row helper text in the keybinding editor",
        selector: ".limux-keybind-row-hint",
        default_size: 12.0,
    },
    UiFontDescriptor {
        id: "toast",
        label: "Toast message",
        subtitle: "Small in-terminal Limux toast notifications",
        selector: ".limux-toast",
        default_size: 12.0,
    },
];

pub fn ui_font_sizes_css(config: &AppConfig) -> String {
    let mut css = String::new();
    for descriptor in UI_FONT_DESCRIPTORS {
        let size = config
            .ui_font_sizes
            .get(descriptor.id)
            .copied()
            .unwrap_or(descriptor.default_size)
            .clamp(MIN_FONT_SIZE as f32, MAX_FONT_SIZE as f32);
        css.push_str(&format!(
            "{} {{ {}: {size}px; }}\n",
            descriptor.selector,
            descriptor.css_property()
        ));
    }
    css
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

fn apply_config_change<F, G>(config: &Rc<RefCell<AppConfig>>, on_changed: &F, update: G)
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
}

fn build_settings_window_content(window: &adw::Window, input: SettingsEditorInput) -> gtk::Widget {
    let stack = adw::ViewStack::new();
    stack.set_hexpand(true);
    stack.set_vexpand(true);

    let general_page = build_general_page(&input);
    let general_stack_page = stack.add_titled(&general_page, Some("general"), "General");
    general_stack_page.set_icon_name(Some("preferences-system-symbolic"));

    let fonts_page = build_fonts_page(&input);
    let fonts_stack_page = stack.add_titled(&fonts_page, Some("fonts"), "Fonts & Icons");
    fonts_stack_page.set_icon_name(Some("preferences-desktop-font-symbolic"));

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

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&page)
        .build();
    scroller.set_hexpand(true);
    scroller.set_vexpand(true);

    scroller.upcast()
}

fn build_fonts_page(input: &SettingsEditorInput) -> gtk::Widget {
    let page = adw::PreferencesPage::new();
    page.set_title("Fonts & Icons");
    page.set_name(Some("fonts"));
    page.set_icon_name(Some("preferences-desktop-font-symbolic"));
    page.set_hexpand(true);
    page.set_vexpand(true);

    let terminal_group = adw::PreferencesGroup::new();
    terminal_group.set_title("Terminal font size");

    let terminal_row = adw::ActionRow::builder()
        .title("Terminal text")
        .subtitle("Default font size for terminal surfaces")
        .build();
    terminal_row.set_title_lines(1);
    terminal_row.set_subtitle_lines(2);

    let terminal_default_size = crate::terminal::default_font_size();
    let terminal_current_size = input
        .config
        .borrow()
        .font_size
        .unwrap_or(terminal_default_size);
    let terminal_adjustment = gtk::Adjustment::new(
        f64::from(terminal_current_size),
        MIN_FONT_SIZE,
        MAX_FONT_SIZE,
        FONT_SIZE_STEP,
        FONT_SIZE_STEP * 2.0,
        0.0,
    );
    let terminal_spin = gtk::SpinButton::builder()
        .adjustment(&terminal_adjustment)
        .digits(1)
        .numeric(true)
        .valign(gtk::Align::Center)
        .width_chars(5)
        .build();
    let terminal_reset_button = gtk::Button::builder()
        .label("Default")
        .tooltip_text("Reset terminal font size")
        .valign(gtk::Align::Center)
        .build();

    terminal_row.add_suffix(&terminal_spin);
    terminal_row.add_suffix(&terminal_reset_button);
    terminal_row.set_activatable_widget(Some(&terminal_spin));
    terminal_group.add(&terminal_row);
    page.add(&terminal_group);

    {
        let config = input.config.clone();
        let on_changed = input.on_config_changed.clone();
        let updating_spin = Rc::new(Cell::new(false));
        let updating_spin_for_reset = updating_spin.clone();
        terminal_spin.connect_value_changed(move |spin| {
            if updating_spin.get() {
                return;
            }
            let font_size = (spin.value() as f32).clamp(MIN_FONT_SIZE as f32, MAX_FONT_SIZE as f32);
            apply_config_change(&config, &*on_changed, move |c| {
                c.font_size = Some(font_size);
            });
        });

        let config = input.config.clone();
        let on_changed = input.on_config_changed.clone();
        let terminal_spin = terminal_spin.clone();
        terminal_reset_button.connect_clicked(move |_| {
            apply_config_change(&config, &*on_changed, move |c| {
                c.font_size = None;
            });
            updating_spin_for_reset.set(true);
            terminal_spin.set_value(f64::from(terminal_default_size));
            updating_spin_for_reset.set(false);
        });
    }

    let group = adw::PreferencesGroup::new();
    group.set_title("UI text and icon sizes");
    group.set_description(Some("Adjust Limux chrome text and pane-header icon sizes."));

    for descriptor in UI_FONT_DESCRIPTORS {
        let row = adw::ActionRow::builder()
            .title(descriptor.label)
            .subtitle(descriptor.subtitle)
            .build();
        row.set_title_lines(1);
        row.set_subtitle_lines(2);

        let current_size = input
            .config
            .borrow()
            .ui_font_sizes
            .get(descriptor.id)
            .copied()
            .unwrap_or(descriptor.default_size);
        let adjustment = gtk::Adjustment::new(
            f64::from(current_size),
            MIN_FONT_SIZE,
            MAX_FONT_SIZE,
            FONT_SIZE_STEP,
            FONT_SIZE_STEP * 2.0,
            0.0,
        );
        let spin = gtk::SpinButton::builder()
            .adjustment(&adjustment)
            .digits(1)
            .numeric(true)
            .valign(gtk::Align::Center)
            .width_chars(5)
            .build();
        let reset_button = gtk::Button::builder()
            .label("Default")
            .tooltip_text("Reset this UI font size")
            .valign(gtk::Align::Center)
            .build();

        row.add_suffix(&spin);
        row.add_suffix(&reset_button);
        row.set_activatable_widget(Some(&spin));
        group.add(&row);

        let config = input.config.clone();
        let on_changed = input.on_config_changed.clone();
        let id = descriptor.id;
        let updating_spin = Rc::new(Cell::new(false));
        let updating_spin_for_reset = updating_spin.clone();
        spin.connect_value_changed(move |spin| {
            if updating_spin.get() {
                return;
            }
            let font_size = (spin.value() as f32).clamp(MIN_FONT_SIZE as f32, MAX_FONT_SIZE as f32);
            apply_config_change(&config, &*on_changed, move |c| {
                c.ui_font_sizes.insert(id.to_string(), font_size);
            });
        });

        let config = input.config.clone();
        let on_changed = input.on_config_changed.clone();
        let spin = spin.clone();
        let default_size = descriptor.default_size;
        reset_button.connect_clicked(move |_| {
            apply_config_change(&config, &*on_changed, move |c| {
                c.ui_font_sizes.remove(id);
            });
            updating_spin_for_reset.set(true);
            spin.set_value(f64::from(default_size));
            updating_spin_for_reset.set(false);
        });
    }

    page.add(&group);

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
    fn apply_config_change_allows_reentrant_config_sync() {
        let config = Rc::new(RefCell::new(AppConfig::default()));

        apply_config_change(
            &config,
            &|_previous, updated| {
                config.borrow_mut().clone_from(updated);
            },
            |current| {
                current.focus.hover_terminal_focus = true;
            },
        );

        assert!(config.borrow().focus.hover_terminal_focus);
    }

    #[test]
    fn ui_font_css_covers_pane_tab_text_and_icons() {
        let mut config = AppConfig::default();
        config
            .ui_font_sizes
            .insert("pane_tab_title".to_string(), 24.0);
        config
            .ui_font_sizes
            .insert("pane_pin_icon".to_string(), 20.0);
        config
            .ui_font_sizes
            .insert("pane_action_icon".to_string(), 18.0);
        config
            .ui_font_sizes
            .insert("pane_tab_close_icon".to_string(), 11.0);

        let css = ui_font_sizes_css(&config);

        assert!(css.contains(".limux-tab { font-size: 24px; }"));
        assert!(css.contains(".limux-pin-icon { font-size: 20px; }"));
        assert!(css.contains(".limux-tab-status { font-size: 8px; }"));
        assert!(css.contains(".limux-tab-rename-entry { font-size: 11px; }"));
        assert!(css.contains(".limux-pane-action image { -gtk-icon-size: 18px; }"));
        assert!(css.contains(".limux-tab-close image { -gtk-icon-size: 11px; }"));
        assert!(UI_FONT_DESCRIPTORS
            .iter()
            .any(|descriptor| descriptor.id == "pane_tab_title"));
        assert!(UI_FONT_DESCRIPTORS
            .iter()
            .any(|descriptor| descriptor.id == "pane_pin_icon"));
    }

    #[test]
    fn ui_font_descriptors_cover_sidebar_notification_text() {
        let descriptor_ids = UI_FONT_DESCRIPTORS
            .iter()
            .map(|descriptor| descriptor.id)
            .collect::<Vec<_>>();

        for expected in [
            "sidebar_workspace_name",
            "sidebar_workspace_path",
            "sidebar_git_branch",
            "sidebar_ports",
            "pane_tab_title",
            "pane_tab_status_icon",
            "pane_pin_icon",
            "pane_tab_rename_entry",
            "pane_action_icon",
            "pane_tab_close_icon",
            "sidebar_notification_message",
            "notification_panel_title",
            "notification_panel_empty",
            "notification_panel_status",
            "notification_panel_workspace",
            "notification_panel_message",
            "notification_panel_detail",
        ] {
            assert!(
                descriptor_ids.contains(&expected),
                "missing UI font descriptor {expected}"
            );
        }
    }
}
