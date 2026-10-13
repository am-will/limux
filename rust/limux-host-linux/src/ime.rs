//! IME plumbing for the embedded ghostty terminal surface.
//!
//! Two GTK input-method contexts run side-by-side per pane:
//!
//! 1. `gtk::IMMulticontext` — the primary, which routes to whichever
//!    slave GTK picks (ibus / fcitx5 / wayland / simple). It handles
//!    full IMEs (CJK and friends).
//!
//! 2. `gtk::IMContextSimple` — a fallback that runs libxkbcommon's
//!    compose tables in-process. It only sees keypresses the
//!    multicontext didn't claim, and exists because the "wayland"
//!    slave (GTK's default on Wayland sessions without ibus/fcitx5)
//!    defers compose to the compositor, and KWin/Plasma 6 does not
//!    drive xkb_compose for AZERTY dead keys over text-input-v3.
//!
//! Both contexts feed the same `TerminalImeState` through the signal
//! handlers wired in [`contexts`]. Only one of the two is ever in a
//! composing state at a time in practice — the multicontext, when it
//! actively handles input; otherwise the fallback, when the
//! multicontext lets a dead key through.
//!
//! Module layout:
//!
//! * [`state`]    — pure state machine, no GTK or FFI; unit-tested.
//! * [`routing`]  — pure routing decisions and compose-initiator
//!   detection; unit-tested.
//! * [`contexts`] — GTK signal wiring and ghostty FFI bridge; only
//!   exercisable with a real GTK runtime.

mod contexts;
mod routing;
mod state;

pub use contexts::{create_pane_ime, filter_key_event, reset_after_consumed_compose, PaneIme};
pub use state::ImeFilterOutcome;
