//! History: whether the library records, how long it keeps entries, and clearing it.

use std::sync::Arc;

use egui::Ui;
use snap_core::{settings::HistorySettings, tf};
use snap_history::{History, Query};
use snap_ui::{ToastKind, text, widgets};

use super::kit::{self, Msg, Rows, managed_banner, opts, s};
use crate::views::ViewCx;

#[derive(Default)]
pub struct State {
    clearing: bool,
}

impl State {
    pub fn on_msg(&mut self, cx: &mut ViewCx, msg: Msg) -> Option<Msg> {
        match msg {
            Msg::HistoryCleared(result) => {
                self.clearing = false;
                match result {
                    Ok(n) => cx.toast(ToastKind::Success, tf("settings.history.cleared", &[("n", &n.to_string())])),
                    Err(e) => cx.toast(ToastKind::Error, e),
                }
                None
            }
            other => Some(other),
        }
    }
}

pub fn show(ui: &mut Ui, rc: &mut Rows, st: &mut State, cx: &mut ViewCx, h: &mut HistorySettings) {
    let policy = &rc.env.policy;
    let disabled_by_policy = policy.disable_history;
    let retention_locked = policy.retention_days.is_some();
    let retention = retention_options(h.retention_days);
    let count = rc.env.probe.as_ref().and_then(|p| p.history_count);

    if disabled_by_policy {
        managed_banner(ui, &s("history.disabled"));
    }
    rc.section(ui, &s("history.section.library"), |ui, rc| {
        rc.toggle_locked(ui, disabled_by_policy, "history.enabled", &mut h.enabled);
        rc.choice_locked(ui, retention_locked, "history.retention_days", &mut h.retention_days, &retention);
        rc.toggle(ui, "history.index_text", &mut h.index_text);
    });

    rc.section(ui, &s("history.section.data"), |ui, rc| {
        if let Some(n) = count {
            rc.content(ui, |ui| {
                ui.label(text::dim(tf("settings.history.count", &[("n", &n.to_string())])));
            });
        }
        rc.row(ui, "history.clear", |ui| {
            if st.clearing {
                widgets::spinner(ui);
            } else if kit::confirm_button(ui, "clear-history", &s("history.clear.button"), &s("history.clear.confirm")) {
                match cx.engine.history() {
                    Some(history) => {
                        st.clearing = true;
                        cx.spawn(move || Msg::HistoryCleared(clear_all(&history)));
                    }
                    None => cx.toast(ToastKind::Info, s("history.off")),
                }
            }
        });
    });
}

/// Forever, then common spans; the current value is added when it is none of them.
fn retention_options(current: u32) -> Vec<(u32, String)> {
    let mut all = opts(&[
        (0, "settings.history.retention.forever"),
        (30, "settings.history.retention.month"),
        (90, "settings.history.retention.quarter"),
        (180, "settings.history.retention.half_year"),
        (365, "settings.history.retention.year"),
    ]);
    if !all.iter().any(|(days, _)| *days == current) {
        all.push((current, tf("settings.history.retention.custom", &[("n", &current.to_string())])));
    }
    all
}

/// Remove every entry from the library. The image files stay where they are.
fn clear_all(history: &Arc<History>) -> Result<usize, String> {
    let mut removed = 0;
    loop {
        let page = history.search(&Query { limit: 500, ..Query::default() }).map_err(|e| e.to_string())?;
        if page.is_empty() {
            return Ok(removed);
        }
        for entry in &page {
            history.delete(&entry.id, false).map_err(|e| e.to_string())?;
            removed += 1;
        }
    }
}
