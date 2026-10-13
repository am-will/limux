use std::os::raw::{c_char, c_void};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, Instant};

use gtk4::glib;

use super::{SurfaceIdentity, SURFACE_MAP};

const DELIVERY_INTERVAL: Duration = Duration::from_millis(100);
static EPOCH: OnceLock<Instant> = OnceLock::new();

pub(super) struct OutputActivity {
    identity: OnceLock<SurfaceIdentity>,
    // Odd generations are enabled. A new generation rejects callbacks that
    // started before the setting was toggled off and back on.
    enabled_generation: AtomicU64,
    queued: AtomicBool,
    last_output_nanos: AtomicU64,
    last_output_generation: AtomicU64,
    weak: Weak<Self>,
}

impl OutputActivity {
    pub(super) fn new(enabled: bool) -> Arc<Self> {
        EPOCH.get_or_init(Instant::now);
        Arc::new_cyclic(|weak| Self {
            identity: OnceLock::new(),
            enabled_generation: AtomicU64::new(u64::from(enabled)),
            queued: AtomicBool::new(false),
            last_output_nanos: AtomicU64::new(0),
            last_output_generation: AtomicU64::new(0),
            weak: weak.clone(),
        })
    }

    pub(super) fn bind(&self, identity: SurfaceIdentity) {
        self.identity
            .set(identity)
            .expect("output activity bound once");
    }

    pub(super) fn set_enabled(&self, enabled: bool) {
        let generation = self.enabled_generation.load(Ordering::Acquire);
        if (generation & 1 != 0) != enabled {
            self.enabled_generation.fetch_add(1, Ordering::AcqRel);
        }
    }

    fn receive(&self) {
        let generation = self.enabled_generation.load(Ordering::Acquire);
        if generation & 1 == 0 {
            return;
        }
        self.record(generation, Instant::now());
        if !self.queued.swap(true, Ordering::AcqRel) {
            if let Some(activity) = self.weak.upgrade() {
                glib::timeout_add_once(DELIVERY_INTERVAL, move || activity.deliver());
            }
        }
    }

    fn record(&self, generation: u64, at: Instant) {
        let elapsed = at.duration_since(*EPOCH.get().expect("activity clock initialized"));
        self.last_output_nanos.store(
            u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        self.last_output_generation
            .store(generation, Ordering::Release);
    }

    fn latest_output(&self) -> Option<Instant> {
        let generation = self.enabled_generation.load(Ordering::Acquire);
        if generation & 1 == 0 || self.last_output_generation.load(Ordering::Acquire) != generation
        {
            return None;
        }
        let nanos = self.last_output_nanos.load(Ordering::Relaxed);
        if nanos == 0 {
            return None;
        }
        Some(*EPOCH.get().expect("activity clock initialized") + Duration::from_nanos(nanos))
    }

    fn deliver(&self) {
        self.queued.store(false, Ordering::Release);
        let Some(at) = self.latest_output() else {
            return;
        };
        let Some(identity) = self.identity.get() else {
            return;
        };
        let callback = SURFACE_MAP.with(|map| {
            map.borrow()
                .get(&identity.surface_key)
                .filter(|entry| entry.identity == *identity)
                .map(|entry| entry.output_callbacks.borrow().on_output_activity.clone())
        });
        if let Some(callback) = callback {
            callback(at);
        }
    }
}

pub(super) unsafe extern "C" fn pty_output_cb(
    userdata: *mut c_void,
    _bytes: *const c_char,
    len: usize,
) {
    if len == 0 {
        return;
    }
    // The surface entry owns this Arc until ghostty_surface_free joins the IO
    // thread. The callback never reads terminal state or touches GTK widgets.
    let activity = unsafe { &*userdata.cast::<OutputActivity>() };
    activity.receive();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_captured_before_disable_is_not_replayed_after_reenable() {
        let activity = OutputActivity::new(true);
        let previous_generation = activity.enabled_generation.load(Ordering::Acquire);
        activity.set_enabled(false);
        activity.set_enabled(true);
        let at = Instant::now();
        activity.record(previous_generation, at);
        assert_eq!(activity.latest_output(), None);
        let generation = activity.enabled_generation.load(Ordering::Acquire);
        activity.record(generation, at);
        assert_eq!(activity.latest_output(), Some(at));
        activity.set_enabled(false);
        assert_eq!(activity.latest_output(), None);
    }
}
