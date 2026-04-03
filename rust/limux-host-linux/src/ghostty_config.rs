const DEFAULT_FONT_SIZE: f32 = 12.0;

fn ghostty_config_contents() -> Option<String> {
    let path = dirs::config_dir()
        .map(|d| d.join("ghostty/config"))
        .filter(|p| p.exists())?;
    std::fs::read_to_string(&path).ok()
}

fn read_ghostty_value(contents: &str, key: &str) -> Option<String> {
    for line in contents.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(key) {
            let rest = rest.trim();
            if let Some(value) = rest.strip_prefix('=') {
                return Some(value.trim().to_string());
            }
        }
    }
    None
}

/// Read background-opacity from the Ghostty config file.
/// Returns a value between 0.0 and 1.0 (default: 1.0 = fully opaque).
#[allow(dead_code)]
pub fn read_background_opacity() -> f64 {
    ghostty_config_contents()
        .and_then(|c| read_ghostty_value(&c, "background-opacity"))
        .and_then(|v| v.parse::<f64>().ok())
        .map(|v| v.clamp(0.0, 1.0))
        .unwrap_or(1.0)
}

/// Read font-size from the Ghostty config file.
/// Returns the configured size in points (default: 12.0).
pub fn read_font_size() -> f32 {
    ghostty_config_contents()
        .and_then(|c| read_ghostty_value(&c, "font-size"))
        .and_then(|v| v.parse::<f32>().ok())
        .map(|v| v.clamp(1.0, 255.0))
        .unwrap_or(DEFAULT_FONT_SIZE)
}
