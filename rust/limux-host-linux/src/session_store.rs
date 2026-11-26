//! Serialize session writers and apply only changes made by this window.
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::layout_state::{
    self, AppSessionState, LoadedSession, WorkspaceGroupState, WorkspaceState,
};

pub(crate) enum SaveOutcome {
    Saved(SavedSessionSummary),
    Conflict {
        recovery: PathBuf,
        workspaces: Vec<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SavedSessionSummary {
    pub(crate) path: PathBuf,
    pub(crate) workspaces: usize,
    pub(crate) tabs: usize,
    pub(crate) restorable_agents: usize,
}

fn count_layout_tabs(layout: &layout_state::LayoutNodeState) -> (usize, usize) {
    match layout {
        layout_state::LayoutNodeState::Pane(pane) => {
            let tabs = pane.tabs.len();
            let agents = pane
                .tabs
                .iter()
                .filter(|tab| match &tab.content {
                    layout_state::TabContentState::Terminal { agent, .. } => {
                        agent.as_ref().is_some_and(|agent| agent.restore_on_startup)
                    }
                    _ => false,
                })
                .count();
            (tabs, agents)
        }
        layout_state::LayoutNodeState::Split(split) => {
            let (start_tabs, start_agents) = count_layout_tabs(&split.start);
            let (end_tabs, end_agents) = count_layout_tabs(&split.end);
            (start_tabs + end_tabs, start_agents + end_agents)
        }
    }
}

fn summarize_saved_session(path: PathBuf, session: &AppSessionState) -> SavedSessionSummary {
    let mut tabs = 0usize;
    let mut restorable_agents = 0usize;
    for workspace in &session.workspaces {
        let (workspace_tabs, workspace_agents) = count_layout_tabs(&workspace.layout);
        tabs += workspace_tabs;
        restorable_agents += workspace_agents;
    }
    SavedSessionSummary {
        path,
        workspaces: session.workspaces.len(),
        tabs,
        restorable_agents,
    }
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
        let (mut loaded, mut changed) = read_session_with_minted_ids(directory)?;
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
        changed |= layout_state::dedup_tab_ids(&mut loaded.state);
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

    pub(crate) fn save_in_background(
        mut self,
        local: AppSessionState,
    ) -> gtk4::gio::JoinHandle<(Self, AppSessionState, io::Result<SaveOutcome>)> {
        gtk4::gio::spawn_blocking(move || {
            let result = self.save_with_agents(&local);
            (self, local, result)
        })
    }

    pub(crate) fn save_with_agents(&mut self, local: &AppSessionState) -> io::Result<SaveOutcome> {
        let mut persisted = local.clone();
        let agents = layout_state::RestorableAgentIndex::load();
        layout_state::attach_restorable_agents_to_session(&mut persisted, &agents);
        self.save(&persisted)
    }

    pub(crate) fn save(&mut self, local: &AppSessionState) -> io::Result<SaveOutcome> {
        let _lock = lock_directory(&self.directory)?;
        let loaded_disk = read_session(&self.directory)?;
        let local = layout_state::normalize_session(local.clone());
        match merge_session(
            &self.local,
            &self.ancestry,
            &local,
            &loaded_disk.state,
            self.first_save,
        ) {
            Ok((merged, ancestry)) => {
                if self.first_save
                    || loaded_disk.source != layout_state::SessionLoadSource::Canonical
                    || merged != loaded_disk.state
                {
                    layout_state::save_session_atomic_in(&self.directory, &merged)?;
                }
                let summary = summarize_saved_session(
                    layout_state::canonical_session_path_in(&self.directory),
                    &merged,
                );
                self.local = local;
                self.ancestry = ancestry;
                self.first_save = false;
                Ok(SaveOutcome::Saved(summary))
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

fn layout_has_empty_pane(layout: &layout_state::LayoutNodeState) -> bool {
    match layout {
        layout_state::LayoutNodeState::Pane(pane) => pane.tabs.is_empty(),
        layout_state::LayoutNodeState::Split(split) => {
            layout_has_empty_pane(&split.start) || layout_has_empty_pane(&split.end)
        }
    }
}

fn read_session(directory: &Path) -> io::Result<LoadedSession> {
    read_session_with_minted_ids(directory).map(|(loaded, _)| loaded)
}

fn read_session_with_minted_ids(directory: &Path) -> io::Result<(LoadedSession, bool)> {
    let canonical = layout_state::canonical_session_path_in(directory);
    match fs::read(&canonical) {
        Ok(bytes) => {
            let state: AppSessionState =
                serde_json::from_slice(&bytes).map_err(io::Error::other)?;
            // Normalization mints a fresh tab id only for empty panes; detect
            // that before normalizing so clean startups avoid a second disk read.
            let minted_ids = state
                .workspaces
                .iter()
                .any(|workspace| layout_has_empty_pane(&workspace.layout));
            Ok((
                LoadedSession {
                    state: layout_state::normalize_session(state),
                    source: layout_state::SessionLoadSource::Canonical,
                },
                minted_ids,
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let legacy = layout_state::legacy_workspaces_path_in(directory);
            match fs::read(legacy) {
                Ok(bytes) => {
                    let workspaces: Vec<layout_state::LegacySavedWorkspace> =
                        serde_json::from_slice(&bytes).map_err(io::Error::other)?;
                    let minted_ids = !workspaces.is_empty();
                    Ok((
                        LoadedSession {
                            state: AppSessionState::from_legacy(workspaces),
                            source: layout_state::SessionLoadSource::Legacy,
                        },
                        minted_ids,
                    ))
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok((
                    LoadedSession {
                        state: AppSessionState::default(),
                        source: layout_state::SessionLoadSource::Empty,
                    },
                    false,
                )),
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

/// `disk` order plus the workspaces only `local` has: each goes before the
/// workspace that follows it in `local`. One with none after it is left out.
fn place_local_additions(disk: &AppSessionState, local: &AppSessionState) -> Vec<String> {
    let mut order = workspace_order(disk);
    let mut next = None;
    for id in workspace_order(local).into_iter().rev() {
        match order.iter().position(|placed| *placed == id) {
            Some(index) => next = Some(index),
            None => {
                if let Some(index) = next {
                    order.insert(index, id);
                }
            }
        }
    }
    order
}

/// Merge groups by id: a group this window created, edited or deleted since
/// its last save takes its local version; any other group keeps the disk
/// version, so groups made by other windows survive.
fn merge_groups(
    previous_local: &AppSessionState,
    local: &AppSessionState,
    disk: &AppSessionState,
) -> Vec<WorkspaceGroupState> {
    fn index_groups(state: &AppSessionState) -> HashMap<&str, &WorkspaceGroupState> {
        let mut map = HashMap::with_capacity(state.groups.len());
        for group in &state.groups {
            map.entry(group.id.as_str()).or_insert(group);
        }
        map
    }
    let prev_map = index_groups(previous_local);
    let local_map = index_groups(local);
    let disk_map = index_groups(disk);

    let mut seen = HashSet::with_capacity(local.groups.len() + disk.groups.len());
    let mut ids = Vec::with_capacity(local.groups.len() + disk.groups.len());
    for group in local.groups.iter().chain(&disk.groups) {
        if seen.insert(group.id.as_str()) {
            ids.push(group.id.as_str());
        }
    }
    ids.into_iter()
        .filter_map(|id| {
            let ours = local_map.get(id).copied();
            if ours != prev_map.get(id).copied() {
                ours.cloned()
            } else {
                disk_map.get(id).copied().cloned()
            }
        })
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
    let remote_reordered = relative(disk) != relative(previous_disk);
    if local_reordered && remote_reordered && relative(local) != relative(disk) {
        return Err(vec!["workspace order".to_string()]);
    }
    let mut order = if local_reordered {
        workspace_order(local)
    } else {
        place_local_additions(disk, local)
    };
    order.extend(if local_reordered {
        workspace_order(disk)
    } else {
        workspace_order(local)
    });
    let mut workspaces = Vec::new();
    for id in order {
        if let Some(workspace) = merged.remove(&id) {
            workspaces.push(workspace);
        }
    }
    workspaces.extend(merged.into_values());
    // Ungrouped favorites are pinned first; a group keeps its favorites in place.
    workspaces.sort_by_key(|workspace| !workspace.favorite || workspace.group_id.is_some());

    // A stale empty-group deletion can race with another window adding a member.
    // Keep both snapshots recoverable instead of silently orphaning that member.
    let groups = merge_groups(previous_local, local, disk);
    let group_ids: BTreeSet<_> = groups.iter().map(|group| group.id.as_str()).collect();
    let group_conflicts: Vec<_> = workspaces
        .iter()
        .filter(|workspace| {
            workspace
                .group_id
                .as_deref()
                .is_some_and(|id| !group_ids.contains(id))
        })
        .map(|workspace| workspace.name.clone())
        .collect();
    if !group_conflicts.is_empty() {
        return Err(group_conflicts);
    }

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
        // Members name their group per workspace, so membership merges with
        // workspaces; group metadata merges per group.
        groups,
        ..AppSessionState::default()
    };
    normalize_pane_ids(&mut result, disk);
    for workspace in &result.workspaces {
        let id = workspace.id.as_deref().expect("merged workspace identity");
        if accepted.contains(id) {
            ancestry.insert(id, workspace);
        }
    }
    // The saved order is ours, unless another window reordered it unseen.
    let mut ancestry_order = if local_reordered || !remote_reordered {
        workspace_order(&result)
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
