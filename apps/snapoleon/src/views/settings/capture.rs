//! Capture: output format and folder, file names, the selection overlay, capture behaviour, the editor.

use egui::Ui;
use snap_core::{
    CaptureKind,
    naming::{self, NameContext},
    paths,
    settings::{HdrMode, ImageFormat},
};
use snap_ui::{Tone, icon, space, widgets};

use super::kit::{self, Pick, Rows, color_well, field_mono, opts, s, wrap};
use crate::{commands::Command, views::ViewCx};

/// Template tokens offered as chips; the order follows how often they are wanted.
const TOKENS: [&str; 15] =
    ["{yyyy}", "{MM}", "{dd}", "{HH}", "{mm}", "{ss}", "{ms}", "{unix}", "{app}", "{title}", "{kind}", "{n}", "{w}", "{h}", "{rand}"];

pub fn show(ui: &mut Ui, rc: &mut Rows, cx: &mut ViewCx, set: &mut snap_core::Settings) {
    let formats = opts(&[
        (ImageFormat::Png, "settings.capture.format.png"),
        (ImageFormat::Jpeg, "settings.capture.format.jpeg"),
        (ImageFormat::Webp, "settings.capture.format.webp"),
    ]);
    let hdr = opts(&[(HdrMode::Auto, "settings.capture.hdr.auto"), (HdrMode::Off, "settings.capture.hdr.off")]);
    let c = &mut set.capture;

    rc.section(ui, &s("capture.section.output"), |ui, rc| {
        rc.segments(ui, "capture.format", &mut c.format, &formats);
        rc.percent(ui, "capture.jpeg_quality", &mut c.jpeg_quality, 10..=100);
        rc.percent(ui, "capture.webp_quality", &mut c.webp_quality, 10..=100);
        rc.toggle(ui, "capture.optimize_png", &mut c.optimize_png);
        let default_dir = paths::default_save_dir().display().to_string();
        rc.row(ui, "capture.save_dir", |ui| {
            if widgets::ghost_button(ui, &s("open")).clicked() {
                cx.push(Command::OpenPath(kit::ensure_save_dir(&c.save_dir)));
            }
            kit::path_field(ui, cx, &mut c.save_dir, &default_dir, Pick::SaveDir, 300.0);
        });
        rc.stacked(ui, false, "capture.filename_template", |ui| template_editor(ui, &mut c.filename_template, c.format));
    });

    rc.section(ui, &s("capture.section.overlay"), |ui, rc| {
        rc.toggle(ui, "capture.show_magnifier", &mut c.show_magnifier);
        rc.number(ui, "capture.magnifier_zoom", &mut c.magnifier_zoom, 2..=16, "×");
        rc.toggle(ui, "capture.show_crosshair", &mut c.show_crosshair);
        rc.toggle(ui, "capture.dim_outside", &mut c.dim_outside);
        rc.toggle(ui, "capture.snap_to_windows", &mut c.snap_to_windows);
    });

    rc.section(ui, &s("capture.section.behavior"), |ui, rc| {
        rc.toggle(ui, "capture.include_cursor", &mut c.include_cursor);
        rc.toggle(ui, "capture.hide_desktop_icons", &mut c.hide_desktop_icons);
        rc.number(ui, "capture.delay_secs", &mut c.delay_secs, 0..=60, &snap_core::t("settings.recording.secs_unit"));
        rc.segments(ui, "capture.hdr", &mut c.hdr, &hdr);
        rc.toggle(ui, "capture.window_shadow", &mut c.window_shadow);
    });

    let e = &mut set.editor;
    rc.section(ui, &s("capture.section.editor"), |ui, rc| {
        rc.stacked(ui, false, "editor.palette", |ui| palette_editor(ui, &mut e.palette));
        rc.row(ui, "editor.stroke_width", |ui| {
            ui.add(widgets::Slider::new(&mut e.stroke_width, 1.0..=24.0).step(1.0).width(160.0).suffix(" px"));
        });
        rc.number(ui, "editor.font_size", &mut e.font_size, 8.0..=160.0, " px");
        rc.toggle(ui, "editor.readable_downscale", &mut e.readable_downscale);
        rc.number(ui, "editor.max_export_width", &mut e.max_export_width, 0..=16384, " px");
        rc.number(ui, "editor.step_badge_start", &mut e.step_badge_start, 0..=99, "");
    });
}

/// Template field, a live preview with a sample capture, and token chips that append to the template.
fn template_editor(ui: &mut Ui, template: &mut String, format: ImageFormat) {
    let width = ui.available_width();
    field_mono(ui, template, "{yyyy}-{MM}-{dd}", width);
    let mut sample = NameContext::now();
    sample.app = Some("Firefox");
    sample.title = Some("Snapoleon");
    sample.kind = Some(CaptureKind::Region);
    sample.counter = 1;
    sample.size = Some((1920, 1080));
    let preview = format!("{}.{}", naming::expand(template, &sample), format.ext());
    ui.horizontal(|ui| {
        ui.label(snap_ui::text::caption(s("capture.preview")));
        ui.label(snap_ui::text::mono_text(preview.as_str()));
        if preview.contains('{') {
            widgets::badge(ui, &s("capture.unknown_token"), Tone::Warning);
        }
    });
    ui.add_space(space::XS);
    wrap(ui, |ui| {
        for token in TOKENS {
            if widgets::chip(ui, token, false).on_hover_text(s("capture.add_token")).clicked() {
                template.push_str(token);
            }
        }
    });
}

/// The editor palette as a row of wells with add, remove and reset.
fn palette_editor(ui: &mut Ui, palette: &mut Vec<String>) {
    wrap(ui, |ui| {
        for hex in palette.iter_mut() {
            color_well(ui, hex, [255, 59, 48, 255], true);
        }
        if widgets::icon_button(ui, icon::PLUS, &s("editor.palette.add"), None).clicked() && palette.len() < 16 {
            palette.push("#FF3B30FF".into());
        }
        if widgets::icon_button(ui, icon::MINUS, &s("editor.palette.remove"), None).clicked() && palette.len() > 2 {
            palette.pop();
        }
        if widgets::icon_button(ui, icon::ARROW_COUNTER_CLOCKWISE, &s("editor.palette.reset"), None).clicked() {
            *palette = snap_core::settings::EditorSettings::default().palette;
        }
    });
}
