use super::*;
use crate::terminal_font::TerminalFontConfig;

#[test]
#[ignore = "requires a graphical display and Ghostty resources"]
fn terminal_font_config_updates_live_and_new_ghostty_surfaces() {
    use std::time::{Duration, Instant};

    let pump_for = |duration: Duration| {
        let context = glib::MainContext::default();
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            for _ in 0..100 {
                if !context.pending() {
                    break;
                }
                context.iteration(false);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    };

    let temp = tempfile::tempdir().unwrap();
    for key in [
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
    ] {
        let path = temp.path().join(key);
        std::fs::create_dir_all(&path).unwrap();
        std::env::set_var(key, path);
    }
    let ghostty_dir = temp.path().join("XDG_CONFIG_HOME/ghostty");
    std::fs::create_dir_all(&ghostty_dir).unwrap();
    std::fs::write(
        ghostty_dir.join("extra.ghostty"),
        "font-size = 14\nadjust-cell-width = 20%\nfont-feature = \"calt\" = 0\nfreetype-load-flags = monochrome\n",
    )
    .unwrap();
    std::fs::write(
        ghostty_dir.join("config.ghostty"),
        "command = direct:/bin/sh\nshell-integration = none\nconfig-file = extra.ghostty\n",
    )
    .unwrap();
    let socket = temp.path().join("limux.sock");
    std::env::set_var("LIMUX_SOCKET", &socket);
    std::env::set_var("LIMUX_SOCKET_PATH", &socket);
    std::env::set_var("LIMUX_AGENT_HOOK_STATE_DIR", temp.path().join("hooks"));
    std::env::set_var("LIMUX_EAGER_RESTORE", "0");

    crate::prepare_ghostty_runtime();
    adw::init().unwrap();
    crate::terminal::init_ghostty();

    // Verify base Ghostty font defaults resolve `config.ghostty` and recursive `config-file`.
    let base_defaults = crate::terminal::base_ghostty_font_defaults();
    assert_eq!(base_defaults.size, 14.0);
    assert_eq!(crate::terminal::default_font_size(), 14.0);
    assert_eq!(base_defaults.cell_width_percent, 120);
    assert!(base_defaults.has_cell_width_modifier);
    assert!(!base_defaults.use_ligatures);
    assert!(!base_defaults.anti_aliased);

    let app = adw::Application::builder()
        .application_id("dev.limux.TerminalFontLiveTest")
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    build_window(&app);

    let state = CONTROL_STATE.with(|slot| slot.borrow().as_ref().unwrap().clone());
    let (root, host_window) = {
        let s = state.borrow();
        (s.active_workspace().unwrap().root.clone(), s.window.clone())
    };

    const PREFERRED_LATIN_MONO_FAMILIES: &[&str] = &[
        "MesloLGS NF",
        "DejaVu Sans Mono",
        "Liberation Mono",
        "Noto Sans Mono",
        "Ubuntu Sans Mono",
        "Ubuntu Mono",
        "JetBrains Mono",
        "Fira Code",
        "Hack",
        "Cascadia Mono",
        "FreeMono",
        "Nimbus Mono PS",
    ];
    let (chosen_family, chosen_style) = {
        let font_map = host_window
            .pango_context()
            .font_map()
            .expect("Pango font map");
        let families = font_map.list_families();
        let preferred = PREFERRED_LATIN_MONO_FAMILIES
            .iter()
            .find_map(|candidate| {
                families
                    .iter()
                    .find(|f| f.name().eq_ignore_ascii_case(candidate))
            })
            .or_else(|| {
                families.iter().find(|f| {
                    let lower = f.name().to_ascii_lowercase();
                    f.is_monospace()
                        && lower != "monospace"
                        && !lower.contains("emoji")
                        && !lower.contains("symbol")
                        && !lower.contains("signwriting")
                })
            })
            .or_else(|| families.first())
            .expect("at least one installed font family");
        let face_names: Vec<String> = preferred
            .list_faces()
            .into_iter()
            .map(|face| face.face_name().to_string())
            .collect();
        let sorted = crate::terminal_font::sort_face_styles(face_names);
        let style = crate::terminal_font::preferred_style_for_family_change(&sorted, None)
            .or_else(|| sorted.into_iter().next());
        (preferred.name().to_string(), style)
    };

    let deadline = Instant::now() + Duration::from_secs(5);
    let (source_pane_id, handle) = loop {
        pump_for(Duration::from_millis(15));
        let panes = pane::pane_summaries_for_root(&root);
        if let [only] = panes.as_slice() {
            if let Some((_, term_handle)) = pane::terminal_handle_for_root(&root) {
                term_handle.realize_in_background();
                if term_handle.health().realized && term_handle.live_font_size_for_test().is_some()
                {
                    break (only.pane_id, term_handle);
                }
            }
        }
        assert!(Instant::now() < deadline, "initial surface never realized");
    };

    // Inheriting Ghostty's `adjust-cell-width = 20%` produces wider cells than explicit 100% (`0%`).
    let inherited_width_cfg = TerminalFontConfig {
        family: Some(chosen_family.clone()),
        style: chosen_style.clone(),
        size: Some(13.0),
        cell_width_percent: None,
        cell_height_percent: None,
        use_ligatures: Some(false),
        anti_aliased: Some(true),
    };
    {
        let previous = state.borrow().config.borrow().clone();
        let mut updated = previous.clone();
        updated.terminal_font = inherited_width_cfg.clone();
        state.borrow().config.borrow_mut().clone_from(&updated);
        handle_config_change(&state, &previous, &updated);
    }
    pump_for(Duration::from_millis(40));

    let resolved_inherited_120 = handle
        .resolve_font_cluster_json_for_test("M", false, false)
        .expect("resolve cluster with Ghostty base 20% width");
    let width_inherited_120 = resolved_inherited_120["metrics"]["cell_width"]
        .as_u64()
        .unwrap();

    let base_cfg = TerminalFontConfig {
        cell_width_percent: Some(100),
        cell_height_percent: Some(100),
        ..inherited_width_cfg
    };
    {
        let previous = state.borrow().config.borrow().clone();
        let mut updated = previous.clone();
        updated.terminal_font = base_cfg.clone();
        state.borrow().config.borrow_mut().clone_from(&updated);
        handle_config_change(&state, &previous, &updated);
    }
    pump_for(Duration::from_millis(40));

    assert_eq!(handle.live_font_size_for_test(), Some(13.0));
    let resolved_100 = handle
        .resolve_font_cluster_json_for_test("M", false, false)
        .expect("resolve cluster at 100%");
    let resolved_family = resolved_100["runs"][0]["family"]
        .as_str()
        .unwrap_or_default();
    assert!(
        resolved_family.eq_ignore_ascii_case(&chosen_family),
        "expected Ghostty primary family `{chosen_family}`, got `{resolved_family}`: {resolved_100}"
    );
    let width_100 = resolved_100["metrics"]["cell_width"].as_u64().unwrap();
    let height_100 = resolved_100["metrics"]["cell_height"].as_u64().unwrap();
    assert!(
        width_100 < width_inherited_120,
        "expected explicit 100% width override to shrink cells below Ghostty's 20% base: {width_inherited_120} -> {width_100}"
    );

    let spaced_cfg = TerminalFontConfig {
        cell_width_percent: Some(140),
        cell_height_percent: Some(135),
        ..base_cfg
    };
    {
        let previous = state.borrow().config.borrow().clone();
        let mut updated = previous.clone();
        updated.terminal_font = spaced_cfg;
        state.borrow().config.borrow_mut().clone_from(&updated);
        handle_config_change(&state, &previous, &updated);
    }
    pump_for(Duration::from_millis(40));

    let resolved_spaced = handle
        .resolve_font_cluster_json_for_test("M", false, false)
        .expect("resolve cluster with spacing");
    let width_spaced = resolved_spaced["metrics"]["cell_width"].as_u64().unwrap();
    let height_spaced = resolved_spaced["metrics"]["cell_height"].as_u64().unwrap();
    assert!(
        width_spaced > width_100,
        "expected cell_width to grow at 140%: {width_100} -> {width_spaced}"
    );
    assert!(
        height_spaced > height_100,
        "expected cell_height to grow at 135%: {height_100} -> {height_spaced}"
    );

    // Zooming via shortcut and resetting via Ctrl+0 returns to configured 13pt, not 12pt.
    assert!(persist_font_size_delta(&state, 2.0));
    assert_eq!(handle.live_font_size_for_test(), Some(15.0));
    assert!(persist_font_size_reset(&state));
    assert_eq!(handle.live_font_size_for_test(), Some(13.0));

    // Clearing a zoom override through Settings resets live surfaces even when terminal_font is unchanged.
    assert!(persist_font_size_delta(&state, 2.0));
    assert_eq!(handle.live_font_size_for_test(), Some(15.0));
    {
        let previous = state.borrow().config.borrow().clone();
        let mut updated = previous.clone();
        updated.font_size = None;
        state.borrow().config.borrow_mut().clone_from(&updated);
        handle_config_change(&state, &previous, &updated);
    }
    pump_for(Duration::from_millis(40));
    assert_eq!(handle.live_font_size_for_test(), Some(13.0));

    // Newly split pane inherits the configured terminal font and size.
    let (reply, rx) = std::sync::mpsc::channel();
    handle_control_command(
        &state,
        ControlCommand::CreatePane {
            request: crate::control_bridge::CreatePaneRequest {
                target: WorkspaceTarget::Active,
                source_pane_id: Some(source_pane_id.to_string()),
                source_surface_id: None,
                direction: BridgePaneCreateDirection::Right,
                pane_type: PaneCreateType::Terminal,
                command: None,
                url: None,
            },
            reply,
        },
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let create_response = loop {
        if let Ok(result) = rx.try_recv() {
            break result.expect("pane.create failed");
        }
        assert!(Instant::now() < deadline, "pane.create never replied");
        pump_for(Duration::from_millis(10));
    };
    let new_pane_id: u32 = create_response["pane_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let new_surface_id = create_response["surface_id"].as_str().unwrap().to_string();

    let deadline = Instant::now() + Duration::from_secs(5);
    let second_handle = loop {
        pump_for(Duration::from_millis(15));
        let current_root = state.borrow().active_workspace().unwrap().root.clone();
        if let Some(pane_widget) = pane::pane_widget_for_root(&current_root, new_pane_id) {
            if let Some((_, h)) =
                pane::terminal_handle_for_surface(&pane_widget, Some(&new_surface_id))
            {
                h.realize_in_background();
                if h.health().realized && h.live_font_size_for_test().is_some() {
                    break h;
                }
            }
        }
        assert!(
            Instant::now() < deadline,
            "new split surface `{new_surface_id}` never realized"
        );
    };
    assert_eq!(second_handle.live_font_size_for_test(), Some(13.0));
    let second_resolved = second_handle
        .resolve_font_cluster_json_for_test("M", false, false)
        .expect("resolve cluster on new surface");
    assert!(
        second_resolved["runs"][0]["family"]
            .as_str()
            .unwrap_or_default()
            .eq_ignore_ascii_case(&chosen_family),
        "new surface did not inherit configured family `{chosen_family}`: {second_resolved}"
    );

    host_window.close();
    pump_for(Duration::from_millis(50));
}
