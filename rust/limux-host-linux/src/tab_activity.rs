use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;

use crate::app_config::InterfaceConfig;

#[derive(Clone)]
pub(crate) struct TabActivity(Rc<ActivityState>);

struct ActivityState {
    spinner: gtk::Spinner,
    last_output: Cell<Option<Instant>>,
    acknowledged_through: Cell<Instant>,
    mapped: Cell<bool>,
    unread: Cell<bool>,
    enabled: Cell<bool>,
    timeout: Cell<Duration>,
    expiry: RefCell<Option<glib::SourceId>>,
}

impl TabActivity {
    pub(crate) fn new(config: &InterfaceConfig) -> Self {
        let spinner = gtk::Spinner::new();
        spinner.set_size_request(12, 12);
        spinner.set_valign(gtk::Align::Center);
        spinner.set_can_target(false);
        spinner.set_visible(false);
        spinner.set_tooltip_text(Some("Recent background terminal output"));
        spinner.update_property(&[gtk::accessible::Property::Label(
            "Recent background terminal output",
        )]);
        spinner.add_css_class("limux-tab-activity");
        Self(Rc::new(ActivityState {
            spinner,
            last_output: Cell::new(None),
            acknowledged_through: Cell::new(Instant::now()),
            mapped: Cell::new(false),
            unread: Cell::new(false),
            enabled: Cell::new(config.show_tab_activity),
            timeout: Cell::new(Duration::from_secs(u64::from(config.tab_activity_seconds))),
            expiry: RefCell::new(None),
        }))
    }

    pub(crate) fn widget(&self) -> &gtk::Spinner {
        &self.0.spinner
    }

    pub(crate) fn watch_content(&self, content: &impl IsA<gtk::Widget>) {
        self.set_mapped(content.is_mapped());
        let weak = Rc::downgrade(&self.0);
        content.connect_map(move |_| {
            if let Some(state) = weak.upgrade() {
                Self(state).set_mapped(true);
            }
        });
        let weak = Rc::downgrade(&self.0);
        content.connect_unmap(move |_| {
            if let Some(state) = weak.upgrade() {
                Self(state).set_mapped(false);
            }
        });
    }

    fn set_mapped(&self, mapped: bool) {
        if self.0.mapped.replace(mapped) != mapped {
            self.clear();
        }
    }

    pub(crate) fn set_unread(&self, unread: bool) {
        if self.0.unread.replace(unread) != unread {
            self.clear();
        }
    }

    fn clear(&self) {
        // PTY delivery is coalesced, so already-seen output may still be queued.
        self.0.acknowledged_through.set(Instant::now());
        self.0.last_output.set(None);
        self.sync();
    }

    pub(crate) fn record_output(&self, at: Instant) {
        if self.0.enabled.get()
            && !self.0.mapped.get()
            && !self.0.unread.get()
            && at > self.0.acknowledged_through.get()
        {
            self.0.last_output.set(Some(at));
            self.sync();
        }
    }

    pub(crate) fn configure(&self, config: &InterfaceConfig) {
        self.0.enabled.set(config.show_tab_activity);
        self.0
            .timeout
            .set(Duration::from_secs(u64::from(config.tab_activity_seconds)));
        if !config.show_tab_activity {
            self.0.acknowledged_through.set(Instant::now());
            self.0.last_output.set(None);
        }
        self.cancel_expiry();
        self.sync();
    }

    fn cancel_expiry(&self) {
        if let Some(source) = self.0.expiry.borrow_mut().take() {
            source.remove();
        }
    }

    fn sync(&self) {
        let remaining = remaining_activity(
            self.0.enabled.get(),
            self.0.last_output.get(),
            self.0.timeout.get(),
            Instant::now(),
        );
        let Some(remaining) = remaining else {
            self.0.spinner.stop();
            self.0.spinner.set_visible(false);
            self.cancel_expiry();
            return;
        };
        self.0.spinner.set_visible(true);
        self.0.spinner.start();
        if self.0.expiry.borrow().is_none() {
            let weak = Rc::downgrade(&self.0);
            let source =
                glib::timeout_add_local_once(remaining.max(Duration::from_millis(1)), move || {
                    if let Some(state) = weak.upgrade() {
                        state.expiry.borrow_mut().take();
                        Self(state).sync();
                    }
                });
            *self.0.expiry.borrow_mut() = Some(source);
        }
    }
}

impl Drop for ActivityState {
    fn drop(&mut self) {
        if let Some(source) = self.expiry.get_mut().take() {
            source.remove();
        }
        self.spinner.stop();
    }
}

fn remaining_activity(
    enabled: bool,
    last_output: Option<Instant>,
    timeout: Duration,
    now: Instant,
) -> Option<Duration> {
    if !enabled {
        return None;
    }
    last_output
        .and_then(|at| timeout.checked_sub(now.saturating_duration_since(at)))
        .filter(|remaining| !remaining.is_zero())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_output_expires_and_new_output_extends_it() {
        let start = Instant::now();
        let timeout = Duration::from_secs(5);
        assert_eq!(remaining_activity(true, None, timeout, start), None);
        assert_eq!(remaining_activity(false, Some(start), timeout, start), None);
        assert_eq!(
            remaining_activity(true, Some(start), timeout, start + Duration::from_secs(4)),
            Some(Duration::from_secs(1))
        );
        assert_eq!(
            remaining_activity(true, Some(start), timeout, start + timeout),
            None
        );
        assert_eq!(
            remaining_activity(
                true,
                Some(start + Duration::from_secs(4)),
                timeout,
                start + timeout
            ),
            Some(Duration::from_secs(4))
        );
    }
}
