//! The renderer backend: how a window's egui output reaches the screen.
//!
//! Two implementations satisfy the same two traits: OpenGL through glutin and egui_glow (`backend_gl`) and wgpu with
//! egui-wgpu (`backend_wgpu`). `build.rs` decides which are compiled in; [`Gfx`] and [`Target`] dispatch between them
//! at run time, so Windows ships both and falls back to wgpu when no usable OpenGL context can be made (remote
//! desktop, VMs, the Basic Display Adapter). The environment variable `SNAPOLEON_RENDERER` forces a backend:
//! `wgpu`, `glow` (no fallback), or `glow-fail` (OpenGL context creation is made to fail, to exercise the fallback).

use std::sync::Arc;

use snap_core::{Error, Result};
use winit::{
    event_loop::{ActiveEventLoop, OwnedDisplayHandle},
    window::{Window, WindowAttributes},
};

#[cfg(renderer_gl)]
mod backend_gl;
#[cfg(renderer_wgpu)]
mod backend_wgpu;

/// Environment variable that picks the backend.
pub(crate) const ENV: &str = "SNAPOLEON_RENDERER";

/// What came of one [`Surface::paint`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Paint {
    /// The frame reached the screen.
    Presented,
    /// Nothing was shown (no drawable area, or the compositor is not taking frames); not an error.
    Skipped,
    /// The surface had to be rebuilt; paint again right away.
    #[cfg_attr(not(renderer_wgpu), allow(dead_code, reason = "OpenGL surfaces never go stale"))]
    Retry,
}

/// The process-wide half of a backend.
pub(crate) trait Graphics: Sized {
    type Target: Surface;

    /// Begin initialising on whatever thread suits the backend. Never fails: a failure surfaces from
    /// [`create_target`](Self::create_target).
    fn start(display: OwnedDisplayHandle) -> Self;

    /// Create the native window. It is the backend's job because some of them must pick the pixel format or visual
    /// first (OpenGL), or need a window before they can initialise at all (WGL).
    fn create_window(&mut self, el: &ActiveEventLoop, attrs: WindowAttributes) -> Result<Window>;

    /// Attach a drawing surface to a window made by [`create_window`](Self::create_window).
    fn create_target(&mut self, window: &Arc<Window>) -> Result<Self::Target>;

    /// The window or target could not be made: switch to another backend if there is one. True means the caller should
    /// make a new window and try again.
    fn fall_back(&mut self, _why: &Error) -> bool {
        false
    }
}

/// The per-window half of a backend.
pub(crate) trait Surface {
    /// Largest texture edge in pixels, for egui's input.
    fn max_texture_side(&self) -> usize;

    /// Upload the frame's texture changes, draw `out` and present it. Texture changes are applied even when nothing
    /// can be drawn, so egui's view of the atlas stays true.
    fn paint(&mut self, ctx: &egui::Context, out: egui::FullOutput) -> Paint;
}

enum Backend {
    #[cfg(renderer_gl)]
    Gl(backend_gl::Gfx),
    #[cfg(renderer_wgpu)]
    Wgpu(backend_wgpu::Gfx),
}

/// The graphics state shared by every window of the shell.
pub(crate) struct Gfx {
    #[cfg_attr(not(all(renderer_gl, renderer_wgpu)), allow(dead_code, reason = "only a fallback needs it"))]
    display: OwnedDisplayHandle,
    backend: Backend,
    /// OpenGL may give way to wgpu (not when OpenGL was asked for by name).
    #[cfg_attr(not(all(renderer_gl, renderer_wgpu)), allow(dead_code, reason = "there is nothing to fall back to"))]
    may_fall_back: bool,
}

/// One window's drawing surface, renderer and textures.
pub(crate) enum Target {
    #[cfg(renderer_gl)]
    Gl(Box<backend_gl::Target>),
    #[cfg(renderer_wgpu)]
    Wgpu(Box<backend_wgpu::Target>),
}

impl Gfx {
    /// Name of the backend in use, for the log.
    pub fn name(&self) -> &'static str {
        match &self.backend {
            #[cfg(renderer_gl)]
            Backend::Gl(_) => "opengl (glutin, egui_glow)",
            #[cfg(renderer_wgpu)]
            Backend::Wgpu(_) => "wgpu",
        }
    }
}

impl Graphics for Gfx {
    type Target = Target;

    fn start(display: OwnedDisplayHandle) -> Self {
        let asked = std::env::var(ENV).ok().map(|v| v.trim().to_ascii_lowercase());
        let asked = asked.as_deref().filter(|v| !v.is_empty());
        let (want_wgpu, want_gl) = match asked {
            Some("wgpu") => (true, false),
            Some("glow" | "gl" | "opengl") => (false, true),
            Some("glow-fail") | None => (false, false),
            Some(other) => {
                tracing::warn!("{ENV}={other} is not a renderer (wgpu, glow); using the default");
                (false, false)
            }
        };
        let gfx = Self::pick(display, want_wgpu, want_gl);
        tracing::info!(renderer = gfx.name(), "snap-shell renderer");
        gfx
    }

    fn create_window(&mut self, el: &ActiveEventLoop, attrs: WindowAttributes) -> Result<Window> {
        match &mut self.backend {
            #[cfg(renderer_gl)]
            Backend::Gl(g) => g.create_window(el, attrs),
            #[cfg(renderer_wgpu)]
            Backend::Wgpu(g) => g.create_window(el, attrs),
        }
    }

    fn create_target(&mut self, window: &Arc<Window>) -> Result<Target> {
        match &mut self.backend {
            #[cfg(renderer_gl)]
            Backend::Gl(g) => g.create_target(window).map(|t| Target::Gl(Box::new(t))),
            #[cfg(renderer_wgpu)]
            Backend::Wgpu(g) => g.create_target(window).map(|t| Target::Wgpu(Box::new(t))),
        }
    }

    fn fall_back(&mut self, why: &Error) -> bool {
        #[cfg(all(renderer_gl, renderer_wgpu))]
        if self.may_fall_back && matches!(self.backend, Backend::Gl(_)) {
            tracing::warn!("OpenGL is not usable ({why}); falling back to wgpu");
            self.backend = Backend::Wgpu(backend_wgpu::Gfx::start(self.display.clone()));
            self.may_fall_back = false;
            tracing::info!(renderer = self.name(), "snap-shell renderer");
            return true;
        }
        let _ = why;
        false
    }
}

impl Gfx {
    /// Choose the starting backend from what is compiled in and what was asked for.
    #[allow(unused_variables, reason = "which flags matter depends on the compiled backends")]
    fn pick(display: OwnedDisplayHandle, want_wgpu: bool, want_gl: bool) -> Self {
        #[cfg(all(renderer_gl, renderer_wgpu))]
        {
            if want_wgpu {
                return Self { backend: Backend::Wgpu(backend_wgpu::Gfx::start(display.clone())), display, may_fall_back: false };
            }
            Self { backend: Backend::Gl(backend_gl::Gfx::start(display.clone())), display, may_fall_back: !want_gl }
        }
        #[cfg(all(renderer_gl, not(renderer_wgpu)))]
        {
            if want_wgpu {
                tracing::warn!("{ENV}=wgpu: this build has no wgpu renderer");
            }
            Self { backend: Backend::Gl(backend_gl::Gfx::start(display.clone())), display, may_fall_back: false }
        }
        #[cfg(all(renderer_wgpu, not(renderer_gl)))]
        {
            if want_gl {
                tracing::warn!("{ENV}=glow: this build has no OpenGL renderer");
            }
            Self { backend: Backend::Wgpu(backend_wgpu::Gfx::start(display.clone())), display, may_fall_back: false }
        }
    }
}

impl Surface for Target {
    fn max_texture_side(&self) -> usize {
        match self {
            #[cfg(renderer_gl)]
            Target::Gl(t) => t.max_texture_side(),
            #[cfg(renderer_wgpu)]
            Target::Wgpu(t) => t.max_texture_side(),
        }
    }

    fn paint(&mut self, ctx: &egui::Context, out: egui::FullOutput) -> Paint {
        match self {
            #[cfg(renderer_gl)]
            Target::Gl(t) => t.paint(ctx, out),
            #[cfg(renderer_wgpu)]
            Target::Wgpu(t) => t.paint(ctx, out),
        }
    }
}
