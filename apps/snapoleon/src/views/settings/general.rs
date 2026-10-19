//! General: language, theme, accent, startup, notifications, tray.

use egui::Ui;
use snap_core::{
    hotkey::Action,
    i18n,
    settings::{Theme, parse_hex_color, to_hex_color},
    t,
};
use snap_ui::{
    Palette, ThemeSpec,
    widgets::{self, ColorSwatches},
};

use super::kit::{self, Rows, opts, s};
use crate::{commands::Command, views::ViewCx};

#[derive(Default)]
pub struct State {
    languages: Option<Vec<(String, String)>>,
}

/// Accent swatches after the brand blue: calm, saturated, readable on both themes.
const ACCENTS: [[u8; 4]; 7] = [
    [0x8E, 0x5B, 0xFF, 0xFF],
    [0xE0, 0x4F, 0x8B, 0xFF],
    [0xE5, 0x48, 0x4D, 0xFF],
    [0xF0, 0x8A, 0x24, 0xFF],
    [0x2A, 0x9D, 0x62, 0xFF],
    [0x12, 0xA5, 0xA5, 0xFF],
    [0x4A, 0x90, 0xE2, 0xFF],
];

pub fn show(ui: &mut Ui, rc: &mut Rows, st: &mut State, cx: &mut ViewCx, set: &mut snap_core::Settings) {
    let languages = st.languages.get_or_insert_with(|| {
        let mut all = vec![(String::new(), s("general.language.system"))];
        all.extend(i18n::available_languages());
        all
    });
    let themes = opts(&[
        (Theme::System, "settings.general.theme.system"),
        (Theme::Dark, "settings.general.theme.dark"),
        (Theme::Light, "settings.general.theme.light"),
    ]);
    let tray_actions: Vec<(Action, String)> = Action::ALL.iter().map(|a| (*a, t(&format!("action.{}", a.key())).into_owned())).collect();
    let language_locked = rc.env.policy.language.is_some();
    let portable = rc.env.portable;
    let g = &mut set.general;

    rc.section(ui, &s("general.section.appearance"), |ui, rc| {
        rc.choice_locked(ui, language_locked, "general.language", &mut g.language, languages);
        rc.segments(ui, "general.theme", &mut g.theme, &themes);
        rc.row(ui, "general.accent", |ui| accent_picker(ui, &mut g.accent));
        rc.toggle(ui, "general.reduce_motion", &mut g.reduce_motion);
    });

    rc.section(ui, &s("general.section.behavior"), |ui, rc| {
        rc.row(ui, if portable { "general.launch_portable" } else { "general.launch_at_login" }, |ui| {
            ui.add_enabled_ui(!portable, |ui| widgets::toggle(ui, &mut g.launch_at_login));
        });
        rc.choice(ui, "general.tray_click", &mut g.tray_click, &tray_actions);
        rc.toggle(ui, "general.show_notifications", &mut g.show_notifications);
        rc.toggle(ui, "general.play_sounds", &mut g.play_sounds);
    });

    rc.section(ui, &s("general.section.help"), |ui, rc| {
        rc.row(ui, "general.welcome", |ui| {
            if widgets::secondary_button(ui, &s("general.welcome.button")).clicked() {
                cx.push(Command::OpenOnboarding);
            }
        });
        rc.row(ui, "general.about", |ui| {
            if widgets::secondary_button(ui, &s("general.about.button")).clicked() {
                cx.push(Command::OpenAbout);
            }
        });
    });
}

/// Brand blue, a handful of alternatives and a custom well. An empty setting means the brand accent.
fn accent_picker(ui: &mut Ui, accent: &mut String) {
    let dark = Palette::of(ui).dark;
    let brand = ThemeSpec::brand(dark).accent;
    let brand = [brand.r(), brand.g(), brand.b(), 255];
    let mut colors = vec![brand];
    colors.extend(ACCENTS);
    let mut color = parse_hex_color(accent).unwrap_or(brand);
    color[3] = 255;
    // Eight swatches and the custom well, 28 px each.
    kit::ltr(ui, 28.0 * (colors.len() as f32 + 1.0), |ui| {
        if ui.add(ColorSwatches::new(&mut color, &colors).alpha(false)).changed() {
            color[3] = 255;
            *accent = if color == brand { String::new() } else { to_hex_color(color) };
        }
    });
}
