//! Serialize session writers and apply only changes made by this window.
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::layout_state::{self, AppSessionState, LoadedSession, WorkspaceState};

pub(crate) enum SaveOutcome {
    Saved,
    Conflict {
        recovery: PathBuf,
        workspaces: Vec<String>,
    },
}

pub(crate) struct SessionStore {
    directory: PathBuf,
    recovery: PathBuf,
    // GTK can materialize missing pane IDs or update restoration metadata.
    // Compare local changes with the actual restored UI, not its serialized input.
    local: AppSessionState,
    // Keep ancestry for each locally known workspace. Preserving an unseen remote
    // edit must not make that edit the baseline for a later stale local change.
    ancestry: AppSessionState,
    first_save: bool,
}

impl SessionStore {
    pub(crate) fn load() -> io::Result<(Self, LoadedSession)> {
        Self::load_from_dir(&layout_state::persistence_dir())
    }

    pub(crate) fn load_from_dir(directory: &Path) -> io::Result<(Self, LoadedSession)> {
        let _lock = lock_directory(directory)?;
        let mut loaded = read_session(directory)?;
        let mut changed = false;
        let mut ids = BTreeSet::new();
        for workspace in &mut loaded.state.workspaces {
            if workspace
                .id
                .as_deref()
                .is_none_or(|id| uuid::Uuid::parse_str(id).is_err() || !ids.insert(id.to_string()))
            {
                let id = uuid::Uuid::new_v4().to_string();
                ids.insert(id.clone());
                workspace.id = Some(id);
                changed = true;
            }
        }
        // Migrate identity once, under the writer lock, before another window loads.
        if changed {
            layout_state::save_session_atomic_in(directory, &loaded.state)?;
        }
        let store = Self::from_snapshot(directory, &loaded.state);
        Ok((store, loaded))
    }

    /// Keep a strictly read startup snapshot when a temporary writer lock or
    /// write-permission failure prevents normal loading. No migration is safe
    /// without the lock, so this requires already stable workspace identities.
    pub(crate) fn load_for_retry() -> io::Result<(Self, LoadedSession)> {
        Self::load_for_retry_from_dir(&layout_state::persistence_dir())
    }

    fn load_for_retry_from_dir(directory: &Path) -> io::Result<(Self, LoadedSession)> {
        let loaded = read_session(directory)?;
        let mut ids = BTreeSet::new();
        if loaded.state.workspaces.iter().any(|workspace| {
            workspace
                .id
                .as_deref()
                .is_none_or(|id| uuid::Uuid::parse_str(id).is_err() || !ids.insert(id.to_string()))
        }) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Session workspace identities require migration before saving. Restart after resolving the storage error.",
            ));
        }
        let store = Self::from_snapshot(directory, &loaded.state);
        Ok((store, loaded))
    }

    fn from_snapshot(directory: &Path, snapshot: &AppSessionState) -> Self {
        Self {
            directory: directory.to_path_buf(),
            recovery: directory.join(format!("session-recovery-{}.json", uuid::Uuid::new_v4())),
            local: snapshot.clone(),
            ancestry: snapshot.clone(),
            first_save: true,
        }
    }

    pub(crate) fn restored(&mut self, local: AppSessionState) {
        self.local = layout_state::normalize_session(local);
    }

    pub(crate) fn save(&mut self, local: &AppSessionState) -> io::Result<SaveOutcome> {
        let _lock = lock_directory(&self.directory)?;
        let disk = read_session(&self.directory)?.state;
        let local = layout_state::normalize_session(local.clone());
        match merge_session(&self.local, &self.ancestry, &local, &disk, self.first_save) {
            Ok((merged, ancestry)) => {
                layout_state::save_session_atomic_in(&self.directory, &merged)?;
                self.local = local;
                self.ancestry = ancestry;
                self.first_save = false;
                Ok(SaveOutcome::Saved)
            }
            Err(workspaces) => {
                layout_state::save_session_atomic_to(&self.recovery, &local)?;
                Ok(SaveOutcome::Conflict {
                    recovery: self.recovery.clone(),
                    workspaces,
                })
            }
        }
    }
}

fn lock_directory(directory: &Path) -> io::Result<File> {
    fs::create_dir_all(directory)?;
    // Never unlink or replace this inode: all writers must lock the same file,
    // including while session.json itself is replaced by an atomic rename.
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(directory.join("session.lock"))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match lock.try_lock() {
            Ok(()) => return Ok(lock),
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Another Limux window is saving the session. Try again shortly.",
                ))
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error),
        }
    }
}

fn read_session(directory: &Path) -> io::Result<LoadedSession> {
    let canonical = layout_state::canonical_session_path_in(directory);
    match fs::read(&canonical) {
        Ok(bytes) => {
            let state = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
            Ok(LoadedSession {
                state: layout_state::normalize_session(state),
                source: layout_state::SessionLoadSource::Canonical,
            })
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let legacy = layout_state::legacy_workspaces_path_in(directory);
            match fs::read(legacy) {
                Ok(bytes) => {
                    let workspaces = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
                    Ok(LoadedSession {
                        state: AppSessionState::from_legacy(workspaces),
                        source: layout_state::SessionLoadSource::Legacy,
                    })
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(LoadedSession {
                    state: AppSessionState::default(),
                    source: layout_state::SessionLoadSource::Empty,
                }),
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

fn workspace_map(state: &AppSessionState) -> BTreeMap<&str, &WorkspaceState> {
    state
        .workspaces
        .iter()
        .filter_map(|workspace| workspace.id.as_deref().map(|id| (id, workspace)))
        .collect()
}

fn workspace_order(state: &AppSessionState) -> Vec<String> {
    state
        .workspaces
        .iter()
        .filter_map(|workspace| workspace.id.clone())
        .collect()
}

fn selected_id(state: &AppSessionState) -> Option<&str> {
    state
        .workspaces
        .get(state.active_workspace_index)
        .and_then(|workspace| workspace.id.as_deref())
}

fn merge_session(
    previous_local: &AppSessionState,
    previous_disk: &AppSessionState,
    local: &AppSessionState,
    disk: &AppSessionState,
    first_save: bool,
) -> Result<(AppSessionState, AppSessionState), Vec<String>> {
    let old_local = workspace_map(previous_local);
    let old_disk = workspace_map(previous_disk);
    let current_local = workspace_map(local);
    let current_disk = workspace_map(disk);
    let ids: BTreeSet<_> = old_local
        .keys()
        .chain(old_disk.keys())
        .chain(current_local.keys())
        .chain(current_disk.keys())
        .copied()
        .collect();
    let mut merged = BTreeMap::new();
    let mut ancestry = old_disk.clone();
    let mut conflicts = Vec::new();
    let mut accepted = BTreeSet::new();
    for id in ids {
        let before = old_local.get(id).copied();
        let ancestor = old_disk.get(id).copied();
        let ours = current_local.get(id).copied();
        let theirs = current_disk.get(id).copied();
        let changed = ours != before || (first_save && ours.is_some() && ancestor.is_none());
        let normalize_own = first_save && ours.is_some() && theirs == ancestor;
        let chosen = if ours == theirs || ((changed || normalize_own) && theirs == ancestor) {
            accepted.insert(id.to_string());
            match ours {
                Some(workspace) => {
                    ancestry.insert(id, workspace);
                }
                None => {
                    ancestry.remove(id);
                }
            }
            ours
        } else if !changed {
            // Do not advance ancestry to a remote edit that this UI never saw.
            theirs
        } else {
            conflicts.push(
                ours.or(theirs)
                    .or(ancestor)
                    .map(|workspace| workspace.name.clone())
                    .unwrap_or_else(|| id.to_string()),
            );
            continue;
        };
        if let Some(workspace) = chosen {
            merged.insert(id.to_string(), workspace.clone());
        }
    }
    if !conflicts.is_empty() {
        return Err(conflicts);
    }

    // Respect explicit local reordering while retaining remotely added workspaces.
    // Two incompatible reorderings are a conflict rather than an implicit winner.
    let common: BTreeSet<_> = old_local
        .keys()
        .filter(|id| current_local.contains_key(**id) && current_disk.contains_key(**id))
        .copied()
        .collect();
    let relative = |state: &AppSessionState| {
        workspace_order(state)
            .into_iter()
            .filter(|id| common.contains(id.as_str()))
            .collect::<Vec<_>>()
    };
    let local_reordered = relative(local) != relative(previous_local);
    if local_reordered
        && relative(disk) != relative(previous_disk)
        && relative(local) != relative(disk)
    {
        return Err(vec!["workspace order".to_string()]);
    }
    let mut order = if local_reordered {
        workspace_order(local)
    } else {
        workspace_order(disk)
    };
    order.extend(workspace_order(local));
    let mut workspaces = Vec::new();
    for id in order {
        if let Some(workspace) = merged.remove(&id) {
            workspaces.push(workspace);
        }
    }
    workspaces.extend(merged.into_values());
    workspaces.sort_by_key(|workspace| !workspace.favorite);

    // Selection and window chrome are view preferences, not workspace contents.
    let selected = if selected_id(local) != selected_id(previous_local) {
        selected_id(local)
    } else {
        selected_id(disk)
    };
    let active_workspace_index = workspaces
        .iter()
        .position(|workspace| workspace.id.as_deref() == selected)
        .unwrap_or(0);
    let mut result = AppSessionState {
        workspaces,
        active_workspace_index,
        top_bar_visible: if local.top_bar_visible != previous_local.top_bar_visible {
            local.top_bar_visible
        } else {
            disk.top_bar_visible
        },
        sidebar: if local.sidebar != previous_local.sidebar {
            local.sidebar.clone()
        } else {
            disk.sidebar.clone()
        },
        ..AppSessionState::default()
    };
    normalize_pane_ids(&mut result, disk);
    for workspace in &result.workspaces {
        let id = workspace.id.as_deref().expect("merged workspace identity");
        if accepted.contains(id) {
            ancestry.insert(id, workspace);
        }
    }
    let mut ancestry_order = if local_reordered {
        workspace_order(local)
    } else {
        workspace_order(previous_disk)
    };
    ancestry_order.extend(workspace_order(local));
    let mut ancestral_workspaces = Vec::new();
    for id in ancestry_order {
        if let Some(workspace) = ancestry.remove(id.as_str()) {
            ancestral_workspaces.push(workspace.clone());
        }
    }
    ancestral_workspaces.extend(ancestry.into_values().cloned());
    let ancestry = AppSessionState {
        workspaces: ancestral_workspaces,
        ..previous_disk.clone()
    };
    Ok((result, ancestry))
}

// Pane IDs are process-local counters. Independent workspace additions in two
// windows can collide even though their workspace and tab UUIDs remain unique.
fn normalize_pane_ids(state: &mut AppSessionState, disk: &AppSessionState) {
    fn collect(layout: &crate::layout_state::LayoutNodeState, reserved: &mut BTreeSet<u32>) {
        match layout {
            crate::layout_state::LayoutNodeState::Pane(pane) => {
                if let Some(id) = pane.pane_id.filter(|id| *id > 0) {
                    reserved.insert(id);
                }
            }
            crate::layout_state::LayoutNodeState::Split(split) => {
                collect(&split.start, reserved);
                collect(&split.end, reserved);
            }
        }
    }
    fn assign(
        layout: &mut crate::layout_state::LayoutNodeState,
        reserved: &mut BTreeSet<u32>,
        seen: &mut BTreeSet<u32>,
    ) {
        match layout {
            crate::layout_state::LayoutNodeState::Pane(pane) => {
                if pane
                    .pane_id
                    .filter(|id| *id > 0)
                    .is_none_or(|id| !seen.insert(id))
                {
                    let id = (1..=u32::MAX)
                        .find(|id| !reserved.contains(id))
                        .expect("available pane identity");
                    pane.pane_id = Some(id);
                    reserved.insert(id);
                    seen.insert(id);
                }
            }
            crate::layout_state::LayoutNodeState::Split(split) => {
                assign(&mut split.start, reserved, seen);
                assign(&mut split.end, reserved, seen);
            }
        }
    }
    let mut reserved = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for workspace in &state.workspaces {
        collect(&workspace.layout, &mut reserved);
    }
    let persisted = workspace_map(disk);
    let mut order: Vec<_> = (0..state.workspaces.len()).collect();
    // Reserve unchanged remote panes first, so a newly created local pane can
    // never renumber a workspace belonging to another writer.
    order.sort_by_key(|index| {
        let workspace = &state.workspaces[*index];
        workspace
            .id
            .as_deref()
            .and_then(|id| persisted.get(id))
            .copied()
            != Some(workspace)
    });
    for index in order {
        assign(
            &mut state.workspaces[index].layout,
            &mut reserved,
            &mut seen,
        );
    }
}

#[cfg(test)]
#[path = "session_store_tests.rs"]
mod tests;
