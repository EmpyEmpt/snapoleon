//! The window registry and the public [`Shell`] handle given to the app.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use snap_core::{Error, Rect, Result};
use winit::{
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    event_loop::ActiveEventLoop,
    window::{Icon, Window, WindowAttributes, WindowId, WindowLevel},
};

use crate::{
    WindowKey, WindowSpec, platform,
    proxy::Proxy,
    render::{Gfx, Graphics, Target},
    window::{Setup, WindowState},
};

/// Size used when a spec gives neither `size` nor `cover`.
const DEFAULT_SIZE: (f32, f32) = (640.0, 480.0);

pub(crate) struct Core<E: Send + 'static> {
    pub app_id: String,
    pub theme_init: Box<dyn Fn(&egui::Context)>,
    pub proxy: Proxy<E>,
    pub gfx: Gfx,
    pub windows: HashMap<WindowKey, WindowState>,
    pub ids: HashMap<WindowId, WindowKey>,
    pub next_key: WindowKey,
    /// Windows closed since the app was last told about it.
    pub closed: Vec<WindowKey>,
}

impl<E: Send + 'static> Core<E> {
    pub fn remove(&mut self, key: WindowKey) -> bool {
        let Some(state) = self.windows.remove(&key) else {
            return false;
        };
        self.ids.remove(&state.window.id());
        self.closed.push(key);
        true
    }
}

pub(crate) fn monitor_list(el: &ActiveEventLoop) -> Vec<(Rect, f32)> {
    el.available_monitors()
        .map(|m| {
            let (p, s) = (m.position(), m.size());
            (Rect::new(p.x, p.y, s.width, s.height), m.scale_factor() as f32)
        })
        .collect()
}

/// Handle to the running shell, passed to every [`ShellApp`](crate::ShellApp)
/// callback. It is only valid for the duration of that callback; use
/// [`Shell::proxy`] to reach the shell from other threads.
pub struct Shell<'a, E: Send + 'static> {
    pub(crate) core: &'a mut Core<E>,
    pub(crate) el: &'a ActiveEventLoop,
}

impl<E: Send + 'static> Shell<'_, E> {
    /// Create a window and return its key right away. The window renders its
    /// first frame hidden and is shown afterwards, so it never flashes blank.
    /// If the window cannot be created the error is logged and the returned
    /// key never [`exists`](Self::exists).
    pub fn open(&mut self, spec: WindowSpec) -> WindowKey {
        let key = self.core.next_key;
        self.core.next_key += 1;
        if let Err(e) = self.try_open(key, spec) {
            tracing::error!("opening window {key} failed: {e}");
        }
        key
    }

    fn try_open(&mut self, key: WindowKey, spec: WindowSpec) -> Result<()> {
        let opened_at = Instant::now();
        let mut attrs = Window::default_attributes()
            .with_title(spec.title.clone())
            .with_visible(false)
            .with_decorations(spec.decorations)
            .with_resizable(spec.resizable)
            .with_active(spec.activate)
            .with_window_level(if spec.always_on_top { WindowLevel::AlwaysOnTop } else { WindowLevel::Normal });
        if let Some(icon) = &spec.icon {
            let (w, h) = icon.dimensions();
            attrs = attrs.with_window_icon(Icon::from_rgba(icon.clone().into_raw(), w, h).ok());
        }
        if let Some(r) = spec.cover {
            attrs = attrs
                .with_decorations(false)
                .with_resizable(false)
                .with_window_level(WindowLevel::AlwaysOnTop)
                .with_inner_size(PhysicalSize::new(r.w.max(1), r.h.max(1)))
                .with_position(PhysicalPosition::new(r.x, r.y));
        } else {
            let (w, h) = spec.size.unwrap_or(DEFAULT_SIZE);
            attrs = attrs.with_inner_size(LogicalSize::new(w, h));
            if let Some((w, h)) = spec.min_size {
                attrs = attrs.with_min_inner_size(LogicalSize::new(w, h));
            }
            if let Some(p) = spec.position {
                attrs = attrs.with_position(PhysicalPosition::new(p.x, p.y));
            }
        }
        let attrs = platform::window_attributes(self.el, attrs, &spec, &self.core.app_id);

        let (window, target) = match self.make_window(attrs.clone(), &spec) {
            Ok(made) => made,
            // The backend may have a second choice (OpenGL to wgpu); the window it failed on is gone.
            Err(e) if self.core.gfx.fall_back(&e) => self.make_window(attrs, &spec)?,
            Err(e) => return Err(e),
        };

        let ctx = egui::Context::default();
        (self.core.theme_init)(&ctx);
        let proxy = self.core.proxy.clone();
        ctx.set_request_repaint_callback(move |info| proxy.repaint_at(key, Instant::now() + info.delay));

        let setup = Setup {
            tool_style: spec.is_tool(),
            pill: spec.pill,
            tag: spec.tag,
            cover: spec.cover,
            want_visible: spec.visible,
            activate: spec.activate,
            opened_at,
        };
        let state = WindowState::new(window.clone(), target, setup, ctx);
        self.core.ids.insert(window.id(), key);
        self.core.windows.insert(key, state);
        // The first frame is rendered from the event loop's idle hook.
        Ok(())
    }

    /// The native window with its native tweaks and drawing surface.
    fn make_window(&mut self, attrs: WindowAttributes, spec: &WindowSpec) -> Result<(Arc<Window>, Target)> {
        let window = Arc::new(self.core.gfx.create_window(self.el, attrs)?);
        platform::configure_window(&window, spec);
        if spec.pill {
            platform::set_pill(&window, true);
        }
        if let Some(r) = spec.cover {
            platform::place_cover(&window, r, &monitor_list(self.el));
        }
        let target = self.core.gfx.create_target(&window)?;
        Ok((window, target))
    }

    /// Destroy a window. The app is told through [`ShellApp::closed`](crate::ShellApp::closed)
    /// once the current callback returns.
    pub fn close(&mut self, w: WindowKey) {
        self.core.remove(w);
    }

    pub fn exists(&self, w: WindowKey) -> bool {
        self.core.windows.contains_key(&w)
    }

    pub fn set_visible(&mut self, w: WindowKey, v: bool) {
        let Some(state) = self.core.windows.get_mut(&w) else { return };
        state.want_visible = v;
        if !v {
            state.window.set_visible(false);
            state.shown = false;
        } else if state.first_pass_done {
            state.window.set_visible(true);
            state.shown = true;
            if let Some(on) = state.pending_passthrough.take() {
                state.set_passthrough(on);
            }
            state.window.request_redraw();
        }
        platform::refresh(&state.window, state.tool_style);
    }

    /// Show a hidden window again with a fresh frame: it renders while still hidden and appears with that frame, so
    /// whatever it showed last never flashes. The window counts as newly opened:
    /// [`first_frame_time`](Self::first_frame_time) measures from this call. Pre-opened windows that are shown on demand
    /// (the selection overlay) use it instead of [`set_visible`](Self::set_visible).
    pub fn present(&mut self, w: WindowKey) {
        let Some(state) = self.core.windows.get_mut(&w) else { return };
        state.want_visible = true;
        state.shown = false;
        state.first_pass_done = false;
        state.opened_at = Instant::now();
        state.first_frame = None;
    }

    /// Run one pass of a hidden window without showing it, so a view can release what it held while it was visible.
    pub fn refresh_hidden(&mut self, w: WindowKey) {
        if let Some(state) = self.core.windows.get_mut(&w) {
            state.first_pass_done = false;
        }
    }

    pub fn focus(&mut self, w: WindowKey) {
        if let Some(state) = self.core.windows.get(&w) {
            platform::activate(&state.window);
            state.window.focus_window();
        }
    }

    pub fn set_title(&mut self, w: WindowKey, t: &str) {
        if let Some(state) = self.core.windows.get(&w) {
            state.window.set_title(t);
        }
    }

    /// Move and resize so the window's outer rect is `r` (physical pixels).
    /// Cover windows adopt `r` as their new enforced rect.
    pub fn set_outer_rect(&mut self, w: WindowKey, r: Rect) {
        let Some(state) = self.core.windows.get_mut(&w) else { return };
        if state.cover.is_some() {
            state.cover = Some(r);
            state.cover_attempts = 0;
            state.cover_dirty = true;
            platform::place_cover(&state.window, r, &monitor_list(self.el));
        } else {
            platform::set_outer_rect(&state.window, r);
        }
    }

    /// Clip the window to a pill, or back to its full rectangle. Windows only.
    pub fn set_pill(&mut self, w: WindowKey, on: bool) {
        if let Some(state) = self.core.windows.get_mut(&w) {
            state.pill = on;
            platform::set_pill(&state.window, on);
        }
    }

    pub fn outer_rect(&self, w: WindowKey) -> Option<Rect> {
        platform::outer_rect(&self.core.windows.get(&w)?.window)
    }

    pub fn set_always_on_top(&mut self, w: WindowKey, on: bool) {
        if let Some(state) = self.core.windows.get(&w) {
            state.window.set_window_level(if on { WindowLevel::AlwaysOnTop } else { WindowLevel::Normal });
            platform::refresh(&state.window, state.tool_style);
        }
    }

    /// Let mouse input fall through the window to whatever is below it.
    pub fn set_passthrough(&mut self, w: WindowKey, on: bool) {
        if let Some(state) = self.core.windows.get_mut(&w) {
            state.set_passthrough(on);
        }
    }

    /// Hide a window from screenshots and recordings. `Error::Unsupported`
    /// where the OS has no such feature (Linux).
    pub fn set_exclude_from_capture(&mut self, w: WindowKey, on: bool) -> Result<()> {
        let state = self.core.windows.get(&w).ok_or_else(|| Error::NotFound(format!("window {w}")))?;
        platform::set_exclude_from_capture(&state.window, on)
    }

    /// Queue an input event for the window's next frame. For input the OS consumed itself, such as the button
    /// release that ended a modal drag-and-drop loop, which egui would otherwise never see.
    pub fn push_input_event(&mut self, w: WindowKey, event: egui::Event) {
        if let Some(state) = self.core.windows.get_mut(&w) {
            state.egui_state.egui_input_mut().events.push(event);
        }
    }

    /// Start an OS-driven window move; call from a pointer-press handler.
    pub fn drag_window(&mut self, w: WindowKey) {
        if let Some(state) = self.core.windows.get(&w)
            && let Err(e) = state.window.drag_window()
        {
            tracing::debug!("drag_window failed: {e}");
        }
    }

    /// HWND, NSWindow number or X11 window id; `None` on Wayland.
    pub fn native_id(&self, w: WindowKey) -> Option<u64> {
        platform::native_id(&self.core.windows.get(&w)?.window)
    }

    /// Every monitor as (physical rect in virtual-desktop space, scale factor).
    pub fn monitors(&self) -> Vec<(Rect, f32)> {
        monitor_list(self.el)
    }

    pub fn scale_factor(&self, w: WindowKey) -> Option<f32> {
        self.core.windows.get(&w).map(|s| s.window.scale_factor() as f32)
    }

    /// Time from [`open`](Self::open) to the first presented frame.
    pub fn first_frame_time(&self, w: WindowKey) -> Option<Duration> {
        self.core.windows.get(&w)?.first_frame
    }

    pub fn proxy(&self) -> Proxy<E> {
        self.core.proxy.clone()
    }

    /// Ask for a repaint of one window as soon as possible.
    pub fn repaint(&self, w: WindowKey) {
        if let Some(state) = self.core.windows.get(&w) {
            state.window.request_redraw();
        }
    }

    pub fn windows_with_tag(&self, tag: &str) -> Vec<WindowKey> {
        let mut keys: Vec<_> = self.core.windows.iter().filter(|(_, s)| s.tag == tag).map(|(k, _)| *k).collect();
        keys.sort_unstable();
        keys
    }

    /// Stop the event loop; `run` returns after the current callback.
    pub fn quit(&mut self) {
        self.el.exit();
    }
}

/// Application logic driven by the shell. All callbacks run on the main thread.
pub trait ShellApp<E: Send + 'static>: 'static {
    /// Called once, after the event loop has started and before any window exists.
    fn init(&mut self, _sh: &mut Shell<E>) {}

    /// A user event sent through a [`Proxy`] from any thread.
    fn event(&mut self, sh: &mut Shell<E>, ev: E);

    /// Build the UI of one window. Called once per frame of that window.
    fn ui(&mut self, sh: &mut Shell<E>, win: WindowKey, ctx: &egui::Context);

    /// The user (or egui) asked to close a window; return false to keep it.
    fn close_requested(&mut self, _sh: &mut Shell<E>, _win: WindowKey) -> bool {
        true
    }

    /// A window has been destroyed, whether by the user or by [`Shell::close`].
    fn closed(&mut self, _sh: &mut Shell<E>, _win: WindowKey) {}
}
