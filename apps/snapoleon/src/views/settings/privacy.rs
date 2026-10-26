//! Privacy: the macOS permissions, what auto-redaction covers and how, metadata and context.

use egui::Ui;
use snap_core::{
    Settings,
    settings::{PrivacySettings, RedactKind},
    t,
};
use snap_platform::permissions::{Perm, Status};
use snap_ui::{space, widgets};

use super::{
    kit::{Rows, s, wrap},
    pipelines::style_options,
};
use crate::views::{
    ViewCx,
    onboarding::perms::{self, Live},
};

pub fn show(ui: &mut Ui, rc: &mut Rows, cx: &mut ViewCx, set: &mut Settings) {
    if cfg!(target_os = "macos") {
        permissions(ui, rc, cx, set.recording.show_keystrokes);
    }
    redaction(ui, rc, &mut set.privacy);
}

/// Live status of every permission a feature in use needs, with the action that can still change it. Input
/// Monitoring is listed only while recordings show keystrokes, the one feature that needs it.
fn permissions(ui: &mut Ui, rc: &mut Rows, cx: &mut ViewCx, keystrokes: bool) {
    let id = egui::Id::unique("settings-privacy-permissions");
    let mut live = ui.data_mut(|d| *d.get_temp_mut_or_insert_with(id, Live::read));
    live.refresh(ui.ctx());
    ui.data_mut(|d| d.insert_temp(id, live));

    let wanted = [Perm::ScreenRecording, Perm::Accessibility, Perm::Microphone, Perm::InputMonitoring];
    rc.section(ui, &s("privacy.permissions.section"), |ui, rc| {
        for perm in wanted {
            if perm == Perm::InputMonitoring && !keystrokes {
                continue;
            }
            let (key, _) = perms::row_texts(perm);
            let label = t(&format!("onboarding.permissions.{key}.title"));
            let desc = t(&format!("onboarding.permissions.{key}.body"));
            let status = live.get(perm);
            let proxy = cx.proxy.clone();
            rc.row_raw(ui, false, &label, &desc, |ui| perms::actions(ui, &proxy, perm, status, perm == Perm::ScreenRecording, true));
        }
        if live.get(Perm::ScreenRecording) == Status::Denied {
            rc.content(ui, |ui| perms::relaunch_hint(ui, cx, false));
        }
    });
}

fn redaction(ui: &mut Ui, rc: &mut Rows, p: &mut PrivacySettings) {
    let styles = style_options();
    let force = rc.env.policy.force_redact_before_upload;

    rc.section(ui, &s("privacy.section.redaction"), |ui, rc| {
        rc.stacked(ui, false, "privacy.redact_kinds", |ui| {
            wrap(ui, |ui| {
                ui.spacing_mut().item_spacing.x = space::XL;
                // The policy decides what is covered before an upload.
                ui.add_enabled_ui(!force, |ui| {
                    for kind in RedactKind::ALL {
                        let mut on = p.redact_kinds.contains(kind);
                        if widgets::checkbox(ui, &mut on, &snap_redact::kind_label(*kind)).changed() {
                            if on {
                                p.redact_kinds.push(*kind);
                            } else {
                                p.redact_kinds.retain(|k| k != kind);
                            }
                        }
                    }
                });
            });
        });
        rc.segments(ui, "privacy.redact_style", &mut p.redact_style, &styles);
    });

    rc.section(ui, &s("privacy.section.files"), |ui, rc| {
        rc.toggle_locked(ui, force, "privacy.strip_metadata", &mut p.strip_metadata);
        rc.toggle(ui, "privacy.record_context", &mut p.record_context);
    });
}
