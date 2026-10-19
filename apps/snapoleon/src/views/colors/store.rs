//! The session's picks. They live in the process, not in the window, so closing the tool does not lose them.

use parking_lot::{Mutex, MutexGuard};

use super::{ColorSample, Measurement};

pub const MAX_COLORS: usize = 24;
pub const MAX_MEASUREMENTS: usize = 12;

/// Newest first.
pub struct History {
    pub colors: Vec<ColorSample>,
    pub measurements: Vec<Measurement>,
}

static HISTORY: Mutex<History> = Mutex::new(History { colors: Vec::new(), measurements: Vec::new() });

pub fn lock() -> MutexGuard<'static, History> {
    HISTORY.lock()
}

impl History {
    /// A repeated color moves to the front instead of piling up.
    pub fn add_color(&mut self, sample: ColorSample) {
        self.colors.retain(|c| c.rgba != sample.rgba);
        self.colors.insert(0, sample);
        self.colors.truncate(MAX_COLORS);
    }

    pub fn add_measurement(&mut self, measurement: Measurement) {
        self.measurements.retain(|m| *m != measurement);
        self.measurements.insert(0, measurement);
        self.measurements.truncate(MAX_MEASUREMENTS);
    }
}
