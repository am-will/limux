use std::slice;
use std::str;
use std::sync::{Mutex, OnceLock};

use cmux_protocol::{parse_v1_command_envelope, V2Request};
use tokio::runtime::{Builder, Runtime};

use crate::Dispatcher;

static DISPATCHER_CELL: OnceLock<Mutex<Option<Dispatcher>>> = OnceLock::new();
static RUNTIME_CELL: OnceLock<Mutex<Option<Runtime>>> = OnceLock::new();

fn dispatcher_slot() -> &'static Mutex<Option<Dispatcher>> {
    DISPATCHER_CELL.get_or_init(|| Mutex::new(None))
}

fn runtime_slot() -> &'static Mutex<Option<Runtime>> {
    RUNTIME_CELL.get_or_init(|| Mutex::new(None))
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
pub extern "C" fn cmux_control_init() -> i32 {
    let runtime = match Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(_) => return 1,
    };

    let dispatcher = Dispatcher::new();

    let mut runtime_guard = match runtime_slot().lock() {
        Ok(guard) => guard,
        Err(_) => return 1,
    };
    let mut dispatcher_guard = match dispatcher_slot().lock() {
        Ok(guard) => guard,
        Err(_) => return 1,
    };

    *runtime_guard = Some(runtime);
    *dispatcher_guard = Some(dispatcher);
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn cmux_control_dispatch(message_ptr: *const u8, message_len: usize) -> i32 {
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

    let mut runtime_guard = match runtime_slot().lock() {
        Ok(guard) => guard,
        Err(_) => return 1,
    };
    let mut dispatcher_guard = match dispatcher_slot().lock() {
        Ok(guard) => guard,
        Err(_) => return 1,
    };

    if runtime_guard.is_none() || dispatcher_guard.is_none() {
        *runtime_guard = Builder::new_multi_thread().enable_all().build().ok();
        *dispatcher_guard = Some(Dispatcher::new());
    }

    let runtime = match runtime_guard.as_mut() {
        Some(runtime) => runtime,
        None => return 1,
    };
    let dispatcher = match dispatcher_guard.as_ref() {
        Some(dispatcher) => dispatcher,
        None => return 1,
    };

    let response = runtime.block_on(dispatcher.dispatch(request));
    if response.error.is_some() {
        3
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cmux_control_shutdown() {
    if let Ok(mut dispatcher_guard) = dispatcher_slot().lock() {
        *dispatcher_guard = None;
    }
    if let Ok(mut runtime_guard) = runtime_slot().lock() {
        *runtime_guard = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffi_init_dispatch_shutdown_roundtrip() {
        assert_eq!(cmux_control_init(), 0);

        let message = b"{\"id\":\"ffi-1\",\"method\":\"system.ping\",\"params\":{}}";
        assert_eq!(cmux_control_dispatch(message.as_ptr(), message.len()), 0);

        cmux_control_shutdown();
    }

    #[test]
    fn ffi_dispatch_rejects_invalid_payload() {
        cmux_control_shutdown();
        let bad = b"not-json";
        assert_eq!(cmux_control_dispatch(bad.as_ptr(), bad.len()), 2);
        cmux_control_shutdown();
    }
}
