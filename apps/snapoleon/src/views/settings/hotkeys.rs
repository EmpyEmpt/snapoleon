//! Hotkeys: one row per action with a recorder, conflict detection, the pipeline each hotkey runs, reset.

use std::collections::HashMap;

use egui::Ui;
use snap_core::{
    hotkey::{Action, HotkeyBinding, default_bindings},
    t,
};
use snap_ui::{
    Tone,
    widgets::{self, RecorderState, badge, hotkey_recorder},
};

use super::kit::{self, Rows, dd, s};
use crate::{commands::Command, views::ViewCx};

/// How the actions are grouped on the page.
const GROUPS: [(&str, &[Action]); 5] = [
    (
        "hotkeys.group.capture",
        &[
            Action::CaptureRegion,
            Action::CaptureWindow,
            Action::CaptureElement,
            Action::CaptureMonitor,
            Action::CaptureAllMonitors,
            Action::CaptureScrolling,
            Action::CaptureLastRegion,
            Action::CaptureDelayed,
        ],
    ),
    ("hotkeys.group.record", &[Action::RecordRegion, Action::RecordScreen, Action::RecordGif, Action::StopRecording]),
    ("hotkeys.group.text", &[Action::OcrRegion, Action::DecodeCodesRegion, Action::TranslateRegion, Action::ColorPicker, Action::Measure]),
    ("hotkeys.group.pin", &[Action::PinRegion, Action::PinClipboard, Action::StepCaptureToggle, Action::OpenEditorFromClipboard]),
    ("hotkeys.group.app", &[Action::OpenLibrary, Action::OpenSettings]),
];

#[derive(Default)]
pub struct State {
    recorders: HashMap<(Action, usize), RecorderState>,
    /// Recorder text of actions that have no binding yet.
    pending: HashMap<Action, String>,
    /// Whether a recorder had focus last frame (global hotkeys are paused meanwhile).
    recording: bool,
}

impl State {
    pub fn recording(&self) -> bool {
        self.recording
    }
}

enum Problem {
    Duplicate(Action),
    Invalid(String),
}

pub fn show(ui: &mut Ui, rc: &mut Rows, st: &mut State, cx: &mut ViewCx, set: &mut snap_core::Settings) {
    let problems = analyse(&set.hotkeys);
    let mut focused = false;
    let mut removed = false;

    for (title, actions) in GROUPS {
        rc.section(ui, &s(title), |ui, rc| {
            for &action in actions {
                let label = t(&format!("action.{}", action.key())).into_owned();
                let indices: Vec<usize> = set.hotkeys.iter().enumerate().filter(|(_, b)| b.action == action).map(|(i, _)| i).collect();
                if indices.is_empty() {
                    rc.row_raw(ui, false, &label, "", |ui| {
                        // Keep the recorders of unassigned actions aligned with the ones that have a switch.
                        ui.add_space(48.0);
                        let accel = st.pending.entry(action).or_default();
                        let state = st.recorders.entry((action, 0)).or_default();
                        let resp = hotkey_recorder(ui, accel, state);
                        focused |= resp.has_focus();
                        if resp.changed() && !accel.is_empty() {
                            let accel = std::mem::take(accel);
                            set.hotkeys.push(HotkeyBinding { accel, action, pipeline: None, enabled: true });
                        }
                    });
                    continue;
                }
                for (nth, &i) in indices.iter().enumerate() {
                    let problem = problems.get(i).and_then(Option::as_ref);
                    let binding = &mut set.hotkeys[i];
                    rc.row_raw(ui, false, &label, "", |ui| {
                        widgets::toggle(ui, &mut binding.enabled).on_hover_text(s("hotkeys.enabled"));
                        let state = st.recorders.entry((action, nth)).or_default();
                        state.invalid = problem.is_some() && binding.enabled;
                        let resp = hotkey_recorder(ui, &mut binding.accel, state);
                        focused |= resp.has_focus();
                        if binding.accel.trim().is_empty() {
                            removed = true;
                        }
                        if binding.enabled
                            && let Some(problem) = problem
                        {
                            problem_badge(ui, problem);
                        }
                    });
                }
            }
        });
    }
    if removed {
        set.hotkeys.retain(|b| !b.accel.trim().is_empty());
    }

    pipeline_section(ui, rc, set);

    rc.section(ui, &s("hotkeys.section.reset"), |ui, rc| {
        rc.row(ui, "hotkeys.reset", |ui| {
            if kit::confirm_button(ui, "reset-hotkeys", &s("hotkeys.reset.button"), &s("hotkeys.reset.confirm")) {
                set.hotkeys = default_bindings();
            }
        });
    });

    if focused != st.recording {
        st.recording = focused;
        cx.push(Command::PauseHotkeys(focused));
    }
}

/// Which pipeline each bound capture hotkey runs.
fn pipeline_section(ui: &mut Ui, rc: &mut Rows, set: &mut snap_core::Settings) {
    let mut options: Vec<(Option<String>, String)> = vec![(None, s("hotkeys.pipeline.default"))];
    options.extend(set.pipelines.iter().map(|p| (Some(p.id.clone()), super::pipeline_label(p))));
    let bound: Vec<usize> =
        set.hotkeys.iter().enumerate().filter(|(_, b)| b.action.is_capture() && !b.accel.is_empty()).map(|(i, _)| i).collect();
    if bound.is_empty() {
        return;
    }
    rc.section(ui, &s("hotkeys.section.pipelines"), |ui, rc| {
        for &i in &bound {
            let binding = &mut set.hotkeys[i];
            let label =
                format!("{}  ({})", t(&format!("action.{}", binding.action.key())), snap_platform::hotkeys::display(&binding.accel));
            rc.row_raw(ui, false, &label, "", |ui| {
                dd(ui, ("hotkey-pipeline", i), &mut binding.pipeline, &options);
            });
        }
    });
}

/// Duplicate and unparsable accelerators among the enabled bindings.
fn analyse(bindings: &[HotkeyBinding]) -> Vec<Option<Problem>> {
    let normalized: Vec<String> = bindings.iter().map(|b| normalize(&b.accel)).collect();
    bindings
        .iter()
        .enumerate()
        .map(|(i, b)| {
            if b.accel.trim().is_empty() {
                return None;
            }
            if let Err(e) = snap_platform::hotkeys::validate(&b.accel) {
                return Some(Problem::Invalid(e.to_string()));
            }
            bindings
                .iter()
                .enumerate()
                .find(|(j, other)| *j != i && b.enabled && other.enabled && normalized[*j] == normalized[i])
                .map(|(_, other)| Problem::Duplicate(other.action))
        })
        .collect()
}

/// Order-independent, alias-free form of an accelerator, for comparing.
fn normalize(accel: &str) -> String {
    let mut mods = Vec::new();
    let mut key = String::new();
    for part in accel.split('+').map(|p| p.trim().to_lowercase()) {
        let m = match part.as_str() {
            "ctrl" | "control" => "ctrl",
            "alt" | "option" => "alt",
            "shift" => "shift",
            "cmd" | "super" | "win" | "meta" | "command" => "super",
            "cmdorctrl" | "commandorcontrol" => {
                if cfg!(target_os = "macos") {
                    "super"
                } else {
                    "ctrl"
                }
            }
            _ => {
                key = part;
                continue;
            }
        };
        mods.push(m);
    }
    mods.sort_unstable();
    mods.dedup();
    format!("{}+{key}", mods.join("+"))
}

fn problem_badge(ui: &mut Ui, problem: &Problem) {
    match problem {
        Problem::Duplicate(other) => {
            let other = t(&format!("action.{}", other.key()));
            badge(ui, &s("hotkeys.conflict"), Tone::Danger)
                .on_hover_text(snap_core::tf("settings.hotkeys.conflict_with", &[("action", &other)]));
        }
        Problem::Invalid(reason) => {
            badge(ui, &s("hotkeys.invalid"), Tone::Warning).on_hover_text(reason);
        }
    }
}
