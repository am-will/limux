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
    pub in_core: bool,
    pub in_gtk_bridge: bool,
    pub wire_aliases: &'static [&'static str],
    pub cli_subcommands: &'static [&'static str],
}

const fn core_only_spec(
    name: &'static str,
    category: MethodCategory,
    cli_subcommands: &'static [&'static str],
) -> MethodSpec {
    MethodSpec {
        name,
        category,
        in_core: true,
        in_gtk_bridge: false,
        wire_aliases: &[],
        cli_subcommands,
    }
}

const fn shared_spec(
    name: &'static str,
    category: MethodCategory,
    wire_aliases: &'static [&'static str],
    cli_subcommands: &'static [&'static str],
) -> MethodSpec {
    MethodSpec {
        name,
        category,
        in_core: true,
        in_gtk_bridge: true,
        wire_aliases,
        cli_subcommands,
    }
}

const fn gtk_bridge_only_spec(
    name: &'static str,
    category: MethodCategory,
    wire_aliases: &'static [&'static str],
    cli_subcommands: &'static [&'static str],
) -> MethodSpec {
    MethodSpec {
        name,
        category,
        in_core: false,
        in_gtk_bridge: true,
        wire_aliases,
        cli_subcommands,
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

pub const GTK_BRIDGE_COMMAND_NAMES: [&str; 35] = [
    "system.ping",
    "system.identify",
    "system.capabilities",
    "window.activate",
    "workspace.list",
    "workspace.current",
    "workspace.create",
    "workspace.select",
    "workspace.rename",
    "workspace.close",
    "workspace.set_color",
    "workspace.set_group",
    "workspace.group_list",
    "workspace.group_create",
    "workspace.group_rename",
    "workspace.group_set_color",
    "workspace.group_set_collapsed",
    "workspace.group_delete",
    "pane.list",
    "pane.surfaces",
    "pane.create",
    "pane.focus",
    "surface.list",
    "surface.create",
    "surface.focus",
    "surface.close",
    "surface.health",
    "surface.read_text",
    "surface.send_text",
    "surface.send_key",
    "notification.create",
    "tab.action",
    "gui_hook.list",
    "gui_hook.set",
    "gui_hook.clear",
];

pub const ALL_METHOD_SPECS: [MethodSpec; 178] = [
    shared_spec("system.ping", MethodCategory::System, &["ping"], &[]),
    shared_spec(
        "system.identify",
        MethodCategory::System,
        &[],
        &["identify"],
    ),
    shared_spec("system.capabilities", MethodCategory::System, &[], &[]),
    core_only_spec("app.focus_override.set", MethodCategory::App, &[]),
    core_only_spec("app.simulate_active", MethodCategory::App, &[]),
    core_only_spec("window.list", MethodCategory::Window, &[]),
    core_only_spec("window.current", MethodCategory::Window, &[]),
    core_only_spec("window.create", MethodCategory::Window, &[]),
    core_only_spec("window.focus", MethodCategory::Window, &[]),
    core_only_spec("window.close", MethodCategory::Window, &[]),
    gtk_bridge_only_spec(
        "window.activate",
        MethodCategory::Window,
        &[],
        &["activate"],
    ),
    shared_spec(
        "workspace.list",
        MethodCategory::Workspace,
        &["list-workspaces"],
        &["list-workspaces"],
    ),
    shared_spec("workspace.current", MethodCategory::Workspace, &[], &[]),
    shared_spec(
        "workspace.create",
        MethodCategory::Workspace,
        &["new-workspace"],
        &["new-workspace"],
    ),
    shared_spec(
        "workspace.select",
        MethodCategory::Workspace,
        &["workspace.activate", "activate-workspace"],
        &["select-workspace"],
    ),
    core_only_spec(
        "workspace.next",
        MethodCategory::Workspace,
        &["next-window"],
    ),
    core_only_spec(
        "workspace.previous",
        MethodCategory::Workspace,
        &["previous-window"],
    ),
    core_only_spec(
        "workspace.last",
        MethodCategory::Workspace,
        &["last-window"],
    ),
    shared_spec(
        "workspace.rename",
        MethodCategory::Workspace,
        &["rename-workspace"],
        &["rename-workspace", "rename-window"],
    ),
    core_only_spec("workspace.reorder", MethodCategory::Workspace, &[]),
    shared_spec(
        "workspace.close",
        MethodCategory::Workspace,
        &["close-workspace"],
        &["close-workspace"],
    ),
    core_only_spec("workspace.move_to_window", MethodCategory::Workspace, &[]),
    core_only_spec("workspace.action", MethodCategory::Workspace, &[]),
    gtk_bridge_only_spec(
        "workspace.set_color",
        MethodCategory::Workspace,
        &["set-workspace-color"],
        &["set-workspace-color"],
    ),
    gtk_bridge_only_spec(
        "workspace.set_group",
        MethodCategory::Workspace,
        &["set-workspace-group"],
        &["set-workspace-group"],
    ),
    gtk_bridge_only_spec(
        "workspace.group_list",
        MethodCategory::Workspace,
        &["list-groups"],
        &["list-groups"],
    ),
    gtk_bridge_only_spec(
        "workspace.group_create",
        MethodCategory::Workspace,
        &["create-group"],
        &["create-group"],
    ),
    gtk_bridge_only_spec(
        "workspace.group_rename",
        MethodCategory::Workspace,
        &["rename-group"],
        &["rename-group"],
    ),
    gtk_bridge_only_spec(
        "workspace.group_set_color",
        MethodCategory::Workspace,
        &["set-group-color"],
        &["set-group-color"],
    ),
    gtk_bridge_only_spec(
        "workspace.group_set_collapsed",
        MethodCategory::Workspace,
        &[],
        &["collapse-group", "expand-group"],
    ),
    gtk_bridge_only_spec(
        "workspace.group_delete",
        MethodCategory::Workspace,
        &["delete-group"],
        &["delete-group"],
    ),
    shared_spec(
        "pane.list",
        MethodCategory::Pane,
        &["list-panes"],
        &["list-panes"],
    ),
    shared_spec("pane.surfaces", MethodCategory::Pane, &[], &[]),
    shared_spec(
        "pane.create",
        MethodCategory::Pane,
        &["new-pane"],
        &["new-pane"],
    ),
    shared_spec(
        "pane.focus",
        MethodCategory::Pane,
        &["focus-pane"],
        &["focus-pane"],
    ),
    core_only_spec("pane.swap", MethodCategory::Pane, &[]),
    core_only_spec("pane.break", MethodCategory::Pane, &["break-pane"]),
    core_only_spec("pane.join", MethodCategory::Pane, &["join-pane"]),
    core_only_spec("pane.last", MethodCategory::Pane, &["last-pane"]),
    core_only_spec("pane.resize", MethodCategory::Pane, &["resize-pane"]),
    shared_spec(
        "surface.list",
        MethodCategory::Surface,
        &["list-panels"],
        &["list-panels"],
    ),
    core_only_spec("surface.current", MethodCategory::Surface, &[]),
    shared_spec(
        "surface.create",
        MethodCategory::Surface,
        &["new-surface"],
        &["new-surface"],
    ),
    core_only_spec("surface.split", MethodCategory::Surface, &[]),
    shared_spec(
        "surface.focus",
        MethodCategory::Surface,
        &["focus-surface"],
        &["focus-surface"],
    ),
    shared_spec(
        "surface.close",
        MethodCategory::Surface,
        &["close-surface"],
        &["close-surface"],
    ),
    core_only_spec("surface.move", MethodCategory::Surface, &[]),
    core_only_spec("surface.reorder", MethodCategory::Surface, &[]),
    core_only_spec("surface.drag_to_split", MethodCategory::Surface, &[]),
    core_only_spec("surface.refresh", MethodCategory::Surface, &[]),
    shared_spec(
        "surface.health",
        MethodCategory::Surface,
        &["surface-health"],
        &["surface-health"],
    ),
    shared_spec(
        "surface.read_text",
        MethodCategory::Surface,
        &["read-screen", "capture-pane"],
        &["read-screen", "capture-pane"],
    ),
    shared_spec(
        "surface.send_text",
        MethodCategory::Surface,
        &["send-text", "send"],
        &["send"],
    ),
    shared_spec(
        "surface.send_key",
        MethodCategory::Surface,
        &["send-key"],
        &["send-key"],
    ),
    core_only_spec("surface.trigger_flash", MethodCategory::Surface, &[]),
    core_only_spec(
        "surface.clear_history",
        MethodCategory::Surface,
        &["clear-history"],
    ),
    core_only_spec("surface.action", MethodCategory::Surface, &[]),
    shared_spec(
        "notification.create",
        MethodCategory::Notification,
        &["notify"],
        &["notify"],
    ),
    core_only_spec(
        "notification.create_for_surface",
        MethodCategory::Notification,
        &[],
    ),
    core_only_spec("notification.list", MethodCategory::Notification, &[]),
    core_only_spec("notification.clear", MethodCategory::Notification, &[]),
    shared_spec(
        "tab.action",
        MethodCategory::Tab,
        &["tab-action"],
        &["tab-action", "rename-tab"],
    ),
    gtk_bridge_only_spec("gui_hook.list", MethodCategory::GuiHook, &[], &[]),
    gtk_bridge_only_spec("gui_hook.set", MethodCategory::GuiHook, &[], &[]),
    gtk_bridge_only_spec("gui_hook.clear", MethodCategory::GuiHook, &[], &[]),
    core_only_spec(
        "browser.open_split",
        MethodCategory::Browser,
        &["open-browser"],
    ),
    core_only_spec(
        "browser.navigate",
        MethodCategory::Browser,
        &["navigate-browser"],
    ),
    core_only_spec("browser.url.get", MethodCategory::Browser, &[]),
    core_only_spec("browser.eval", MethodCategory::Browser, &[]),
    core_only_spec("browser.wait", MethodCategory::Browser, &[]),
    core_only_spec("browser.click", MethodCategory::Browser, &[]),
    core_only_spec("browser.fill", MethodCategory::Browser, &[]),
    core_only_spec("browser.get.text", MethodCategory::Browser, &[]),
    core_only_spec("browser.get.value", MethodCategory::Browser, &[]),
    core_only_spec("browser.get.title", MethodCategory::Browser, &[]),
    core_only_spec("browser.snapshot", MethodCategory::Browser, &[]),
    core_only_spec("browser.focus_webview", MethodCategory::Browser, &[]),
    core_only_spec("browser.is_webview_focused", MethodCategory::Browser, &[]),
    core_only_spec("browser.screenshot", MethodCategory::Browser, &[]),
    core_only_spec("browser.back", MethodCategory::Browser, &["browser-back"]),
    core_only_spec(
        "browser.forward",
        MethodCategory::Browser,
        &["browser-forward"],
    ),
    core_only_spec(
        "browser.reload",
        MethodCategory::Browser,
        &["browser-reload"],
    ),
    core_only_spec("browser.focus", MethodCategory::Browser, &[]),
    core_only_spec("browser.hover", MethodCategory::Browser, &[]),
    core_only_spec("browser.dblclick", MethodCategory::Browser, &[]),
    core_only_spec("browser.press", MethodCategory::Browser, &[]),
    core_only_spec("browser.keydown", MethodCategory::Browser, &[]),
    core_only_spec("browser.keyup", MethodCategory::Browser, &[]),
    core_only_spec("browser.type", MethodCategory::Browser, &[]),
    core_only_spec("browser.check", MethodCategory::Browser, &[]),
    core_only_spec("browser.uncheck", MethodCategory::Browser, &[]),
    core_only_spec("browser.select", MethodCategory::Browser, &[]),
    core_only_spec("browser.scroll", MethodCategory::Browser, &[]),
    core_only_spec("browser.scroll_into_view", MethodCategory::Browser, &[]),
    core_only_spec("browser.get.attr", MethodCategory::Browser, &[]),
    core_only_spec("browser.get.box", MethodCategory::Browser, &[]),
    core_only_spec("browser.get.count", MethodCategory::Browser, &[]),
    core_only_spec("browser.get.html", MethodCategory::Browser, &[]),
    core_only_spec("browser.get.styles", MethodCategory::Browser, &[]),
    core_only_spec("browser.is.checked", MethodCategory::Browser, &[]),
    core_only_spec("browser.is.enabled", MethodCategory::Browser, &[]),
    core_only_spec("browser.is.visible", MethodCategory::Browser, &[]),
    core_only_spec("browser.find.role", MethodCategory::Browser, &[]),
    core_only_spec("browser.find.text", MethodCategory::Browser, &[]),
    core_only_spec("browser.find.label", MethodCategory::Browser, &[]),
    core_only_spec("browser.find.placeholder", MethodCategory::Browser, &[]),
    core_only_spec("browser.find.alt", MethodCategory::Browser, &[]),
    core_only_spec("browser.find.title", MethodCategory::Browser, &[]),
    core_only_spec("browser.find.testid", MethodCategory::Browser, &[]),
    core_only_spec("browser.find.first", MethodCategory::Browser, &[]),
    core_only_spec("browser.find.last", MethodCategory::Browser, &[]),
    core_only_spec("browser.find.nth", MethodCategory::Browser, &[]),
    core_only_spec("browser.highlight", MethodCategory::Browser, &[]),
    core_only_spec("browser.viewport.set", MethodCategory::Browser, &[]),
    core_only_spec("browser.geolocation.set", MethodCategory::Browser, &[]),
    core_only_spec("browser.offline.set", MethodCategory::Browser, &[]),
    core_only_spec("browser.trace.start", MethodCategory::Browser, &[]),
    core_only_spec("browser.trace.stop", MethodCategory::Browser, &[]),
    core_only_spec("browser.network.route", MethodCategory::Browser, &[]),
    core_only_spec("browser.network.unroute", MethodCategory::Browser, &[]),
    core_only_spec("browser.network.requests", MethodCategory::Browser, &[]),
    core_only_spec("browser.screencast.start", MethodCategory::Browser, &[]),
    core_only_spec("browser.screencast.stop", MethodCategory::Browser, &[]),
    core_only_spec("browser.input_mouse", MethodCategory::Browser, &[]),
    core_only_spec("browser.input_keyboard", MethodCategory::Browser, &[]),
    core_only_spec("browser.input_touch", MethodCategory::Browser, &[]),
    core_only_spec("browser.addscript", MethodCategory::Browser, &[]),
    core_only_spec("browser.addinitscript", MethodCategory::Browser, &[]),
    core_only_spec("browser.addstyle", MethodCategory::Browser, &[]),
    core_only_spec("browser.console.list", MethodCategory::Browser, &[]),
    core_only_spec("browser.console.clear", MethodCategory::Browser, &[]),
    core_only_spec("browser.errors.list", MethodCategory::Browser, &[]),
    core_only_spec("browser.cookies.get", MethodCategory::Browser, &[]),
    core_only_spec("browser.cookies.set", MethodCategory::Browser, &[]),
    core_only_spec("browser.cookies.clear", MethodCategory::Browser, &[]),
    core_only_spec("browser.storage.get", MethodCategory::Browser, &[]),
    core_only_spec("browser.storage.set", MethodCategory::Browser, &[]),
    core_only_spec("browser.storage.clear", MethodCategory::Browser, &[]),
    core_only_spec("browser.tab.list", MethodCategory::Browser, &[]),
    core_only_spec("browser.tab.new", MethodCategory::Browser, &[]),
    core_only_spec("browser.tab.switch", MethodCategory::Browser, &[]),
    core_only_spec("browser.tab.close", MethodCategory::Browser, &[]),
    core_only_spec("browser.frame.main", MethodCategory::Browser, &[]),
    core_only_spec("browser.frame.select", MethodCategory::Browser, &[]),
    core_only_spec("browser.dialog.accept", MethodCategory::Browser, &[]),
    core_only_spec("browser.dialog.dismiss", MethodCategory::Browser, &[]),
    core_only_spec("browser.download.wait", MethodCategory::Browser, &[]),
    core_only_spec("browser.state.save", MethodCategory::Browser, &[]),
    core_only_spec("browser.state.load", MethodCategory::Browser, &[]),
    core_only_spec("debug.app.activate", MethodCategory::Debug, &[]),
    core_only_spec("debug.bonsplit_underflow.count", MethodCategory::Debug, &[]),
    core_only_spec("debug.bonsplit_underflow.reset", MethodCategory::Debug, &[]),
    core_only_spec(
        "debug.command_palette.rename_input.delete_backward",
        MethodCategory::Debug,
        &[],
    ),
    core_only_spec(
        "debug.command_palette.rename_input.interact",
        MethodCategory::Debug,
        &[],
    ),
    core_only_spec(
        "debug.command_palette.rename_input.select_all",
        MethodCategory::Debug,
        &[],
    ),
    core_only_spec(
        "debug.command_palette.rename_input.selection",
        MethodCategory::Debug,
        &[],
    ),
    core_only_spec(
        "debug.command_palette.rename_tab.open",
        MethodCategory::Debug,
        &[],
    ),
    core_only_spec("debug.command_palette.results", MethodCategory::Debug, &[]),
    core_only_spec(
        "debug.command_palette.selection",
        MethodCategory::Debug,
        &[],
    ),
    core_only_spec("debug.command_palette.toggle", MethodCategory::Debug, &[]),
    core_only_spec("debug.command_palette.visible", MethodCategory::Debug, &[]),
    core_only_spec("debug.empty_panel.count", MethodCategory::Debug, &[]),
    core_only_spec("debug.empty_panel.reset", MethodCategory::Debug, &[]),
    core_only_spec("debug.flash.count", MethodCategory::Debug, &[]),
    core_only_spec("debug.flash.reset", MethodCategory::Debug, &[]),
    core_only_spec("debug.layout", MethodCategory::Debug, &[]),
    core_only_spec("debug.notification.focus", MethodCategory::Debug, &[]),
    core_only_spec("debug.panel_snapshot", MethodCategory::Debug, &[]),
    core_only_spec("debug.panel_snapshot.reset", MethodCategory::Debug, &[]),
    core_only_spec("debug.portal.stats", MethodCategory::Debug, &[]),
    core_only_spec("debug.shortcut.set", MethodCategory::Debug, &[]),
    core_only_spec("debug.shortcut.simulate", MethodCategory::Debug, &[]),
    core_only_spec("debug.sidebar.visible", MethodCategory::Debug, &[]),
    core_only_spec("debug.terminal.is_focused", MethodCategory::Debug, &[]),
    core_only_spec("debug.terminal.read_text", MethodCategory::Debug, &[]),
    core_only_spec("debug.terminal.render_stats", MethodCategory::Debug, &[]),
    core_only_spec("debug.type", MethodCategory::Debug, &[]),
    core_only_spec("debug.window.screenshot", MethodCategory::Debug, &[]),
];

pub fn resolve_wire_method_spec(method_or_wire_alias: &str) -> Option<&'static MethodSpec> {
    if method_or_wire_alias.is_empty() {
        return None;
    }
    ALL_METHOD_SPECS.iter().find(|spec| {
        spec.name == method_or_wire_alias || spec.wire_aliases.contains(&method_or_wire_alias)
    })
}

pub fn canonical_wire_method_name(method_or_wire_alias: &str) -> Option<&'static str> {
    resolve_wire_method_spec(method_or_wire_alias).map(|spec| spec.name)
}

pub fn resolve_cli_subcommand_spec(subcommand: &str) -> Option<&'static MethodSpec> {
    if subcommand.is_empty() {
        return None;
    }
    ALL_METHOD_SPECS
        .iter()
        .find(|spec| spec.cli_subcommands.contains(&subcommand))
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use super::{
        canonical_wire_method_name, resolve_cli_subcommand_spec, resolve_wire_method_spec,
        MethodCategory, ALL_METHOD_SPECS, CORE_COMMAND_NAMES, GTK_BRIDGE_COMMAND_NAMES,
        HOST_ONLY_COMMAND_NAMES,
    };

    #[test]
    fn method_specs_match_core_and_gtk_bridge_sets_without_duplicates() {
        let mut seen_names = HashSet::new();
        let mut seen_wire_tokens = HashMap::new();
        let mut seen_cli_subcommands = HashMap::new();

        for spec in &ALL_METHOD_SPECS {
            assert!(!spec.name.is_empty(), "method spec name must not be empty");
            assert!(
                spec.in_core || spec.in_gtk_bridge,
                "method {} must belong to core, GTK bridge, or both",
                spec.name
            );
            assert!(
                seen_names.insert(spec.name),
                "duplicate method name {}",
                spec.name
            );
            assert!(
                seen_wire_tokens.insert(spec.name, spec.name).is_none(),
                "method name {} collides with a wire alias",
                spec.name
            );

            for &alias in spec.wire_aliases {
                assert!(!alias.is_empty(), "empty wire alias on {}", spec.name);
                assert!(
                    spec.in_gtk_bridge,
                    "wire alias {alias} on {} requires GTK bridge support",
                    spec.name
                );
                assert!(
                    seen_wire_tokens.insert(alias, spec.name).is_none(),
                    "duplicate wire alias {alias} on {}",
                    spec.name
                );
            }

            for &subcommand in spec.cli_subcommands {
                assert!(
                    !subcommand.is_empty(),
                    "empty cli subcommand on {}",
                    spec.name
                );
                assert!(
                    seen_cli_subcommands.insert(subcommand, spec.name).is_none(),
                    "duplicate cli subcommand {subcommand} on {}",
                    spec.name
                );
            }
        }

        for (&subcommand, &cli_owner) in &seen_cli_subcommands {
            assert!(
                !seen_names.contains(subcommand),
                "cli subcommand {subcommand} must not equal a canonical RPC method name"
            );
            if let Some(&wire_owner) = seen_wire_tokens.get(subcommand) {
                assert_eq!(
                    cli_owner, wire_owner,
                    "token {subcommand} maps to {cli_owner} on CLI but {wire_owner} on wire"
                );
            }
        }

        let core_from_specs: Vec<&str> = ALL_METHOD_SPECS
            .iter()
            .filter(|spec| spec.in_core)
            .map(|spec| spec.name)
            .collect();
        assert_eq!(core_from_specs.as_slice(), &CORE_COMMAND_NAMES);

        let host_only_from_specs: Vec<&str> = ALL_METHOD_SPECS
            .iter()
            .filter(|spec| !spec.in_core)
            .map(|spec| spec.name)
            .collect();
        assert_eq!(host_only_from_specs.as_slice(), &HOST_ONLY_COMMAND_NAMES);

        let gtk_bridge_from_specs: Vec<&str> = ALL_METHOD_SPECS
            .iter()
            .filter(|spec| spec.in_gtk_bridge)
            .map(|spec| spec.name)
            .collect();
        assert_eq!(gtk_bridge_from_specs.as_slice(), &GTK_BRIDGE_COMMAND_NAMES);
    }

    #[test]
    fn wire_aliases_and_cli_subcommands_resolve_distinctly() {
        assert_eq!(canonical_wire_method_name("ping"), Some("system.ping"));
        assert_eq!(resolve_cli_subcommand_spec("ping"), None);

        assert_eq!(canonical_wire_method_name("identify"), None);
        assert_eq!(
            resolve_cli_subcommand_spec("identify").map(|spec| spec.name),
            Some("system.identify")
        );

        assert_eq!(canonical_wire_method_name("activate"), None);
        assert_eq!(
            resolve_cli_subcommand_spec("activate").map(|spec| spec.name),
            Some("window.activate")
        );

        assert_eq!(
            canonical_wire_method_name("workspace.activate"),
            Some("workspace.select")
        );
        assert_eq!(
            canonical_wire_method_name("activate-workspace"),
            Some("workspace.select")
        );
        assert_eq!(canonical_wire_method_name("select-workspace"), None);
        assert_eq!(
            resolve_cli_subcommand_spec("select-workspace").map(|spec| spec.name),
            Some("workspace.select")
        );

        assert_eq!(canonical_wire_method_name("next-window"), None);
        assert_eq!(
            resolve_cli_subcommand_spec("next-window").map(|spec| spec.name),
            Some("workspace.next")
        );
        assert_eq!(
            resolve_cli_subcommand_spec("previous-window").map(|spec| spec.name),
            Some("workspace.previous")
        );
        assert_eq!(
            resolve_cli_subcommand_spec("last-window").map(|spec| spec.name),
            Some("workspace.last")
        );

        assert_eq!(
            resolve_cli_subcommand_spec("collapse-group").map(|spec| spec.name),
            Some("workspace.group_set_collapsed")
        );
        assert_eq!(
            resolve_cli_subcommand_spec("expand-group").map(|spec| spec.name),
            Some("workspace.group_set_collapsed")
        );
        assert_eq!(
            resolve_cli_subcommand_spec("rename-window").map(|spec| spec.name),
            Some("workspace.rename")
        );
        assert_eq!(
            resolve_cli_subcommand_spec("rename-tab").map(|spec| spec.name),
            Some("tab.action")
        );

        assert_eq!(
            canonical_wire_method_name("capture-pane"),
            Some("surface.read_text")
        );
        assert_eq!(
            canonical_wire_method_name("send-text"),
            Some("surface.send_text")
        );
        assert_eq!(resolve_cli_subcommand_spec("send-text"), None);
        assert_eq!(
            resolve_cli_subcommand_spec("send").map(|spec| spec.name),
            Some("surface.send_text")
        );

        assert_eq!(
            resolve_wire_method_spec("list-groups").map(|spec| (
                spec.name,
                spec.category,
                spec.in_core,
                spec.in_gtk_bridge,
            )),
            Some((
                "workspace.group_list",
                MethodCategory::Workspace,
                false,
                true
            ))
        );

        for removed in [
            "capabilities",
            "list-windows",
            "new-window",
            "focus-window",
            "close-window",
            "next-workspace",
            "previous-workspace",
            "last-workspace",
            "swap-pane",
            "new-split",
            "browser.open-split",
        ] {
            assert_eq!(canonical_wire_method_name(removed), None, "{removed}");
            assert_eq!(resolve_cli_subcommand_spec(removed), None, "{removed}");
        }

        assert_eq!(canonical_wire_method_name(""), None);
        assert_eq!(resolve_cli_subcommand_spec(""), None);
        assert_eq!(canonical_wire_method_name("unknown.method"), None);
    }
}
