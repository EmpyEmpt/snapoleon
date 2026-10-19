//! The state shared by the cover windows of one overlay session.

use std::{cell::RefCell, rc::Rc, time::Instant};

use egui::{Pos2, Vec2};
use snap_capture::Frozen;
use snap_core::{CaptureKind, Point, Rect as PxRect, Settings};
use snap_shell::WindowKey;

use super::{
    OverlayAction, OverlayMode, OverlayRequest,
    bars::RecordOptions,
    loupe::ColorFormat,
    measure::Measure,
    selection::{Selection, SnapLines},
    space::pixel_at,
    targets::{Target, Targets},
};
use crate::views::colors::{ColorSample, Measurement};

pub type Shared = Rc<RefCell<Session>>;

/// How the overlay ends.
pub enum Finish {
    Cancel,
    Selected(PxRect, CaptureKind),
    Action(OverlayAction, PxRect, CaptureKind),
    Color(ColorSample),
    Measured(Measurement),
}

/// The bar that belongs under a finished selection, by mode.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BarKind {
    Instant,
    Record,
    Scroll,
}

/// The parts of the capture settings the overlay reads.
pub struct Opts {
    pub dim: bool,
    pub crosshair: bool,
    pub magnifier: bool,
    pub zoom: u32,
    pub snap_to_windows: bool,
}

/// The pointer in virtual-desktop pixels. Arrow keys nudge it by whole pixels without moving the OS cursor.
#[derive(Default)]
pub struct Cursor {
    pub raw: Option<Pos2>,
    pub nudge: Vec2,
    /// The window that last saw the pointer.
    pub owner: Option<WindowKey>,
}

impl Cursor {
    pub fn pos(&self) -> Option<Pos2> {
        self.raw.map(|p| p + self.nudge)
    }
}

pub struct Session {
    pub frozen: Frozen,
    pub request: OverlayRequest,
    pub windows: Vec<WindowKey>,
    pub done: bool,
    pub opened: Instant,
    /// When the cover windows were asked to show this session.
    pub presented: Instant,
    pub bounds: PxRect,
    pub lines: SnapLines,
    pub opts: Opts,
    pub sel: Selection,
    pub cursor: Cursor,
    pub targets: Targets,
    pub measure: Measure,
    pub record: RecordOptions,
    pub format: ColorFormat,
    /// Ctrl as seen by the focused window; the other windows follow it.
    pub ctrl: bool,
    pub shift_was_down: bool,
    pub flash: Option<(String, Instant)>,
    /// The user has started doing something, so the hint goes away.
    pub engaged: bool,
    /// A press that began on empty space (not on a bar) is held.
    pub pressing: bool,
    pub ready_logged: bool,
}

impl Session {
    pub fn new(frozen: Frozen, request: OverlayRequest, settings: &Settings, opened: Instant) -> Self {
        let capture = &settings.capture;
        let format = match request.mode {
            OverlayMode::Record(format) => format,
            _ => settings.recording.format,
        };
        let cursor = Cursor { raw: Some(Pos2::new(frozen.cursor.x as f32, frozen.cursor.y as f32)), ..Cursor::default() };
        Self {
            bounds: frozen.bounds(),
            lines: SnapLines::new(&frozen),
            opts: Opts {
                dim: capture.dim_outside,
                crosshair: capture.show_crosshair,
                magnifier: capture.show_magnifier,
                zoom: capture.magnifier_zoom,
                snap_to_windows: capture.snap_to_windows,
            },
            frozen,
            request,
            windows: Vec::new(),
            done: false,
            opened,
            presented: opened,
            sel: Selection::new(),
            cursor,
            targets: Targets::default(),
            measure: Measure::default(),
            record: RecordOptions { format, microphone: settings.recording.microphone, system_audio: settings.recording.system_audio },
            format: ColorFormat::last(),
            ctrl: false,
            shift_was_down: false,
            flash: None,
            engaged: false,
            pressing: false,
            ready_logged: false,
        }
    }

    pub fn mode(&self) -> OverlayMode {
        self.request.mode
    }

    /// Modes where the user drags out a rectangle.
    pub fn selects_region(&self) -> bool {
        use OverlayMode as M;
        matches!(self.mode(), M::Region | M::Ocr | M::Codes | M::Translate | M::Pin | M::Record(_) | M::Scroll)
    }

    /// Modes where a click on a window or element is the whole interaction.
    pub fn picks_target(&self) -> bool {
        matches!(self.mode(), OverlayMode::Window | OverlayMode::Element)
    }

    /// Modes that end as soon as the rectangle is chosen.
    pub fn finishes_on_release(&self) -> bool {
        matches!(self.mode(), OverlayMode::Ocr | OverlayMode::Codes | OverlayMode::Translate | OverlayMode::Pin)
    }

    pub fn bar(&self) -> Option<BarKind> {
        match self.mode() {
            OverlayMode::Region => Some(BarKind::Instant),
            OverlayMode::Record(_) => Some(BarKind::Record),
            OverlayMode::Scroll => Some(BarKind::Scroll),
            _ => None,
        }
    }

    /// Modes that look up UI elements under the pointer.
    pub fn wants_elements(&self) -> bool {
        match self.mode() {
            OverlayMode::Element | OverlayMode::Measure => true,
            _ => self.selects_region() && self.ctrl,
        }
    }

    pub fn point(&self) -> Option<Point> {
        self.cursor.pos().map(pixel_at)
    }

    /// The window or element the pointer would select now.
    pub fn hover_target(&self) -> Option<Target> {
        let at = self.point()?;
        if self.wants_elements()
            && let Some(target) = self.targets.element(at)
        {
            return Some(target);
        }
        let windows = self.opts.snap_to_windows || self.mode() == OverlayMode::Window;
        if windows && !matches!(self.mode(), OverlayMode::ColorPicker | OverlayMode::Measure) {
            return Targets::window(&self.frozen, at);
        }
        None
    }

    /// The monitor containing `p`, as an index into the frozen snapshot.
    pub fn monitor_at(&self, p: Point) -> Option<usize> {
        self.frozen.monitors.iter().position(|(m, _)| m.rect.contains(p))
    }

    /// The monitor most of `r` is on: where its labels and bars go.
    pub fn dominant_monitor(&self, r: PxRect) -> usize {
        self.frozen.monitors.iter().enumerate().max_by_key(|(_, (m, _))| m.rect.intersect(r).map_or(0, PxRect::area)).map_or(0, |(i, _)| i)
    }

    /// The whole monitor under the pointer (what Enter captures when nothing is selected).
    pub fn monitor_rect(&self) -> PxRect {
        let index = self.point().and_then(|p| self.monitor_at(p)).unwrap_or(0);
        self.frozen.monitors[index].0.rect
    }

    pub fn flash(&mut self, text: String) {
        self.flash = Some((text, Instant::now()));
    }
}
