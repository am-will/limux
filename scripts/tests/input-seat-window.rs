use gtk4::glib::translate::{IntoGlib, ToGlibPtr};
use gtk4::{gdk, glib, prelude::*};
use std::ffi::{c_int, c_ulong, c_void};
use std::time::Duration;

#[path = "../../rust/limux-host-linux/src/input_seats.rs"]
mod input_seats;
#[path = "../../rust/limux-host-linux/src/x11_cursor_errors.rs"]
mod x11_cursor_errors;

#[link(name = "gtk-4")]
extern "C" {
    fn gdk_x11_surface_get_xid(surface: *mut gdk::ffi::GdkSurface) -> c_ulong;
    fn gdk_x11_display_get_xdisplay(display: *mut gdk::ffi::GdkDisplay) -> *mut c_void;
    fn gdk_x11_display_error_trap_push(display: *mut gdk::ffi::GdkDisplay);
    fn gdk_x11_display_error_trap_pop(display: *mut gdk::ffi::GdkDisplay) -> c_int;
}

#[link(name = "Xi")]
extern "C" {
    fn XIDefineCursor(
        display: *mut c_void,
        device: c_int,
        window: c_ulong,
        cursor: c_ulong,
    ) -> c_int;
    fn XIQueryDevice(display: *mut c_void, device: c_int, count: *mut c_int) -> *mut c_void;
    fn XIFreeDeviceInfo(info: *mut c_void);
}

fn check_error_traps(display: &gdk::Display, window: c_ulong) {
    // This newly created Xvfb has no device with ID 65535. Cursor updates to a
    // removed device are harmless; other XI_BadDevice requests must still
    // reach GTK's normal error trap.
    let (cursor_error, query_error) = unsafe {
        let raw = display.to_glib_none().0;
        let xdisplay = gdk_x11_display_get_xdisplay(raw);
        gdk_x11_display_error_trap_push(raw);
        XIDefineCursor(xdisplay, 65535, window, 0);
        let cursor_error = gdk_x11_display_error_trap_pop(raw);

        gdk_x11_display_error_trap_push(raw);
        let mut count = 0;
        let info = XIQueryDevice(xdisplay, 65535, &mut count);
        if !info.is_null() {
            XIFreeDeviceInfo(info);
        }
        let query_error = gdk_x11_display_error_trap_pop(raw);
        (cursor_error, query_error)
    };
    assert_eq!(cursor_error, 0, "cursor-removal errors should be filtered");
    assert_ne!(query_error, 0, "other device errors must reach GTK's trap");
}

fn main() {
    assert_eq!(std::env::var("LIMUX_INPUT_SEAT_TEST").as_deref(), Ok("1"));
    let display_name = std::env::var("DISPLAY").unwrap();
    // The harness owns the Xvfb that returned this number through displayfd.
    // A headless CI runner can legitimately allocate its private display at 0.
    display_name
        .strip_prefix(':')
        .unwrap()
        .parse::<u32>()
        .unwrap();
    gtk4::init().unwrap();
    let display = gdk::Display::default().unwrap();
    let bridge = input_seats::InputSeatRemovalBridge::install(&display)
        .expect("expected an X11 removal bridge");
    let window = gtk4::Window::builder()
        .title("Limux isolated input-seat regression")
        .default_width(640)
        .default_height(400)
        .build();
    let child = gtk4::DrawingArea::builder()
        .hexpand(true)
        .vexpand(true)
        .focusable(true)
        .build();
    window.set_child(Some(&child));
    let keys = gtk4::EventControllerKey::new();
    keys.connect_key_pressed(|_, value, _, _| {
        println!("{{\"event\":\"key\",\"value\":{}}}", value.into_glib());
        glib::Propagation::Proceed
    });
    window.add_controller(keys);
    let clicks = gtk4::GestureClick::new();
    clicks.connect_pressed(|_, _, _, _| println!("{{\"event\":\"click\"}}"));
    child.add_controller(clicks);
    window.present();
    child.grab_focus();
    let ready_window = window.clone();
    glib::timeout_add_local_once(Duration::from_millis(100), move || {
        let surface = ready_window.surface().unwrap();
        let id = unsafe { gdk_x11_surface_get_xid(surface.to_glib_none().0) };
        check_error_traps(&WidgetExt::display(&ready_window), id);
        println!("{{\"ready\":{id}}}");
    });
    let event_loop = glib::MainLoop::new(None, false);
    let cursor_loop = event_loop.clone();
    let mut count = 0;
    glib::timeout_add_local(Duration::from_millis(20), move || {
        count += 1;
        child.set_cursor_from_name(Some(if count % 2 == 0 { "pointer" } else { "text" }));
        if count == 500 {
            println!("{{\"event\":\"cursor-updates\",\"count\":{count}}}");
            cursor_loop.quit();
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
    event_loop.run();
    window.destroy();
    drop(window);
    drop(bridge);
    // A normal drop must restore the hook and allow reinstallation. An
    // explicitly closed GDK display must never be passed back into Xlib.
    let replacement = input_seats::InputSeatRemovalBridge::install(&display)
        .expect("expected bridge reinstallation after clean shutdown");
    drop(replacement);
    let auxiliary =
        gdk::Display::open(Some(&display_name)).expect("expected another private connection");
    let closed_bridge = input_seats::InputSeatRemovalBridge::install(&auxiliary)
        .expect("expected bridge on the auxiliary display");
    auxiliary.close();
    assert!(auxiliary.is_closed());
    drop(closed_bridge);
    assert!(input_seats::InputSeatRemovalBridge::install(&auxiliary).is_none());
    assert!(x11_cursor_errors::XiCursorErrorFilter::install(&auxiliary).is_none());
}
