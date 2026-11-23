//! Check WebKit in a disposable process before it can abort the terminal host.

#[cfg(feature = "webkit")]
mod probe;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BrowserSandbox {
    pub(crate) sandboxed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ProbeState {
    #[cfg_attr(not(feature = "webkit"), allow(dead_code))]
    Pending,
    Ready(Result<BrowserSandbox, String>),
}

pub(crate) fn capture_launch_env() {
    #[cfg(feature = "webkit")]
    probe::capture_launch_env();
}

#[cfg(feature = "webkit")]
mod async_probe {
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Condvar, LazyLock, Mutex};
    use std::thread::JoinHandle;

    use gtk4::glib;

    use super::{probe, BrowserSandbox, ProbeState};

    type MainThreadWaiter = Box<dyn FnOnce(&Result<BrowserSandbox, String>)>;

    enum ProbeSlot {
        Unstarted,
        Running,
        Ready(Result<BrowserSandbox, String>),
    }

    pub(super) struct ProbeController {
        slot: Mutex<ProbeSlot>,
        condvar: Condvar,
        cancel: Arc<AtomicBool>,
        worker: Mutex<Option<JoinHandle<()>>>,
    }

    impl ProbeController {
        pub(super) fn new() -> Self {
            Self {
                slot: Mutex::new(ProbeSlot::Unstarted),
                condvar: Condvar::new(),
                cancel: Arc::new(AtomicBool::new(false)),
                worker: Mutex::new(None),
            }
        }

        pub(super) fn peek_ready(&self) -> Option<Result<BrowserSandbox, String>> {
            let slot = self
                .slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match &*slot {
                ProbeSlot::Ready(result) => Some(result.clone()),
                ProbeSlot::Unstarted | ProbeSlot::Running => None,
            }
        }

        pub(super) fn ensure_started_with<F, N>(
            self: &Arc<Self>,
            run_probe: F,
            notify_main_thread: N,
        ) -> Option<Result<BrowserSandbox, String>>
        where
            F: FnOnce(&AtomicBool) -> Result<BrowserSandbox, String> + Send + 'static,
            N: FnOnce(Result<BrowserSandbox, String>) + Send + 'static,
        {
            // Fast path: already running or ready without contending on worker lock.
            {
                let slot = self
                    .slot
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                match &*slot {
                    ProbeSlot::Ready(result) => return Some(result.clone()),
                    ProbeSlot::Running => return None,
                    ProbeSlot::Unstarted => {}
                }
            }

            // Hold `worker` across the cancel check, spawn, and handle storage
            // so `cancel_and_join_on_shutdown()` can never race with or miss a
            // newly spawned probe thread. Lock order is always `worker` -> `slot`.
            let mut worker = self
                .worker
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());

            {
                let mut slot = self
                    .slot
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                match &*slot {
                    ProbeSlot::Ready(result) => return Some(result.clone()),
                    ProbeSlot::Running => return None,
                    ProbeSlot::Unstarted => {
                        if self.cancel.load(Ordering::Acquire) {
                            let cancelled =
                                Err("Browser startup check cancelled during shutdown.".to_string());
                            *slot = ProbeSlot::Ready(cancelled.clone());
                            drop(slot);
                            notify_main_thread(cancelled.clone());
                            self.condvar.notify_all();
                            return Some(cancelled);
                        }
                        *slot = ProbeSlot::Running;
                    }
                }
            }

            let notify = Arc::new(Mutex::new(Some(notify_main_thread)));
            let notify_bg = Arc::clone(&notify);
            let controller = Arc::clone(self);
            let handle = std::thread::Builder::new()
                .name("limux-browser-probe".to_string())
                .spawn(move || {
                    let result = run_probe(controller.cancel.as_ref());
                    if let Err(error) = &result {
                        if !controller.cancel.load(Ordering::Acquire) {
                            eprintln!("limux: {error}");
                        }
                    }
                    {
                        let mut slot = controller
                            .slot
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        *slot = ProbeSlot::Ready(result.clone());
                    }
                    // Queue main-thread waiter upgrades BEFORE waking blocked
                    // control-socket threads so pending browser tabs upgrade
                    // ahead of control commands dispatched onto MainContext.
                    if let Some(notify_fn) = notify_bg
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take()
                    {
                        notify_fn(result);
                    }
                    controller.condvar.notify_all();
                });

            match handle {
                Ok(join_handle) => {
                    *worker = Some(join_handle);
                    None
                }
                Err(error) => {
                    let err = Err(format!(
                        "Could not start browser sandbox probe thread: {error}"
                    ));
                    {
                        let mut slot = self
                            .slot
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        *slot = ProbeSlot::Ready(err.clone());
                    }
                    if let Some(notify_fn) = notify
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take()
                    {
                        notify_fn(err.clone());
                    }
                    self.condvar.notify_all();
                    Some(err)
                }
            }
        }

        pub(super) fn wait_for_ready(&self) -> Result<BrowserSandbox, String> {
            let mut slot = self
                .slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            loop {
                match &*slot {
                    ProbeSlot::Ready(result) => return result.clone(),
                    ProbeSlot::Unstarted | ProbeSlot::Running => {
                        slot = self
                            .condvar
                            .wait(slot)
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                    }
                }
            }
        }

        pub(super) fn cancel_and_join(&self) {
            let handle = {
                let mut worker = self
                    .worker
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                self.cancel.store(true, Ordering::Release);
                worker.take()
            };
            if let Some(handle) = handle {
                let _ = handle.join();
            }
        }
    }

    static CONTROLLER: LazyLock<Arc<ProbeController>> =
        LazyLock::new(|| Arc::new(ProbeController::new()));

    thread_local! {
        static MAIN_THREAD_WAITERS: RefCell<Vec<MainThreadWaiter>> =
            const { RefCell::new(Vec::new()) };
    }

    fn flush_main_thread_waiters(result: &Result<BrowserSandbox, String>) {
        let waiters = MAIN_THREAD_WAITERS.with(|slot| std::mem::take(&mut *slot.borrow_mut()));
        for waiter in waiters {
            waiter(result);
        }
    }

    fn ensure_started() -> Option<Result<BrowserSandbox, String>> {
        CONTROLLER.ensure_started_with(
            |cancel| probe::check_with_cancel(Some(cancel)),
            |result| {
                glib::MainContext::default().invoke(move || {
                    flush_main_thread_waiters(&result);
                });
            },
        )
    }

    pub(super) fn flush_ready_waiters() {
        if let Some(result) = CONTROLLER.peek_ready() {
            flush_main_thread_waiters(&result);
        }
    }

    pub(super) fn register_main_thread_waiter(
        on_ready: impl FnOnce(&Result<BrowserSandbox, String>) + 'static,
    ) {
        if let Some(result) = CONTROLLER.peek_ready() {
            flush_main_thread_waiters(&result);
            on_ready(&result);
            return;
        }
        MAIN_THREAD_WAITERS.with(|slot| {
            slot.borrow_mut().push(Box::new(on_ready));
        });
        if let Some(result) = CONTROLLER.peek_ready() {
            flush_main_thread_waiters(&result);
        }
    }

    pub(super) fn status_nonblocking_or_start() -> ProbeState {
        if let Some(result) = ensure_started() {
            flush_main_thread_waiters(&result);
            ProbeState::Ready(result)
        } else {
            ProbeState::Pending
        }
    }

    pub(super) fn status_nonblocking_or_start_without_flush() -> ProbeState {
        if let Some(result) = ensure_started() {
            ProbeState::Ready(result)
        } else {
            ProbeState::Pending
        }
    }

    pub(super) fn wait_for_availability_blocking() -> Result<BrowserSandbox, String> {
        if let Some(result) = ensure_started() {
            return result;
        }
        CONTROLLER.wait_for_ready()
    }

    pub(super) fn cancel_and_join_on_shutdown() {
        CONTROLLER.cancel_and_join();
    }
}

#[cfg(feature = "webkit")]
pub(crate) fn register_main_thread_waiter(
    on_ready: impl FnOnce(&Result<BrowserSandbox, String>) + 'static,
) {
    async_probe::register_main_thread_waiter(on_ready);
}

pub(crate) fn flush_ready_waiters() {
    #[cfg(feature = "webkit")]
    async_probe::flush_ready_waiters();
}

#[cfg(feature = "webkit")]
pub(crate) fn status_nonblocking_or_start() -> ProbeState {
    async_probe::status_nonblocking_or_start()
}

#[cfg(not(feature = "webkit"))]
pub(crate) fn status_nonblocking_or_start() -> ProbeState {
    ProbeState::Ready(Err("This Limux build does not include WebKit.".to_string()))
}

#[cfg(feature = "webkit")]
pub(crate) fn status_nonblocking_or_start_without_flush() -> ProbeState {
    async_probe::status_nonblocking_or_start_without_flush()
}

#[cfg(not(feature = "webkit"))]
pub(crate) fn status_nonblocking_or_start_without_flush() -> ProbeState {
    ProbeState::Ready(Err("This Limux build does not include WebKit.".to_string()))
}

#[cfg(feature = "webkit")]
pub(crate) fn wait_for_availability_blocking() -> Result<BrowserSandbox, String> {
    async_probe::wait_for_availability_blocking()
}

#[cfg(not(feature = "webkit"))]
pub(crate) fn wait_for_availability_blocking() -> Result<BrowserSandbox, String> {
    Err("This Limux build does not include WebKit.".to_string())
}

pub(crate) fn cancel_and_join_on_shutdown() {
    #[cfg(feature = "webkit")]
    async_probe::cancel_and_join_on_shutdown();
}

pub(crate) fn run_if_requested() -> Option<i32> {
    #[cfg(feature = "webkit")]
    {
        probe::run_if_requested()
    }
    #[cfg(not(feature = "webkit"))]
    {
        None
    }
}

#[cfg(all(test, feature = "webkit"))]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};
    use std::time::Duration;

    use super::async_probe::ProbeController;
    use super::BrowserSandbox;

    #[test]
    fn probe_controller_spawns_once_notifies_before_unblocking_and_rejects_after_shutdown() {
        let controller = Arc::new(ProbeController::new());
        let runs = Arc::new(AtomicUsize::new(0));
        let notified_before_waiter = Arc::new(AtomicBool::new(false));
        let start_barrier = Arc::new(Barrier::new(2));
        let release_probe = Arc::new(Barrier::new(2));

        let runs_bg = runs.clone();
        let start_bg = start_barrier.clone();
        let release_bg = release_probe.clone();
        let notified_bg = notified_before_waiter.clone();
        let first = controller.ensure_started_with(
            move |_cancel| {
                runs_bg.fetch_add(1, Ordering::SeqCst);
                start_bg.wait();
                release_bg.wait();
                Ok(BrowserSandbox { sandboxed: true })
            },
            move |_result| {
                notified_bg.store(true, Ordering::SeqCst);
            },
        );
        assert_eq!(first, None);
        start_barrier.wait();

        // A concurrent caller while Running must not spawn a second probe.
        let runs_second = runs.clone();
        let second = controller.ensure_started_with(
            move |_| {
                runs_second.fetch_add(1, Ordering::SeqCst);
                Ok(BrowserSandbox { sandboxed: false })
            },
            |_| {},
        );
        assert_eq!(second, None);

        let wait_ctrl = controller.clone();
        let notified_check = notified_before_waiter.clone();
        let waiter = std::thread::spawn(move || {
            let res = wait_ctrl.wait_for_ready();
            let saw_notify = notified_check.load(Ordering::SeqCst);
            (res, saw_notify)
        });

        release_probe.wait();
        let (res, saw_notify) = waiter.join().unwrap();
        assert_eq!(res, Ok(BrowserSandbox { sandboxed: true }));
        assert!(
            saw_notify,
            "main-thread waiter notification must run before waking condvar waiters"
        );
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        controller.cancel_and_join();

        // A controller cancelled before start must fail closed without spawning.
        let cancelled_ctrl = Arc::new(ProbeController::new());
        cancelled_ctrl.cancel_and_join();
        let spawned_after_cancel = Arc::new(AtomicBool::new(false));
        let notified_after_cancel = Arc::new(AtomicBool::new(false));
        let s_flag = spawned_after_cancel.clone();
        let n_flag = notified_after_cancel.clone();
        let immediate = cancelled_ctrl.ensure_started_with(
            move |_| {
                s_flag.store(true, Ordering::SeqCst);
                Ok(BrowserSandbox { sandboxed: true })
            },
            move |_| {
                n_flag.store(true, Ordering::SeqCst);
            },
        );
        assert!(immediate.is_some_and(|r| r.is_err()));
        assert!(!spawned_after_cancel.load(Ordering::SeqCst));
        assert!(notified_after_cancel.load(Ordering::SeqCst));

        // Cancelling an in-flight probe flips the cancel flag and joins cleanly.
        let inflight_ctrl = Arc::new(ProbeController::new());
        let entered = Arc::new(Barrier::new(2));
        let entered_bg = entered.clone();
        let observed_cancel = Arc::new(AtomicBool::new(false));
        let observed_bg = observed_cancel.clone();
        assert_eq!(
            inflight_ctrl.ensure_started_with(
                move |cancel| {
                    entered_bg.wait();
                    while !cancel.load(Ordering::Acquire) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    observed_bg.store(true, Ordering::SeqCst);
                    Err("cancelled".to_string())
                },
                |_| {},
            ),
            None
        );
        entered.wait();
        inflight_ctrl.cancel_and_join();
        assert!(observed_cancel.load(Ordering::SeqCst));
        assert!(inflight_ctrl.peek_ready().is_some_and(|r| r.is_err()));
    }
}
