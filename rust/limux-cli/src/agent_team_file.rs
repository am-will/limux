use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use tempfile::NamedTempFile;

/// Stage instructions beside their destination, then publish without clobbering.
/// Existing files are always user-owned, even if Limux originally generated them.
pub(crate) struct PendingInstructions {
    path: PathBuf,
    staged: NamedTempFile,
}

impl PendingInstructions {
    pub(crate) fn new(path: &Path) -> Result<Self> {
        match fs::symlink_metadata(path) {
            Ok(_) => bail!(
                "agent-team: refusing to replace {}; preserve or move the existing instructions before creating a team",
                path.display()
            ),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("failed to inspect {}", path.display()));
            }
        }
        let parent = path.parent().context("instructions path has no parent")?;
        let staged = NamedTempFile::new_in(parent)
            .with_context(|| format!("failed to stage instructions in {}", parent.display()))?;
        Ok(Self {
            path: path.to_path_buf(),
            staged,
        })
    }

    pub(crate) fn publish(mut self, body: &str) -> Result<()> {
        self.staged
            .write_all(body.as_bytes())
            .with_context(|| format!("failed to write instructions for {}", self.path.display()))?;
        self.staged
            .persist_noclobber(&self.path)
            .map_err(|error| error.error)
            .with_context(|| {
                format!(
                    "failed to publish {} without replacing existing instructions",
                    self.path.display()
                )
            })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_instructions_are_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("AGENTS.md");
        let original = "# Project instructions\nKeep my local policies.\n";
        fs::write(&path, original).unwrap();
        assert!(PendingInstructions::new(&path).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), original);
    }

    #[test]
    fn concurrent_creation_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("AGENTS.md");
        let pending = PendingInstructions::new(&path).unwrap();
        fs::write(&path, "Created while panes were starting").unwrap();
        assert!(pending.publish("Generated protocol").is_err());
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "Created while panes were starting"
        );
    }

    #[test]
    fn published_instructions_are_preserved_on_repeat() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("AGENTS.md");
        PendingInstructions::new(&path)
            .unwrap()
            .publish("Generated protocol with edited policies")
            .unwrap();
        assert!(PendingInstructions::new(&path).is_err());
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "Generated protocol with edited policies"
        );
    }

    #[test]
    fn abandoned_stage_leaves_no_instructions_or_temporary_files() {
        let dir = tempfile::tempdir().unwrap();
        drop(PendingInstructions::new(&dir.path().join("AGENTS.md")).unwrap());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_is_not_followed_or_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("AGENTS.md");
        let target = dir.path().join("missing-target");
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(PendingInstructions::new(&path).is_err());
        assert_eq!(fs::read_link(path).unwrap(), target);
        assert!(!target.exists());
    }
}
