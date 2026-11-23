//! Check WebKit in a disposable process before it can abort the terminal host.

#[cfg(feature = "webkit")]
mod probe;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BrowserSandbox {
    pub(crate) sandboxed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ProbeState {
    #[cfg(feature = "webkit")]
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

    struct ProbeController {
        slot: Mutex<ProbeSlot>,
        condvar: Condvar,
        cancel: Arc<AtomicBool>,
        worker: Mutex<Option<JoinHandle<()>>>,
    }

    impl ProbeController {
        fn new() -> Self {
            Self {
                slot: Mutex::new(ProbeSlot::Unstarted),
                condvar: Condvar::new(),
                cancel: Arc::new(AtomicBool::new(false)),
                worker: Mutex::new(None),
            }
        }
    }

    static CONTROLLER: LazyLock<ProbeController> = LazyLock::new(ProbeController::new);

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
        let controller = &*CONTROLLER;
        let should_spawn = {
            let mut slot = controller
                .slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match &*slot {
                ProbeSlot::Ready(result) => return Some(result.clone()),
                ProbeSlot::Running => false,
                ProbeSlot::Unstarted => {
                    *slot = ProbeSlot::Running;
                    true
                }
            }
        };

        if should_spawn {
            let cancel = controller.cancel.clone();
            let handle = std::thread::Builder::new()
                .name("limux-browser-probe".to_string())
                .spawn(move || {
                    let result = probe::check_with_cancel(Some(cancel.as_ref()));
                    if let Err(error) = &result {
                        if !cancel.load(Ordering::Acquire) {
                            eprintln!("limux: {error}");
                        }
                    }
                    let controller = &*CONTROLLER;
                    {
                        let mut slot = controller
                            .slot
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        *slot = ProbeSlot::Ready(result.clone());
                    }
                    controller.condvar.notify_all();
                    glib::MainContext::default().invoke(move || {
                        flush_main_thread_waiters(&result);
                    });
                });

            match handle {
                Ok(join_handle) => {
                    *controller
                        .worker
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(join_handle);
                }
                Err(error) => {
                    let err = Err(format!(
                        "Could not start browser sandbox probe thread: {error}"
                    ));
                    {
                        let mut slot = controller
                            .slot
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        *slot = ProbeSlot::Ready(err.clone());
                    }
                    controller.condvar.notify_all();
                    return Some(err);
                }
            }
        }

        None
    }

    pub(super) fn register_main_thread_waiter(
        on_ready: impl FnOnce(&Result<BrowserSandbox, String>) + 'static,
    ) {
        let ready = {
            let slot = CONTROLLER
                .slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match &*slot {
                ProbeSlot::Ready(result) => Some(result.clone()),
                _ => None,
            }
        };
        if let Some(result) = ready {
            flush_main_thread_waiters(&result);
            on_ready(&result);
            return;
        }
        MAIN_THREAD_WAITERS.with(|slot| {
            slot.borrow_mut().push(Box::new(on_ready));
        });
        let ready_after_push = {
            let slot = CONTROLLER
                .slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match &*slot {
                ProbeSlot::Ready(result) => Some(result.clone()),
                _ => None,
            }
        };
        if let Some(result) = ready_after_push {
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

    pub(super) fn wait_for_availability_blocking() -> Result<BrowserSandbox, String> {
        if let Some(result) = ensure_started() {
            return result;
        }
        let controller = &*CONTROLLER;
        let mut slot = controller
            .slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            match &*slot {
                ProbeSlot::Ready(result) => return result.clone(),
                ProbeSlot::Unstarted | ProbeSlot::Running => {
                    slot = controller
                        .condvar
                        .wait(slot)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
            }
        }
    }

    pub(super) fn cancel_and_join_on_shutdown() {
        let controller = &*CONTROLLER;
        controller.cancel.store(true, Ordering::Release);
        let handle = controller
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }
}

#[cfg(feature = "webkit")]
pub(crate) fn register_main_thread_waiter(
    on_ready: impl FnOnce(&Result<BrowserSandbox, String>) + 'static,
) {
    async_probe::register_main_thread_waiter(on_ready);
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
