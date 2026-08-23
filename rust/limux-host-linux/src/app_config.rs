use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::shortcut_config;

pub const SETTINGS_FILE_NAME: &str = "settings.json";
pub const DEFAULT_UI_SCALE: f32 = 1.0;
pub const MIN_UI_SCALE: f32 = 0.8;
pub const MAX_UI_SCALE: f32 = 2.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorScheme {
    #[default]
    System,
    Dark,
    Light,
}

impl ColorScheme {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "system" => Some(Self::System),
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub focus: FocusConfig,
    #[serde(skip)]
    pub appearance: AppearanceConfig,
    #[serde(skip)]
    pub workspace: WorkspaceConfig,
    #[serde(skip)]
    pub notifications: NotificationConfig,
    #[serde(skip)]
    pub clipboard: ClipboardConfig,
    #[serde(skip)]
    pub links: LinkConfig,
    #[serde(skip)]
    pub font_size: Option<f32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceConfig {
    pub keep_open_after_last_terminal_closes: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppearanceConfig {
    pub color_scheme: ColorScheme,
    pub ghostty_color_scheme: ColorScheme,
    pub ui_scale: UiScale,
    pub show_workspace_path: bool,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            color_scheme: ColorScheme::default(),
            ghostty_color_scheme: ColorScheme::default(),
            ui_scale: UiScale::default(),
            show_workspace_path: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiScale(f32);

impl Default for UiScale {
    fn default() -> Self {
        Self(DEFAULT_UI_SCALE)
    }
}

impl UiScale {
    pub fn new(value: f32) -> Option<Self> {
        value
            .is_finite()
            .then_some(Self(value.clamp(MIN_UI_SCALE, MAX_UI_SCALE)))
    }

    pub fn get(self) -> f32 {
        self.0
    }

    pub fn is_default(self) -> bool {
        (self.0 - DEFAULT_UI_SCALE).abs() < f32::EPSILON
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct FocusConfig {
    #[serde(default)]
    pub hover_terminal_focus: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipboardConfig {
    pub copy_selection_to_clipboard: bool,
}

impl Default for ClipboardConfig {
    fn default() -> Self {
        Self {
            copy_selection_to_clipboard: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LinkOpenDestination {
    #[default]
    DefaultBrowser,
    BrowserTab,
}

impl LinkOpenDestination {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DefaultBrowser => "default_browser",
            Self::BrowserTab => "browser_tab",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "default_browser" => Some(Self::DefaultBrowser),
            "browser_tab" => Some(Self::BrowserTab),
            _ => None,
        }
    }

    pub fn dropdown_index(self) -> u32 {
        match self {
            Self::DefaultBrowser => 0,
            Self::BrowserTab => 1,
        }
    }

    pub fn from_dropdown_index(index: u32) -> Self {
        match index {
            1 => Self::BrowserTab,
            _ => Self::DefaultBrowser,
        }
    }

    pub fn effective(self, embedded_browser_available: bool) -> Self {
        if self == Self::BrowserTab && !embedded_browser_available {
            Self::DefaultBrowser
        } else {
            self
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinkConfig {
    pub open_destination: LinkOpenDestination,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NotificationSound {
    #[default]
    Default,
    Message,
    Bell,
    Complete,
    Alert,
    None,
}

impl NotificationSound {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Message => "message",
            Self::Bell => "bell",
            Self::Complete => "complete",
            Self::Alert => "alert",
            Self::None => "none",
        }
    }

    pub fn labels() -> &'static [&'static str] {
        &["Default", "Message", "Bell", "Complete", "Alert", "None"]
    }

    pub fn dropdown_index(self) -> u32 {
        match self {
            Self::Default => 0,
            Self::Message => 1,
            Self::Bell => 2,
            Self::Complete => 3,
            Self::Alert => 4,
            Self::None => 5,
        }
    }

    pub fn from_dropdown_index(index: u32) -> Self {
        match index {
            1 => Self::Message,
            2 => Self::Bell,
            3 => Self::Complete,
            4 => Self::Alert,
            5 => Self::None,
            _ => Self::Default,
        }
    }

    pub fn freedesktop_sound_name(self) -> Option<&'static str> {
        match self {
            Self::Default | Self::None => None,
            Self::Message => Some("message-new-instant"),
            Self::Bell => Some("bell-terminal"),
            Self::Complete => Some("complete"),
            Self::Alert => Some("dialog-warning"),
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "default" => Some(Self::Default),
            "message" => Some(Self::Message),
            "bell" => Some(Self::Bell),
            "complete" => Some(Self::Complete),
            "alert" => Some(Self::Alert),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotificationConfig {
    pub enabled: bool,
    pub sound: NotificationSound,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            sound: NotificationSound::Default,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoadedAppConfig {
    pub config: AppConfig,
    pub warnings: Vec<String>,
}

pub fn load() -> LoadedAppConfig {
    let Some(path) = settings_path() else {
        let mut loaded = LoadedAppConfig::default();
        loaded
            .warnings
            .push("config_dir unavailable; using default app settings".to_string());
        return loaded;
    };

    if let Err(err) = ensure_default_config_file(&path) {
        let mut loaded = LoadedAppConfig::default();
        loaded.warnings.push(format!(
            "failed to create default app config `{}`: {err}",
            path.display()
        ));
        return loaded;
    }

    load_from_path(&path)
}

pub fn settings_path() -> Option<std::path::PathBuf> {
    shortcut_config::config_dir_path().map(|dir| dir.join(SETTINGS_FILE_NAME))
}

#[cfg(test)]
pub fn settings_path_in(base: &Path) -> std::path::PathBuf {
    shortcut_config::config_dir_path_in(base).join(SETTINGS_FILE_NAME)
}

pub fn load_from_path(path: &Path) -> LoadedAppConfig {
    if !path.exists() {
        return LoadedAppConfig::default();
    }

    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) => {
            let mut loaded = LoadedAppConfig::default();
            loaded.warnings.push(format!(
                "failed to read app config `{}`: {err}",
                path.display()
            ));
            return loaded;
        }
    };

    match serde_json::from_str::<Value>(&raw) {
        Ok(root) => LoadedAppConfig {
            config: parse_app_config_value(&root),
            warnings: Vec::new(),
        },
        Err(err) => {
            let mut loaded = LoadedAppConfig::default();
            loaded.warnings.push(format!(
                "failed to load app config `{}`: {err}",
                path.display()
            ));
            loaded
        }
    }
}

fn parse_app_config_value(root: &Value) -> AppConfig {
    let hover_terminal_focus = root
        .get("focus")
        .and_then(Value::as_object)
        .and_then(|focus| focus.get("hover_terminal_focus"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let appearance = root.get("appearance").and_then(Value::as_object);

    let color_scheme = appearance
        .and_then(|appearance| appearance.get("color_scheme"))
        .and_then(Value::as_str)
        .and_then(ColorScheme::from_str)
        .unwrap_or_default();

    let ghostty_color_scheme = appearance
        .and_then(|appearance| appearance.get("ghostty_color_scheme"))
        .and_then(Value::as_str)
        .and_then(ColorScheme::from_str)
        .unwrap_or(color_scheme);

    let ui_scale = appearance
        .and_then(|appearance| appearance.get("ui_scale"))
        .and_then(Value::as_f64)
        .map(|v| v as f32)
        .and_then(UiScale::new)
        .unwrap_or_default();

    let show_workspace_path = appearance
        .and_then(|appearance| appearance.get("show_workspace_path"))
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let workspace = root.get("workspace").and_then(Value::as_object);
    let keep_open_after_last_terminal_closes = workspace
        .and_then(|workspace| workspace.get("keep_open_after_last_terminal_closes"))
        .and_then(Value::as_bool)
        .unwrap_or_default();

    let notifications = root.get("notifications").and_then(Value::as_object);
    let notification_defaults = NotificationConfig::default();
    let notifications_enabled = notifications
        .and_then(|notifications| notifications.get("enabled"))
        .and_then(Value::as_bool)
        .unwrap_or(notification_defaults.enabled);
    let notification_sound = notifications
        .and_then(|notifications| notifications.get("sound"))
        .and_then(Value::as_str)
        .and_then(NotificationSound::from_str)
        .unwrap_or(notification_defaults.sound);

    let clipboard = root.get("clipboard").and_then(Value::as_object);
    let clipboard_defaults = ClipboardConfig::default();
    let copy_selection_to_clipboard = clipboard
        .and_then(|clipboard| clipboard.get("copy_selection_to_clipboard"))
        .and_then(Value::as_bool)
        .unwrap_or(clipboard_defaults.copy_selection_to_clipboard);

    let open_destination = root
        .get("links")
        .and_then(Value::as_object)
        .and_then(|links| links.get("open_destination"))
        .and_then(Value::as_str)
        .and_then(LinkOpenDestination::from_str)
        .unwrap_or_default();

    let font_size = root
        .get("font_size")
        .and_then(Value::as_f64)
        .map(|v| v as f32)
        .filter(|v| (1.0..=255.0).contains(v));

    AppConfig {
        focus: FocusConfig {
            hover_terminal_focus,
        },
        appearance: AppearanceConfig {
            color_scheme,
            ghostty_color_scheme,
            ui_scale,
            show_workspace_path,
        },
        workspace: WorkspaceConfig {
            keep_open_after_last_terminal_closes,
        },
        notifications: NotificationConfig {
            enabled: notifications_enabled,
            sound: notification_sound,
        },
        clipboard: ClipboardConfig {
            copy_selection_to_clipboard,
        },
        links: LinkConfig { open_destination },
        font_size,
    }
}

pub fn save(config: &AppConfig) -> Result<(), String> {
    let Some(path) = settings_path() else {
        return Err("config_dir unavailable; cannot save app settings".to_string());
    };

    save_to_path(&path, config)
        .map_err(|err| format!("failed to save app config `{}`: {err}", path.display()))
}

fn save_to_path(path: &Path, config: &AppConfig) -> Result<(), String> {
    let mut root = read_existing_config_root_for_save(path)?;

    let mut appearance = serde_json::Map::new();
    appearance.insert(
        "color_scheme".to_string(),
        json!(config.appearance.color_scheme.as_str()),
    );
    appearance.insert(
        "ghostty_color_scheme".to_string(),
        json!(config.appearance.ghostty_color_scheme.as_str()),
    );
    appearance.insert(
        "show_workspace_path".to_string(),
        json!(config.appearance.show_workspace_path),
    );
    if !config.appearance.ui_scale.is_default() {
        appearance.insert(
            "ui_scale".to_string(),
            json!(config.appearance.ui_scale.get()),
        );
    }
    root.insert("appearance".to_string(), Value::Object(appearance));
    root.insert(
        "focus".to_string(),
        json!({ "hover_terminal_focus": config.focus.hover_terminal_focus }),
    );
    root.insert(
        "workspace".to_string(),
        json!({
            "keep_open_after_last_terminal_closes": config
                .workspace
                .keep_open_after_last_terminal_closes,
        }),
    );
    root.insert(
        "notifications".to_string(),
        json!({
            "enabled": config.notifications.enabled,
            "sound": config.notifications.sound.as_str(),
        }),
    );
    root.insert(
        "clipboard".to_string(),
        json!({
            "copy_selection_to_clipboard": config.clipboard.copy_selection_to_clipboard,
        }),
    );
    root.insert(
        "links".to_string(),
        json!({ "open_destination": config.links.open_destination.as_str() }),
    );

    if let Some(size) = config.font_size {
        root.insert("font_size".to_string(), json!(size));
    } else {
        root.remove("font_size");
    }

    let serialized =
        serde_json::to_string_pretty(&Value::Object(root)).expect("config should serialize");
    write_config_root_atomically(path, &serialized)
}

fn read_existing_config_root_for_save(
    path: &Path,
) -> Result<serde_json::Map<String, Value>, String> {
    if !path.exists() {
        return Ok(serde_json::Map::new());
    }

    let raw = fs::read_to_string(path).map_err(|err| err.to_string())?;
    match serde_json::from_str::<Value>(&raw) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => {
            backup_invalid_existing_config(path)?;
            Ok(serde_json::Map::new())
        }
        Err(err) => {
            let detail = format!("existing app config is invalid JSON: {err}");
            backup_invalid_existing_config_with_detail(path, &detail)?;
            Ok(serde_json::Map::new())
        }
    }
}

fn backup_invalid_existing_config(path: &Path) -> Result<(), String> {
    backup_invalid_existing_config_with_detail(
        path,
        "existing app config root must be a JSON object",
    )
}

fn backup_invalid_existing_config_with_detail(path: &Path, detail: &str) -> Result<(), String> {
    let backup_path = invalid_config_backup_path(path);
    fs::rename(path, &backup_path).map_err(|err| {
        format!(
            "{detail}; failed to back up `{}` to `{}`: {err}",
            path.display(),
            backup_path.display()
        )
    })?;
    eprintln!(
        "limux: {detail}; backed up `{}` to `{}` before rewriting settings",
        path.display(),
        backup_path.display()
    );
    Ok(())
}

fn write_config_root_atomically(path: &Path, serialized: &str) -> Result<(), String> {
    let Some(parent) = path.parent() else {
        return Err("config path has no parent directory".to_string());
    };
    fs::create_dir_all(parent).map_err(|err| err.to_string())?;

    let temp_path = temp_config_path(path);
    fs::write(&temp_path, format!("{serialized}\n")).map_err(|err| err.to_string())?;

    if let Err(err) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(err.to_string());
    }

    Ok(())
}

fn temp_config_path(path: &Path) -> std::path::PathBuf {
    timestamped_sibling_path(path, "tmp")
}

fn invalid_config_backup_path(path: &Path) -> std::path::PathBuf {
    timestamped_sibling_path(path, "bak")
}

fn timestamped_sibling_path(path: &Path, suffix: &str) -> std::path::PathBuf {
    let stem = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("settings.json");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let file_name = format!(".{stem}.{suffix}-{}-{nonce}", std::process::id());
    path.with_file_name(file_name)
}

fn ensure_default_config_file(path: &Path) -> std::io::Result<()> {
    if path.exists() {
        return Ok(());
    }

    let Some(parent) = path.parent() else {
        return Ok(());
    };

    fs::create_dir_all(parent)?;
    let default_root = json!({
        "appearance": {
            "color_scheme": "dark",
            "ghostty_color_scheme": "dark",
            "show_workspace_path": true
        },
        "focus": {
            "hover_terminal_focus": false
        },
        "workspace": {
            "keep_open_after_last_terminal_closes": false
        },
        "notifications": {
            "enabled": true,
            "sound": "default"
        },
        "clipboard": {
            "copy_selection_to_clipboard": true
        },
        "links": {
            "open_destination": "default_browser"
        }
    });
    let serialized = serde_json::to_string_pretty(&default_root)
        .expect("default app config should always serialize");
    fs::write(path, format!("{serialized}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::ffi::OsString;

    use tempfile::TempDir;

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let previous = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(value) = self.previous.as_ref() {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn load_from_path_uses_defaults_when_file_is_missing() {
        let dir = TempDir::new().expect("temp dir");
        let path = settings_path_in(dir.path());

        let loaded = load_from_path(&path);

        assert_eq!(loaded, LoadedAppConfig::default());
    }

    #[test]
    fn settings_path_in_uses_limux_settings_json() {
        let path = settings_path_in(Path::new("/tmp/example"));

        assert_eq!(path, Path::new("/tmp/example/limux/settings.json"));
    }

    #[test]
    fn ensure_default_config_file_writes_default_sections() {
        let dir = TempDir::new().expect("temp dir");
        let path = settings_path_in(dir.path());

        ensure_default_config_file(&path).expect("write default config");

        let raw = fs::read_to_string(&path).expect("read config");
        let parsed: Value = serde_json::from_str(&raw).expect("parse config");
        assert_eq!(parsed["focus"]["hover_terminal_focus"], Value::Bool(false));
        assert_eq!(
            parsed["appearance"]["color_scheme"],
            Value::String("dark".to_string())
        );
        assert_eq!(
            parsed["appearance"]["ghostty_color_scheme"],
            Value::String("dark".to_string())
        );
        assert_eq!(
            parsed["appearance"]["show_workspace_path"],
            Value::Bool(true)
        );
        assert_eq!(
            parsed["workspace"]["keep_open_after_last_terminal_closes"],
            Value::Bool(false)
        );
        assert_eq!(parsed["notifications"]["enabled"], Value::Bool(true));
        assert_eq!(
            parsed["notifications"]["sound"],
            Value::String("default".to_string())
        );
        assert_eq!(
            parsed["clipboard"]["copy_selection_to_clipboard"],
            Value::Bool(true)
        );
        assert_eq!(
            parsed["links"]["open_destination"],
            Value::String("default_browser".to_string())
        );
    }

    #[test]
    fn load_from_path_reads_focus_settings_and_ignores_other_sections() {
        let dir = TempDir::new().expect("temp dir");
        let path = settings_path_in(dir.path());
        fs::create_dir_all(path.parent().expect("config dir")).expect("create config dir");
        fs::write(
            &path,
            r#"{
  "focus": {
    "hover_terminal_focus": true
  }
}
"#,
        )
        .expect("write config");

        let loaded = load_from_path(&path);

        assert!(loaded.warnings.is_empty());
        assert!(loaded.config.focus.hover_terminal_focus);
    }

    #[test]
    fn load_from_path_defaults_ghostty_scheme_to_gtk_scheme_for_legacy_configs() {
        let dir = TempDir::new().expect("temp dir");
        let path = settings_path_in(dir.path());
        fs::create_dir_all(path.parent().expect("config dir")).expect("create config dir");
        fs::write(
            &path,
            r#"{
  "appearance": {
    "color_scheme": "dark"
  }
}
"#,
        )
        .expect("write config");

        let loaded = load_from_path(&path);

        assert!(loaded.warnings.is_empty());
        assert_eq!(loaded.config.appearance.color_scheme, ColorScheme::Dark);
        assert_eq!(
            loaded.config.appearance.ghostty_color_scheme,
            ColorScheme::Dark
        );
        assert_eq!(loaded.config.appearance.ui_scale.get(), DEFAULT_UI_SCALE);
    }

    #[test]
    fn load_from_path_reads_and_clamps_ui_scale() {
        let dir = TempDir::new().expect("temp dir");
        let path = settings_path_in(dir.path());
        fs::create_dir_all(path.parent().expect("config dir")).expect("create config dir");
        fs::write(
            &path,
            r#"{
  "appearance": {
    "ui_scale": 3.0
  }
}
"#,
        )
        .expect("write config");

        let loaded = load_from_path(&path);

        assert!(loaded.warnings.is_empty());
        assert_eq!(loaded.config.appearance.ui_scale.get(), MAX_UI_SCALE);
    }

    #[test]
    fn load_from_path_reads_font_size_when_valid() {
        let dir = TempDir::new().expect("temp dir");
        let path = settings_path_in(dir.path());
        fs::create_dir_all(path.parent().expect("config dir")).expect("create config dir");
        fs::write(
            &path,
            r#"{
  "font_size": 18.5
}
"#,
        )
        .expect("write config");

        let loaded = load_from_path(&path);

        assert!(loaded.warnings.is_empty());
        assert_eq!(loaded.config.font_size, Some(18.5));
    }

    #[test]
    fn load_from_path_reads_notification_preferences() {
        let dir = TempDir::new().expect("temp dir");
        let path = settings_path_in(dir.path());
        fs::create_dir_all(path.parent().expect("config dir")).expect("create config dir");
        fs::write(
            &path,
            r#"{
  "notifications": {
    "enabled": false,
    "sound": "bell"
  }
}
"#,
        )
        .expect("write config");

        let loaded = load_from_path(&path);

        assert!(loaded.warnings.is_empty());
        assert!(!loaded.config.notifications.enabled);
        assert_eq!(loaded.config.notifications.sound, NotificationSound::Bell);
    }

    #[test]
    fn load_from_path_reads_clipboard_preferences() {
        let dir = TempDir::new().expect("temp dir");
        let path = settings_path_in(dir.path());
        fs::create_dir_all(path.parent().expect("config dir")).expect("create config dir");
        fs::write(
            &path,
            r#"{
  "clipboard": {
    "copy_selection_to_clipboard": false
  }
}
"#,
        )
        .expect("write config");

        let loaded = load_from_path(&path);

        assert!(loaded.warnings.is_empty());
        assert!(!loaded.config.clipboard.copy_selection_to_clipboard);
    }

    #[test]
    fn load_from_path_reads_link_destination_and_defaults_invalid_values() {
        let dir = TempDir::new().expect("temp dir");
        let path = settings_path_in(dir.path());
        fs::create_dir_all(path.parent().expect("config dir")).expect("create config dir");
        fs::write(
            &path,
            r#"{
  "links": {
    "open_destination": "browser_tab"
  }
}
"#,
        )
        .expect("write config");

        let loaded = load_from_path(&path);
        assert_eq!(
            loaded.config.links.open_destination,
            LinkOpenDestination::BrowserTab
        );

        fs::write(
            &path,
            r#"{
  "links": {
    "open_destination": "invalid"
  }
}
"#,
        )
        .expect("write invalid destination");

        let loaded = load_from_path(&path);
        assert_eq!(
            loaded.config.links.open_destination,
            LinkOpenDestination::DefaultBrowser
        );
    }

    #[test]
    fn link_destination_matches_embedded_browser_availability() {
        assert_eq!(
            LinkOpenDestination::BrowserTab.effective(true),
            LinkOpenDestination::BrowserTab
        );
        assert_eq!(
            LinkOpenDestination::BrowserTab.effective(false),
            LinkOpenDestination::DefaultBrowser
        );
        assert_eq!(
            LinkOpenDestination::DefaultBrowser.effective(false),
            LinkOpenDestination::DefaultBrowser
        );
    }

    #[test]
    fn load_from_path_reads_workspace_path_visibility() {
        let dir = TempDir::new().expect("temp dir");
        let path = settings_path_in(dir.path());
        fs::create_dir_all(path.parent().expect("config dir")).expect("create config dir");
        fs::write(
            &path,
            r#"{
  "appearance": {
    "show_workspace_path": false
  }
}
"#,
        )
        .expect("write config");

        let loaded = load_from_path(&path);

        assert!(loaded.warnings.is_empty());
        assert!(!loaded.config.appearance.show_workspace_path);
    }

    #[test]
    fn load_from_path_reads_workspace_lifecycle_preference() {
        let dir = TempDir::new().expect("temp dir");
        let path = settings_path_in(dir.path());
        fs::create_dir_all(path.parent().expect("config dir")).expect("create config dir");
        fs::write(
            &path,
            r#"{
  "workspace": {
    "keep_open_after_last_terminal_closes": true
  }
}
"#,
        )
        .expect("write config");

        let loaded = load_from_path(&path);

        assert!(loaded.warnings.is_empty());
        assert!(loaded.config.workspace.keep_open_after_last_terminal_closes);
    }

    #[test]
    fn save_writes_gtk_and_ghostty_color_schemes() {
        let dir = TempDir::new().expect("temp dir");
        let path = settings_path_in(dir.path());
        fs::create_dir_all(path.parent().expect("config dir")).expect("create config dir");
        let _env_guard = EnvVarGuard::set("XDG_CONFIG_HOME", dir.path());

        let mut config = AppConfig::default();
        config.appearance.color_scheme = ColorScheme::Light;
        config.appearance.ghostty_color_scheme = ColorScheme::Dark;
        save(&config).expect("save config");

        let raw = fs::read_to_string(&path).expect("read config");
        let parsed: Value = serde_json::from_str(&raw).expect("parse config");
        assert_eq!(
            parsed["appearance"]["color_scheme"],
            Value::String("light".to_string())
        );
        assert_eq!(
            parsed["appearance"]["ghostty_color_scheme"],
            Value::String("dark".to_string())
        );
        assert!(parsed["appearance"].get("ui_scale").is_none());
    }

    #[test]
    fn save_to_path_writes_non_default_ui_scale() {
        let dir = TempDir::new().expect("temp dir");
        let path = settings_path_in(dir.path());
        fs::create_dir_all(path.parent().expect("config dir")).expect("create config dir");

        let mut config = AppConfig::default();
        config.appearance.ui_scale = UiScale::new(1.5).expect("valid scale");
        save_to_path(&path, &config).expect("save config");

        let raw = fs::read_to_string(&path).expect("read config");
        let parsed: Value = serde_json::from_str(&raw).expect("parse config");
        assert_eq!(parsed["appearance"]["ui_scale"], json!(1.5));
    }

    #[test]
    fn save_to_path_writes_workspace_path_visibility() {
        let dir = TempDir::new().expect("temp dir");
        let path = settings_path_in(dir.path());
        fs::create_dir_all(path.parent().expect("config dir")).expect("create config dir");

        let mut config = AppConfig::default();
        config.appearance.show_workspace_path = false;
        save_to_path(&path, &config).expect("save workspace path visibility");

        let raw = fs::read_to_string(&path).expect("read config");
        let parsed: Value = serde_json::from_str(&raw).expect("parse config");
        assert_eq!(
            parsed["appearance"]["show_workspace_path"],
            Value::Bool(false)
        );
    }

    #[test]
    fn save_to_path_writes_workspace_lifecycle_preference() {
        let dir = TempDir::new().expect("temp dir");
        let path = settings_path_in(dir.path());
        fs::create_dir_all(path.parent().expect("config dir")).expect("create config dir");

        let mut config = AppConfig::default();
        config.workspace.keep_open_after_last_terminal_closes = true;
        save_to_path(&path, &config).expect("save workspace lifecycle preference");

        let raw = fs::read_to_string(&path).expect("read config");
        let parsed: Value = serde_json::from_str(&raw).expect("parse config");
        assert_eq!(
            parsed["workspace"]["keep_open_after_last_terminal_closes"],
            Value::Bool(true)
        );
    }

    #[test]
    fn save_preserves_unrelated_top_level_keys() {
        let dir = TempDir::new().expect("temp dir");
        let path = settings_path_in(dir.path());
        fs::create_dir_all(path.parent().expect("config dir")).expect("create config dir");
        fs::write(
            &path,
            r#"{
  "custom": {
    "keep": true
  },
  "focus": {
    "hover_terminal_focus": false
  }
}
"#,
        )
        .expect("write config");

        let mut config = AppConfig::default();
        config.appearance.color_scheme = ColorScheme::Dark;
        save_to_path(&path, &config).expect("save config");

        let raw = fs::read_to_string(&path).expect("read config");
        let parsed: Value = serde_json::from_str(&raw).expect("parse config");
        assert_eq!(parsed["custom"]["keep"], Value::Bool(true));
        assert_eq!(
            parsed["appearance"]["color_scheme"],
            Value::String("dark".to_string())
        );
    }

    #[test]
    fn save_to_path_writes_and_clears_font_size() {
        let dir = TempDir::new().expect("temp dir");
        let path = settings_path_in(dir.path());
        fs::create_dir_all(path.parent().expect("config dir")).expect("create config dir");

        let mut config = AppConfig {
            font_size: Some(16.25),
            ..AppConfig::default()
        };
        save_to_path(&path, &config).expect("save font size");

        let raw = fs::read_to_string(&path).expect("read config");
        let parsed: Value = serde_json::from_str(&raw).expect("parse config");
        assert_eq!(parsed["font_size"], json!(16.25));

        config.font_size = None;
        save_to_path(&path, &config).expect("clear font size");

        let raw = fs::read_to_string(&path).expect("read cleared config");
        let parsed: Value = serde_json::from_str(&raw).expect("parse cleared config");
        assert!(parsed.get("font_size").is_none());
    }

    #[test]
    fn save_to_path_writes_notification_preferences() {
        let dir = TempDir::new().expect("temp dir");
        let path = settings_path_in(dir.path());
        fs::create_dir_all(path.parent().expect("config dir")).expect("create config dir");

        let mut config = AppConfig::default();
        config.notifications.enabled = false;
        config.notifications.sound = NotificationSound::Alert;
        save_to_path(&path, &config).expect("save notifications");

        let raw = fs::read_to_string(&path).expect("read config");
        let parsed: Value = serde_json::from_str(&raw).expect("parse config");
        assert_eq!(parsed["notifications"]["enabled"], Value::Bool(false));
        assert_eq!(
            parsed["notifications"]["sound"],
            Value::String("alert".to_string())
        );
    }

    #[test]
    fn save_to_path_writes_clipboard_preferences() {
        let dir = TempDir::new().expect("temp dir");
        let path = settings_path_in(dir.path());
        fs::create_dir_all(path.parent().expect("config dir")).expect("create config dir");

        let mut config = AppConfig::default();
        config.clipboard.copy_selection_to_clipboard = false;
        save_to_path(&path, &config).expect("save clipboard");

        let raw = fs::read_to_string(&path).expect("read config");
        let parsed: Value = serde_json::from_str(&raw).expect("parse config");
        assert_eq!(
            parsed["clipboard"]["copy_selection_to_clipboard"],
            Value::Bool(false)
        );
    }

    #[test]
    fn save_to_path_writes_link_destination() {
        let dir = TempDir::new().expect("temp dir");
        let path = settings_path_in(dir.path());
        fs::create_dir_all(path.parent().expect("config dir")).expect("create config dir");

        let mut config = AppConfig::default();
        config.links.open_destination = LinkOpenDestination::BrowserTab;
        save_to_path(&path, &config).expect("save link destination");

        let raw = fs::read_to_string(&path).expect("read config");
        let parsed: Value = serde_json::from_str(&raw).expect("parse config");
        assert_eq!(
            parsed["links"]["open_destination"],
            Value::String("browser_tab".to_string())
        );
    }

    #[test]
    fn notification_sound_maps_supported_freedesktop_events() {
        assert_eq!(
            NotificationSound::Message.freedesktop_sound_name(),
            Some("message-new-instant")
        );
        assert_eq!(
            NotificationSound::Bell.freedesktop_sound_name(),
            Some("bell-terminal")
        );
        assert_eq!(
            NotificationSound::Complete.freedesktop_sound_name(),
            Some("complete")
        );
        assert_eq!(
            NotificationSound::Alert.freedesktop_sound_name(),
            Some("dialog-warning")
        );
        assert_eq!(NotificationSound::Default.freedesktop_sound_name(), None);
        assert_eq!(NotificationSound::None.freedesktop_sound_name(), None);
    }

    #[test]
    fn save_to_path_recovers_invalid_existing_json_by_backing_it_up() {
        let dir = TempDir::new().expect("temp dir");
        let path = settings_path_in(dir.path());
        fs::create_dir_all(path.parent().expect("config dir")).expect("create config dir");
        fs::write(&path, "not json").expect("write invalid config");

        let config = AppConfig::default();
        save_to_path(&path, &config).expect("save should recover");

        let raw = fs::read_to_string(&path).expect("read repaired config");
        let parsed: Value = serde_json::from_str(&raw).expect("parse repaired config");
        assert_eq!(
            parsed["appearance"]["color_scheme"],
            Value::String("system".to_string())
        );

        let backup = fs::read_dir(path.parent().expect("config dir"))
            .expect("list config dir")
            .find_map(|entry| {
                let entry = entry.expect("dir entry");
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.contains(".settings.json.bak-").then_some(entry.path())
            })
            .expect("backup file");
        assert_eq!(
            fs::read_to_string(backup).expect("read backup config"),
            "not json"
        );
    }

    #[test]
    fn load_from_path_falls_back_to_defaults_on_invalid_json() {
        let dir = TempDir::new().expect("temp dir");
        let path = settings_path_in(dir.path());
        fs::create_dir_all(path.parent().expect("config dir")).expect("create config dir");
        fs::write(&path, "not json").expect("write config");

        let loaded = load_from_path(&path);

        assert_eq!(loaded.config, AppConfig::default());
        assert_eq!(loaded.warnings.len(), 1);
        assert!(loaded.warnings[0].contains("failed to load app config"));
    }
}
