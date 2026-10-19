//! Every Snapoleon widget in dark and light, side by side.
//!
//!     cargo run -p snap-ui --example gallery
//!
//! Environment (for automated screenshots): `GALLERY_SHOT=<png>` saves the window contents after a few
//! frames and exits; `GALLERY_SCROLL=<px>` scrolls both panels; `GALLERY_TOASTS=1` pushes sample toasts;
//! `GALLERY_HOVER=<x>,<y>` parks the pointer; `GALLERY_CLICK=<x>,<y>` clicks there (to open a popup);
//! `GALLERY_SIZE=<w>x<h>` sets the window size.

use eframe::egui::{self, Align, Layout, Rect, ScrollArea, Sense, UiBuilder, pos2, vec2};
use snap_ui::{
    Palette, ThemeSpec, ToastKind, Toasts, icon, style_for,
    widgets::{self, ButtonKind, RecorderState, TitleBarActions},
};

struct PanelState {
    dark: bool,
    toggle_a: bool,
    toggle_b: bool,
    segmented: Format,
    tool: usize,
    chip_all: bool,
    chip_shots: bool,
    chip_fav: bool,
    dropdown: String,
    slider: f32,
    number: f32,
    count: i32,
    stroke: f32,
    color: [u8; 4],
    search: String,
    name: String,
    token: String,
    notes: String,
    accel: String,
    recorder: RecorderState,
    nav: usize,
    progress: f32,
}

#[derive(Clone, Copy, PartialEq)]
enum Format {
    Png,
    Jpeg,
    Webp,
}

impl PanelState {
    fn new(dark: bool) -> Self {
        Self {
            dark,
            toggle_a: true,
            toggle_b: false,
            segmented: Format::Png,
            tool: 1,
            chip_all: true,
            chip_shots: false,
            chip_fav: false,
            dropdown: "clipboard".into(),
            slider: 60.0,
            number: 12.5,
            count: 3,
            stroke: 4.0,
            color: [0x3D, 0x5A, 0xFE, 0xFF],
            search: String::new(),
            name: "Snapoleon_{date}_{time}".into(),
            token: "sk-live-1234".into(),
            notes: String::new(),
            accel: "Ctrl+Shift+4".into(),
            recorder: RecorderState::default(),
            nav: 1,
            progress: 0.64,
        }
    }
}

struct Gallery {
    panels: [PanelState; 2],
    toasts: Toasts,
    chrome: TitleBarActions,
    frame: u32,
    frame_hint: u32,
    shot: Option<String>,
    scroll: f32,
    hover: Option<egui::Pos2>,
    clicks: Vec<egui::Pos2>,
    shot_frame: Option<u32>,
}

fn parse_pos(v: &str) -> Option<egui::Pos2> {
    let (x, y) = v.split_once(',')?;
    Some(pos2(x.trim().parse().ok()?, y.trim().parse().ok()?))
}

fn env_pos(name: &str) -> Option<egui::Pos2> {
    parse_pos(&std::env::var(name).ok()?)
}

impl Gallery {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        snap_ui::install(&cc.egui_ctx, &ThemeSpec::default());
        if std::env::var("GALLERY_HOVER").is_ok() {
            // The synthetic pointer re-sends its position every frame, so it never counts as "still".
            cc.egui_ctx.all_styles_mut(|s| {
                s.interaction.show_tooltips_only_when_still = false;
                s.interaction.tooltip_delay = 0.0;
            });
        }
        let mut toasts = Toasts::new();
        if std::env::var("GALLERY_TOASTS").is_ok() {
            toasts.push(ToastKind::Success, "Screenshot copied to clipboard");
            toasts.push_toast(snap_ui::Toast::new(ToastKind::Info, "Saved to Pictures/Snapoleon").action("Open", "open"));
            toasts.push(ToastKind::Error, "Upload failed: the server did not respond in time");
        }
        Self {
            panels: [PanelState::new(true), PanelState::new(false)],
            toasts,
            chrome: TitleBarActions::default(),
            frame: 0,
            frame_hint: 0,
            shot: std::env::var("GALLERY_SHOT").ok(),
            scroll: std::env::var("GALLERY_SCROLL").ok().and_then(|s| s.parse().ok()).unwrap_or(0.0),
            hover: env_pos("GALLERY_HOVER"),
            clicks: std::env::var("GALLERY_CLICK").map(|v| v.split(';').filter_map(parse_pos).collect()).unwrap_or_default(),
            shot_frame: std::env::var("GALLERY_SHOT_FRAME").ok().and_then(|s| s.parse().ok()),
        }
    }
}

impl eframe::App for Gallery {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        // Keep screenshots deterministic even when the OS does not hand the window focus.
        raw.focused = true;
        // Each click takes two frames: move there, then press and release.
        self.frame_hint += 1;
        if self.frame_hint >= 3
            && let Some(pos) = self.hover
        {
            raw.events.push(egui::Event::PointerMoved(pos));
        }
        for (i, &pos) in self.clicks.iter().enumerate() {
            let start = 4 + 2 * i as u32;
            if self.frame_hint == start {
                raw.events.push(egui::Event::PointerMoved(pos));
            }
            if self.frame_hint == start + 1 {
                for pressed in [true, false] {
                    raw.events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    });
                }
            }
        }
        let typed_at = 8 + 2 * self.clicks.len() as u32;
        if self.frame_hint == typed_at {
            if let Ok(text) = std::env::var("GALLERY_TEXT") {
                raw.events.push(egui::Event::Text(text));
            }
            if let Ok(spec) = std::env::var("GALLERY_KEY") {
                // e.g. `ctrl+shift+Z`: modifiers and a key, sent as a key press.
                let mut mods = egui::Modifiers::default();
                let mut key = None;
                for part in spec.split('+') {
                    match part.to_ascii_lowercase().as_str() {
                        "ctrl" => mods.ctrl = true,
                        "alt" => mods.alt = true,
                        "shift" => mods.shift = true,
                        _ => key = egui::Key::from_name(part),
                    }
                }
                if let Some(key) = key {
                    raw.events.push(egui::Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers: mods });
                }
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.frame += 1;

        // Save the requested screenshot once the first frames have settled.
        if let Some(path) = self.shot.clone() {
            if self.frame == self.shot_frame.unwrap_or(14 + 2 * self.clicks.len() as u32) {
                let callback = egui::ScreenshotCallback::new(move |image| {
                    let [w, h] = image.size;
                    let rgba: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
                    if let Some(buf) = image::RgbaImage::from_raw(w as u32, h as u32, rgba) {
                        buf.save(&path).expect("write screenshot");
                    }
                    std::process::exit(0);
                });
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(callback));
            }
            ctx.request_repaint();
        }

        self.chrome.maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        let bar_rect = {
            let full = ui.max_rect();
            Rect::from_min_size(full.min, vec2(full.width(), widgets::TITLE_BAR_HEIGHT))
        };
        ui.scope_builder(UiBuilder::new().max_rect(bar_rect), |ui| {
            ui.set_style(style_for(true, ThemeSpec::default().accent, false));
            widgets::title_bar_with(ui, "Snapoleon UI gallery", &mut self.chrome, |ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::primary_button(ui, "Show toast").clicked() {
                        self.toasts.push(ToastKind::Success, "Screenshot copied to clipboard");
                    }
                });
            });
        });
        self.chrome.apply(&ctx);

        let body = Rect::from_min_max(pos2(ui.max_rect().left(), bar_rect.bottom()), ui.max_rect().max);
        let half = body.width() / 2.0;
        for (i, state) in self.panels.iter_mut().enumerate() {
            let rect = Rect::from_min_size(pos2(body.left() + half * i as f32, body.top()), vec2(half, body.height()));
            ui.scope_builder(UiBuilder::new().id_salt(i).max_rect(rect), |ui| {
                ui.set_style(style_for(state.dark, ThemeSpec::brand(state.dark).accent, false));
                let p = Palette::of(ui);
                ui.painter().rect_filled(rect, 0.0, p.bg);
                ScrollArea::vertical().id_salt(i).auto_shrink([false, false]).vertical_scroll_offset(self.scroll).show(ui, |ui| {
                    ui.add_space(20.0);
                    egui::Frame::new().inner_margin(egui::Margin::symmetric(24, 0)).show(ui, |ui| {
                        panel(ui, state);
                    });
                    ui.add_space(24.0);
                });
            });
        }

        if std::env::var("GALLERY_FLOAT").is_ok() {
            // A fake capture selection near the bottom edge: the toolbar must flip above it.
            let screen = ctx.content_rect();
            let selection = Rect::from_min_size(pos2(screen.center().x - 160.0, screen.bottom() - 220.0), vec2(320.0, 190.0));
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::unique("fake-selection")));
            painter.rect_stroke(selection, 0.0, egui::Stroke::new(1.0, snap_ui::palette(&ctx).accent), egui::StrokeKind::Inside);
            widgets::floating_toolbar_anchored(&ctx, egui::Id::unique("float"), selection, screen, |ui| {
                for (ic, tip) in [(icon::COPY, "Copy"), (icon::FLOPPY_DISK, "Save"), (icon::PENCIL_SIMPLE, "Edit"), (icon::PUSH_PIN, "Pin")]
                {
                    widgets::icon_button(ui, ic, tip, None);
                }
                widgets::toolbar_separator(ui);
                widgets::icon_button(ui, icon::X, "Cancel", Some("Esc"));
            });
        }

        let activated = self.toasts.show(&ctx);
        for id in activated {
            self.toasts.push(ToastKind::Info, format!("Action: {id}"));
        }
    }
}

fn panel(ui: &mut egui::Ui, s: &mut PanelState) {
    let p = Palette::of(ui);
    ui.set_width(ui.available_width());

    // Brand + type scale.
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(56.0, 56.0), Sense::hover());
        snap_ui::logo_colored(&ui.painter_at(rect), rect, snap_ui::LogoColors::from_palette(&p));
        ui.vertical(|ui| {
            ui.add_space(4.0);
            ui.label(snap_ui::text::headline("Snapoleon"));
            ui.label(snap_ui::text::caption(if s.dark { "Dark theme" } else { "Light theme" }));
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            for c in [p.accent, p.gold, p.success, p.warning, p.danger] {
                let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 8.0, c);
            }
        });
    });
    ui.add_space(20.0);

    widgets::section(ui, "Type", |ui| {
        ui.add_space(8.0);
        ui.label(snap_ui::text::headline("Headline 22 bold"));
        ui.label(snap_ui::text::title("Title 16 semibold"));
        ui.label(snap_ui::text::body_strong("Body strong 13 semibold"));
        ui.label("Body 13 regular — Съешь ещё этих мягких французских булок");
        ui.label(snap_ui::text::caption("Caption 11 regular, dimmed"));
        ui.label(snap_ui::text::mono_text("Mono 12 · #3D5AFE · 1280 × 720"));
        ui.add_space(8.0);
    });

    widgets::section(ui, "Buttons", |ui| {
        ui.add_space(12.0);
        ui.horizontal_wrapped(|ui| {
            widgets::primary_button(ui, "Copy");
            widgets::secondary_button(ui, "Save");
            widgets::ghost_button(ui, "Cancel");
            widgets::danger_button(ui, "Delete");
            ui.add(widgets::Button::new("Upload").kind(ButtonKind::Primary).icon(icon::UPLOAD_SIMPLE));
            ui.add(widgets::Button::new("Pin").icon(icon::PUSH_PIN));
            ui.add_enabled(false, widgets::Button::new("Disabled"));
        });
        ui.horizontal(|ui| {
            for (ic, tip, key) in [
                (icon::COPY, "Copy", Some("Ctrl+C")),
                (icon::FLOPPY_DISK, "Save", Some("Ctrl+S")),
                (icon::PENCIL_SIMPLE, "Edit", Some("E")),
                (icon::PUSH_PIN, "Pin", None),
                (icon::X, "Close", Some("Esc")),
            ] {
                widgets::icon_button(ui, ic, tip, key);
            }
        });
        ui.horizontal(|ui| {
            for (i, (ic, tip)) in [
                (icon::CURSOR, "Select"),
                (icon::ARROW_UP_RIGHT, "Arrow"),
                (icon::RECTANGLE, "Rectangle"),
                (icon::TEXT_T, "Text"),
                (icon::NUMBER_CIRCLE_ONE, "Step"),
                (icon::EYE_SLASH, "Redact"),
            ]
            .into_iter()
            .enumerate()
            {
                if widgets::tool_button(ui, ic, tip, Some(&(i + 1).to_string()), s.tool == i).clicked() {
                    s.tool = i;
                }
            }
        });
        ui.horizontal(|ui| {
            for (label, on) in [("All", &mut s.chip_all), ("Screenshots", &mut s.chip_shots)] {
                if widgets::chip(ui, label, *on).clicked() {
                    *on = !*on;
                }
            }
            if widgets::chip_with_icon(ui, icon::STAR, "Favorites", s.chip_fav).clicked() {
                s.chip_fav = !s.chip_fav;
            }
            widgets::badge(ui, "Beta", snap_ui::Tone::Accent);
            widgets::badge(ui, "12", snap_ui::Tone::Neutral);
            widgets::badge(ui, "Rec", snap_ui::Tone::Danger);
            widgets::badge(ui, "Pro", snap_ui::Tone::Gold);
            widgets::kbd(ui, "Ctrl+Shift+4");
            widgets::kbd(ui, "PrintScreen");
        });
        ui.add_space(8.0);
    });

    widgets::section(ui, "Controls", |ui| {
        widgets::setting_row(ui, "Show notifications", "A toast appears after every capture.", |ui| {
            widgets::toggle(ui, &mut s.toggle_a);
        });
        widgets::setting_row(ui, "Play shutter sound", "", |ui| {
            widgets::toggle(ui, &mut s.toggle_b);
        });
        widgets::setting_row(ui, "Format", "Used when saving to disk.", |ui| {
            widgets::segmented(ui, &mut s.segmented, &[(Format::Png, "PNG"), (Format::Jpeg, "JPEG"), (Format::Webp, "WebP")]);
        });
        widgets::setting_row(ui, "After capture", "What happens right after you release the mouse.", |ui| {
            widgets::dropdown(
                ui,
                "after",
                &mut s.dropdown,
                &[("clipboard".to_owned(), "Copy to clipboard"), ("editor".to_owned(), "Open editor"), ("file".to_owned(), "Save to file")],
            );
        });
        widgets::setting_row(ui, "JPEG quality", "", |ui| {
            widgets::slider(ui, &mut s.slider, 1.0..=100.0);
        });
        widgets::setting_row(ui, "Delay", "Wait before capturing.", |ui| {
            widgets::number_field(ui, &mut s.number, 0.0..=60.0, " s");
        });
        widgets::setting_row(ui, "Recent items", "", |ui| {
            widgets::number_field(ui, &mut s.count, 0..=500, "");
        });
        widgets::setting_row(ui, "Capture hotkey", "Press the combination you want.", |ui| {
            widgets::hotkey_recorder(ui, &mut s.accel, &mut s.recorder);
        });
    });

    widgets::section(ui, "Annotation", |ui| {
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.label(snap_ui::text::caption("Color"));
            widgets::color_swatches(ui, &mut s.color, &widgets::DEFAULT_PALETTE);
        });
        ui.horizontal(|ui| {
            ui.label(snap_ui::text::caption("Width"));
            widgets::stroke_picker(ui, &mut s.stroke);
            ui.label(snap_ui::text::mono_text(format!("{} px", s.stroke)));
        });
        ui.add_space(8.0);
    });

    widgets::section(ui, "Inputs", |ui| {
        ui.add_space(12.0);
        widgets::search_field(ui, &mut s.search, "Search captures, text, apps…");
        ui.horizontal(|ui| {
            ui.add(widgets::TextField::new(&mut s.name).mono(true).width(220.0));
            ui.add(widgets::TextField::new(&mut s.token).password(true).hint("Token").width(160.0));
        });
        ui.add(widgets::TextField::new(&mut s.notes).hint("Notes").rows(3).width(ui.available_width()));
        ui.add_space(12.0);
    });

    ui.label(snap_ui::text::body_strong("Navigation"));
    ui.add_space(4.0);
    widgets::card(ui, |ui| {
        ui.set_max_width(220.0);
        ui.spacing_mut().item_spacing.y = 2.0;
        for (i, (ic, label)) in [
            (icon::GEAR, "General"),
            (icon::CAMERA, "Capture"),
            (icon::KEYBOARD, "Hotkeys"),
            (icon::CLOUD_ARROW_UP, "Destinations"),
            (icon::SHIELD_CHECK, "Privacy"),
        ]
        .into_iter()
        .enumerate()
        {
            if widgets::nav_item(ui, ic, label, s.nav == i).clicked() {
                s.nav = i;
            }
        }
    });
    ui.add_space(24.0);

    ui.label(snap_ui::text::body_strong("Toolbar"));
    ui.add_space(4.0);
    widgets::toolbar_pill(ui, |ui| {
        for (ic, tip) in [
            (icon::COPY, "Copy"),
            (icon::FLOPPY_DISK, "Save"),
            (icon::PENCIL_SIMPLE, "Edit"),
            (icon::PUSH_PIN, "Pin"),
            (icon::CLOUD_ARROW_UP, "Upload"),
        ] {
            widgets::icon_button(ui, ic, tip, None);
        }
        widgets::toolbar_separator(ui);
        for (ic, tip) in [(icon::TEXT_AA, "Text"), (icon::QR_CODE, "QR"), (icon::EYE_SLASH, "Redact")] {
            widgets::icon_button(ui, ic, tip, None);
        }
        widgets::toolbar_separator(ui);
        widgets::icon_button(ui, icon::RECORD, "Record", None);
        widgets::toolbar_separator(ui);
        widgets::icon_button(ui, icon::X, "Cancel", Some("Esc"));
    });
    ui.add_space(24.0);

    ui.label(snap_ui::text::body_strong("Progress"));
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        widgets::spinner(ui);
        ui.label(snap_ui::text::dim("Uploading…"));
    });
    widgets::progress(ui, s.progress);
    ui.add_space(8.0);
    widgets::progress_indeterminate(ui);
    ui.add_space(24.0);

    ui.label(snap_ui::text::body_strong("Window chrome"));
    ui.add_space(4.0);
    for mac_style in [false, true] {
        let mut chrome = TitleBarActions { mac_style, ..Default::default() };
        egui::Frame::new().stroke(egui::Stroke::new(1.0, p.border)).corner_radius(10).show(ui, |ui| {
            widgets::title_bar_with(ui, "Editor", &mut chrome, |ui| {
                widgets::search_field(ui, &mut s.search, "Search");
            });
        });
        ui.add_space(8.0);
    }
    ui.add_space(16.0);

    ui.label(snap_ui::text::body_strong("Stock egui widgets"));
    ui.add_space(4.0);
    widgets::card(ui, |ui| {
        widgets::checkbox(ui, &mut s.chip_all, "Checkbox");
        widgets::checkbox(ui, &mut s.chip_fav, "Checkbox off");
        ui.checkbox(&mut s.toggle_a, "Stock checkbox");
        ui.radio_value(&mut s.nav, 0, "Radio A");
        ui.radio_value(&mut s.nav, 1, "Radio B");
        egui::ComboBox::from_label("Combo").selected_text("Option").show_ui(ui, |ui| {
            ui.selectable_value(&mut s.nav, 0, "Option");
            ui.selectable_value(&mut s.nav, 2, "Other");
        });
        ui.collapsing("Collapsing header", |ui| {
            ui.label("Hidden content");
        });
        ui.add(egui::ProgressBar::new(0.4).text("40%"));
        ui.add(egui::Slider::new(&mut s.slider, 0.0..=100.0));
        ui.separator();
        ui.hyperlink_to("A hyperlink", "https://snapoleon.app");
    });
    ui.add_space(24.0);

    widgets::card(ui, |ui| {
        widgets::empty_state_brand(ui, "Nothing here yet", "Press PrintScreen to take your first shot.", |ui| {
            widgets::primary_button(ui, "Capture now");
        });
    });
    ui.add_space(16.0);
    widgets::card(ui, |ui| {
        widgets::empty_state(ui, icon::MAGNIFYING_GLASS, "No results", "Try a different word or clear the filters.");
    });
}

fn main() -> eframe::Result {
    let size = std::env::var("GALLERY_SIZE")
        .ok()
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some([w.parse().ok()?, h.parse().ok()?])
        })
        .unwrap_or([1440.0, 960.0]);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(size)
            .with_min_inner_size([900.0, 600.0])
            .with_decorations(false)
            .with_title("Snapoleon UI gallery")
            .with_position([40.0, 40.0]),
        ..Default::default()
    };
    eframe::run_native("Snapoleon UI gallery", options, Box::new(|cc| Ok(Box::new(Gallery::new(cc)))))
}
