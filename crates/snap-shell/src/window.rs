//! Per-window state: native window, render target, egui context and input state.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use egui::{FullOutput, RawInput, ViewportId};
use snap_core::Rect;
use winit::{event_loop::ActiveEventLoop, window::Window};

use crate::render::{Paint, Surface, Target};

pub(crate) struct WindowState {
    pub window: Arc<Window>,
    target: Target,
    pub ctx: egui::Context,
    pub egui_state: egui_winit::State,
    viewport_info: egui::ViewportInfo,
    first_input: bool,

    pub tag: String,
    pub cover: Option<Rect>,
    pub cover_dirty: bool,
    pub cover_attempts: u8,
    pub tool_style: bool,
    /// The window is clipped to a pill; the clip follows every resize.
    pub pill: bool,
    pub want_visible: bool,
    /// Ask the OS for the foreground when the window appears (`WindowSpec::activate`).
    pub activate: bool,
    pub first_pass_done: bool,
    pub shown: bool,
    /// Click-through requested before the window was first shown.
    pub pending_passthrough: Option<bool>,
    pub opened_at: Instant,
    pub first_frame: Option<Duration>,
    pub repaint_at: Option<Instant>,
}

/// Non-GPU bookkeeping handed over by `Shell::open`.
pub(crate) struct Setup {
    pub tag: String,
    pub cover: Option<Rect>,
    pub tool_style: bool,
    pub pill: bool,
    pub want_visible: bool,
    pub activate: bool,
    pub opened_at: Instant,
}

impl WindowState {
    pub fn new(window: Arc<Window>, target: Target, setup: Setup, ctx: egui::Context) -> Self {
        let egui_state = egui_winit::State::new(
            ctx.clone(),
            ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(target.max_texture_side()),
        );

        Self {
            window,
            target,
            ctx,
            egui_state,
            viewport_info: egui::ViewportInfo::default(),
            first_input: true,
            tag: setup.tag,
            cover: setup.cover,
            cover_dirty: setup.cover.is_some(),
            cover_attempts: 0,
            tool_style: setup.tool_style,
            pill: setup.pill,
            want_visible: setup.want_visible,
            activate: setup.activate,
            first_pass_done: false,
            shown: false,
            pending_passthrough: None,
            opened_at: setup.opened_at,
            first_frame: None,
            repaint_at: None,
        }
    }

    /// Toggle click-through. A layered window must have presented once before
    /// its style may change, so an early request is applied on first show.
    pub fn set_passthrough(&mut self, on: bool) {
        if !self.shown {
            self.pending_passthrough = Some(on);
            return;
        }
        if let Err(e) = self.window.set_cursor_hittest(!on) {
            tracing::warn!("set_passthrough failed: {e}");
        }
        crate::platform::refresh(&self.window, self.tool_style);
    }

    /// Collect input for the next egui pass, or `None` while the window has no
    /// drawable area (minimized).
    pub fn begin_frame(&mut self) -> Option<RawInput> {
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return None;
        }
        egui_winit::update_viewport_info(&mut self.viewport_info, &self.ctx, &self.window, self.first_input);
        self.first_input = false;
        let mut input = self.egui_state.take_egui_input(&self.window);
        input.viewports.insert(ViewportId::ROOT, self.viewport_info.clone());
        self.viewport_info.events.clear();
        Some(input)
    }

    /// Apply egui's cursor, clipboard, IME and viewport output. Returns true
    /// when egui asked for the window to be closed.
    pub fn handle_output(&mut self, el: &ActiveEventLoop, out: &mut FullOutput) -> bool {
        self.egui_state.handle_platform_output_with_event_loop(&self.window, el, std::mem::take(&mut out.platform_output));

        let Some(viewport) = out.viewport_output.remove(&ViewportId::ROOT) else {
            return false;
        };
        let mut actions = Vec::new();
        egui_winit::process_viewport_commands(&self.ctx, &mut self.viewport_info, viewport.commands, &self.window, &mut actions);
        for action in actions {
            let event = match action {
                egui_winit::ActionRequested::Cut => egui::Event::Cut,
                egui_winit::ActionRequested::Copy => egui::Event::Copy,
                egui_winit::ActionRequested::Paste => match self.egui_state.clipboard_text() {
                    Some(text) => egui::Event::Paste(text),
                    None => continue,
                },
                egui_winit::ActionRequested::Screenshot(_) | egui_winit::ActionRequested::PaintWhileHidden => continue,
            };
            self.egui_state.egui_input_mut().events.push(event);
        }
        let close = self.viewport_info.events.iter().any(|e| matches!(e, egui::ViewportEvent::Close));
        self.viewport_info.events.clear();
        close
    }

    /// Render one egui frame. Returns true when the frame could not be
    /// presented and a new one should be rendered right away.
    pub fn paint(&mut self, out: FullOutput) -> bool {
        match self.target.paint(&self.ctx, out) {
            Paint::Presented => {
                self.first_frame.get_or_insert_with(|| self.opened_at.elapsed());
                false
            }
            Paint::Skipped => false,
            Paint::Retry => true,
        }
    }
}
