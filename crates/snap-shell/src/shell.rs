//! The window registry and the public [`Shell`] handle given to the app.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use snap_core::{Error, Point, Rect, Result};
use winit::{
    dpi::{LogicalPosition, LogicalSize, PhysicalPosition, PhysicalSize, Position},
    event_loop::ActiveEventLoop,
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
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

/// The largest share of the free screen area a new app window takes.
const FIT: f64 = 0.92;

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
    /// The window with the keyboard focus; the Edit menu acts on it.
    pub focused: Option<WindowKey>,
    /// `ShellConfig::accessory`: the macOS app is regular only while an app window is shown.
    pub accessory: bool,
    /// The activation policy last set: regular (Dock icon, app switcher, menu bar) or accessory.
    pub regular: bool,
    /// The OS started the app to open files or a URL ([`Shell::launched_to_open`]).
    pub launched_to_open: bool,
}

impl<E: Send + 'static> Core<E> {
    pub fn remove(&mut self, key: WindowKey) -> bool {
        let Some(state) = self.windows.remove(&key) else {
            return false;
        };
        self.ids.remove(&state.window.id());
        if self.focused == Some(key) {
            self.focused = None;
        }
        self.closed.push(key);
        true
    }
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
            let (mut w, mut h) = (f64::from(w), f64::from(h));
            let mut min = spec.min_size.map(|(w, h)| (f64::from(w), f64::from(h)));
            let mut position: Option<Position> = spec.position.map(|p| PhysicalPosition::new(p.x, p.y).into());
            // Default sizes are made for a roomy screen. On a small one (a 13-inch laptop with larger text) the window
            // must still fit, and the OS does not pull a borderless window back onto the screen.
            if spec.is_app_window()
                && let Some(area) = platform::app::new_window_area(self.el, spec.position)
            {
                min = min.map(|(mw, mh)| (mw.min(area.w), mh.min(area.h)));
                let (min_w, min_h) = min.unwrap_or_default();
                w = w.min(area.w * FIT).max(min_w);
                h = h.min(area.h * FIT).max(min_h);
                if area.centre && position.is_none() {
                    position = Some(LogicalPosition::new(area.x + (area.w - w) / 2.0, area.y + (area.h - h) / 2.0).into());
                }
            }
            attrs = attrs.with_inner_size(LogicalSize::new(w, h));
            if let Some((w, h)) = min {
                attrs = attrs.with_min_inner_size(LogicalSize::new(w, h));
            }
            if let Some(p) = position {
                attrs = attrs.with_position(p);
            }
        }
        let attrs = platform::window_attributes(self.el, attrs, &spec, &self.core.app_id);
        let attrs = platform::app::chrome_attributes(attrs, &spec);

        let (window, target) = match self.make_window(attrs.clone(), &spec) {
            Ok(made) => made,
            // The backend may have a second choice (OpenGL to wgpu); the window it failed on is gone.
            Err(e) if self.core.gfx.fall_back(&e) => self.make_window(attrs, &spec)?,
            Err(e) => return Err(e),
        };
        platform::app::hook_edit(&window);

        let ctx = egui::Context::default();
        (self.core.theme_init)(&ctx);
        let proxy = self.core.proxy.clone();
        ctx.set_request_repaint_callback(move |info| proxy.repaint_at(key, Instant::now() + info.delay));

        let title_bar = spec.title_bar.filter(|_| spec.cover.is_none());
        let setup = Setup {
            tool_style: spec.is_tool(),
            pill: spec.pill,
            app_window: spec.is_app_window(),
            title_bar,
            controls_inset: title_bar.and_then(|height| platform::app::layout_title_bar(&window, height)),
            tag: spec.tag,
            cover: spec.cover,
            want_visible: spec.visible,
            activate: spec.activate,
            opened_at,
        };
        let mut state = WindowState::new(window.clone(), target, setup, ctx);
        // The adapter must exist before the window is first shown (it panics on a visible one); every window is created
        // hidden. It only reports to the loop when an accessibility client asks (see `Runner::accesskit`).
        if window.is_visible() != Some(true) {
            state.egui_state.init_accesskit(self.el, &window, self.core.proxy.event_loop());
        }
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
            platform::place_cover(&window, r);
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
            state.focus_on_show = false;
            platform::refresh(&state.window, state.tool_style);
        } else if state.first_pass_done {
            state.show(false);
        }
    }

    /// Take the window off the screen for as long as a screen recording runs. Wayland cannot unmap a toplevel, so
    /// there it is minimized instead, and compositors ignore a client's request to restore it: the result says
    /// whether [`set_visible`](Self::set_visible) can bring the window back.
    pub fn conceal(&mut self, w: WindowKey) -> bool {
        let Some(state) = self.core.windows.get(&w) else { return true };
        if matches!(state.window.window_handle().map(|h| h.as_raw()), Ok(RawWindowHandle::Wayland(_))) {
            state.window.set_minimized(true);
            return false;
        }
        self.set_visible(w, false);
        true
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

    /// Give the window the keyboard focus, bringing the app forward. A window that is not on screen yet (it renders its
    /// first frame hidden) takes the focus when it appears: the OS ignores the request for a hidden window.
    pub fn focus(&mut self, w: WindowKey) {
        let Some(state) = self.core.windows.get_mut(&w) else { return };
        // winit does not focus a minimized window; it has to come back first.
        if state.window.is_minimized() == Some(true) {
            state.window.set_minimized(false);
        }
        if !state.shown {
            state.focus_on_show = true;
            return;
        }
        platform::activate(&state.window);
        state.window.focus_window();
    }

    /// Whether this app is the one the user is in (macOS: the active app; elsewhere: one of its windows has focus).
    pub fn app_active(&self) -> bool {
        platform::app_active(&mut self.core.windows.values().map(|s| s.window.as_ref()))
    }

    /// Remember the app the user is in, before a window of ours takes the keyboard, so
    /// [`restore_focus`](Self::restore_focus) can hand it back. macOS only: Windows and X11 give the focus back to the
    /// window below by themselves when ours goes away.
    pub fn remember_focus(&mut self) {
        platform::remember_frontmost();
    }

    /// Hand the keyboard back to the app [`remember_focus`](Self::remember_focus) saw, when this app is still the
    /// active one. An accessory app whose last window was ordered out otherwise stays active with no key window.
    pub fn restore_focus(&mut self) {
        platform::restore_frontmost();
    }

    /// Height in points at the top of the window's screen that is hidden by a camera housing (MacBook notch); 0 where
    /// there is none.
    pub fn safe_area_top(&self, w: WindowKey) -> f32 {
        self.core.windows.get(&w).map_or(0.0, |s| platform::safe_area_top(&s.window))
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
            platform::place_cover(&state.window, r);
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

    /// The width, in egui points from the left edge (scaled by the window's zoom), that the OS's own window controls
    /// take in the title bar the app draws (`WindowSpec::title_bar`): the macOS traffic lights, 0 in full screen.
    /// `None` where the app draws its own controls (Windows, Linux, windows without a `title_bar`).
    pub fn title_bar_inset(&self, w: WindowKey) -> Option<f32> {
        let w = self.core.windows.get(&w)?;
        w.controls_inset.map(|inset| inset / w.ctx.zoom_factor())
    }

    /// The user double-clicked the title bar the app draws: do what the OS does for its own. macOS follows the Desktop
    /// & Dock setting (zoom, fill, minimize or nothing); elsewhere the window is maximized or restored.
    pub fn title_bar_double_clicked(&mut self, w: WindowKey) {
        if let Some(state) = self.core.windows.get(&w) {
            platform::app::title_bar_double_click(&state.window);
        }
    }

    /// The part of the monitor containing `at` (physical pixels) where windows belong, in physical pixels: without the
    /// menu bar and the Dock on macOS, the whole monitor elsewhere. For placing popup windows such as menus.
    pub fn work_area(&self, at: Point) -> Option<Rect> {
        platform::app::work_area(self.el, at)
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
        platform::monitors(self.el)
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

    /// Stop the event loop; `run` returns after the current callback. When macOS asked the app to quit (logout,
    /// restart, Quit in the Dock) and waits for the answer, the answer is given instead and the OS ends the process.
    pub fn quit(&mut self) {
        if platform::app::allow_terminate() {
            return;
        }
        self.el.exit();
    }

    /// The OS started the app to open files or a URL (macOS: a document or a link opened from Finder or a browser).
    /// They arrive right after [`ShellApp::init`] as [`OsRequest`]s, so the app need not show its usual first window.
    pub fn launched_to_open(&self) -> bool {
        self.core.launched_to_open
    }

    /// Rebuild the macOS app menu, after the UI language changed. Nothing to do elsewhere.
    pub fn refresh_app_menu(&mut self) {
        platform::app::build_menu();
    }
}

/// What the OS asks of the app as a whole. Only macOS sends these, from the app menu, the Dock, Finder and
/// LaunchServices; on Windows and Linux the same things arrive as command-line arguments of a second launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OsRequest {
    /// The app was launched again while it runs (Finder, Spotlight, the Dock icon).
    Reopen,
    /// A URL of a scheme the app registered (`snapoleon://...`). Untrusted: any web page can open one.
    OpenUrl(String),
    /// A document opened with the app (double-click in Finder, a drop on the Dock icon).
    OpenFile(PathBuf),
    /// "About" in the app menu.
    About,
    /// "Settings..." in the app menu (Cmd+,).
    Settings,
    /// "Quit" in the app menu (Cmd+Q), or the OS ending the session (logout, restart, Quit in the Dock). The app ends
    /// with [`Shell::quit`], which may come later (after a recording is saved).
    Quit,
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

    /// A request from the OS for the app as a whole (macOS only). The default ends the app on
    /// [`OsRequest::Quit`] and ignores the rest.
    fn os_request(&mut self, sh: &mut Shell<E>, request: OsRequest) {
        if request == OsRequest::Quit {
            sh.quit();
        }
    }
}
