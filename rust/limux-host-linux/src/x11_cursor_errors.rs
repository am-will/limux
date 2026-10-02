//! Narrowly consume XInput `XI_BadDevice` replies to `XIChangeCursor`.
//!
//! GTK 4.14 can enqueue a cursor update for a secondary master while the X
//! server is removing that device. Xlib's per-display wire converter runs
//! before GTK's global error handler, including when GTK has an error trap
//! active, so this filter can consume only that stale cursor request and leave
//! all other X errors on GTK's normal path.

use gtk4::gdk;
use gtk4::gdk::prelude::*;
use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_ulong, c_void};
use std::sync::{Mutex, OnceLock};

const XI_BAD_DEVICE: u8 = 0;
const XI_CHANGE_CURSOR: u8 = 42;
const X_ERROR: c_int = 0;

type WireToError =
    unsafe extern "C" fn(*mut c_void, *mut XErrorEvent, *mut XProtocolError) -> c_int;

/// Host-ABI layout of Xlib's public `XErrorEvent`.
#[repr(C)]
struct XErrorEvent {
    type_: c_int,
    display: *mut c_void,
    resourceid: c_ulong,
    serial: c_ulong,
    error_code: u8,
    request_code: u8,
    minor_code: u8,
}

/// Wire layout of the protocol `xError` record passed by Xlib's converter.
#[repr(C)]
struct XProtocolError {
    type_: u8,
    error_code: u8,
    sequence_number: u16,
    resource_id: u32,
    minor_code: u16,
    major_code: u8,
    pad1: u8,
    pad3: u32,
    pad4: u32,
    pad5: u32,
    pad6: u32,
    pad7: u32,
}

#[derive(Clone, Copy)]
struct FilterContext {
    bad_device_error: u8,
    xinput_opcode: u8,
    previous: Option<WireToError>,
}

static FILTERS: OnceLock<Mutex<HashMap<usize, FilterContext>>> = OnceLock::new();

fn filters() -> &'static Mutex<HashMap<usize, FilterContext>> {
    FILTERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn delegate(
    context: FilterContext,
    display: *mut c_void,
    event: *mut XErrorEvent,
    wire: *mut XProtocolError,
) -> c_int {
    context
        .previous
        .map(|previous| unsafe { previous(display, event, wire) })
        .unwrap_or(1)
}

fn matches_stale_cursor_error(
    display: *mut c_void,
    event: &XErrorEvent,
    context: FilterContext,
) -> bool {
    event.type_ == X_ERROR
        && event.display == display
        && event.error_code == context.bad_device_error
        && event.request_code == context.xinput_opcode
        && event.minor_code == XI_CHANGE_CURSOR
}

unsafe extern "C" fn filter_xinput_cursor_error(
    display: *mut c_void,
    event: *mut XErrorEvent,
    wire: *mut XProtocolError,
) -> c_int {
    let context = filters()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&(display as usize))
        .copied();
    let Some(context) = context else {
        return 1;
    };

    // Xlib supplies both structures for every error. If a caller violates
    // that contract, preserve the prior converter's behavior instead.
    let Some(event_ref) = (unsafe { event.as_ref() }) else {
        return delegate(context, display, event, wire);
    };
    if matches_stale_cursor_error(display, event_ref, context) {
        return 0;
    }

    delegate(context, display, event, wire)
}

/// Per-display Xlib hook guard. Keep it alive for the input-seat bridge's
/// lifetime so XInput cursor errors are filtered only while GTK can enqueue
/// those requests.
pub(crate) struct XiCursorErrorFilter {
    display: gdk::Display,
    xdisplay: *mut c_void,
    bad_device_error: u8,
    context: FilterContext,
}

impl XiCursorErrorFilter {
    pub(crate) fn install(display: &gdk::Display) -> Option<Self> {
        if display.is_closed() || display.type_().name() != "GdkX11Display" {
            return None;
        }

        let xdisplay = unsafe { gdk_x11_display_get_xdisplay(display.as_ptr()) };
        if xdisplay.is_null() {
            return None;
        }

        let (opcode, error_base) = {
            let mut opcode = 0;
            let mut first_event = 0;
            let mut error_base = 0;
            unsafe {
                XLockDisplay(xdisplay);
                let available = XQueryExtension(
                    xdisplay,
                    c"XInputExtension".as_ptr(),
                    &mut opcode,
                    &mut first_event,
                    &mut error_base,
                );
                XUnlockDisplay(xdisplay);
                if available == 0 {
                    return None;
                }
            }
            (u8::try_from(opcode).ok()?, error_base)
        };
        let bad_device_error = u8::try_from(error_base + i32::from(XI_BAD_DEVICE)).ok()?;

        // Xlib serializes this per-display mutation with pending error
        // conversion. Publish the context before installing the callback, and
        // release the Rust lock before each Xlib call.
        let previous = unsafe {
            XLockDisplay(xdisplay);
            {
                let mut registry = filters()
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if registry.contains_key(&(xdisplay as usize)) {
                    drop(registry);
                    XUnlockDisplay(xdisplay);
                    return None;
                }
                registry.insert(
                    xdisplay as usize,
                    FilterContext {
                        bad_device_error,
                        xinput_opcode: opcode,
                        previous: None,
                    },
                );
            }

            let previous = XESetWireToError(
                xdisplay,
                c_int::from(bad_device_error),
                Some(filter_xinput_cursor_error),
            );
            if let Some(context) = filters()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get_mut(&(xdisplay as usize))
            {
                context.previous = previous;
            }
            XUnlockDisplay(xdisplay);
            previous
        };

        Some(Self {
            display: display.clone(),
            xdisplay,
            bad_device_error,
            context: FilterContext {
                bad_device_error,
                xinput_opcode: opcode,
                previous,
            },
        })
    }
}

impl Drop for XiCursorErrorFilter {
    fn drop(&mut self) {
        // Restore only when our hook is still installed. If another Xlib
        // extension wrapped it after us, put that wrapper back and retain our
        // registry entry so a chained call can still reach its prior handler.
        let display_key = self.xdisplay as usize;
        if self.display.is_closed() {
            filters()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&display_key);
            return;
        }

        self.display.sync();
        if self.display.is_closed() {
            filters()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&display_key);
            return;
        }

        unsafe {
            XLockDisplay(self.xdisplay);
            let current = XESetWireToError(
                self.xdisplay,
                c_int::from(self.bad_device_error),
                self.context.previous,
            );
            if current.is_some_and(|handler| {
                std::ptr::fn_addr_eq(handler, filter_xinput_cursor_error as WireToError)
            }) {
                filters()
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&display_key);
            } else {
                XESetWireToError(self.xdisplay, c_int::from(self.bad_device_error), current);
            }
            XUnlockDisplay(self.xdisplay);
        }
    }
}

#[link(name = "gtk-4")]
unsafe extern "C" {
    fn gdk_x11_display_get_xdisplay(display: *mut gdk::ffi::GdkDisplay) -> *mut c_void;
}

#[link(name = "X11")]
unsafe extern "C" {
    fn XQueryExtension(
        display: *mut c_void,
        name: *const c_char,
        major_opcode_return: *mut c_int,
        first_event_return: *mut c_int,
        first_error_return: *mut c_int,
    ) -> c_int;
    fn XESetWireToError(
        display: *mut c_void,
        error_number: c_int,
        converter: Option<WireToError>,
    ) -> Option<WireToError>;
    fn XLockDisplay(display: *mut c_void);
    fn XUnlockDisplay(display: *mut c_void);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static PREVIOUS_CALLS: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn previous_returns_false(
        _display: *mut c_void,
        _event: *mut XErrorEvent,
        _wire: *mut XProtocolError,
    ) -> c_int {
        PREVIOUS_CALLS.fetch_add(1, Ordering::Relaxed);
        0
    }

    fn test_context() -> FilterContext {
        FilterContext {
            bad_device_error: 137,
            xinput_opcode: 131,
            previous: Some(previous_returns_false),
        }
    }

    #[test]
    fn callback_filters_only_xinput_bad_device_change_cursor_errors() {
        let display = 0x5100usize as *mut c_void;
        filters()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(display as usize, test_context());
        PREVIOUS_CALLS.store(0, Ordering::Relaxed);

        let mut event = XErrorEvent {
            type_: X_ERROR,
            display,
            resourceid: 0,
            serial: 0,
            error_code: 137,
            request_code: 131,
            minor_code: XI_CHANGE_CURSOR,
        };
        let mut wire = XProtocolError {
            type_: 0,
            error_code: 0,
            sequence_number: 0,
            resource_id: 0,
            minor_code: 0,
            major_code: 0,
            pad1: 0,
            pad3: 0,
            pad4: 0,
            pad5: 0,
            pad6: 0,
            pad7: 0,
        };

        let filtered = unsafe { filter_xinput_cursor_error(display, &mut event, &mut wire) };
        assert_eq!(filtered, 0);
        assert_eq!(PREVIOUS_CALLS.load(Ordering::Relaxed), 0);

        let nonmatching_events = [
            XErrorEvent {
                error_code: 138,
                ..copy_error(&event)
            },
            XErrorEvent {
                request_code: 130,
                ..copy_error(&event)
            },
            XErrorEvent {
                minor_code: 41,
                ..copy_error(&event)
            },
            XErrorEvent {
                display: 0x5200usize as *mut c_void,
                ..copy_error(&event)
            },
            XErrorEvent {
                type_: 1,
                ..copy_error(&event)
            },
        ];
        for mut mismatch in nonmatching_events {
            let delegated =
                unsafe { filter_xinput_cursor_error(display, &mut mismatch, &mut wire) };
            assert_eq!(
                delegated, 0,
                "the previous converter's false result is preserved"
            );
        }
        assert_eq!(PREVIOUS_CALLS.load(Ordering::Relaxed), 5);

        // Another display cannot inherit this display's filter.
        let other_display = 0x5300usize as *mut c_void;
        let other_result =
            unsafe { filter_xinput_cursor_error(other_display, &mut event, &mut wire) };
        assert_eq!(other_result, 1);

        filters()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&(display as usize));
    }

    fn copy_error(event: &XErrorEvent) -> XErrorEvent {
        XErrorEvent {
            type_: event.type_,
            display: event.display,
            resourceid: event.resourceid,
            serial: event.serial,
            error_code: event.error_code,
            request_code: event.request_code,
            minor_code: event.minor_code,
        }
    }
}
