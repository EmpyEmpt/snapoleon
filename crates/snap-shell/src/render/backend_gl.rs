//! OpenGL backend: glutin and glutin-winit for the context, egui_glow for drawing.
//!
//! One GL context serves every window. Each window owns a glutin surface (on WGL a device context with the shared pixel
//! format) and its own `egui_glow::Painter`; before touching either, the context is made current on that window's
//! surface. All of it runs on the UI thread, so there is no worker-thread start-up as with wgpu: the first window
//! brings up the display, which WGL cannot do without a window anyway.

use std::{num::NonZeroU32, rc::Rc, sync::Arc};

use anyhow::anyhow;
use egui::FullOutput;
use egui_glow::Painter;
use glutin::{
    config::{Config, ConfigTemplateBuilder, GlConfig},
    context::{ContextApi, ContextAttributesBuilder, PossiblyCurrentContext, Version},
    display::{GetGlDisplay, GlDisplay},
    prelude::{GlSurface, NotCurrentGlContext, PossiblyCurrentGlContext},
    surface::{Surface as GlSurfaceHandle, SwapInterval, WindowSurface},
};
use glutin_winit::{DisplayBuilder, GlWindow};
use snap_core::{Error, Result};
use winit::{
    dpi::PhysicalSize,
    event_loop::{ActiveEventLoop, OwnedDisplayHandle},
    raw_window_handle::HasWindowHandle,
    window::{Window, WindowAttributes},
};

use super::{Graphics, Paint, Surface};

fn other(e: impl std::fmt::Display) -> Error {
    Error::Other(anyhow!("{e}"))
}

/// What every window shares: the context and the function table loaded through it.
struct Shared {
    // Declaration order is drop order: the table before the context before the config that made it.
    gl: Arc<glow::Context>,
    ctx: PossiblyCurrentContext,
    _config: Config,
}

pub(crate) struct Gfx {
    /// Picked together with the first window, then reused so every window has the same pixel format.
    config: Option<Config>,
    shared: Option<Rc<Shared>>,
}

impl Gfx {
    /// Prefer the plainest framebuffer: the shell draws opaque windows and egui needs no depth, stencil or MSAA.
    fn pick(configs: Box<dyn Iterator<Item = Config> + '_>) -> Config {
        configs.min_by_key(|c| (c.num_samples(), c.depth_size(), c.stencil_size())).expect("glutin returned at least one config")
    }

    /// The first context that works and is new enough: desktop OpenGL, then OpenGL ES 3.0 and 2.0. Drivers such as
    /// Mesa's VC4 and V3D hand out a desktop 2.1 compatibility context next to a capable ES one, so a context that
    /// was created but is too old does not end the search.
    fn open_context(
        config: &Config,
        window: &Window,
        surface: &GlSurfaceHandle<WindowSurface>,
    ) -> Result<(PossiblyCurrentContext, Arc<glow::Context>)> {
        if std::env::var(super::ENV).is_ok_and(|v| v.eq_ignore_ascii_case("glow-fail")) {
            return Err(other("simulated failure (SNAPOLEON_RENDERER=glow-fail)"));
        }
        let handle = window.window_handle().ok().map(|h| h.as_raw());
        let display = config.display();
        let gles = |version| ContextAttributesBuilder::new().with_context_api(ContextApi::Gles(version)).build(handle);
        let candidates = [ContextAttributesBuilder::new().build(handle), gles(Some(Version::new(3, 0))), gles(Some(Version::new(2, 0)))];
        let mut last = other("no OpenGL context could be created");
        for attrs in &candidates {
            // SAFETY: `handle` belongs to `window`, which the caller keeps alive for the call.
            let ctx = match unsafe { display.create_context(config, attrs) }
                .map_err(other)
                .and_then(|c| c.make_current(surface).map_err(other))
            {
                Ok(ctx) => ctx,
                Err(e) => {
                    last = e;
                    continue;
                }
            };
            // SAFETY: the context is current on this thread, and `get_proc_address` is the loader for it.
            let gl = Arc::new(unsafe { glow::Context::from_loader_function_cstr(|name| display.get_proc_address(name)) });
            let version = glow::HasContext::version(&*gl);
            if if version.is_embedded { version.major >= 2 } else { version.major >= 3 } {
                return Ok((ctx, gl));
            }
            // A software-only driver (Basic Display Adapter, remote desktop) offers OpenGL 1.1.
            last = other(format_args!(
                "OpenGL {}.{}{} is too old, need 3.0 or ES 2.0",
                version.major,
                version.minor,
                if version.is_embedded { " ES" } else { "" }
            ));
            drop(gl);
            let _ = ctx.make_not_current_in_place();
        }
        Err(last)
    }
}

impl Graphics for Gfx {
    type Target = Target;

    fn start(_display: OwnedDisplayHandle) -> Self {
        Self { config: None, shared: None }
    }

    fn create_window(&mut self, el: &ActiveEventLoop, attrs: WindowAttributes) -> Result<Window> {
        if let Some(config) = &self.config {
            return glutin_winit::finalize_window(el, attrs, config).map_err(other);
        }
        let template = ConfigTemplateBuilder::new().with_depth_size(0).with_stencil_size(0).with_transparency(false);
        let (window, config) = DisplayBuilder::new()
            .with_window_attributes(Some(attrs))
            .build(el, template, Self::pick)
            .map_err(|e| other(format_args!("OpenGL is not available: {e}")))?;
        self.config = Some(config);
        window.ok_or_else(|| other("the window was not created"))
    }

    fn create_target(&mut self, window: &Arc<Window>) -> Result<Target> {
        let config = self.config.clone().ok_or_else(|| other("no OpenGL config; the window did not come from create_window"))?;
        let display = config.display();
        let attrs = window.build_surface_attributes(Default::default()).map_err(other)?;
        // SAFETY: the handle in `attrs` is `window`'s, and the surface is dropped before the window (see `Target`).
        let surface = unsafe { display.create_window_surface(&config, &attrs) }.map_err(other)?;

        let shared = match &self.shared {
            Some(shared) => {
                shared.ctx.make_current(&surface).map_err(other)?;
                shared.clone()
            }
            None => {
                let (ctx, gl) = Self::open_context(&config, window, &surface)?;
                let shared = Rc::new(Shared { gl, ctx, _config: config });
                self.shared = Some(shared.clone());
                shared
            }
        };
        // Vsync, one blank per frame; hidden or occluded windows are free to refuse.
        if let Err(e) = surface.set_swap_interval(&shared.ctx, SwapInterval::Wait(NonZeroU32::MIN)) {
            tracing::debug!("swap interval: {e}");
        }
        let painter = Painter::new(shared.gl.clone(), "", None, true).map_err(|e| other(format_args!("egui_glow painter: {e}")))?;
        Ok(Target { painter: Some(painter), surface, size: window.inner_size(), window: window.clone(), shared })
    }
}

pub(crate) struct Target {
    // Drop order matters: the painter's GL objects are freed in `drop` below, then the surface goes before the window.
    painter: Option<Painter>,
    surface: GlSurfaceHandle<WindowSurface>,
    size: PhysicalSize<u32>,
    window: Arc<Window>,
    shared: Rc<Shared>,
}

impl Target {
    /// Make the shared context current on this window's surface and keep it in step with the window's size.
    fn bind(&mut self, size: PhysicalSize<u32>) -> bool {
        if let Err(e) = self.shared.ctx.make_current(&self.surface) {
            tracing::warn!("making the OpenGL context current failed: {e}");
            return false;
        }
        if size != self.size
            && let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        {
            self.surface.resize(&self.shared.ctx, w, h);
            self.size = size;
        }
        true
    }
}

impl Surface for Target {
    fn max_texture_side(&self) -> usize {
        self.painter.as_ref().map_or(2048, Painter::max_texture_side)
    }

    fn paint(&mut self, ctx: &egui::Context, mut out: FullOutput) -> Paint {
        let size = self.window.inner_size();
        if !self.bind(size) {
            out.textures_delta.clear();
            return Paint::Skipped;
        }
        let Some(painter) = self.painter.as_mut() else { return Paint::Skipped };
        let screen = [size.width, size.height];
        if size.width == 0 || size.height == 0 {
            painter.paint_and_update_textures(screen, out.pixels_per_point, &[], &mut out.textures_delta);
            return Paint::Skipped;
        }

        let primitives = ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
        let [r, g, b, _] = ctx.global_style().visuals.panel_fill.to_normalized_gamma_f32();
        painter.clear(screen, [r, g, b, 1.0]);
        painter.paint_and_update_textures(screen, out.pixels_per_point, &primitives, &mut out.textures_delta);

        self.window.pre_present_notify();
        match self.surface.swap_buffers(&self.shared.ctx) {
            Ok(()) => Paint::Presented,
            Err(e) => {
                tracing::warn!("presenting the OpenGL frame failed: {e}");
                Paint::Skipped
            }
        }
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        if let Some(mut painter) = self.painter.take() {
            if self.shared.ctx.make_current(&self.surface).is_ok() {
                painter.destroy();
            }
            // Leave nothing current on a surface that is about to go away.
            let _ = self.shared.ctx.make_not_current_in_place();
        }
    }
}
