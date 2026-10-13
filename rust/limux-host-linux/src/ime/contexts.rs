//! GTK + ghostty wiring for the per-pane IME. Everything in this
//! module touches either a `gtk::IMContext` or the libghostty C API,
//! so it can only be exercised with a real GTK runtime — see the
//! pure-logic tests in [`super::state`] and [`super::routing`] for
//! everything that can be unit-tested in isolation.

use gtk::prelude::*;
use gtk4 as gtk;

use std::cell::{Cell, RefCell};
use std::ffi::CString;
use std::ptr;
use std::rc::Rc;

use limux_ghostty_sys::*;

use super::routing::{
    decide_routing, is_compose_initiator, update_latch_after_fallback_first, ComposeRouting,
};
use super::state::{ImeCommitOutcome, ImeFilterOutcome, TerminalImeState};

/// Per-pane IME contexts and state. Returned by [`create_pane_ime`]
/// and threaded through [`filter_key_event`] / [`reset_after_consumed_compose`].
pub struct PaneIme {
    pub primary: gtk::IMMulticontext,
    pub fallback: gtk::IMContextSimple,
    pub state: Rc<RefCell<TerminalImeState>>,
    /// True while [`Self::fallback`] holds a partial compose
    /// sequence — either the initiator (a dead key or Multi_key) has
    /// been consumed by it but the sequence has not yet completed,
    /// or it is in a longer in-flight compose. While set, subsequent
    /// keypresses are routed through [`Self::fallback`] first so the
    /// compose can finish without [`Self::primary`] (the Wayland
    /// slave) intercepting and swallowing the follow-up.
    fallback_composing: Rc<Cell<bool>>,
}

/// Build the two IM contexts for a terminal pane and wire their
/// preedit / commit signals into a shared [`TerminalImeState`].
///
/// The caller is responsible for routing keypresses through both
/// contexts (see [`filter_key_event`]) and calling `focus_in` /
/// `focus_out` / `set_client_widget(None)` on both at the right
/// lifecycle points.
pub fn create_pane_ime(
    gl_area: &gtk::GLArea,
    surface_cell: &Rc<RefCell<Option<ghostty_surface_t>>>,
) -> PaneIme {
    let primary = gtk::IMMulticontext::new();
    primary.set_client_widget(Some(gl_area));
    primary.set_use_preedit(true);

    let fallback = gtk::IMContextSimple::new();
    fallback.set_client_widget(Some(gl_area));
    fallback.set_use_preedit(true);

    let state = Rc::new(RefCell::new(TerminalImeState::default()));
    let fallback_composing = Rc::new(Cell::new(false));

    register_im_signal_handlers(primary.upcast_ref(), surface_cell, &state);
    register_im_signal_handlers(fallback.upcast_ref(), surface_cell, &state);

    // The fallback compose sequence is done when it either commits a
    // result or ends preedit without a commit, so clear the latch and
    // return the next key event to the multicontext-first path.
    {
        let fallback_composing = fallback_composing.clone();
        fallback.connect_commit(move |_, _| {
            fallback_composing.set(false);
        });
    }
    {
        let fallback_composing = fallback_composing.clone();
        fallback.connect_preedit_end(move |_| {
            fallback_composing.set(false);
        });
    }

    PaneIme {
        primary,
        fallback,
        state,
        fallback_composing,
    }
}

/// Run a single keypress through the IM contexts and return the
/// resulting filter outcome.
///
/// The routing decision is made by [`decide_routing`]; see its
/// documentation for the rules.
///
/// The caller must already have called
/// [`TerminalImeState::begin_key_event`] for this key event, and must
/// call [`TerminalImeState::finish_key_event`] after acting on the
/// returned outcome.
pub fn filter_key_event(
    surface: ghostty_surface_t,
    ime: &PaneIme,
    event: &gtk::gdk::KeyEvent,
) -> ImeFilterOutcome {
    update_ime_cursor_location(surface, ime.primary.upcast_ref());
    update_ime_cursor_location(surface, ime.fallback.upcast_ref());

    let in_compose = ime.fallback_composing.get();
    let is_initiator = is_compose_initiator(event.keyval());

    let im_handled = match decide_routing(in_compose, is_initiator) {
        ComposeRouting::FallbackFirst => {
            let handled = ime.fallback.filter_keypress(event);
            if let Some(new_latch) = update_latch_after_fallback_first(is_initiator, handled) {
                ime.fallback_composing.set(new_latch);
            }
            handled || ime.primary.filter_keypress(event)
        }
        ComposeRouting::PrimaryFirst => {
            ime.primary.filter_keypress(event) || ime.fallback.filter_keypress(event)
        }
    };

    ime.state.borrow().filter_outcome(im_handled)
}

/// Reset both IM contexts and clear any preedit visible in ghostty.
/// Use after ghostty consumes a key while a composition is in flight.
pub fn reset_after_consumed_compose(surface: ghostty_surface_t, ime: &PaneIme) {
    ime.primary.reset();
    ime.fallback.reset();
    ime.fallback_composing.set(false);
    clear_ghostty_preedit(surface);
}

pub fn clear_ghostty_preedit(surface: ghostty_surface_t) {
    unsafe { ghostty_surface_preedit(surface, ptr::null(), 0) };
}

pub fn update_ime_cursor_location(surface: ghostty_surface_t, im_context: &gtk::IMContext) {
    let mut x = 0.0;
    let mut y = 0.0;
    let mut width = 1.0;
    let mut height = 1.0;
    unsafe {
        ghostty_surface_ime_point(surface, &mut x, &mut y, &mut width, &mut height);
    }
    im_context.set_cursor_location(&gtk::gdk::Rectangle::new(
        x.round() as i32,
        y.round() as i32,
        width.max(1.0).round() as i32,
        height.max(1.0).round() as i32,
    ));
}

pub fn send_committed_text(surface: ghostty_surface_t, text: &str) {
    let Ok(c_text) = CString::new(text) else {
        return;
    };

    let event = ghostty_input_key_s {
        action: GHOSTTY_ACTION_PRESS,
        mods: GHOSTTY_MODS_NONE,
        consumed_mods: GHOSTTY_MODS_NONE,
        keycode: 0,
        text: c_text.as_ptr(),
        unshifted_codepoint: 0,
        composing: false,
    };

    unsafe {
        ghostty_surface_key(surface, event);
    }
}

fn update_ghostty_preedit(
    surface_cell: &Rc<RefCell<Option<ghostty_surface_t>>>,
    im_context: &gtk::IMContext,
) {
    let Some(surface) = *surface_cell.borrow() else {
        return;
    };

    let (preedit, _, cursor_pos) = im_context.preedit_string();
    if preedit.is_empty() {
        clear_ghostty_preedit(surface);
        return;
    }

    if let Ok(text) = CString::new(preedit.as_str()) {
        unsafe {
            ghostty_surface_preedit(surface, text.as_ptr(), cursor_pos.max(0) as usize);
        }
    }
}

fn register_im_signal_handlers(
    im_context: &gtk::IMContext,
    surface_cell: &Rc<RefCell<Option<ghostty_surface_t>>>,
    ime_state: &Rc<RefCell<TerminalImeState>>,
) {
    {
        let ime_state = ime_state.clone();
        im_context.connect_preedit_start(move |_| {
            ime_state.borrow_mut().preedit_started();
        });
    }
    {
        let surface_cell = surface_cell.clone();
        let ime_state = ime_state.clone();
        im_context.connect_preedit_changed(move |ctx| {
            ime_state.borrow_mut().preedit_changed();
            update_ghostty_preedit(&surface_cell, ctx);
        });
    }
    {
        let surface_cell = surface_cell.clone();
        let ime_state = ime_state.clone();
        im_context.connect_preedit_end(move |_| {
            ime_state.borrow_mut().preedit_ended();
            let Some(surface) = *surface_cell.borrow() else {
                return;
            };
            clear_ghostty_preedit(surface);
        });
    }
    {
        let surface_cell = surface_cell.clone();
        let ime_state = ime_state.clone();
        im_context.connect_commit(move |_, text| {
            let Some(surface) = *surface_cell.borrow() else {
                return;
            };

            match ime_state.borrow_mut().commit_text(text) {
                ImeCommitOutcome::BufferForKeyEvent => {}
                ImeCommitOutcome::CommitDirectly(text) => {
                    clear_ghostty_preedit(surface);
                    send_committed_text(surface, &text);
                }
            }
        });
    }
}
