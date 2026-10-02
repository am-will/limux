//! Keep GTK windows' borrowed pointer-focus device valid while X11 removes a
//! secondary XInput seat. GTK 4.14 listens for removals on the default seat,
//! even when the removed device belongs to another seat.

use crate::x11_cursor_errors::XiCursorErrorFilter;
use gtk4::gdk;
use gtk4::gdk::prelude::*;
use gtk4::glib;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

struct SecondarySeat {
    devices: HashMap<usize, gdk::Device>,
}

struct SignalHandler {
    owner: glib::Object,
    id: glib::SignalHandlerId,
}

struct BridgeState {
    default_seat: gdk::Seat,
    secondary_seats: HashMap<usize, SecondarySeat>,
    handlers: Vec<SignalHandler>,
}

/// Lifetime guard for the X11 secondary-seat removal compatibility bridge.
///
/// Keep this alive for the GTK application lifetime. Signal closures hold only
/// weak state references, so they cannot form a cycle with the display.
pub(crate) struct InputSeatRemovalBridge {
    state: Rc<RefCell<BridgeState>>,
    _cursor_error_filter: Option<XiCursorErrorFilter>,
}

impl InputSeatRemovalBridge {
    /// Installs the GTK 4.14 X11 seat-removal workaround for `display`.
    /// Wayland and other display backends are left untouched.
    pub(crate) fn install(display: &gdk::Display) -> Option<Self> {
        if display.is_closed() || display.type_().name() != "GdkX11Display" {
            return None;
        }

        let cursor_error_filter = XiCursorErrorFilter::install(display);
        let default_seat = display.default_seat()?;
        let state = Rc::new(RefCell::new(BridgeState {
            default_seat,
            secondary_seats: HashMap::new(),
            handlers: Vec::new(),
        }));

        let weak = Rc::downgrade(&state);
        let id = display.connect_seat_added(move |_display, seat| {
            if let Some(state) = weak.upgrade() {
                watch_secondary_seat(&state, seat);
            }
        });
        state.borrow_mut().handlers.push(SignalHandler {
            owner: display.clone().upcast(),
            id,
        });

        let weak = Rc::downgrade(&state);
        let id = display.connect_seat_removed(move |_display, seat| {
            if let Some(state) = weak.upgrade() {
                finish_secondary_seat_removal(&state, seat);
            }
        });
        state.borrow_mut().handlers.push(SignalHandler {
            owner: display.clone().upcast(),
            id,
        });

        for seat in display.list_seats() {
            watch_secondary_seat(&state, &seat);
        }

        Some(Self {
            state,
            _cursor_error_filter: cursor_error_filter,
        })
    }
}

impl Drop for InputSeatRemovalBridge {
    fn drop(&mut self) {
        let handlers = {
            let mut state = self.state.borrow_mut();
            state.secondary_seats.clear();
            std::mem::take(&mut state.handlers)
        };
        disconnect_handlers(handlers);
    }
}

fn watch_secondary_seat(state: &Rc<RefCell<BridgeState>>, seat: &gdk::Seat) {
    let seat_key = seat.as_ptr() as usize;
    let (already_watched, default_seat_key) = {
        let state = state.borrow();
        (
            state.secondary_seats.contains_key(&seat_key),
            state.default_seat.as_ptr() as usize,
        )
    };
    if already_watched || seat_key == default_seat_key {
        return;
    }

    let mut devices = HashMap::new();
    if let Some(device) = seat.pointer() {
        devices.insert(device.as_ptr() as usize, device);
    }
    if let Some(device) = seat.keyboard() {
        devices.insert(device.as_ptr() as usize, device);
    }
    for device in seat.devices(gdk::SeatCapabilities::ALL) {
        devices.insert(device.as_ptr() as usize, device);
    }

    state
        .borrow_mut()
        .secondary_seats
        .insert(seat_key, SecondarySeat { devices });

    let weak = Rc::downgrade(state);
    let id = seat.connect_device_added(move |_seat, device| {
        if let Some(state) = weak.upgrade() {
            remember_secondary_device(&state, seat_key, device);
        }
    });
    state.borrow_mut().handlers.push(SignalHandler {
        owner: seat.clone().upcast(),
        id,
    });

    let weak = Rc::downgrade(state);
    let id = seat.connect_device_removed(move |_seat, device| {
        if let Some(state) = weak.upgrade() {
            forward_secondary_device_removal(&state, seat_key, device);
        }
    });
    state.borrow_mut().handlers.push(SignalHandler {
        owner: seat.clone().upcast(),
        id,
    });
}

fn remember_secondary_device(
    state: &Rc<RefCell<BridgeState>>,
    seat_key: usize,
    device: &gdk::Device,
) {
    let device_key = device.as_ptr() as usize;
    if let Some(seat) = state.borrow_mut().secondary_seats.get_mut(&seat_key) {
        seat.devices.insert(device_key, device.clone());
    }
}

fn forward_secondary_device_removal(
    state: &Rc<RefCell<BridgeState>>,
    seat_key: usize,
    device: &gdk::Device,
) {
    let device_key = device.as_ptr() as usize;
    let (default_seat, retained_device) = {
        let mut state = state.borrow_mut();
        let default_seat = state.default_seat.clone();
        let Some(seat) = state.secondary_seats.get_mut(&seat_key) else {
            return;
        };
        let Some(retained_device) = seat.devices.remove(&device_key) else {
            return;
        };
        (default_seat, retained_device)
    };

    // GTK 4.14's GtkWindow focus cleanup only listens to this public signal on
    // the default seat. Keep the cached device alive through the emission.
    default_seat.emit_by_name::<()>("device-removed", &[&retained_device]);
}

fn finish_secondary_seat_removal(state: &Rc<RefCell<BridgeState>>, seat: &gdk::Seat) {
    let seat_key = seat.as_ptr() as usize;
    let (default_seat, devices, stale_handlers) = {
        let mut state = state.borrow_mut();
        if state.default_seat.as_ptr() as usize == seat_key {
            return;
        }

        let Some(removed) = state.secondary_seats.remove(&seat_key) else {
            return;
        };
        let devices = removed.devices.into_values().collect::<Vec<_>>();

        let mut stale_handlers = Vec::new();
        let mut active_handlers = Vec::with_capacity(state.handlers.len());
        for handler in std::mem::take(&mut state.handlers) {
            if handler.owner.as_ptr() as usize == seat_key {
                stale_handlers.push(handler);
            } else {
                active_handlers.push(handler);
            }
        }
        state.handlers = active_handlers;

        (state.default_seat.clone(), devices, stale_handlers)
    };

    disconnect_handlers(stale_handlers);
    for device in devices {
        default_seat.emit_by_name::<()>("device-removed", &[&device]);
    }
}

fn disconnect_handlers(handlers: Vec<SignalHandler>) {
    for handler in handlers {
        let connected = unsafe {
            glib::gobject_ffi::g_signal_handler_is_connected(
                handler.owner.as_ptr(),
                handler.id.as_raw(),
            ) != 0
        };
        if connected {
            handler.owner.disconnect(handler.id);
        }
    }
}
