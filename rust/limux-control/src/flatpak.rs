//! Host-visible entry points for the installed Flatpak package.

use std::path::{Path, PathBuf};

const APP_ID: &str = "dev.limux.linux";
const HOST_CLI: &str = "libexec/limux/limux-host-cli";

pub fn host_cli_executable() -> Option<PathBuf> {
    if std::env::var_os("FLATPAK_ID").is_none_or(|id| id != APP_ID) {
        return None;
    }
    host_cli_from_info(&std::fs::read_to_string("/.flatpak-info").ok()?)
}

fn host_cli_from_info(contents: &str) -> Option<PathBuf> {
    let files = PathBuf::from(key_file_value(contents, "Instance", "app-path")?);
    if !files.is_absolute() {
        return None;
    }
    // Deployment paths contain a commit ID. Hooks must use the branch's
    // active symlink so that a later update and garbage collection are safe.
    let app_dir = files.ancestors().nth(4);
    if files.file_name().is_some_and(|name| name == "files")
        && app_dir
            .and_then(Path::file_name)
            .is_some_and(|name| name == APP_ID)
        && app_dir
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .is_some_and(|name| name == "app")
    {
        return Some(
            files
                .ancestors()
                .nth(2)?
                .join("active/files")
                .join(HOST_CLI),
        );
    }
    Some(files.join(HOST_CLI))
}

fn key_file_value(contents: &str, section: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    let mut result = None;
    for line in contents.lines().map(str::trim) {
        if line.starts_with('[') {
            in_section = line.strip_prefix('[')?.strip_suffix(']')? == section;
        } else if in_section {
            if let Some((name, value)) = line.split_once('=') {
                if name.trim() == key {
                    result = Some(unescape_key_file(value.trim())?);
                }
            }
        }
    }
    result
}

fn unescape_key_file(value: &str) -> Option<String> {
    let mut result = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        result.push(if ch == '\\' {
            match chars.next()? {
                's' => ' ',
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                '\\' => '\\',
                _ => return None,
            }
        } else {
            ch
        });
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::{host_cli_from_info, key_file_value};
    use std::path::Path;

    #[test]
    fn launcher_uses_a_stable_deployment_path_across_updates() {
        for commit in ["old-commit", "new-commit"] {
            let info = format!("[Instance]\napp-path=/home/user/.local/share/flatpak/app/dev.limux.linux/x86_64/master/{commit}/files\n");
            assert_eq!(
                host_cli_from_info(&info).as_deref(),
                Some(Path::new("/home/user/.local/share/flatpak/app/dev.limux.linux/x86_64/master/active/files/libexec/limux/limux-host-cli"))
            );
        }
    }

    #[test]
    fn metadata_paths_preserve_spaces_quotes_and_key_file_escapes() {
        let info = "[Instance]\napp-path=/home/user's project\\s/cache\\\\build/files\n";
        assert_eq!(
            host_cli_from_info(info).as_deref(),
            Some(Path::new(
                "/home/user's project /cache\\build/files/libexec/limux/limux-host-cli"
            ))
        );
        assert_eq!(
            key_file_value(
                "[Instance]\napp-path=first\napp-path=last\n",
                "Instance",
                "app-path"
            )
            .as_deref(),
            Some("last")
        );
        assert!(host_cli_from_info("[Instance]\napp-path=relative/files\n").is_none());
    }
}
