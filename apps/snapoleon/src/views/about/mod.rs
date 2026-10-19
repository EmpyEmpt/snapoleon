//! About: the brand mark, version, license, project links and the third-party credits.

use egui::{Align, Context, Layout, ScrollArea, Sense, Ui, Widget, vec2};
use snap_core::{VERSION, t, tf};
use snap_ui::{
    ToastKind, icon, logo, space, text,
    widgets::{self, Button, ButtonKind, section, setting_row},
};

use super::{Chrome, OpenCx, View, ViewCx, window_spec};
use crate::commands::Command;

pub const TAG: &str = "about";

/// The project page, taken from the package metadata.
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const LICENSE: &str = env!("CARGO_PKG_LICENSE");

/// Third-party work that ships inside the app: (name, license, home page).
const CREDITS: [(&str, &str, &str); 8] = [
    ("Inter", "SIL OFL 1.1", "https://rsms.me/inter/"),
    ("JetBrains Mono", "SIL OFL 1.1", "https://www.jetbrains.com/lp/mono/"),
    ("Phosphor Icons", "MIT", "https://phosphoricons.com/"),
    ("egui", "MIT OR Apache-2.0", "https://github.com/emilk/egui"),
    ("winit", "Apache-2.0", "https://github.com/rust-windowing/winit"),
    ("wgpu", "MIT OR Apache-2.0", "https://wgpu.rs/"),
    ("tiny-skia", "BSD-3-Clause", "https://github.com/linebender/tiny-skia"),
    ("SQLite", "Public domain", "https://www.sqlite.org/"),
];

pub fn open(cx: &mut OpenCx) {
    if cx.focus_tagged(TAG) {
        return;
    }
    cx.open(window_spec(t("about.title"), TAG, (440.0, 600.0), (380.0, 440.0)), AboutView);
}

struct AboutView;

impl View for AboutView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        let title = t("about.title");
        Chrome::new(&title).no_maximize().show(cx, ctx, |ui, cx| {
            ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                let width = (ui.available_width() - 2.0 * space::XL).clamp(280.0, 440.0);
                ui.vertical_centered(|ui| {
                    ui.add_space(space::XL);
                    ui.allocate_ui_with_layout(vec2(width, 0.0), Layout::top_down(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing = vec2(space::SM, space::SM);
                        hero(ui);
                        links(ui, cx);
                        ui.add_space(space::LG);
                        ui.allocate_ui_with_layout(vec2(width, 0.0), Layout::top_down(Align::Min), |ui| body(ui, cx));
                    });
                    ui.add_space(space::XL);
                });
            });
        });
    }
}

fn hero(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(88.0, 88.0), Sense::hover());
    logo(&ui.painter_at(rect), rect);
    ui.add_space(space::XS);
    ui.label(text::headline("Snapoleon"));
    ui.label(text::mono_text(tf("about.version", &[("version", VERSION)])));
    ui.label(text::caption(format!("{} · {}", std::env::consts::OS, std::env::consts::ARCH)));
    ui.add_space(space::XS);
    ui.add(egui::Label::new(text::dim(t("about.tagline"))).halign(Align::Center));
}

fn links(ui: &mut Ui, cx: &mut ViewCx) {
    ui.add_space(space::SM);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = space::SM;
        if Button::new(&t("about.repository")).kind(ButtonKind::Secondary).icon(icon::CODE).ui(ui).clicked() {
            cx.push(Command::OpenUrl(REPOSITORY.to_owned()));
        }
        if Button::new(&t("about.issues")).kind(ButtonKind::Ghost).icon(icon::BUG).ui(ui).clicked() {
            cx.push(Command::OpenUrl(format!("{}/issues", REPOSITORY.trim_end_matches('/'))));
        }
        if Button::new(&t("about.copy_info")).kind(ButtonKind::Ghost).icon(icon::COPY).ui(ui).clicked() {
            let info = format!("Snapoleon {VERSION} ({} {})", std::env::consts::OS, std::env::consts::ARCH);
            cx.push(Command::CopyText(info));
            cx.toast(ToastKind::Success, t("common.copied"));
        }
    });
}

fn body(ui: &mut Ui, cx: &mut ViewCx) {
    section(ui, &t("about.license"), |ui| {
        setting_row(ui, &t("about.license_name"), &t("about.license_desc"), |ui| {
            ui.label(text::mono_text(LICENSE));
        });
    });
    section(ui, &t("about.credits"), |ui| {
        for (name, license, url) in CREDITS {
            setting_row(ui, name, license, |ui| {
                if widgets::icon_button(ui, icon::ARROW_SQUARE_OUT, &t("about.open_site"), None).clicked() {
                    cx.push(Command::OpenUrl(url.to_owned()));
                }
            });
        }
    });
    ui.label(text::caption(t("about.credits_note")));
}
