use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use gtk4::gdk;
use gtk4::gdk::prelude::DisplayExtManual;
use gtk4::glib::translate::IntoGlib;
use serde::Deserialize;
use serde_json::{Map, Value};

pub const CONFIG_DIR_NAME: &str = "limux";
pub const SHORTCUTS_FILE_NAME: &str = "shortcuts.json";
const LEGACY_CONFIG_FILE_NAME: &str = "config.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShortcutId {
    NewWorkspace,
    CloseWorkspace,
    QuitApp,
    NewInstance,
    ToggleSidebar,
    ToggleTopBar,
    ToggleFullscreen,
    NextWorkspace,
    PrevWorkspace,
    CycleTabPrev,
    CycleTabNext,
    SplitDown,
    NewTerminalInFocusedPane,
    SplitRight,
    CloseFocusedPane,
    CloseFocusedTab,
    ToggleFocusedPaneZoom,
    NewTerminal,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    ActivateWorkspace1,
    ActivateWorkspace2,
    ActivateWorkspace3,
    ActivateWorkspace4,
    ActivateWorkspace5,
    ActivateWorkspace6,
    ActivateWorkspace7,
    ActivateWorkspace8,
    ActivateLastWorkspace,
    RecentWorkspace,
    NextUnreadTerminal,
    PrevUnreadTerminal,
    OpenBrowserInSplit,
    BrowserFocusLocation,
    BrowserBack,
    BrowserForward,
    BrowserReload,
    BrowserInspector,
    BrowserConsole,
    SurfaceFind,
    SurfaceFindNext,
    SurfaceFindPrevious,
    SurfaceFindHide,
    SurfaceUseSelectionForFind,
    TerminalClearScrollback,
    TerminalCopy,
    TerminalPaste,
    TerminalIncreaseFontSize,
    TerminalDecreaseFontSize,
    TerminalResetFontSize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShortcutCommand {
    NewWorkspace,
    CloseWorkspace,
    QuitApp,
    NewInstance,
    ToggleSidebar,
    ToggleTopBar,
    ToggleFullscreen,
    NextWorkspace,
    PrevWorkspace,
    CycleTabPrev,
    CycleTabNext,
    SplitDown,
    NewTerminal,
    SplitRight,
    CloseFocusedPane,
    CloseFocusedTab,
    ToggleFocusedPaneZoom,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    ActivateWorkspace1,
    ActivateWorkspace2,
    ActivateWorkspace3,
    ActivateWorkspace4,
    ActivateWorkspace5,
    ActivateWorkspace6,
    ActivateWorkspace7,
    ActivateWorkspace8,
    ActivateLastWorkspace,
    RecentWorkspace,
    NextUnreadTerminal,
    PrevUnreadTerminal,
    OpenBrowserInSplit,
    BrowserFocusLocation,
    BrowserBack,
    BrowserForward,
    BrowserReload,
    BrowserInspector,
    BrowserConsole,
    SurfaceFind,
    SurfaceFindNext,
    SurfaceFindPrevious,
    SurfaceFindHide,
    SurfaceUseSelectionForFind,
    TerminalClearScrollback,
    TerminalCopy,
    TerminalPaste,
    TerminalIncreaseFontSize,
    TerminalDecreaseFontSize,
    TerminalResetFontSize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShortcutScope {
    AppGlobal,
    Window,
    FocusedTerminal,
    FocusedBrowser,
    FocusedSurface,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EditableCapturePolicy {
    AlwaysCapture,
    BypassInEditable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShortcutDefinition {
    pub id: ShortcutId,
    pub config_key: &'static str,
    pub action_name: &'static str,
    pub default_accel: &'static str,
    pub label: &'static str,
    pub registers_gtk_accel: bool,
    pub command: ShortcutCommand,
    pub scope: ShortcutScope,
    pub editable_capture_policy: EditableCapturePolicy,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NormalizedShortcut {
    key: String,
    ctrl: bool,
    shift: bool,
    alt: bool,
    cmd: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedShortcut {
    pub definition: &'static ShortcutDefinition,
    pub binding: Option<NormalizedShortcut>,
}

#[derive(Clone, Debug, Default)]
struct ShortcutLookupIndex {
    by_binding: HashMap<NormalizedShortcut, usize>,
    by_alternate: HashMap<NormalizedShortcut, usize>,
    unmodified_keyvals: Vec<(u32, usize)>,
    has_unmodified_fallback: bool,
    has_shift_only_binding: bool,
}

impl ShortcutLookupIndex {
    fn build(shortcuts: &[ResolvedShortcut]) -> Self {
        let mut index = Self {
            by_binding: HashMap::with_capacity(shortcuts.len()),
            by_alternate: HashMap::new(),
            unmodified_keyvals: Vec::new(),
            has_unmodified_fallback: false,
            has_shift_only_binding: false,
        };
        for (idx, shortcut) in shortcuts.iter().enumerate() {
            let Some(ref binding) = shortcut.binding else {
                continue;
            };
            index.by_binding.insert(binding.clone(), idx);
            let alternates = shortcut.definition.alternate_accels();
            if !alternates.is_empty() && *binding == shortcut.definition.default_binding() {
                index.by_alternate.extend(alternates.iter().map(|accel| {
                    let alternate = NormalizedShortcut::parse(accel)
                        .expect("alternate shortcuts should be valid");
                    (alternate, idx)
                }));
            }
            if !binding.ctrl && !binding.alt && !binding.cmd {
                if binding.shift {
                    index.has_shift_only_binding = true;
                } else if let Some(keyval) =
                    gdk::Key::from_name(runtime_key_to_gtk_key(&binding.key))
                        .filter(|key| is_direct_unmodified_keyval(*key))
                {
                    index
                        .unmodified_keyvals
                        .push((keyval.to_lower().into_glib(), idx));
                } else {
                    index.has_unmodified_fallback = true;
                }
            }
        }
        index
    }

    fn get(&self, binding: &NormalizedShortcut) -> Option<usize> {
        self.by_binding
            .get(binding)
            .or_else(|| self.by_alternate.get(binding))
            .copied()
    }
}

fn is_direct_unmodified_keyval(keyval: gdk::Key) -> bool {
    let raw = keyval.into_glib();
    // Standard X11/GDK function keys (F1..F35) and fixed non-shifted navigation/editing keys
    // that cannot be produced by level-shifting another key via AltGr/CapsLock.
    (gdk::Key::F1.into_glib()..=gdk::Key::F35.into_glib()).contains(&raw)
        || matches!(
            keyval,
            gdk::Key::Escape
                | gdk::Key::Return
                | gdk::Key::Tab
                | gdk::Key::Page_Up
                | gdk::Key::Page_Down
                | gdk::Key::Left
                | gdk::Key::Right
                | gdk::Key::Up
                | gdk::Key::Down
        )
}

#[derive(Clone, Debug)]
pub struct ResolvedShortcutConfig {
    pub shortcuts: Vec<ResolvedShortcut>,
    pub warnings: Vec<String>,
    index: ShortcutLookupIndex,
}

impl PartialEq for ResolvedShortcutConfig {
    fn eq(&self, other: &Self) -> bool {
        self.shortcuts == other.shortcuts && self.warnings == other.warnings
    }
}

impl Eq for ResolvedShortcutConfig {}

#[derive(Debug, PartialEq, Eq)]
pub enum ShortcutConfigError {
    InvalidBindingFormat {
        input: String,
    },
    MissingKey {
        input: String,
    },
    UnknownModifier {
        input: String,
        modifier: String,
    },
    InvalidBindingType {
        shortcut_id: String,
    },
    DuplicateBinding {
        first: ShortcutId,
        second: ShortcutId,
        accel: String,
    },
    BaseModifierRequired {
        shortcut_id: String,
        input: String,
    },
    ModifierOnlyBinding {
        shortcut_id: String,
        input: String,
    },
    InvalidJson(String),
}

impl std::fmt::Display for ShortcutConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBindingFormat { input } => {
                write!(f, "invalid shortcut format `{input}`")
            }
            Self::MissingKey { input } => {
                write!(f, "shortcut `{input}` is missing a key")
            }
            Self::UnknownModifier { input, modifier } => {
                write!(f, "shortcut `{input}` uses unknown modifier `{modifier}`")
            }
            Self::InvalidBindingType { shortcut_id } => {
                write!(
                    f,
                    "shortcut `{shortcut_id}` must be a string, empty string, or null"
                )
            }
            Self::DuplicateBinding {
                first,
                second,
                accel,
            } => {
                let first_label = definition_by_id(*first)
                    .map(|definition| definition.label)
                    .unwrap_or("another shortcut");
                let second_label = definition_by_id(*second)
                    .map(|definition| definition.label)
                    .unwrap_or("this shortcut");
                write!(
                    f,
                    "`{accel}` is already assigned to {first_label} and conflicts with {second_label}"
                )
            }
            Self::BaseModifierRequired { .. } => {
                write!(f, "use Ctrl, Alt, or Cmd with another key")
            }
            Self::ModifierOnlyBinding { .. } => {
                write!(f, "choose a non-modifier key with Ctrl, Alt, or Cmd")
            }
            Self::InvalidJson(reason) => write!(f, "invalid shortcut config JSON: {reason}"),
        }
    }
}

impl std::error::Error for ShortcutConfigError {}

#[derive(Debug)]
pub enum ShortcutConfigWriteError {
    InvalidExistingJson {
        path: PathBuf,
        reason: String,
    },
    InvalidExistingRoot {
        path: PathBuf,
    },
    CreateParentDir {
        path: PathBuf,
        reason: String,
    },
    WriteTempFile {
        path: PathBuf,
        reason: String,
    },
    PersistTempFile {
        from: PathBuf,
        to: PathBuf,
        reason: String,
    },
}

impl std::fmt::Display for ShortcutConfigWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidExistingJson { path, reason } => {
                write!(f, "invalid existing config `{}`: {reason}", path.display())
            }
            Self::InvalidExistingRoot { path } => {
                write!(
                    f,
                    "existing config `{}` is not a JSON object",
                    path.display()
                )
            }
            Self::CreateParentDir { path, reason } => {
                write!(
                    f,
                    "failed to create config directory `{}`: {reason}",
                    path.display()
                )
            }
            Self::WriteTempFile { path, reason } => {
                write!(
                    f,
                    "failed to write temp config `{}`: {reason}",
                    path.display()
                )
            }
            Self::PersistTempFile { from, to, reason } => write!(
                f,
                "failed to persist temp config `{}` -> `{}`: {reason}",
                from.display(),
                to.display()
            ),
        }
    }
}

impl std::error::Error for ShortcutConfigWriteError {}

#[derive(Debug, Default, Deserialize)]
struct ShortcutConfigFile {
    #[serde(default)]
    shortcuts: HashMap<String, serde_json::Value>,
}

const SHORTCUT_DEFINITIONS: [ShortcutDefinition; 52] = [
    ShortcutDefinition {
        id: ShortcutId::NewWorkspace,
        config_key: "new_workspace",
        action_name: "win.new-workspace",
        default_accel: "<Ctrl><Alt><Shift>n",
        label: "New Workspace",
        registers_gtk_accel: true,
        command: ShortcutCommand::NewWorkspace,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::CloseWorkspace,
        config_key: "close_workspace",
        action_name: "win.close-workspace",
        default_accel: "<Ctrl><Alt><Shift>w",
        label: "Close Workspace",
        registers_gtk_accel: true,
        command: ShortcutCommand::CloseWorkspace,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::QuitApp,
        config_key: "quit_app",
        action_name: "app.quit",
        default_accel: "<Ctrl><Alt>q",
        label: "Quit Limux",
        registers_gtk_accel: true,
        command: ShortcutCommand::QuitApp,
        scope: ShortcutScope::AppGlobal,
        editable_capture_policy: EditableCapturePolicy::AlwaysCapture,
    },
    ShortcutDefinition {
        id: ShortcutId::NewInstance,
        config_key: "new_instance",
        action_name: "app.new-instance",
        default_accel: "<Ctrl><Alt>n",
        label: "New Limux Instance",
        registers_gtk_accel: true,
        command: ShortcutCommand::NewInstance,
        scope: ShortcutScope::AppGlobal,
        editable_capture_policy: EditableCapturePolicy::AlwaysCapture,
    },
    ShortcutDefinition {
        id: ShortcutId::ToggleSidebar,
        config_key: "toggle_sidebar",
        action_name: "win.toggle-sidebar",
        default_accel: "<Ctrl><Alt>m",
        label: "Toggle Sidebar",
        registers_gtk_accel: true,
        command: ShortcutCommand::ToggleSidebar,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::ToggleTopBar,
        config_key: "toggle_top_bar",
        action_name: "win.toggle-top-bar",
        default_accel: "<Ctrl><Alt><Shift>m",
        label: "Toggle Top Bar",
        registers_gtk_accel: true,
        command: ShortcutCommand::ToggleTopBar,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::ToggleFullscreen,
        config_key: "toggle_fullscreen",
        action_name: "win.toggle-fullscreen",
        default_accel: "F11",
        label: "Toggle Fullscreen",
        registers_gtk_accel: true,
        command: ShortcutCommand::ToggleFullscreen,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::AlwaysCapture,
    },
    ShortcutDefinition {
        id: ShortcutId::NextWorkspace,
        config_key: "next_workspace",
        action_name: "win.next-workspace",
        default_accel: "<Ctrl><Alt>Page_Down",
        label: "Next Workspace",
        registers_gtk_accel: true,
        command: ShortcutCommand::NextWorkspace,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::PrevWorkspace,
        config_key: "prev_workspace",
        action_name: "win.prev-workspace",
        default_accel: "<Ctrl><Alt>Page_Up",
        label: "Previous Workspace",
        registers_gtk_accel: true,
        command: ShortcutCommand::PrevWorkspace,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::CycleTabPrev,
        config_key: "cycle_tab_prev",
        action_name: "win.cycle-tab-prev",
        default_accel: "<Ctrl><Alt><Shift>Left",
        label: "Previous Tab",
        registers_gtk_accel: false,
        command: ShortcutCommand::CycleTabPrev,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::CycleTabNext,
        config_key: "cycle_tab_next",
        action_name: "win.cycle-tab-next",
        default_accel: "<Ctrl><Alt><Shift>Right",
        label: "Next Tab",
        registers_gtk_accel: false,
        command: ShortcutCommand::CycleTabNext,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::SplitDown,
        config_key: "split_down",
        action_name: "win.split-down",
        default_accel: "<Ctrl><Alt><Shift>d",
        label: "Split Down",
        registers_gtk_accel: false,
        command: ShortcutCommand::SplitDown,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::NewTerminalInFocusedPane,
        config_key: "new_terminal_in_focused_pane",
        action_name: "win.new-terminal-in-focused-pane",
        default_accel: "<Ctrl><Shift>t",
        label: "New Terminal In Focused Pane",
        registers_gtk_accel: false,
        command: ShortcutCommand::NewTerminal,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::SplitRight,
        config_key: "split_right",
        action_name: "win.split-right",
        default_accel: "<Ctrl><Alt>d",
        label: "Split Right",
        registers_gtk_accel: false,
        command: ShortcutCommand::SplitRight,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::CloseFocusedPane,
        config_key: "close_focused_pane",
        action_name: "win.close-focused-pane",
        default_accel: "<Ctrl><Alt>w",
        label: "Close Focused Pane",
        registers_gtk_accel: false,
        command: ShortcutCommand::CloseFocusedPane,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::CloseFocusedTab,
        config_key: "close_focused_tab",
        action_name: "win.close-focused-tab",
        default_accel: "<Ctrl><Shift>w",
        label: "Close Focused Tab",
        registers_gtk_accel: false,
        command: ShortcutCommand::CloseFocusedTab,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::ToggleFocusedPaneZoom,
        config_key: "toggle_focused_pane_zoom",
        action_name: "win.toggle-focused-pane-zoom",
        default_accel: "<Ctrl><Shift>z",
        label: "Toggle Focused Pane Zoom",
        registers_gtk_accel: false,
        command: ShortcutCommand::ToggleFocusedPaneZoom,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::NewTerminal,
        config_key: "new_terminal",
        action_name: "win.new-terminal",
        default_accel: "<Ctrl><Alt>t",
        label: "New Terminal",
        registers_gtk_accel: false,
        command: ShortcutCommand::NewTerminal,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::FocusLeft,
        config_key: "focus_left",
        action_name: "win.focus-left",
        default_accel: "<Ctrl><Alt>Left",
        label: "Focus Left",
        registers_gtk_accel: false,
        command: ShortcutCommand::FocusLeft,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::FocusRight,
        config_key: "focus_right",
        action_name: "win.focus-right",
        default_accel: "<Ctrl><Alt>Right",
        label: "Focus Right",
        registers_gtk_accel: false,
        command: ShortcutCommand::FocusRight,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::FocusUp,
        config_key: "focus_up",
        action_name: "win.focus-up",
        default_accel: "<Ctrl><Alt>Up",
        label: "Focus Up",
        registers_gtk_accel: false,
        command: ShortcutCommand::FocusUp,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::FocusDown,
        config_key: "focus_down",
        action_name: "win.focus-down",
        default_accel: "<Ctrl><Alt>Down",
        label: "Focus Down",
        registers_gtk_accel: false,
        command: ShortcutCommand::FocusDown,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::ActivateWorkspace1,
        config_key: "activate_workspace_1",
        action_name: "win.activate-workspace-1",
        default_accel: "<Ctrl><Alt>1",
        label: "Activate Workspace 1",
        registers_gtk_accel: false,
        command: ShortcutCommand::ActivateWorkspace1,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::ActivateWorkspace2,
        config_key: "activate_workspace_2",
        action_name: "win.activate-workspace-2",
        default_accel: "<Ctrl><Alt>2",
        label: "Activate Workspace 2",
        registers_gtk_accel: false,
        command: ShortcutCommand::ActivateWorkspace2,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::ActivateWorkspace3,
        config_key: "activate_workspace_3",
        action_name: "win.activate-workspace-3",
        default_accel: "<Ctrl><Alt>3",
        label: "Activate Workspace 3",
        registers_gtk_accel: false,
        command: ShortcutCommand::ActivateWorkspace3,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::ActivateWorkspace4,
        config_key: "activate_workspace_4",
        action_name: "win.activate-workspace-4",
        default_accel: "<Ctrl><Alt>4",
        label: "Activate Workspace 4",
        registers_gtk_accel: false,
        command: ShortcutCommand::ActivateWorkspace4,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::ActivateWorkspace5,
        config_key: "activate_workspace_5",
        action_name: "win.activate-workspace-5",
        default_accel: "<Ctrl><Alt>5",
        label: "Activate Workspace 5",
        registers_gtk_accel: false,
        command: ShortcutCommand::ActivateWorkspace5,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::ActivateWorkspace6,
        config_key: "activate_workspace_6",
        action_name: "win.activate-workspace-6",
        default_accel: "<Ctrl><Alt>6",
        label: "Activate Workspace 6",
        registers_gtk_accel: false,
        command: ShortcutCommand::ActivateWorkspace6,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::ActivateWorkspace7,
        config_key: "activate_workspace_7",
        action_name: "win.activate-workspace-7",
        default_accel: "<Ctrl><Alt>7",
        label: "Activate Workspace 7",
        registers_gtk_accel: false,
        command: ShortcutCommand::ActivateWorkspace7,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::ActivateWorkspace8,
        config_key: "activate_workspace_8",
        action_name: "win.activate-workspace-8",
        default_accel: "<Ctrl><Alt>8",
        label: "Activate Workspace 8",
        registers_gtk_accel: false,
        command: ShortcutCommand::ActivateWorkspace8,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::ActivateLastWorkspace,
        config_key: "activate_last_workspace",
        action_name: "win.activate-last-workspace",
        default_accel: "<Ctrl><Alt>9",
        label: "Activate Last Workspace",
        registers_gtk_accel: false,
        command: ShortcutCommand::ActivateLastWorkspace,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::RecentWorkspace,
        config_key: "recent_workspace",
        action_name: "win.recent-workspace",
        default_accel: "<Ctrl><Alt>o",
        label: "Switch to Most Recent Workspace",
        registers_gtk_accel: false,
        command: ShortcutCommand::RecentWorkspace,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::NextUnreadTerminal,
        config_key: "next_unread_terminal",
        action_name: "win.next-unread-terminal",
        default_accel: "<Ctrl><Alt>u",
        label: "Next Unread Terminal",
        registers_gtk_accel: false,
        command: ShortcutCommand::NextUnreadTerminal,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::PrevUnreadTerminal,
        config_key: "prev_unread_terminal",
        action_name: "win.prev-unread-terminal",
        default_accel: "<Ctrl><Alt><Shift>u",
        label: "Previous Unread Terminal",
        registers_gtk_accel: false,
        command: ShortcutCommand::PrevUnreadTerminal,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::OpenBrowserInSplit,
        config_key: "open_browser_in_split",
        action_name: "win.open-browser-in-split",
        default_accel: "<Ctrl><Shift>l",
        label: "Open Browser In Split",
        registers_gtk_accel: false,
        command: ShortcutCommand::OpenBrowserInSplit,
        scope: ShortcutScope::FocusedBrowser,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::BrowserFocusLocation,
        config_key: "browser_focus_location",
        action_name: "win.browser-focus-location",
        default_accel: "<Ctrl>l",
        label: "Browser Focus Location",
        registers_gtk_accel: false,
        command: ShortcutCommand::BrowserFocusLocation,
        scope: ShortcutScope::FocusedBrowser,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::BrowserBack,
        config_key: "browser_back",
        action_name: "win.browser-back",
        default_accel: "<Ctrl>bracketleft",
        label: "Browser Back",
        registers_gtk_accel: false,
        command: ShortcutCommand::BrowserBack,
        scope: ShortcutScope::FocusedBrowser,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::BrowserForward,
        config_key: "browser_forward",
        action_name: "win.browser-forward",
        default_accel: "<Ctrl>bracketright",
        label: "Browser Forward",
        registers_gtk_accel: false,
        command: ShortcutCommand::BrowserForward,
        scope: ShortcutScope::FocusedBrowser,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::BrowserReload,
        config_key: "browser_reload",
        action_name: "win.browser-reload",
        default_accel: "<Ctrl>r",
        label: "Browser Reload",
        registers_gtk_accel: false,
        command: ShortcutCommand::BrowserReload,
        scope: ShortcutScope::FocusedBrowser,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::BrowserInspector,
        config_key: "browser_inspector",
        action_name: "win.browser-inspector",
        default_accel: "<Ctrl><Alt>i",
        label: "Browser Inspector",
        registers_gtk_accel: false,
        command: ShortcutCommand::BrowserInspector,
        scope: ShortcutScope::FocusedBrowser,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::BrowserConsole,
        config_key: "browser_console",
        action_name: "win.browser-console",
        default_accel: "<Ctrl><Alt>c",
        label: "Browser JavaScript Console",
        registers_gtk_accel: false,
        command: ShortcutCommand::BrowserConsole,
        scope: ShortcutScope::FocusedBrowser,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::SurfaceFind,
        config_key: "surface_find",
        action_name: "win.surface-find",
        default_accel: "<Ctrl><Alt>f",
        label: "Find",
        registers_gtk_accel: false,
        command: ShortcutCommand::SurfaceFind,
        scope: ShortcutScope::FocusedSurface,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::SurfaceFindNext,
        config_key: "surface_find_next",
        action_name: "win.surface-find-next",
        default_accel: "<Ctrl><Alt>g",
        label: "Find Next",
        registers_gtk_accel: false,
        command: ShortcutCommand::SurfaceFindNext,
        scope: ShortcutScope::FocusedSurface,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::SurfaceFindPrevious,
        config_key: "surface_find_previous",
        action_name: "win.surface-find-previous",
        default_accel: "<Ctrl><Alt><Shift>g",
        label: "Find Previous",
        registers_gtk_accel: false,
        command: ShortcutCommand::SurfaceFindPrevious,
        scope: ShortcutScope::FocusedSurface,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::SurfaceFindHide,
        config_key: "surface_find_hide",
        action_name: "win.surface-find-hide",
        default_accel: "<Ctrl><Alt><Shift>f",
        label: "Hide Find",
        registers_gtk_accel: false,
        command: ShortcutCommand::SurfaceFindHide,
        scope: ShortcutScope::FocusedSurface,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::SurfaceUseSelectionForFind,
        config_key: "surface_use_selection_for_find",
        action_name: "win.surface-use-selection-for-find",
        default_accel: "<Ctrl><Alt>e",
        label: "Use Selection For Find",
        registers_gtk_accel: false,
        command: ShortcutCommand::SurfaceUseSelectionForFind,
        scope: ShortcutScope::FocusedSurface,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::TerminalClearScrollback,
        config_key: "terminal_clear_scrollback",
        action_name: "win.terminal-clear-scrollback",
        default_accel: "<Ctrl><Alt>k",
        label: "Terminal Clear Scrollback",
        registers_gtk_accel: false,
        command: ShortcutCommand::TerminalClearScrollback,
        scope: ShortcutScope::FocusedTerminal,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::TerminalCopy,
        config_key: "terminal_copy",
        action_name: "win.terminal-copy",
        default_accel: "<Ctrl><Shift>c",
        label: "Terminal Copy",
        registers_gtk_accel: false,
        command: ShortcutCommand::TerminalCopy,
        scope: ShortcutScope::FocusedTerminal,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::TerminalPaste,
        config_key: "terminal_paste",
        action_name: "win.terminal-paste",
        default_accel: "<Ctrl><Shift>v",
        label: "Terminal Paste",
        registers_gtk_accel: false,
        command: ShortcutCommand::TerminalPaste,
        scope: ShortcutScope::FocusedTerminal,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::TerminalIncreaseFontSize,
        config_key: "terminal_increase_font_size",
        action_name: "win.terminal-increase-font-size",
        default_accel: "<Ctrl><Alt>equal",
        label: "Terminal Increase Font Size",
        registers_gtk_accel: false,
        command: ShortcutCommand::TerminalIncreaseFontSize,
        scope: ShortcutScope::FocusedTerminal,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::TerminalDecreaseFontSize,
        config_key: "terminal_decrease_font_size",
        action_name: "win.terminal-decrease-font-size",
        default_accel: "<Ctrl><Alt>minus",
        label: "Terminal Decrease Font Size",
        registers_gtk_accel: false,
        command: ShortcutCommand::TerminalDecreaseFontSize,
        scope: ShortcutScope::FocusedTerminal,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::TerminalResetFontSize,
        config_key: "terminal_reset_font_size",
        action_name: "win.terminal-reset-font-size",
        default_accel: "<Ctrl><Alt><Shift>0",
        label: "Terminal Reset Font Size",
        registers_gtk_accel: false,
        command: ShortcutCommand::TerminalResetFontSize,
        scope: ShortcutScope::FocusedTerminal,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
];

impl NormalizedShortcut {
    #[cfg(test)]
    pub fn from_gdk_key(keyval: gdk::Key, modifier: gdk::ModifierType) -> Option<Self> {
        Self::from_gdk_key_event(None, keyval, 0, modifier)
    }

    pub fn from_gdk_key_event(
        display: Option<&gdk::Display>,
        keyval: gdk::Key,
        keycode: u32,
        modifier: gdk::ModifierType,
    ) -> Option<Self> {
        let key = normalized_event_key(display, keyval, keycode)?;
        if is_modifier_only_key(&key) {
            return None;
        }

        Some(Self {
            key,
            ctrl: modifier.contains(gdk::ModifierType::CONTROL_MASK),
            shift: modifier.contains(gdk::ModifierType::SHIFT_MASK),
            alt: modifier.contains(gdk::ModifierType::ALT_MASK),
            cmd: modifier.intersects(gdk::ModifierType::META_MASK | gdk::ModifierType::SUPER_MASK),
        })
    }

    pub fn parse(input: &str) -> Result<Self, ShortcutConfigError> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(ShortcutConfigError::MissingKey {
                input: input.to_string(),
            });
        }

        let mut rest = trimmed;
        let mut ctrl = false;
        let mut shift = false;
        let mut alt = false;
        let mut cmd = false;

        while let Some(stripped) = rest.strip_prefix('<') {
            let Some(end) = stripped.find('>') else {
                return Err(ShortcutConfigError::InvalidBindingFormat {
                    input: input.to_string(),
                });
            };
            let modifier = stripped[..end].trim().to_ascii_lowercase();
            match modifier.as_str() {
                "ctrl" | "control" => ctrl = true,
                "shift" => shift = true,
                "alt" | "option" => alt = true,
                "meta" | "super" | "cmd" | "command" => cmd = true,
                _ => {
                    return Err(ShortcutConfigError::UnknownModifier {
                        input: input.to_string(),
                        modifier,
                    });
                }
            }
            rest = stripped[end + 1..].trim_start();
        }

        if rest.is_empty() {
            return Err(ShortcutConfigError::MissingKey {
                input: input.to_string(),
            });
        }

        if rest.contains('<') || rest.contains('>') {
            return Err(ShortcutConfigError::InvalidBindingFormat {
                input: input.to_string(),
            });
        }

        Ok(Self {
            key: normalize_runtime_key(rest),
            ctrl,
            shift,
            alt,
            cmd,
        })
    }

    pub fn validate_host_binding(
        &self,
        definition: &ShortcutDefinition,
    ) -> Result<(), ShortcutConfigError> {
        if is_modifier_only_key(&self.key) {
            return Err(ShortcutConfigError::ModifierOnlyBinding {
                shortcut_id: definition.config_key.to_string(),
                input: self.to_config_accel(),
            });
        }
        if definition.requires_base_modifier() && !self.ctrl && !self.alt && !self.cmd {
            return Err(ShortcutConfigError::BaseModifierRequired {
                shortcut_id: definition.config_key.to_string(),
                input: self.to_config_accel(),
            });
        }
        Ok(())
    }

    pub fn to_config_accel(&self) -> String {
        let mut accel = String::new();
        if self.ctrl {
            accel.push_str("<Ctrl>");
        }
        if self.alt {
            accel.push_str("<Alt>");
        }
        if self.shift {
            accel.push_str("<Shift>");
        }
        if self.cmd {
            accel.push_str("<Cmd>");
        }
        accel.push_str(&runtime_key_to_gtk_key(&self.key));
        accel
    }

    pub fn gtk_accel_variants(&self) -> Vec<String> {
        let mut variants = Vec::new();
        for command_prefix in self.command_prefixes() {
            let mut accel = String::new();
            if self.ctrl {
                accel.push_str("<Ctrl>");
            }
            if self.alt {
                accel.push_str("<Alt>");
            }
            accel.push_str(command_prefix);
            if self.shift {
                accel.push_str("<Shift>");
            }
            accel.push_str(&runtime_key_to_gtk_key(&self.key));
            variants.push(accel);
        }
        variants
    }

    fn command_prefixes(&self) -> &'static [&'static str] {
        if self.cmd {
            &["<Meta>", "<Super>"]
        } else {
            &[""]
        }
    }

    pub fn to_runtime_combo(&self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("ctrl");
        }
        if self.alt {
            parts.push("alt");
        }
        if self.shift {
            parts.push("shift");
        }
        if self.cmd {
            parts.push("cmd");
        }
        parts.push(self.key.as_str());
        parts.join("+")
    }

    pub fn to_display_label(&self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.alt {
            parts.push("Alt".to_string());
        }
        if self.shift {
            parts.push("Shift".to_string());
        }
        if self.cmd {
            parts.push("Cmd".to_string());
        }
        parts.push(display_key_label(&self.key));
        parts.join("+")
    }
}

impl ResolvedShortcut {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn gtk_accel(&self) -> Option<String> {
        self.binding
            .as_ref()
            .map(NormalizedShortcut::to_config_accel)
    }

    pub fn gtk_accel_variants(&self) -> Vec<String> {
        self.binding
            .as_ref()
            .map(NormalizedShortcut::gtk_accel_variants)
            .unwrap_or_default()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn runtime_combo(&self) -> Option<String> {
        self.binding
            .as_ref()
            .map(NormalizedShortcut::to_runtime_combo)
    }

    pub fn display_label(&self) -> Option<String> {
        self.binding
            .as_ref()
            .map(NormalizedShortcut::to_display_label)
    }

    pub fn default_display_label(&self) -> String {
        self.definition.default_display_label()
    }
}

impl ResolvedShortcutConfig {
    fn from_parts(shortcuts: Vec<ResolvedShortcut>, warnings: Vec<String>) -> Self {
        let index = ShortcutLookupIndex::build(&shortcuts);
        Self {
            shortcuts,
            warnings,
            index,
        }
    }

    fn rebuild_index(&mut self) {
        self.index = ShortcutLookupIndex::build(&self.shortcuts);
    }

    pub fn gtk_accel_entries(&self) -> Vec<(&'static str, Vec<String>)> {
        self.shortcuts
            .iter()
            .filter(|shortcut| shortcut.definition.registers_gtk_accel)
            .map(|shortcut| {
                let accels = shortcut.gtk_accel_variants();
                (shortcut.definition.action_name, accels)
            })
            .collect()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn command_for_runtime_combo(&self, combo: &str) -> Option<ShortcutCommand> {
        self.shortcuts
            .iter()
            .find(|shortcut| shortcut.runtime_combo().as_deref() == Some(combo))
            .map(|shortcut| shortcut.definition.command)
    }

    pub fn display_label_for_id(&self, id: ShortcutId) -> Option<String> {
        self.find_by_id(id)
            .and_then(ResolvedShortcut::display_label)
    }

    pub fn default_display_label_for_id(&self, id: ShortcutId) -> Option<String> {
        self.find_by_id(id)
            .map(ResolvedShortcut::default_display_label)
    }

    pub fn tooltip_text(&self, id: ShortcutId, base: &str) -> String {
        self.display_label_for_id(id)
            .map(|label| format!("{base} ({label})"))
            .unwrap_or_else(|| base.to_string())
    }

    pub fn find_by_id(&self, id: ShortcutId) -> Option<&ResolvedShortcut> {
        self.shortcuts
            .iter()
            .find(|shortcut| shortcut.definition.id == id)
    }

    #[cfg(test)]
    pub fn find_for_key_press(
        &self,
        display: Option<&gdk::Display>,
        keyval: gdk::Key,
        keycode: u32,
        modifier: gdk::ModifierType,
    ) -> Option<&ResolvedShortcut> {
        self.find_for_key_press_with_display(|| display.cloned(), keyval, keycode, modifier)
    }

    pub fn find_for_key_press_with_display(
        &self,
        display: impl FnOnce() -> Option<gdk::Display>,
        keyval: gdk::Key,
        keycode: u32,
        modifier: gdk::ModifierType,
    ) -> Option<&ResolvedShortcut> {
        let has_base_modifier = modifier.intersects(
            gdk::ModifierType::CONTROL_MASK
                | gdk::ModifierType::ALT_MASK
                | gdk::ModifierType::META_MASK
                | gdk::ModifierType::SUPER_MASK,
        );
        if !has_base_modifier {
            let has_shift = modifier.contains(gdk::ModifierType::SHIFT_MASK);
            if !has_shift && !self.index.has_unmodified_fallback {
                let raw = keyval.to_lower().into_glib();
                return self
                    .index
                    .unmodified_keyvals
                    .iter()
                    .find(|(candidate, _)| *candidate == raw)
                    .and_then(|(_, idx)| self.shortcuts.get(*idx));
            }
            if has_shift && !self.index.has_shift_only_binding {
                return None;
            }
        }
        let resolved_display = display();
        let shortcut = NormalizedShortcut::from_gdk_key_event(
            resolved_display.as_ref(),
            keyval,
            keycode,
            modifier,
        )?;
        self.index
            .get(&shortcut)
            .or_else(|| {
                if !has_base_modifier {
                    return None;
                }
                fallback_shortcuts(resolved_display.as_ref()?, keyval, keycode, &shortcut)
                    .into_iter()
                    .find_map(|fallback| self.index.get(&fallback))
            })
            .and_then(|idx| self.shortcuts.get(idx))
    }

    pub fn override_bindings_json(&self) -> Map<String, Value> {
        self.shortcuts
            .iter()
            .filter_map(|shortcut| {
                let default_binding = shortcut.definition.default_binding();
                match &shortcut.binding {
                    Some(binding) if binding == &default_binding => None,
                    Some(binding) => Some((
                        shortcut.definition.config_key.to_string(),
                        Value::String(binding.to_config_accel()),
                    )),
                    None => Some((shortcut.definition.config_key.to_string(), Value::Null)),
                }
            })
            .collect()
    }

    pub fn with_binding(
        &self,
        id: ShortcutId,
        binding: Option<NormalizedShortcut>,
    ) -> Result<Self, ShortcutConfigError> {
        let mut updated = self.clone();
        if let Some(shortcut) = updated
            .shortcuts
            .iter_mut()
            .find(|shortcut| shortcut.definition.id == id)
        {
            shortcut.binding = binding;
        }
        ensure_valid_active_bindings(&updated.shortcuts)?;
        updated.rebuild_index();
        Ok(updated)
    }
}

pub fn definitions() -> &'static [ShortcutDefinition] {
    &SHORTCUT_DEFINITIONS
}

impl ShortcutDefinition {
    pub fn requires_base_modifier(&self) -> bool {
        !matches!(self.id, ShortcutId::ToggleFullscreen)
    }

    /// Other combos that run the action while it has its default binding.
    /// `+` shares a key with `=` on some layouts and has its own on others.
    fn alternate_accels(&self) -> &'static [&'static str] {
        match self.id {
            ShortcutId::TerminalIncreaseFontSize => &["<Ctrl><Alt>plus", "<Ctrl><Alt>KP_Add"],
            ShortcutId::TerminalDecreaseFontSize => &["<Ctrl><Alt>KP_Subtract"],
            _ => &[],
        }
    }

    pub fn default_binding(&self) -> NormalizedShortcut {
        NormalizedShortcut::parse(self.default_accel).expect("default shortcuts should be valid")
    }

    pub fn default_display_label(&self) -> String {
        self.default_binding().to_display_label()
    }

    pub fn action_basename(&self) -> &'static str {
        self.action_name
            .split_once('.')
            .map(|(_, name)| name)
            .unwrap_or(self.action_name)
    }
}

pub fn config_dir_path() -> Option<PathBuf> {
    dirs::config_dir().map(|base| config_dir_path_in(&base))
}

pub fn config_dir_path_in(base: &Path) -> PathBuf {
    base.join(CONFIG_DIR_NAME)
}

pub fn shortcuts_path() -> Option<PathBuf> {
    config_dir_path().map(|dir| dir.join(SHORTCUTS_FILE_NAME))
}

#[cfg(test)]
pub fn shortcuts_path_in(base: &Path) -> PathBuf {
    config_dir_path_in(base).join(SHORTCUTS_FILE_NAME)
}

fn legacy_config_path_for(shortcuts_path: &Path) -> Option<PathBuf> {
    shortcuts_path
        .parent()
        .map(|parent| parent.join(LEGACY_CONFIG_FILE_NAME))
}

pub fn default_shortcuts() -> ResolvedShortcutConfig {
    let shortcuts = definitions()
        .iter()
        .map(|definition| ResolvedShortcut {
            definition,
            binding: Some(definition.default_binding()),
        })
        .collect();
    ResolvedShortcutConfig::from_parts(shortcuts, Vec::new())
}

#[cfg(test)]
pub fn resolve_shortcuts_from_str(
    raw: &str,
) -> Result<ResolvedShortcutConfig, ShortcutConfigError> {
    resolve_shortcuts_from_str_with_display(raw, None)
}

pub fn resolve_shortcuts_from_str_with_display(
    raw: &str,
    display: Option<&gdk::Display>,
) -> Result<ResolvedShortcutConfig, ShortcutConfigError> {
    let parsed: ShortcutConfigFile = serde_json::from_str(raw)
        .map_err(|err| ShortcutConfigError::InvalidJson(err.to_string()))?;
    resolve_shortcuts_from_file(parsed, display)
}

#[cfg(test)]
pub fn load_shortcuts_or_default(path: &Path) -> ResolvedShortcutConfig {
    load_shortcuts_or_default_with_display(path, None)
}

pub fn load_shortcuts_or_default_with_display(
    path: &Path,
    display: Option<&gdk::Display>,
) -> ResolvedShortcutConfig {
    if !path.exists() {
        return load_legacy_shortcuts_or_default(path, display);
    }

    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) => {
            let mut defaults = default_shortcuts();
            defaults.warnings.push(format!(
                "failed to read shortcut config `{}`: {err}",
                path.display()
            ));
            return defaults;
        }
    };

    match resolve_shortcuts_from_str_with_display(&raw, display) {
        Ok(config) => config,
        Err(err) => {
            let mut defaults = default_shortcuts();
            defaults.warnings.push(format!(
                "failed to load shortcut config `{}`: {err:?}",
                path.display()
            ));
            defaults
        }
    }
}

fn load_legacy_shortcuts_or_default(
    canonical_path: &Path,
    display: Option<&gdk::Display>,
) -> ResolvedShortcutConfig {
    let Some(legacy_path) = legacy_config_path_for(canonical_path) else {
        return default_shortcuts();
    };
    if !legacy_path.exists() {
        return default_shortcuts();
    }

    let raw = match fs::read_to_string(&legacy_path) {
        Ok(raw) => raw,
        Err(err) => {
            let mut defaults = default_shortcuts();
            defaults.warnings.push(format!(
                "failed to read legacy shortcut config `{}`: {err}",
                legacy_path.display()
            ));
            return defaults;
        }
    };

    let Some(parsed) = (match parse_legacy_shortcut_config(&raw) {
        Ok(parsed) => parsed,
        Err(err) => {
            let mut defaults = default_shortcuts();
            defaults.warnings.push(format!(
                "failed to load legacy shortcut config `{}`: {err:?}",
                legacy_path.display()
            ));
            return defaults;
        }
    }) else {
        return default_shortcuts();
    };

    match resolve_shortcuts_from_file(parsed, display) {
        Ok(mut config) => {
            if let Err(err) = write_shortcuts(canonical_path, &config) {
                config.warnings.push(format!(
                    "loaded legacy shortcut config `{}` but failed to migrate to `{}`: {err}",
                    legacy_path.display(),
                    canonical_path.display()
                ));
            }
            config
        }
        Err(err) => {
            let mut defaults = default_shortcuts();
            defaults.warnings.push(format!(
                "failed to load legacy shortcut config `{}`: {err:?}",
                legacy_path.display()
            ));
            defaults
        }
    }
}

fn parse_legacy_shortcut_config(
    raw: &str,
) -> Result<Option<ShortcutConfigFile>, ShortcutConfigError> {
    let root: Value = serde_json::from_str(raw)
        .map_err(|err| ShortcutConfigError::InvalidJson(err.to_string()))?;
    let Value::Object(root) = root else {
        return Err(ShortcutConfigError::InvalidJson(
            "legacy shortcut config root must be a JSON object".to_string(),
        ));
    };

    let Some(shortcuts) = root.get("shortcuts") else {
        return Ok(None);
    };

    if !shortcuts.is_object() {
        return Err(ShortcutConfigError::InvalidJson(
            "legacy `shortcuts` must be a JSON object".to_string(),
        ));
    }

    let mut legacy_root = Map::new();
    legacy_root.insert("shortcuts".to_string(), shortcuts.clone());

    serde_json::from_value(Value::Object(legacy_root))
        .map(Some)
        .map_err(|err| ShortcutConfigError::InvalidJson(err.to_string()))
}

pub fn load_shortcuts_for_display(display: &gdk::Display) -> ResolvedShortcutConfig {
    let Some(path) = shortcuts_path() else {
        let mut defaults = default_shortcuts();
        defaults
            .warnings
            .push("config_dir unavailable; using default shortcuts".to_string());
        return defaults;
    };
    load_shortcuts_or_default_with_display(&path, Some(display))
}

pub fn write_shortcuts(
    path: &Path,
    shortcuts: &ResolvedShortcutConfig,
) -> Result<(), ShortcutConfigWriteError> {
    let mut root = read_existing_config_root(path)?;
    let overrides = shortcuts.override_bindings_json();
    if overrides.is_empty() {
        root.remove("shortcuts");
    } else {
        root.insert("shortcuts".to_string(), Value::Object(overrides));
    }
    write_config_root_atomically(path, root)
}

fn resolve_shortcuts_from_file(
    parsed: ShortcutConfigFile,
    display: Option<&gdk::Display>,
) -> Result<ResolvedShortcutConfig, ShortcutConfigError> {
    let mut resolved = default_shortcuts();

    for (shortcut_id, value) in parsed.shortcuts {
        let Some(definition) = definition_by_config_key(&shortcut_id) else {
            resolved
                .warnings
                .push(format!("ignoring unknown shortcut id `{shortcut_id}`"));
            continue;
        };

        let binding = match value {
            serde_json::Value::Null => None,
            serde_json::Value::String(value) => {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(canonicalize_loaded_binding(
                        display,
                        NormalizedShortcut::parse(trimmed)?,
                    ))
                }
            }
            _ => {
                return Err(ShortcutConfigError::InvalidBindingType {
                    shortcut_id: shortcut_id.clone(),
                });
            }
        };

        if let Some(slot) = resolved
            .shortcuts
            .iter_mut()
            .find(|shortcut| shortcut.definition.id == definition.id)
        {
            slot.binding = binding;
        }
    }

    ensure_valid_active_bindings(&resolved.shortcuts)?;
    resolved.rebuild_index();
    Ok(resolved)
}

fn ensure_valid_active_bindings(shortcuts: &[ResolvedShortcut]) -> Result<(), ShortcutConfigError> {
    let mut active: HashMap<NormalizedShortcut, ShortcutId> = HashMap::new();
    for shortcut in shortcuts {
        let Some(binding) = shortcut.binding.clone() else {
            continue;
        };
        binding.validate_host_binding(shortcut.definition)?;
        if let Some(existing) = active.insert(binding.clone(), shortcut.definition.id) {
            return Err(ShortcutConfigError::DuplicateBinding {
                first: existing,
                second: shortcut.definition.id,
                accel: binding.to_config_accel(),
            });
        }
    }
    Ok(())
}

fn read_existing_config_root(path: &Path) -> Result<Map<String, Value>, ShortcutConfigWriteError> {
    if !path.exists() {
        return Ok(Map::new());
    }

    let raw =
        fs::read_to_string(path).map_err(|err| ShortcutConfigWriteError::InvalidExistingJson {
            path: path.to_path_buf(),
            reason: err.to_string(),
        })?;

    let root: Value = serde_json::from_str(&raw).map_err(|err| {
        ShortcutConfigWriteError::InvalidExistingJson {
            path: path.to_path_buf(),
            reason: err.to_string(),
        }
    })?;

    match root {
        Value::Object(map) => Ok(map),
        _ => Err(ShortcutConfigWriteError::InvalidExistingRoot {
            path: path.to_path_buf(),
        }),
    }
}

fn write_config_root_atomically(
    path: &Path,
    root: Map<String, Value>,
) -> Result<(), ShortcutConfigWriteError> {
    let Some(parent) = path.parent() else {
        return Err(ShortcutConfigWriteError::CreateParentDir {
            path: path.to_path_buf(),
            reason: "config path has no parent directory".to_string(),
        });
    };
    fs::create_dir_all(parent).map_err(|err| ShortcutConfigWriteError::CreateParentDir {
        path: parent.to_path_buf(),
        reason: err.to_string(),
    })?;

    let temp_path = temp_config_path(path);
    let serialized = serde_json::to_string_pretty(&Value::Object(root))
        .expect("shortcut config root should always serialize");
    if let Err(err) = fs::write(&temp_path, format!("{serialized}\n")) {
        return Err(ShortcutConfigWriteError::WriteTempFile {
            path: temp_path,
            reason: err.to_string(),
        });
    }

    if let Err(err) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(ShortcutConfigWriteError::PersistTempFile {
            from: temp_path,
            to: path.to_path_buf(),
            reason: err.to_string(),
        });
    }

    Ok(())
}

fn temp_config_path(path: &Path) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time went backwards")
        .as_nanos();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(SHORTCUTS_FILE_NAME);
    path.with_file_name(format!(".{file_name}.tmp-{}-{nanos}", std::process::id()))
}

pub(crate) fn definition_by_config_key(config_key: &str) -> Option<&'static ShortcutDefinition> {
    definitions()
        .iter()
        .find(|definition| definition.config_key == config_key)
}

fn definition_by_id(id: ShortcutId) -> Option<&'static ShortcutDefinition> {
    definitions().iter().find(|definition| definition.id == id)
}

fn normalized_event_key(
    display: Option<&gdk::Display>,
    keyval: gdk::Key,
    keycode: u32,
) -> Option<String> {
    let resolved = display
        .and_then(|display| unshifted_keyval_for_event(display, keyval, keycode))
        .unwrap_or(keyval);
    resolved
        .name()
        .map(|key_name| normalize_runtime_key(key_name.as_str()))
}

fn unshifted_keyval_for_event(
    display: &gdk::Display,
    keyval: gdk::Key,
    keycode: u32,
) -> Option<gdk::Key> {
    if keycode == 0 {
        return None;
    }

    let keyval_mappings = display.map_keyval(keyval)?;
    let keycode_mappings = display.map_keycode(keycode)?;
    unshifted_keyval_from_mappings(keycode, &keyval_mappings, &keycode_mappings)
}

/// Further readings of a key press, for a binding that names a key which the
/// active layout has elsewhere. The caller tries the exact reading first.
fn fallback_shortcuts(
    display: &gdk::Display,
    keyval: gdk::Key,
    keycode: u32,
    exact: &NormalizedShortcut,
) -> Vec<NormalizedShortcut> {
    let mut readings = Vec::new();
    // The typed symbol. Shift only selected it, so it is not part of the combo.
    let is_letter = keyval.to_unicode().is_some_and(char::is_alphabetic);
    if let Some(key) = keyval.name().map(|name| normalize_runtime_key(&name)) {
        if !is_letter && key != exact.key {
            readings.push(NormalizedShortcut {
                key,
                shift: false,
                ..exact.clone()
            });
        }
    }

    if let (Some(keyval_mappings), Some(keycode_mappings)) =
        (display.map_keyval(keyval), display.map_keycode(keycode))
    {
        let has_latin_letters = |group| {
            display
                .map_keyval(gdk::Key::a)
                .is_some_and(|mappings| mappings.iter().any(|mapping| mapping.group() == group))
        };
        let key = fallback_keyval_from_mappings(
            keycode,
            &keyval_mappings,
            &keycode_mappings,
            has_latin_letters,
        );
        if let Some(key) = key.and_then(|key| key.name()) {
            readings.push(NormalizedShortcut {
                key: normalize_runtime_key(&key),
                ..exact.clone()
            });
        }
    }
    readings
}

/// The digit on the key, as on a French layout where it needs Shift. On a
/// layout without Latin letters, the key that a Latin layout has there.
fn fallback_keyval_from_mappings(
    keycode: u32,
    keyval_mappings: &[gdk::KeymapKey],
    keycode_mappings: &[(gdk::KeymapKey, gdk::Key)],
    has_latin_letters: impl Fn(i32) -> bool,
) -> Option<gdk::Key> {
    let group = group_from_mappings(keycode, keyval_mappings)?;
    let mut on_key = keycode_mappings
        .iter()
        .filter(|(mapping, _)| mapping.keycode() == keycode);
    let digit = on_key.clone().find(|(mapping, key)| {
        mapping.group() == group && key.to_unicode().is_some_and(|ch| ch.is_ascii_digit())
    });
    if let Some((_, digit)) = digit {
        return Some(*digit);
    }
    if has_latin_letters(group) {
        return None;
    }
    on_key
        .find(|(mapping, key)| {
            mapping.group() != group
                && mapping.level() == 0
                && key.to_unicode().is_some_and(|ch| ch.is_ascii_graphic())
        })
        .map(|(_, key)| *key)
}

fn group_from_mappings(keycode: u32, keyval_mappings: &[gdk::KeymapKey]) -> Option<i32> {
    keyval_mappings
        .iter()
        .find(|mapping| mapping.keycode() == keycode)
        .map(gdk::KeymapKey::group)
}

fn canonicalize_loaded_binding(
    display: Option<&gdk::Display>,
    binding: NormalizedShortcut,
) -> NormalizedShortcut {
    let Some(display) = display else {
        return binding;
    };
    let Some(keyval) = gdk::Key::from_name(runtime_key_to_gtk_key(&binding.key)) else {
        return binding;
    };
    let Some(unshifted) = unshifted_keyval_for_loaded_binding(display, keyval) else {
        return binding;
    };
    let Some(key_name) = unshifted.name() else {
        return binding;
    };

    NormalizedShortcut {
        key: normalize_runtime_key(key_name.as_str()),
        ..binding
    }
}

fn unshifted_keyval_for_loaded_binding(
    display: &gdk::Display,
    keyval: gdk::Key,
) -> Option<gdk::Key> {
    let keyval_mappings = display.map_keyval(keyval)?;
    for mapping in keyval_mappings {
        let keycode_mappings = display.map_keycode(mapping.keycode())?;
        if let Some(unshifted) = unshifted_keyval_from_mappings(
            mapping.keycode(),
            std::slice::from_ref(&mapping),
            &keycode_mappings,
        ) {
            return Some(unshifted);
        }
    }
    None
}

fn unshifted_keyval_from_mappings(
    keycode: u32,
    keyval_mappings: &[gdk::KeymapKey],
    keycode_mappings: &[(gdk::KeymapKey, gdk::Key)],
) -> Option<gdk::Key> {
    let group = group_from_mappings(keycode, keyval_mappings)?;

    keycode_mappings
        .iter()
        .find(|(mapping, _)| {
            mapping.keycode() == keycode && mapping.group() == group && mapping.level() == 0
        })
        .map(|(_, key)| *key)
}

fn normalize_runtime_key(key: &str) -> String {
    let normalized = key.trim().replace(['-', ' '], "_").to_ascii_lowercase();
    match normalized.as_str() {
        "pageup" => "page_up".to_string(),
        "pagedown" => "page_down".to_string(),
        "return" => "enter".to_string(),
        "esc" => "escape".to_string(),
        other => other.to_string(),
    }
}

fn is_modifier_only_key(key: &str) -> bool {
    matches!(
        key,
        "shift_l"
            | "shift_r"
            | "control_l"
            | "control_r"
            | "alt_l"
            | "alt_r"
            | "meta_l"
            | "meta_r"
            | "super_l"
            | "super_r"
    )
}

fn runtime_key_to_gtk_key(key: &str) -> String {
    match key {
        "page_up" => "Page_Up".to_string(),
        "page_down" => "Page_Down".to_string(),
        "left" => "Left".to_string(),
        "right" => "Right".to_string(),
        "up" => "Up".to_string(),
        "down" => "Down".to_string(),
        "enter" => "Return".to_string(),
        "escape" => "Escape".to_string(),
        "tab" => "Tab".to_string(),
        other if is_function_key(other) => other.to_ascii_uppercase(),
        other => other.to_string(),
    }
}

fn display_key_label(key: &str) -> String {
    match key {
        "page_up" => "Page Up".to_string(),
        "page_down" => "Page Down".to_string(),
        "left" => "Left".to_string(),
        "right" => "Right".to_string(),
        "up" => "Up".to_string(),
        "down" => "Down".to_string(),
        "enter" => "Enter".to_string(),
        "escape" => "Esc".to_string(),
        "tab" => "Tab".to_string(),
        other if other.chars().count() == 1 => other.to_ascii_uppercase(),
        other => other
            .split('_')
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => {
                        let mut label = first.to_ascii_uppercase().to_string();
                        label.push_str(chars.as_str());
                        label
                    }
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn is_function_key(key: &str) -> bool {
    key.strip_prefix('f')
        .map(|suffix| {
            !suffix.is_empty()
                && suffix.chars().all(|ch| ch.is_ascii_digit())
                && suffix
                    .parse::<u8>()
                    .map(|value| value >= 1)
                    .unwrap_or(false)
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn definitions_cover_current_host_shortcuts() {
        assert_eq!(definitions().len(), 52);
    }

    #[test]
    fn definitions_have_unique_ids_and_action_names_and_accels() {
        let defs = definitions();
        let mut ids = HashMap::new();
        let mut actions = HashMap::new();
        let mut accel_keys = HashMap::new();

        for def in defs {
            assert!(ids.insert(def.id, def.config_key).is_none());
            assert!(actions.insert(def.action_name, def.config_key).is_none());
            assert!(accel_keys
                .insert(def.config_key, def.default_accel)
                .is_none());
        }
    }

    #[test]
    fn definitions_have_expected_gtk_accel_subset() {
        let gtk_actions: Vec<_> = definitions()
            .iter()
            .filter(|def| def.registers_gtk_accel)
            .map(|def| def.action_name)
            .collect();

        assert_eq!(
            gtk_actions,
            vec![
                "win.new-workspace",
                "win.close-workspace",
                "app.quit",
                "app.new-instance",
                "win.toggle-sidebar",
                "win.toggle-top-bar",
                "win.toggle-fullscreen",
                "win.next-workspace",
                "win.prev-workspace",
            ]
        );
    }

    #[test]
    fn normalized_shortcut_round_trips_between_gtk_and_runtime_forms() {
        let shortcut = NormalizedShortcut::parse("<Shift><Ctrl>Page_Down").unwrap();
        assert_eq!(shortcut.to_config_accel(), "<Ctrl><Shift>Page_Down");
        assert_eq!(shortcut.to_runtime_combo(), "ctrl+shift+page_down");
    }

    #[test]
    fn normalized_shortcut_round_trips_cmd_modifier_forms() {
        let shortcut = NormalizedShortcut::parse("<Super><Shift>t").unwrap();
        assert_eq!(shortcut.to_config_accel(), "<Shift><Cmd>t");
        assert_eq!(
            shortcut.gtk_accel_variants(),
            vec!["<Meta><Shift>t".to_string(), "<Super><Shift>t".to_string()]
        );
        assert_eq!(shortcut.to_runtime_combo(), "shift+cmd+t");
        assert_eq!(shortcut.to_display_label(), "Shift+Cmd+T");
    }

    #[test]
    fn unshifted_keyval_from_mappings_uses_same_group_level_zero_key() {
        let keyval_mappings = vec![gdk::KeymapKey::new(10, 1, 1)];
        let keycode_mappings = vec![
            (gdk::KeymapKey::new(10, 0, 0), gdk::Key::_1),
            (gdk::KeymapKey::new(10, 1, 0), gdk::Key::_0),
            (gdk::KeymapKey::new(10, 1, 1), gdk::Key::parenleft),
        ];

        let unshifted =
            unshifted_keyval_from_mappings(10, &keyval_mappings, &keycode_mappings).unwrap();
        assert_eq!(unshifted, gdk::Key::_0);

        let shortcut = NormalizedShortcut::from_gdk_key_event(
            None,
            unshifted,
            10,
            gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK,
        )
        .unwrap();
        assert_eq!(shortcut.to_display_label(), "Ctrl+Shift+0");
    }

    #[test]
    fn fallback_keyval_reads_digits_and_the_latin_key_of_another_layout() {
        // Group 0 is French or Russian, and group 1 is US. Key 12 has a digit
        // behind Shift in group 0, and key 40 has the letter D in group 1.
        let on_key = vec![
            (gdk::KeymapKey::new(12, 0, 0), gdk::Key::quotedbl),
            (gdk::KeymapKey::new(12, 0, 1), gdk::Key::_3),
            (gdk::KeymapKey::new(40, 0, 0), gdk::Key::Cyrillic_ve),
            (gdk::KeymapKey::new(40, 1, 0), gdk::Key::d),
            (gdk::KeymapKey::new(34, 0, 0), gdk::Key::udiaeresis),
            (gdk::KeymapKey::new(34, 1, 0), gdk::Key::bracketleft),
        ];
        let fallback = |keycode, has_latin_letters| {
            fallback_keyval_from_mappings(
                keycode,
                &[gdk::KeymapKey::new(keycode, 0, 0)],
                &on_key,
                |_| has_latin_letters,
            )
        };

        assert_eq!(fallback(12, true), Some(gdk::Key::_3));
        assert_eq!(fallback(40, false), Some(gdk::Key::d));
        assert_eq!(
            fallback(34, true),
            None,
            "a Latin layout keeps its own keys"
        );
        assert_eq!(fallback(99, false), None);
    }

    #[test]
    fn plus_increases_the_font_size_until_the_shortcut_changes() {
        let modifiers = gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK;
        let command = |shortcuts: &ResolvedShortcutConfig, key| {
            shortcuts
                .find_for_key_press(None, key, 0, modifiers)
                .map(|shortcut| shortcut.definition.command)
        };
        let defaults = default_shortcuts();
        for key in [gdk::Key::equal, gdk::Key::plus, gdk::Key::KP_Add] {
            assert_eq!(
                command(&defaults, key),
                Some(ShortcutCommand::TerminalIncreaseFontSize)
            );
        }
        assert_eq!(
            command(&defaults, gdk::Key::KP_Subtract),
            Some(ShortcutCommand::TerminalDecreaseFontSize)
        );

        let plus = NormalizedShortcut::parse("<Ctrl><Alt>plus").unwrap();
        let taken = defaults
            .with_binding(ShortcutId::SplitRight, Some(plus))
            .unwrap();
        assert_eq!(
            command(&taken, gdk::Key::plus),
            Some(ShortcutCommand::SplitRight)
        );
        let unbound = defaults
            .with_binding(ShortcutId::TerminalIncreaseFontSize, None)
            .unwrap();
        assert_eq!(command(&unbound, gdk::Key::plus), None);
    }

    #[test]
    #[ignore = "requires X11 and setxkbmap; exercised by xvfb-smoke-test.sh"]
    fn default_shortcuts_work_on_other_layouts() {
        use crate::shortcut_config::ShortcutCommand::*;
        use gdk::{Key, ModifierType as Mods};
        let ctrl = Mods::CONTROL_MASK;
        let ctrl_alt = ctrl | Mods::ALT_MASK;
        let shift = Mods::SHIFT_MASK;
        let altgr = Mods::from_bits_retain(1 << 7);

        gtk4::init().expect("GTK display required");
        let display = gdk::Display::default().unwrap();
        let defaults = default_shortcuts();
        for (layout, presses) in [
            (
                "us",
                vec![
                    (
                        "Ctrl+Alt+=",
                        Key::equal,
                        21,
                        ctrl_alt,
                        Some(TerminalIncreaseFontSize),
                    ),
                    (
                        "Ctrl+Alt++",
                        Key::plus,
                        21,
                        ctrl_alt | shift,
                        Some(TerminalIncreaseFontSize),
                    ),
                    (
                        "Ctrl+Alt+-",
                        Key::minus,
                        20,
                        ctrl_alt,
                        Some(TerminalDecreaseFontSize),
                    ),
                    (
                        "Ctrl+Alt+Shift+0",
                        Key::parenright,
                        19,
                        ctrl_alt | shift,
                        Some(TerminalResetFontSize),
                    ),
                    (
                        "Ctrl+Alt+1",
                        Key::_1,
                        10,
                        ctrl_alt,
                        Some(ActivateWorkspace1),
                    ),
                    ("Ctrl+Alt+Shift+1", Key::exclam, 10, ctrl_alt | shift, None),
                    ("Ctrl+[", Key::bracketleft, 34, ctrl, Some(BrowserBack)),
                    ("Ctrl+Shift+Y", Key::Y, 29, ctrl | shift, None),
                ],
            ),
            (
                "de,us",
                vec![
                    (
                        "Ctrl+Alt++",
                        Key::plus,
                        35,
                        ctrl_alt,
                        Some(TerminalIncreaseFontSize),
                    ),
                    (
                        "Ctrl+Alt+-",
                        Key::minus,
                        61,
                        ctrl_alt,
                        Some(TerminalDecreaseFontSize),
                    ),
                    (
                        "Ctrl+Alt+Shift+0",
                        Key::equal,
                        19,
                        ctrl_alt | shift,
                        Some(TerminalResetFontSize),
                    ),
                    (
                        "Ctrl+Shift+Z",
                        Key::Z,
                        29,
                        ctrl | shift,
                        Some(ToggleFocusedPaneZoom),
                    ),
                    ("Ctrl+Shift+Y", Key::Y, 52, ctrl | shift, None),
                    (
                        "Ctrl+[",
                        Key::bracketleft,
                        17,
                        ctrl | altgr,
                        Some(BrowserBack),
                    ),
                    ("Ctrl+ü", Key::udiaeresis, 34, ctrl, None),
                    ("@", Key::at, 24, altgr, None),
                ],
            ),
            (
                "fr",
                vec![
                    (
                        "Ctrl+Alt+1",
                        Key::ampersand,
                        10,
                        ctrl_alt,
                        Some(ActivateWorkspace1),
                    ),
                    (
                        "Ctrl+Alt+Shift+1",
                        Key::_1,
                        10,
                        ctrl_alt | shift,
                        Some(ActivateWorkspace1),
                    ),
                    (
                        "Ctrl+Alt+2",
                        Key::eacute,
                        11,
                        ctrl_alt,
                        Some(ActivateWorkspace2),
                    ),
                    (
                        "Ctrl+Alt+9",
                        Key::ccedilla,
                        18,
                        ctrl_alt,
                        Some(ActivateLastWorkspace),
                    ),
                    (
                        "Ctrl+Alt+Shift+0",
                        Key::_0,
                        19,
                        ctrl_alt | shift,
                        Some(TerminalResetFontSize),
                    ),
                    (
                        "Ctrl+Alt+-",
                        Key::minus,
                        15,
                        ctrl_alt,
                        Some(TerminalDecreaseFontSize),
                    ),
                    (
                        "Ctrl+Alt+Shift+6",
                        Key::_6,
                        15,
                        ctrl_alt | shift,
                        Some(ActivateWorkspace6),
                    ),
                    (
                        "Ctrl+Alt+=",
                        Key::equal,
                        21,
                        ctrl_alt,
                        Some(TerminalIncreaseFontSize),
                    ),
                    ("Ctrl+Alt+Q", Key::q, 38, ctrl_alt, Some(QuitApp)),
                    ("#", Key::numbersign, 12, altgr, None),
                ],
            ),
            (
                "il",
                vec![
                    ("Ctrl+Alt+מ", Key::hebrew_mem, 57, ctrl_alt, None),
                    ("Ctrl+Alt+Shift+N", Key::N, 57, ctrl_alt | shift, None),
                ],
            ),
            (
                "us,ru",
                vec![
                    ("Ctrl+Alt+D", Key::d, 40, ctrl_alt, Some(SplitRight)),
                    (
                        "Ctrl+Alt+В",
                        Key::Cyrillic_ve,
                        40,
                        ctrl_alt,
                        Some(SplitRight),
                    ),
                    (
                        "Ctrl+Shift+С",
                        Key::Cyrillic_ES,
                        54,
                        ctrl | shift,
                        Some(TerminalCopy),
                    ),
                    ("Ctrl+С", Key::Cyrillic_es, 54, ctrl, None),
                    (
                        "Ctrl+Alt+1",
                        Key::_1,
                        10,
                        ctrl_alt,
                        Some(ActivateWorkspace1),
                    ),
                ],
            ),
        ] {
            let status = std::process::Command::new("setxkbmap")
                .args(["-layout", layout])
                .status();
            assert!(status.is_ok_and(|status| status.success()), "{layout}");
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let keymap_arrived = || {
                presses.iter().all(|(_, keyval, keycode, ..)| {
                    display
                        .map_keyval(*keyval)
                        .is_some_and(|keys| keys.iter().any(|key| key.keycode() == *keycode))
                })
            };
            while !keymap_arrived() && std::time::Instant::now() < until {
                gtk4::glib::MainContext::default().iteration(false);
            }
            for (press, keyval, keycode, modifiers, command) in presses {
                let found = defaults
                    .find_for_key_press(Some(&display), keyval, keycode, modifiers)
                    .map(|shortcut| shortcut.definition.command);
                assert_eq!(found, command, "{press} on {layout}");
            }
        }
    }

    #[test]
    fn config_dir_path_in_uses_limux_config_dir() {
        let base = Path::new("/tmp/example");
        assert_eq!(
            config_dir_path_in(base),
            PathBuf::from("/tmp/example/limux")
        );
    }

    #[test]
    fn shortcuts_path_in_uses_limux_shortcuts_json() {
        let base = Path::new("/tmp/example");
        assert_eq!(
            shortcuts_path_in(base),
            PathBuf::from("/tmp/example/limux/shortcuts.json")
        );
    }

    #[test]
    fn resolve_shortcuts_from_str_applies_custom_bindings_and_unbinds() {
        let resolved = resolve_shortcuts_from_str(
            r#"{
                "shortcuts": {
                    "toggle_sidebar": "<Ctrl><Alt>b",
                    "split_right": null,
                    "new_terminal": ""
                }
            }"#,
        )
        .unwrap();

        assert_eq!(
            resolved
                .find_by_id(ShortcutId::ToggleSidebar)
                .and_then(ResolvedShortcut::gtk_accel)
                .as_deref(),
            Some("<Ctrl><Alt>b")
        );
        assert_eq!(
            resolved
                .find_by_id(ShortcutId::SplitRight)
                .and_then(ResolvedShortcut::gtk_accel),
            None
        );
        assert_eq!(
            resolved
                .find_by_id(ShortcutId::NewTerminal)
                .and_then(ResolvedShortcut::gtk_accel),
            None
        );
    }

    #[test]
    fn focused_pane_zoom_shortcut_is_remappable_window_command() {
        let defaults = default_shortcuts();
        let shortcut = defaults
            .find_by_id(ShortcutId::ToggleFocusedPaneZoom)
            .expect("focused pane zoom shortcut should be registered");

        assert_eq!(shortcut.definition.config_key, "toggle_focused_pane_zoom");
        assert_eq!(
            shortcut.definition.command,
            ShortcutCommand::ToggleFocusedPaneZoom
        );
        assert_eq!(shortcut.definition.scope, ShortcutScope::Window);
        assert_eq!(shortcut.runtime_combo().as_deref(), Some("ctrl+shift+z"));
        assert_eq!(
            defaults.command_for_runtime_combo("ctrl+shift+z"),
            Some(ShortcutCommand::ToggleFocusedPaneZoom)
        );
    }

    #[test]
    fn resolve_shortcuts_from_str_warns_on_unknown_ids() {
        let resolved = resolve_shortcuts_from_str(
            r#"{
                "shortcuts": {
                    "toggle_sidebar": "<Ctrl><Alt>b",
                    "unknown_action": "<Ctrl>x"
                }
            }"#,
        )
        .unwrap();

        assert_eq!(resolved.warnings.len(), 1);
        assert!(resolved.warnings[0].contains("unknown shortcut id `unknown_action`"));
    }

    #[test]
    fn resolve_shortcuts_from_str_rejects_duplicate_active_bindings() {
        let err = resolve_shortcuts_from_str(
            r#"{
                "shortcuts": {
                    "toggle_sidebar": "<Ctrl><Alt>b",
                    "split_right": "<Ctrl><Alt>b"
                }
            }"#,
        )
        .unwrap_err();

        assert!(matches!(err, ShortcutConfigError::DuplicateBinding { .. }));
    }

    #[test]
    fn load_shortcuts_or_default_falls_back_on_invalid_json() {
        let dir = tempdir().unwrap();
        let path = shortcuts_path_in(dir.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{ this is not json").unwrap();

        let resolved = load_shortcuts_or_default(&path);

        assert_eq!(resolved.shortcuts.len(), definitions().len());
        assert_eq!(resolved.warnings.len(), 1);
        assert!(resolved.warnings[0].contains("failed to load shortcut config"));
    }

    #[test]
    fn load_shortcuts_or_default_uses_defaults_when_file_is_missing() {
        let dir = tempdir().unwrap();
        let path = shortcuts_path_in(dir.path());
        let resolved = load_shortcuts_or_default(&path);
        assert!(resolved.warnings.is_empty());
        assert_eq!(resolved.shortcuts.len(), definitions().len());
    }

    #[test]
    fn load_shortcuts_or_default_migrates_legacy_config_shortcuts() {
        let dir = tempdir().unwrap();
        let path = shortcuts_path_in(dir.path());
        let legacy_path = legacy_config_path_for(&path).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &legacy_path,
            r#"{
                "theme": "legacy",
                "shortcuts": {
                    "split_right": "<Ctrl>h",
                    "close_focused_pane": null,
                    "toggle_sidebar": "<Ctrl><Alt>m"
                }
            }"#,
        )
        .unwrap();

        let resolved = load_shortcuts_or_default(&path);

        assert!(resolved.warnings.is_empty());
        assert_eq!(
            resolved
                .find_by_id(ShortcutId::SplitRight)
                .and_then(ResolvedShortcut::gtk_accel)
                .as_deref(),
            Some("<Ctrl>h")
        );
        assert_eq!(
            resolved
                .find_by_id(ShortcutId::CloseFocusedPane)
                .and_then(ResolvedShortcut::gtk_accel),
            None
        );

        let legacy: Value = serde_json::from_str(&fs::read_to_string(&legacy_path).unwrap())
            .expect("legacy config should remain valid");
        assert_eq!(legacy["theme"], "legacy");

        let migrated: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap())
            .expect("migrated shortcuts config should be valid");
        assert_eq!(migrated["shortcuts"]["split_right"], "<Ctrl>h");
        assert_eq!(
            migrated["shortcuts"]["close_focused_pane"],
            serde_json::Value::Null
        );
        assert!(migrated["shortcuts"].get("toggle_sidebar").is_none());
        assert!(migrated.get("theme").is_none());
    }

    #[test]
    fn load_shortcuts_or_default_prefers_existing_canonical_shortcuts() {
        let dir = tempdir().unwrap();
        let path = shortcuts_path_in(dir.path());
        let legacy_path = legacy_config_path_for(&path).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{
                "shortcuts": {
                    "split_right": "<Ctrl>h"
                }
            }"#,
        )
        .unwrap();
        fs::write(
            &legacy_path,
            r#"{
                "shortcuts": {
                    "split_right": "<Ctrl><Alt>h"
                }
            }"#,
        )
        .unwrap();

        let resolved = load_shortcuts_or_default(&path);

        assert!(resolved.warnings.is_empty());
        assert_eq!(
            resolved
                .find_by_id(ShortcutId::SplitRight)
                .and_then(ResolvedShortcut::gtk_accel)
                .as_deref(),
            Some("<Ctrl>h")
        );
    }

    #[test]
    fn load_shortcuts_or_default_ignores_legacy_config_without_shortcuts_key() {
        let dir = tempdir().unwrap();
        let path = shortcuts_path_in(dir.path());
        let legacy_path = legacy_config_path_for(&path).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &legacy_path,
            r#"{
                "theme": "legacy"
            }"#,
        )
        .unwrap();

        let resolved = load_shortcuts_or_default(&path);

        assert!(resolved.warnings.is_empty());
        assert_eq!(resolved, default_shortcuts());
        assert!(!path.exists());
    }

    #[test]
    fn load_shortcuts_or_default_migrates_explicit_empty_legacy_shortcuts() {
        let dir = tempdir().unwrap();
        let path = shortcuts_path_in(dir.path());
        let legacy_path = legacy_config_path_for(&path).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &legacy_path,
            r#"{
                "shortcuts": {}
            }"#,
        )
        .unwrap();

        let resolved = load_shortcuts_or_default(&path);

        assert!(resolved.warnings.is_empty());
        assert_eq!(resolved, default_shortcuts());
        let migrated: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap())
            .expect("migrated empty shortcuts config should be valid");
        assert_eq!(migrated, serde_json::json!({}));
    }

    #[test]
    fn load_shortcuts_or_default_rejects_invalid_legacy_shortcuts_without_migrating() {
        let dir = tempdir().unwrap();
        let path = shortcuts_path_in(dir.path());
        let legacy_path = legacy_config_path_for(&path).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &legacy_path,
            r#"{
                "shortcuts": {
                    "split_right": "<Shift>h"
                }
            }"#,
        )
        .unwrap();

        let resolved = load_shortcuts_or_default(&path);

        assert_eq!(resolved.shortcuts, default_shortcuts().shortcuts);
        assert_eq!(resolved.warnings.len(), 1);
        assert!(resolved.warnings[0].contains("failed to load legacy shortcut config"));
        assert!(!path.exists());
    }

    #[test]
    fn load_shortcuts_or_default_preserves_unknown_legacy_warnings_and_omits_unknown_ids() {
        let dir = tempdir().unwrap();
        let path = shortcuts_path_in(dir.path());
        let legacy_path = legacy_config_path_for(&path).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &legacy_path,
            r#"{
                "shortcuts": {
                    "split_right": "<Ctrl>h",
                    "unknown_action": "<Ctrl><Alt>u"
                }
            }"#,
        )
        .unwrap();

        let resolved = load_shortcuts_or_default(&path);

        assert_eq!(resolved.warnings.len(), 1);
        assert!(resolved.warnings[0].contains("unknown shortcut id `unknown_action`"));
        let migrated: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap())
            .expect("migrated shortcuts config should be valid");
        assert_eq!(migrated["shortcuts"]["split_right"], "<Ctrl>h");
        assert!(migrated["shortcuts"].get("unknown_action").is_none());
    }

    #[test]
    fn load_legacy_shortcuts_uses_resolved_config_when_migration_write_fails() {
        let dir = tempdir().unwrap();
        let path = shortcuts_path_in(dir.path());
        let legacy_path = legacy_config_path_for(&path).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::create_dir(&path).unwrap();
        fs::write(
            &legacy_path,
            r#"{
                "shortcuts": {
                    "split_right": "<Ctrl>h"
                }
            }"#,
        )
        .unwrap();

        let resolved = load_legacy_shortcuts_or_default(&path, None);

        assert_eq!(
            resolved
                .find_by_id(ShortcutId::SplitRight)
                .and_then(ResolvedShortcut::gtk_accel)
                .as_deref(),
            Some("<Ctrl>h")
        );
        assert_eq!(resolved.warnings.len(), 1);
        assert!(resolved.warnings[0].contains("failed to migrate"));
    }

    #[test]
    fn resolved_shortcuts_expose_registered_gtk_accels_and_clear_unbound_actions() {
        let resolved = resolve_shortcuts_from_str(
            r#"{
                "shortcuts": {
                    "toggle_sidebar": null
                }
            }"#,
        )
        .unwrap();

        let gtk_accels = resolved.gtk_accel_entries();
        assert_eq!(gtk_accels.len(), 9);
        assert_eq!(
            gtk_accels
                .iter()
                .find(|(action, _)| *action == "win.toggle-sidebar")
                .map(|(_, accels)| accels.clone()),
            Some(Vec::<String>::new())
        );
    }

    #[test]
    fn gtk_accel_entries_keep_ctrl_defaults_single_and_expand_cmd_remaps() {
        let resolved = default_shortcuts();
        let app_quit = resolved
            .gtk_accel_entries()
            .into_iter()
            .find(|(action, _)| *action == "app.quit")
            .map(|(_, accels)| accels)
            .unwrap();
        assert_eq!(app_quit, vec!["<Ctrl><Alt>q".to_string()]);

        let remapped = resolve_shortcuts_from_str(
            r#"{
                "shortcuts": {
                    "quit_app": "<Super>q"
                }
            }"#,
        )
        .unwrap();
        let remapped_quit = remapped
            .gtk_accel_entries()
            .into_iter()
            .find(|(action, _)| *action == "app.quit")
            .map(|(_, accels)| accels)
            .unwrap();
        assert_eq!(
            remapped_quit,
            vec!["<Meta>q".to_string(), "<Super>q".to_string()]
        );
    }

    #[test]
    fn resolved_shortcuts_route_runtime_combos_to_canonical_commands() {
        let resolved = default_shortcuts();

        assert_eq!(
            resolved.command_for_runtime_combo("ctrl+alt+t"),
            Some(ShortcutCommand::NewTerminal)
        );
        assert_eq!(resolved.command_for_runtime_combo("ctrl+t"), None);
        assert_eq!(resolved.command_for_runtime_combo("ctrl+c"), None);
        assert_eq!(
            resolved.command_for_runtime_combo("ctrl+shift+c"),
            Some(ShortcutCommand::TerminalCopy)
        );
        assert_eq!(
            resolved.command_for_runtime_combo("ctrl+shift+t"),
            Some(ShortcutCommand::NewTerminal)
        );
        assert_eq!(resolved.command_for_runtime_combo("ctrl+w"), None);
        assert_eq!(
            resolved.command_for_runtime_combo("ctrl+shift+w"),
            Some(ShortcutCommand::CloseFocusedTab)
        );
        assert_eq!(
            resolved.command_for_runtime_combo("ctrl+alt+w"),
            Some(ShortcutCommand::CloseFocusedPane)
        );
        assert_eq!(resolved.command_for_runtime_combo("ctrl+9"), None);
        assert_eq!(
            resolved.command_for_runtime_combo("ctrl+alt+9"),
            Some(ShortcutCommand::ActivateLastWorkspace)
        );
    }

    #[test]
    fn resolved_shortcuts_expose_default_display_labels_for_editor_rows() {
        let resolved = default_shortcuts();

        assert_eq!(
            resolved
                .default_display_label_for_id(ShortcutId::SplitRight)
                .as_deref(),
            Some("Ctrl+Alt+D")
        );
        assert_eq!(
            resolved
                .find_by_id(ShortcutId::SplitRight)
                .map(ResolvedShortcut::default_display_label)
                .as_deref(),
            Some("Ctrl+Alt+D")
        );
        assert_eq!(
            resolved
                .default_display_label_for_id(ShortcutId::CloseFocusedTab)
                .as_deref(),
            Some("Ctrl+Shift+W")
        );
        assert_eq!(
            resolved
                .default_display_label_for_id(ShortcutId::CloseFocusedPane)
                .as_deref(),
            Some("Ctrl+Alt+W")
        );
        assert_eq!(
            resolved
                .default_display_label_for_id(ShortcutId::TerminalPaste)
                .as_deref(),
            Some("Ctrl+Shift+V")
        );
    }

    #[test]
    fn default_terminal_paste_does_not_claim_plain_ctrl_v() {
        let resolved = default_shortcuts();

        assert_eq!(resolved.command_for_runtime_combo("ctrl+v"), None);
        assert_eq!(
            resolved.command_for_runtime_combo("ctrl+shift+v"),
            Some(ShortcutCommand::TerminalPaste)
        );
    }

    #[test]
    fn override_bindings_json_only_serializes_non_default_bindings() {
        let resolved = resolve_shortcuts_from_str(
            r#"{
                "shortcuts": {
                    "split_right": "<Ctrl>h",
                    "close_focused_pane": null
                }
            }"#,
        )
        .unwrap();

        let overrides = resolved.override_bindings_json();
        assert_eq!(overrides.len(), 2);
        assert_eq!(
            overrides.get("split_right"),
            Some(&Value::String("<Ctrl>h".to_string()))
        );
        assert_eq!(overrides.get("close_focused_pane"), Some(&Value::Null));
        assert!(!overrides.contains_key("toggle_sidebar"));
    }

    #[test]
    fn with_binding_updates_one_shortcut_and_revalidates() {
        let updated = default_shortcuts()
            .with_binding(
                ShortcutId::SplitRight,
                Some(NormalizedShortcut::parse("<Ctrl>h").unwrap()),
            )
            .unwrap();

        assert_eq!(
            updated
                .display_label_for_id(ShortcutId::SplitRight)
                .as_deref(),
            Some("Ctrl+H")
        );
    }

    #[test]
    fn write_shortcuts_preserves_unrelated_top_level_config_keys() {
        let dir = tempdir().unwrap();
        let path = shortcuts_path_in(dir.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{
                "appearance": {
                    "theme": "solarized"
                },
                "shortcuts": {
                    "toggle_sidebar": "<Ctrl><Alt>b"
                }
            }"#,
        )
        .unwrap();

        let resolved = resolve_shortcuts_from_str(
            r#"{
                "shortcuts": {
                    "split_right": "<Ctrl>h"
                }
            }"#,
        )
        .unwrap();
        write_shortcuts(&path, &resolved).unwrap();

        let saved: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["appearance"]["theme"], "solarized");
        assert_eq!(saved["shortcuts"]["split_right"], "<Ctrl>h");
        assert!(saved["shortcuts"].get("toggle_sidebar").is_none());
    }

    #[test]
    fn write_shortcuts_rejects_invalid_existing_json_without_clobbering_file() {
        let dir = tempdir().unwrap();
        let path = shortcuts_path_in(dir.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{ invalid").unwrap();

        let original = fs::read_to_string(&path).unwrap();
        let resolved = default_shortcuts();
        let err = write_shortcuts(&path, &resolved).unwrap_err();

        assert!(matches!(
            err,
            ShortcutConfigWriteError::InvalidExistingJson { .. }
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn resolved_shortcuts_format_tooltip_text_and_omit_unbound_suffixes() {
        let defaults = default_shortcuts();
        assert_eq!(
            defaults.tooltip_text(ShortcutId::ToggleSidebar, "Toggle Sidebar"),
            "Toggle Sidebar (Ctrl+Alt+M)"
        );

        let remapped = resolve_shortcuts_from_str(
            r#"{
                "shortcuts": {
                    "toggle_sidebar": "<Ctrl><Alt>b"
                }
            }"#,
        )
        .unwrap();
        assert_eq!(
            remapped.tooltip_text(ShortcutId::ToggleSidebar, "Toggle Sidebar"),
            "Toggle Sidebar (Ctrl+Alt+B)"
        );

        let unbound = resolve_shortcuts_from_str(
            r#"{
                "shortcuts": {
                    "toggle_sidebar": null
                }
            }"#,
        )
        .unwrap();
        assert_eq!(
            unbound.tooltip_text(ShortcutId::ToggleSidebar, "Toggle Sidebar"),
            "Toggle Sidebar"
        );
    }

    #[test]
    fn resolve_shortcuts_from_str_rejects_bindings_without_ctrl_alt_or_cmd() {
        let err = resolve_shortcuts_from_str(
            r#"{
                "shortcuts": {
                    "split_right": "<Shift>h"
                }
            }"#,
        )
        .unwrap_err();

        assert!(matches!(
            err,
            ShortcutConfigError::BaseModifierRequired { shortcut_id, .. }
                if shortcut_id == "split_right"
        ));
    }

    #[test]
    fn resolve_shortcuts_from_str_allows_unmodified_fullscreen_binding() {
        let resolved = resolve_shortcuts_from_str(
            r#"{
                "shortcuts": {
                    "toggle_fullscreen": "F11"
                }
            }"#,
        )
        .unwrap();

        assert_eq!(
            resolved
                .display_label_for_id(ShortcutId::ToggleFullscreen)
                .as_deref(),
            Some("F11")
        );
        assert_eq!(
            resolved.command_for_runtime_combo("f11"),
            Some(ShortcutCommand::ToggleFullscreen)
        );
        assert_eq!(
            resolved
                .find_by_id(ShortcutId::ToggleFullscreen)
                .map(ResolvedShortcut::gtk_accel_variants),
            Some(vec!["F11".to_string()])
        );
    }

    #[test]
    fn resolve_shortcuts_from_str_accepts_super_based_bindings() {
        let resolved = resolve_shortcuts_from_str(
            r#"{
                "shortcuts": {
                    "split_right": "<Super>h"
                }
            }"#,
        )
        .unwrap();

        assert_eq!(
            resolved
                .display_label_for_id(ShortcutId::SplitRight)
                .as_deref(),
            Some("Cmd+H")
        );
        assert_eq!(
            resolved.command_for_runtime_combo("cmd+h"),
            Some(ShortcutCommand::SplitRight)
        );
    }

    #[test]
    fn resolve_shortcuts_from_str_rejects_modifier_only_keys() {
        let err = resolve_shortcuts_from_str(
            r#"{
                "shortcuts": {
                    "split_right": "<Ctrl>Control_L"
                }
            }"#,
        )
        .unwrap_err();

        assert!(matches!(
            err,
            ShortcutConfigError::ModifierOnlyBinding { shortcut_id, .. }
                if shortcut_id == "split_right"
        ));
    }

    #[test]
    fn write_shortcuts_omits_defaults_and_preserves_unrelated_settings() {
        let dir = tempdir().unwrap();
        let path = shortcuts_path_in(dir.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{
                "theme": "nord",
                "shortcuts": {
                    "split_right": "<Ctrl><Alt>d"
                }
            }"#,
        )
        .unwrap();

        let updated = resolve_shortcuts_from_str(
            r#"{
                "shortcuts": {
                    "split_right": "<Ctrl><Alt>h",
                    "close_focused_pane": null
                }
            }"#,
        )
        .unwrap();

        write_shortcuts(&path, &updated).unwrap();

        let written: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["theme"], "nord");
        assert_eq!(written["shortcuts"]["split_right"], "<Ctrl><Alt>h");
        assert_eq!(
            written["shortcuts"]["close_focused_pane"],
            serde_json::Value::Null
        );
        assert!(written["shortcuts"].get("toggle_sidebar").is_none());
    }

    #[test]
    fn write_shortcuts_removes_shortcuts_section_when_all_bindings_match_defaults() {
        let dir = tempdir().unwrap();
        let path = shortcuts_path_in(dir.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{
                "theme": "nord",
                "shortcuts": {
                    "split_right": "<Ctrl><Alt>h"
                }
            }"#,
        )
        .unwrap();

        write_shortcuts(&path, &default_shortcuts()).unwrap();

        let written: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["theme"], "nord");
        assert!(written.get("shortcuts").is_none());
    }

    #[test]
    fn find_for_key_press_matches_defaults_and_tracks_remaps() {
        let defaults = default_shortcuts();
        assert!(!defaults.index.has_unmodified_fallback);
        assert!(!defaults.index.has_shift_only_binding);

        assert!(defaults
            .find_for_key_press(None, gdk::Key::a, 38, gdk::ModifierType::empty())
            .is_none());
        assert!(defaults
            .find_for_key_press(None, gdk::Key::A, 38, gdk::ModifierType::SHIFT_MASK)
            .is_none());
        assert_eq!(
            defaults
                .find_for_key_press(None, gdk::Key::F11, 95, gdk::ModifierType::empty())
                .map(|shortcut| shortcut.definition.command),
            Some(ShortcutCommand::ToggleFullscreen)
        );
        assert_eq!(
            defaults
                .find_for_key_press(
                    None,
                    gdk::Key::T,
                    28,
                    gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK,
                )
                .map(|shortcut| shortcut.definition.command),
            Some(ShortcutCommand::NewTerminal)
        );

        let shift_fullscreen = defaults
            .with_binding(
                ShortcutId::ToggleFullscreen,
                Some(NormalizedShortcut::parse("<Shift>F12").unwrap()),
            )
            .unwrap();
        assert!(shift_fullscreen.index.has_shift_only_binding);
        assert!(shift_fullscreen
            .find_for_key_press(None, gdk::Key::F11, 95, gdk::ModifierType::empty())
            .is_none());
        assert_eq!(
            shift_fullscreen
                .find_for_key_press(None, gdk::Key::F12, 96, gdk::ModifierType::SHIFT_MASK)
                .map(|shortcut| shortcut.definition.command),
            Some(ShortcutCommand::ToggleFullscreen)
        );

        let letter_fullscreen = defaults
            .with_binding(
                ShortcutId::ToggleFullscreen,
                Some(NormalizedShortcut::parse("a").unwrap()),
            )
            .unwrap();
        assert!(letter_fullscreen.index.has_unmodified_fallback);
        assert_eq!(
            letter_fullscreen
                .find_for_key_press(None, gdk::Key::a, 38, gdk::ModifierType::empty())
                .map(|shortcut| shortcut.definition.command),
            Some(ShortcutCommand::ToggleFullscreen)
        );

        let disabled_fullscreen = defaults
            .with_binding(ShortcutId::ToggleFullscreen, None)
            .unwrap();
        assert!(disabled_fullscreen.index.unmodified_keyvals.is_empty());
        assert!(disabled_fullscreen
            .find_for_key_press(None, gdk::Key::F11, 95, gdk::ModifierType::empty())
            .is_none());

        // CapsLock (LOCK_MASK) is ignored on unmodified F11; Shift+F11 is not matched.
        assert_eq!(
            defaults
                .find_for_key_press(None, gdk::Key::F11, 95, gdk::ModifierType::LOCK_MASK)
                .map(|shortcut| shortcut.definition.command),
            Some(ShortcutCommand::ToggleFullscreen)
        );
        assert!(defaults
            .find_for_key_press(None, gdk::Key::F11, 95, gdk::ModifierType::SHIFT_MASK)
            .is_none());

        // Unmodified printable keys skip display resolution entirely.
        let mut display_called = false;
        assert!(defaults
            .find_for_key_press_with_display(
                || {
                    display_called = true;
                    None
                },
                gdk::Key::a,
                38,
                gdk::ModifierType::empty(),
            )
            .is_none());
        assert!(!display_called);

        // Loading overrides from JSON rebuilds the lookup index too.
        let loaded =
            resolve_shortcuts_from_str(r#"{"shortcuts": {"toggle_fullscreen": "<Shift>F12"}}"#)
                .expect("valid shortcut JSON");
        assert!(loaded.index.has_shift_only_binding);
        assert_eq!(
            loaded
                .find_for_key_press(None, gdk::Key::F12, 96, gdk::ModifierType::SHIFT_MASK)
                .map(|shortcut| shortcut.definition.command),
            Some(ShortcutCommand::ToggleFullscreen)
        );
    }
}
