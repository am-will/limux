//! Per-pane IME state machine — pure logic, no GTK or ghostty FFI.
//!
//! Models the lifetime of a single key event from press through
//! filter / commit / forward, so the GTK keypress handler in
//! [`super::contexts`] can serialize signals coming from either of
//! the two IM contexts (multicontext and simple-compose fallback)
//! into one consistent stream for the ghostty surface.
//!
//! The four states (`Idle`, `NotComposing`, `Composing`) describe
//! where we are inside a single keypress, not the IM's compose
//! state. `composing` (the boolean field) tracks the latter and is
//! set by `preedit_started` / `preedit_changed` and cleared by
//! `preedit_ended`.

use std::ffi::CString;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ImeKeyEventPhase {
    #[default]
    Idle,
    NotComposing,
    Composing,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TerminalImeState {
    pub composing: bool,
    pub key_event_phase: ImeKeyEventPhase,
    pub pending_key_text: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImeCommitOutcome {
    BufferForKeyEvent,
    CommitDirectly(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImeFilterOutcome {
    ForwardToGhostty,
    ConsumeForIme,
}

impl TerminalImeState {
    pub fn begin_key_event(&mut self) {
        self.key_event_phase = if self.composing {
            ImeKeyEventPhase::Composing
        } else {
            ImeKeyEventPhase::NotComposing
        };
        self.pending_key_text = None;
    }

    pub fn finish_key_event(&mut self) {
        self.key_event_phase = ImeKeyEventPhase::Idle;
        self.pending_key_text = None;
    }

    pub fn preedit_started(&mut self) {
        self.composing = true;
    }

    pub fn preedit_changed(&mut self) {
        self.composing = true;
    }

    pub fn preedit_ended(&mut self) {
        self.composing = false;
    }

    pub fn commit_text(&mut self, text: &str) -> ImeCommitOutcome {
        match self.key_event_phase {
            ImeKeyEventPhase::Idle | ImeKeyEventPhase::Composing => {
                self.composing = false;
                ImeCommitOutcome::CommitDirectly(text.to_string())
            }
            ImeKeyEventPhase::NotComposing => {
                self.pending_key_text = Some(text.to_string());
                ImeCommitOutcome::BufferForKeyEvent
            }
        }
    }

    pub fn filter_outcome(&self, im_handled: bool) -> ImeFilterOutcome {
        if !im_handled {
            return ImeFilterOutcome::ForwardToGhostty;
        }

        if self.composing
            || self.key_event_phase == ImeKeyEventPhase::Composing
            || self.pending_key_text.is_none()
        {
            ImeFilterOutcome::ConsumeForIme
        } else {
            ImeFilterOutcome::ForwardToGhostty
        }
    }

    pub fn take_pending_key_text(&mut self) -> Option<CString> {
        self.pending_key_text
            .take()
            .and_then(|text| CString::new(text).ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ime_state_consumes_composing_key_events() {
        let mut state = TerminalImeState::default();
        state.preedit_started();
        state.begin_key_event();

        assert_eq!(state.filter_outcome(true), ImeFilterOutcome::ConsumeForIme);

        state.finish_key_event();
        assert_eq!(state.key_event_phase, ImeKeyEventPhase::Idle);
    }

    #[test]
    fn ime_state_treats_preedit_changed_as_composing() {
        let mut state = TerminalImeState::default();
        state.preedit_changed();

        assert!(state.composing);
    }

    #[test]
    fn ime_state_buffers_plain_commit_for_key_event_text() {
        let mut state = TerminalImeState::default();
        state.begin_key_event();

        assert_eq!(state.commit_text("a"), ImeCommitOutcome::BufferForKeyEvent);
        assert_eq!(
            state.filter_outcome(true),
            ImeFilterOutcome::ForwardToGhostty
        );

        let text = state
            .take_pending_key_text()
            .and_then(|text| text.into_string().ok());
        assert_eq!(text.as_deref(), Some("a"));
    }

    #[test]
    fn ime_state_commits_composed_text_outside_key_event() {
        let mut state = TerminalImeState::default();
        state.preedit_changed();

        assert_eq!(
            state.commit_text("á"),
            ImeCommitOutcome::CommitDirectly("á".to_string())
        );
        assert!(!state.composing);
    }

    /// Dead-key press path: the simple-compose fallback claims the
    /// event (`filter_keypress` → true) without ever emitting a
    /// preedit-changed signal, and no commit fires on this keystroke.
    /// We must still consume the event so the raw dead-key glyph
    /// (`^`, `¨`, `~`, …) never reaches ghostty.
    #[test]
    fn ime_state_consumes_dead_key_press_without_preedit() {
        let mut state = TerminalImeState::default();
        state.begin_key_event();

        assert_eq!(state.filter_outcome(true), ImeFilterOutcome::ConsumeForIme);
    }

    /// Dead-key follow-up path: on the second keystroke of a compose
    /// sequence (e.g. `e` after `^`), the simple-compose fallback
    /// synchronously emits the commit signal with the composed glyph
    /// before `filter_keypress` returns. We buffer that text, forward
    /// the key event to ghostty, and use the buffered glyph in place
    /// of the key's own text payload.
    #[test]
    fn ime_state_buffers_compose_commit_synchronously() {
        let mut state = TerminalImeState::default();
        state.begin_key_event();

        assert_eq!(state.commit_text("ê"), ImeCommitOutcome::BufferForKeyEvent);
        assert_eq!(
            state.filter_outcome(true),
            ImeFilterOutcome::ForwardToGhostty
        );

        let text = state
            .take_pending_key_text()
            .and_then(|text| text.into_string().ok());
        assert_eq!(text.as_deref(), Some("ê"));
    }

    /// Full state-machine trace for `^` then `e` → `ê`, the scenario
    /// the user originally reported. Mirrors what `filter_key_event`
    /// drives through `TerminalImeState` when the simple-compose
    /// fallback handles both keystrokes.
    #[test]
    fn state_machine_compose_trace_caret_e_to_e_circumflex() {
        let mut state = TerminalImeState::default();

        // ---- Keystroke 1: dead circumflex -----------------------
        // begin: not yet composing → phase NotComposing.
        state.begin_key_event();
        assert_eq!(state.key_event_phase, ImeKeyEventPhase::NotComposing);
        // Fallback claims, no commit signal fires on this keystroke.
        // filter_outcome with im_handled=true must consume so the raw
        // `^` glyph never reaches ghostty.
        assert_eq!(state.filter_outcome(true), ImeFilterOutcome::ConsumeForIme);
        state.finish_key_event();

        // ---- Keystroke 2: `e` ----------------------------------
        // Latch is still set externally; state machine doesn't know.
        // begin: composing is still false (no preedit was emitted by
        // the simple module), so phase is NotComposing.
        state.begin_key_event();
        assert_eq!(state.key_event_phase, ImeKeyEventPhase::NotComposing);
        // Compose completes synchronously inside filter_keypress —
        // the commit signal fires with "ê" while we're still
        // processing this key event, so commit_text buffers it.
        assert_eq!(state.commit_text("ê"), ImeCommitOutcome::BufferForKeyEvent);
        // filter_keypress returns true; we forward the key event
        // (with `text = "ê"`) to ghostty rather than consume it.
        assert_eq!(
            state.filter_outcome(true),
            ImeFilterOutcome::ForwardToGhostty
        );
        let composed = state
            .take_pending_key_text()
            .and_then(|text| text.into_string().ok());
        assert_eq!(composed.as_deref(), Some("ê"));
        state.finish_key_event();
    }
}
