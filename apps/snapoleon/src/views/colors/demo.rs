//! Demo picks for the dev harness (`SNAPOLEON_DEV_DEMO=1`), so screenshots show a filled window.

use snap_core::{Point, Rect};

use super::{ColorSample, Measurement, store::History};

pub fn seed(history: &mut History) {
    if std::env::var_os("SNAPOLEON_DEV_DEMO").is_none() || !history.colors.is_empty() {
        return;
    }
    const PICKS: [[u8; 3]; 7] =
        [[23, 24, 28], [247, 247, 245], [61, 90, 254], [227, 179, 65], [48, 197, 122], [255, 90, 95], [142, 91, 255]];
    for (i, [r, g, b]) in PICKS.into_iter().enumerate() {
        history.add_color(ColorSample { rgba: [r, g, b, 255], at: Point::new(120 + 90 * i as i32, 240 + 31 * i as i32) });
    }
    history.add_measurement(Measurement { a: Rect::new(160, 96, 128, 48), b: None });
    history.add_measurement(Measurement { a: Rect::new(160, 96, 128, 48), b: Some(Rect::new(304, 160, 96, 32)) });
    history.add_measurement(Measurement { a: Rect::new(32, 400, 1280, 720), b: None });
}
