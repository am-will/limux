use super::*;

// ---------------------------------------------------------------------------
// Context menu
// ---------------------------------------------------------------------------

/// Send a binding action to every live surface.
pub(crate) fn broadcast_binding_action(action: &str) {
    SURFACE_MAP.with(|map| {
        for &key in map.borrow().keys() {
            let surface = key as ghostty_surface_t;
            unsafe {
                ghostty_surface_binding_action(
                    surface,
                    action.as_ptr() as *const c_char,
                    action.len(),
                );
            }
        }
    });
}

pub(super) fn surface_action(surface: Option<ghostty_surface_t>, action: &str) {
    if let Some(surface) = surface {
        unsafe {
            ghostty_surface_binding_action(surface, action.as_ptr() as *const c_char, action.len());
        }
    }
}

pub(super) fn url_at_position(
    surface: Option<ghostty_surface_t>,
    x: f64,
    y: f64,
    restore_mods: c_int,
) -> Option<String> {
    let surface = surface?;
    // Mouse-position callbacks become terminal input while a TUI has mouse
    // reporting enabled, so probing there would alter terminal state.
    if unsafe { ghostty_surface_mouse_captured(surface) } {
        return None;
    }
    let clipboard_context = SURFACE_MAP.with(|map| {
        map.borrow()
            .get(&(surface as usize))
            .map(|entry| entry.clipboard_context)
    })?;
    let context = unsafe { clipboard_context.as_ref() }?;

    context.url_probe_active.set(true);

    // Let Ghostty resolve OSC 8 targets and wrapped URLs. Leave and re-enter
    // because embedded surfaces deduplicate same-position moves.
    *context.url_probe.borrow_mut() = None;
    unsafe {
        ghostty_surface_mouse_pos(surface, -1.0, -1.0, GHOSTTY_MODS_CTRL);
        ghostty_surface_mouse_pos(surface, x, y, GHOSTTY_MODS_CTRL);
    }
    surface_action(Some(surface), "copy_url_to_clipboard");
    let url = context.url_probe.borrow_mut().take();
    context.url_probe_active.set(false);

    unsafe {
        ghostty_surface_mouse_pos(surface, -1.0, -1.0, restore_mods);
        ghostty_surface_mouse_pos(surface, x, y, restore_mods);
    }
    SURFACE_MAP.with(|map| {
        if let Some(entry) = map.borrow().get(&(surface as usize)) {
            entry.link_popover.popdown();
        }
    });

    url
}

pub(super) fn copy_text_to_clipboards(text: &str) {
    if let Some(display) = gtk::gdk::Display::default() {
        display.clipboard().set_text(text);
        display.primary_clipboard().set_text(text);
    }
}

/// Build a libadwaita-styled floating popover (no arrow), parented to
/// `parent`. Shared by the right-click context menu and the OSC 8 hover
/// URL preview so they look identical by construction. The caller is
/// responsible for `set_pointing_to` and `popup`/`popdown`.
pub(super) fn build_floating_popover(
    parent: &impl IsA<gtk::Widget>,
    child: &impl IsA<gtk::Widget>,
) -> gtk::Popover {
    let popover = gtk::Popover::new();
    popover.set_child(Some(child));
    popover.set_has_arrow(false);
    popover.set_parent(parent);
    popover
}

pub(super) fn widget_has_native_surface(widget: &impl IsA<gtk::Widget>) -> bool {
    widget.is_mapped()
        && widget
            .native()
            .and_then(|native| native.surface())
            .is_some()
}

/// 4 px box wrapper that matches the inner margin used by the right-click
/// context menu items. Reused for the hover preview so both popovers have
/// the same visual breathing room around their content.
pub(super) fn build_popover_inner_box() -> gtk::Box {
    let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    menu_box.set_margin_top(4);
    menu_box.set_margin_bottom(4);
    menu_box.set_margin_start(4);
    menu_box.set_margin_end(4);
    menu_box
}

pub(super) fn build_submenu_button(label: &str, popover: &gtk::Popover) -> gtk::MenuButton {
    let button = gtk::MenuButton::new();
    button.set_label(label);
    button.set_direction(gtk::ArrowType::Right);
    button.set_has_frame(false);
    button.set_halign(gtk::Align::Fill);
    button.set_popover(Some(popover));
    button.add_css_class("flat");
    // Hover opens this submenu, so it must never take a GTK grab: an autohide
    // popover swallows the next click to dismiss itself, which turns every
    // item below the submenu into a two-click item. The parent menu closes it
    // on `closed`, and sibling items close it on hover.
    popover.set_autohide(false);

    let weak_button = button.downgrade();
    let motion = gtk::EventControllerMotion::new();
    motion.connect_enter(move |motion, _, _| {
        if motion
            .widget()
            .is_some_and(|widget| widget_has_native_surface(&widget))
        {
            if let Some(button) = weak_button.upgrade() {
                button.popup();
            }
        }
    });
    button.add_controller(motion);
    button
}

pub(super) fn show_terminal_context_menu(
    gl_area: &gtk::GLArea,
    overlay: &gtk::Overlay,
    surface: Option<ghostty_surface_t>,
    callbacks: &Rc<RefCell<TerminalCallbacks>>,
    x: f64,
    y: f64,
    mods: c_int,
) {
    if !widget_has_native_surface(gl_area) {
        return;
    }

    let popover = build_terminal_context_menu(gl_area, overlay, surface, callbacks, x, y, mods);
    if widget_has_native_surface(gl_area) {
        popover.popup();
    }
    // The `closed` handler detaches the menu, but GTK emits `closed` only when
    // it hides a menu it presented: emit it for one never shown, or one the
    // compositor refused (a popup grab it rejects, or no seat at all).
    if !popover.is_mapped() {
        popover.emit_by_name::<()>("closed", &[]);
    }
}

pub(super) fn build_terminal_context_menu(
    gl_area: &gtk::GLArea,
    overlay: &gtk::Overlay,
    surface: Option<ghostty_surface_t>,
    callbacks: &Rc<RefCell<TerminalCallbacks>>,
    x: f64,
    y: f64,
    mods: c_int,
) -> gtk::Popover {
    let menu_box = build_popover_inner_box();
    let url = url_at_position(surface, x, y, mods);

    let has_selection = surface
        .map(|s| unsafe { ghostty_surface_has_selection(s) })
        .unwrap_or(false);

    let mut items: Vec<(&str, bool)> = vec![("Copy", has_selection)];
    if url.is_some() {
        items.push(("Copy URL", true));
    }
    if url.as_deref().is_some_and(link_uri::is_safe_external_url) {
        items.push(("Open in", true));
    }
    items.extend([
        ("Paste", true),
        ("---", false),
        ("IDs", true),
        ("---", false),
        ("Browser", true),
        ("Split Right", true),
        ("Split Down", true),
        ("Keybinds", true),
        ("---", false),
        ("Clear", true),
    ]);

    let identity = (callbacks.borrow().identity)();
    let ids_popover = gtk::Popover::new();
    ids_popover.set_has_arrow(false);
    ids_popover.set_position(gtk::PositionType::Right);
    let ids_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    ids_box.set_margin_top(4);
    ids_box.set_margin_bottom(4);
    ids_box.set_margin_start(4);
    ids_box.set_margin_end(4);
    let copy_workspace_btn = gtk::Button::with_label("Copy Workspace ID");
    copy_workspace_btn.add_css_class("flat");
    copy_workspace_btn.set_sensitive(identity.workspace_id.is_some());
    let copy_surface_btn = gtk::Button::with_label("Copy Surface ID");
    copy_surface_btn.add_css_class("flat");
    for btn in [&copy_workspace_btn, &copy_surface_btn] {
        btn.set_halign(gtk::Align::Fill);
        if let Some(lbl) = btn.child().and_then(|c| c.downcast::<gtk::Label>().ok()) {
            lbl.set_xalign(0.0);
        }
        ids_box.append(btn);
    }
    ids_popover.set_child(Some(&ids_box));
    let ids_menu_button = build_submenu_button("IDs", &ids_popover);

    let open_in_popover = gtk::Popover::new();
    open_in_popover.set_has_arrow(false);
    open_in_popover.set_position(gtk::PositionType::Right);
    let open_in_box = build_popover_inner_box();
    let open_default_browser_btn = gtk::Button::with_label("Default browser");
    let open_browser_tab_btn = gtk::Button::with_label("Browser tab");
    open_browser_tab_btn.set_sensitive(
        cfg!(feature = "webkit")
            && url
                .as_deref()
                .is_some_and(link_uri::is_embedded_browser_url),
    );
    for btn in [&open_default_browser_btn, &open_browser_tab_btn] {
        btn.add_css_class("flat");
        btn.set_halign(gtk::Align::Fill);
        if let Some(label) = btn
            .child()
            .and_then(|child| child.downcast::<gtk::Label>().ok())
        {
            label.set_xalign(0.0);
        }
        open_in_box.append(btn);
    }
    open_in_popover.set_child(Some(&open_in_box));
    let open_in_menu_button = build_submenu_button("Open in…", &open_in_popover);

    for (label, enabled) in &items {
        if *label == "---" {
            let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
            sep.set_margin_top(4);
            sep.set_margin_bottom(4);
            menu_box.append(&sep);
            continue;
        }

        if *label == "IDs" {
            menu_box.append(&ids_menu_button);
            continue;
        }
        if *label == "Open in" {
            menu_box.append(&open_in_menu_button);
            continue;
        }

        let btn = gtk::Button::with_label(label);
        btn.add_css_class("flat");
        btn.set_sensitive(*enabled);
        btn.set_halign(gtk::Align::Fill);
        if let Some(lbl) = btn.child().and_then(|c| c.downcast::<gtk::Label>().ok()) {
            lbl.set_xalign(0.0);
        }
        let motion = gtk::EventControllerMotion::new();
        let ids_pop = ids_popover.clone();
        let open_in_pop = open_in_popover.clone();
        motion.connect_enter(move |_, _, _| {
            ids_pop.popdown();
            open_in_pop.popdown();
        });
        btn.add_controller(motion);
        menu_box.append(&btn);
    }

    let popover = build_floating_popover(gl_area, &menu_box);
    popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));

    // Wire up each button
    let mut child = menu_box.first_child();
    while let Some(widget) = child {
        if let Some(btn) = widget.downcast_ref::<gtk::Button>() {
            let label = btn.label().unwrap_or_default().to_string();
            // Weak: GTK 4.22 can keep a closed menu alive, which must not keep
            // the terminal alive with it.
            let pop = popover.downgrade();
            let cb = callbacks.clone();
            let gl_area = gl_area.downgrade();
            let url = url.clone();
            let overlay = overlay.downgrade();

            btn.connect_clicked(move |_| {
                let Some(pop) = pop.upgrade() else {
                    return;
                };
                pop.popdown();
                match label.as_str() {
                    "Copy" => surface_action(surface, "copy_to_clipboard"),
                    "Copy URL" => {
                        if let Some(url) = url.as_deref() {
                            copy_text_to_clipboards(url);
                            if let Some(overlay) = overlay.upgrade() {
                                show_clipboard_toast(&overlay);
                            }
                        }
                    }
                    "Paste" => paste_clipboard(surface, &cb),
                    "Browser" => {
                        let callbacks = cb.borrow();
                        (callbacks.on_open_browser_here)();
                    }
                    "Split Right" => {
                        let callbacks = cb.borrow();
                        (callbacks.on_split_right)();
                    }
                    "Split Down" => {
                        let callbacks = cb.borrow();
                        (callbacks.on_split_down)();
                    }
                    "Keybinds" => {
                        if let Some(gl_area) = gl_area.upgrade() {
                            let anchor: gtk::Widget = gl_area.upcast();
                            let cb = cb.clone();
                            glib::timeout_add_local_once(Duration::from_millis(80), move || {
                                let callbacks = cb.borrow();
                                (callbacks.on_open_keybinds)(&anchor);
                            });
                        }
                    }
                    "Clear" => surface_action(surface, "clear_screen"),
                    _ => {}
                }
            });
        }
        child = widget.next_sibling();
    }

    for (button, destination) in [
        (
            open_default_browser_btn,
            LinkOpenDestination::DefaultBrowser,
        ),
        (open_browser_tab_btn, LinkOpenDestination::BrowserTab),
    ] {
        let pop = popover.downgrade();
        let callbacks = callbacks.clone();
        let url = url.clone();
        button.connect_clicked(move |_| {
            // The menu's closed handler closes the submenu too.
            if let Some(pop) = pop.upgrade() {
                pop.popdown();
            }
            if let Some(url) = url.as_deref() {
                (callbacks.borrow().on_open_url)(url, LinkOpenRequest::Destination(destination));
            }
        });
    }

    {
        let pop = popover.downgrade();
        let overlay = overlay.downgrade();
        let workspace_id = identity.workspace_id.clone();
        copy_workspace_btn.connect_clicked(move |_| {
            if let Some(workspace_id) = workspace_id.as_deref() {
                copy_text_to_clipboards(workspace_id);
                if let Some(overlay) = overlay.upgrade() {
                    show_clipboard_toast(&overlay);
                }
            }
            // The menu's closed handler closes the submenu too.
            if let Some(pop) = pop.upgrade() {
                pop.popdown();
            }
        });
    }

    {
        let pop = popover.downgrade();
        let overlay = overlay.downgrade();
        let surface_id = identity.surface_id.clone();
        copy_surface_btn.connect_clicked(move |_| {
            copy_text_to_clipboards(&surface_id);
            if let Some(overlay) = overlay.upgrade() {
                show_clipboard_toast(&overlay);
            }
            // The menu's closed handler closes the submenu too.
            if let Some(pop) = pop.upgrade() {
                pop.popdown();
            }
        });
    }

    {
        let ids_menu_button = ids_menu_button.clone();
        let open_in_menu_button = open_in_menu_button.clone();
        let gl = gl_area.downgrade();
        popover.connect_closed(move |p| {
            // Give the focus back to the terminal before the submenus and the
            // menu are unparented, or GTK 4.22 leaks them (see
            // `unset_focus_within`). A visible focus also drops the move the
            // closing hide queued.
            if focus_is_within(p.upcast_ref()) && !gl.upgrade().is_some_and(|gl| gl.grab_focus()) {
                unset_focus_within(p.upcast_ref());
            }
            ids_menu_button.popdown();
            ids_menu_button.set_popover(None::<&gtk::Popover>);
            open_in_menu_button.popdown();
            open_in_menu_button.set_popover(None::<&gtk::Popover>);
            p.unparent();
        });
    }

    popover
}
