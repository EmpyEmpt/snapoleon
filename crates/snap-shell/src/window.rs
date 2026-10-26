//! Per-window state: native window, render target, egui context and input state.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use egui::{Event, FullOutput, Key, Modifiers, RawInput, ViewportId};
use snap_core::Rect;
use winit::{event_loop::ActiveEventLoop, window::Window};

use crate::render::{Paint, Surface, Target};

/// A command from the Edit menu (macOS), for the window with the keyboard focus.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) enum EditAction {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
}

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
    /// An ordinary app window (`WindowSpec::is_app_window`).
    pub app_window: bool,
    /// Height of the title bar the app draws (`WindowSpec::title_bar`).
    pub title_bar: Option<f32>,
    /// Width the OS's own window controls take at the left of that title bar (macOS traffic lights).
    pub controls_inset: Option<f32>,
    pub want_visible: bool,
    /// Ask the OS for the foreground when the window appears (`WindowSpec::activate`).
    pub activate: bool,
    pub first_pass_done: bool,
    pub shown: bool,
    /// Focus was asked for while the window was hidden, when the OS ignores it; it is taken on show instead.
    pub focus_on_show: bool,
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
    pub app_window: bool,
    pub title_bar: Option<f32>,
    pub controls_inset: Option<f32>,
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
            app_window: setup.app_window,
            title_bar: setup.title_bar,
            controls_inset: setup.controls_inset,
            want_visible: setup.want_visible,
            activate: setup.activate,
            first_pass_done: false,
            shown: false,
            focus_on_show: false,
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

    /// Put the window on screen. It takes the keyboard when focus was asked for while it was hidden, or when it is an
    /// ordinary window that activates; covers and `activate: false` windows appear without it, so pins and HUDs leave
    /// the keyboard where it is and the covers of an overlay do not take it from the one under the pointer.
    /// `activate_app` also asks the OS for the foreground: a window opened by a hotkey or the tray must not appear
    /// behind the app the user is in.
    pub fn show(&mut self, activate_app: bool) {
        self.shown = true;
        let focus = std::mem::take(&mut self.focus_on_show);
        let take_key = focus || (self.activate && self.cover.is_none());
        crate::platform::show(&self.window, take_key);
        crate::platform::refresh(&self.window, self.tool_style);
        if focus || (activate_app && take_key) {
            crate::platform::activate(&self.window);
        }
        if focus {
            self.window.focus_window();
        }
        if let Some(on) = self.pending_passthrough.take() {
            self.set_passthrough(on);
        }
        self.window.request_redraw();
    }

    /// Hand an Edit menu command to egui as the input its keyboard shortcut produces: the menu takes the shortcut
    /// before the window sees it. Cut, copy and paste mirror `egui_winit`; the others are the Cmd+key presses that
    /// text fields and views look for.
    pub fn edit(&mut self, action: EditAction) {
        let key = |key: Key, shift: bool| {
            let modifiers = Modifiers { mac_cmd: true, command: true, shift, ..Modifiers::default() };
            [true, false].map(|pressed| Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers })
        };
        let events = match action {
            EditAction::Cut => vec![Event::Cut],
            EditAction::Copy => vec![Event::Copy],
            EditAction::Paste => match self.egui_state.clipboard_text() {
                Some(text) if text.is_empty() => Vec::new(),
                Some(text) => vec![Event::Paste(text.replace("\r\n", "\n"))],
                None => self.egui_state.clipboard_image().map(|image| Event::PasteImage(Arc::new(image))).into_iter().collect(),
            },
            EditAction::Undo => key(Key::Z, false).into(),
            EditAction::Redo => key(Key::Z, true).into(),
            EditAction::SelectAll => key(Key::A, false).into(),
        };
        self.egui_state.egui_input_mut().events.extend(events);
        self.window.request_redraw();
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
