use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SessionSaveRequest {
    Ignore,
    RetryOnIdle,
    Schedule,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SessionCloseMode {
    Save,
    AcceptRecovery,
}

pub(super) trait SessionSaveAccess {
    fn persistence_suspended(&self) -> bool;
    fn save_queued(&self) -> bool;
    fn set_save_queued(&mut self, queued: bool);
}

impl SessionSaveAccess for AppState {
    fn persistence_suspended(&self) -> bool {
        self.persistence_suspended
    }

    fn save_queued(&self) -> bool {
        self.save_queued
    }

    fn set_save_queued(&mut self, queued: bool) {
        self.save_queued = queued;
    }
}

pub(super) fn queue_session_save_request<T: SessionSaveAccess>(
    state: &Rc<RefCell<T>>,
) -> SessionSaveRequest {
    let Ok(mut s) = state.try_borrow_mut() else {
        return SessionSaveRequest::RetryOnIdle;
    };

    if s.persistence_suspended() || s.save_queued() {
        SessionSaveRequest::Ignore
    } else {
        s.set_save_queued(true);
        SessionSaveRequest::Schedule
    }
}

pub(crate) fn request_session_save(state: &State) {
    match queue_session_save_request(state) {
        SessionSaveRequest::Ignore => {}
        SessionSaveRequest::RetryOnIdle => {
            let state = state.clone();
            glib::idle_add_local_once(move || {
                request_session_save(&state);
            });
        }
        SessionSaveRequest::Schedule => {
            let callback_state = state.clone();
            let timer = glib::timeout_add_local_once(SESSION_SAVE_DELAY, move || {
                let state = callback_state;
                let should_save = {
                    let mut s = state.borrow_mut();
                    s.session_save_timer.take();
                    let should_save = s.save_queued && !s.persistence_suspended;
                    s.save_queued = false;
                    should_save
                };
                if should_save {
                    save_session_now(&state);
                }
            });
            state.borrow_mut().session_save_timer = Some(timer);
        }
    }
}

pub(super) fn session_conflict_detail(recovery: &Path, workspaces: &[String]) -> String {
    format!("Another Limux window changed the same workspace(s): {}. The shared session was not overwritten. This window's complete session was saved to {}.", workspaces.join(", "), recovery.display())
}

pub(super) fn report_session_save_notice(state: &State, detail: String) {
    if state.borrow().session_save_notice.as_ref() == Some(&detail) {
        return;
    }
    state.borrow_mut().session_save_notice = Some(detail.clone());
    eprintln!("limux: {detail}");
    show_runtime_error(state, "Session could not be merged", &detail);
}

pub(super) fn save_session_for_rpc(
    state: &State,
    reply: std::sync::mpsc::Sender<crate::control_bridge::BridgeResult>,
) {
    {
        let mut s = state.borrow_mut();
        if s.persistence_suspended {
            let _ = reply.send(Err(crate::control_bridge::BridgeError::conflict(
                "session persistence is suspended",
            )));
            return;
        }
        if s.session_close_dialog_open {
            let _ = reply.send(Err(crate::control_bridge::BridgeError::conflict(
                "session close dialog is open",
            )));
            return;
        }
        s.session_save_waiters.push(reply);
    }
    save_session_now(state);
}

fn reject_session_save_waiters(
    waiters: Vec<std::sync::mpsc::Sender<crate::control_bridge::BridgeResult>>,
    error: crate::control_bridge::BridgeError,
) {
    for waiter in waiters {
        let _ = waiter.send(Err(error.clone()));
    }
}

fn notify_session_save_waiters(
    waiters: Vec<std::sync::mpsc::Sender<crate::control_bridge::BridgeResult>>,
    result: &Result<crate::session_store::SaveOutcome, String>,
) {
    if waiters.is_empty() {
        return;
    }
    let response = match result {
        Ok(crate::session_store::SaveOutcome::Saved(summary)) => Ok(serde_json::json!({
            "saved": true,
            "path": summary.path.to_string_lossy(),
            "workspaces": summary.workspaces,
            "tabs": summary.tabs,
            "restorable_agents": summary.restorable_agents,
        })),
        Ok(crate::session_store::SaveOutcome::Conflict {
            recovery,
            workspaces,
        }) => Err(crate::control_bridge::BridgeError::conflict(
            session_conflict_detail(recovery, workspaces),
        )),
        Err(error) => Err(crate::control_bridge::BridgeError::internal(format!(
            "Failed to save session: {error}"
        ))),
    };
    for waiter in waiters {
        let _ = waiter.send(response.clone());
    }
}

pub(super) fn save_session_now(state: &State) {
    let (store, waiters) = {
        let mut s = state.borrow_mut();
        if s.persistence_suspended {
            let waiters = std::mem::take(&mut s.session_save_waiters);
            drop(s);
            reject_session_save_waiters(
                waiters,
                crate::control_bridge::BridgeError::conflict("session persistence is suspended"),
            );
            return;
        }
        if let Some(timer) = s.session_save_timer.take() {
            timer.remove();
        }
        if s.session_close_dialog_open {
            s.save_queued = true;
            let waiters = std::mem::take(&mut s.session_save_waiters);
            drop(s);
            reject_session_save_waiters(
                waiters,
                crate::control_bridge::BridgeError::conflict("session close dialog is open"),
            );
            return;
        }
        s.save_queued = false;
        match s.session_store.as_mut() {
            Ok(store) => match store.take() {
                Some(store) => (Ok(store), std::mem::take(&mut s.session_save_waiters)),
                None => {
                    // The in-flight writer owns the store and its merge ancestry.
                    // Its completion captures the latest GTK state before saving again,
                    // carrying any queued RPC waiters onto that fresh save.
                    s.save_queued = true;
                    return;
                }
            },
            Err(error) => (
                Err(format!("Session storage could not be opened: {error}. Resolve the storage error and restart before making session changes.")),
                std::mem::take(&mut s.session_save_waiters),
            ),
        }
    };
    let store = match store {
        Ok(store) => store,
        Err(error) => {
            notify_session_save_waiters(waiters, &Err(error.clone()));
            finish_session_save(state, Err(error));
            return;
        }
    };
    let session = snapshot_session_state(state);
    let task = store.save_in_background(session);
    let state = state.clone();
    glib::MainContext::default().spawn_local(async move {
        let result = match task.await {
            Ok((store, saved_session, result)) => {
                state.borrow_mut().session_store = Ok(Some(store));
                // A close request must include changes made while the worker was
                // saving, including control-socket commands behind a modal dialog.
                let closing = state.borrow().session_close_pending.is_some();
                if closing && result.is_ok() && snapshot_session_state(&state) != saved_session {
                    if !waiters.is_empty() {
                        let mut s = state.borrow_mut();
                        let mut requeued = waiters;
                        requeued.append(&mut s.session_save_waiters);
                        s.session_save_waiters = requeued;
                    }
                    save_session_now(&state);
                    return;
                }
                result.map_err(|error| error.to_string())
            }
            Err(_) => {
                let error = "Session writer stopped unexpectedly".to_string();
                state.borrow_mut().session_store = Err(error.clone());
                Err(error)
            }
        };
        notify_session_save_waiters(waiters, &result);
        finish_session_save(&state, result);
        let (pending, dialog_waiters) = {
            let mut s = state.borrow_mut();
            let dialog_waiters = if s.session_close_dialog_open {
                std::mem::take(&mut s.session_save_waiters)
            } else {
                Vec::new()
            };
            let pending =
                s.save_queued && s.session_save_timer.is_none() && !s.session_close_dialog_open;
            (pending, dialog_waiters)
        };
        if !dialog_waiters.is_empty() {
            reject_session_save_waiters(
                dialog_waiters,
                crate::control_bridge::BridgeError::conflict("session close dialog is open"),
            );
        }
        if pending {
            save_session_now(&state);
        }
    });
}

pub(super) fn finish_session_save(
    state: &State,
    result: Result<crate::session_store::SaveOutcome, String>,
) {
    let closing = state.borrow_mut().session_close_pending.take();
    if let Some(mode) = closing {
        finish_session_close(state, mode, result);
        return;
    }
    match result {
        Ok(crate::session_store::SaveOutcome::Saved(_)) => {
            state.borrow_mut().session_save_notice = None
        }
        Ok(crate::session_store::SaveOutcome::Conflict {
            recovery,
            workspaces,
        }) => {
            report_session_save_notice(state, session_conflict_detail(&recovery, &workspaces));
        }
        Err(error) => report_session_save_notice(state, format!("Failed to save session: {error}")),
    }
}

pub(super) fn prepare_session_close(state: &State) -> bool {
    if std::mem::take(&mut state.borrow_mut().close_after_recovery) {
        return true;
    }
    if state.borrow().session_close_dialog_open || state.borrow().session_close_pending.is_some() {
        return false;
    }
    state.borrow_mut().session_close_pending = Some(SessionCloseMode::Save);
    save_session_now(state);
    false
}

pub(super) fn close_after_session_save(state: &State) {
    state.borrow_mut().close_after_recovery = true;
    let window = state.borrow().window.clone();
    window.close();
}

pub(super) fn resume_session_saves(state: &State) {
    {
        let mut s = state.borrow_mut();
        s.save_queued = false;
        if let Some(timer) = s.session_save_timer.take() {
            timer.remove();
        }
    }
    request_session_save(state);
}

pub(super) fn finish_session_close(
    state: &State,
    mode: SessionCloseMode,
    result: Result<crate::session_store::SaveOutcome, String>,
) {
    match result {
        Ok(crate::session_store::SaveOutcome::Saved(_)) => close_after_session_save(state),
        Err(error) => {
            let window = state.borrow().window.clone();
            let dialog = gtk::AlertDialog::builder().modal(true)
                .message("Close without saving this session?")
                .detail(format!("The session could not be saved: {error}. Closing will discard this window's unsaved session changes."))
                .buttons(["Cancel", "Close without Saving"]).cancel_button(0).default_button(0).build();
            let waiters = {
                let mut s = state.borrow_mut();
                s.session_close_dialog_open = true;
                std::mem::take(&mut s.session_save_waiters)
            };
            reject_session_save_waiters(
                waiters,
                crate::control_bridge::BridgeError::conflict("session close dialog is open"),
            );
            let state = state.clone();
            dialog.choose(Some(&window), gio::Cancellable::NONE, move |answer| {
                state.borrow_mut().session_close_dialog_open = false;
                if answer == Ok(1) {
                    close_after_session_save(&state);
                } else {
                    resume_session_saves(&state);
                }
            });
        }
        Ok(crate::session_store::SaveOutcome::Conflict {
            recovery,
            workspaces,
        }) => {
            if mode == SessionCloseMode::AcceptRecovery {
                close_after_session_save(state);
                return;
            }
            let detail = session_conflict_detail(&recovery, &workspaces);
            let window = state.borrow().window.clone();
            let dialog = gtk::AlertDialog::builder()
                .modal(true)
                .message("Close with a saved recovery session?")
                .detail(&detail)
                .buttons(["Cancel", "Close after Saving Recovery"])
                .cancel_button(0)
                .default_button(0)
                .build();
            let waiters = {
                let mut s = state.borrow_mut();
                s.session_close_dialog_open = true;
                std::mem::take(&mut s.session_save_waiters)
            };
            reject_session_save_waiters(
                waiters,
                crate::control_bridge::BridgeError::conflict("session close dialog is open"),
            );
            let state = state.clone();
            dialog.choose(Some(&window), gio::Cancellable::NONE, move |answer| {
                state.borrow_mut().session_close_dialog_open = false;
                if answer != Ok(1) {
                    resume_session_saves(&state);
                    return;
                }
                // Control-socket commands can still change the session while the
                // dialog is open. Capture the latest state before allowing close.
                state.borrow_mut().session_close_pending = Some(SessionCloseMode::AcceptRecovery);
                save_session_now(&state);
            });
        }
    }
}

pub(super) fn suspend_persistence(state: &State, suspended: bool) {
    state.borrow_mut().persistence_suspended = suspended;
}

pub(super) fn stop_session_saves_for_shutdown(state: &State) {
    let waiters = {
        let mut s = state.borrow_mut();
        s.save_queued = false;
        s.persistence_suspended = true;
        if let Some(timer) = s.session_save_timer.take() {
            timer.remove();
        }
        std::mem::take(&mut s.session_save_waiters)
    };
    for waiter in waiters {
        let _ = waiter.send(Err(crate::control_bridge::BridgeError::conflict(
            "window is closing",
        )));
    }
}

pub(super) fn apply_loaded_session(state: &State, mut loaded: LoadedSession) {
    suspend_persistence(state, true);

    apply_top_bar_state_immediately(state, loaded.state.top_bar_visible);

    let restorable_agents = layout_state::RestorableAgentIndex::load();
    let restored_any = !loaded.state.workspaces.is_empty();
    if restored_any {
        layout_state::attach_restorable_agents_to_session(&mut loaded.state, &restorable_agents);
        for workspace in &loaded.state.workspaces {
            add_restored_workspace_from_state(state, workspace);
        }
    } else {
        let workspace = initial_workspace_state(
            dirs::home_dir().as_deref(),
            std::env::current_dir().ok().as_deref(),
        );
        add_restored_workspace_from_state(state, &workspace);
    }
    // Empty groups survive even when startup has to create a fallback workspace.
    // Restoring them can repair the order, so track the active workspace by id.
    let active_id = state
        .borrow()
        .workspaces
        .get(loaded.state.active_workspace_index)
        .map(|workspace| workspace.id.clone());
    sidebar_groups::restore_groups(state, &loaded.state);
    let active_index = active_id
        .and_then(|id| {
            state
                .borrow()
                .workspaces
                .iter()
                .position(|workspace| workspace.id == id)
        })
        .unwrap_or(loaded.state.active_workspace_index);
    restore_active_workspace(state, active_index);
    state.borrow_mut().last_workspace_id = None;
    apply_sidebar_state_immediately(state, &loaded.state.sidebar);

    let mut restored = snapshot_session_state(state);
    layout_state::attach_restorable_agents_to_session(&mut restored, &restorable_agents);
    if let Ok(Some(store)) = state.borrow_mut().session_store.as_mut() {
        store.restored(restored);
    }
    suspend_persistence(state, false);

    save_session_now(state);
}

pub(super) fn restore_active_workspace(state: &State, index: usize) {
    let target = {
        let mut s = state.borrow_mut();
        if s.workspaces.is_empty() {
            None
        } else {
            let clamped = index.min(s.workspaces.len() - 1);
            s.active_idx = clamped;
            sync_indicator_active_state(&s);
            let stack_name = format!("ws-{}", s.workspaces[clamped].id);
            Some((
                s.stack.clone(),
                stack_name,
                s.workspaces[clamped].root.clone(),
                s.workspaces[clamped].sidebar_row.clone(),
                s.sidebar_list.clone(),
            ))
        }
    };

    if let Some((stack, stack_name, focus_root, row, sidebar_list)) = target {
        stack.set_visible_child_name(&stack_name);
        select_sidebar_row_silently(state, &sidebar_list, &row);
        sidebar_groups::sync_row_visibility(state);
        glib::idle_add_local_once(move || {
            focus_workspace_entrypoint(&focus_root);
        });
        apply_top_bar_mode(state);
    }
}

pub(super) fn apply_sidebar_state_immediately(
    state: &State,
    sidebar_state: &layout_state::SidebarState,
) {
    let (sidebar_shell, sidebar_handle, width) = {
        let mut s = state.borrow_mut();
        s.sidebar_expanded_width = sidebar_state.width.max(SIDEBAR_WIDTH);
        (
            s.sidebar_shell.clone(),
            s.sidebar_handle.clone(),
            s.sidebar_expanded_width,
        )
    };

    // Apply restored sidebar visibility directly; using the animated toggle path during
    // startup would create flicker and extra persistence churn while restore is suspended.
    set_sidebar_state_widgets(
        &sidebar_shell,
        &sidebar_handle,
        if sidebar_state.visible { width } else { 0 },
        sidebar_state.visible,
    );
    apply_top_bar_mode(state);
    sync_sidebar_toggle(state);
}

pub(super) fn apply_top_bar_state_immediately(state: &State, visible: bool) {
    state.borrow_mut().top_bar_visible = visible;
    sync_top_bar_visibility(state);
}

pub(super) fn snapshot_session_state(state: &State) -> AppSessionState {
    let s = state.borrow();
    let sidebar_visible = sidebar_is_visible(&s);
    let sidebar_width = if sidebar_visible {
        sidebar_width(&s.sidebar_shell)
    } else {
        s.sidebar_expanded_width
    }
    .max(SIDEBAR_WIDTH);

    let favorites = s.sidebar.favorite_ids();
    let group_ids = s.sidebar.workspace_group_ids();
    let workspaces = s
        .workspaces
        .iter()
        .map(|workspace| {
            let cwd = workspace.cwd.borrow().clone();
            let folder_path = workspace.folder_path.clone();
            let working_directory = folder_path.clone().or(cwd.clone());
            let layout = workspace
                .split_container
                .tree()
                .snapshot(working_directory.as_deref());
            WorkspaceState {
                id: Some(workspace.id.clone()),
                name: workspace.name.clone(),
                favorite: favorites.contains(workspace.id.as_str()),
                cwd,
                folder_path,
                autostart_command: workspace.autostart_command.borrow().clone(),
                color: workspace.color,
                group_id: group_ids
                    .get(workspace.id.as_str())
                    .map(|id| (*id).to_string()),
                layout,
            }
        })
        .collect();

    layout_state::normalize_session(AppSessionState {
        version: layout_state::SESSION_VERSION,
        active_workspace_index: s.active_idx,
        top_bar_visible: s.top_bar_visible,
        sidebar: layout_state::SidebarState {
            visible: sidebar_visible,
            width: sidebar_width,
        },
        workspaces,
        groups: s.sidebar.saved_groups(),
    })
}

pub(super) fn sidebar_is_visible(state: &AppState) -> bool {
    // `is_visible` is false for a shown sidebar until its window is presented,
    // and the session is restored before that.
    state.sidebar_shell.get_visible() && sidebar_width(&state.sidebar_shell) > 10
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notify_and_reject_session_save_waiters_deliver_expected_bridge_results() {
        let (tx_ok, rx_ok) = std::sync::mpsc::channel();
        let summary = crate::session_store::SavedSessionSummary {
            path: PathBuf::from("/tmp/limux/session.json"),
            workspaces: 2,
            tabs: 5,
            restorable_agents: 1,
        };
        notify_session_save_waiters(
            vec![tx_ok],
            &Ok(crate::session_store::SaveOutcome::Saved(summary)),
        );
        let payload = rx_ok.try_recv().expect("saved reply").expect("ok payload");
        assert_eq!(payload["saved"], true);
        assert_eq!(payload["path"], "/tmp/limux/session.json");
        assert_eq!(payload["workspaces"], 2);
        assert_eq!(payload["tabs"], 5);
        assert_eq!(payload["restorable_agents"], 1);

        let (tx_conflict, rx_conflict) = std::sync::mpsc::channel();
        notify_session_save_waiters(
            vec![tx_conflict],
            &Ok(crate::session_store::SaveOutcome::Conflict {
                recovery: PathBuf::from("/tmp/recovery.json"),
                workspaces: vec!["ws-1".to_string()],
            }),
        );
        let conflict_err = rx_conflict
            .try_recv()
            .expect("conflict reply")
            .expect_err("conflict error");
        let conflict_debug = format!("{conflict_err:?}");
        assert!(conflict_debug.contains("ws-1"), "{conflict_debug}");

        let (tx_dialog, rx_dialog) = std::sync::mpsc::channel();
        reject_session_save_waiters(
            vec![tx_dialog],
            crate::control_bridge::BridgeError::conflict("session close dialog is open"),
        );
        let dialog_err = rx_dialog
            .try_recv()
            .expect("dialog reply")
            .expect_err("dialog conflict");
        let dialog_debug = format!("{dialog_err:?}");
        assert!(
            dialog_debug.contains("session close dialog is open"),
            "{dialog_debug}"
        );
    }
}
