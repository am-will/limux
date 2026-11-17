//! What happens to a terminal tab when its process ends on its own.
//!
//! A terminal closes when the user ends it in front of them, as in every
//! terminal. A process that ends while nobody is watching (a hidden project,
//! a background tab, an inactive window) keeps its tab, its output and its
//! project, with a banner to restart or close it. A process that fails right
//! after starting is held even in front of the user, so a mistyped command
//! does not flash and vanish.

use super::*;
use std::time::Duration;

/// Matches Ghostty's `abnormal-command-exit-runtime` default.
pub(super) const ABNORMAL_EXIT_RUNTIME: Duration = Duration::from_millis(250);

pub(super) const EXITED_TAB_CSS_CLASS: &str = "limux-tab-exited";
const EXIT_BANNER_CSS_CLASS: &str = "limux-exit-banner";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ExitDisposition {
    /// Remove the tab as before (and the project with its last terminal,
    /// unless the keep-open setting is on).
    Close,
    /// Keep the tab with its output and show the exit banner. `notify` marks
    /// the tab and project unread because the user did not see it happen.
    Hold { notify: bool },
}

/// `code` is `None` when the surface asked to close itself rather than its
/// child exiting; that is always a close. `attended` means the tab is the
/// active tab of a pane shown in the active project of the active window.
pub(super) fn exit_disposition(
    code: Option<u32>,
    attended: bool,
    runtime: Duration,
) -> ExitDisposition {
    let Some(code) = code else {
        return ExitDisposition::Close;
    };
    if !attended {
        return ExitDisposition::Hold { notify: true };
    }
    if code != 0 && runtime <= ABNORMAL_EXIT_RUNTIME {
        return ExitDisposition::Hold { notify: false };
    }
    ExitDisposition::Close
}

/// A non-zero code (signals arrive as one too). Only a failure that nobody
/// saw also raises a desktop notification; a clean exit stays in the sidebar.
pub(super) fn exit_failed(code: Option<u32>) -> bool {
    code.is_some_and(|code| code != 0)
}

pub(super) fn exit_message(code: u32) -> String {
    if code == 0 {
        "Process exited".to_string()
    } else {
        format!("Process exited with code {code}")
    }
}

const EXIT_RESTART_CSS_CLASS: &str = "limux-exit-restart";

/// The Restart button of a held tab's banner inside its content, so a tab
/// opened later puts the keyboard on an action instead of the dead terminal.
pub(super) fn restart_button_in(root: &gtk::Widget) -> Option<gtk::Button> {
    let mut child = root.first_child();
    while let Some(widget) = child {
        if widget.has_css_class(EXIT_RESTART_CSS_CLASS) {
            if let Ok(button) = widget.clone().downcast::<gtk::Button>() {
                return Some(button);
            }
        }
        if let Some(found) = restart_button_in(&widget) {
            return Some(found);
        }
        child = widget.next_sibling();
    }
    None
}

pub(super) struct ExitBanner {
    pub(super) root: gtk::Widget,
    pub(super) restart_button: gtk::Button,
}

pub(super) fn build_exit_banner(
    message: &str,
    on_restart: impl Fn() + 'static,
    on_close: impl Fn() + 'static,
) -> ExitBanner {
    let banner = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    banner.add_css_class(EXIT_BANNER_CSS_CLASS);
    banner.add_css_class("osd");
    banner.add_css_class("toolbar");
    banner.set_halign(gtk::Align::Center);
    banner.set_valign(gtk::Align::End);
    banner.set_margin_bottom(16);

    let label = gtk::Label::new(Some(message));
    label.set_margin_start(6);
    banner.append(&label);

    let restart_button = gtk::Button::with_label("Restart");
    restart_button.add_css_class(EXIT_RESTART_CSS_CLASS);
    restart_button.set_tooltip_text(Some("Start the same command again in this tab"));
    restart_button.connect_clicked(move |_| on_restart());
    banner.append(&restart_button);

    let close_button = gtk::Button::with_label("Close");
    close_button.set_tooltip_text(Some("Close this tab"));
    close_button.connect_clicked(move |_| on_close());
    banner.append(&close_button);

    ExitBanner {
        root: banner.upcast(),
        restart_button,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LONG: Duration = Duration::from_secs(5);
    const SHORT: Duration = Duration::from_millis(40);

    #[test]
    fn attended_exit_closes_like_any_terminal() {
        assert_eq!(
            exit_disposition(Some(0), true, LONG),
            ExitDisposition::Close
        );
        assert_eq!(
            exit_disposition(Some(1), true, LONG),
            ExitDisposition::Close
        );
        assert_eq!(
            exit_disposition(Some(0), true, SHORT),
            ExitDisposition::Close
        );
    }

    #[test]
    fn unattended_exit_is_held_and_notifies() {
        for code in [0, 1, 129] {
            for runtime in [SHORT, LONG] {
                assert_eq!(
                    exit_disposition(Some(code), false, runtime),
                    ExitDisposition::Hold { notify: true }
                );
            }
        }
    }

    #[test]
    fn attended_failure_right_after_start_is_held_quietly() {
        assert_eq!(
            exit_disposition(Some(127), true, SHORT),
            ExitDisposition::Hold { notify: false }
        );
        assert_eq!(
            exit_disposition(Some(127), true, ABNORMAL_EXIT_RUNTIME),
            ExitDisposition::Hold { notify: false }
        );
    }

    #[test]
    fn surface_close_request_always_closes() {
        assert_eq!(exit_disposition(None, false, LONG), ExitDisposition::Close);
        assert_eq!(exit_disposition(None, true, SHORT), ExitDisposition::Close);
    }

    #[test]
    fn only_failures_ask_for_a_desktop_notice() {
        assert!(!exit_failed(Some(0)));
        assert!(!exit_failed(None));
        assert!(exit_failed(Some(1)));
        assert!(exit_failed(Some(137)));
    }

    #[test]
    fn exit_message_names_non_zero_codes() {
        assert_eq!(exit_message(0), "Process exited");
        assert_eq!(exit_message(2), "Process exited with code 2");
    }
}
