//! Privacy: what auto-redaction covers and how, metadata and context.

use egui::Ui;
use snap_core::settings::{PrivacySettings, RedactKind};
use snap_ui::{space, widgets};

use super::{
    kit::{Rows, s, wrap},
    pipelines::style_options,
};

pub fn show(ui: &mut Ui, rc: &mut Rows, p: &mut PrivacySettings) {
    let styles = style_options();
    let force = rc.env.policy.force_redact_before_upload;

    rc.section(ui, &s("privacy.section.redaction"), |ui, rc| {
        rc.stacked(ui, false, "privacy.redact_kinds", |ui| {
            wrap(ui, |ui| {
                ui.spacing_mut().item_spacing.x = space::XL;
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
        rc.segments(ui, "privacy.redact_style", &mut p.redact_style, &styles);
    });

    rc.section(ui, &s("privacy.section.files"), |ui, rc| {
        rc.toggle_locked(ui, force, "privacy.strip_metadata", &mut p.strip_metadata);
        rc.toggle(ui, "privacy.record_context", &mut p.record_context);
    });
}
