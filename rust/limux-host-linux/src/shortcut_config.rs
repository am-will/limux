use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use gtk4::gdk;
use gtk4::gdk::prelude::DisplayExtManual;
use serde::Deserialize;
use serde_json::{Map, Value};

pub const CONFIG_DIR_NAME: &str = "limux";
pub const SHORTCUTS_FILE_NAME: &str = "shortcuts.json";

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedShortcutConfig {
    pub shortcuts: Vec<ResolvedShortcut>,
    pub warnings: Vec<String>,
}

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

const SHORTCUT_DEFINITIONS: [ShortcutDefinition; 47] = [
    ShortcutDefinition {
        id: ShortcutId::NewWorkspace,
        config_key: "new_workspace",
        action_name: "win.new-workspace",
        default_accel: "<Ctrl><Shift>n",
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
        default_accel: "<Ctrl><Shift>w",
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
        default_accel: "<Ctrl>q",
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
        default_accel: "<Ctrl>m",
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
        default_accel: "<Ctrl><Shift>m",
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
        default_accel: "<Ctrl>Page_Down",
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
        default_accel: "<Ctrl>Page_Up",
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
        default_accel: "<Ctrl><Shift>Left",
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
        default_accel: "<Ctrl><Shift>Right",
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
        default_accel: "<Ctrl><Shift>d",
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
        default_accel: "<Ctrl>d",
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
        default_accel: "<Ctrl>w",
        label: "Close Focused Pane",
        registers_gtk_accel: false,
        command: ShortcutCommand::CloseFocusedPane,
        scope: ShortcutScope::Window,
        editable_capture_policy: EditableCapturePolicy::BypassInEditable,
    },
    ShortcutDefinition {
        id: ShortcutId::NewTerminal,
        config_key: "new_terminal",
        action_name: "win.new-terminal",
        default_accel: "<Ctrl>t",
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
        default_accel: "<Ctrl>Left",
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
        default_accel: "<Ctrl>Right",
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
        default_accel: "<Ctrl>Up",
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
        default_accel: "<Ctrl>Down",
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
        default_accel: "<Ctrl>1",
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
        default_accel: "<Ctrl>2",
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
        default_accel: "<Ctrl>3",
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
        default_accel: "<Ctrl>4",
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
        default_accel: "<Ctrl>5",
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
        default_accel: "<Ctrl>6",
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
        default_accel: "<Ctrl>7",
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
        default_accel: "<Ctrl>8",
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
        default_accel: "<Ctrl>9",
        label: "Activate Last Workspace",
        registers_gtk_accel: false,
        command: ShortcutCommand::ActivateLastWorkspace,
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
        default_accel: "<Ctrl>f",
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
        default_accel: "<Ctrl>g",
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
        default_accel: "<Ctrl><Shift>g",
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
        default_accel: "<Ctrl><Shift>f",
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
        default_accel: "<Ctrl>e",
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
        default_accel: "<Ctrl>k",
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
        default_accel: "<Ctrl>plus",
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
        default_accel: "<Ctrl>minus",
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
        default_accel: "<Ctrl><Shift>0",
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
        self.find_by_runtime_combo(combo)
            .map(|shortcut| shortcut.definition.command)
    }

    pub fn shortcut_for_runtime_combo(&self, combo: &str) -> Option<&ResolvedShortcut> {
        self.find_by_runtime_combo(combo)
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

    pub fn find_by_runtime_combo(&self, combo: &str) -> Option<&ResolvedShortcut> {
        self.shortcuts
            .iter()
            .find(|shortcut| shortcut.runtime_combo().as_deref() == Some(combo))
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

pub fn default_shortcuts() -> ResolvedShortcutConfig {
    ResolvedShortcutConfig {
        shortcuts: definitions()
            .iter()
            .map(|definition| ResolvedShortcut {
                definition,
                binding: Some(definition.default_binding()),
            })
            .collect(),
        warnings: Vec::new(),
    }
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
        return default_shortcuts();
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
    let group = keyval_mappings
        .iter()
        .find(|mapping| mapping.keycode() == keycode)
        .map(gdk::KeymapKey::group)?;

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
        assert_eq!(definitions().len(), 47);
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
        assert_eq!(app_quit, vec!["<Ctrl>q".to_string()]);

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
            resolved.command_for_runtime_combo("ctrl+t"),
            Some(ShortcutCommand::NewTerminal)
        );
        assert_eq!(resolved.command_for_runtime_combo("ctrl+c"), None);
        assert_eq!(
            resolved.command_for_runtime_combo("ctrl+shift+c"),
            Some(ShortcutCommand::TerminalCopy)
        );
        assert_eq!(
            resolved.command_for_runtime_combo("ctrl+shift+t"),
            Some(ShortcutCommand::NewTerminal)
        );
        assert_eq!(
            resolved.command_for_runtime_combo("ctrl+9"),
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
            Some("Ctrl+D")
        );
        assert_eq!(
            resolved
                .find_by_id(ShortcutId::SplitRight)
                .map(ResolvedShortcut::default_display_label)
                .as_deref(),
            Some("Ctrl+D")
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
            "Toggle Sidebar (Ctrl+M)"
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
                    "split_right": "<Ctrl>d"
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
}
