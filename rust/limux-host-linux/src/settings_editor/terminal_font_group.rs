use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;

use super::{apply_config_change, SettingsEditorInput};
use crate::app_config::AppConfig;
use crate::ghostty_config::GhosttyFontDefaults;
use crate::terminal_font::{
    preferred_style_for_family_change, sanitize_font_token, sort_face_styles, TerminalFontConfig,
    MAX_CELL_METRIC_PERCENT, MAX_UI_TERMINAL_FONT_SIZE, MIN_CELL_METRIC_PERCENT,
    MIN_UI_TERMINAL_FONT_SIZE,
};

#[derive(Clone, Debug, Default)]
pub(super) struct FontCatalog {
    families: Vec<String>,
    styles_by_family: BTreeMap<String, Vec<String>>,
}

impl FontCatalog {
    fn discover(host_widget: &impl IsA<gtk::Widget>, initial: &TerminalFontConfig) -> Self {
        let mut by_lower: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
        if let Some(font_map) = host_widget.pango_context().font_map() {
            for family in font_map.list_families() {
                let Some(name) = sanitize_font_token(family.name().as_str()) else {
                    continue;
                };
                let face_names: Vec<String> = family
                    .list_faces()
                    .into_iter()
                    .map(|face| face.face_name().to_string())
                    .collect();
                let styles = sort_face_styles(face_names);
                let entry = by_lower
                    .entry(name.to_ascii_lowercase())
                    .or_insert_with(|| (name, Vec::new()));
                entry.1 = sort_face_styles(entry.1.iter().cloned().chain(styles));
            }
        }

        if let Some(configured_family) = initial.family.as_deref().and_then(sanitize_font_token) {
            let extra_style: Vec<String> = initial
                .style
                .as_deref()
                .and_then(sanitize_font_token)
                .into_iter()
                .collect();
            let entry = by_lower
                .entry(configured_family.to_ascii_lowercase())
                .or_insert_with(|| (configured_family, Vec::new()));
            entry.1 = sort_face_styles(entry.1.iter().cloned().chain(extra_style));
        }

        let mut families = Vec::with_capacity(by_lower.len());
        let mut styles_by_family = BTreeMap::new();
        for (_, (family, styles)) in by_lower {
            styles_by_family.insert(family.clone(), styles);
            families.push(family);
        }

        Self {
            families,
            styles_by_family,
        }
    }

    #[cfg(test)]
    pub(super) fn from_entries(entries: &[(&str, &[&str])]) -> Self {
        let mut families = Vec::new();
        let mut styles_by_family = BTreeMap::new();
        for (family, styles) in entries {
            if let Some(clean_family) = sanitize_font_token(family) {
                let clean_styles = sort_face_styles(styles.iter().copied());
                families.push(clean_family.clone());
                styles_by_family.insert(clean_family, clean_styles);
            }
        }
        families.sort_by_key(|f| f.to_ascii_lowercase());
        families.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        Self {
            families,
            styles_by_family,
        }
    }

    fn family_dropdown_index(&self, family: Option<&str>) -> u32 {
        let Some(target) = family else {
            return 0;
        };
        self.families
            .iter()
            .position(|candidate| candidate.eq_ignore_ascii_case(target))
            .map(|idx| (idx + 1) as u32)
            .unwrap_or(0)
    }

    fn family_for_dropdown_index(&self, index: u32) -> Option<String> {
        index
            .checked_sub(1)
            .and_then(|idx| self.families.get(idx as usize))
            .cloned()
    }

    fn styles_for(&self, family: &str) -> Vec<String> {
        self.styles_by_family
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(family))
            .map(|(_, styles)| styles.clone())
            .unwrap_or_default()
    }
}

struct TerminalFontRowWidgets {
    catalog: Rc<FontCatalog>,
    defaults: GhosttyFontDefaults,
    reset_button: gtk::glib::WeakRef<gtk::Button>,
    family_dropdown: gtk::glib::WeakRef<gtk::DropDown>,
    style_row: gtk::glib::WeakRef<adw::ActionRow>,
    style_dropdown: gtk::glib::WeakRef<gtk::DropDown>,
    style_model: gtk::glib::WeakRef<gtk::StringList>,
    size_spin: gtk::glib::WeakRef<gtk::SpinButton>,
    width_spin: gtk::glib::WeakRef<gtk::SpinButton>,
    height_spin: gtk::glib::WeakRef<gtk::SpinButton>,
    ligatures_switch: gtk::glib::WeakRef<gtk::Switch>,
    anti_aliased_switch: gtk::glib::WeakRef<gtk::Switch>,
}

impl TerminalFontRowWidgets {
    fn show(&self, config: &AppConfig) -> Option<()> {
        let font = &config.terminal_font;
        self.reset_button
            .upgrade()?
            .set_sensitive(!font.is_default() || config.font_size.is_some());

        let family_dropdown = self.family_dropdown.upgrade()?;
        let desired_family_idx = self.catalog.family_dropdown_index(font.family.as_deref());
        if family_dropdown.selected() != desired_family_idx {
            family_dropdown.set_selected(desired_family_idx);
        }

        let style_row = self.style_row.upgrade()?;
        let style_dropdown = self.style_dropdown.upgrade()?;
        let style_model = self.style_model.upgrade()?;
        style_row.set_sensitive(font.family.is_some());
        sync_style_dropdown(
            &self.catalog,
            &style_model,
            &style_dropdown,
            font.family.as_deref(),
            font.style.as_deref(),
        );

        let size_spin = self.size_spin.upgrade()?;
        let effective_size = f64::from(config.effective_font_size(self.defaults.size))
            .clamp(MIN_UI_TERMINAL_FONT_SIZE, MAX_UI_TERMINAL_FONT_SIZE);
        if (size_spin.value() - effective_size).abs() >= 0.05 {
            size_spin.set_value(effective_size);
        }

        let width_spin = self.width_spin.upgrade()?;
        let cell_width = font
            .cell_width_percent
            .unwrap_or(self.defaults.cell_width_percent);
        if width_spin.value_as_int() != i32::from(cell_width) {
            width_spin.set_value(f64::from(cell_width));
        }

        let height_spin = self.height_spin.upgrade()?;
        let cell_height = font
            .cell_height_percent
            .unwrap_or(self.defaults.cell_height_percent);
        if height_spin.value_as_int() != i32::from(cell_height) {
            height_spin.set_value(f64::from(cell_height));
        }

        let ligatures_switch = self.ligatures_switch.upgrade()?;
        let use_ligatures = font.use_ligatures.unwrap_or(self.defaults.use_ligatures);
        if ligatures_switch.is_active() != use_ligatures {
            ligatures_switch.set_active(use_ligatures);
        }

        let anti_aliased_switch = self.anti_aliased_switch.upgrade()?;
        let anti_aliased = font.anti_aliased.unwrap_or(self.defaults.anti_aliased);
        if anti_aliased_switch.is_active() != anti_aliased {
            anti_aliased_switch.set_active(anti_aliased);
        }

        Some(())
    }
}

fn sync_style_dropdown(
    catalog: &FontCatalog,
    style_model: &gtk::StringList,
    style_dropdown: &gtk::DropDown,
    family: Option<&str>,
    selected_style: Option<&str>,
) {
    let mut styles = family.map(|f| catalog.styles_for(f)).unwrap_or_default();
    if let Some(custom) = selected_style.and_then(sanitize_font_token) {
        if !styles.iter().any(|s| s.eq_ignore_ascii_case(&custom)) {
            styles.push(custom);
            styles = sort_face_styles(styles);
        }
    }

    let mut labels = Vec::with_capacity(1 + styles.len());
    labels.push("Default".to_string());
    labels.extend(styles.iter().cloned());

    let existing: Vec<String> = (0..style_model.n_items())
        .filter_map(|idx| style_model.string(idx).map(|s| s.to_string()))
        .collect();
    if existing != labels {
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        style_model.splice(0, style_model.n_items(), &label_refs);
    }

    let target_index = selected_style
        .and_then(|target| {
            styles
                .iter()
                .position(|candidate| candidate.eq_ignore_ascii_case(target))
                .map(|idx| (idx + 1) as u32)
        })
        .unwrap_or(0);
    if style_dropdown.selected() != target_index {
        style_dropdown.set_selected(target_index);
    }
}

thread_local! {
    static TERMINAL_FONT_ROWS: RefCell<Vec<TerminalFontRowWidgets>> = const { RefCell::new(Vec::new()) };
    static REFRESHING_TERMINAL_FONT_ROWS: Cell<bool> = const { Cell::new(false) };
}

struct TerminalFontRefreshGuard(bool);

impl TerminalFontRefreshGuard {
    fn new() -> Self {
        Self(REFRESHING_TERMINAL_FONT_ROWS.with(|flag| flag.replace(true)))
    }
}

impl Drop for TerminalFontRefreshGuard {
    fn drop(&mut self) {
        REFRESHING_TERMINAL_FONT_ROWS.with(|flag| flag.set(self.0));
    }
}

pub fn refresh_terminal_font_rows(config: &AppConfig) {
    let _guard = TerminalFontRefreshGuard::new();
    TERMINAL_FONT_ROWS.with(|rows| {
        let mut rows = rows.borrow_mut();
        rows.retain(|row| row.family_dropdown.upgrade().is_some());
        for row in rows.iter() {
            row.show(config);
        }
    });
}

pub(super) fn build_terminal_font_group(
    input: &SettingsEditorInput,
    host_widget: &impl IsA<gtk::Widget>,
) -> adw::PreferencesGroup {
    let catalog = Rc::new(FontCatalog::discover(
        host_widget,
        &input.config.borrow().terminal_font,
    ));
    let defaults = crate::terminal::base_ghostty_font_defaults();
    build_terminal_font_group_with_catalog(input, catalog, defaults)
}

pub(super) fn build_terminal_font_group_with_catalog(
    input: &SettingsEditorInput,
    catalog: Rc<FontCatalog>,
    defaults: GhosttyFontDefaults,
) -> adw::PreferencesGroup {
    let initial_snapshot = input.config.borrow().clone();
    let initial_font = &initial_snapshot.terminal_font;

    let group = adw::PreferencesGroup::new();
    group.set_title("Terminal font");
    group.set_description(Some(
        "Customize terminal typeface, size, cell spacing, ligatures, and smoothing",
    ));

    let reset_button = gtk::Button::builder()
        .label("Default")
        .tooltip_text("Reset terminal font settings to Ghostty defaults")
        .valign(gtk::Align::Center)
        .sensitive(!initial_font.is_default() || initial_snapshot.font_size.is_some())
        .build();

    let default_family_label = match &defaults.family {
        Some(family) => format!("Default ({family})"),
        None => "Default (Ghostty)".to_string(),
    };
    let mut family_labels = Vec::with_capacity(1 + catalog.families.len());
    family_labels.push(default_family_label);
    family_labels.extend(catalog.families.iter().cloned());
    let family_refs: Vec<&str> = family_labels.iter().map(String::as_str).collect();

    let family_row = adw::ActionRow::builder()
        .title("Font family")
        .subtitle("Terminal typeface; Default inherits Ghostty configuration")
        .build();
    family_row.set_title_lines(1);
    family_row.set_subtitle_lines(2);
    let family_dropdown = gtk::DropDown::from_strings(&family_refs);
    let family_expr = gtk::PropertyExpression::new(
        gtk::StringObject::static_type(),
        None::<gtk::Expression>,
        "string",
    );
    family_dropdown.set_expression(Some(&family_expr));
    family_dropdown.set_enable_search(true);
    family_dropdown.set_selected(catalog.family_dropdown_index(initial_font.family.as_deref()));
    family_dropdown.set_valign(gtk::Align::Center);
    family_row.add_suffix(&family_dropdown);
    family_row.add_suffix(&reset_button);
    family_row.set_activatable_widget(Some(&family_dropdown));
    group.add(&family_row);

    let style_row = adw::ActionRow::builder()
        .title("Font style")
        .subtitle("Named face style for regular terminal text, such as Regular or Book")
        .build();
    style_row.set_title_lines(1);
    style_row.set_subtitle_lines(2);
    let style_model = gtk::StringList::new(&["Default"]);
    let style_dropdown = gtk::DropDown::new(Some(style_model.clone()), None::<gtk::Expression>);
    style_dropdown.set_valign(gtk::Align::Center);
    sync_style_dropdown(
        &catalog,
        &style_model,
        &style_dropdown,
        initial_font.family.as_deref(),
        initial_font.style.as_deref(),
    );
    style_row.set_sensitive(initial_font.family.is_some());
    style_row.add_suffix(&style_dropdown);
    style_row.set_activatable_widget(Some(&style_dropdown));
    group.add(&style_row);

    let size_row = adw::ActionRow::builder()
        .title("Font size")
        .subtitle("Terminal text size in points")
        .build();
    size_row.set_title_lines(1);
    size_row.set_subtitle_lines(2);
    let initial_size = f64::from(initial_snapshot.effective_font_size(defaults.size))
        .clamp(MIN_UI_TERMINAL_FONT_SIZE, MAX_UI_TERMINAL_FONT_SIZE);
    let size_adjustment = gtk::Adjustment::new(
        initial_size,
        MIN_UI_TERMINAL_FONT_SIZE,
        MAX_UI_TERMINAL_FONT_SIZE,
        1.0,
        2.0,
        0.0,
    );
    let size_spin = gtk::SpinButton::builder()
        .adjustment(&size_adjustment)
        .digits(1)
        .numeric(true)
        .valign(gtk::Align::Center)
        .width_chars(4)
        .build();
    size_spin.connect_output(|spin| {
        let value = (spin.value() * 10.0).round() / 10.0;
        if (value - value.round()).abs() < f64::EPSILON {
            let text = format!("{:.0}", value.round());
            if spin.text() != text {
                spin.set_text(&text);
            }
            gtk::glib::Propagation::Stop
        } else {
            // Defer fractional values to GTK's default locale-aware formatter so
            // comma-decimal locales never reject an ASCII '.' under numeric(true).
            gtk::glib::Propagation::Proceed
        }
    });
    let rounded_initial = (initial_size * 10.0).round() / 10.0;
    if (rounded_initial - rounded_initial.round()).abs() < f64::EPSILON {
        let text = format!("{:.0}", rounded_initial.round());
        if size_spin.text() != text {
            size_spin.set_text(&text);
        }
    }
    size_row.add_suffix(&size_spin);
    size_row.set_activatable_widget(Some(&size_spin));
    group.add(&size_row);

    let width_row = adw::ActionRow::builder()
        .title("Horizontal spacing")
        .subtitle("Character cell width percentage (100% is normal)")
        .build();
    width_row.set_title_lines(1);
    width_row.set_subtitle_lines(2);
    let width_adjustment = gtk::Adjustment::new(
        f64::from(
            initial_font
                .cell_width_percent
                .unwrap_or(defaults.cell_width_percent),
        ),
        f64::from(MIN_CELL_METRIC_PERCENT),
        f64::from(MAX_CELL_METRIC_PERCENT),
        1.0,
        5.0,
        0.0,
    );
    let width_spin = gtk::SpinButton::builder()
        .adjustment(&width_adjustment)
        .digits(0)
        .numeric(true)
        .valign(gtk::Align::Center)
        .width_chars(4)
        .build();
    width_row.add_suffix(&width_spin);
    width_row.set_activatable_widget(Some(&width_spin));
    group.add(&width_row);

    let height_row = adw::ActionRow::builder()
        .title("Vertical spacing")
        .subtitle("Line height percentage (100% is normal)")
        .build();
    height_row.set_title_lines(1);
    height_row.set_subtitle_lines(2);
    let height_adjustment = gtk::Adjustment::new(
        f64::from(
            initial_font
                .cell_height_percent
                .unwrap_or(defaults.cell_height_percent),
        ),
        f64::from(MIN_CELL_METRIC_PERCENT),
        f64::from(MAX_CELL_METRIC_PERCENT),
        1.0,
        5.0,
        0.0,
    );
    let height_spin = gtk::SpinButton::builder()
        .adjustment(&height_adjustment)
        .digits(0)
        .numeric(true)
        .valign(gtk::Align::Center)
        .width_chars(4)
        .build();
    height_row.add_suffix(&height_spin);
    height_row.set_activatable_widget(Some(&height_spin));
    group.add(&height_row);

    let ligatures_row = adw::ActionRow::builder()
        .title("Use ligatures")
        .subtitle("Enable programming and typographic font ligatures")
        .build();
    ligatures_row.set_title_lines(1);
    ligatures_row.set_subtitle_lines(2);
    let ligatures_switch = gtk::Switch::new();
    ligatures_switch.set_active(initial_font.use_ligatures.unwrap_or(defaults.use_ligatures));
    ligatures_switch.set_valign(gtk::Align::Center);
    ligatures_row.add_suffix(&ligatures_switch);
    ligatures_row.set_activatable_widget(Some(&ligatures_switch));
    group.add(&ligatures_row);

    let anti_aliased_row = adw::ActionRow::builder()
        .title("Anti-aliased")
        .subtitle("Smooth glyph edges; turn off for 1-bit monochrome rendering")
        .build();
    anti_aliased_row.set_title_lines(1);
    anti_aliased_row.set_subtitle_lines(2);
    let anti_aliased_switch = gtk::Switch::new();
    anti_aliased_switch.set_active(initial_font.anti_aliased.unwrap_or(defaults.anti_aliased));
    anti_aliased_switch.set_valign(gtk::Align::Center);
    anti_aliased_row.add_suffix(&anti_aliased_switch);
    anti_aliased_row.set_activatable_widget(Some(&anti_aliased_switch));
    group.add(&anti_aliased_row);

    TERMINAL_FONT_ROWS.with(|rows| {
        let mut rows = rows.borrow_mut();
        rows.retain(|row| row.family_dropdown.upgrade().is_some());
        rows.push(TerminalFontRowWidgets {
            catalog: catalog.clone(),
            defaults: defaults.clone(),
            reset_button: reset_button.downgrade(),
            family_dropdown: family_dropdown.downgrade(),
            style_row: style_row.downgrade(),
            style_dropdown: style_dropdown.downgrade(),
            style_model: style_model.downgrade(),
            size_spin: size_spin.downgrade(),
            width_spin: width_spin.downgrade(),
            height_spin: height_spin.downgrade(),
            ligatures_switch: ligatures_switch.downgrade(),
            anti_aliased_switch: anti_aliased_switch.downgrade(),
        });
    });

    {
        let config = input.config.clone();
        let on_changed = input.on_config_changed.clone();
        let catalog = catalog.clone();
        family_dropdown.connect_selected_notify(move |dropdown| {
            if REFRESHING_TERMINAL_FONT_ROWS.with(Cell::get) {
                return;
            }
            let selected_family = catalog.family_for_dropdown_index(dropdown.selected());
            if selected_family == config.borrow().terminal_font.family {
                return;
            }
            let next_style = selected_family.as_deref().and_then(|family| {
                let styles = catalog.styles_for(family);
                preferred_style_for_family_change(
                    &styles,
                    config.borrow().terminal_font.style.as_deref(),
                )
            });
            let effective = apply_config_change(&config, &*on_changed, move |c| {
                c.terminal_font.family = selected_family;
                c.terminal_font.style = next_style;
            });
            refresh_terminal_font_rows(&effective);
        });
    }

    {
        let config = input.config.clone();
        let on_changed = input.on_config_changed.clone();
        let style_model = style_model.clone();
        style_dropdown.connect_selected_notify(move |dropdown| {
            if REFRESHING_TERMINAL_FONT_ROWS.with(Cell::get) {
                return;
            }
            if config.borrow().terminal_font.family.is_none() {
                return;
            }
            let selected_style = if dropdown.selected() == 0 {
                None
            } else {
                style_model
                    .string(dropdown.selected())
                    .and_then(|s| sanitize_font_token(s.as_str()))
            };
            if selected_style == config.borrow().terminal_font.style {
                return;
            }
            let effective = apply_config_change(&config, &*on_changed, move |c| {
                c.terminal_font.style = selected_style;
            });
            refresh_terminal_font_rows(&effective);
        });
    }

    {
        let config = input.config.clone();
        let on_changed = input.on_config_changed.clone();
        let default_size = defaults.size;
        size_spin.connect_value_changed(move |spin| {
            if REFRESHING_TERMINAL_FONT_ROWS.with(Cell::get) {
                return;
            }
            let size = ((spin.value() as f32) * 10.0).round() / 10.0;
            {
                let cfg = config.borrow();
                if cfg.terminal_font.size == Some(size) && cfg.font_size.is_none() {
                    return;
                }
                if cfg.terminal_font.size.is_none()
                    && cfg.font_size.is_none()
                    && (size - default_size).abs() < 0.05
                {
                    return;
                }
            }
            let effective = apply_config_change(&config, &*on_changed, move |c| {
                c.terminal_font.size = Some(size);
                c.font_size = None;
            });
            refresh_terminal_font_rows(&effective);
        });
    }

    {
        let config = input.config.clone();
        let on_changed = input.on_config_changed.clone();
        let default_width = defaults.cell_width_percent;
        let has_width_modifier = defaults.has_cell_width_modifier;
        width_spin.connect_value_changed(move |spin| {
            if REFRESHING_TERMINAL_FONT_ROWS.with(Cell::get) {
                return;
            }
            let pct = (spin.value().round() as i32).clamp(
                i32::from(MIN_CELL_METRIC_PERCENT),
                i32::from(MAX_CELL_METRIC_PERCENT),
            ) as u16;
            let next_value = if pct == default_width && !has_width_modifier {
                None
            } else {
                Some(pct)
            };
            {
                let cfg = config.borrow();
                if cfg.terminal_font.cell_width_percent == next_value
                    || (cfg.terminal_font.cell_width_percent.is_none() && pct == default_width)
                {
                    return;
                }
            }
            let effective = apply_config_change(&config, &*on_changed, move |c| {
                c.terminal_font.cell_width_percent = next_value;
            });
            refresh_terminal_font_rows(&effective);
        });
    }

    {
        let config = input.config.clone();
        let on_changed = input.on_config_changed.clone();
        let default_height = defaults.cell_height_percent;
        let has_height_modifier = defaults.has_cell_height_modifier;
        height_spin.connect_value_changed(move |spin| {
            if REFRESHING_TERMINAL_FONT_ROWS.with(Cell::get) {
                return;
            }
            let pct = (spin.value().round() as i32).clamp(
                i32::from(MIN_CELL_METRIC_PERCENT),
                i32::from(MAX_CELL_METRIC_PERCENT),
            ) as u16;
            let next_value = if pct == default_height && !has_height_modifier {
                None
            } else {
                Some(pct)
            };
            {
                let cfg = config.borrow();
                if cfg.terminal_font.cell_height_percent == next_value
                    || (cfg.terminal_font.cell_height_percent.is_none() && pct == default_height)
                {
                    return;
                }
            }
            let effective = apply_config_change(&config, &*on_changed, move |c| {
                c.terminal_font.cell_height_percent = next_value;
            });
            refresh_terminal_font_rows(&effective);
        });
    }

    {
        let config = input.config.clone();
        let on_changed = input.on_config_changed.clone();
        let default_ligatures = defaults.use_ligatures;
        ligatures_switch.connect_active_notify(move |switch| {
            if REFRESHING_TERMINAL_FONT_ROWS.with(Cell::get) {
                return;
            }
            let active = switch.is_active();
            {
                let cfg = config.borrow();
                if cfg.terminal_font.use_ligatures == Some(active)
                    || (cfg.terminal_font.use_ligatures.is_none() && active == default_ligatures)
                {
                    return;
                }
            }
            let effective = apply_config_change(&config, &*on_changed, move |c| {
                c.terminal_font.use_ligatures = Some(active);
            });
            refresh_terminal_font_rows(&effective);
        });
    }

    {
        let config = input.config.clone();
        let on_changed = input.on_config_changed.clone();
        let default_anti_aliased = defaults.anti_aliased;
        anti_aliased_switch.connect_active_notify(move |switch| {
            if REFRESHING_TERMINAL_FONT_ROWS.with(Cell::get) {
                return;
            }
            let active = switch.is_active();
            {
                let cfg = config.borrow();
                if cfg.terminal_font.anti_aliased == Some(active)
                    || (cfg.terminal_font.anti_aliased.is_none() && active == default_anti_aliased)
                {
                    return;
                }
            }
            let effective = apply_config_change(&config, &*on_changed, move |c| {
                c.terminal_font.anti_aliased = Some(active);
            });
            refresh_terminal_font_rows(&effective);
        });
    }

    {
        let config = input.config.clone();
        let on_changed = input.on_config_changed.clone();
        reset_button.connect_clicked(move |_| {
            if REFRESHING_TERMINAL_FONT_ROWS.with(Cell::get) {
                return;
            }
            {
                let cfg = config.borrow();
                if cfg.terminal_font.is_default() && cfg.font_size.is_none() {
                    return;
                }
            }
            let effective = apply_config_change(&config, &*on_changed, move |c| {
                c.terminal_font = TerminalFontConfig::default();
                c.font_size = None;
            });
            refresh_terminal_font_rows(&effective);
        });
    }

    group
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find_row(widget: &gtk::Widget, title: &str) -> Option<adw::ActionRow> {
        if let Some(row) = widget.downcast_ref::<adw::ActionRow>() {
            if row.title() == title {
                return Some(row.clone());
            }
        }
        let mut child = widget.first_child();
        while let Some(next) = child {
            if let Some(found) = find_row(&next, title) {
                return Some(found);
            }
            child = next.next_sibling();
        }
        None
    }

    fn find_button_by_label(widget: &gtk::Widget, label: &str) -> Option<gtk::Button> {
        if let Some(btn) = widget.downcast_ref::<gtk::Button>() {
            if btn.label().as_deref() == Some(label) {
                return Some(btn.clone());
            }
        }
        let mut child = widget.first_child();
        while let Some(next) = child {
            if let Some(found) = find_button_by_label(&next, label) {
                return Some(found);
            }
            child = next.next_sibling();
        }
        None
    }

    #[test]
    #[ignore = "requires GTK; exercised by xvfb-smoke-test.sh"]
    fn terminal_font_controls_update_auto_style_rollback_and_reset() {
        adw::init().expect("initialize GTK");
        let config = Rc::new(RefCell::new(AppConfig::default()));
        let reject_changes = Rc::new(Cell::new(false));
        let change_calls = Rc::new(Cell::new(0));

        let catalog = Rc::new(FontCatalog::from_entries(&[
            ("DejaVu Sans Mono", &["Book", "Bold", "Oblique"]),
            ("MesloLGS NF", &["Bold Italic", "Italic", "Bold", "Regular"]),
        ]));
        let defaults = GhosttyFontDefaults::default();
        let group = build_terminal_font_group_with_catalog(
            &SettingsEditorInput {
                config: config.clone(),
                shortcuts: Rc::new(crate::shortcut_config::default_shortcuts()),
                on_capture: Rc::new(|_, _| Ok(crate::shortcut_config::default_shortcuts())),
                on_config_changed: Rc::new({
                    let config = config.clone();
                    let reject_changes = reject_changes.clone();
                    let change_calls = change_calls.clone();
                    move |previous, _| {
                        change_calls.set(change_calls.get() + 1);
                        if reject_changes.get() {
                            config.borrow_mut().clone_from(previous);
                        }
                    }
                }),
            },
            catalog.clone(),
            defaults,
        );
        let root: gtk::Widget = group.upcast();

        let family_row = find_row(&root, "Font family").expect("family row");
        let family_dropdown = family_row
            .activatable_widget()
            .unwrap()
            .downcast::<gtk::DropDown>()
            .unwrap();
        let reset_button =
            find_button_by_label(family_row.upcast_ref(), "Default").expect("Default reset button");
        let style_row = find_row(&root, "Font style").expect("style row");
        let style_dropdown = style_row
            .activatable_widget()
            .unwrap()
            .downcast::<gtk::DropDown>()
            .unwrap();
        let size_spin = find_row(&root, "Font size")
            .unwrap()
            .activatable_widget()
            .unwrap()
            .downcast::<gtk::SpinButton>()
            .unwrap();
        let width_spin = find_row(&root, "Horizontal spacing")
            .unwrap()
            .activatable_widget()
            .unwrap()
            .downcast::<gtk::SpinButton>()
            .unwrap();
        let height_spin = find_row(&root, "Vertical spacing")
            .unwrap()
            .activatable_widget()
            .unwrap()
            .downcast::<gtk::SpinButton>()
            .unwrap();
        let ligatures_switch = find_row(&root, "Use ligatures")
            .unwrap()
            .activatable_widget()
            .unwrap()
            .downcast::<gtk::Switch>()
            .unwrap();
        let anti_aliased_switch = find_row(&root, "Anti-aliased")
            .unwrap()
            .activatable_widget()
            .unwrap()
            .downcast::<gtk::Switch>()
            .unwrap();

        assert!(!reset_button.is_sensitive());
        assert!(!style_row.is_sensitive());
        assert_eq!(family_dropdown.selected(), 0);
        assert_eq!(size_spin.text(), "12");

        // Index 2 corresponds to "MesloLGS NF" (0 = Default, 1 = DejaVu Sans Mono, 2 = MesloLGS NF)
        family_dropdown.set_selected(2);
        assert!(reset_button.is_sensitive());
        assert!(style_row.is_sensitive());
        assert_eq!(
            config.borrow().terminal_font.family.as_deref(),
            Some("MesloLGS NF")
        );
        assert_eq!(
            config.borrow().terminal_font.style.as_deref(),
            Some("Regular")
        );
        assert_eq!(style_dropdown.selected(), 1);

        size_spin.set_value(13.5);
        assert!(
            matches!(size_spin.text().as_str(), "13.5" | "13,5"),
            "unexpected fractional font size text: {:?}",
            size_spin.text()
        );
        size_spin.update();
        assert!((size_spin.value() - 13.5).abs() < 0.05);
        assert_eq!(config.borrow().terminal_font.size, Some(13.5));
        size_spin.set_value(13.0);
        assert_eq!(size_spin.text(), "13");
        assert_eq!(config.borrow().terminal_font.size, Some(13.0));

        width_spin.set_value(110.0);
        height_spin.set_value(95.0);
        ligatures_switch.set_active(false);
        anti_aliased_switch.set_active(false);
        assert_eq!(config.borrow().terminal_font.cell_width_percent, Some(110));
        assert_eq!(config.borrow().terminal_font.cell_height_percent, Some(95));
        assert_eq!(config.borrow().terminal_font.use_ligatures, Some(false));
        assert_eq!(config.borrow().terminal_font.anti_aliased, Some(false));

        let saved_snapshot = config.borrow().clone();
        reject_changes.set(true);
        let calls_before = change_calls.get();

        family_dropdown.set_selected(1);
        assert_eq!(family_dropdown.selected(), 2);
        style_dropdown.set_selected(2);
        assert_eq!(style_dropdown.selected(), 1);
        size_spin.set_value(18.0);
        assert_eq!(size_spin.text(), "13");
        width_spin.set_value(130.0);
        assert_eq!(width_spin.value_as_int(), 110);
        height_spin.set_value(125.0);
        assert_eq!(height_spin.value_as_int(), 95);
        ligatures_switch.set_active(true);
        assert!(!ligatures_switch.is_active());
        anti_aliased_switch.set_active(true);
        assert!(!anti_aliased_switch.is_active());
        reset_button.emit_clicked();
        assert!(reset_button.is_sensitive());
        assert_eq!(*config.borrow(), saved_snapshot);
        assert_eq!(
            change_calls.get() - calls_before,
            8,
            "rollback refreshes must not emit recursive saves"
        );

        reject_changes.set(false);
        reset_button.emit_clicked();
        assert!(config.borrow().terminal_font.is_default());
        assert!(!reset_button.is_sensitive());
        assert!(!style_row.is_sensitive());
        assert_eq!(family_dropdown.selected(), 0);
        assert_eq!(size_spin.text(), "12");
        assert_eq!(width_spin.value_as_int(), 100);
        assert_eq!(height_spin.value_as_int(), 100);
        assert!(ligatures_switch.is_active());
        assert!(anti_aliased_switch.is_active());

        // Drop the first group's widgets so the next build must prune its dead row entry.
        drop((
            family_row,
            family_dropdown,
            reset_button,
            style_row,
            style_dropdown,
            size_spin,
            width_spin,
            height_spin,
            ligatures_switch,
            anti_aliased_switch,
            root,
        ));

        // Explicit 100% override when Ghostty base config sets adjust-cell-width = 20%.
        let spaced_defaults = GhosttyFontDefaults {
            cell_width_percent: 120,
            has_cell_width_modifier: true,
            ..GhosttyFontDefaults::default()
        };
        let spaced_config = Rc::new(RefCell::new(AppConfig::default()));
        let spaced_group = build_terminal_font_group_with_catalog(
            &SettingsEditorInput {
                config: spaced_config.clone(),
                shortcuts: Rc::new(crate::shortcut_config::default_shortcuts()),
                on_capture: Rc::new(|_, _| Ok(crate::shortcut_config::default_shortcuts())),
                on_config_changed: Rc::new(|_, _| {}),
            },
            catalog.clone(),
            spaced_defaults,
        );
        TERMINAL_FONT_ROWS.with(|rows| {
            assert_eq!(
                rows.borrow().len(),
                1,
                "dead widget entries must be pruned when building a new settings group"
            );
        });
        let spaced_root: gtk::Widget = spaced_group.upcast();
        let spaced_width_spin = find_row(&spaced_root, "Horizontal spacing")
            .unwrap()
            .activatable_widget()
            .unwrap()
            .downcast::<gtk::SpinButton>()
            .unwrap();
        let spaced_reset = find_button_by_label(
            find_row(&spaced_root, "Font family").unwrap().upcast_ref(),
            "Default",
        )
        .unwrap();
        assert_eq!(spaced_width_spin.value_as_int(), 120);
        spaced_width_spin.set_value(100.0);
        assert_eq!(
            spaced_config.borrow().terminal_font.cell_width_percent,
            Some(100)
        );
        assert!(spaced_reset.is_sensitive());
        spaced_reset.emit_clicked();
        assert_eq!(
            spaced_config.borrow().terminal_font.cell_width_percent,
            None
        );
        assert_eq!(spaced_width_spin.value_as_int(), 120);
        drop((spaced_width_spin, spaced_reset, spaced_root));

        // Verify fractional font sizes stay valid and never blank/collapse to 4pt
        // under comma-decimal locales (e.g. en_DK.utf8, de_DE.UTF-8, pt_BR.UTF-8).
        struct NumericLocaleGuard(std::ffi::CString);
        impl NumericLocaleGuard {
            fn activate_comma_locale() -> Option<Self> {
                unsafe {
                    let prev_ptr = libc::setlocale(libc::LC_NUMERIC, std::ptr::null());
                    if prev_ptr.is_null() {
                        return None;
                    }
                    let prev = std::ffi::CStr::from_ptr(prev_ptr).to_owned();
                    for candidate in [
                        c"en_DK.utf8",
                        c"en_DK.UTF-8",
                        c"de_DE.utf8",
                        c"de_DE.UTF-8",
                        c"fr_FR.utf8",
                        c"fr_FR.UTF-8",
                        c"pt_BR.utf8",
                        c"pt_BR.UTF-8",
                    ] {
                        if !libc::setlocale(libc::LC_NUMERIC, candidate.as_ptr()).is_null() {
                            return Some(Self(prev));
                        }
                    }
                    None
                }
            }
        }
        impl Drop for NumericLocaleGuard {
            fn drop(&mut self) {
                unsafe {
                    libc::setlocale(libc::LC_NUMERIC, self.0.as_ptr());
                }
            }
        }

        if let Some(_locale_guard) = NumericLocaleGuard::activate_comma_locale() {
            let mut initial_cfg = AppConfig::default();
            initial_cfg.terminal_font.size = Some(13.5);
            let comma_config = Rc::new(RefCell::new(initial_cfg));
            let comma_group = build_terminal_font_group_with_catalog(
                &SettingsEditorInput {
                    config: comma_config.clone(),
                    shortcuts: Rc::new(crate::shortcut_config::default_shortcuts()),
                    on_capture: Rc::new(|_, _| Ok(crate::shortcut_config::default_shortcuts())),
                    on_config_changed: Rc::new(|_, _| {}),
                },
                catalog,
                GhosttyFontDefaults::default(),
            );
            let comma_root: gtk::Widget = comma_group.upcast();
            let comma_size_spin = find_row(&comma_root, "Font size")
                .unwrap()
                .activatable_widget()
                .unwrap()
                .downcast::<gtk::SpinButton>()
                .unwrap();

            assert_eq!(comma_size_spin.text(), "13,5");
            comma_size_spin.update();
            assert!((comma_size_spin.value() - 13.5).abs() < 0.05);
            assert_eq!(comma_config.borrow().terminal_font.size, Some(13.5));

            comma_size_spin.set_value(14.5);
            assert_eq!(comma_size_spin.text(), "14,5");
            comma_size_spin.update();
            assert!((comma_size_spin.value() - 14.5).abs() < 0.05);
            assert_eq!(comma_config.borrow().terminal_font.size, Some(14.5));

            comma_size_spin.set_value(14.0);
            assert_eq!(comma_size_spin.text(), "14");
            comma_size_spin.update();
            assert!((comma_size_spin.value() - 14.0).abs() < 0.05);
            assert_eq!(comma_config.borrow().terminal_font.size, Some(14.0));
        }
    }
}
