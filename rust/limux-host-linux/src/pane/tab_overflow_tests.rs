use super::*;
use crate::shortcut_config::default_shortcuts;
use std::time::{Duration, Instant};

fn callbacks(shortcuts: Rc<ResolvedShortcutConfig>) -> Rc<PaneCallbacks> {
    Rc::new(PaneCallbacks {
        workspace_id: "tab-overflow-test".to_string(),
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
        current_config: Box::new(|| Rc::new(RefCell::new(AppConfig::default()))),
        workspace_for_pane: Box::new(|_| None),
    })
}

fn wait_for(description: &str, mut ready: impl FnMut() -> bool) {
    let context = glib::MainContext::default();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        while context.pending() {
            context.iteration(false);
        }
        if ready() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {description}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn button(internals: &PaneInternals, id: &str) -> gtk::Box {
    internals
        .tab_state
        .borrow()
        .tabs
        .iter()
        .find(|entry| entry.id == id)
        .expect("tab exists")
        .tab_button
        .clone()
}

fn fully_visible(button: &gtk::Box, viewport: &gtk::Widget) -> bool {
    button.compute_bounds(viewport).is_some_and(|bounds| {
        bounds.width() > 0.0
            && bounds.x() >= -1.0
            && bounds.x() + bounds.width() <= viewport.width() as f32 + 1.0
    })
}

#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn overflowing_tabs_bound_terminal_geometry_and_reveal_selection() {
    crate::prepare_ghostty_runtime();
    gtk::init().expect("GTK display required");
    terminal::init_ghostty();
    for direction in [gtk::TextDirection::Ltr, gtk::TextDirection::Rtl] {
        gtk::Widget::set_default_direction(direction);
        let shortcuts = Rc::new(default_shortcuts());
        let pane = create_pane(callbacks(shortcuts.clone()), shortcuts, None, None, true);
        let internals = find_pane_internals(pane.upcast_ref()).expect("registered pane");

        // Exercise the real header widgets without starting a hundred PTYs.
        for index in 0..99 {
            let id = format!("overflow-{index}");
            let (tab_button, title_label, unread_dot, activity) =
                build_tab_button(&format!("Tab {index}"), &id, &internals);
            let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
            internals.content_stack.add_named(&content, Some(&id));
            internals.tab_strip.append(&tab_button);
            internals.tab_state.borrow_mut().tabs.push(TabEntry {
                id,
                tab_button,
                title_label,
                unread_dot,
                activity,
                content: content.upcast(),
                custom_name: None,
                automatic_title: None,
                pinned: false,
                unread: false,
                manual_unread: false,
                kind: TabKind::Keybinds,
            });
            activate_tab(
                &internals.tab_strip,
                &internals.content_stack,
                &internals.tab_state,
                &format!("overflow-{index}"),
            );
        }
        add_terminal_tab_to_pane(pane.upcast_ref());
        let terminal_id = active_tab_in_pane(pane.upcast_ref()).expect("active terminal");
        let content = internals
            .content_stack
            .visible_child()
            .expect("terminal content");
        let mut widgets = vec![content];
        let gl_area = std::iter::from_fn(|| {
            let widget = widgets.pop()?;
            let mut child = widget.first_child();
            while let Some(current) = child {
                child = current.next_sibling();
                widgets.push(current);
            }
            Some(widget)
        })
        .find_map(|widget| widget.downcast::<gtk::GLArea>().ok())
        .expect("terminal GLArea");
        let window = gtk::Window::builder()
            .default_width(800)
            .default_height(600)
            .child(&pane)
            .build();
        window.present();
        wait_for("allocated terminal", || {
            gl_area.is_mapped() && gl_area.width() > 0
        });

        assert!(
            window.width() <= 800,
            "100 tabs widened the window to {}",
            window.width()
        );
        assert!(
            pane.width() <= 800,
            "100 tabs widened the pane to {}",
            pane.width()
        );
        assert!(
            gl_area.width() <= 800,
            "100 tabs widened the terminal to {}",
            gl_area.width()
        );

        let viewport = internals.drop_indicator.parent().expect("tab viewport");
        let last = button(&internals, &terminal_id);
        wait_for("last selected tab in the viewport", || {
            fully_visible(&last, &viewport)
        });
        assert!(activate_tab_in_pane(pane.upcast_ref(), "overflow-0"));
        let first = button(&internals, "overflow-0");
        wait_for("first selected tab in the viewport", || {
            fully_visible(&first, &viewport)
        });
        assert!(activate_tab_in_pane(pane.upcast_ref(), &terminal_id));
        wait_for("last selected tab after switching back", || {
            fully_visible(&last, &viewport)
        });
        if direction == gtk::TextDirection::Ltr {
            let last_bounds = last.compute_bounds(&viewport).expect("last tab bounds");
            let drop_x = (last_bounds.x() + last_bounds.width() / 2.0 - 1.0) as f64;
            assert_eq!(
                insert_index_for_drop(&internals.tab_state, &viewport, drop_x, None),
                99,
                "drop coordinates must refer to the scrolled viewport"
            );
            position_indicator(&internals.tab_state, &internals.drop_indicator, drop_x);
            assert!(
                (internals.drop_indicator.margin_start() as f32 - last_bounds.x()).abs() <= 1.0,
                "the drop indicator must line up with the visible tab"
            );
            internals.drop_indicator.set_visible(false);
            assert!(reorder_tab_to_index(
                &internals.tab_strip,
                &internals.tab_state,
                &internals.callbacks,
                &terminal_id,
                0,
            ));
            wait_for("selected tab reordered to the start", || {
                fully_visible(&last, &viewport)
                    && internals.tab_state.borrow().tabs[0].id == terminal_id
                    && last.compute_bounds(&internals.tab_strip).unwrap().x() < 1.0
            });
            assert!(reorder_tab_to_index(
                &internals.tab_strip,
                &internals.tab_state,
                &internals.callbacks,
                &terminal_id,
                100,
            ));
            wait_for("selected tab reordered to the end", || {
                fully_visible(&last, &viewport)
                    && internals.tab_state.borrow().tabs[99].id == terminal_id
                    && last.compute_bounds(&internals.tab_strip).unwrap().x()
                        > viewport.width() as f32
            });
        }

        assert!(close_tab_in_pane(pane.upcast_ref(), &terminal_id));
        let replacement = button(&internals, "overflow-98");
        wait_for("replacement tab after closing", || {
            fully_visible(&replacement, &viewport)
        });
        window.close();
    }
    gtk::Widget::set_default_direction(gtk::TextDirection::Ltr);
}
