use super::*;
use crate::layout_state::{LayoutNodeState, PaneState, TabState};
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Barrier};
use tempfile::tempdir;

fn workspace(number: u32) -> WorkspaceState {
    WorkspaceState {
        id: Some(format!("00000000-0000-4000-8000-{number:012}")),
        name: format!("workspace-{number}"),
        favorite: false,
        cwd: Some("/tmp".into()),
        folder_path: None,
        autostart_command: None,
        layout: LayoutNodeState::Pane(PaneState {
            pane_id: Some(number),
            active_tab_id: Some(format!("tab-{number}")),
            tabs: vec![TabState::terminal(format!("tab-{number}"), Some("/tmp"))],
        }),
    }
}
fn initial(directory: &Path) -> AppSessionState {
    let state = AppSessionState {
        workspaces: vec![workspace(1), workspace(2)],
        ..AppSessionState::default()
    };
    layout_state::save_session_atomic_in(directory, &state).unwrap();
    state
}
fn saved(store: &mut SessionStore, state: &AppSessionState) {
    assert!(matches!(store.save(state).unwrap(), SaveOutcome::Saved));
}
fn disk(directory: &Path) -> AppSessionState {
    read_session(directory).unwrap().state
}

#[test]
fn independent_additions_survive_repeated_saves_and_reverse_close_order() {
    let dir = tempdir().unwrap();
    let base = initial(dir.path());
    let (mut a, _) = SessionStore::load_from_dir(dir.path()).unwrap();
    let (mut b, _) = SessionStore::load_from_dir(dir.path()).unwrap();
    let mut ours = base.clone();
    ours.workspaces.push(workspace(3));
    saved(&mut a, &ours);
    let mut theirs = base;
    theirs.workspaces[0].name = "renamed by b".into();
    saved(&mut b, &theirs);
    saved(&mut a, &ours);
    saved(&mut b, &theirs);
    let result = disk(dir.path());
    assert_eq!(result.workspaces.len(), 3);
    assert_eq!(result.workspaces[0].name, "renamed by b");
    assert_eq!(result.workspaces[2].name, "workspace-3");
}

#[test]
fn intentional_deletion_is_not_resurrected_by_stale_noop_saves() {
    let dir = tempdir().unwrap();
    let base = initial(dir.path());
    let (mut a, _) = SessionStore::load_from_dir(dir.path()).unwrap();
    let (mut b, _) = SessionStore::load_from_dir(dir.path()).unwrap();
    let mut ours = base.clone();
    ours.workspaces.remove(1);
    saved(&mut a, &ours);
    saved(&mut b, &base);
    saved(&mut b, &base);
    assert_eq!(disk(dir.path()).workspaces.len(), 1);
}

#[test]
fn stale_edit_after_observing_remote_edit_still_conflicts() {
    let dir = tempdir().unwrap();
    let base = initial(dir.path());
    let (mut a, _) = SessionStore::load_from_dir(dir.path()).unwrap();
    let (mut b, _) = SessionStore::load_from_dir(dir.path()).unwrap();
    let mut theirs = base.clone();
    theirs.workspaces[0].name = "remote".into();
    saved(&mut b, &theirs);
    saved(&mut a, &base);
    saved(&mut a, &base);
    let mut ours = base;
    ours.workspaces[0].name = "local".into();
    let SaveOutcome::Conflict { recovery, .. } = a.save(&ours).unwrap() else {
        panic!("expected conflict")
    };
    assert_eq!(disk(dir.path()).workspaces[0].name, "remote");
    let recovered: AppSessionState = serde_json::from_slice(&fs::read(&recovery).unwrap()).unwrap();
    assert_eq!(recovered, ours);
    assert_eq!(
        fs::metadata(&recovery).unwrap().permissions().mode() & 0o777,
        0o600
    );
    ours.workspaces[1].name = "later local change".into();
    let SaveOutcome::Conflict {
        recovery: latest, ..
    } = a.save(&ours).unwrap()
    else {
        panic!("expected conflict")
    };
    assert_eq!(latest, recovery);
    let recovered: AppSessionState = serde_json::from_slice(&fs::read(latest).unwrap()).unwrap();
    assert_eq!(recovered, ours);
}

#[test]
fn concurrent_delete_and_edit_conflict_in_both_directions() {
    for delete_first in [false, true] {
        let dir = tempdir().unwrap();
        let base = initial(dir.path());
        let (mut a, _) = SessionStore::load_from_dir(dir.path()).unwrap();
        let (mut b, _) = SessionStore::load_from_dir(dir.path()).unwrap();
        let mut deleted = base.clone();
        deleted.workspaces.remove(0);
        let mut edited = base;
        edited.workspaces[0].name = "edited".into();
        let (first, second) = if delete_first {
            (&deleted, &edited)
        } else {
            (&edited, &deleted)
        };
        saved(&mut a, first);
        assert!(matches!(
            b.save(second).unwrap(),
            SaveOutcome::Conflict { .. }
        ));
        assert_eq!(disk(dir.path()), *first);
    }
}

#[test]
fn restored_layout_differences_are_not_user_edits() {
    let dir = tempdir().unwrap();
    let base = initial(dir.path());
    let (mut a, _) = SessionStore::load_from_dir(dir.path()).unwrap();
    let (mut b, _) = SessionStore::load_from_dir(dir.path()).unwrap();
    let mut restored = base.clone();
    restored.workspaces[0].cwd = Some("/restored".into());
    a.restored(restored.clone());
    let mut remote = base;
    remote.workspaces[0].name = "remote".into();
    saved(&mut b, &remote);
    saved(&mut a, &restored);
    saved(&mut a, &restored);
    assert_eq!(disk(dir.path()).workspaces[0], remote.workspaces[0]);
    restored.workspaces[0].name = "explicit local edit".into();
    assert!(matches!(
        a.save(&restored).unwrap(),
        SaveOutcome::Conflict { .. }
    ));
}

#[test]
fn simultaneous_writers_do_not_lose_independent_workspaces() {
    let dir = tempdir().unwrap();
    initial(dir.path());
    let barrier = Arc::new(Barrier::new(8));
    let handles: Vec<_> = (10..18)
        .map(|number| {
            let path = dir.path().to_path_buf();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let (mut store, loaded) = SessionStore::load_from_dir(&path).unwrap();
                let mut state = loaded.state;
                state.workspaces.push(workspace(number));
                barrier.wait();
                saved(&mut store, &state);
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(disk(dir.path()).workspaces.len(), 10);
}

#[test]
fn lock_inode_survives_atomic_replacements_and_blocks_other_writers() {
    use std::os::unix::fs::MetadataExt;
    use std::sync::mpsc;
    use std::time::Duration;
    let dir = tempdir().unwrap();
    let base = initial(dir.path());
    let first = lock_directory(dir.path()).unwrap();
    let inode = first.metadata().unwrap().ino();
    let path = dir.path().to_path_buf();
    let (tx, rx) = mpsc::channel();
    let writer = std::thread::spawn(move || {
        let lock = lock_directory(&path).unwrap();
        tx.send(lock.metadata().unwrap().ino()).unwrap();
    });
    assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    layout_state::save_session_atomic_in(dir.path(), &base).unwrap();
    drop(first);
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), inode);
    writer.join().unwrap();
}

#[test]
fn invalid_canonical_file_is_not_overwritten() {
    let dir = tempdir().unwrap();
    let base = initial(dir.path());
    let (mut store, _) = SessionStore::load_from_dir(dir.path()).unwrap();
    let path = layout_state::canonical_session_path_in(dir.path());
    fs::write(&path, "not json").unwrap();
    assert!(store.save(&base).is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), "not json");
}

#[test]
fn colliding_pane_ids_are_unique_and_remain_editable_after_saving() {
    let dir = tempdir().unwrap();
    let base = initial(dir.path());
    let (mut a, _) = SessionStore::load_from_dir(dir.path()).unwrap();
    let (mut b, _) = SessionStore::load_from_dir(dir.path()).unwrap();
    let mut ours = base.clone();
    ours.workspaces.push(workspace(3));
    let mut theirs = base;
    let mut addition = workspace(4);
    if let LayoutNodeState::Pane(pane) = &mut addition.layout {
        pane.pane_id = Some(3);
    }
    theirs.workspaces.push(addition);
    saved(&mut a, &ours);
    saved(&mut b, &theirs);
    let result = disk(dir.path());
    let ids: BTreeSet<_> = result
        .workspaces
        .iter()
        .map(|workspace| match &workspace.layout {
            LayoutNodeState::Pane(pane) => pane.pane_id.unwrap(),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(ids.len(), 4);
    theirs.workspaces[2].name = "renamed after normalized save".into();
    saved(&mut b, &theirs);
    saved(&mut a, &ours);
    saved(&mut b, &theirs);
    assert_eq!(
        disk(dir.path()).workspaces[3].name,
        "renamed after normalized save"
    );
}

#[test]
fn merge_keeps_selected_workspace_by_identity_and_local_reordering() {
    let dir = tempdir().unwrap();
    let base = initial(dir.path());
    let (mut a, _) = SessionStore::load_from_dir(dir.path()).unwrap();
    let (mut b, _) = SessionStore::load_from_dir(dir.path()).unwrap();
    let mut ours = base.clone();
    ours.workspaces.push(workspace(3));
    saved(&mut a, &ours);
    let mut theirs = base;
    theirs.workspaces.swap(0, 1);
    theirs.active_workspace_index = 0;
    saved(&mut b, &theirs);
    saved(&mut b, &theirs);
    let result = disk(dir.path());
    assert_eq!(
        result
            .workspaces
            .iter()
            .map(|workspace| workspace.name.as_str())
            .collect::<Vec<_>>(),
        vec!["workspace-2", "workspace-1", "workspace-3"]
    );
    assert_eq!(selected_id(&result), selected_id(&theirs));
}

#[test]
fn concurrent_workspace_reorders_do_not_silently_overwrite_each_other() {
    let dir = tempdir().unwrap();
    let mut base = initial(dir.path());
    base.workspaces.push(workspace(3));
    layout_state::save_session_atomic_in(dir.path(), &base).unwrap();
    let (mut a, _) = SessionStore::load_from_dir(dir.path()).unwrap();
    let (mut b, _) = SessionStore::load_from_dir(dir.path()).unwrap();
    let mut ours = base.clone();
    ours.workspaces.swap(0, 1);
    saved(&mut a, &ours);
    saved(&mut b, &base);
    let mut theirs = base;
    theirs.workspaces.swap(1, 2);
    assert!(matches!(
        b.save(&theirs).unwrap(),
        SaveOutcome::Conflict { .. }
    ));
    assert_eq!(disk(dir.path()), ours);
}

#[test]
fn accepted_reorder_retains_nonlexical_ancestry_on_later_edits() {
    let dir = tempdir().unwrap();
    let base = AppSessionState {
        workspaces: vec![workspace(30), workspace(10), workspace(20)],
        ..AppSessionState::default()
    };
    layout_state::save_session_atomic_in(dir.path(), &base).unwrap();
    let (mut store, _) = SessionStore::load_from_dir(dir.path()).unwrap();
    saved(&mut store, &base);
    let mut local = base;
    local.workspaces.swap(0, 1);
    saved(&mut store, &local);
    local.workspaces.swap(1, 2);
    saved(&mut store, &local);
    saved(&mut store, &local);
    assert_eq!(disk(dir.path()), local);
}

#[test]
fn busy_lock_has_a_bounded_wait() {
    let dir = tempdir().unwrap();
    let _owner = lock_directory(dir.path()).unwrap();
    let start = std::time::Instant::now();
    assert_eq!(
        lock_directory(dir.path()).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    assert!(start.elapsed() < std::time::Duration::from_secs(4));
}

#[test]
fn startup_lock_failure_can_retry_without_losing_independent_remote_edits() {
    let dir = tempdir().unwrap();
    let base = initial(dir.path());
    let owner = lock_directory(dir.path()).unwrap();
    assert_eq!(
        SessionStore::load_from_dir(dir.path())
            .err()
            .unwrap()
            .kind(),
        io::ErrorKind::WouldBlock
    );
    let (mut local_store, loaded) = SessionStore::load_for_retry_from_dir(dir.path()).unwrap();
    local_store.restored(loaded.state.clone());
    drop(owner);

    let (mut remote_store, _) = SessionStore::load_from_dir(dir.path()).unwrap();
    let mut remote = base;
    remote.workspaces[0].name = "remote change after startup".into();
    saved(&mut remote_store, &remote);
    let mut local = loaded.state;
    local.workspaces[1].name = "local change after startup".into();
    saved(&mut local_store, &local);
    saved(&mut local_store, &local);
    let result = disk(dir.path());
    assert_eq!(result.workspaces[0].name, "remote change after startup");
    assert_eq!(result.workspaces[1].name, "local change after startup");
}

#[test]
fn startup_retry_does_not_adopt_concurrent_same_workspace_edit_as_ancestry() {
    let dir = tempdir().unwrap();
    let base = initial(dir.path());
    let owner = lock_directory(dir.path()).unwrap();
    let (mut local_store, loaded) = SessionStore::load_for_retry_from_dir(dir.path()).unwrap();
    local_store.restored(loaded.state.clone());
    drop(owner);

    let (mut remote_store, _) = SessionStore::load_from_dir(dir.path()).unwrap();
    let mut remote = base;
    remote.workspaces[0].name = "remote".into();
    saved(&mut remote_store, &remote);
    let mut local = loaded.state;
    local.workspaces[0].name = "local".into();
    let SaveOutcome::Conflict { recovery, .. } = local_store.save(&local).unwrap() else {
        panic!("expected conflict after startup retry")
    };
    assert_eq!(disk(dir.path()), remote);
    let recovered: AppSessionState = serde_json::from_slice(&fs::read(recovery).unwrap()).unwrap();
    assert_eq!(recovered, local);
}

#[test]
fn startup_retry_rejects_unreadable_corrupt_or_ambiguous_snapshots() {
    let dir = tempdir().unwrap();
    let mut base = initial(dir.path());
    let path = layout_state::canonical_session_path_in(dir.path());
    fs::write(&path, "invalid json").unwrap();
    assert!(SessionStore::load_for_retry_from_dir(dir.path()).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "invalid json");

    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(SessionStore::load_for_retry_from_dir(dir.path()).is_err());
    assert!(path.is_dir());
    fs::remove_dir(&path).unwrap();

    base.workspaces[1].id = base.workspaces[0].id.clone();
    layout_state::save_session_atomic_in(dir.path(), &base).unwrap();
    assert!(SessionStore::load_for_retry_from_dir(dir.path()).is_err());
    assert_eq!(disk(dir.path()), base);

    base.workspaces[1].id = None;
    layout_state::save_session_atomic_in(dir.path(), &base).unwrap();
    assert!(SessionStore::load_for_retry_from_dir(dir.path()).is_err());
    assert_eq!(disk(dir.path()), base);
}

#[test]
fn identity_migration_is_shared_by_independent_loaders() {
    let dir = tempdir().unwrap();
    let mut initial = initial(dir.path());
    initial.workspaces[0].id = None;
    layout_state::save_session_atomic_in(dir.path(), &initial).unwrap();
    let (_, a) = SessionStore::load_from_dir(dir.path()).unwrap();
    let (_, b) = SessionStore::load_from_dir(dir.path()).unwrap();
    assert_eq!(a.state, b.state);
    assert!(uuid::Uuid::parse_str(a.state.workspaces[0].id.as_deref().unwrap()).is_ok());
}

#[test]
fn remote_favorite_stays_above_locally_reordered_workspaces() {
    let dir = tempdir().unwrap();
    let base = initial(dir.path());
    let (mut a, _) = SessionStore::load_from_dir(dir.path()).unwrap();
    let (mut b, _) = SessionStore::load_from_dir(dir.path()).unwrap();
    let mut ours = base.clone();
    let mut addition = workspace(3);
    addition.favorite = true;
    ours.workspaces.insert(0, addition);
    saved(&mut a, &ours);
    let mut theirs = base;
    theirs.workspaces.swap(0, 1);
    saved(&mut b, &theirs);
    let result = disk(dir.path());
    assert!(result.workspaces[0].favorite);
    assert_eq!(result.workspaces[1].name, "workspace-2");
}
