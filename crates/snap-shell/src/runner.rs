//! The winit event loop: input routing, repaint scheduling and frame driving.

use std::{collections::HashMap, time::Instant};

use anyhow::anyhow;
use egui_winit::accesskit_winit;
use snap_core::{Error, Result};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::WindowId,
};

use crate::{
    OsRequest, Shell, ShellApp, ShellConfig, WindowKey, platform,
    proxy::{AppHook, Msg, Proxy},
    render::{Gfx, Graphics},
    shell::Core,
};

/// Run the shell until [`Shell::quit`] is called. The loop keeps running with
/// zero windows. Must be called on the main thread (required by macOS).
pub fn run<E: Send + 'static>(cfg: ShellConfig, app: impl ShellApp<E>) -> Result<()> {
    let mut builder = EventLoop::<Msg<E>>::with_user_event();
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        // The shell builds the app menu itself (`platform::app`), with its own Quit.
        builder.with_default_menu(false);
        if cfg.accessory {
            builder.with_activation_policy(ActivationPolicy::Accessory).with_activate_ignoring_other_apps(false);
        }
    }
    let event_loop = builder.build().map_err(|e| Error::Other(anyhow!(e)))?;
    event_loop.set_control_flow(ControlFlow::Wait);
    // Before the loop runs: the OS hands over the file or URL the app was launched for while it starts up.
    let hooks = event_loop.create_proxy();
    platform::app::install(Box::new(move |hook| hooks.send_event(Msg::Hook(hook)).is_ok()));

    let display = event_loop.owned_display_handle();
    let mut runner = Runner {
        app,
        core: Core {
            app_id: cfg.app_id,
            theme_init: cfg.theme_init,
            proxy: Proxy::new(event_loop.create_proxy()),
            gfx: Gfx::start(display),
            windows: HashMap::new(),
            ids: HashMap::new(),
            next_key: 1,
            closed: Vec::new(),
            focused: None,
            accessory: cfg.accessory,
            regular: !cfg.accessory,
            launched_to_open: false,
        },
        started: false,
    };
    event_loop.run_app(&mut runner).map_err(|e| Error::Other(anyhow!(e)))
}

struct Runner<E: Send + 'static, A: ShellApp<E>> {
    app: A,
    core: Core<E>,
    started: bool,
}

impl<E: Send + 'static, A: ShellApp<E>> Runner<E, A> {
    /// Deliver `closed` notifications queued by `Shell::close` and friends.
    fn flush(&mut self, el: &ActiveEventLoop) {
        while !self.core.closed.is_empty() {
            for key in std::mem::take(&mut self.core.closed) {
                self.app.closed(&mut Shell { core: &mut self.core, el }, key);
            }
        }
    }

    fn request_close(&mut self, el: &ActiveEventLoop, key: WindowKey) {
        let mut sh = Shell { core: &mut self.core, el };
        if self.app.close_requested(&mut sh, key) {
            sh.close(key);
        }
        self.flush(el);
    }

    fn hook(&mut self, el: &ActiveEventLoop, hook: AppHook) {
        match hook {
            AppHook::Request(request) => {
                self.app.os_request(&mut Shell { core: &mut self.core, el }, request);
                self.flush(el);
            }
            AppHook::Edit(action) => {
                if let Some(w) = self.core.focused.and_then(|key| self.core.windows.get_mut(&key)) {
                    w.edit(action);
                }
            }
        }
    }

    /// macOS: a regular app (Dock icon, app switcher, menu bar) while an app window is shown, an accessory one
    /// otherwise. Runs before new windows are shown, so they come up with the app menu.
    fn sync_activation_policy(&mut self) {
        if !self.core.accessory {
            return;
        }
        let regular = self.core.windows.values().any(|w| w.app_window && w.want_visible);
        if regular != self.core.regular {
            self.core.regular = regular;
            platform::app::set_regular(regular);
        }
    }

    fn schedule(&mut self, key: WindowKey, at: Instant) {
        let Some(w) = self.core.windows.get_mut(&key) else { return };
        if at <= Instant::now() {
            w.window.request_redraw();
        } else {
            w.repaint_at = Some(w.repaint_at.map_or(at, |old| old.min(at)));
        }
    }

    fn redraw(&mut self, el: &ActiveEventLoop, key: WindowKey) {
        let Some(w) = self.core.windows.get_mut(&key) else { return };
        w.repaint_at = None;
        let Some(input) = w.begin_frame() else {
            w.first_pass_done = true;
            return;
        };
        let ctx = w.ctx.clone();
        let mut out = ctx.run_pass(input, |ctx| {
            self.app.ui(&mut Shell { core: &mut self.core, el }, key, ctx);
        });
        self.flush(el);

        let Some(w) = self.core.windows.get_mut(&key) else {
            out.drop_without_applying_deltas();
            return;
        };
        let close = w.handle_output(el, &mut out);
        // AppKit puts the traffic lights back into its own title bar now and then (a changed style or appearance
        // among them); on most frames this only checks that they are still where they belong.
        if let Some(height) = w.title_bar {
            w.controls_inset = platform::app::layout_title_bar(&w.window, height);
        }
        let retry = w.paint(out);
        w.first_pass_done = true;
        if retry {
            w.window.request_redraw();
        }
        if w.want_visible && !w.shown {
            w.show(true);
        }
        if close {
            self.request_close(el, key);
        }
    }

    /// egui builds its accessibility tree only while a client (VoiceOver, an agent reading the UI) is attached, so a
    /// window without one pays nothing for it.
    fn accesskit(&mut self, event: accesskit_winit::Event) {
        let Some(w) = self.core.ids.get(&event.window_id).and_then(|key| self.core.windows.get_mut(key)) else { return };
        match event.window_event {
            accesskit_winit::WindowEvent::InitialTreeRequested => w.ctx.enable_accesskit(),
            accesskit_winit::WindowEvent::ActionRequested(request) => w.egui_state.on_accesskit_action_request(request),
            accesskit_winit::WindowEvent::AccessibilityDeactivated => return w.ctx.disable_accesskit(),
        }
        w.window.request_redraw();
    }

    /// Re-apply an overlay's exact rect when the OS has moved or rescaled it.
    fn enforce_cover(&mut self, key: WindowKey) {
        let Some(w) = self.core.windows.get_mut(&key) else { return };
        let Some(want) = w.cover else { return };
        w.cover_dirty = false;
        let Some(have) = platform::outer_rect(&w.window) else { return };
        let tol = platform::COVER_TOLERANCE;
        let close_enough = (have.x - want.x).abs() <= tol
            && (have.y - want.y).abs() <= tol
            && have.w.abs_diff(want.w) as i32 <= tol
            && have.h.abs_diff(want.h) as i32 <= tol;
        if close_enough {
            w.cover_attempts = 0;
        } else if w.cover_attempts < 8 {
            w.cover_attempts += 1;
            platform::place_cover(&w.window, want);
        } else {
            tracing::warn!(key, ?want, ?have, "the OS keeps refusing the exact cover rect");
        }
    }
}

impl<E: Send + 'static, A: ShellApp<E>> ApplicationHandler<Msg<E>> for Runner<E, A> {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if !std::mem::replace(&mut self.started, true) {
            let launch = platform::app::start();
            self.core.launched_to_open =
                launch.iter().any(|hook| matches!(hook, AppHook::Request(OsRequest::OpenUrl(_) | OsRequest::OpenFile(_))));
            self.app.init(&mut Shell { core: &mut self.core, el });
            self.flush(el);
            for hook in launch {
                self.hook(el, hook);
            }
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, msg: Msg<E>) {
        match msg {
            Msg::User(ev) => {
                self.app.event(&mut Shell { core: &mut self.core, el }, ev);
                self.flush(el);
            }
            Msg::Hook(hook) => self.hook(el, hook),
            Msg::AccessKit(event) => self.accesskit(event),
            Msg::Repaint { key, at } => self.schedule(key, at),
            Msg::RepaintAll => {
                let now = Instant::now();
                let keys: Vec<_> = self.core.windows.keys().copied().collect();
                for key in keys {
                    self.schedule(key, now);
                }
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, mut event: WindowEvent) {
        let Some(&key) = self.core.ids.get(&id) else { return };
        match event {
            WindowEvent::RedrawRequested => return self.redraw(el, key),
            WindowEvent::CloseRequested => return self.request_close(el, key),
            WindowEvent::Destroyed => return,
            _ => {}
        }

        let Some(w) = self.core.windows.get_mut(&key) else { return };
        match &mut event {
            WindowEvent::Focused(true) => self.core.focused = Some(key),
            WindowEvent::Focused(false) if self.core.focused == Some(key) => self.core.focused = None,
            WindowEvent::ScaleFactorChanged { inner_size_writer, .. } => {
                if let Some(cover) = w.cover {
                    w.cover_attempts = 0;
                    let _ = inner_size_writer.request_inner_size(PhysicalSize::new(cover.w, cover.h));
                }
                w.cover_dirty = w.cover.is_some();
            }
            WindowEvent::Resized(_) | WindowEvent::Moved(_) => {
                w.cover_dirty = w.cover.is_some();
                if w.pill && matches!(event, WindowEvent::Resized(_)) {
                    platform::set_pill(&w.window, true);
                }
                // AppKit has just laid its title bar out for the new size; redo ours before the window is drawn.
                if let (Some(height), WindowEvent::Resized(_)) = (w.title_bar, &event) {
                    w.controls_inset = platform::app::layout_title_bar(&w.window, height);
                }
            }
            _ => {}
        }
        let response = w.egui_state.on_window_event(&w.window, &event);
        if response.repaint || matches!(event, WindowEvent::Resized(_) | WindowEvent::Occluded(false)) {
            w.window.request_redraw();
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        self.flush(el);
        self.sync_activation_policy();

        // First frames render here, hidden, so showing a window never flashes blank.
        let fresh: Vec<_> = self.core.windows.iter().filter(|(_, w)| !w.first_pass_done).map(|(k, _)| *k).collect();
        for key in fresh {
            self.redraw(el, key);
        }

        let dirty: Vec<_> = self.core.windows.iter().filter(|(_, w)| w.cover_dirty).map(|(k, _)| *k).collect();
        for key in dirty {
            self.enforce_cover(key);
        }

        let now = Instant::now();
        let mut next: Option<Instant> = None;
        for w in self.core.windows.values_mut() {
            match w.repaint_at {
                Some(at) if at <= now => {
                    w.repaint_at = None;
                    w.window.request_redraw();
                }
                Some(at) => next = Some(next.map_or(at, |n| n.min(at))),
                None => {}
            }
        }
        el.set_control_flow(next.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
    }
}
