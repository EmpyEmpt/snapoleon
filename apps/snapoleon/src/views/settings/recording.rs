//! Recording: formats, frame rate, audio, overlays, encoders and ffmpeg.

use egui::Ui;
use snap_core::{
    settings::{EncoderPref, RecordingSettings, VideoFormat},
    t, tf,
};
use snap_ui::{Tone, space, text, widgets};

use super::kit::{self, Pick, Rows, managed_banner, opts, s};
use crate::views::ViewCx;

pub fn show(ui: &mut Ui, rc: &mut Rows, cx: &mut ViewCx, r: &mut RecordingSettings) {
    let locked = rc.env.policy.disable_recording;
    if locked {
        managed_banner(ui, &s("recording.disabled"));
    }
    ui.add_enabled_ui(!locked, |ui| body(ui, rc, cx, r));
}

fn body(ui: &mut Ui, rc: &mut Rows, cx: &mut ViewCx, r: &mut RecordingSettings) {
    let formats = opts(&[
        (VideoFormat::Mp4, "settings.recording.format.mp4"),
        (VideoFormat::Gif, "settings.recording.format.gif"),
        (VideoFormat::Webp, "settings.recording.format.webp"),
    ]);
    let encoders = opts(&[
        (EncoderPref::Auto, "settings.recording.encoder.auto"),
        (EncoderPref::Hardware, "settings.recording.encoder.hardware"),
        (EncoderPref::Software, "settings.recording.encoder.software"),
    ]);
    let fps = number_options(r.fps, &[15, 24, 30, 60], "settings.recording.fps_unit");
    let gif_fps = number_options(r.gif_fps, &[5, 10, 15, 20, 25], "settings.recording.fps_unit");
    let bitrate = bitrate_options(r.bitrate);
    let mics = mic_options(rc, &r.mic_device);
    let encoder_list = probe_encoders(rc);

    rc.section(ui, &s("recording.section.output"), |ui, rc| {
        rc.segments(ui, "recording.format", &mut r.format, &formats);
        rc.choice(ui, "recording.fps", &mut r.fps, &fps);
        rc.choice(ui, "recording.gif_fps", &mut r.gif_fps, &gif_fps);
        rc.choice(ui, "recording.bitrate", &mut r.bitrate, &bitrate);
        rc.number(ui, "recording.max_gif_width", &mut r.max_gif_width, 160..=3840, " px");
        rc.number(ui, "recording.countdown_secs", &mut r.countdown_secs, 0..=10, &t("settings.recording.secs_unit"));
    });

    rc.section(ui, &s("recording.section.audio"), |ui, rc| {
        rc.toggle(ui, "recording.system_audio", &mut r.system_audio);
        rc.toggle(ui, "recording.microphone", &mut r.microphone);
        rc.choice(ui, "recording.mic_device", &mut r.mic_device, &mics);
    });

    rc.section(ui, &s("recording.section.overlays"), |ui, rc| {
        rc.toggle(ui, "recording.show_cursor", &mut r.show_cursor);
        rc.toggle(ui, "recording.highlight_clicks", &mut r.highlight_clicks);
        rc.toggle(ui, "recording.show_keystrokes", &mut r.show_keystrokes);
    });

    rc.section(ui, &s("recording.section.encoding"), |ui, rc| {
        rc.segments(ui, "recording.encoder", &mut r.encoder, &encoders);
        rc.stacked(ui, false, "recording.encoders", |ui| match &encoder_list {
            None => {
                ui.horizontal(|ui| {
                    widgets::spinner(ui);
                    ui.label(text::dim(s("probing")));
                });
            }
            Some(list) if list.is_empty() => {
                ui.label(text::dim(s("recording.no_encoders")));
            }
            Some(list) => {
                for (name, hardware) in list {
                    ui.horizontal(|ui| {
                        ui.label(text::mono_text(name.as_str()));
                        let (label, tone) =
                            if *hardware { (s("recording.hardware"), Tone::Success) } else { (s("recording.software"), Tone::Neutral) };
                        widgets::badge(ui, &label, tone);
                    });
                }
            }
        });
        let ffmpeg_hint = s("recording.ffmpeg_hint");
        rc.row(ui, "recording.ffmpeg_path", |ui| {
            kit::path_field(ui, cx, &mut r.ffmpeg_path, &ffmpeg_hint, Pick::FfmpegPath, 320.0);
        });
        let found = rc.env.probe.as_ref().map(|p| p.ffmpeg.clone());
        rc.content(ui, |ui| match found {
            None => {
                ui.label(text::caption(s("probing")));
            }
            Some(Some(path)) => {
                ui.label(text::caption(tf("settings.recording.ffmpeg_found", &[("path", &path.display().to_string())])));
            }
            Some(None) => {
                ui.label(text::caption(s("recording.ffmpeg_missing")));
            }
        });
    });
    ui.add_space(space::XS);
}

/// Encoder names with their hardware flag, once the probe has run.
fn probe_encoders(rc: &Rows) -> Option<Vec<(String, bool)>> {
    rc.env.probe.as_ref().map(|p| p.encoders.iter().map(|e| (e.name.clone(), e.hardware)).collect())
}

/// Preset values, plus the current one when it is not among them.
fn number_options(current: u32, presets: &[u32], unit_key: &str) -> Vec<(u32, String)> {
    let mut values = presets.to_vec();
    if !values.contains(&current) {
        values.push(current);
        values.sort_unstable();
    }
    values.into_iter().map(|n| (n, tf(unit_key, &[("n", &n.to_string())]))).collect()
}

/// Automatic plus common MP4 bitrates in bits per second.
fn bitrate_options(current: u32) -> Vec<(u32, String)> {
    let mut values = vec![0, 2_000_000, 4_000_000, 8_000_000, 16_000_000, 32_000_000, 64_000_000];
    if !values.contains(&current) {
        values.push(current);
        values.sort_unstable();
    }
    values
        .into_iter()
        .map(|bps| {
            if bps == 0 {
                (0, s("recording.bitrate.auto"))
            } else {
                (bps, tf("settings.recording.bitrate_unit", &[("n", &format!("{:.1}", f64::from(bps) / 1_000_000.0).replace(".0", ""))]))
            }
        })
        .collect()
}

fn mic_options(rc: &Rows, current: &str) -> Vec<(String, String)> {
    let mut all = vec![(String::new(), s("recording.mic_default"))];
    if let Some(probe) = &rc.env.probe {
        all.extend(probe.mics.iter().map(|m| (m.clone(), m.clone())));
    }
    if !current.is_empty() && !all.iter().any(|(v, _)| v == current) {
        all.push((current.to_owned(), current.to_owned()));
    }
    all
}
