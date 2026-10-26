//! Hotkey bindings. Accelerators are strings like `Ctrl+Shift+4`,
//! `Alt+PrintScreen`, `Cmd+Shift+2` (`Cmd`/`Super`/`Win` are the same key;
//! `CmdOrCtrl` resolves per platform). Parsing into OS codes is done by
//! `snap-platform::hotkeys`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HotkeyBinding {
    pub accel: String,
    pub action: Action,
    /// Pipeline to run after a capture action (`None` = action default).
    #[serde(default)]
    pub pipeline: Option<String>,
    #[serde(default = "enabled")]
    pub enabled: bool,
}

fn enabled() -> bool {
    true
}

/// Everything a hotkey, tray item, CLI call or URL can trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    CaptureRegion,
    CaptureWindow,
    CaptureElement,
    CaptureMonitor,
    CaptureAllMonitors,
    CaptureScrolling,
    CaptureLastRegion,
    CaptureDelayed,
    RecordRegion,
    RecordScreen,
    RecordGif,
    StopRecording,
    OcrRegion,
    DecodeCodesRegion,
    TranslateRegion,
    ColorPicker,
    Measure,
    PinRegion,
    PinClipboard,
    StepCaptureToggle,
    OpenEditorFromClipboard,
    OpenLibrary,
    OpenSettings,
}

impl Action {
    pub const ALL: &'static [Action] = &[
        Action::CaptureRegion,
        Action::CaptureWindow,
        Action::CaptureElement,
        Action::CaptureMonitor,
        Action::CaptureAllMonitors,
        Action::CaptureScrolling,
        Action::CaptureLastRegion,
        Action::CaptureDelayed,
        Action::RecordRegion,
        Action::RecordScreen,
        Action::RecordGif,
        Action::StopRecording,
        Action::OcrRegion,
        Action::DecodeCodesRegion,
        Action::TranslateRegion,
        Action::ColorPicker,
        Action::Measure,
        Action::PinRegion,
        Action::PinClipboard,
        Action::StepCaptureToggle,
        Action::OpenEditorFromClipboard,
        Action::OpenLibrary,
        Action::OpenSettings,
    ];

    /// Stable key used for i18n (`action.<key>`) and the CLI/URL scheme.
    pub fn key(self) -> &'static str {
        match self {
            Action::CaptureRegion => "capture_region",
            Action::CaptureWindow => "capture_window",
            Action::CaptureElement => "capture_element",
            Action::CaptureMonitor => "capture_monitor",
            Action::CaptureAllMonitors => "capture_all_monitors",
            Action::CaptureScrolling => "capture_scrolling",
            Action::CaptureLastRegion => "capture_last_region",
            Action::CaptureDelayed => "capture_delayed",
            Action::RecordRegion => "record_region",
            Action::RecordScreen => "record_screen",
            Action::RecordGif => "record_gif",
            Action::StopRecording => "stop_recording",
            Action::OcrRegion => "ocr_region",
            Action::DecodeCodesRegion => "decode_codes_region",
            Action::TranslateRegion => "translate_region",
            Action::ColorPicker => "color_picker",
            Action::Measure => "measure",
            Action::PinRegion => "pin_region",
            Action::PinClipboard => "pin_clipboard",
            Action::StepCaptureToggle => "step_capture_toggle",
            Action::OpenEditorFromClipboard => "open_editor_from_clipboard",
            Action::OpenLibrary => "open_library",
            Action::OpenSettings => "open_settings",
        }
    }

    pub fn from_key(key: &str) -> Option<Action> {
        Action::ALL.iter().copied().find(|a| a.key() == key)
    }

    pub fn is_capture(self) -> bool {
        matches!(
            self,
            Action::CaptureRegion
                | Action::CaptureWindow
                | Action::CaptureElement
                | Action::CaptureMonitor
                | Action::CaptureAllMonitors
                | Action::CaptureScrolling
                | Action::CaptureLastRegion
                | Action::CaptureDelayed
        )
    }
}

/// Version of [`default_bindings`]. Settings remember the version they were written with, so that a change of the
/// defaults reaches existing settings files (see [`migrate`]).
pub const DEFAULTS_REV: u32 = 1;

/// The built-in bindings. Modifiers are written in the order the recorder produces (Ctrl, Alt, Shift, Cmd), which is
/// also Apple's order, so the keycaps read like a macOS menu.
pub fn default_bindings() -> Vec<HotkeyBinding> {
    let b = |accel: &str, action: Action| HotkeyBinding { accel: accel.into(), action, pipeline: None, enabled: true };
    if cfg!(target_os = "macos") {
        // A Carbon global hotkey does not displace a system shortcut, both fire. macOS owns Shift+Cmd+3/4/5/6 (and
        // Ctrl+Shift+Cmd+3/4/5) for screenshots and, on recent versions, Shift+Cmd+7; Slack and Google Docs use
        // Shift+Cmd+7/8/9 for lists and quotes; Alt+Shift+Cmd+V/S/Q are paste-and-match-style, Save As and log out.
        // Shift+Cmd+1/2 are what other capture tools use, the rest takes Alt+Shift+Cmd with a mnemonic letter.
        vec![
            b("Shift+Cmd+2", Action::CaptureRegion),
            b("Alt+Shift+Cmd+2", Action::CaptureWindow),
            b("Shift+Cmd+1", Action::CaptureAllMonitors),
            b("Alt+Shift+Cmd+R", Action::RecordRegion),
            b("Alt+Shift+Cmd+.", Action::StopRecording),
            b("Alt+Shift+Cmd+T", Action::OcrRegion),
            b("Alt+Shift+Cmd+P", Action::PinRegion),
        ]
    } else {
        vec![
            b("PrintScreen", Action::CaptureRegion),
            b("Alt+PrintScreen", Action::CaptureWindow),
            b("Ctrl+PrintScreen", Action::CaptureAllMonitors),
            b("Ctrl+Shift+PrintScreen", Action::RecordRegion),
            b("Ctrl+Shift+F12", Action::StopRecording),
            b("Ctrl+Shift+T", Action::OcrRegion),
            b("Ctrl+Shift+P", Action::PinRegion),
        ]
    }
}

/// The macOS defaults before revision 1. They took the system screenshot shortcuts, so pressing one ran both the
/// macOS screenshot tool and Snapoleon.
const MAC_DEFAULTS_REV0: [(&str, Action); 7] = [
    ("Cmd+Shift+4", Action::CaptureRegion),
    ("Cmd+Shift+5", Action::CaptureWindow),
    ("Cmd+Shift+3", Action::CaptureAllMonitors),
    ("Cmd+Shift+6", Action::RecordRegion),
    ("Cmd+Shift+7", Action::StopRecording),
    ("Cmd+Shift+2", Action::OcrRegion),
    ("Cmd+Shift+1", Action::PinRegion),
];

/// Moves bindings written with the defaults of revision `from_rev` to the current defaults. Only a binding that still
/// holds its old default (same action, same accelerator) is touched, so a shortcut the user picked is kept; its
/// pipeline and enabled state stay as they are. A binding is also left alone when its new default is already taken
/// by a binding that is not being moved.
pub(crate) fn migrate(bindings: &mut [HotkeyBinding], from_rev: u32) {
    if from_rev >= 1 || !cfg!(target_os = "macos") {
        return;
    }
    let stale: Vec<bool> =
        bindings.iter().map(|b| MAC_DEFAULTS_REV0.iter().any(|(accel, action)| *action == b.action && *accel == b.accel)).collect();
    let defaults = default_bindings();
    for i in (0..bindings.len()).filter(|&i| stale[i]) {
        let Some(new) = defaults.iter().find(|d| d.action == bindings[i].action) else { continue };
        let taken = bindings.iter().enumerate().any(|(j, other)| !stale[j] && same_accel(&other.accel, &new.accel));
        if !taken {
            bindings[i].accel = new.accel.clone();
        }
    }
}

/// Whether two accelerators name the same keys, in any order and letter case.
fn same_accel(a: &str, b: &str) -> bool {
    let parts = |s: &str| {
        let mut v: Vec<String> = s.split('+').map(|p| p.trim().to_ascii_lowercase()).collect();
        v.sort_unstable();
        v
    };
    parts(a) == parts(b)
}
