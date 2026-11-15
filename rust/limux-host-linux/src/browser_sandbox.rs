//! Check WebKit in a disposable process before it can abort the terminal host.

#[cfg(feature = "webkit")]
mod probe;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BrowserSandbox {
    pub(crate) sandboxed: bool,
}

#[cfg(feature = "webkit")]
pub(crate) fn availability() -> &'static Result<BrowserSandbox, String> {
    static RESULT: std::sync::OnceLock<Result<BrowserSandbox, String>> = std::sync::OnceLock::new();
    RESULT.get_or_init(probe::check)
}

#[cfg(not(feature = "webkit"))]
pub(crate) fn availability() -> &'static Result<BrowserSandbox, String> {
    static RESULT: std::sync::OnceLock<Result<BrowserSandbox, String>> = std::sync::OnceLock::new();
    RESULT.get_or_init(|| Err("This Limux build does not include WebKit.".to_string()))
}

pub(crate) fn unavailable_reason() -> Option<&'static str> {
    availability().as_ref().err().map(String::as_str)
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
