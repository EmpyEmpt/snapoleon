//! Stub for pinned images: a borderless always-on-top window showing the image. Drag moves it,
//! double click or Esc closes it. Keep `TAG` and the `open` signature: the app already calls them.

use egui::{Color32, ColorImage, Context, Key, Sense, TextureHandle, TextureOptions};
use snap_core::{Point, Rect, RgbaImage, t};
use snap_shell::WindowSpec;
use snap_ui::palette;

use super::{OpenCx, View, ViewCx, app_icon};

pub const TAG: &str = "pin";

/// Pins `image`. `origin` is the physical screen rect the pixels came from: the pin appears right
/// over them. `None` lets the OS place the window.
pub fn open(cx: &mut OpenCx, image: RgbaImage, origin: Option<Rect>) {
    let scale = origin
        .and_then(|r| {
            let center = Point::new(r.x + r.w as i32 / 2, r.y + r.h as i32 / 2);
            cx.shell.monitors().into_iter().find(|(m, _)| m.contains(center)).map(|(_, scale)| scale)
        })
        .unwrap_or(1.0);
    let spec = WindowSpec {
        title: t("app.stub.pin.title").into_owned(),
        size: Some((image.width() as f32 / scale, image.height() as f32 / scale)),
        position: origin.map(|r| r.origin()),
        decorations: false,
        resizable: false,
        always_on_top: true,
        skip_taskbar: true,
        icon: Some(app_icon()),
        tag: TAG.into(),
        ..WindowSpec::default()
    };
    cx.open(spec, PinView { image, texture: None });
}

struct PinView {
    image: RgbaImage,
    texture: Option<TextureHandle>,
}

impl View for PinView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        let image = &self.image;
        let texture = self.texture.get_or_insert_with(|| {
            let size = [image.width() as usize, image.height() as usize];
            ctx.load_texture("pin-image", ColorImage::from_rgba_unmultiplied(size, image.as_raw()), TextureOptions::LINEAR)
        });
        let mut drag = false;
        let mut close = ctx.input(|i| i.key_pressed(Key::Escape));
        ctx.root_ui(|ui| {
            egui::CentralPanel::default().frame(egui::Frame::new()).show(ui, |ui| {
                let rect = ui.max_rect();
                let response = ui.allocate_rect(rect, Sense::click_and_drag());
                drag = response.drag_started();
                close |= response.double_clicked();
                let painter = ui.painter();
                painter.image(texture.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                let border = palette(ui.ctx()).accent.gamma_multiply(0.6);
                painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.0, border), egui::StrokeKind::Inside);
            });
        });
        if drag {
            cx.shell.drag_window(cx.win);
        }
        if close {
            cx.close();
        }
    }
}
