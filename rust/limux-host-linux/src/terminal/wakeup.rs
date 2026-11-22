use gtk4::glib;
use std::sync::atomic::{fence, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

type Tick = Box<dyn FnMut() + Send>;

const PHASE_IDLE: u8 = 0;
const PHASE_PENDING: u8 = 1;
const PHASE_RUNNING: u8 = 2;
const PHASE_REQUESTED: u8 = 3;

const FALLBACK_DELAY: Duration = Duration::from_millis(4);

struct Pending {
    idle: glib::Source,
    fallback: glib::Source,
    tick: Tick,
}

#[derive(Default)]
struct State {
    pending: Option<Pending>,
}

#[derive(Default)]
struct Shared {
    phase: AtomicU8,
    state: Mutex<State>,
}

/// Coalesce Ghostty mailbox notifications without polling an idle app.
#[derive(Default)]
pub(super) struct Wakeup {
    shared: Arc<Shared>,
}

impl Wakeup {
    pub(super) fn queue(&self, context: &glib::MainContext, tick: impl FnMut() + Send + 'static) {
        // Pair with `phase.store(PHASE_RUNNING, SeqCst)` in `dispatch_tick` so a
        // producer's mailbox push before `queue()` can never be reordered past
        // our phase observation on weak or x86 StoreLoad paths.
        fence(Ordering::SeqCst);
        loop {
            match self.shared.phase.load(Ordering::SeqCst) {
                PHASE_PENDING | PHASE_REQUESTED => return,
                PHASE_RUNNING => {
                    if self
                        .shared
                        .phase
                        .compare_exchange_weak(
                            PHASE_RUNNING,
                            PHASE_REQUESTED,
                            Ordering::SeqCst,
                            Ordering::SeqCst,
                        )
                        .is_ok()
                    {
                        return;
                    }
                }
                PHASE_IDLE => break,
                _ => unreachable!(),
            }
        }

        let mut state = self.shared.state.lock().unwrap();
        match self.shared.phase.load(Ordering::SeqCst) {
            PHASE_PENDING | PHASE_REQUESTED => {}
            PHASE_RUNNING => {
                self.shared.phase.store(PHASE_REQUESTED, Ordering::SeqCst);
            }
            PHASE_IDLE => {
                attach_tick(context, &self.shared, &mut state, Box::new(tick));
                self.shared.phase.store(PHASE_PENDING, Ordering::SeqCst);
            }
            _ => unreachable!(),
        }
    }
}

fn attach_tick(context: &glib::MainContext, shared: &Arc<Shared>, state: &mut State, tick: Tick) {
    let dispatch = || {
        let context = context.clone();
        let shared = shared.clone();
        move || {
            dispatch_tick(&context, &shared);
            glib::ControlFlow::Break
        }
    };
    let idle = glib::idle_source_new(
        Some("limux-ghostty-wakeup"),
        glib::Priority::DEFAULT_IDLE,
        dispatch(),
    );
    // Preserve the immediate idle path and keep a default-priority fallback so
    // mailbox work cannot starve under busy input, while leaving enough room
    // for GTK frame paints (GDK_PRIORITY_REDRAW = 120) under sustained output.
    let fallback = glib::timeout_source_new(
        FALLBACK_DELAY,
        Some("limux-ghostty-wakeup-fallback"),
        glib::Priority::DEFAULT,
        dispatch(),
    );
    idle.attach(Some(context));
    fallback.attach(Some(context));
    // The state lock prevents either callback from running before both sources
    // are attached and their cancellation handles are available.
    state.pending = Some(Pending {
        idle,
        fallback,
        tick,
    });
}

fn dispatch_tick(context: &glib::MainContext, shared: &Arc<Shared>) {
    let mut pending = {
        let mut state = shared.state.lock().unwrap();
        let Some(pending) = state.pending.take() else {
            return;
        };
        shared.phase.store(PHASE_RUNNING, Ordering::SeqCst);
        pending
    };
    pending.idle.destroy();
    pending.fallback.destroy();
    (pending.tick)();

    let mut state = shared.state.lock().unwrap();
    if shared
        .phase
        .compare_exchange(
            PHASE_RUNNING,
            PHASE_IDLE,
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_err()
    {
        // Arm the next fallback after the drain. It must not already be due
        // when a long tick returns and prevent GTK's frame sources from running.
        attach_tick(context, shared, &mut state, pending.tick);
        shared.phase.store(PHASE_PENDING, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    fn run_until(context: &glib::MainContext, ready: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(1);
        while !ready() {
            assert!(
                Instant::now() < deadline,
                "main context stopped delivering work"
            );
            context.iteration(false);
            std::thread::yield_now();
        }
    }

    #[test]
    fn coalesces_wakeups_and_leaves_idle_context_without_a_timer() {
        let context = glib::MainContext::new();
        let wakeup = Wakeup::default();
        let ticks = Arc::new(AtomicUsize::new(0));
        for _ in 0..64 {
            let ticks = ticks.clone();
            wakeup.queue(&context, move || {
                ticks.fetch_add(1, Ordering::Relaxed);
            });
        }

        run_until(&context, || ticks.load(Ordering::Relaxed) > 0);
        assert_eq!(ticks.load(Ordering::Relaxed), 1);
        std::thread::sleep(Duration::from_millis(10));
        assert!(!context.pending(), "idle app retained a polling source");
        assert!(!context.iteration(false));
    }

    #[test]
    fn wakeup_from_another_thread_during_tick_gets_its_own_turn() {
        let context = glib::MainContext::new();
        let wakeup = Arc::new(Wakeup::default());
        let ticks = Arc::new(AtomicUsize::new(0));
        wakeup.queue(&context, {
            let context = context.clone();
            let wakeup = wakeup.clone();
            let ticks = ticks.clone();
            move || {
                if ticks.fetch_add(1, Ordering::Relaxed) == 0 {
                    let context = context.clone();
                    let wakeup = wakeup.clone();
                    let ticks = ticks.clone();
                    std::thread::spawn(move || {
                        wakeup.queue(&context, move || {
                            ticks.fetch_add(1, Ordering::Relaxed);
                        });
                    })
                    .join()
                    .unwrap();
                }
            }
        });

        run_until(&context, || ticks.load(Ordering::Relaxed) == 2);
        assert!(!context.pending());
    }

    #[test]
    fn tick_survives_busy_input_and_yields_to_frame_sources() {
        let context = glib::MainContext::new();
        let wakeup = Arc::new(Wakeup::default());
        let ticks = Arc::new(AtomicUsize::new(0));
        let input = glib::idle_source_new(None, glib::Priority::DEFAULT, || {
            glib::ControlFlow::Continue
        });
        input.attach(Some(&context));
        wakeup.queue(&context, {
            let ticks = ticks.clone();
            move || {
                ticks.fetch_add(1, Ordering::Relaxed);
            }
        });
        run_until(&context, || ticks.load(Ordering::Relaxed) == 1);
        input.destroy();
        assert!(!context.pending(), "fallback left a stale idle callback");

        let frames = Arc::new(AtomicUsize::new(0));
        // GTK frame-clock paint sources run at GDK_PRIORITY_REDRAW (120).
        let frame = glib::idle_source_new(None, glib::Priority::from(120), {
            let frames = frames.clone();
            move || {
                frames.fetch_add(1, Ordering::Relaxed);
                glib::ControlFlow::Continue
            }
        });
        frame.attach(Some(&context));
        wakeup.queue(&context, {
            let context = context.clone();
            let wakeup = wakeup.clone();
            let ticks = ticks.clone();
            let frames = frames.clone();
            move || {
                let previous = ticks.fetch_add(1, Ordering::Relaxed);
                if previous == 1 {
                    frames.store(0, Ordering::Relaxed);
                }
                if previous < 9 {
                    wakeup.queue(&context, || {});
                }
                // A refill arrives before a tick exceeding the timer interval
                // completes, as it can under sustained terminal output.
                std::thread::sleep(Duration::from_millis(2));
            }
        });
        run_until(&context, || ticks.load(Ordering::Relaxed) == 10);
        frame.destroy();
        assert!(
            frames.load(Ordering::Relaxed) > 0,
            "ticks starved GTK frames"
        );
    }

    #[test]
    fn high_frequency_cross_thread_wakeups_never_lose_final_message() {
        let context = glib::MainContext::new();
        let wakeup = Arc::new(Wakeup::default());
        let produced = Arc::new(AtomicUsize::new(0));
        let observed = Arc::new(AtomicUsize::new(0));
        let total = 2_000usize;

        let producer = {
            let context = context.clone();
            let wakeup = wakeup.clone();
            let produced = produced.clone();
            let observed = observed.clone();
            std::thread::spawn(move || {
                for seq in 1..=total {
                    produced.store(seq, Ordering::SeqCst);
                    let produced = produced.clone();
                    let observed = observed.clone();
                    wakeup.queue(&context, move || {
                        let latest = produced.load(Ordering::SeqCst);
                        observed.store(latest, Ordering::SeqCst);
                    });
                }
            })
        };

        let deadline = Instant::now() + Duration::from_secs(5);
        while observed.load(Ordering::SeqCst) < total {
            assert!(
                Instant::now() < deadline,
                "lost final wakeup: observed={} expected={total}",
                observed.load(Ordering::SeqCst)
            );
            context.iteration(false);
            std::thread::yield_now();
        }
        producer.join().unwrap();
        assert!(!context.pending());
    }
}
