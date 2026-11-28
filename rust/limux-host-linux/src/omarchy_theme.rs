use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::FileExt;
use gtk4::prelude::FileMonitorExt;

use crate::app_config::UiTheme;

const OMARCHY_RELOAD_DEBOUNCE_MS: u64 = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Parse `#RRGGBB`, `#RGB`, or `#RRGGBBAA` (ignoring alpha) into an `Rgb`.
    pub fn parse_hex(raw: &str) -> Option<Self> {
        let hex = raw.trim().strip_prefix('#')?;
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        match hex.len() {
            3 => {
                let r = u8::from_str_radix(&hex[0..1], 16).ok()?;
                let g = u8::from_str_radix(&hex[1..2], 16).ok()?;
                let b = u8::from_str_radix(&hex[2..3], 16).ok()?;
                Some(Self::new(r * 17, g * 17, b * 17))
            }
            6 | 8 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some(Self::new(r, g, b))
            }
            _ => None,
        }
    }

    pub fn mix(self, other: Self, ratio: f32) -> Self {
        let t = ratio.clamp(0.0, 1.0);
        let inv = 1.0 - t;
        Self::new(
            (f32::from(self.r) * inv + f32::from(other.r) * t).round() as u8,
            (f32::from(self.g) * inv + f32::from(other.g) * t).round() as u8,
            (f32::from(self.b) * inv + f32::from(other.b) * t).round() as u8,
        )
    }

    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    pub fn to_rgba(self, alpha: f32) -> String {
        format!(
            "rgba({}, {}, {}, {:.2})",
            self.r,
            self.g,
            self.b,
            alpha.clamp(0.0, 1.0)
        )
    }

    /// Relative luminance approximation (Rec. 709) in `[0.0, 1.0]`.
    pub fn luminance(self) -> f32 {
        (0.2126 * f32::from(self.r) + 0.7152 * f32::from(self.g) + 0.0722 * f32::from(self.b))
            / 255.0
    }

    pub fn is_dark(self) -> bool {
        self.luminance() < 0.5
    }

    pub fn contrast_fg(self) -> Self {
        if self.is_dark() {
            Self::new(0xff, 0xff, 0xff)
        } else {
            Self::new(0x11, 0x11, 0x11)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OmarchyPalette {
    pub is_dark: bool,
    pub background: Rgb,
    pub dark_background: Option<Rgb>,
    pub darker_background: Option<Rgb>,
    pub lighter_background: Option<Rgb>,
    pub foreground: Rgb,
    pub dark_foreground: Option<Rgb>,
    pub bright_foreground: Option<Rgb>,
    pub muted: Option<Rgb>,
    pub selection: Option<Rgb>,
    pub accent: Rgb,
    pub accent_fg: Rgb,
    pub unread: Rgb,
    pub error: Rgb,
}

#[cfg(test)]
fn omarchy_colors_path_in(state_dir: Option<&Path>, config_dir: Option<&Path>) -> Option<PathBuf> {
    omarchy_theme_file_in(state_dir, config_dir, "colors.toml")
}

fn omarchy_theme_file_in(
    state_dir: Option<&Path>,
    config_dir: Option<&Path>,
    file_name: &str,
) -> Option<PathBuf> {
    let rel = format!("omarchy/current/theme/{file_name}");
    let state_candidate = state_dir.map(|d| d.join(&rel));
    if let Some(path) = state_candidate.as_ref().filter(|p| p.is_file()) {
        return Some(path.clone());
    }
    let config_candidate = config_dir.map(|d| d.join(&rel));
    if let Some(path) = config_candidate.as_ref().filter(|p| p.is_file()) {
        return Some(path.clone());
    }
    state_candidate.or(config_candidate)
}

fn omarchy_dir_exists_in(state_dir: Option<&Path>, config_dir: Option<&Path>) -> bool {
    [state_dir, config_dir]
        .into_iter()
        .flatten()
        .any(|base| base.join("omarchy").is_dir())
}

/// Returns whether the Omarchy UI and terminal theme should be active for `ui_theme`.
///
/// - `UiTheme::Omarchy` always enables the Omarchy theme (falling back to structural CSS if absent).
/// - `UiTheme::Default` always uses Limux's stock Adwaita appearance.
/// - `UiTheme::Auto` activates Omarchy automatically when a valid Omarchy `colors.toml` exists.
pub fn is_omarchy_active(ui_theme: UiTheme) -> bool {
    is_omarchy_active_in(
        ui_theme,
        dirs::state_dir().as_deref(),
        dirs::config_dir().as_deref(),
    )
}

fn is_omarchy_active_in(
    ui_theme: UiTheme,
    state_dir: Option<&Path>,
    config_dir: Option<&Path>,
) -> bool {
    match ui_theme {
        UiTheme::Omarchy => true,
        UiTheme::Default => false,
        UiTheme::Auto => load_active_palette_in(state_dir, config_dir).is_some(),
    }
}

/// Returns whether the Omarchy live-reload directory watcher should run.
///
/// In `UiTheme::Auto`, only attaches file monitors when an `omarchy` directory exists
/// under XDG state or config dirs so non-Omarchy systems incur zero background watches.
pub fn should_watch_omarchy(ui_theme: UiTheme) -> bool {
    should_watch_omarchy_in(
        ui_theme,
        dirs::state_dir().as_deref(),
        dirs::config_dir().as_deref(),
    )
}

fn should_watch_omarchy_in(
    ui_theme: UiTheme,
    state_dir: Option<&Path>,
    config_dir: Option<&Path>,
) -> bool {
    match ui_theme {
        UiTheme::Omarchy => true,
        UiTheme::Default => false,
        UiTheme::Auto => omarchy_dir_exists_in(state_dir, config_dir),
    }
}

/// Return watchable Omarchy directories, falling back to existing parent
/// directories so live reload still activates if Omarchy creates its theme
/// directories after Limux starts.
pub fn omarchy_watch_dirs() -> Vec<PathBuf> {
    omarchy_watch_dirs_in(dirs::state_dir().as_deref(), dirs::config_dir().as_deref())
}

fn omarchy_watch_dirs_in(state_dir: Option<&Path>, config_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs_out = Vec::new();
    for base in [state_dir, config_dir].into_iter().flatten() {
        let current_dir = base.join("omarchy/current");
        let theme_dir = base.join("omarchy/current/theme");
        let omarchy_dir = base.join("omarchy");

        if current_dir.is_dir() {
            push_unique_dir(&mut dirs_out, current_dir);
            if theme_dir.is_dir() {
                push_unique_dir(&mut dirs_out, theme_dir);
            }
        } else if omarchy_dir.is_dir() {
            push_unique_dir(&mut dirs_out, omarchy_dir);
        } else if base.is_dir() {
            push_unique_dir(&mut dirs_out, base.to_path_buf());
        }
    }
    dirs_out
}

fn push_unique_dir(out: &mut Vec<PathBuf>, dir: PathBuf) {
    if !out.contains(&dir) {
        out.push(dir);
    }
}

/// Load and parse the active Omarchy theme palette from disk, if present.
pub fn load_active_palette() -> Option<OmarchyPalette> {
    load_active_palette_in(dirs::state_dir().as_deref(), dirs::config_dir().as_deref())
}

fn load_active_palette_in(
    state_dir: Option<&Path>,
    config_dir: Option<&Path>,
) -> Option<OmarchyPalette> {
    let path = omarchy_theme_file_in(state_dir, config_dir, "colors.toml")?;
    let contents = fs::read_to_string(&path).ok()?;
    let light_mode_marker = path
        .parent()
        .is_some_and(|dir| dir.join("light.mode").is_file());
    parse_colors_toml_with_light_marker(&contents, light_mode_marker)
}

/// Load a sanitized Ghostty color overlay from the active Omarchy theme so
/// embedded terminals follow Omarchy even without a `~/.config/ghostty/config`.
///
/// Only strictly validated color and ANSI palette assignments are emitted; any
/// command, keybind, or `config-file` directives in `ghostty.conf` are ignored.
pub fn load_ghostty_theme_overlay() -> Option<String> {
    load_ghostty_theme_overlay_in(dirs::state_dir().as_deref(), dirs::config_dir().as_deref())
}

fn load_ghostty_theme_overlay_in(
    state_dir: Option<&Path>,
    config_dir: Option<&Path>,
) -> Option<String> {
    if let Some(path) = omarchy_theme_file_in(state_dir, config_dir, "ghostty.conf") {
        if let Ok(contents) = fs::read_to_string(&path) {
            if let Some(sanitized) = sanitize_ghostty_theme_conf(&contents) {
                return Some(sanitized);
            }
        }
    }
    let palette = load_active_palette_in(state_dir, config_dir)?;
    Some(build_ghostty_theme_from_palette(&palette))
}

fn sanitize_ghostty_theme_conf(contents: &str) -> Option<String> {
    let mut lines = Vec::new();
    for raw_line in contents.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((raw_key, raw_val)) = line.split_once('=') else {
            continue;
        };
        let key = raw_key.trim();
        let val = raw_val.trim();
        match key {
            "background"
            | "foreground"
            | "cursor-color"
            | "cursor-text"
            | "selection-background"
            | "selection-foreground" => {
                if let Some(rgb) = Rgb::parse_hex(val) {
                    lines.push(format!("{key} = {}", rgb.to_hex()));
                }
            }
            "palette" => {
                if let Some((idx_raw, color_raw)) = val.split_once('=') {
                    if let (Ok(idx), Some(rgb)) =
                        (idx_raw.trim().parse::<u8>(), Rgb::parse_hex(color_raw))
                    {
                        lines.push(format!("palette = {idx}={}", rgb.to_hex()));
                    }
                }
            }
            _ => {}
        }
    }
    (!lines.is_empty()).then(|| format!("{}\n", lines.join("\n")))
}

fn build_ghostty_theme_from_palette(palette: &OmarchyPalette) -> String {
    let cursor = palette.bright_foreground.unwrap_or(palette.foreground);
    let selection_bg = palette
        .selection
        .unwrap_or_else(|| palette.background.mix(palette.foreground, 0.16));
    format!(
        "background = {}\nforeground = {}\ncursor-color = {}\nselection-background = {}\nselection-foreground = {}\n",
        palette.background.to_hex(),
        palette.foreground.to_hex(),
        cursor.to_hex(),
        selection_bg.to_hex(),
        cursor.to_hex(),
    )
}

/// Parse Omarchy's `colors.toml` into an `OmarchyPalette`.
///
/// Only strictly validated ASCII `#RGB` / `#RRGGBB` / `#RRGGBBAA` hex values
/// are accepted so malformed theme files cannot panic or inject arbitrary CSS.
#[cfg(test)]
pub fn parse_colors_toml(contents: &str) -> Option<OmarchyPalette> {
    parse_colors_toml_with_light_marker(contents, false)
}

fn parse_colors_toml_with_light_marker(
    contents: &str,
    light_mode_marker: bool,
) -> Option<OmarchyPalette> {
    let mut mode: Option<&str> = None;
    let mut background: Option<Rgb> = None;
    let mut bg: Option<Rgb> = None;
    let mut dark_background: Option<Rgb> = None;
    let mut dark_bg: Option<Rgb> = None;
    let mut darker_background: Option<Rgb> = None;
    let mut darker_bg: Option<Rgb> = None;
    let mut lighter_background: Option<Rgb> = None;
    let mut lighter_bg: Option<Rgb> = None;
    let mut foreground: Option<Rgb> = None;
    let mut fg: Option<Rgb> = None;
    let mut dark_foreground: Option<Rgb> = None;
    let mut dark_fg: Option<Rgb> = None;
    let mut bright_foreground: Option<Rgb> = None;
    let mut bright_fg: Option<Rgb> = None;
    let mut muted: Option<Rgb> = None;
    let mut accent: Option<Rgb> = None;
    let mut selection: Option<Rgb> = None;
    let mut selection_background: Option<Rgb> = None;
    let mut selection_foreground: Option<Rgb> = None;
    let mut blue: Option<Rgb> = None;
    let mut color4: Option<Rgb> = None;
    let mut yellow: Option<Rgb> = None;
    let mut color3: Option<Rgb> = None;
    let mut orange: Option<Rgb> = None;
    let mut red: Option<Rgb> = None;
    let mut color1: Option<Rgb> = None;

    for raw_line in contents.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        let Some((raw_key, raw_val)) = line.split_once('=') else {
            continue;
        };
        let key = raw_key.trim();
        let val = parse_toml_string_value(raw_val);
        let Some(val) = val else {
            continue;
        };

        match key {
            "mode" => mode = Some(val),
            "background" => background = Rgb::parse_hex(val),
            "bg" => bg = Rgb::parse_hex(val),
            "dark_background" => dark_background = Rgb::parse_hex(val),
            "dark_bg" => dark_bg = Rgb::parse_hex(val),
            "darker_background" => darker_background = Rgb::parse_hex(val),
            "darker_bg" => darker_bg = Rgb::parse_hex(val),
            "lighter_background" => lighter_background = Rgb::parse_hex(val),
            "lighter_bg" => lighter_bg = Rgb::parse_hex(val),
            "foreground" => foreground = Rgb::parse_hex(val),
            "fg" => fg = Rgb::parse_hex(val),
            "dark_foreground" => dark_foreground = Rgb::parse_hex(val),
            "dark_fg" => dark_fg = Rgb::parse_hex(val),
            "bright_foreground" => bright_foreground = Rgb::parse_hex(val),
            "bright_fg" => bright_fg = Rgb::parse_hex(val),
            "muted" => muted = Rgb::parse_hex(val),
            "accent" => accent = Rgb::parse_hex(val),
            "selection" => selection = Rgb::parse_hex(val),
            "selection_background" => selection_background = Rgb::parse_hex(val),
            "selection_foreground" => selection_foreground = Rgb::parse_hex(val),
            "blue" => blue = Rgb::parse_hex(val),
            "color4" => color4 = Rgb::parse_hex(val),
            "yellow" => yellow = Rgb::parse_hex(val),
            "color3" => color3 = Rgb::parse_hex(val),
            "orange" => orange = Rgb::parse_hex(val),
            "red" => red = Rgb::parse_hex(val),
            "color1" => color1 = Rgb::parse_hex(val),
            _ => {}
        }
    }

    let background = background.or(bg)?;
    let foreground = foreground.or(fg)?;
    let is_dark = match mode {
        Some("light") => false,
        Some("dark") => true,
        _ if light_mode_marker => false,
        _ => background.is_dark(),
    };
    let selection = selection.or(selection_background);
    let accent = accent
        .or(selection)
        .or(blue)
        .or(color4)
        .unwrap_or_else(|| Rgb::new(0x3b, 0x82, 0xf6));
    let accent_fg = selection_foreground.unwrap_or_else(|| accent.contrast_fg());
    let unread = yellow.or(color3).or(orange).unwrap_or(accent);
    let error = red.or(color1).unwrap_or_else(|| Rgb::new(0xdc, 0x5d, 0x44));

    Some(OmarchyPalette {
        is_dark,
        background,
        dark_background: dark_background.or(dark_bg),
        darker_background: darker_background.or(darker_bg),
        lighter_background: lighter_background.or(lighter_bg),
        foreground,
        dark_foreground: dark_foreground.or(dark_fg),
        bright_foreground: bright_foreground.or(bright_fg),
        muted,
        selection,
        accent,
        accent_fg,
        unread,
        error,
    })
}

fn parse_toml_string_value(raw: &str) -> Option<&str> {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix('"') {
        let end = rest.find('"')?;
        Some(&rest[..end])
    } else if let Some(rest) = trimmed.strip_prefix('\'') {
        let end = rest.find('\'')?;
        Some(&rest[..end])
    } else {
        let unquoted = trimmed.split('#').next()?.trim();
        (!unquoted.is_empty()).then_some(unquoted)
    }
}

/// Build the Omarchy GTK4 / libadwaita + Limux CSS override layer.
///
/// When `palette` is `Some`, semantic `@define-color` tokens and Limux chrome
/// colors are derived dynamically from the active Omarchy theme. When `None`
/// (e.g. on systems without Omarchy installed), the sharp flat Omarchy layout
/// and sidebar styling still applies using GTK's existing semantic colors.
pub fn build_omarchy_css(
    palette: Option<&OmarchyPalette>,
    unread_color_override: Option<&str>,
) -> String {
    let Some(p) = palette else {
        return build_omarchy_structural_css(unread_color_override);
    };

    let bg = p.background;
    let fg = p.foreground;
    let active_fg = p.bright_foreground.unwrap_or(fg);
    let accent = p.accent;
    let accent_fg = p.accent_fg;

    // Prefer Omarchy's authored semantic surface ramp when present (Quattro+),
    // falling back to tonal mixes that work identically for dark and light themes.
    let chrome_bg = p
        .dark_background
        .filter(|&c| c != bg)
        .unwrap_or_else(|| bg.mix(fg, 0.015));
    let sidebar_bg = chrome_bg;
    let list_bg = chrome_bg.mix(bg, 0.45);
    let card_bg = p
        .lighter_background
        .filter(|&c| c != bg)
        .unwrap_or_else(|| bg.mix(fg, 0.04));
    let dialog_bg = bg.mix(card_bg, 0.45);
    let popover_bg = card_bg;
    let row_hover_bg = p
        .lighter_background
        .filter(|&c| c != chrome_bg && c != bg)
        .map(|c| chrome_bg.mix(c, 0.55))
        .unwrap_or_else(|| chrome_bg.mix(fg, 0.055));
    let switch_bg = bg.mix(fg, 0.06);
    let row_selected_bg = p
        .selection
        .filter(|&c| c != chrome_bg && c != bg)
        .map(|c| chrome_bg.mix(c, 0.72))
        .unwrap_or_else(|| chrome_bg.mix(fg, 0.085));
    let border_subtle = p
        .muted
        .filter(|&c| c != chrome_bg && c != bg)
        .map(|m| chrome_bg.mix(m, 0.40))
        .unwrap_or_else(|| bg.mix(fg, 0.09));
    let btn_border = p
        .muted
        .filter(|&c| c != chrome_bg && c != bg)
        .map(|m| chrome_bg.mix(m, 0.55))
        .unwrap_or_else(|| bg.mix(fg, 0.12));
    let btn_hover_bg = row_hover_bg.mix(fg, 0.04);
    let popover_border = p
        .muted
        .filter(|&c| c != chrome_bg && c != bg)
        .map(|m| chrome_bg.mix(m, 0.65))
        .unwrap_or_else(|| bg.mix(fg, 0.14));
    let switch_border = p
        .muted
        .filter(|&c| c != chrome_bg && c != bg)
        .map(|m| bg.mix(m, 0.70))
        .unwrap_or_else(|| bg.mix(fg, 0.17));
    let slider_inactive = fg.mix(bg, 0.22);
    let ws_name_inactive = p
        .dark_foreground
        .filter(|&c| c != chrome_bg && c != bg)
        .map(|c| fg.mix(c, 0.42))
        .unwrap_or_else(|| fg.mix(bg, 0.38));
    let placeholder_fg = p
        .dark_foreground
        .filter(|&c| c != chrome_bg && c != bg)
        .unwrap_or_else(|| fg.mix(bg, 0.48));

    let bg_hex = bg.to_hex();
    let chrome_bg_hex = chrome_bg.to_hex();
    let fg_hex = fg.to_hex();
    let active_fg_hex = active_fg.to_hex();
    let sidebar_bg_hex = sidebar_bg.to_hex();
    let list_bg_hex = list_bg.to_hex();
    let dialog_bg_hex = dialog_bg.to_hex();
    let card_bg_hex = card_bg.to_hex();
    let popover_bg_hex = popover_bg.to_hex();
    let row_hover_hex = row_hover_bg.to_hex();
    let switch_bg_hex = switch_bg.to_hex();
    let row_selected_hex = row_selected_bg.to_hex();
    let border_hex = border_subtle.to_hex();
    let btn_border_hex = btn_border.to_hex();
    let btn_hover_hex = btn_hover_bg.to_hex();
    let popover_border_hex = popover_border.to_hex();
    let switch_border_hex = switch_border.to_hex();
    let slider_inactive_hex = slider_inactive.to_hex();
    let ws_name_inactive_hex = ws_name_inactive.to_hex();
    let placeholder_fg_hex = placeholder_fg.to_hex();
    let accent_hex = accent.to_hex();
    let accent_fg_hex = accent_fg.to_hex();
    let error_hex = p.error.to_hex();

    let (unread_border, unread_bg, unread_msg) = match unread_color_override {
        Some(custom) => (
            custom.to_string(),
            format!("alpha({custom}, 0.10)"),
            format!("alpha({custom}, 0.90)"),
        ),
        None => (
            p.unread.to_hex(),
            p.unread.to_rgba(0.10),
            p.unread.to_rgba(0.90),
        ),
    };

    let popover_shadow = if p.is_dark {
        "0 4px 16px rgba(0, 0, 0, 0.55)"
    } else {
        "0 4px 16px rgba(0, 0, 0, 0.16)"
    };

    format!(
        r#"/* Omarchy dynamic GTK4 / libadwaita + Limux theme */
@define-color window_bg_color {bg_hex};
@define-color window_fg_color {fg_hex};

@define-color view_bg_color {bg_hex};
@define-color view_fg_color {fg_hex};

@define-color headerbar_bg_color {chrome_bg_hex};
@define-color headerbar_fg_color {fg_hex};
@define-color headerbar_border_color {border_hex};
@define-color headerbar_backdrop_color {chrome_bg_hex};
@define-color headerbar_shade_color transparent;

@define-color sidebar_bg_color {sidebar_bg_hex};
@define-color sidebar_fg_color {fg_hex};
@define-color sidebar_backdrop_color {sidebar_bg_hex};
@define-color sidebar_shade_color transparent;

@define-color secondary_sidebar_bg_color {chrome_bg_hex};
@define-color secondary_sidebar_fg_color {fg_hex};

@define-color card_bg_color {card_bg_hex};
@define-color card_fg_color {fg_hex};

@define-color popover_bg_color {popover_bg_hex};
@define-color popover_fg_color {fg_hex};

@define-color dialog_bg_color {dialog_bg_hex};
@define-color dialog_fg_color {fg_hex};

@define-color accent_bg_color {accent_hex};
@define-color accent_fg_color {accent_fg_hex};
@define-color accent_color {accent_hex};

@define-color error_color {error_hex};
@define-color borders {border_hex};

window,
window.csd,
window.background,
decoration {{
    border-radius: 0px;
    box-shadow: none;
}}

headerbar {{
    min-height: 30px;
    padding: 2px 6px;
    border-radius: 0px;
    box-shadow: none;
    border-bottom: 1px solid {border_hex};
}}

button,
togglebutton,
menubutton > button,
entry,
spinbutton,
dropdown > button {{
    border-radius: 3px;
    box-shadow: none;
}}

switch {{
    border-radius: 2px;
    min-width: 34px;
    min-height: 18px;
    padding: 2px;
    background-color: {switch_bg_hex};
    border: 1px solid {switch_border_hex};
    box-shadow: none;
}}

switch:checked {{
    background-color: {accent_hex};
    border-color: {accent_hex};
}}

switch > slider {{
    border-radius: 1px;
    min-width: 14px;
    min-height: 14px;
    margin: 0px;
    background-color: {slider_inactive_hex};
    box-shadow: none;
}}

switch:checked > slider {{
    background-color: {accent_fg_hex};
}}

checkbutton > check {{
    border-radius: 2px;
    min-width: 14px;
    min-height: 14px;
    background-color: {switch_bg_hex};
    border: 1px solid {switch_border_hex};
}}

checkbutton > check:checked {{
    background-color: {accent_hex};
    border-color: {accent_hex};
}}

list.boxed-list {{
    background-color: {list_bg_hex};
    border: 1px solid {border_hex};
    border-radius: 3px;
    box-shadow: none;
}}

list.boxed-list > row,
list.boxed-list > row:first-child,
list.boxed-list > row:last-child {{
    border-radius: 0px;
    background-color: transparent;
    border-bottom: 1px solid {border_hex};
}}

list.boxed-list > row:last-child {{
    border-bottom: none;
}}

paned > separator {{
    min-width: 1px;
    min-height: 1px;
    background-color: {border_hex};
}}

popover > contents,
menu,
.card {{
    border-radius: 3px;
    border: 1px solid {popover_border_hex};
    box-shadow: {popover_shadow};
}}

scrollbar slider {{
    border-radius: 1px;
    min-width: 4px;
    min-height: 4px;
}}

/* Limux-specific sharp Omarchy overrides */
.limux-host-entry {{
    background-color: {card_bg_hex};
    color: {fg_hex};
    border: 1px solid {btn_border_hex};
    border-radius: 2px;
}}

.limux-host-entry:focus-within {{
    border-color: {accent_hex};
}}

.limux-host-entry text {{
    color: {fg_hex};
}}

.limux-host-entry text placeholder,
.limux-host-entry image {{
    color: {placeholder_fg_hex};
}}

.limux-top-bar {{
    background-color: {chrome_bg_hex};
    border-bottom: 1px solid {border_hex};
}}

.limux-top-bar-btn,
.limux-top-bar-close,
.limux-pane-action,
.limux-tab-close,
.limux-split-btn {{
    border-radius: 2px;
}}

.limux-indicator-pill {{
    border-radius: 2px;
}}

.limux-indicator-pill-active {{
    background: {row_selected_hex};
    color: {active_fg_hex};
}}

.limux-pane-header {{
    background-color: {chrome_bg_hex};
    border-bottom: 1px solid {border_hex};
}}

.limux-tab {{
    border-radius: 2px 2px 0 0;
}}

.limux-tab-active {{
    color: {active_fg_hex};
    background: {row_selected_hex};
}}

.limux-drop-preview {{
    border-radius: 2px;
}}

.limux-sidebar {{
    background-color: {sidebar_bg_hex};
    border-right: 1px solid {border_hex};
}}

.limux-sidebar-handle {{
    background-color: transparent;
}}

.limux-sidebar-handle:hover {{
    background-color: alpha({accent_hex}, 0.45);
}}

.limux-sidebar-row-box {{
    padding: 6px 10px;
    border-radius: 2px;
    margin: 1px 4px;
    border-left: 2px solid transparent;
    background: transparent;
}}

.limux-sidebar-list row:not(.limux-drop-above):not(.limux-drop-below):hover .limux-sidebar-row-box:not(.limux-ws-colored) {{
    background: {row_hover_hex};
    box-shadow: none;
}}

.limux-sidebar-list row:not(.limux-drop-above):not(.limux-drop-below):selected .limux-sidebar-row-box:not(.limux-ws-colored),
.limux-sidebar-list row:not(.limux-drop-above):not(.limux-drop-below):selected:hover .limux-sidebar-row-box:not(.limux-ws-colored) {{
    background: {row_selected_hex};
    border-left: 2px solid {accent_hex};
    box-shadow: none;
}}

.limux-ws-name {{
    color: {ws_name_inactive_hex};
}}

.limux-sidebar-list row:selected .limux-ws-name {{
    color: {active_fg_hex};
    font-weight: 600;
}}

.limux-indicator-unread-dot,
.limux-notify-dot,
.limux-tab-unread-dot {{
    color: {unread_border};
}}

.limux-notify-msg-unread {{
    color: {unread_msg};
}}

.limux-sidebar-row-unread:not(.limux-ws-colored),
.limux-sidebar-list row:not(.limux-drop-above):not(.limux-drop-below):selected .limux-sidebar-row-box.limux-sidebar-row-unread:not(.limux-ws-colored) {{
    border-radius: 2px;
    margin-left: 4px;
    border-left: 2px solid {unread_border};
    background: {unread_bg};
    box-shadow: none;
}}

.limux-sidebar-btn:not(.limux-sidebar-btn-trash):not(.limux-sidebar-btn-trash-hover) {{
    border-radius: 2px;
    background: {row_hover_hex};
    border: 1px solid {btn_border_hex};
    color: {ws_name_inactive_hex};
    padding: 4px 10px;
}}

.limux-sidebar-btn:not(.limux-sidebar-btn-trash):not(.limux-sidebar-btn-trash-hover):hover {{
    background: {btn_hover_hex};
    border-color: {accent_hex};
    color: {active_fg_hex};
}}

.limux-sidebar-ssh-btn {{
    border-radius: 2px;
    background: transparent;
    border: 1px solid {border_hex};
    color: {ws_name_inactive_hex};
    padding: 3px 8px;
    margin: 2px 4px 6px 4px;
    min-height: 0;
}}

.limux-sidebar-ssh-btn:hover {{
    background: {row_hover_hex};
    border-color: {accent_hex};
    color: {active_fg_hex};
}}

.limux-dialog-secondary-btn {{
    border-radius: 2px;
    border: 1px solid {border_hex};
    color: {ws_name_inactive_hex};
}}

.limux-dialog-secondary-btn:hover {{
    background: {row_hover_hex};
    border-color: {accent_hex};
    color: {active_fg_hex};
}}

.limux-ws-star-btn,
.limux-ws-close-btn {{
    border-radius: 2px;
}}
"#
    )
}

fn build_omarchy_structural_css(unread_color_override: Option<&str>) -> String {
    let unread_color = unread_color_override.unwrap_or("#ffbe71");
    format!(
        r#"/* Omarchy structural GTK4 / libadwaita + Limux theme (fallback colors) */
window,
window.csd,
window.background,
decoration {{
    border-radius: 0px;
    box-shadow: none;
}}

headerbar {{
    min-height: 30px;
    padding: 2px 6px;
    border-radius: 0px;
    box-shadow: none;
    border-bottom: 1px solid alpha(@window_fg_color, 0.10);
}}

button,
togglebutton,
menubutton > button,
entry,
spinbutton,
dropdown > button {{
    border-radius: 3px;
    box-shadow: none;
}}

switch {{
    border-radius: 2px;
    min-width: 34px;
    min-height: 18px;
    padding: 2px;
    box-shadow: none;
}}

switch > slider {{
    border-radius: 1px;
    min-width: 14px;
    min-height: 14px;
    margin: 0px;
    box-shadow: none;
}}

checkbutton > check {{
    border-radius: 2px;
    min-width: 14px;
    min-height: 14px;
}}

list.boxed-list {{
    border: 1px solid alpha(@window_fg_color, 0.10);
    border-radius: 3px;
    box-shadow: none;
}}

list.boxed-list > row,
list.boxed-list > row:first-child,
list.boxed-list > row:last-child {{
    border-radius: 0px;
    background-color: transparent;
}}

paned > separator {{
    min-width: 1px;
    min-height: 1px;
    background-color: alpha(@window_fg_color, 0.10);
}}

popover > contents,
menu,
.card {{
    border-radius: 3px;
    border: 1px solid alpha(@window_fg_color, 0.14);
}}

scrollbar slider {{
    border-radius: 1px;
    min-width: 4px;
    min-height: 4px;
}}

.limux-host-entry,
.limux-top-bar-btn,
.limux-top-bar-close,
.limux-pane-action,
.limux-tab-close,
.limux-split-btn,
.limux-indicator-pill,
.limux-drop-preview,
.limux-ws-star-btn,
.limux-ws-close-btn {{
    border-radius: 2px;
}}

.limux-tab {{
    border-radius: 2px 2px 0 0;
}}

.limux-sidebar-row-box {{
    padding: 6px 10px;
    border-radius: 2px;
    margin: 1px 4px;
    border-left: 2px solid transparent;
    background: transparent;
}}

.limux-sidebar-list row:not(.limux-drop-above):not(.limux-drop-below):hover .limux-sidebar-row-box:not(.limux-ws-colored) {{
    background: alpha(@window_fg_color, 0.05);
    box-shadow: none;
}}

.limux-sidebar-list row:not(.limux-drop-above):not(.limux-drop-below):selected .limux-sidebar-row-box:not(.limux-ws-colored),
.limux-sidebar-list row:not(.limux-drop-above):not(.limux-drop-below):selected:hover .limux-sidebar-row-box:not(.limux-ws-colored) {{
    background: alpha(@window_fg_color, 0.08);
    border-left: 2px solid @accent_bg_color;
    box-shadow: none;
}}

.limux-sidebar-row-unread:not(.limux-ws-colored),
.limux-sidebar-list row:not(.limux-drop-above):not(.limux-drop-below):selected .limux-sidebar-row-box.limux-sidebar-row-unread:not(.limux-ws-colored) {{
    border-radius: 2px;
    margin-left: 4px;
    border-left: 2px solid {unread_color};
    background: alpha({unread_color}, 0.10);
    box-shadow: none;
}}

.limux-sidebar-btn:not(.limux-sidebar-btn-trash):not(.limux-sidebar-btn-trash-hover) {{
    border-radius: 2px;
    border: 1px solid alpha(@window_fg_color, 0.12);
    padding: 4px 10px;
}}

.limux-sidebar-btn:not(.limux-sidebar-btn-trash):not(.limux-sidebar-btn-trash-hover):hover {{
    border-color: @accent_bg_color;
}}

.limux-sidebar-ssh-btn {{
    border-radius: 2px;
    background: transparent;
    border: 1px solid alpha(@window_fg_color, 0.10);
    padding: 3px 8px;
    margin: 2px 4px 6px 4px;
    min-height: 0;
}}

.limux-sidebar-ssh-btn:hover {{
    border-color: @accent_bg_color;
}}

.limux-dialog-secondary-btn {{
    border-radius: 2px;
    border: 1px solid alpha(@window_fg_color, 0.10);
}}

.limux-dialog-secondary-btn:hover {{
    border-color: @accent_bg_color;
}}
"#
    )
}

/// Manages `gio::FileMonitor` watches for Omarchy theme directories,
/// automatically re-attaching monitors when `omarchy/current/theme` symlinks
/// are retargeted or created after startup, and cancelling cleanly on stop/drop.
pub struct OmarchyThemeWatcher {
    inner: Rc<RefCell<WatcherInner>>,
}

struct WatcherInner {
    monitors: Vec<gio::FileMonitor>,
    pending_reload: Option<glib::SourceId>,
    pending_reattach: bool,
    callback: Rc<dyn Fn()>,
    stopped: bool,
}

impl OmarchyThemeWatcher {
    pub fn start<F>(on_change: F) -> Self
    where
        F: Fn() + 'static,
    {
        let inner = Rc::new(RefCell::new(WatcherInner {
            monitors: Vec::new(),
            pending_reload: None,
            pending_reattach: false,
            callback: Rc::new(on_change),
            stopped: false,
        }));
        Self::reattach_monitors(&inner);
        Self { inner }
    }

    pub fn stop(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.stopped = true;
        inner.pending_reattach = false;
        if let Some(source) = inner.pending_reload.take() {
            source.remove();
        }
        for monitor in inner.monitors.drain(..) {
            monitor.cancel();
        }
    }

    fn reattach_monitors(inner: &Rc<RefCell<WatcherInner>>) {
        {
            let mut state = inner.borrow_mut();
            if state.stopped {
                return;
            }
            for monitor in state.monitors.drain(..) {
                monitor.cancel();
            }
        }

        let mut new_monitors = Vec::new();
        for dir in omarchy_watch_dirs() {
            let file = gio::File::for_path(&dir);
            let Ok(monitor) =
                file.monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE)
            else {
                continue;
            };

            let weak_inner = Rc::downgrade(inner);
            monitor.connect_changed(move |_, changed_file, other_file, _| {
                if !is_relevant_omarchy_theme_change(changed_file, other_file) {
                    return;
                }
                let needs_reattach = is_omarchy_dir_or_symlink_change(changed_file, other_file);
                let Some(inner) = weak_inner.upgrade() else {
                    return;
                };
                let mut state = inner.borrow_mut();
                if state.stopped {
                    return;
                }
                state.pending_reattach |= needs_reattach;
                if let Some(existing) = state.pending_reload.take() {
                    existing.remove();
                }
                let weak_for_timer = Rc::downgrade(&inner);
                let source_id = glib::timeout_add_local_once(
                    Duration::from_millis(OMARCHY_RELOAD_DEBOUNCE_MS),
                    move || {
                        let Some(inner) = weak_for_timer.upgrade() else {
                            return;
                        };
                        let (should_reattach, callback) = {
                            let mut state = inner.borrow_mut();
                            state.pending_reload = None;
                            if state.stopped {
                                return;
                            }
                            (
                                std::mem::take(&mut state.pending_reattach),
                                state.callback.clone(),
                            )
                        };
                        if should_reattach {
                            Self::reattach_monitors(&inner);
                        }
                        callback();
                    },
                );
                state.pending_reload = Some(source_id);
            });
            new_monitors.push(monitor);
        }

        let mut state = inner.borrow_mut();
        if state.stopped {
            for monitor in new_monitors {
                monitor.cancel();
            }
        } else {
            state.monitors = new_monitors;
        }
    }
}

impl Drop for OmarchyThemeWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

fn is_relevant_omarchy_theme_change(changed: &gio::File, other: Option<&gio::File>) -> bool {
    let matches = |file: &gio::File| {
        file.basename()
            .and_then(|name| name.to_str().map(str::to_owned))
            .is_some_and(|name| {
                matches!(
                    name.as_str(),
                    "omarchy"
                        | "current"
                        | "theme"
                        | "theme.name"
                        | "theme.generation"
                        | "colors.toml"
                        | "ghostty.conf"
                        | "light.mode"
                )
            })
    };
    matches(changed) || other.is_some_and(matches)
}

fn is_omarchy_dir_or_symlink_change(changed: &gio::File, other: Option<&gio::File>) -> bool {
    let matches = |file: &gio::File| {
        file.basename()
            .and_then(|name| name.to_str().map(str::to_owned))
            .is_some_and(|name| matches!(name.as_str(), "omarchy" | "current" | "theme"))
    };
    matches(changed) || other.is_some_and(matches)
}

#[cfg(test)]
mod tests {
    use super::{
        build_omarchy_css, is_omarchy_active_in, load_active_palette_in,
        load_ghostty_theme_overlay_in, omarchy_colors_path_in, omarchy_watch_dirs_in,
        parse_colors_toml, sanitize_ghostty_theme_conf, should_watch_omarchy_in, OmarchyPalette,
        Rgb,
    };
    use crate::app_config::UiTheme;

    const SAMPLE_DARK_COLORS_TOML: &str = r##"
# Generated by omarchy-themes.
mode = "dark"
accent = "#e37057"
cursor = "#fffeff"
foreground = "#fffeff"
background = "#0a0816"
selection_foreground = "#fffeff"
selection_background = "#e37057"
yellow = "#ffbe71"
red = "#dc5d44"
color3 = "#ffbe71"
"##;

    const SAMPLE_QUATTRO_COLORS_TOML: &str = r##"
mode = "dark"
accent = "#7aa2f7"
selection = "#292e42"
muted = "#414868"
background = "#1a1b26"
dark_background = "#13141c"
darker_background = "#0e0e14"
lighter_background = "#24283b"
foreground = "#a9b1d6"
dark_foreground = "#565f89"
bright_foreground = "#c0caf5"
red = "#f7768e"
yellow = "#e0af68"
"##;

    const SAMPLE_LIGHT_COLORS_TOML: &str = r#"
mode = 'light'
background = '#eff1f5' # latte base
foreground = '#4c4f69'
accent = '#1e66f5'
yellow = '#df8e1d'
red = '#d20f39'
"#;

    #[test]
    fn rgb_parses_short_long_and_alpha_hex_and_rejects_malformed_utf8() {
        assert_eq!(Rgb::parse_hex("#fff"), Some(Rgb::new(255, 255, 255)));
        assert_eq!(Rgb::parse_hex("#0a0816"), Some(Rgb::new(10, 8, 22)));
        assert_eq!(Rgb::parse_hex("#e37057cc"), Some(Rgb::new(227, 112, 87)));
        assert_eq!(Rgb::parse_hex("0a0816"), None);
        assert_eq!(Rgb::parse_hex("#zzzzzz"), None);
        assert_eq!(Rgb::parse_hex("#e37057zz"), None);
        assert_eq!(Rgb::parse_hex("#+12345"), None);
        assert_eq!(Rgb::parse_hex("#€"), None);
        assert_eq!(Rgb::parse_hex("#日本"), None);
        assert_eq!(Rgb::parse_hex("#0a0816; } * { color: red;"), None);
    }

    #[test]
    fn parses_dark_omarchy_colors_toml() {
        let palette = parse_colors_toml(SAMPLE_DARK_COLORS_TOML).expect("palette should parse");
        assert!(palette.is_dark);
        assert_eq!(palette.background, Rgb::new(0x0a, 0x08, 0x16));
        assert_eq!(palette.foreground, Rgb::new(0xff, 0xfe, 0xff));
        assert_eq!(palette.accent, Rgb::new(0xe3, 0x70, 0x57));
        assert_eq!(palette.accent_fg, Rgb::new(0xff, 0xfe, 0xff));
        assert_eq!(palette.unread, Rgb::new(0xff, 0xbe, 0x71));
        assert_eq!(palette.error, Rgb::new(0xdc, 0x5d, 0x44));
    }

    #[test]
    fn parses_light_omarchy_colors_toml_with_single_quotes_and_comments() {
        let palette = parse_colors_toml(SAMPLE_LIGHT_COLORS_TOML).expect("palette should parse");
        assert!(!palette.is_dark);
        assert_eq!(palette.background, Rgb::new(0xef, 0xf1, 0xf5));
        assert_eq!(palette.foreground, Rgb::new(0x4c, 0x4f, 0x69));
        assert_eq!(palette.accent, Rgb::new(0x1e, 0x66, 0xf5));
        assert_eq!(palette.unread, Rgb::new(0xdf, 0x8e, 0x1d));
        assert_eq!(palette.error, Rgb::new(0xd2, 0x0f, 0x39));
    }

    #[test]
    fn rejects_invalid_or_incomplete_colors_toml() {
        assert!(parse_colors_toml("accent = \"#e37057\"\n").is_none());
        assert!(parse_colors_toml(
            "background = \"#000000; } window { opacity: 0;\"\nforeground = \"#ffffff\"\n"
        )
        .is_none());
    }

    #[test]
    fn css_preserves_colored_rows_trash_buttons_drop_indicators_and_entry_text() {
        let palette = parse_colors_toml(SAMPLE_DARK_COLORS_TOML).unwrap();
        let css = build_omarchy_css(Some(&palette), None);
        assert!(css.contains("@define-color window_bg_color #0a0816;"));
        assert!(css.contains("@define-color accent_bg_color #e37057;"));
        assert!(css.contains("border-left: 2px solid #e37057;"));
        assert!(css.contains("border-left: 2px solid #ffbe71;"));
        assert!(css.contains(":not(.limux-ws-colored)"));
        assert!(css.contains(":not(.limux-sidebar-btn-trash):not(.limux-sidebar-btn-trash-hover)"));
        assert!(css.contains(":not(.limux-drop-above):not(.limux-drop-below)"));
        assert!(css.contains(".limux-host-entry text"));
    }

    #[test]
    fn css_respects_explicit_unread_color_override() {
        let palette = parse_colors_toml(SAMPLE_DARK_COLORS_TOML).unwrap();
        let css = build_omarchy_css(Some(&palette), Some("#10b981"));
        assert!(css.contains("border-left: 2px solid #10b981;"));
        assert!(css.contains("background: alpha(#10b981, 0.10);"));
    }

    #[test]
    fn structural_fallback_css_works_without_palette() {
        let css = build_omarchy_css(None, None);
        assert!(!css.contains("@define-color"));
        assert!(css.contains("border-left: 2px solid @accent_bg_color;"));
        assert!(css.contains(".limux-sidebar-row-box"));
    }

    #[test]
    fn resolves_state_path_and_parent_watch_fallback() {
        let temp = tempfile::tempdir().unwrap();
        let state_dir = temp.path().join("state");
        let config_dir = temp.path().join("config");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::create_dir_all(&config_dir).unwrap();

        // Before omarchy dirs exist, watch dirs fall back to existing XDG parents.
        let initial_watched = omarchy_watch_dirs_in(Some(&state_dir), Some(&config_dir));
        assert!(initial_watched.contains(&state_dir));
        assert!(initial_watched.contains(&config_dir));

        let config_colors = config_dir.join("omarchy/current/theme/colors.toml");
        std::fs::create_dir_all(config_colors.parent().unwrap()).unwrap();
        std::fs::write(&config_colors, SAMPLE_DARK_COLORS_TOML).unwrap();

        assert_eq!(
            omarchy_colors_path_in(Some(&state_dir), Some(&config_dir)),
            Some(config_colors.clone())
        );

        let state_colors = state_dir.join("omarchy/current/theme/colors.toml");
        std::fs::create_dir_all(state_colors.parent().unwrap()).unwrap();
        std::fs::write(&state_colors, SAMPLE_LIGHT_COLORS_TOML).unwrap();

        assert_eq!(
            omarchy_colors_path_in(Some(&state_dir), Some(&config_dir)),
            Some(state_colors)
        );

        let watched = omarchy_watch_dirs_in(Some(&state_dir), Some(&config_dir));
        assert!(watched.contains(&state_dir.join("omarchy/current")));
        assert!(watched.contains(&state_dir.join("omarchy/current/theme")));
        assert!(watched.contains(&config_dir.join("omarchy/current")));
    }

    #[test]
    fn light_and_dark_palette_surface_mixing_preserves_direction() {
        let dark = OmarchyPalette {
            is_dark: true,
            background: Rgb::new(10, 8, 22),
            dark_background: None,
            darker_background: None,
            lighter_background: None,
            foreground: Rgb::new(255, 254, 255),
            dark_foreground: None,
            bright_foreground: None,
            muted: None,
            selection: None,
            accent: Rgb::new(227, 112, 87),
            accent_fg: Rgb::new(255, 255, 255),
            unread: Rgb::new(255, 190, 113),
            error: Rgb::new(220, 93, 68),
        };
        let dark_lifted = dark.background.mix(dark.foreground, 0.05);
        assert!(dark_lifted.luminance() > dark.background.luminance());

        let light = OmarchyPalette {
            is_dark: false,
            background: Rgb::new(239, 241, 245),
            dark_background: None,
            darker_background: None,
            lighter_background: None,
            foreground: Rgb::new(76, 79, 105),
            dark_foreground: None,
            bright_foreground: None,
            muted: None,
            selection: None,
            accent: Rgb::new(30, 102, 245),
            accent_fg: Rgb::new(255, 255, 255),
            unread: Rgb::new(223, 142, 29),
            error: Rgb::new(210, 15, 57),
        };
        let light_shaded = light.background.mix(light.foreground, 0.05);
        assert!(light_shaded.luminance() < light.background.luminance());
    }

    #[test]
    fn parses_quattro_semantic_ramp_and_styles_chrome_and_ssh_button() {
        let palette = parse_colors_toml(SAMPLE_QUATTRO_COLORS_TOML).expect("quattro palette");
        assert_eq!(palette.dark_background, Some(Rgb::new(0x13, 0x14, 0x1c)));
        assert_eq!(palette.lighter_background, Some(Rgb::new(0x24, 0x28, 0x3b)));
        assert_eq!(palette.muted, Some(Rgb::new(0x41, 0x48, 0x68)));
        assert_eq!(palette.selection, Some(Rgb::new(0x29, 0x2e, 0x42)));
        assert_eq!(palette.bright_foreground, Some(Rgb::new(0xc0, 0xca, 0xf5)));

        let css = build_omarchy_css(Some(&palette), None);
        assert!(css.contains("@define-color window_bg_color #1a1b26;"));
        assert!(css.contains("@define-color headerbar_bg_color #13141c;"));
        assert!(css.contains("@define-color sidebar_bg_color #13141c;"));
        assert!(css.contains("@define-color card_bg_color #24283b;"));
        assert!(css.contains(".limux-sidebar-ssh-btn"));
        assert!(css.contains(".limux-dialog-secondary-btn"));
    }

    #[test]
    fn auto_theme_activates_and_watches_only_when_omarchy_is_present() {
        let temp = tempfile::tempdir().unwrap();
        let state_dir = temp.path().join("state");
        let config_dir = temp.path().join("config");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::create_dir_all(&config_dir).unwrap();

        assert!(!is_omarchy_active_in(
            UiTheme::Auto,
            Some(&state_dir),
            Some(&config_dir)
        ));
        assert!(!should_watch_omarchy_in(
            UiTheme::Auto,
            Some(&state_dir),
            Some(&config_dir)
        ));
        assert!(is_omarchy_active_in(
            UiTheme::Omarchy,
            Some(&state_dir),
            Some(&config_dir)
        ));
        assert!(!is_omarchy_active_in(
            UiTheme::Default,
            Some(&state_dir),
            Some(&config_dir)
        ));

        let colors = state_dir.join("omarchy/current/theme/colors.toml");
        std::fs::create_dir_all(colors.parent().unwrap()).unwrap();
        std::fs::write(&colors, SAMPLE_QUATTRO_COLORS_TOML).unwrap();

        assert!(is_omarchy_active_in(
            UiTheme::Auto,
            Some(&state_dir),
            Some(&config_dir)
        ));
        assert!(should_watch_omarchy_in(
            UiTheme::Auto,
            Some(&state_dir),
            Some(&config_dir)
        ));
        assert!(!is_omarchy_active_in(
            UiTheme::Default,
            Some(&state_dir),
            Some(&config_dir)
        ));
    }

    #[test]
    fn legacy_light_mode_marker_file_sets_light_mode_when_mode_key_absent() {
        let temp = tempfile::tempdir().unwrap();
        let state_dir = temp.path().join("state");
        let theme_dir = state_dir.join("omarchy/current/theme");
        std::fs::create_dir_all(&theme_dir).unwrap();
        std::fs::write(
            theme_dir.join("colors.toml"),
            "background = \"#404040\"\nforeground = \"#101010\"\n",
        )
        .unwrap();
        std::fs::write(theme_dir.join("light.mode"), "").unwrap();

        let palette = load_active_palette_in(Some(&state_dir), None).expect("palette");
        assert!(!palette.is_dark);
    }

    #[test]
    fn sanitizes_ghostty_theme_conf_and_falls_back_to_colors_toml() {
        let raw = r#"
background = #1a1b26
foreground = #a9b1d6
cursor-color = #c0caf5
selection-background = #292e42
selection-foreground = #c0caf5
palette = 0=#1a1b26
palette = 1=#f7768e
palette = 999=#ffffff
command = /bin/evil
config-file = /etc/passwd
background = not-a-color
"#;
        let sanitized = sanitize_ghostty_theme_conf(raw).expect("sanitized");
        assert!(sanitized.contains("background = #1a1b26\n"));
        assert!(sanitized.contains("palette = 0=#1a1b26\n"));
        assert!(sanitized.contains("palette = 1=#f7768e\n"));
        assert!(!sanitized.contains("command"));
        assert!(!sanitized.contains("config-file"));
        assert!(!sanitized.contains("999"));

        let temp = tempfile::tempdir().unwrap();
        let state_dir = temp.path().join("state");
        let theme_dir = state_dir.join("omarchy/current/theme");
        std::fs::create_dir_all(&theme_dir).unwrap();
        std::fs::write(theme_dir.join("colors.toml"), SAMPLE_QUATTRO_COLORS_TOML).unwrap();

        let fallback = load_ghostty_theme_overlay_in(Some(&state_dir), None).expect("fallback");
        assert!(fallback.contains("background = #1a1b26\n"));
        assert!(fallback.contains("foreground = #a9b1d6\n"));
        assert!(fallback.contains("cursor-color = #c0caf5\n"));
        assert!(fallback.contains("selection-background = #292e42\n"));
    }

    #[test]
    #[ignore = "requires GTK; exercised by xvfb-smoke-test.sh"]
    fn gtk_css_provider_parses_omarchy_css_without_errors() {
        gtk4::init().expect("initialize GTK");
        let dark_palette = parse_colors_toml(SAMPLE_DARK_COLORS_TOML).unwrap();
        let light_palette = parse_colors_toml(SAMPLE_LIGHT_COLORS_TOML).unwrap();

        for (label, css) in [
            ("dark", build_omarchy_css(Some(&dark_palette), None)),
            (
                "dark-custom-unread",
                build_omarchy_css(Some(&dark_palette), Some("#10b981")),
            ),
            ("light", build_omarchy_css(Some(&light_palette), None)),
            ("fallback", build_omarchy_css(None, None)),
        ] {
            let errors = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let provider = gtk4::CssProvider::new();
            let errors_clone = errors.clone();
            provider.connect_parsing_error(move |_, section, err| {
                errors_clone.borrow_mut().push(format!("{section}: {err}"));
            });
            provider.load_from_data(&css);
            assert!(
                errors.borrow().is_empty(),
                "GTK CSS parse errors for {label}: {:?}",
                errors.borrow()
            );
        }
    }
}
