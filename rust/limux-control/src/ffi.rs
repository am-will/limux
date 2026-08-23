use std::slice;
use std::str;
use std::sync::{Mutex, OnceLock};

use limux_protocol::{parse_v1_command_envelope, V2Request};
use tokio::runtime::{Builder, Runtime};

use crate::Dispatcher;

struct ControlSession {
    dispatcher: Dispatcher,
    runtime: Runtime,
}

static SESSION_CELL: OnceLock<Mutex<Option<ControlSession>>> = OnceLock::new();

fn session_slot() -> &'static Mutex<Option<ControlSession>> {
    SESSION_CELL.get_or_init(|| Mutex::new(None))
}

fn parse_request(input: &str) -> Result<V2Request, ()> {
    if let Ok(v2) = serde_json::from_str::<V2Request>(input) {
        return Ok(v2);
    }

    parse_v1_command_envelope(input)
        .map(|v1| v1.into_v2_request(None))
        .map_err(|_| ())
}

#[unsafe(no_mangle)]
pub extern "C" fn limux_control_init() -> i32 {
    let runtime = match Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(_) => return 1,
    };

    let mut session_guard = match session_slot().lock() {
        Ok(guard) => guard,
        Err(_) => return 1,
    };

    *session_guard = Some(ControlSession {
        dispatcher: Dispatcher::new(),
        runtime,
    });
    0
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `message_ptr` must point to a readable buffer of exactly `message_len` bytes
/// for the duration of this call.
pub unsafe extern "C" fn limux_control_dispatch(message_ptr: *const u8, message_len: usize) -> i32 {
    if message_ptr.is_null() {
        return 2;
    }

    let message = unsafe { slice::from_raw_parts(message_ptr, message_len) };
    let message = match str::from_utf8(message) {
        Ok(message) => message,
        Err(_) => return 2,
    };

    let request = match parse_request(message) {
        Ok(request) => request,
        Err(_) => return 2,
    };

    let mut session_guard = match session_slot().lock() {
        Ok(guard) => guard,
        Err(_) => return 1,
    };

    if session_guard.is_none() {
        let runtime = match Builder::new_multi_thread().enable_all().build() {
            Ok(runtime) => runtime,
            Err(_) => return 1,
        };
        *session_guard = Some(ControlSession {
            dispatcher: Dispatcher::new(),
            runtime,
        });
    }

    let session = session_guard.as_mut().expect("session initialized above");

    let response = session
        .runtime
        .block_on(session.dispatcher.dispatch(request));
    if response.error.is_some() {
        3
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn limux_control_shutdown() {
    if let Ok(mut session_guard) = session_slot().lock() {
        *session_guard = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Arc, Barrier};

    #[test]
    fn ffi_init_dispatch_shutdown_roundtrip() {
        assert_eq!(limux_control_init(), 0);

        let message = b"{\"id\":\"ffi-1\",\"method\":\"system.ping\",\"params\":{}}";
        assert_eq!(
            unsafe { limux_control_dispatch(message.as_ptr(), message.len()) },
            0
        );

        limux_control_shutdown();
    }

    #[test]
    fn ffi_dispatch_rejects_invalid_payload() {
        limux_control_shutdown();
        let bad = b"not-json";
        assert_eq!(
            unsafe { limux_control_dispatch(bad.as_ptr(), bad.len()) },
            2
        );
        limux_control_shutdown();
    }

    #[test]
    fn ffi_lifecycle_stays_consistent_under_concurrent_calls() {
        const THREADS: usize = 6;
        const ITERATIONS: usize = 18;
        const MESSAGE: &[u8] = br#"{"id":"ffi-concurrent","method":"system.ping","params":{}}"#;

        limux_control_shutdown();
        let start = Arc::new(Barrier::new(THREADS));

        std::thread::scope(|scope| {
            for worker in 0..THREADS {
                let start = Arc::clone(&start);
                scope.spawn(move || {
                    start.wait();
                    for iteration in 0..ITERATIONS {
                        match (worker + iteration) % 3 {
                            0 => assert_eq!(limux_control_init(), 0),
                            1 => assert_eq!(
                                unsafe { limux_control_dispatch(MESSAGE.as_ptr(), MESSAGE.len()) },
                                0
                            ),
                            _ => limux_control_shutdown(),
                        }
                    }
                });
            }
        });

        assert_eq!(
            unsafe { limux_control_dispatch(MESSAGE.as_ptr(), MESSAGE.len()) },
            0
        );
        limux_control_shutdown();
    }
}
