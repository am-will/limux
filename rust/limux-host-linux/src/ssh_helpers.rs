//! Explicit, one-time provisioning. Restoring/reconnecting never installs code.
use crate::ssh_hosts::SshTarget;
use crate::ssh_transfer::Upload;
use gtk::prelude::*;
use gtk4::{self as gtk, gio, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const INSTALL_SCRIPT: &str = include_str!("../../../scripts/remote-helper-install.sh");
const MAX_HELPER_BYTES: usize = 64 * 1024 * 1024;

fn prepare(path: &std::path::Path, agent: &str) -> Result<(Vec<u8>, String), String> {
    use std::io::Read;
    if !matches!(agent, "codex" | "claude" | "gemini") {
        return Err("Unsupported agent".into());
    }
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Select a regular helper file".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_HELPER_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_HELPER_BYTES {
        return Err("Helper exceeds 64 MiB; select a release build".into());
    }
    let arch = architecture(&bytes)?;
    let mut checksum = glib::Checksum::new(glib::ChecksumType::Sha256).unwrap();
    checksum.update(&bytes);
    let hash = checksum.string().unwrap();
    let script = format!(
        "set -- '{hash}' '{arch}' '{agent}' '{}'\n{INSTALL_SCRIPT}",
        bytes.len()
    );
    Ok((bytes, script))
}

fn architecture(bytes: &[u8]) -> Result<&'static str, String> {
    if bytes.len() < 20 || &bytes[..6] != b"\x7fELF\x02\x01" {
        return Err("Select a trusted 64-bit Linux ELF limux-cli helper".into());
    }
    match u16::from_le_bytes([bytes[18], bytes[19]]) {
        62 => Ok("x86_64"),
        183 => Ok("aarch64"),
        _ => Err("Helper must target Linux x86_64 or aarch64".into()),
    }
}

pub fn show(parent: &gtk::Window, target: SshTarget) {
    let dialog = gtk::Window::builder()
        .title("Install SSH agent helper")
        .transient_for(parent)
        .modal(true)
        .default_width(520)
        .build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(20);
    content.set_margin_bottom(20);
    content.set_margin_start(20);
    content.set_margin_end(20);
    let info = gtk::Label::new(Some(&format!("Install on {}{}\n\nSelect a trusted Linux limux-cli build (prefer static musl). This uploads and executes it on the host, backs up the selected agent’s configuration, and replaces that agent’s existing Limux hooks with terminal notifications. Other hooks are preserved. No executable on PATH or tmux options are changed.\n\nInstallation requires noninteractive SSH authentication and a known host key. Reconnect does not reinstall. Restart the agent afterward; review any hook approval it requests.", target.destination(), target.port().map(|p| format!(":{p}")).unwrap_or_default())));
    info.set_wrap(true);
    info.set_xalign(0.0);
    content.append(&info);
    let agent = gtk::DropDown::from_strings(&["Codex", "Claude Code", "Gemini CLI"]);
    content.append(&agent);
    let choose = gtk::Button::with_label("Choose helper executable…");
    content.append(&choose);
    let status = gtk::Label::new(Some(
        "Build with scripts/build-remote-helper.sh, then select its limux-cli output.",
    ));
    status.set_wrap(true);
    content.append(&status);
    let install = gtk::Button::with_label("Install helper and configure hooks");
    install.add_css_class("suggested-action");
    install.set_sensitive(false);
    content.append(&install);
    let close = gtk::Button::with_label("Close");
    content.append(&close);
    dialog.set_child(Some(&content));
    let selected = Rc::new(RefCell::new(None::<std::path::PathBuf>));
    let active = Rc::new(RefCell::new(None::<Rc<Upload>>));
    let closed = Rc::new(Cell::new(false));
    let weak = dialog.downgrade();
    close.connect_clicked(move |_| {
        if let Some(dialog) = weak.upgrade() {
            dialog.close();
        }
    });
    let cancel = active.clone();
    let closing = closed.clone();
    dialog.connect_close_request(move |_| {
        closing.set(true);
        let upload = cancel.borrow_mut().take();
        if let Some(upload) = upload {
            upload.cancel();
        }
        glib::Propagation::Proceed
    });
    let weak = dialog.downgrade();
    let selected_file = selected.clone();
    let status_file = status.clone();
    let install_file = install.clone();
    choose.connect_clicked(move |_| {
        let Some(parent) = weak.upgrade() else {
            return;
        };
        let picker = gtk::FileDialog::builder()
            .title("Select trusted Linux limux-cli")
            .modal(true)
            .build();
        let selected = selected_file.clone();
        let status = status_file.clone();
        let install = install_file.clone();
        picker.open(Some(&parent), None::<&gio::Cancellable>, move |result| {
            if let Ok(file) = result {
                if let Some(path) = file.path() {
                    status.set_text(&path.display().to_string());
                    *selected.borrow_mut() = Some(path);
                    install.set_sensitive(true);
                }
            }
        });
    });
    install.connect_clicked(move |install| {
        let Some(path) = selected.borrow().clone() else {
            return;
        };
        let agent_name = ["codex", "claude", "gemini"][agent.selected() as usize];
        install.set_sensitive(false);
        choose.set_sensitive(false);
        agent.set_sensitive(false);
        status.set_text("Verifying and installing helper…");
        let status = status.clone();
        let install = install.clone();
        let choose = choose.clone();
        let agent = agent.clone();
        let target = target.clone();
        let closed = closed.clone();
        let active = active.clone();
        glib::MainContext::default().spawn_local(async move {
            let prepared = gio::spawn_blocking(move || prepare(&path, agent_name)).await;
            if closed.get() {
                return;
            }
            let (bytes, script) = match prepared {
                Ok(Ok(value)) => value,
                other => {
                    let error = match other {
                        Ok(Err(error)) => error,
                        _ => "worker failed".into(),
                    };
                    status.set_text(&format!("Helper verification failed: {error}"));
                    install.set_sensitive(true);
                    choose.set_sensitive(true);
                    agent.set_sensitive(true);
                    return;
                }
            };
            let completed_status = status.clone();
            let completed_install = install.clone();
            let completed_choose = choose.clone();
            let completed_agent = agent.clone();
            let completed_active = active.clone();
            let flatpak = std::env::var_os("FLATPAK_ID").is_some_and(|id| id == crate::APP_ID);
            let result = Upload::install_helper(
                &target,
                glib::Bytes::from_owned(bytes),
                &script,
                flatpak,
                move |result| {
                    completed_active.borrow_mut().take();
                    if !closed.get() {
                        let message = match result {
                            Ok(_) => concat!(
                                "Installed. Restart the agent. ",
                                "Configuration backup details are in the launch log."
                            )
                            .into(),
                            Err(error) => format!(
                                "{error} Installation may have completed remotely; \
                                 inspect the launch log before retrying."
                            ),
                        };
                        completed_status.set_text(&message);
                        completed_install.set_sensitive(true);
                        completed_choose.set_sensitive(true);
                        completed_agent.set_sensitive(true);
                    }
                },
            );
            match result {
                Ok(upload) => *active.borrow_mut() = Some(upload),
                Err(error) => {
                    status.set_text(&error);
                    install.set_sensitive(true);
                    choose.set_sensitive(true);
                    agent.set_sensitive(true);
                }
            }
        });
    });
    dialog.present();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn helper_architecture_is_read_from_bytes_not_local_machine() {
        assert!(architecture(b"#!/bin/sh\n").is_err());
        let mut elf = [0u8; 20];
        elf[..6].copy_from_slice(b"\x7fELF\x02\x01");
        elf[18] = 62;
        assert_eq!(architecture(&elf).unwrap(), "x86_64");
        elf[18] = 183;
        assert_eq!(architecture(&elf).unwrap(), "aarch64");
        elf[18] = 3;
        assert!(architecture(&elf).is_err());
        elf[5] = 2;
        assert!(architecture(&elf).is_err());
    }
    fn install_fixture(
        home: &std::path::Path,
        bytes: &[u8],
        hash: Option<&str>,
        arch: &str,
    ) -> std::process::Output {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let mut checksum = glib::Checksum::new(glib::ChecksumType::Sha256).unwrap();
        checksum.update(bytes);
        let hash = hash
            .map(str::to_owned)
            .unwrap_or_else(|| checksum.string().unwrap());
        let mut child = Command::new("/bin/sh")
            .args([
                "-c",
                INSTALL_SCRIPT,
                "installer",
                &hash,
                arch,
                "codex",
                &bytes.len().to_string(),
            ])
            .env("HOME", home)
            // An empty override must resolve to the same path for backup and CLI.
            .env("CODEX_HOME", "")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let _ = child.stdin.take().unwrap().write_all(bytes);
        child.wait_with_output().unwrap()
    }

    #[test]
    fn installer_checks_bytes_before_execution_and_backs_up_config() {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join(".codex/hooks.json");
        std::fs::create_dir(config.parent().unwrap()).unwrap();
        std::fs::write(&config, "original config").unwrap();
        let helper = b"#!/bin/sh\nif [ \"$1\" = --help ]; then echo 'hooks setup [agent] [--osc]'; else test \"$1 $2 $3 $4\" = 'hooks setup codex --osc' || exit 19; echo installed > \"$CODEX_HOME/hooks.json\"; fi\n";
        for (hash, arch) in [
            (
                Some("0000000000000000000000000000000000000000000000000000000000000000"),
                std::env::consts::ARCH,
            ),
            (None, "unsupported"),
        ] {
            assert!(!install_fixture(home.path(), helper, hash, arch)
                .status
                .success());
            assert_eq!(std::fs::read_to_string(&config).unwrap(), "original config");
        }
        let old = b"#!/bin/sh\necho old-version\n";
        assert!(
            !install_fixture(home.path(), old, None, std::env::consts::ARCH)
                .status
                .success()
        );
        assert_eq!(std::fs::read_to_string(&config).unwrap(), "original config");
        for _ in 0..2 {
            let result = install_fixture(home.path(), helper, None, std::env::consts::ARCH);
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert_eq!(result.stdout, b"LIMUX_HELPER_INSTALLED\n");
        }
        let base = home.path().join(".local/share/limux/ssh-helpers");
        let entries: Vec<_> = std::fs::read_dir(base)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        let backups: Vec<_> = entries
            .iter()
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("backup-")
            })
            .collect();
        assert_eq!(backups.len(), 2);
        assert!(backups
            .iter()
            .any(|p| std::fs::read_to_string(p.join("config.json")).unwrap() == "original config"));
        assert!(!entries.iter().filter(|p| p.is_dir()).any(|p| p
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(".install.")));
        assert_eq!(
            entries
                .iter()
                .filter(|p| p.file_name().unwrap().len() == 64)
                .count(),
            1
        );
    }

    #[test]
    fn installer_refuses_redirected_config_and_writable_helper_parents() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join(".codex");
        std::fs::create_dir(&config).unwrap();
        let original = home.path().join("precious");
        std::fs::write(&original, "unchanged").unwrap();
        std::os::unix::fs::symlink(&original, config.join("hooks.json")).unwrap();
        let result = install_fixture(home.path(), b"unused", None, std::env::consts::ARCH);
        assert!(!result.status.success());
        assert_eq!(std::fs::read_to_string(&original).unwrap(), "unchanged");
        std::fs::remove_file(config.join("hooks.json")).unwrap();
        std::fs::set_permissions(
            home.path().join(".local"),
            std::fs::Permissions::from_mode(0o777),
        )
        .unwrap();
        assert!(
            !install_fixture(home.path(), b"unused", None, std::env::consts::ARCH)
                .status
                .success()
        );
    }
    #[test]
    #[ignore = "requires built limux-cli; runs in maintained GUI harness"]
    fn helper_transfer_configures_only_selected_agent_and_keeps_backup() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("remote-home");
        std::fs::create_dir(&home).unwrap();
        let config_dir = home.join(".codex");
        std::fs::create_dir(&config_dir).unwrap();
        let original = br#"{"custom":42,"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo unrelated"}]}]}}"#;
        std::fs::write(config_dir.join("hooks.json"), original).unwrap();
        let ssh = root.path().join("ssh");
        std::fs::write(&ssh, format!("#!/bin/sh\nexport HOME='{}' CODEX_HOME='{}'\nfor arg; do command=$arg; done\nexec /bin/sh -c \"$command\"\n", home.display(), config_dir.display())).unwrap();
        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::env::set_var(
            "PATH",
            format!(
                "{}:{}",
                root.path().display(),
                std::env::var("PATH").unwrap()
            ),
        );
        let cli = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("limux-cli");
        let (bytes, script) = prepare(&cli, "codex").unwrap();
        let context = glib::MainContext::default();
        let _guard = context.acquire().unwrap();
        let result = Rc::new(RefCell::new(None));
        let completed = result.clone();
        let upload = Upload::install_helper(
            &SshTarget::parse("fixture.invalid", "2222").unwrap(),
            glib::Bytes::from_owned(bytes),
            &script,
            false,
            move |value| *completed.borrow_mut() = Some(value),
        )
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while result.borrow().is_none() {
            for _ in 0..100 {
                if !context.pending() {
                    break;
                }
                context.iteration(false);
            }
            assert!(
                std::time::Instant::now() < deadline,
                "helper transfer timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            result.borrow().as_ref().unwrap().is_ok(),
            "{:?}",
            *result.borrow()
        );
        upload.cancel();
        let config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(config_dir.join("hooks.json")).unwrap()).unwrap();
        assert_eq!(config["custom"], 42);
        assert_eq!(config["hooks"]["Stop"].as_array().unwrap().len(), 2);
        let command = config["hooks"]["Stop"][1]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert!(command.contains("LIMUX_NOTIFY_OSC=1"));
        assert!(command.contains("/ssh-helpers/"));
        assert!(!home.join(".claude").exists());
        assert!(!home.join(".gemini").exists());
        let base = home.join(".local/share/limux/ssh-helpers");
        let backup = std::fs::read_dir(base)
            .unwrap()
            .map(|p| p.unwrap().path())
            .find(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("backup-codex.")
            })
            .unwrap();
        assert_eq!(std::fs::read(backup.join("config.json")).unwrap(), original);
    }
}
