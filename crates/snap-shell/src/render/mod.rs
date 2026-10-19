//! The renderer backend: how a window's egui output reaches the screen.
//!
//! Two implementations satisfy the same two traits and exactly one is compiled in (see `build.rs`): OpenGL through
//! glutin and egui_glow (`backend_gl`, the default on Windows and Linux) and wgpu with egui-wgpu (`backend_wgpu`, the
//! `wgpu` feature, and always on macOS). Nothing outside this module knows which one it is.

use std::sync::Arc;

use snap_core::Result;
use winit::{
    event_loop::{ActiveEventLoop, OwnedDisplayHandle},
    window::{Window, WindowAttributes},
};

#[cfg(renderer_glow)]
mod backend_gl;
#[cfg(renderer_wgpu)]
mod backend_wgpu;

#[cfg(renderer_glow)]
use backend_gl as backend;
#[cfg(renderer_wgpu)]
use backend_wgpu as backend;

/// The graphics state shared by every window of the shell.
pub(crate) type Gfx = backend::Gfx;
/// One window's drawing surface, renderer and textures.
pub(crate) type Target = backend::Target;

/// Name of the compiled-in backend, for the log.
pub const NAME: &str = if cfg!(renderer_glow) { "opengl (glutin, egui_glow)" } else { "wgpu" };

/// What came of one [`Surface::paint`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Paint {
    /// The frame reached the screen.
    Presented,
    /// Nothing was shown (no drawable area, or the compositor is not taking frames); not an error.
    Skipped,
    /// The surface had to be rebuilt; paint again right away.
    #[cfg_attr(renderer_glow, allow(dead_code, reason = "OpenGL surfaces never go stale"))]
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
}

/// The per-window half of a backend.
pub(crate) trait Surface {
    /// Largest texture edge in pixels, for egui's input.
    fn max_texture_side(&self) -> usize;

    /// Upload the frame's texture changes, draw `out` and present it. Texture changes are applied even when nothing
    /// can be drawn, so egui's view of the atlas stays true.
    fn paint(&mut self, ctx: &egui::Context, out: egui::FullOutput) -> Paint;
}
