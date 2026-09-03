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
    marker: String,
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
        let marker = format!(
            "<!-- limux-agent-team:{} -->",
            staged.path().file_name().unwrap().to_string_lossy()
        );
        Ok(Self {
            path: path.to_path_buf(),
            staged,
            marker,
        })
    }

    pub(crate) fn pane_command(&self, launch: Option<&str>) -> String {
        let cwd = quote(&self.path.parent().unwrap().to_string_lossy());
        let command = match launch {
            Some(launch) => format!(
                "cd {cwd} || exit 1; remaining=1200; \
                 while [ -e {staged} ]; do \
                 [ \"$remaining\" -gt 0 ] || exit 1; \
                 remaining=$((remaining - 1)); sleep 0.05; done; \
                 [ -f AGENTS.md ] && grep -Fqx -- {marker} AGENTS.md && exec {launch}",
                staged = quote(&self.staged.path().to_string_lossy()),
                marker = quote(&self.marker),
            ),
            None => format!("cd {cwd} && exec \"${{SHELL:-/bin/sh}}\""),
        };
        format!("/bin/sh -c {}", quote(&command))
    }

    pub(crate) fn publish(mut self, body: &str) -> Result<()> {
        self.staged
            .write_all(body.as_bytes())
            .with_context(|| format!("failed to write instructions for {}", self.path.display()))?;
        // This per-run marker prevents a failed launch from consuming another
        // invocation's instructions. Removing the staging name releases waiters.
        writeln!(self.staged, "\n{}", self.marker)?;
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

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
#[path = "agent_team_file_tests.rs"]
mod tests;
