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

pub fn default_bindings() -> Vec<HotkeyBinding> {
    let b = |accel: &str, action: Action| HotkeyBinding { accel: accel.into(), action, pipeline: None, enabled: true };
    if cfg!(target_os = "macos") {
        vec![
            b("Cmd+Shift+4", Action::CaptureRegion),
            b("Cmd+Shift+5", Action::CaptureWindow),
            b("Cmd+Shift+3", Action::CaptureAllMonitors),
            b("Cmd+Shift+6", Action::RecordRegion),
            b("Cmd+Shift+7", Action::StopRecording),
            b("Cmd+Shift+2", Action::OcrRegion),
            b("Cmd+Shift+1", Action::PinRegion),
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
