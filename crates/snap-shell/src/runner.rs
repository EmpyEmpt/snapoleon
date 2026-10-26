//! The winit event loop: input routing, repaint scheduling and frame driving.

use std::{collections::HashMap, time::Instant};

use anyhow::anyhow;
use snap_core::{Error, Result};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::WindowId,
};

use crate::{
    Shell, ShellApp, ShellConfig, WindowKey, platform,
    proxy::{Msg, Proxy},
    render::{Gfx, Graphics},
    shell::Core,
};

/// Run the shell until [`Shell::quit`] is called. The loop keeps running with
/// zero windows. Must be called on the main thread (required by macOS).
pub fn run<E: Send + 'static>(cfg: ShellConfig, app: impl ShellApp<E>) -> Result<()> {
    let mut builder = EventLoop::<Msg<E>>::with_user_event();
    #[cfg(target_os = "macos")]
    if cfg.accessory {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder.with_activation_policy(ActivationPolicy::Accessory).with_activate_ignoring_other_apps(false);
    }
    let event_loop = builder.build().map_err(|e| Error::Other(anyhow!(e)))?;
    event_loop.set_control_flow(ControlFlow::Wait);

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
            self.app.init(&mut Shell { core: &mut self.core, el });
            self.flush(el);
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, msg: Msg<E>) {
        match msg {
            Msg::User(ev) => {
                self.app.event(&mut Shell { core: &mut self.core, el }, ev);
                self.flush(el);
            }
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
