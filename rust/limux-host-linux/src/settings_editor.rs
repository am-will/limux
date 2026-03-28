use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;

use crate::app_config::{AppConfig, ColorScheme};
use crate::keybind_editor;
use crate::shortcut_config::{NormalizedShortcut, ResolvedShortcutConfig, ShortcutId};

pub const SETTINGS_CSS: &str = r#"
.limux-settings-window {
    background-color: @window_bg_color;
    color: @window_fg_color;
}
"#;

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
}
