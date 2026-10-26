//! General: language, theme, accent, startup, notifications, tray.

use std::time::{Duration, Instant};

use egui::Ui;
use snap_core::{
    hotkey::Action,
    i18n,
    settings::{Theme, parse_hex_color, to_hex_color},
    t,
};
use snap_platform::autostart;
use snap_ui::{
    Palette, ThemeSpec,
    widgets::{self, ColorSwatches},
};

use super::kit::{self, Rows, opts, s};
use crate::{commands::Command, views::ViewCx};

#[derive(Default)]
pub struct State {
    languages: Option<Vec<(String, String)>>,
    /// The OS's view of the login item and when it was read.
    login_item: Option<(Instant, autostart::Status)>,
}

impl State {
    /// Re-read every few seconds: the user can change it in the system settings while this page is open.
    fn login_item(&mut self) -> autostart::Status {
        if self.login_item.is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(2)) {
            self.login_item = Some((Instant::now(), autostart::status()));
        }
        self.login_item.map_or(autostart::Status::Off, |(_, status)| status)
    }
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
    let needs_approval = !rc.env.portable && st.login_item() == autostart::Status::NeedsApproval;
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
        let mut scale = g.interface_scale();
        let offered = scales(scale);
        rc.choice(ui, "general.ui_scale", &mut scale, &offered);
        if scale != g.interface_scale() {
            g.ui_scale = scale;
        }
        rc.toggle(ui, "general.reduce_motion", &mut g.reduce_motion);
    });

    rc.section(ui, &s("general.section.behavior"), |ui, rc| {
        rc.row(ui, if portable { "general.launch_portable" } else { "general.launch_at_login" }, |ui| {
            if portable {
                // Disabled and shown off: the stored preference still applies to an installed copy.
                ui.add_enabled_ui(false, |ui| widgets::toggle(ui, &mut false));
            } else {
                widgets::toggle(ui, &mut g.launch_at_login);
            }
        });
        if needs_approval && g.launch_at_login {
            rc.row(ui, "general.login_item_approval", |ui| {
                if widgets::secondary_button(ui, &s("general.login_item_approval.button")).clicked() {
                    kit::report(cx, autostart::open_login_items());
                }
            });
        }
        rc.toggle(ui, "general.instant_overlay", &mut g.instant_overlay);
        // macOS menu bar convention: any click on the icon opens the menu, so there is no click action to choose.
        if !cfg!(target_os = "macos") {
            rc.choice(ui, "general.tray_click", &mut g.tray_click, &tray_actions);
        }
        rc.toggle(ui, "general.show_notifications", &mut g.show_notifications);
        if g.show_notifications && snap_platform::notify::blocked() {
            rc.row(ui, "general.notifications_blocked", |ui| {
                if widgets::secondary_button(ui, &s("general.notifications_blocked.button")).clicked() {
                    kit::report(cx, snap_platform::notify::open_settings());
                }
            });
        }
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

/// The interface sizes offered, plus `current` when the keyboard zoom (Cmd+= / Cmd+-) left it between them.
fn scales(current: f32) -> Vec<(f32, String)> {
    let mut values = vec![0.8, 0.9, 1.0, 1.1, 1.25, 1.5];
    if !values.iter().any(|v| (v - current).abs() < 0.001) {
        values.push(current);
        values.sort_by(f32::total_cmp);
    }
    values.into_iter().map(|v| (v, format!("{} %", (v * 100.0).round()))).collect()
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
