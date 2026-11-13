#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MethodCategory {
    System,
    App,
    Window,
    Workspace,
    Pane,
    Surface,
    Notification,
    Tab,
    GuiHook,
    Browser,
    Debug,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodSpec {
    pub name: &'static str,
    pub category: MethodCategory,
    pub aliases: &'static [&'static str],
    pub in_core: bool,
}

const fn core_spec(
    name: &'static str,
    category: MethodCategory,
    aliases: &'static [&'static str],
) -> MethodSpec {
    MethodSpec {
        name,
        category,
        aliases,
        in_core: true,
    }
}

const fn host_spec(
    name: &'static str,
    category: MethodCategory,
    aliases: &'static [&'static str],
) -> MethodSpec {
    MethodSpec {
        name,
        category,
        aliases,
        in_core: false,
    }
}

pub const CORE_COMMAND_NAMES: [&str; 166] = [
    "system.ping",
    "system.identify",
    "system.capabilities",
    "app.focus_override.set",
    "app.simulate_active",
    "window.list",
    "window.current",
    "window.create",
    "window.focus",
    "window.close",
    "workspace.list",
    "workspace.current",
    "workspace.create",
    "workspace.select",
    "workspace.next",
    "workspace.previous",
    "workspace.last",
    "workspace.rename",
    "workspace.reorder",
    "workspace.close",
    "workspace.move_to_window",
    "workspace.action",
    "pane.list",
    "pane.surfaces",
    "pane.create",
    "pane.focus",
    "pane.swap",
    "pane.break",
    "pane.join",
    "pane.last",
    "pane.resize",
    "surface.list",
    "surface.current",
    "surface.create",
    "surface.split",
    "surface.focus",
    "surface.close",
    "surface.move",
    "surface.reorder",
    "surface.drag_to_split",
    "surface.refresh",
    "surface.health",
    "surface.read_text",
    "surface.send_text",
    "surface.send_key",
    "surface.trigger_flash",
    "surface.clear_history",
    "surface.action",
    "notification.create",
    "notification.create_for_surface",
    "notification.list",
    "notification.clear",
    "tab.action",
    "browser.open_split",
    "browser.navigate",
    "browser.url.get",
    "browser.eval",
    "browser.wait",
    "browser.click",
    "browser.fill",
    "browser.get.text",
    "browser.get.value",
    "browser.get.title",
    "browser.snapshot",
    "browser.focus_webview",
    "browser.is_webview_focused",
    "browser.screenshot",
    "browser.back",
    "browser.forward",
    "browser.reload",
    "browser.focus",
    "browser.hover",
    "browser.dblclick",
    "browser.press",
    "browser.keydown",
    "browser.keyup",
    "browser.type",
    "browser.check",
    "browser.uncheck",
    "browser.select",
    "browser.scroll",
    "browser.scroll_into_view",
    "browser.get.attr",
    "browser.get.box",
    "browser.get.count",
    "browser.get.html",
    "browser.get.styles",
    "browser.is.checked",
    "browser.is.enabled",
    "browser.is.visible",
    "browser.find.role",
    "browser.find.text",
    "browser.find.label",
    "browser.find.placeholder",
    "browser.find.alt",
    "browser.find.title",
    "browser.find.testid",
    "browser.find.first",
    "browser.find.last",
    "browser.find.nth",
    "browser.highlight",
    "browser.viewport.set",
    "browser.geolocation.set",
    "browser.offline.set",
    "browser.trace.start",
    "browser.trace.stop",
    "browser.network.route",
    "browser.network.unroute",
    "browser.network.requests",
    "browser.screencast.start",
    "browser.screencast.stop",
    "browser.input_mouse",
    "browser.input_keyboard",
    "browser.input_touch",
    "browser.addscript",
    "browser.addinitscript",
    "browser.addstyle",
    "browser.console.list",
    "browser.console.clear",
    "browser.errors.list",
    "browser.cookies.get",
    "browser.cookies.set",
    "browser.cookies.clear",
    "browser.storage.get",
    "browser.storage.set",
    "browser.storage.clear",
    "browser.tab.list",
    "browser.tab.new",
    "browser.tab.switch",
    "browser.tab.close",
    "browser.frame.main",
    "browser.frame.select",
    "browser.dialog.accept",
    "browser.dialog.dismiss",
    "browser.download.wait",
    "browser.state.save",
    "browser.state.load",
    "debug.app.activate",
    "debug.bonsplit_underflow.count",
    "debug.bonsplit_underflow.reset",
    "debug.command_palette.rename_input.delete_backward",
    "debug.command_palette.rename_input.interact",
    "debug.command_palette.rename_input.select_all",
    "debug.command_palette.rename_input.selection",
    "debug.command_palette.rename_tab.open",
    "debug.command_palette.results",
    "debug.command_palette.selection",
    "debug.command_palette.toggle",
    "debug.command_palette.visible",
    "debug.empty_panel.count",
    "debug.empty_panel.reset",
    "debug.flash.count",
    "debug.flash.reset",
    "debug.layout",
    "debug.notification.focus",
    "debug.panel_snapshot",
    "debug.panel_snapshot.reset",
    "debug.portal.stats",
    "debug.shortcut.set",
    "debug.shortcut.simulate",
    "debug.sidebar.visible",
    "debug.terminal.is_focused",
    "debug.terminal.read_text",
    "debug.terminal.render_stats",
    "debug.type",
    "debug.window.screenshot",
];

pub const HOST_ONLY_COMMAND_NAMES: [&str; 12] = [
    "window.activate",
    "workspace.set_color",
    "workspace.set_group",
    "workspace.group_list",
    "workspace.group_create",
    "workspace.group_rename",
    "workspace.group_set_color",
    "workspace.group_set_collapsed",
    "workspace.group_delete",
    "gui_hook.list",
    "gui_hook.set",
    "gui_hook.clear",
];

pub const ALL_METHOD_SPECS: [MethodSpec; 178] = [
    core_spec("system.ping", MethodCategory::System, &["ping"]),
    core_spec("system.identify", MethodCategory::System, &["identify"]),
    core_spec(
        "system.capabilities",
        MethodCategory::System,
        &["capabilities"],
    ),
    core_spec("app.focus_override.set", MethodCategory::App, &[]),
    core_spec("app.simulate_active", MethodCategory::App, &[]),
    core_spec("window.list", MethodCategory::Window, &["list-windows"]),
    core_spec("window.current", MethodCategory::Window, &[]),
    core_spec("window.create", MethodCategory::Window, &["new-window"]),
    core_spec("window.focus", MethodCategory::Window, &["focus-window"]),
    core_spec("window.close", MethodCategory::Window, &["close-window"]),
    host_spec("window.activate", MethodCategory::Window, &["activate"]),
    core_spec(
        "workspace.list",
        MethodCategory::Workspace,
        &["list-workspaces"],
    ),
    core_spec("workspace.current", MethodCategory::Workspace, &[]),
    core_spec(
        "workspace.create",
        MethodCategory::Workspace,
        &["new-workspace"],
    ),
    core_spec(
        "workspace.select",
        MethodCategory::Workspace,
        &[
            "workspace.activate",
            "activate-workspace",
            "select-workspace",
        ],
    ),
    core_spec(
        "workspace.next",
        MethodCategory::Workspace,
        &["next-workspace"],
    ),
    core_spec(
        "workspace.previous",
        MethodCategory::Workspace,
        &["previous-workspace"],
    ),
    core_spec(
        "workspace.last",
        MethodCategory::Workspace,
        &["last-workspace"],
    ),
    core_spec(
        "workspace.rename",
        MethodCategory::Workspace,
        &["rename-workspace"],
    ),
    core_spec("workspace.reorder", MethodCategory::Workspace, &[]),
    core_spec(
        "workspace.close",
        MethodCategory::Workspace,
        &["close-workspace"],
    ),
    core_spec("workspace.move_to_window", MethodCategory::Workspace, &[]),
    core_spec("workspace.action", MethodCategory::Workspace, &[]),
    host_spec(
        "workspace.set_color",
        MethodCategory::Workspace,
        &["set-workspace-color"],
    ),
    host_spec(
        "workspace.set_group",
        MethodCategory::Workspace,
        &["set-workspace-group"],
    ),
    host_spec(
        "workspace.group_list",
        MethodCategory::Workspace,
        &["list-groups"],
    ),
    host_spec(
        "workspace.group_create",
        MethodCategory::Workspace,
        &["create-group"],
    ),
    host_spec(
        "workspace.group_rename",
        MethodCategory::Workspace,
        &["rename-group"],
    ),
    host_spec(
        "workspace.group_set_color",
        MethodCategory::Workspace,
        &["set-group-color"],
    ),
    host_spec(
        "workspace.group_set_collapsed",
        MethodCategory::Workspace,
        &[],
    ),
    host_spec(
        "workspace.group_delete",
        MethodCategory::Workspace,
        &["delete-group"],
    ),
    core_spec("pane.list", MethodCategory::Pane, &["list-panes"]),
    core_spec("pane.surfaces", MethodCategory::Pane, &[]),
    core_spec("pane.create", MethodCategory::Pane, &["new-pane"]),
    core_spec("pane.focus", MethodCategory::Pane, &["focus-pane"]),
    core_spec("pane.swap", MethodCategory::Pane, &["swap-pane"]),
    core_spec("pane.break", MethodCategory::Pane, &["break-pane"]),
    core_spec("pane.join", MethodCategory::Pane, &["join-pane"]),
    core_spec("pane.last", MethodCategory::Pane, &["last-pane"]),
    core_spec("pane.resize", MethodCategory::Pane, &["resize-pane"]),
    core_spec("surface.list", MethodCategory::Surface, &["list-panels"]),
    core_spec("surface.current", MethodCategory::Surface, &[]),
    core_spec("surface.create", MethodCategory::Surface, &["new-surface"]),
    core_spec("surface.split", MethodCategory::Surface, &["new-split"]),
    core_spec("surface.focus", MethodCategory::Surface, &["focus-surface"]),
    core_spec("surface.close", MethodCategory::Surface, &["close-surface"]),
    core_spec("surface.move", MethodCategory::Surface, &[]),
    core_spec("surface.reorder", MethodCategory::Surface, &[]),
    core_spec("surface.drag_to_split", MethodCategory::Surface, &[]),
    core_spec("surface.refresh", MethodCategory::Surface, &[]),
    core_spec(
        "surface.health",
        MethodCategory::Surface,
        &["surface-health"],
    ),
    core_spec(
        "surface.read_text",
        MethodCategory::Surface,
        &["read-screen", "capture-pane"],
    ),
    core_spec(
        "surface.send_text",
        MethodCategory::Surface,
        &["send-text", "send"],
    ),
    core_spec("surface.send_key", MethodCategory::Surface, &["send-key"]),
    core_spec("surface.trigger_flash", MethodCategory::Surface, &[]),
    core_spec(
        "surface.clear_history",
        MethodCategory::Surface,
        &["clear-history"],
    ),
    core_spec("surface.action", MethodCategory::Surface, &[]),
    core_spec(
        "notification.create",
        MethodCategory::Notification,
        &["notify"],
    ),
    core_spec(
        "notification.create_for_surface",
        MethodCategory::Notification,
        &[],
    ),
    core_spec("notification.list", MethodCategory::Notification, &[]),
    core_spec("notification.clear", MethodCategory::Notification, &[]),
    core_spec("tab.action", MethodCategory::Tab, &["tab-action"]),
    host_spec("gui_hook.list", MethodCategory::GuiHook, &[]),
    host_spec("gui_hook.set", MethodCategory::GuiHook, &[]),
    host_spec("gui_hook.clear", MethodCategory::GuiHook, &[]),
    core_spec(
        "browser.open_split",
        MethodCategory::Browser,
        &["browser.open-split"],
    ),
    core_spec("browser.navigate", MethodCategory::Browser, &[]),
    core_spec("browser.url.get", MethodCategory::Browser, &[]),
    core_spec("browser.eval", MethodCategory::Browser, &[]),
    core_spec("browser.wait", MethodCategory::Browser, &[]),
    core_spec("browser.click", MethodCategory::Browser, &[]),
    core_spec("browser.fill", MethodCategory::Browser, &[]),
    core_spec("browser.get.text", MethodCategory::Browser, &[]),
    core_spec("browser.get.value", MethodCategory::Browser, &[]),
    core_spec("browser.get.title", MethodCategory::Browser, &[]),
    core_spec("browser.snapshot", MethodCategory::Browser, &[]),
    core_spec("browser.focus_webview", MethodCategory::Browser, &[]),
    core_spec("browser.is_webview_focused", MethodCategory::Browser, &[]),
    core_spec("browser.screenshot", MethodCategory::Browser, &[]),
    core_spec("browser.back", MethodCategory::Browser, &[]),
    core_spec("browser.forward", MethodCategory::Browser, &[]),
    core_spec("browser.reload", MethodCategory::Browser, &[]),
    core_spec("browser.focus", MethodCategory::Browser, &[]),
    core_spec("browser.hover", MethodCategory::Browser, &[]),
    core_spec("browser.dblclick", MethodCategory::Browser, &[]),
    core_spec("browser.press", MethodCategory::Browser, &[]),
    core_spec("browser.keydown", MethodCategory::Browser, &[]),
    core_spec("browser.keyup", MethodCategory::Browser, &[]),
    core_spec("browser.type", MethodCategory::Browser, &[]),
    core_spec("browser.check", MethodCategory::Browser, &[]),
    core_spec("browser.uncheck", MethodCategory::Browser, &[]),
    core_spec("browser.select", MethodCategory::Browser, &[]),
    core_spec("browser.scroll", MethodCategory::Browser, &[]),
    core_spec("browser.scroll_into_view", MethodCategory::Browser, &[]),
    core_spec("browser.get.attr", MethodCategory::Browser, &[]),
    core_spec("browser.get.box", MethodCategory::Browser, &[]),
    core_spec("browser.get.count", MethodCategory::Browser, &[]),
    core_spec("browser.get.html", MethodCategory::Browser, &[]),
    core_spec("browser.get.styles", MethodCategory::Browser, &[]),
    core_spec("browser.is.checked", MethodCategory::Browser, &[]),
    core_spec("browser.is.enabled", MethodCategory::Browser, &[]),
    core_spec("browser.is.visible", MethodCategory::Browser, &[]),
    core_spec("browser.find.role", MethodCategory::Browser, &[]),
    core_spec("browser.find.text", MethodCategory::Browser, &[]),
    core_spec("browser.find.label", MethodCategory::Browser, &[]),
    core_spec("browser.find.placeholder", MethodCategory::Browser, &[]),
    core_spec("browser.find.alt", MethodCategory::Browser, &[]),
    core_spec("browser.find.title", MethodCategory::Browser, &[]),
    core_spec("browser.find.testid", MethodCategory::Browser, &[]),
    core_spec("browser.find.first", MethodCategory::Browser, &[]),
    core_spec("browser.find.last", MethodCategory::Browser, &[]),
    core_spec("browser.find.nth", MethodCategory::Browser, &[]),
    core_spec("browser.highlight", MethodCategory::Browser, &[]),
    core_spec("browser.viewport.set", MethodCategory::Browser, &[]),
    core_spec("browser.geolocation.set", MethodCategory::Browser, &[]),
    core_spec("browser.offline.set", MethodCategory::Browser, &[]),
    core_spec("browser.trace.start", MethodCategory::Browser, &[]),
    core_spec("browser.trace.stop", MethodCategory::Browser, &[]),
    core_spec("browser.network.route", MethodCategory::Browser, &[]),
    core_spec("browser.network.unroute", MethodCategory::Browser, &[]),
    core_spec("browser.network.requests", MethodCategory::Browser, &[]),
    core_spec("browser.screencast.start", MethodCategory::Browser, &[]),
    core_spec("browser.screencast.stop", MethodCategory::Browser, &[]),
    core_spec("browser.input_mouse", MethodCategory::Browser, &[]),
    core_spec("browser.input_keyboard", MethodCategory::Browser, &[]),
    core_spec("browser.input_touch", MethodCategory::Browser, &[]),
    core_spec("browser.addscript", MethodCategory::Browser, &[]),
    core_spec("browser.addinitscript", MethodCategory::Browser, &[]),
    core_spec("browser.addstyle", MethodCategory::Browser, &[]),
    core_spec("browser.console.list", MethodCategory::Browser, &[]),
    core_spec("browser.console.clear", MethodCategory::Browser, &[]),
    core_spec("browser.errors.list", MethodCategory::Browser, &[]),
    core_spec("browser.cookies.get", MethodCategory::Browser, &[]),
    core_spec("browser.cookies.set", MethodCategory::Browser, &[]),
    core_spec("browser.cookies.clear", MethodCategory::Browser, &[]),
    core_spec("browser.storage.get", MethodCategory::Browser, &[]),
    core_spec("browser.storage.set", MethodCategory::Browser, &[]),
    core_spec("browser.storage.clear", MethodCategory::Browser, &[]),
    core_spec("browser.tab.list", MethodCategory::Browser, &[]),
    core_spec("browser.tab.new", MethodCategory::Browser, &[]),
    core_spec("browser.tab.switch", MethodCategory::Browser, &[]),
    core_spec("browser.tab.close", MethodCategory::Browser, &[]),
    core_spec("browser.frame.main", MethodCategory::Browser, &[]),
    core_spec("browser.frame.select", MethodCategory::Browser, &[]),
    core_spec("browser.dialog.accept", MethodCategory::Browser, &[]),
    core_spec("browser.dialog.dismiss", MethodCategory::Browser, &[]),
    core_spec("browser.download.wait", MethodCategory::Browser, &[]),
    core_spec("browser.state.save", MethodCategory::Browser, &[]),
    core_spec("browser.state.load", MethodCategory::Browser, &[]),
    core_spec("debug.app.activate", MethodCategory::Debug, &[]),
    core_spec("debug.bonsplit_underflow.count", MethodCategory::Debug, &[]),
    core_spec("debug.bonsplit_underflow.reset", MethodCategory::Debug, &[]),
    core_spec(
        "debug.command_palette.rename_input.delete_backward",
        MethodCategory::Debug,
        &[],
    ),
    core_spec(
        "debug.command_palette.rename_input.interact",
        MethodCategory::Debug,
        &[],
    ),
    core_spec(
        "debug.command_palette.rename_input.select_all",
        MethodCategory::Debug,
        &[],
    ),
    core_spec(
        "debug.command_palette.rename_input.selection",
        MethodCategory::Debug,
        &[],
    ),
    core_spec(
        "debug.command_palette.rename_tab.open",
        MethodCategory::Debug,
        &[],
    ),
    core_spec("debug.command_palette.results", MethodCategory::Debug, &[]),
    core_spec(
        "debug.command_palette.selection",
        MethodCategory::Debug,
        &[],
    ),
    core_spec("debug.command_palette.toggle", MethodCategory::Debug, &[]),
    core_spec("debug.command_palette.visible", MethodCategory::Debug, &[]),
    core_spec("debug.empty_panel.count", MethodCategory::Debug, &[]),
    core_spec("debug.empty_panel.reset", MethodCategory::Debug, &[]),
    core_spec("debug.flash.count", MethodCategory::Debug, &[]),
    core_spec("debug.flash.reset", MethodCategory::Debug, &[]),
    core_spec("debug.layout", MethodCategory::Debug, &[]),
    core_spec("debug.notification.focus", MethodCategory::Debug, &[]),
    core_spec("debug.panel_snapshot", MethodCategory::Debug, &[]),
    core_spec("debug.panel_snapshot.reset", MethodCategory::Debug, &[]),
    core_spec("debug.portal.stats", MethodCategory::Debug, &[]),
    core_spec("debug.shortcut.set", MethodCategory::Debug, &[]),
    core_spec("debug.shortcut.simulate", MethodCategory::Debug, &[]),
    core_spec("debug.sidebar.visible", MethodCategory::Debug, &[]),
    core_spec("debug.terminal.is_focused", MethodCategory::Debug, &[]),
    core_spec("debug.terminal.read_text", MethodCategory::Debug, &[]),
    core_spec("debug.terminal.render_stats", MethodCategory::Debug, &[]),
    core_spec("debug.type", MethodCategory::Debug, &[]),
    core_spec("debug.window.screenshot", MethodCategory::Debug, &[]),
];

pub fn resolve_method_spec(method_or_alias: &str) -> Option<&'static MethodSpec> {
    let needle = method_or_alias.trim();
    if needle.is_empty() {
        return None;
    }
    ALL_METHOD_SPECS
        .iter()
        .find(|spec| spec.name == needle || spec.aliases.contains(&needle))
}

pub fn canonical_method_name(method_or_alias: &str) -> Option<&'static str> {
    resolve_method_spec(method_or_alias).map(|spec| spec.name)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{
        canonical_method_name, resolve_method_spec, MethodCategory, ALL_METHOD_SPECS,
        CORE_COMMAND_NAMES, HOST_ONLY_COMMAND_NAMES,
    };

    #[test]
    fn method_specs_match_core_and_host_command_sets_without_duplicates() {
        let mut seen_tokens = HashSet::new();
        for spec in &ALL_METHOD_SPECS {
            assert!(
                seen_tokens.insert(spec.name),
                "duplicate method name {}",
                spec.name
            );
            for alias in spec.aliases {
                assert!(
                    seen_tokens.insert(*alias),
                    "duplicate method alias {alias} on {}",
                    spec.name
                );
            }
        }

        let core_from_specs: Vec<&str> = ALL_METHOD_SPECS
            .iter()
            .filter(|spec| spec.in_core)
            .map(|spec| spec.name)
            .collect();
        assert_eq!(core_from_specs.as_slice(), &CORE_COMMAND_NAMES);

        let host_from_specs: Vec<&str> = ALL_METHOD_SPECS
            .iter()
            .filter(|spec| !spec.in_core)
            .map(|spec| spec.name)
            .collect();
        assert_eq!(host_from_specs.as_slice(), &HOST_ONLY_COMMAND_NAMES);
    }

    #[test]
    fn aliases_resolve_to_canonical_method_and_category() {
        assert_eq!(canonical_method_name("ping"), Some("system.ping"));
        assert_eq!(
            canonical_method_name("capture-pane"),
            Some("surface.read_text")
        );
        assert_eq!(
            canonical_method_name("workspace.activate"),
            Some("workspace.select")
        );
        assert_eq!(
            resolve_method_spec("list-groups").map(|spec| (spec.name, spec.category, spec.in_core)),
            Some(("workspace.group_list", MethodCategory::Workspace, false))
        );
        assert_eq!(canonical_method_name("unknown.method"), None);
    }
}
