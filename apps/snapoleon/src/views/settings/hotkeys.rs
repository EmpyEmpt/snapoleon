//! Hotkeys: one row per action with a recorder, conflict detection (within the list and with the OS), the pipeline
//! each hotkey runs, reset.

use std::collections::HashMap;

use egui::{Margin, RichText, Ui};
use snap_core::{
    hotkey::{Action, HotkeyBinding, default_bindings},
    t, tf,
};
use snap_platform::hotkeys::{self, SystemConflict};
use snap_ui::{
    Palette, Tone, icon, space,
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

/// A recorder row: the action and which of its bindings (0 for an action that has none yet).
type Row = (Action, usize);

#[derive(Default)]
pub struct State {
    recorders: HashMap<Row, RecorderState>,
    /// Recorder text of actions that have no binding yet.
    pending: HashMap<Action, String>,
    /// Whether a recorder had focus last frame (global hotkeys are paused meanwhile).
    recording: bool,
    /// A shortcut just recorded that another binding already has. It is not saved unless the user moves it over.
    taken: Option<Taken>,
}

impl State {
    pub fn recording(&self) -> bool {
        self.recording
    }
}

struct Taken {
    row: Row,
    /// The binding of `row`, `None` when the action had none.
    index: Option<usize>,
    accel: String,
    /// The binding that has `accel`.
    owner: usize,
}

impl Taken {
    /// Still true of `bindings`: the owner kept the shortcut and the indices still point at the same bindings.
    fn holds(&self, bindings: &[HotkeyBinding]) -> bool {
        bindings.get(self.owner).is_some_and(|b| normalize(&b.accel) == normalize(&self.accel))
            && self.index.is_none_or(|i| bindings.get(i).is_some_and(|b| b.action == self.row.0))
    }

    fn message(&self, bindings: &[HotkeyBinding]) -> String {
        let owner = t(&format!("action.{}", bindings[self.owner].action.key()));
        tf("settings.hotkeys.conflicts.taken", &[("accel", &hotkeys::display(&self.accel)), ("action", &owner)])
    }
}

enum Problem {
    Duplicate(Action),
    Invalid(String),
    /// The OS reacts to the shortcut too; a warning, the binding still works.
    System(SystemConflict),
}

pub fn show(ui: &mut Ui, rc: &mut Rows, st: &mut State, cx: &mut ViewCx, set: &mut snap_core::Settings) {
    if st.taken.as_ref().is_some_and(|taken| !taken.holds(&set.hotkeys)) {
        st.taken = None;
    }
    let problems = analyse(&set.hotkeys);
    if !rc.searching() {
        system_summary(ui, cx, &set.hotkeys, &problems);
    }
    let mut focused = false;
    let mut removed = false;
    // (row, its binding, the shortcut): applied after the rows, where every binding can be looked at.
    let mut recorded: Option<(Row, Option<usize>, String)> = None;
    let mut replace = false;

    for (title, actions) in GROUPS {
        rc.section(ui, &s(title), |ui, rc| {
            for &action in actions {
                let label = t(&format!("action.{}", action.key())).into_owned();
                let indices: Vec<usize> = set.hotkeys.iter().enumerate().filter(|(_, b)| b.action == action).map(|(i, _)| i).collect();
                if indices.is_empty() {
                    let row = (action, 0);
                    let taken = st.taken.as_ref().filter(|taken| taken.row == row);
                    let desc = taken.map(|taken| taken.message(&set.hotkeys)).unwrap_or_default();
                    rc.row_raw(ui, false, &label, &desc, |ui| {
                        // Keep the recorders of unassigned actions aligned with the ones that have a switch.
                        ui.add_space(48.0);
                        let accel = st.pending.entry(action).or_default();
                        let state = st.recorders.entry(row).or_default();
                        let resp = hotkey_recorder(ui, accel, state);
                        focused |= resp.has_focus();
                        if resp.changed() && !accel.is_empty() {
                            recorded = Some((row, None, std::mem::take(accel)));
                        }
                        if taken.is_some() {
                            replace |= widgets::secondary_button(ui, &s("hotkeys.conflicts.replace")).clicked();
                        }
                    });
                    continue;
                }
                for (nth, &i) in indices.iter().enumerate() {
                    let row = (action, nth);
                    let problem = problems.get(i).and_then(Option::as_ref);
                    let taken = st.taken.as_ref().filter(|taken| taken.row == row);
                    let desc = taken.map(|taken| taken.message(&set.hotkeys)).unwrap_or_default();
                    let binding = &mut set.hotkeys[i];
                    rc.row_raw(ui, false, &label, &desc, |ui| {
                        widgets::toggle(ui, &mut binding.enabled).on_hover_text(s("hotkeys.enabled"));
                        let state = st.recorders.entry(row).or_default();
                        state.invalid = binding.enabled && matches!(problem, Some(Problem::Duplicate(_) | Problem::Invalid(_)));
                        // The recorder writes into a copy: a shortcut another binding has is only kept on "Replace".
                        let mut accel = binding.accel.clone();
                        let resp = hotkey_recorder(ui, &mut accel, state);
                        focused |= resp.has_focus();
                        if resp.changed() {
                            if accel.trim().is_empty() {
                                binding.accel.clear();
                            } else {
                                recorded = Some((row, Some(i), accel));
                            }
                        }
                        if binding.accel.trim().is_empty() {
                            removed = true;
                        }
                        if taken.is_some() {
                            replace |= widgets::secondary_button(ui, &s("hotkeys.conflicts.replace")).clicked();
                        } else if binding.enabled
                            && let Some(problem) = problem
                        {
                            problem_badge(ui, problem);
                        }
                    });
                }
            }
        });
    }
    if let Some((row, index, accel)) = recorded {
        record(st, &mut set.hotkeys, row, index, accel);
    }
    if replace && let Some(taken) = st.taken.take() {
        // The shortcut moves: its old binding is emptied, which removes it below like a cleared one.
        set.hotkeys[taken.owner].accel.clear();
        match taken.index {
            Some(i) => set.hotkeys[i].accel = taken.accel,
            None => set.hotkeys.push(HotkeyBinding { accel: taken.accel, action: taken.row.0, pipeline: None, enabled: true }),
        }
        removed = true;
    }
    if removed {
        set.hotkeys.retain(|b| !b.accel.trim().is_empty());
    }

    pipeline_section(ui, rc, set);

    rc.section(ui, &s("hotkeys.section.reset"), |ui, rc| {
        rc.row(ui, "hotkeys.reset", |ui| {
            if kit::confirm_button(ui, "reset-hotkeys", &s("hotkeys.reset.button"), &s("hotkeys.reset.confirm")) {
                set.hotkeys = default_bindings();
                st.taken = None;
            }
        });
    });

    if focused != st.recording {
        st.recording = focused;
        cx.push(Command::PauseHotkeys(focused));
    }
}

/// Saves a recorded shortcut, unless another binding already has it: then it waits in `st.taken` for "Replace".
fn record(st: &mut State, bindings: &mut Vec<HotkeyBinding>, row: Row, index: Option<usize>, accel: String) {
    st.taken = None;
    let key = normalize(&accel);
    let owner = bindings.iter().enumerate().position(|(j, b)| Some(j) != index && normalize(&b.accel) == key);
    match (owner, index) {
        (Some(owner), _) => st.taken = Some(Taken { row, index, accel, owner }),
        (None, Some(i)) => bindings[i].accel = accel,
        (None, None) => bindings.push(HotkeyBinding { accel, action: row.0, pipeline: None, enabled: true }),
    }
}

/// Warning above the groups when enabled bindings are also system shortcuts, with a way to the OS setting.
fn system_summary(ui: &mut Ui, cx: &mut ViewCx, bindings: &[HotkeyBinding], problems: &[Option<Problem>]) {
    let mut lines = Vec::new();
    let mut first = None;
    for (b, problem) in bindings.iter().zip(problems) {
        if let Some(Problem::System(conflict)) = problem {
            let action = t(&format!("action.{}", b.action.key()));
            lines.push(tf("settings.hotkeys.conflicts.system_line", &[("action", &action), ("message", &conflict.message)]));
            first.get_or_insert(conflict);
        }
    }
    if let Some(conflict) = first {
        system_card(ui, cx, &lines, conflict);
        ui.add_space(space::LG);
    }
}

/// The same warning for a single shortcut, for the onboarding's hotkey card.
pub fn system_warning(ui: &mut Ui, cx: &mut ViewCx, accel: &str) {
    if let Some(conflict) = hotkeys::system_conflict(accel) {
        system_card(ui, cx, std::slice::from_ref(&conflict.message), &conflict);
    }
}

fn system_card(ui: &mut Ui, cx: &mut ViewCx, lines: &[String], conflict: &SystemConflict) {
    let p = Palette::of(ui);
    egui::Frame::new().fill(p.tone_soft(Tone::Warning)).corner_radius(8).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            ui.label(RichText::new(icon::WARNING).color(p.warning));
            ui.vertical(|ui| {
                for line in lines {
                    ui.add(egui::Label::new(RichText::new(line).color(p.text)).wrap());
                }
                ui.add_space(space::XS);
                if widgets::secondary_button(ui, &conflict.open_label).clicked() {
                    cx.push(Command::OpenUrl(conflict.settings_url.to_owned()));
                }
            });
        });
    });
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
            let label = format!("{}  ({})", t(&format!("action.{}", binding.action.key())), hotkeys::display(&binding.accel));
            rc.row_raw(ui, false, &label, "", |ui| {
                dd(ui, ("hotkey-pipeline", i), &mut binding.pipeline, &options);
            });
        }
    });
}

/// Unparsable accelerators, duplicates among the enabled bindings, and enabled bindings the OS also uses.
fn analyse(bindings: &[HotkeyBinding]) -> Vec<Option<Problem>> {
    let normalized: Vec<String> = bindings.iter().map(|b| normalize(&b.accel)).collect();
    bindings
        .iter()
        .enumerate()
        .map(|(i, b)| {
            if b.accel.trim().is_empty() {
                return None;
            }
            if let Err(e) = hotkeys::validate(&b.accel) {
                return Some(Problem::Invalid(e.to_string()));
            }
            if !b.enabled {
                return None;
            }
            bindings
                .iter()
                .enumerate()
                .find(|(j, other)| *j != i && other.enabled && normalized[*j] == normalized[i])
                .map(|(_, other)| Problem::Duplicate(other.action))
                .or_else(|| hotkeys::system_conflict(&b.accel).map(Problem::System))
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
            badge(ui, &s("hotkeys.conflict"), Tone::Danger).on_hover_text(tf("settings.hotkeys.conflict_with", &[("action", &other)]));
        }
        Problem::Invalid(reason) => {
            badge(ui, &s("hotkeys.invalid"), Tone::Warning).on_hover_text(reason);
        }
        Problem::System(conflict) => {
            badge(ui, &s("hotkeys.conflicts.system"), Tone::Warning).on_hover_text(&conflict.message);
        }
    }
}
