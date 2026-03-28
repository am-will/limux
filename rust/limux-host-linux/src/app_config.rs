use std::fs;
use std::path::Path;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::shortcut_config;

pub const SETTINGS_FILE_NAME: &str = "settings.json";

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

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub focus: FocusConfig,
    #[serde(skip)]
    pub appearance: AppearanceConfig,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppearanceConfig {
    pub color_scheme: ColorScheme,
    pub ghostty_color_scheme: ColorScheme,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct FocusConfig {
    #[serde(default)]
    pub hover_terminal_focus: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
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

    AppConfig {
        focus: FocusConfig {
            hover_terminal_focus,
        },
        appearance: AppearanceConfig {
            color_scheme,
            ghostty_color_scheme,
        },
    }
}

pub fn save(config: &AppConfig) {
    let Some(path) = settings_path() else {
        eprintln!("limux: config_dir unavailable; cannot save app settings");
        return;
    };

    let mut root = match fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str::<Value>(&raw)
            .ok()
            .and_then(|value| match value {
                Value::Object(map) => Some(map),
                _ => None,
            })
            .unwrap_or_default(),
        Err(_) => serde_json::Map::new(),
    };

    root.insert(
        "appearance".to_string(),
        json!({
            "color_scheme": config.appearance.color_scheme.as_str(),
            "ghostty_color_scheme": config.appearance.ghostty_color_scheme.as_str(),
        }),
    );
    root.insert(
        "focus".to_string(),
        json!({ "hover_terminal_focus": config.focus.hover_terminal_focus }),
    );

    let serialized =
        serde_json::to_string_pretty(&Value::Object(root)).expect("config should serialize");
    if let Err(err) = fs::write(&path, format!("{serialized}\n")) {
        eprintln!(
            "limux: failed to save app config `{}`: {err}",
            path.display()
        );
    }
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
            "color_scheme": "system",
            "ghostty_color_scheme": "system"
        },
        "focus": {
            "hover_terminal_focus": false
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
    fn ensure_default_config_file_writes_opt_in_false_setting() {
        let dir = TempDir::new().expect("temp dir");
        let path = settings_path_in(dir.path());

        ensure_default_config_file(&path).expect("write default config");

        let raw = fs::read_to_string(&path).expect("read config");
        let parsed: Value = serde_json::from_str(&raw).expect("parse config");
        assert_eq!(parsed["focus"]["hover_terminal_focus"], Value::Bool(false));
        assert_eq!(
            parsed["appearance"]["ghostty_color_scheme"],
            Value::String("system".to_string())
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
        save(&config);

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
