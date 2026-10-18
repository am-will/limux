use super::*;
use crate::layout_state::{LayoutNodeState, PaneState, SplitOrientation, SplitState, TabState};
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
        color: None,
        group_id: None,
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
fn background_save_keeps_the_main_context_responsive_and_preserves_merge_ancestry() {
    use std::time::{Duration, Instant};
    let dir = tempdir().unwrap();
    let base = initial(dir.path());
    let (store, _) = SessionStore::load_from_dir(dir.path()).unwrap();
    let lock = lock_directory(dir.path()).unwrap();
    let mut local = base.clone();
    local.workspaces[0].name = "first local edit".into();
    let context = gtk4::glib::MainContext::new();
    context
        .with_thread_default(|| {
            context.block_on(async {
                let start = Instant::now();
                let task = store.save_in_background(local.clone());
                gtk4::glib::timeout_future(Duration::from_millis(50)).await;
                assert!(start.elapsed() < Duration::from_secs(1));
                let mut remote = base;
                remote.workspaces[1].name = "independent remote edit".into();
                layout_state::save_session_atomic_in(dir.path(), &remote).unwrap();
                drop(lock);
                let (store, snapshot, result) = task.await.unwrap();
                assert!(matches!(result.unwrap(), SaveOutcome::Saved));
                assert_eq!(snapshot, local);

                local.workspaces[0].name = "latest local edit".into();
                let (_, snapshot, result) = store.save_in_background(local.clone()).await.unwrap();
                assert!(matches!(result.unwrap(), SaveOutcome::Saved));
                assert_eq!(snapshot, local);
                let saved = disk(dir.path());
                assert_eq!(saved.workspaces[0].name, "latest local edit");
                assert_eq!(saved.workspaces[1].name, "independent remote edit");
            });
        })
        .unwrap();
}

#[test]
fn background_save_returns_conflicts_and_preserves_the_latest_recovery() {
    let dir = tempdir().unwrap();
    let base = initial(dir.path());
    let (store, _) = SessionStore::load_from_dir(dir.path()).unwrap();
    let mut remote = base.clone();
    remote.workspaces[0].name = "remote edit".into();
    layout_state::save_session_atomic_in(dir.path(), &remote).unwrap();
    let mut local = base;
    local.workspaces[0].name = "local edit".into();
    let context = gtk4::glib::MainContext::new();
    context
        .with_thread_default(|| {
            context.block_on(async {
                let (store, _, result) = store.save_in_background(local.clone()).await.unwrap();
                assert!(matches!(result.unwrap(), SaveOutcome::Conflict { .. }));
                local.workspaces[0].name = "edit while confirming recovery".into();
                let (_, _, result) = store.save_in_background(local.clone()).await.unwrap();
                let SaveOutcome::Conflict { recovery, .. } = result.unwrap() else {
                    panic!("stale local edits must still conflict");
                };
                let recovered: AppSessionState =
                    serde_json::from_slice(&fs::read(recovery).unwrap()).unwrap();
                assert_eq!(recovered, local);
                assert_eq!(disk(dir.path()), remote);
            });
        })
        .unwrap();
}

#[test]
fn background_save_returns_errors_without_losing_the_store_or_unsaved_changes() {
    let dir = tempdir().unwrap();
    let base = initial(dir.path());
    let (store, _) = SessionStore::load_from_dir(dir.path()).unwrap();
    let mut local = base.clone();
    local.workspaces[0].name = "unsaved edit".into();
    let path = layout_state::canonical_session_path_in(dir.path());
    fs::write(&path, "invalid session").unwrap();
    let context = gtk4::glib::MainContext::new();
    context
        .with_thread_default(|| {
            context.block_on(async {
                let (store, snapshot, result) =
                    store.save_in_background(local.clone()).await.unwrap();
                assert!(result.is_err());
                assert_eq!(snapshot, local);
                assert_eq!(fs::read_to_string(&path).unwrap(), "invalid session");
                layout_state::save_session_atomic_in(dir.path(), &base).unwrap();
                let (_, _, result) = store.save_in_background(local.clone()).await.unwrap();
                assert!(matches!(result.unwrap(), SaveOutcome::Saved));
                assert_eq!(disk(dir.path()), local);
            });
        })
        .unwrap();
}

#[test]
#[ignore = "manual before/after session-save contention measurement; no display required"]
fn measure_session_save_lock_contention() {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};
    for background in [false, true] {
        for trial in 1..=3 {
            let dir = tempdir().unwrap();
            let state = AppSessionState {
                workspaces: (1..=20)
                    .map(|id| {
                        let mut workspace = workspace(id);
                        let LayoutNodeState::Pane(pane) = &mut workspace.layout else {
                            unreachable!();
                        };
                        for tab in 1..if id <= 15 { 3 } else { 2 } {
                            pane.tabs
                                .push(TabState::terminal(format!("tab-{id}-{tab}"), Some("/tmp")));
                        }
                        workspace
                    })
                    .collect(),
                ..AppSessionState::default()
            };
            layout_state::save_session_atomic_in(dir.path(), &state).unwrap();
            let (mut store, _) = SessionStore::load_from_dir(dir.path()).unwrap();
            let lock = lock_directory(dir.path()).unwrap();
            let context = gtk4::glib::MainContext::new();
            context
                .with_thread_default(|| {
                    let max_gap = Rc::new(Cell::new(Duration::ZERO));
                    let ticks = Rc::new(Cell::new(0));
                    let tick_max = max_gap.clone();
                    let tick_count = ticks.clone();
                    let heartbeat = context.spawn_local(async move {
                        let mut last = Instant::now();
                        loop {
                            gtk4::glib::timeout_future(Duration::from_millis(10)).await;
                            let now = Instant::now();
                            tick_max.set(tick_max.get().max(now.duration_since(last)));
                            tick_count.set(tick_count.get() + 1);
                            last = now;
                        }
                    });
                    context.block_on(async {
                        gtk4::glib::timeout_future(Duration::from_millis(30)).await;
                        let owner = std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(1200));
                            drop(lock);
                        });
                        let start = Instant::now();
                        if background {
                            let (_, _, result) =
                                store.save_in_background(state.clone()).await.unwrap();
                            assert!(matches!(result.unwrap(), SaveOutcome::Saved));
                        } else {
                            saved(&mut store, &state);
                        }
                        let save_elapsed = start.elapsed();
                        gtk4::glib::timeout_future(Duration::from_millis(30)).await;
                        owner.join().unwrap();
                        assert_eq!(disk(dir.path()), state);
                        eprintln!(
                            "session-save background={background} trial={trial} workspaces=20 tabs=55 lock_ms=1200 elapsed_ms={:.3} heartbeat_max_gap_ms={:.3} heartbeat_ticks={}",
                            save_elapsed.as_secs_f64() * 1000.0,
                            max_gap.get().as_secs_f64() * 1000.0,
                            ticks.get(),
                        );
                    });
                    heartbeat.abort();
                })
                .unwrap();
        }
    }
}

fn split_workspace(ratio: f64) -> WorkspaceState {
    let mut state = workspace(1);
    state.layout = LayoutNodeState::Split(SplitState {
        orientation: SplitOrientation::Horizontal,
        ratio,
        start: Box::new(state.layout),
        end: Box::new(workspace(3).layout),
    });
    state
}

#[test]
fn fractional_split_ratio_survives_repeated_edits_and_reload() {
    let dir = tempdir().unwrap();
    initial(dir.path());
    let (mut store, loaded) = SessionStore::load_from_dir(dir.path()).unwrap();
    let mut local = loaded.state;
    // This ratio previously lost one ULP when parsed from the saved JSON.
    local.workspaces[0] = split_workspace(0.49948293691830403);
    saved(&mut store, &local);

    for name in ["first edit", "second edit"] {
        local.workspaces[0].name = name.into();
        saved(&mut store, &local);
        assert_eq!(disk(dir.path()), local);
    }

    let (mut reloaded, loaded) = SessionStore::load_from_dir(dir.path()).unwrap();
    assert_eq!(loaded.state, local);
    reloaded.restored(loaded.state);
    local.workspaces[0].name = "edit after reload".into();
    saved(&mut reloaded, &local);
    assert_eq!(disk(dir.path()), local);
}

#[test]
fn concurrent_fractional_split_ratio_edits_still_conflict() {
    let dir = tempdir().unwrap();
    let mut base = initial(dir.path());
    base.workspaces[0] = split_workspace(0.5);
    layout_state::save_session_atomic_in(dir.path(), &base).unwrap();
    let (mut a, _) = SessionStore::load_from_dir(dir.path()).unwrap();
    let (mut b, _) = SessionStore::load_from_dir(dir.path()).unwrap();
    let ratio: f64 = 0.49948293691830403;
    let mut ours = base.clone();
    ours.workspaces[0] = split_workspace(ratio);
    let mut theirs = base;
    theirs.workspaces[0] = split_workspace(f64::from_bits(ratio.to_bits() + 1));
    saved(&mut a, &ours);

    let SaveOutcome::Conflict {
        recovery,
        workspaces,
    } = b.save(&theirs).unwrap()
    else {
        panic!("expected conflict between distinct split ratios")
    };
    assert_eq!(workspaces, vec!["workspace-1"]);
    assert_eq!(disk(dir.path()), ours);
    let recovered: AppSessionState = serde_json::from_slice(&fs::read(recovery).unwrap()).unwrap();
    assert_eq!(recovered, theirs);
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
fn local_reorder_preserves_remote_addition_order() {
    let dir = tempdir().unwrap();
    let base = initial(dir.path());
    let (mut remote_store, _) = SessionStore::load_from_dir(dir.path()).unwrap();
    let (mut local_store, _) = SessionStore::load_from_dir(dir.path()).unwrap();
    let mut remote = base.clone();
    remote.workspaces.extend([workspace(40), workspace(30)]);
    saved(&mut remote_store, &remote);
    let mut local = base;
    local.workspaces.swap(0, 1);
    saved(&mut local_store, &local);
    assert_eq!(
        disk(dir.path())
            .workspaces
            .iter()
            .map(|workspace| workspace.name.as_str())
            .collect::<Vec<_>>(),
        vec!["workspace-2", "workspace-1", "workspace-40", "workspace-30"]
    );
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

#[test]
fn load_gives_fresh_ids_to_duplicate_tab_ids_from_older_sessions() {
    // Sessions saved before issue #201 start every workspace at "terminal-0",
    // and a pane that hit the bug holds that id twice.
    let dir = tempdir().unwrap();
    let mut first = workspace(1);
    first.layout = LayoutNodeState::Pane(PaneState {
        pane_id: Some(1),
        active_tab_id: Some("terminal-0".into()),
        tabs: vec![
            TabState::terminal("terminal-0", Some("/tmp")),
            TabState::terminal("terminal-0", Some("/tmp")),
        ],
    });
    let mut second = workspace(2);
    second.layout = LayoutNodeState::Pane(PaneState {
        pane_id: Some(2),
        active_tab_id: Some("terminal-0".into()),
        tabs: vec![
            TabState::terminal("tab-2", Some("/tmp")),
            TabState::terminal("terminal-0", Some("/tmp")),
        ],
    });
    let mut third = workspace(3);
    third.layout = LayoutNodeState::Pane(PaneState {
        pane_id: Some(3),
        active_tab_id: Some("browser-0".into()),
        tabs: vec![TabState::browser("browser-0", None)],
    });
    // Two renamed duplicates in one pane: the selection must follow the active one.
    let mut fourth = workspace(4);
    fourth.layout = LayoutNodeState::Pane(PaneState {
        pane_id: Some(4),
        active_tab_id: Some("browser-0".into()),
        tabs: vec![
            TabState::terminal("terminal-0", Some("/tmp")),
            TabState::browser("browser-0", None),
        ],
    });
    let state = AppSessionState {
        workspaces: vec![first, second, third, fourth],
        ..AppSessionState::default()
    };
    layout_state::save_session_atomic_in(dir.path(), &state).unwrap();

    let (_store, loaded) = SessionStore::load_from_dir(dir.path()).unwrap();

    let panes: Vec<_> = loaded
        .state
        .workspaces
        .iter()
        .map(|workspace| match &workspace.layout {
            LayoutNodeState::Pane(pane) => pane.clone(),
            LayoutNodeState::Split(_) => panic!("expected a single pane"),
        })
        .collect();
    let ids: Vec<_> = panes
        .iter()
        .flat_map(|pane| pane.tabs.iter().map(|tab| tab.id.clone()))
        .collect();
    let unique: BTreeSet<_> = ids.iter().collect();
    assert_eq!(unique.len(), ids.len(), "duplicate tab ids: {ids:?}");
    // The first occurrence keeps its id, so its agent hook records still match.
    assert_eq!(panes[0].tabs[0].id, "terminal-0");
    assert_eq!(panes[0].active_tab_id.as_deref(), Some("terminal-0"));
    assert_eq!(panes[1].tabs[0].id, "tab-2");
    // The renamed active tab stays the active one.
    assert_eq!(
        panes[1].active_tab_id.as_deref(),
        Some(panes[1].tabs[1].id.as_str())
    );
    assert_eq!(panes[2].tabs[0].id, "browser-0");
    assert_eq!(
        panes[3].active_tab_id.as_deref(),
        Some(panes[3].tabs[1].id.as_str())
    );
    // Migrated once under the lock, so every later load sees the same ids.
    assert_eq!(disk(dir.path()), loaded.state);
    let (_store, reloaded) = SessionStore::load_from_dir(dir.path()).unwrap();
    assert_eq!(reloaded.state, loaded.state);
}

#[test]
fn load_persists_ids_minted_for_empty_panes() {
    // Normalization gives an empty pane (only a hand-edited file has one) a fresh tab
    // id; unless that id reaches the disk, every later read mints another one and each
    // save looks like a remote edit.
    let dir = tempdir().unwrap();
    let mut state = initial(dir.path());
    state.workspaces[1].layout = LayoutNodeState::Pane(PaneState {
        pane_id: Some(2),
        active_tab_id: None,
        tabs: Vec::new(),
    });
    fs::write(
        layout_state::canonical_session_path_in(dir.path()),
        serde_json::to_vec(&state).unwrap(),
    )
    .unwrap();

    let (_store, loaded) = SessionStore::load_from_dir(dir.path()).unwrap();

    assert_eq!(disk(dir.path()), loaded.state);
}

fn group(id: &str, name: &str) -> layout_state::WorkspaceGroupState {
    layout_state::WorkspaceGroupState {
        id: id.to_string(),
        name: name.to_string(),
        color: Some(crate::workspace_color::WorkspaceColor::Teal),
        collapsed: false,
        position: 0,
    }
}

#[test]
fn groups_survive_a_remote_save_and_follow_the_newer_edit() {
    let dir = tempdir().unwrap();
    let base = initial(dir.path());
    let (mut a, _) = SessionStore::load_from_dir(dir.path()).unwrap();
    let (mut b, _) = SessionStore::load_from_dir(dir.path()).unwrap();

    // Window A groups workspace 1.
    let mut ours = base.clone();
    ours.groups = vec![group("g", "Backend")];
    ours.workspaces[0].group_id = Some("g".to_string());
    saved(&mut a, &ours);

    // Window B, which never touched groups, saves an unrelated addition.
    let mut theirs = base;
    theirs.workspaces.push(workspace(3));
    saved(&mut b, &theirs);
    let result = disk(dir.path());
    assert_eq!(result.groups, vec![group("g", "Backend")]);
    assert_eq!(result.workspaces[0].group_id.as_deref(), Some("g"));

    // A later rename in B wins over A's unchanged groups.
    let mut renamed = result.clone();
    renamed.groups[0].name = "Services".to_string();
    saved(&mut b, &renamed);
    saved(&mut a, &ours);
    assert_eq!(disk(dir.path()).groups, vec![group("g", "Services")]);
}

#[test]
fn grouped_favorites_keep_their_place_across_saves() {
    let dir = tempdir().unwrap();
    let mut base = initial(dir.path());
    base.workspaces.push(workspace(3));
    base.groups = vec![group("g", "Backend")];
    base.workspaces[1].group_id = Some("g".to_string());
    base.workspaces[2].group_id = Some("g".to_string());
    base.workspaces[2].favorite = true;
    // The sidebar order: workspace 1, then group g with its favorite 3 first.
    base.workspaces.swap(1, 2);
    layout_state::save_session_atomic_in(dir.path(), &base).unwrap();
    let (mut store, _) = SessionStore::load_from_dir(dir.path()).unwrap();
    // Collapsing the group is saved without moving anything.
    let mut collapsed = base.clone();
    collapsed.groups[0].collapsed = true;
    saved(&mut store, &collapsed);
    let names = |state: &AppSessionState| -> Vec<String> {
        state
            .workspaces
            .iter()
            .map(|workspace| workspace.name.clone())
            .collect()
    };
    assert_eq!(
        names(&disk(dir.path())),
        ["workspace-1", "workspace-3", "workspace-2"]
    );

    // Reordering again is not mistaken for a clash with another window.
    let mut reordered = collapsed.clone();
    reordered.workspaces.rotate_left(1);
    saved(&mut store, &reordered);
    assert_eq!(
        names(&disk(dir.path())),
        ["workspace-3", "workspace-2", "workspace-1"]
    );
}
