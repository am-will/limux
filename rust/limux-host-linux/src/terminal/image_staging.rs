//! Private local PNG files for an explicit clipboard image paste.
//!
//! Delivered files stay on disk. Dropping a guard before delivery removes only
//! that guard's exact file and directory.
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::ssh_transfer::{validate_clipboard_png, validated_paste_path};

#[derive(Debug)]
pub(crate) struct StagedImage {
    path: String,
    host: bool,
    retained: bool,
}

impl StagedImage {
    pub(crate) fn native(png: &[u8]) -> Result<Self, String> {
        validate_clipboard_png(png)?;
        let directory = tempfile::Builder::new()
            .prefix("limux-paste.")
            .rand_bytes(10)
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")
            .map_err(stage_error)?;
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .map_err(stage_error)?;
        let Some(path) = owned_paste_path(directory.path()) else {
            return Err(untrusted_path());
        };
        let file_path = directory.path().join("image.png");
        if let Err(error) = write_private_png(&file_path, png) {
            return Err(stage_error(error));
        }
        let persisted = directory.keep();
        if owned_paste_path(&persisted).as_deref() != Some(path.as_str()) {
            remove_native(&path);
            return Err(untrusted_path());
        }
        Ok(Self {
            path,
            host: false,
            retained: false,
        })
    }

    pub(crate) fn host(path: String) -> Result<Self, String> {
        let path = validated_paste_path(&path)
            .ok_or_else(|| "Invalid image staging response".to_string())?;
        Ok(Self {
            path,
            host: true,
            retained: false,
        })
    }

    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    pub(crate) fn retain(&mut self) {
        self.retained = true;
    }
}

impl Drop for StagedImage {
    fn drop(&mut self) {
        if self.retained {
            return;
        }
        if self.host {
            spawn_host_cleanup(&self.path);
        } else {
            remove_native(&self.path);
        }
    }
}

fn untrusted_path() -> String {
    "Could not save the clipboard image: the staging path was not private".into()
}

fn stage_error(error: io::Error) -> String {
    format!("Could not save the clipboard image: {error}")
}

fn owned_paste_path(directory: &Path) -> Option<String> {
    if directory.parent() != Some(Path::new("/tmp")) {
        return None;
    }
    let name = directory.file_name()?.to_str()?;
    validated_paste_path(&format!("/tmp/{name}/image.png"))
}

fn write_private_png(path: &Path, png: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    if let Err(error) = write_png_bytes(&mut file, png) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error);
    }
    file.sync_all()?;
    drop(file);
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    let written = fs::read(path)?;
    if written != png {
        let _ = fs::remove_file(path);
        return Err(io::Error::other("clipboard image was not fully written"));
    }
    Ok(())
}

fn write_png_bytes(file: &mut fs::File, png: &[u8]) -> io::Result<()> {
    #[cfg(test)]
    if FAIL_WRITE.with(std::cell::Cell::get) {
        FAIL_WRITE.with(|flag| flag.set(false));
        return Err(io::Error::other("injected write failure"));
    }
    file.write_all(png)
}

fn remove_native(path: &str) {
    let Some(path) = validated_paste_path(path) else {
        return;
    };
    let file = PathBuf::from(&path);
    let Some(directory) = file.parent() else {
        return;
    };
    if directory.parent() != Some(Path::new("/tmp")) {
        return;
    }
    let _ = fs::remove_file(&file);
    let _ = fs::remove_dir(directory);
}

pub(crate) fn host_cleanup_script(path: &str) -> Option<String> {
    let path = validated_paste_path(path)?;
    let directory = path.strip_suffix("/image.png")?;
    Some(format!("rm -f -- '{path}'; rmdir -- '{directory}'"))
}

pub(crate) fn host_cleanup_arguments(path: &str) -> Option<Vec<String>> {
    Some(vec![
        "/app/bin/host-spawn".into(),
        "--no-pty".into(),
        "--cwd=/".into(),
        "/bin/sh".into(),
        "-c".into(),
        host_cleanup_script(path)?,
    ])
}

fn spawn_host_cleanup(path: &str) {
    let Some(arguments) = host_cleanup_arguments(path) else {
        return;
    };
    let _ = std::process::Command::new(&arguments[0])
        .args(&arguments[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

#[cfg(test)]
std::thread_local! {
    static FAIL_WRITE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0fixture";
    static LOCK: Mutex<()> = Mutex::new(());

    unsafe extern "C" {
        fn umask(mask: u32) -> u32;
    }

    struct UmaskGuard(u32);

    impl Drop for UmaskGuard {
        fn drop(&mut self) {
            unsafe { umask(self.0) };
        }
    }

    fn set_umask(mask: u32) -> UmaskGuard {
        UmaskGuard(unsafe { umask(mask) })
    }

    fn paste_directories() -> Vec<PathBuf> {
        fs::read_dir("/tmp")
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("limux-paste."))
            })
            .collect()
    }

    fn assert_private(path: &str) {
        let file = Path::new(path);
        let directory = file.parent().unwrap();
        assert_eq!(
            fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(validated_paste_path(path).as_deref(), Some(path));
        assert!(!path.contains(' ') && !path.contains('$') && !path.contains(';'));
    }

    #[test]
    fn native_staging_round_trips_a_private_unique_file_and_ignores_tmpdir() {
        let _lock = LOCK.lock().unwrap();
        let previous = std::env::var_os("TMPDIR");
        std::env::set_var("TMPDIR", "/var/tmp");
        let first = StagedImage::native(PNG).unwrap();
        let second = StagedImage::native(PNG).unwrap();
        if let Some(previous) = previous {
            std::env::set_var("TMPDIR", previous);
        } else {
            std::env::remove_var("TMPDIR");
        }
        let kept = first.path().to_string();
        let removed = second.path().to_string();
        assert_ne!(kept, removed);
        assert!(kept.starts_with("/tmp/limux-paste."));
        assert!(removed.starts_with("/tmp/limux-paste."));
        assert_eq!(fs::read(&kept).unwrap(), PNG);
        assert_eq!(fs::read(&removed).unwrap(), PNG);
        assert_private(&kept);
        assert_private(&removed);
        drop(second);
        assert!(!Path::new(&removed).exists());
        assert!(!Path::new(&parent_dir(&removed)).exists());
        assert_eq!(fs::read(&kept).unwrap(), PNG);
        drop(first);
        fs::remove_file(&kept).ok();
        fs::remove_dir(parent_dir(&kept)).ok();
    }

    fn parent_dir(path: &str) -> String {
        Path::new(path)
            .parent()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn umask_cannot_widen_the_staging_directory_or_file() {
        let _lock = LOCK.lock().unwrap();
        for mask in [0o000, 0o022, 0o077] {
            let _umask = set_umask(mask);
            let staged = StagedImage::native(PNG).unwrap();
            assert_private(staged.path());
            let path = staged.path().to_string();
            drop(staged);
            assert!(!Path::new(&path).exists());
            assert!(!Path::new(&parent_dir(&path)).exists());
        }
    }

    #[test]
    fn undelivered_files_are_removed_and_delivered_files_stay() {
        let _lock = LOCK.lock().unwrap();
        let staged = StagedImage::native(PNG).unwrap();
        let path = staged.path().to_string();
        drop(staged);
        assert!(!Path::new(&path).exists());
        assert!(!Path::new(&parent_dir(&path)).exists());

        let mut staged = StagedImage::native(PNG).unwrap();
        let path = staged.path().to_string();
        staged.retain();
        drop(staged);
        assert_eq!(fs::read(&path).unwrap(), PNG);
        fs::remove_file(&path).unwrap();
        fs::remove_dir(parent_dir(&path)).unwrap();
    }

    #[test]
    fn write_failure_removes_the_partial_directory() {
        let _lock = LOCK.lock().unwrap();
        let before = paste_directories();
        FAIL_WRITE.with(|flag| flag.set(true));
        let error = StagedImage::native(PNG).unwrap_err();
        assert!(error.contains("injected write failure"), "{error}");
        let after = paste_directories();
        assert_eq!(before.len(), after.len());
        for path in &after {
            assert!(before.contains(path), "left behind {}", path.display());
        }
    }

    #[test]
    fn oversized_or_non_png_bytes_create_no_directory() {
        let _lock = LOCK.lock().unwrap();
        let before = paste_directories();
        assert!(StagedImage::native(b"GIF89a").is_err());
        assert!(StagedImage::native(&[]).is_err());
        let mut oversized = vec![0_u8; crate::ssh_transfer::MAX_PNG_BYTES + 1];
        oversized[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        assert!(StagedImage::native(&oversized).is_err());
        assert_eq!(paste_directories().len(), before.len());
    }

    #[test]
    fn host_cleanup_accepts_only_the_generated_path() {
        let path = "/tmp/limux-paste.zzCleanup1/image.png";
        let arguments = host_cleanup_arguments(path).unwrap();
        assert_eq!(
            &arguments[..5],
            [
                "/app/bin/host-spawn",
                "--no-pty",
                "--cwd=/",
                "/bin/sh",
                "-c"
            ]
        );
        assert_eq!(
            arguments[5],
            "rm -f -- '/tmp/limux-paste.zzCleanup1/image.png'; rmdir -- '/tmp/limux-paste.zzCleanup1'"
        );
        for rejected in [
            "/tmp/limux-paste.zzCleanup1/image.png;rm -rf /",
            "/tmp/limux-paste.abcdefghij/../image.png",
            "/tmp/limux-paste.short/image.png",
            " /tmp/limux-paste.zzCleanup1/image.png",
        ] {
            assert!(host_cleanup_arguments(rejected).is_none(), "{rejected}");
            assert!(StagedImage::host(rejected.into()).is_err(), "{rejected}");
        }
        fs::create_dir_all("/tmp/limux-paste.zzCleanup1").unwrap();
        fs::write(path, b"partial").unwrap();
        let status = std::process::Command::new("/bin/sh")
            .args(["-c", host_cleanup_script(path).unwrap().as_str()])
            .status()
            .unwrap();
        assert!(status.success());
        assert!(!Path::new(path).exists());
        assert!(!Path::new("/tmp/limux-paste.zzCleanup1").exists());
    }
}
