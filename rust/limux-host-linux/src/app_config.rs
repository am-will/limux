use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::gui_hooks::GuiHooksConfig;
use crate::shortcut_config;
use crate::terminal_font::TerminalFontConfig;

pub const SETTINGS_FILE_NAME: &str = "settings.json";
pub const DEFAULT_UI_SCALE: f32 = 1.0;
pub const MIN_UI_SCALE: f32 = 0.8;
pub const MAX_UI_SCALE: f32 = 2.0;
pub const DEFAULT_TAB_ACTIVITY_SECONDS: u32 = 5;
pub const MIN_TAB_ACTIVITY_SECONDS: u32 = 1;
pub const MAX_TAB_ACTIVITY_SECONDS: u32 = 60;

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WindowControlsSide {
    Left,
    #[default]
    Right,
}

impl WindowControlsSide {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
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
    pub interface: InterfaceConfig,
    #[serde(skip)]
    pub links: LinkConfig,
    #[serde(skip)]
    pub font_size: Option<f32>,
    #[serde(skip)]
    pub terminal_font: TerminalFontConfig,
    #[serde(skip)]
    pub hooks: GuiHooksConfig,
}

impl AppConfig {
    pub fn effective_font_size(&self, default_size: f32) -> f32 {
        self.font_size
            .or(self.terminal_font.size)
            .unwrap_or(default_size)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceConfig {
    pub keep_open_after_last_terminal_closes: bool,
    pub prompt_for_folder_on_create: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UnreadColor {
    #[default]
    Amber,
    Emerald,
    Rose,
    Cyan,
    AccentBlue,
}

impl UnreadColor {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Amber => "amber",
            Self::Emerald => "emerald",
            Self::Rose => "rose",
            Self::Cyan => "cyan",
            Self::AccentBlue => "accent_blue",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "amber" => Some(Self::Amber),
            "emerald" => Some(Self::Emerald),
            "rose" => Some(Self::Rose),
            "cyan" => Some(Self::Cyan),
            "accent_blue" => Some(Self::AccentBlue),
            _ => None,
        }
    }

    pub fn css_color(self) -> &'static str {
        match self {
            Self::Amber => "#f59e0b",
            Self::Emerald => "#10b981",
            Self::Rose => "#f43f5e",
            Self::Cyan => "#06b6d4",
            Self::AccentBlue => "@accent_bg_color",
        }
    }

    pub fn labels() -> &'static [&'static str] {
        &["Amber (Default)", "Emerald", "Rose", "Cyan", "Accent Blue"]
    }

    pub fn dropdown_index(self) -> u32 {
        match self {
            Self::Amber => 0,
            Self::Emerald => 1,
            Self::Rose => 2,
            Self::Cyan => 3,
            Self::AccentBlue => 4,
        }
    }

    pub fn from_dropdown_index(index: u32) -> Self {
        match index {
            1 => Self::Emerald,
            2 => Self::Rose,
            3 => Self::Cyan,
            4 => Self::AccentBlue,
            _ => Self::Amber,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppearanceConfig {
    pub color_scheme: ColorScheme,
    pub ghostty_color_scheme: ColorScheme,
    pub ui_scale: UiScale,
    pub show_workspace_path: bool,
    pub unread_color: UnreadColor,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            color_scheme: ColorScheme::default(),
            ghostty_color_scheme: ColorScheme::default(),
            ui_scale: UiScale::default(),
            show_workspace_path: true,
            unread_color: UnreadColor::default(),
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterfaceConfig {
    pub window_controls_side: WindowControlsSide,
    pub show_top_bar: bool,
    pub show_workspace_indicators: bool,
    pub show_tab_activity: bool,
    pub tab_activity_seconds: u32,
}

impl Default for InterfaceConfig {
    fn default() -> Self {
        Self {
            window_controls_side: WindowControlsSide::default(),
            show_top_bar: true,
            show_workspace_indicators: true,
            show_tab_activity: true,
            tab_activity_seconds: DEFAULT_TAB_ACTIVITY_SECONDS,
        }
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

    let unread_color = appearance
        .and_then(|appearance| appearance.get("unread_color"))
        .and_then(Value::as_str)
        .and_then(UnreadColor::from_str)
        .unwrap_or_default();

    let workspace = root.get("workspace").and_then(Value::as_object);
    let keep_open_after_last_terminal_closes = workspace
        .and_then(|workspace| workspace.get("keep_open_after_last_terminal_closes"))
        .and_then(Value::as_bool)
        .unwrap_or_default();
    let prompt_for_folder_on_create = workspace
        .and_then(|workspace| workspace.get("prompt_for_folder_on_create"))
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

    let interface_obj = root.get("interface").and_then(Value::as_object);

    let window_controls_side = interface_obj
        .and_then(|interface| interface.get("window_controls_side"))
        .and_then(Value::as_str)
        .and_then(WindowControlsSide::from_str)
        .unwrap_or_default();

    let show_top_bar = interface_obj
        .and_then(|interface| interface.get("show_top_bar"))
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let show_workspace_indicators = interface_obj
        .and_then(|interface| interface.get("show_workspace_indicators"))
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let show_tab_activity = interface_obj
        .and_then(|interface| interface.get("show_tab_activity"))
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let tab_activity_seconds = interface_obj
        .and_then(|interface| interface.get("tab_activity_seconds"))
        .and_then(Value::as_u64)
        .filter(|seconds| {
            (u64::from(MIN_TAB_ACTIVITY_SECONDS)..=u64::from(MAX_TAB_ACTIVITY_SECONDS))
                .contains(seconds)
        })
        .map(|seconds| seconds as u32)
        .unwrap_or(DEFAULT_TAB_ACTIVITY_SECONDS);

    AppConfig {
        focus: FocusConfig {
            hover_terminal_focus,
        },
        appearance: AppearanceConfig {
            color_scheme,
            ghostty_color_scheme,
            ui_scale,
            show_workspace_path,
            unread_color,
        },
        workspace: WorkspaceConfig {
            keep_open_after_last_terminal_closes,
            prompt_for_folder_on_create,
        },
        notifications: NotificationConfig {
            enabled: notifications_enabled,
            sound: notification_sound,
        },
        clipboard: ClipboardConfig {
            copy_selection_to_clipboard,
        },
        interface: InterfaceConfig {
            window_controls_side,
            show_top_bar,
            show_workspace_indicators,
            show_tab_activity,
            tab_activity_seconds,
        },
        links: LinkConfig { open_destination },
        font_size,
        terminal_font: TerminalFontConfig::from_json(root.get("terminal_font")),
        hooks: GuiHooksConfig::from_value(root.get("hooks")),
    }
}

/// Save what changed from `previous` to `updated`. Everything else in the file
/// stays as it is, because another instance or the user can have written it.
pub fn save(previous: &AppConfig, updated: &AppConfig) -> Result<(), String> {
    let Some(path) = settings_path() else {
        return Err("config_dir unavailable; cannot save app settings".to_string());
    };

    save_to_path(&path, previous, updated)
        .map_err(|err| format!("failed to save app config `{}`: {err}", path.display()))
}

fn save_to_path(path: &Path, previous: &AppConfig, updated: &AppConfig) -> Result<(), String> {
    let updated = config_to_map(updated);
    let root = match read_existing_config_root_for_save(path)? {
        Some(mut root) => {
            let mut previous = config_to_map(previous);
            // Without a value of its own, the Ghostty scheme follows the GTK
            // scheme. Store it, so that a new GTK scheme does not change it.
            if value_at(&root, "appearance", "ghostty_color_scheme").is_none() {
                if let Some(Value::Object(appearance)) = previous.get_mut("appearance") {
                    appearance.remove("ghostty_color_scheme");
                }
            }
            merge_changes(&mut root, &previous, &updated);
            root
        }
        None => updated,
    };

    let serialized =
        serde_json::to_string_pretty(&Value::Object(root)).expect("config should serialize");
    write_config_root_atomically(path, &serialized)
}

fn value_at<'a>(root: &'a Map<String, Value>, section: &str, key: &str) -> Option<&'a Value> {
    root.get(section)?.get(key)
}

/// Apply the differences between `previous` and `updated` to `target`.
fn merge_changes(
    target: &mut Map<String, Value>,
    previous: &Map<String, Value>,
    updated: &Map<String, Value>,
) {
    let empty = Map::new();
    let keys: BTreeSet<&String> = previous.keys().chain(updated.keys()).collect();
    for key in keys {
        let (before, after) = (previous.get(key), updated.get(key));
        if before == after {
            continue;
        }
        let is_section = before.or(after).is_some_and(Value::is_object);
        if !is_section {
            match after {
                Some(value) => target.insert(key.clone(), value.clone()),
                None => target.remove(key),
            };
            continue;
        }

        let mut section = match target.remove(key) {
            Some(Value::Object(section)) => section,
            _ => Map::new(),
        };
        merge_changes(
            &mut section,
            before.and_then(Value::as_object).unwrap_or(&empty),
            after.and_then(Value::as_object).unwrap_or(&empty),
        );
        if !section.is_empty() {
            target.insert(key.clone(), Value::Object(section));
        }
    }
}

fn config_to_map(config: &AppConfig) -> Map<String, Value> {
    let mut root = Map::new();
    let mut appearance = Map::new();
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
    if config.appearance.unread_color != UnreadColor::default() {
        appearance.insert(
            "unread_color".to_string(),
            json!(config.appearance.unread_color.as_str()),
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
            "prompt_for_folder_on_create": config
                .workspace
                .prompt_for_folder_on_create,
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
        "interface".to_string(),
        json!({
            "window_controls_side": config.interface.window_controls_side.as_str(),
            "show_top_bar": config.interface.show_top_bar,
            "show_workspace_indicators": config.interface.show_workspace_indicators,
            "show_tab_activity": config.interface.show_tab_activity,
            "tab_activity_seconds": config.interface.tab_activity_seconds,
        }),
    );
    root.insert(
        "links".to_string(),
        json!({ "open_destination": config.links.open_destination.as_str() }),
    );

    if let Some(size) = config.font_size {
        root.insert("font_size".to_string(), json!(size));
    }

    if let Some(terminal_font) = config.terminal_font.to_json() {
        root.insert("terminal_font".to_string(), terminal_font);
    }

    if let Some(hooks) = config.hooks.to_value() {
        root.insert("hooks".to_string(), hooks);
    }

    root
}

/// `None` when there is no usable file to keep settings from.
fn read_existing_config_root_for_save(path: &Path) -> Result<Option<Map<String, Value>>, String> {
    if !path.exists() {
        return Ok(None);
    }

    let raw = fs::read_to_string(path).map_err(|err| err.to_string())?;
    match serde_json::from_str::<Value>(&raw) {
        Ok(Value::Object(map)) => Ok(Some(map)),
        Ok(_) => {
            backup_invalid_existing_config(path)?;
            Ok(None)
        }
        Err(err) => {
            let detail = format!("existing app config is invalid JSON: {err}");
            backup_invalid_existing_config_with_detail(path, &detail)?;
            Ok(None)
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
    // A copy, because the path can be a symlink to the file to keep.
    fs::copy(path, &backup_path).map_err(|err| {
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

    // Keep a symlink and replace the file it points to, unless that place is
    // read-only.
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    replace_file(&target, serialized)
        .or_else(|err| {
            if target == path {
                return Err(err);
            }
            replace_file(path, serialized)
        })
        .map_err(|err| err.to_string())
}

fn replace_file(path: &Path, serialized: &str) -> io::Result<()> {
    let temp_path = temp_config_path(path);
    let permissions = fs::symlink_metadata(path)
        .ok()
        .filter(|meta| meta.is_file())
        .map(|meta| meta.permissions());
    let write = || {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(permissions.as_ref().map_or(0o666, |p| p.mode()))
            .open(&temp_path)?;
        file.write_all(format!("{serialized}\n").as_bytes())?;
        if let Some(permissions) = permissions {
            // The umask can have removed some of these.
            file.set_permissions(permissions)?;
        }
        fs::rename(&temp_path, path)
    };
    write().inspect_err(|_| {
        let _ = fs::remove_file(&temp_path);
    })
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
            "keep_open_after_last_terminal_closes": false,
            "prompt_for_folder_on_create": false
        },
        "notifications": {
            "enabled": true,
            "sound": "default"
        },
        "clipboard": {
            "copy_selection_to_clipboard": true
        },
        "interface": {
            "window_controls_side": "right",
            "show_top_bar": true,
            "show_workspace_indicators": true,
            "show_tab_activity": true,
            "tab_activity_seconds": 5
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

    /// Save `config` the way an instance that loaded the file would.
    fn save_over(path: &Path, config: &AppConfig) -> Result<(), String> {
        save_to_path(path, &load_from_path(path).config, config)
    }

    #[test]
    fn interface_preferences_round_trip_without_losing_link_settings() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("settings.json");
        for side in [WindowControlsSide::Left, WindowControlsSide::Right] {
            let config = AppConfig {
                interface: InterfaceConfig {
                    window_controls_side: side,
                    show_top_bar: false,
                    show_workspace_indicators: false,
                    show_tab_activity: false,
                    tab_activity_seconds: 12,
                },
                links: LinkConfig {
                    open_destination: LinkOpenDestination::BrowserTab,
                },
                ..Default::default()
            };

            save_over(&path, &config).expect("save config");
            let loaded = load_from_path(&path);
            assert!(loaded.warnings.is_empty());
            assert_eq!(loaded.config, config);
        }
    }

    #[test]
    fn gui_hooks_round_trip_and_clearing_them_removes_the_key() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("settings.json");
        let mut config = AppConfig {
            hooks: GuiHooksConfig {
                on_workspace_delete: crate::gui_hooks::GuiHook {
                    enabled: false,
                    command: "cleanup.sh -y".to_string(),
                    show_terminal: true,
                    suppress_default: true,
                },
                ..Default::default()
            },
            ..Default::default()
        };

        save_over(&path, &config).expect("save config");
        assert_eq!(load_from_path(&path).config, config);

        config.hooks = GuiHooksConfig::default();
        save_over(&path, &config).expect("save config");
        let root: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(root.get("hooks").is_none(), "{root}");
    }

    #[test]
    fn missing_and_invalid_interface_preferences_use_defaults() {
        for root in [
            json!({}),
            json!({"interface": {
                "window_controls_side": "unknown",
                "show_top_bar": "false",
                "show_workspace_indicators": null,
                "show_tab_activity": null,
                "tab_activity_seconds": 0
            }}),
        ] {
            assert_eq!(
                parse_app_config_value(&root).interface,
                InterfaceConfig::default()
            );
        }
    }

    #[test]
    fn tab_activity_timeout_accepts_only_integer_values_in_range() {
        for seconds in [1, DEFAULT_TAB_ACTIVITY_SECONDS, 60] {
            let root = json!({ "interface": { "tab_activity_seconds": seconds } });
            assert_eq!(
                parse_app_config_value(&root).interface.tab_activity_seconds,
                seconds
            );
        }

        for invalid in [
            json!(0),
            json!(61),
            json!(-1),
            json!(5.5),
            json!("5"),
            json!(null),
        ] {
            let root = json!({ "interface": { "tab_activity_seconds": invalid } });
            assert_eq!(
                parse_app_config_value(&root).interface.tab_activity_seconds,
                DEFAULT_TAB_ACTIVITY_SECONDS,
                "invalid timeout should use the default: {root}"
            );
        }
    }

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
        assert_eq!(parsed["interface"]["show_tab_activity"], Value::Bool(true));
        assert_eq!(parsed["interface"]["tab_activity_seconds"], Value::from(5));
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
    fn save_and_load_round_trips_unread_color() {
        let dir = TempDir::new().expect("temp dir");
        let path = settings_path_in(dir.path());
        let mut config = AppConfig::default();
        assert_eq!(config.appearance.unread_color, UnreadColor::Amber);

        for color in [
            UnreadColor::Amber,
            UnreadColor::Emerald,
            UnreadColor::Rose,
            UnreadColor::Cyan,
            UnreadColor::AccentBlue,
        ] {
            config.appearance.unread_color = color;
            save_over(&path, &config).expect("save config");
            let loaded = load_from_path(&path);
            assert!(loaded.warnings.is_empty());
            assert_eq!(loaded.config.appearance.unread_color, color);
        }
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
        save(&AppConfig::default(), &config).expect("save config");

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
        save_over(&path, &config).expect("save config");

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
        save_over(&path, &config).expect("save workspace path visibility");

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
        save_over(&path, &config).expect("save workspace lifecycle preference");

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
        save_over(&path, &config).expect("save config");

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
        save_over(&path, &config).expect("save font size");

        let raw = fs::read_to_string(&path).expect("read config");
        let parsed: Value = serde_json::from_str(&raw).expect("parse config");
        assert_eq!(parsed["font_size"], json!(16.25));

        config.font_size = None;
        save_over(&path, &config).expect("clear font size");

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
        save_over(&path, &config).expect("save notifications");

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
        save_over(&path, &config).expect("save clipboard");

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
        save_over(&path, &config).expect("save link destination");

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
        save_over(&path, &config).expect("save should recover");

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

    #[test]
    fn save_and_load_roundtrips_terminal_font_and_clears_when_default() {
        let dir = TempDir::new().expect("temp dir");
        let path = settings_path_in(dir.path());
        fs::create_dir_all(path.parent().expect("config dir")).expect("create config dir");

        let mut config = AppConfig {
            terminal_font: TerminalFontConfig {
                family: Some("MesloLGS NF".to_string()),
                style: Some("Regular".to_string()),
                size: Some(13.0),
                cell_width_percent: Some(105),
                cell_height_percent: Some(110),
                use_ligatures: Some(false),
                anti_aliased: Some(true),
            },
            ..AppConfig::default()
        };
        save_over(&path, &config).expect("save terminal font");

        let loaded = load_from_path(&path);
        assert!(loaded.warnings.is_empty());
        assert_eq!(loaded.config.terminal_font, config.terminal_font);
        assert_eq!(loaded.config.effective_font_size(12.0), 13.0);

        config.terminal_font = TerminalFontConfig::default();
        save_over(&path, &config).expect("clear terminal font");
        let parsed: Value =
            serde_json::from_str(&fs::read_to_string(&path).expect("read cleared config"))
                .expect("parse cleared config");
        assert!(parsed.get("terminal_font").is_none());
    }

    fn saved_root(path: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(path).expect("read config")).expect("parse config")
    }

    #[test]
    fn saving_keeps_what_another_instance_or_the_user_wrote() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("settings.json");
        fs::write(
            &path,
            r#"{
  "appearance": { "color_scheme": "dark", "ghostty_color_scheme": "dark", "newer": 1 },
  "font_size": 15.0,
  "hooks": { "on_workspace_create": { "command": "create.sh" } },
  "newer": { "keep": true }
}"#,
        )
        .expect("write config");
        let first = load_from_path(&path).config;
        let second = first.clone();

        let mut updated = first.clone();
        updated.appearance.color_scheme = ColorScheme::Light;
        updated.font_size = None;
        updated.hooks.on_workspace_delete.command = "delete.sh".to_string();
        save_to_path(&path, &first, &updated).expect("save from the first instance");

        let mut zoomed = second.clone();
        zoomed.interface.show_top_bar = false;
        zoomed.hooks.after_workspace_create.command = "after.sh".to_string();
        save_to_path(&path, &second, &zoomed).expect("save from the second instance");

        let root = saved_root(&path);
        assert_eq!(root["appearance"]["color_scheme"], "light");
        assert_eq!(root["appearance"]["newer"], 1);
        assert_eq!(root["newer"]["keep"], true);
        assert_eq!(root["interface"]["show_top_bar"], false);
        assert!(root.get("font_size").is_none(), "{root}");
        for (hook, command) in [
            ("on_workspace_create", "create.sh"),
            ("on_workspace_delete", "delete.sh"),
            ("after_workspace_create", "after.sh"),
        ] {
            assert_eq!(root["hooks"][hook]["command"], command);
        }
    }

    #[test]
    fn saving_a_gtk_scheme_keeps_the_ghostty_scheme_that_followed_it() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("settings.json");
        fs::write(&path, r#"{"appearance": {"color_scheme": "dark"}}"#).expect("write config");
        let previous = load_from_path(&path).config;
        let mut updated = previous.clone();
        updated.appearance.color_scheme = ColorScheme::Light;
        save_to_path(&path, &previous, &updated).expect("save color scheme");

        assert_eq!(load_from_path(&path).config, updated);
    }

    #[test]
    fn saving_follows_a_symlink_and_keeps_the_file_mode() {
        let dir = TempDir::new().expect("temp dir");
        let dotfiles = dir.path().join("dotfiles");
        fs::create_dir(&dotfiles).expect("create dotfiles dir");
        let real = dotfiles.join("limux.json");
        let path = dir.path().join("settings.json");
        fs::write(&real, "{}").expect("write config");
        fs::set_permissions(&real, fs::Permissions::from_mode(0o640)).expect("set mode");
        std::os::unix::fs::symlink(&real, &path).expect("link config");

        let mut config = AppConfig::default();
        for (contents, size) in [("{}", 14.0), ("broken", 15.0)] {
            fs::write(&real, contents).expect("write config");
            config.font_size = Some(size);
            save_over(&path, &config).expect("save config");

            assert!(path.symlink_metadata().expect("link").is_symlink());
            assert_eq!(saved_root(&real)["font_size"], json!(size));
            let mode = fs::metadata(&real).expect("config").permissions().mode();
            assert_eq!(mode & 0o777, 0o640);
        }
        assert_eq!(fs::read_dir(&dotfiles).expect("list dotfiles").count(), 1);
    }
}
