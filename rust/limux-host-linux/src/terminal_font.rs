use serde_json::{json, Map, Value};

pub const DEFAULT_CELL_METRIC_PERCENT: u16 = 100;
pub const MIN_CELL_METRIC_PERCENT: u16 = 50;
pub const MAX_CELL_METRIC_PERCENT: u16 = 200;
pub const MIN_TERMINAL_FONT_SIZE: f32 = 1.0;
pub const MAX_TERMINAL_FONT_SIZE: f32 = 255.0;
pub const MIN_UI_TERMINAL_FONT_SIZE: f64 = 4.0;
pub const MAX_UI_TERMINAL_FONT_SIZE: f64 = 96.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TerminalFontConfig {
    pub family: Option<String>,
    pub style: Option<String>,
    pub size: Option<f32>,
    pub cell_width_percent: Option<u16>,
    pub cell_height_percent: Option<u16>,
    pub use_ligatures: Option<bool>,
    pub anti_aliased: Option<bool>,
}

impl TerminalFontConfig {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }

    pub fn sanitized(&self) -> Self {
        let family = self.family.as_deref().and_then(sanitize_font_token);
        let style = if family.is_some() {
            self.style.as_deref().and_then(sanitize_font_token)
        } else {
            None
        };
        let size = self.size.and_then(sanitize_font_size);
        let cell_width_percent = self
            .cell_width_percent
            .map(|v| v.clamp(MIN_CELL_METRIC_PERCENT, MAX_CELL_METRIC_PERCENT));
        let cell_height_percent = self
            .cell_height_percent
            .map(|v| v.clamp(MIN_CELL_METRIC_PERCENT, MAX_CELL_METRIC_PERCENT));

        Self {
            family,
            style,
            size,
            cell_width_percent,
            cell_height_percent,
            use_ligatures: self.use_ligatures,
            anti_aliased: self.anti_aliased,
        }
    }

    pub fn from_json(value: Option<&Value>) -> Self {
        let Some(obj) = value.and_then(Value::as_object) else {
            return Self::default();
        };

        let family = obj
            .get("family")
            .and_then(Value::as_str)
            .and_then(sanitize_font_token);
        let style = if family.is_some() {
            obj.get("style")
                .and_then(Value::as_str)
                .and_then(sanitize_font_token)
        } else {
            None
        };
        let size = obj
            .get("size")
            .and_then(Value::as_f64)
            .and_then(|v| sanitize_font_size(v as f32));
        let cell_width_percent = obj
            .get("cell_width_percent")
            .and_then(Value::as_u64)
            .and_then(|v| u16::try_from(v).ok())
            .filter(|v| (MIN_CELL_METRIC_PERCENT..=MAX_CELL_METRIC_PERCENT).contains(v));
        let cell_height_percent = obj
            .get("cell_height_percent")
            .and_then(Value::as_u64)
            .and_then(|v| u16::try_from(v).ok())
            .filter(|v| (MIN_CELL_METRIC_PERCENT..=MAX_CELL_METRIC_PERCENT).contains(v));
        let use_ligatures = obj.get("use_ligatures").and_then(Value::as_bool);
        let anti_aliased = obj.get("anti_aliased").and_then(Value::as_bool);

        Self {
            family,
            style,
            size,
            cell_width_percent,
            cell_height_percent,
            use_ligatures,
            anti_aliased,
        }
    }

    pub fn to_json(&self) -> Option<Value> {
        let clean = self.sanitized();
        if clean.is_default() {
            return None;
        }

        let mut map = Map::new();
        if let Some(family) = clean.family {
            map.insert("family".to_string(), json!(family));
        }
        if let Some(style) = clean.style {
            map.insert("style".to_string(), json!(style));
        }
        if let Some(size) = clean.size {
            map.insert("size".to_string(), json!(f64::from(size)));
        }
        if let Some(cell_width_percent) = clean.cell_width_percent {
            map.insert("cell_width_percent".to_string(), json!(cell_width_percent));
        }
        if let Some(cell_height_percent) = clean.cell_height_percent {
            map.insert(
                "cell_height_percent".to_string(),
                json!(cell_height_percent),
            );
        }
        if let Some(use_ligatures) = clean.use_ligatures {
            map.insert("use_ligatures".to_string(), json!(use_ligatures));
        }
        if let Some(anti_aliased) = clean.anti_aliased {
            map.insert("anti_aliased".to_string(), json!(anti_aliased));
        }
        (!map.is_empty()).then_some(Value::Object(map))
    }

    /// Format a Ghostty configuration snippet overlaying user terminal font choices.
    pub fn ghostty_overlay(&self, base_freetype_flags: Option<&str>) -> Option<String> {
        let clean = self.sanitized();
        if clean.is_default() {
            return None;
        }

        let mut lines = Vec::new();
        if let Some(family) = clean.family {
            lines.push("font-family = \"\"".to_string());
            lines.push("font-family-bold = \"\"".to_string());
            lines.push("font-family-italic = \"\"".to_string());
            lines.push("font-family-bold-italic = \"\"".to_string());
            lines.push(format!("font-family = {family}"));
            let style = clean.style.as_deref().unwrap_or("default");
            lines.push(format!("font-style = {style}"));
            lines.push("font-style-bold = default".to_string());
            lines.push("font-style-italic = default".to_string());
            lines.push("font-style-bold-italic = default".to_string());
        }
        if let Some(size) = clean.size {
            lines.push(format!("font-size = {}", format_font_size(size)));
        }
        if let Some(cell_width_percent) = clean.cell_width_percent {
            let delta = i32::from(cell_width_percent) - i32::from(DEFAULT_CELL_METRIC_PERCENT);
            lines.push(format!("adjust-cell-width = {delta}%"));
        }
        if let Some(cell_height_percent) = clean.cell_height_percent {
            let delta = i32::from(cell_height_percent) - i32::from(DEFAULT_CELL_METRIC_PERCENT);
            lines.push(format!("adjust-cell-height = {delta}%"));
        }
        if let Some(use_ligatures) = clean.use_ligatures {
            if use_ligatures {
                lines.push("font-feature = +calt, +liga, +dlig".to_string());
            } else {
                lines.push("font-feature = -calt, -liga, -dlig".to_string());
            }
        }
        if let Some(anti_aliased) = clean.anti_aliased {
            lines.push(format!(
                "freetype-load-flags = {}",
                merge_freetype_monochrome_flag(base_freetype_flags, !anti_aliased)
            ));
        }

        (!lines.is_empty()).then(|| {
            let mut out = lines.join("\n");
            out.push('\n');
            out
        })
    }
}

pub fn sanitize_font_token(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("default")
        || trimmed.eq_ignore_ascii_case("false")
        || trimmed.chars().any(|c| c.is_control() || c == '"')
    {
        return None;
    }
    Some(trimmed.to_string())
}

pub fn sanitize_font_size(size: f32) -> Option<f32> {
    if !size.is_finite() {
        return None;
    }
    let rounded = (size * 100.0).round() / 100.0;
    (MIN_TERMINAL_FONT_SIZE..=MAX_TERMINAL_FONT_SIZE)
        .contains(&rounded)
        .then_some(rounded)
}

pub fn format_font_size(size: f32) -> String {
    let rounded = (f64::from(size) * 100.0).round() / 100.0;
    if (rounded - rounded.round()).abs() < f64::EPSILON {
        format!("{:.0}", rounded.round())
    } else if ((rounded * 10.0) - (rounded * 10.0).round()).abs() < f64::EPSILON {
        format!("{rounded:.1}")
    } else {
        format!("{rounded:.2}")
    }
}

pub fn sort_face_styles<I, S>(raw_styles: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut styles: Vec<String> = raw_styles
        .into_iter()
        .filter_map(|s| sanitize_font_token(s.as_ref()))
        .collect();
    styles.sort_by(|a, b| {
        style_rank(a)
            .cmp(&style_rank(b))
            .then_with(|| a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase()))
            .then_with(|| a.cmp(b))
    });
    styles.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    styles
}

fn style_rank(style: &str) -> u8 {
    match style.to_ascii_lowercase().as_str() {
        "regular" => 0,
        "book" | "roman" | "normal" => 1,
        "thin" | "hairline" | "extralight" | "extra light" | "ultralight" => 2,
        "light" | "semilight" | "demilight" => 3,
        "medium" => 4,
        "semibold" | "semi bold" | "demibold" | "demi bold" => 5,
        "bold" => 6,
        "extrabold" | "extra bold" | "heavy" | "black" => 7,
        "italic" | "oblique" => 8,
        "bold italic" | "bold oblique" => 9,
        _ => 10,
    }
}

pub fn preferred_style_for_family_change(
    styles: &[String],
    previous_style: Option<&str>,
) -> Option<String> {
    if let Some(prev) = previous_style {
        if let Some(existing) = styles.iter().find(|s| s.eq_ignore_ascii_case(prev)) {
            return Some(existing.clone());
        }
    }
    for candidate in ["Regular", "Book", "Roman", "Normal"] {
        if let Some(found) = styles.iter().find(|s| s.eq_ignore_ascii_case(candidate)) {
            return Some(found.clone());
        }
    }
    None
}

fn merge_freetype_monochrome_flag(base: Option<&str>, monochrome: bool) -> String {
    let target = if monochrome {
        "monochrome"
    } else {
        "no-monochrome"
    };
    let Some(raw) = base
        .map(crate::ghostty_config::strip_optional_quotes)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return target.to_string();
    };
    if raw.eq_ignore_ascii_case("false") {
        return format!("no-hinting,no-force-autohint,no-autohint,no-light,{target}");
    }
    if raw.eq_ignore_ascii_case("true") {
        return format!("hinting,force-autohint,autohint,light,{target}");
    }
    let mut parts: Vec<&str> = raw
        .split(',')
        .map(str::trim)
        .filter(|part| {
            !part.is_empty()
                && !part.eq_ignore_ascii_case("monochrome")
                && !part.eq_ignore_ascii_case("no-monochrome")
        })
        .collect();
    parts.push(target);
    parts.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_produces_no_json_or_overlay() {
        let cfg = TerminalFontConfig::default();
        assert!(cfg.is_default());
        assert_eq!(cfg.to_json(), None);
        assert_eq!(cfg.ghostty_overlay(None), None);
    }

    #[test]
    fn iterm_profile_renders_expected_ghostty_overlay_and_json_roundtrip() {
        let cfg = TerminalFontConfig {
            family: Some("MesloLGS NF".to_string()),
            style: Some("Regular".to_string()),
            size: Some(13.0),
            cell_width_percent: None,
            cell_height_percent: None,
            use_ligatures: Some(false),
            anti_aliased: Some(true),
        };

        let json = cfg.to_json().expect("non-default json");
        assert_eq!(TerminalFontConfig::from_json(Some(&json)), cfg);

        let overlay = cfg.ghostty_overlay(None).expect("overlay");
        assert!(overlay.contains("font-family = \"\"\n"));
        assert!(overlay.contains("font-family = MesloLGS NF\n"));
        assert!(overlay.contains("font-style = Regular\n"));
        assert!(overlay.contains("font-size = 13\n"));
        assert!(overlay.contains("font-feature = -calt, -liga, -dlig\n"));
        assert!(overlay.contains("freetype-load-flags = no-monochrome\n"));
        assert!(!overlay.contains("adjust-cell-width"));
        assert!(!overlay.contains("adjust-cell-height"));
    }

    #[test]
    fn cell_spacing_percentages_translate_to_ghostty_metric_modifiers() {
        let cfg = TerminalFontConfig {
            cell_width_percent: Some(112),
            cell_height_percent: Some(92),
            ..TerminalFontConfig::default()
        };
        let overlay = cfg.ghostty_overlay(None).expect("overlay");
        assert!(overlay.contains("adjust-cell-width = 12%\n"));
        assert!(overlay.contains("adjust-cell-height = -8%\n"));

        let explicit_normal = TerminalFontConfig {
            cell_width_percent: Some(100),
            cell_height_percent: Some(100),
            ..TerminalFontConfig::default()
        };
        let explicit_json = explicit_normal.to_json().expect("explicit 100% json");
        assert_eq!(
            TerminalFontConfig::from_json(Some(&explicit_json)),
            explicit_normal
        );
        let explicit_overlay = explicit_normal.ghostty_overlay(None).expect("0% overlay");
        assert!(explicit_overlay.contains("adjust-cell-width = 0%\n"));
        assert!(explicit_overlay.contains("adjust-cell-height = 0%\n"));
    }

    #[test]
    fn freetype_monochrome_merge_preserves_other_flags() {
        let cfg = TerminalFontConfig {
            anti_aliased: Some(false),
            ..TerminalFontConfig::default()
        };
        let overlay = cfg
            .ghostty_overlay(Some("no-hinting, no-monochrome"))
            .expect("overlay");
        assert!(overlay.contains("freetype-load-flags = no-hinting,monochrome\n"));
    }

    #[test]
    fn sanitize_rejects_control_characters_quotes_and_orphan_style() {
        let parsed = TerminalFontConfig::from_json(Some(&json!({
            "family": "Bad\nFont",
            "style": "Regular",
            "size": 500.0,
            "cell_width_percent": 10,
            "cell_height_percent": 999
        })));
        assert_eq!(parsed, TerminalFontConfig::default());

        assert_eq!(sanitize_font_token("MesloLGS \"NF\""), None);
        assert_eq!(sanitize_font_token("default"), None);
        assert_eq!(sanitize_font_token("false"), None);
    }

    #[test]
    fn sort_styles_and_preferred_style_prioritize_regular() {
        let styles = sort_face_styles(["Bold Italic", "Italic", "Bold", "Regular"]);
        assert_eq!(styles, ["Regular", "Bold", "Italic", "Bold Italic"]);
        assert_eq!(
            preferred_style_for_family_change(&styles, None).as_deref(),
            Some("Regular")
        );
        assert_eq!(
            preferred_style_for_family_change(&styles, Some("Bold")).as_deref(),
            Some("Bold")
        );
    }
}
