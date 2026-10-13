use super::{
    activate_tab_in_pane, active_tab_in_pane, add_terminal_tab_to_pane, apply_tab_activity_config,
    close_tab_in_pane, create_pane, find_pane_internals, move_tab_to_pane, rename_tab_in_pane,
    tab_title, PaneCallbacks, TabKind,
};
use crate::app_config::AppConfig;
use crate::shortcut_config::{default_shortcuts, ResolvedShortcutConfig};
use crate::tab_activity::TabActivity;
use crate::terminal::TerminalHandle;
use gtk::prelude::*;
use gtk4 as gtk;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

fn pane_callbacks(
    config: Rc<RefCell<AppConfig>>,
    shortcuts: Rc<ResolvedShortcutConfig>,
) -> Rc<PaneCallbacks> {
    Rc::new(PaneCallbacks {
        workspace_id: "tab-activity-test".to_string(),
        autostart_command: Rc::default(),
        suppress_next_autostart: Cell::new(false),
        initial_command: RefCell::new(None),
        on_split: Box::new(|_, _| {}),
        on_close_pane: Box::new(|_| {}),
        on_bell: Box::new(|_, _, _| {}),
        on_desktop_notification: Box::new(|_, _, _, _, _| {}),
        on_open_browser_here: Box::new(|_| {}),
        on_open_url_in_browser: Box::new(|_, _| {}),
        on_open_keybinds: Box::new(|_| {}),
        current_shortcuts: Box::new(move || shortcuts.clone()),
        on_capture_shortcut: Rc::new(|_, _| Err(String::new())),
        on_pwd_changed: Box::new(|_| {}),
        on_empty: Box::new(|_, _| {}),
        on_state_changed: Box::new(|| {}),
        on_unread_changed: Box::new(|| {}),
        is_pane_visible: Box::new(|_| true),
        on_split_with_tab: Box::new(|_, _, _, _, _| {}),
        current_config: Box::new(move || config.clone()),
        workspace_for_pane: Box::new(|_| None),
    })
}

fn pump_for(context: &gtk::glib::MainContext, duration: Duration) {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        while context.pending() {
            context.iteration(false);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    while context.pending() {
        context.iteration(false);
    }
}

fn wait_for(
    context: &gtk::glib::MainContext,
    timeout: Duration,
    description: &str,
    mut condition: impl FnMut() -> bool,
) {
    let deadline = Instant::now() + timeout;
    loop {
        while context.pending() {
            context.iteration(false);
        }
        if condition() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {description}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn terminal_entry(
    pane: &gtk::Widget,
    tab_id: &str,
) -> (TabActivity, TerminalHandle, gtk::Widget, Option<String>) {
    let internals = find_pane_internals(pane).expect("registered pane");
    let tabs = internals.tab_state.borrow();
    let entry = tabs
        .tabs
        .iter()
        .find(|entry| entry.id == tab_id)
        .expect("terminal tab exists");
    let TabKind::Terminal { state } = &entry.kind else {
        panic!("tab is not a terminal");
    };
    (
        entry.activity.clone(),
        state.handle.clone(),
        entry.content.clone(),
        entry.automatic_title.clone(),
    )
}

fn add_shell(pane: &gtk::Box, callbacks: &PaneCallbacks, command: &str) -> String {
    *callbacks.initial_command.borrow_mut() = Some(command.to_string());
    add_terminal_tab_to_pane(pane.upcast_ref());
    active_tab_in_pane(pane.upcast_ref()).expect("new terminal tab becomes active")
}

fn send_command(handle: &TerminalHandle, command: &str) {
    assert!(handle.send_text(command), "terminal accepts command text");
    assert!(handle.send_key("Enter"), "terminal accepts Enter");
}

fn wait_for_marker(context: &gtk::glib::MainContext, handle: &TerminalHandle, marker: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        pump_for(context, Duration::from_millis(5));
        let text = handle.read_viewport_text().unwrap_or_default();
        if text.contains(marker) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for terminal output marker {marker}; viewport: {text:?}"
        );
    }
}

#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn real_pty_output_drives_tab_activity_lifecycle() {
    let xdg = tempfile::tempdir().expect("private XDG directory");
    for name in [
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
    ] {
        let path = xdg.path().join(name);
        std::fs::create_dir_all(&path).expect("create private XDG directory");
        std::env::set_var(name, path);
    }

    crate::prepare_ghostty_runtime();
    gtk::init().expect("GTK display required");
    crate::terminal::init_ghostty();

    let context = gtk::glib::MainContext::default();
    let config = Rc::new(RefCell::new(AppConfig::default()));
    let shortcuts = Rc::new(default_shortcuts());
    let source_callbacks = pane_callbacks(config.clone(), shortcuts.clone());
    let target_callbacks = pane_callbacks(config.clone(), shortcuts.clone());
    let source = create_pane(
        source_callbacks.clone(),
        shortcuts.clone(),
        None,
        None,
        true,
    );
    let target = create_pane(target_callbacks, shortcuts.clone(), None, None, true);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    row.append(&source);
    row.append(&target);
    let window = gtk::Window::builder()
        .default_width(1024)
        .default_height(600)
        .child(&row)
        .build();
    window.present();

    let first_id = add_shell(
        &source,
        &source_callbacks,
        r#"/bin/sh -c 'stty -echo; export PS1="" PS2=""; printf "\033[?12h"; i=0; while [ "$i" -lt 40 ]; do printf "HISTORY_%s\n" "$i"; i=$((i + 1)); done; printf "ACTIVITY_READY\n"; exec /bin/sh'"#,
    );
    let source_widget: gtk::Widget = source.clone().upcast();
    let (first_activity, first_handle, first_content, _) =
        terminal_entry(&source_widget, &first_id);
    wait_for(
        &context,
        Duration::from_secs(5),
        "realized first PTY",
        || first_handle.health().realized && source.is_mapped(),
    );
    wait_for_marker(&context, &first_handle, "ACTIVITY_READY");
    wait_for(
        &context,
        Duration::from_secs(2),
        "startup activity spinner",
        || first_activity.widget().is_visible(),
    );

    first_handle.focus_surface();
    first_handle.perform_binding_action("scroll_to_row:0");
    wait_for(
        &context,
        Duration::from_secs(7),
        "startup activity to expire without further PTY output",
        || !first_activity.widget().is_visible(),
    );
    first_handle.perform_binding_action("scroll_to_bottom");
    pump_for(&context, Duration::from_millis(150));
    assert!(
        !first_activity.widget().is_visible(),
        "scrolling without PTY output does not restart activity"
    );

    config.borrow_mut().interface.tab_activity_seconds = 1;
    apply_tab_activity_config(&config);
    send_command(
        &first_handle,
        r#"(while printf 'STREAM_OUTPUT\n'; do sleep 0.03; done) & activity_stream_pid=$!"#,
    );
    wait_for(
        &context,
        Duration::from_secs(2),
        "continuous output activity",
        || first_activity.widget().is_visible(),
    );
    pump_for(&context, Duration::from_millis(1100));
    assert!(
        first_activity.widget().is_visible(),
        "continuous output keeps activity visible beyond one expiry interval"
    );
    send_command(
        &first_handle,
        r#"kill "$activity_stream_pid"; wait "$activity_stream_pid" 2>/dev/null; printf 'STREAM_DONE\n'"#,
    );
    wait_for_marker(&context, &first_handle, "STREAM_DONE");
    wait_for(
        &context,
        Duration::from_secs(2),
        "continuous output activity to expire",
        || !first_activity.widget().is_visible(),
    );

    config.borrow_mut().interface.tab_activity_seconds = 5;
    apply_tab_activity_config(&config);
    wait_for(
        &context,
        Duration::from_secs(7),
        "previous output to expire before OSC-only output",
        || !first_activity.widget().is_visible(),
    );
    assert!(rename_tab_in_pane(&source_widget, &first_id, "Pinned name"));
    send_command(
        &first_handle,
        r#"i=0; while [ "$i" -lt 5 ]; do printf '\033]0;Animated Title %s\007' "$i"; i=$((i + 1)); sleep 0.03; done"#,
    );
    wait_for(
        &context,
        Duration::from_secs(3),
        "animated OSC title update",
        || terminal_entry(&source_widget, &first_id).3.as_deref() == Some("Animated Title 4"),
    );
    wait_for(
        &context,
        Duration::from_secs(2),
        "OSC-only title activity on a renamed tab",
        || first_activity.widget().is_visible(),
    );
    assert_eq!(
        tab_title(&source_widget, &first_id).as_deref(),
        Some("Pinned name"),
        "OSC title updates do not replace a renamed tab"
    );

    let second_id = add_shell(
        &source,
        &source_callbacks,
        r#"/bin/sh -c 'stty -echo; export PS1="" PS2=""; printf "SECOND_READY\n"; exec /bin/sh'"#,
    );
    let (second_activity, second_handle, _, _) = terminal_entry(&source_widget, &second_id);
    wait_for_marker(&context, &second_handle, "SECOND_READY");
    wait_for(
        &context,
        Duration::from_secs(2),
        "second terminal startup activity",
        || second_activity.widget().is_visible(),
    );
    wait_for(
        &context,
        Duration::from_secs(7),
        "both startup activities to expire",
        || !first_activity.widget().is_visible() && !second_activity.widget().is_visible(),
    );
    assert_eq!(
        active_tab_in_pane(&source_widget).as_deref(),
        Some(second_id.as_str())
    );

    send_command(&first_handle, "printf 'HIDDEN_TAB_OUTPUT\\n'");
    wait_for_marker(&context, &first_handle, "HIDDEN_TAB_OUTPUT");
    wait_for(
        &context,
        Duration::from_secs(2),
        "hidden tab activity",
        || first_activity.widget().is_visible(),
    );
    assert!(move_tab_to_pane(
        &source_widget,
        &first_id,
        target.upcast_ref()
    ));
    let target_widget: gtk::Widget = target.clone().upcast();
    let (moved_activity, moved_handle, _, _) = terminal_entry(&target_widget, &first_id);
    wait_for(
        &context,
        Duration::from_secs(3),
        "moved terminal content to reach the target stack",
        || {
            find_pane_internals(&target_widget)
                .and_then(|pane| pane.content_stack.first_child())
                .is_some_and(|child| child == first_content)
        },
    );
    assert!(
        moved_activity.widget().is_visible(),
        "activity follows the moved tab"
    );
    assert!(activate_tab_in_pane(&target_widget, &first_id));

    config.borrow_mut().interface.show_tab_activity = false;
    apply_tab_activity_config(&config);
    assert!(
        !moved_activity.widget().is_visible(),
        "disabling activity hides an active spinner immediately"
    );
    send_command(&moved_handle, "printf 'DISABLED_OUTPUT\\n'");
    wait_for_marker(&context, &moved_handle, "DISABLED_OUTPUT");
    pump_for(&context, Duration::from_millis(250));
    assert!(
        !moved_activity.widget().is_visible(),
        "output while disabled does not restart activity"
    );

    config.borrow_mut().interface.show_tab_activity = true;
    apply_tab_activity_config(&config);
    send_command(&moved_handle, "printf 'REENABLED_OUTPUT\\n'");
    wait_for_marker(&context, &moved_handle, "REENABLED_OUTPUT");
    wait_for(
        &context,
        Duration::from_secs(2),
        "reenabled output activity",
        || moved_activity.widget().is_visible(),
    );

    send_command(&moved_handle, "printf 'PENDING_CLOSE_OUTPUT\\n'; sleep 0.3");
    let close_marker_deadline = Instant::now() + Duration::from_secs(2);
    while !moved_handle
        .read_viewport_text()
        .is_some_and(|text| text.contains("PENDING_CLOSE_OUTPUT"))
        && Instant::now() < close_marker_deadline
    {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        moved_handle
            .read_viewport_text()
            .is_some_and(|text| text.contains("PENDING_CLOSE_OUTPUT")),
        "real PTY produced output before close"
    );
    std::thread::sleep(Duration::from_millis(150));
    assert!(close_tab_in_pane(&target_widget, &first_id));
    pump_for(&context, Duration::from_millis(250));
    assert!(tab_title(&target_widget, &first_id).is_none());
    assert!(
        !moved_handle.health().realized,
        "closed terminal surface is released"
    );

    window.close();
}
