//! Per-window state: native window, wgpu surface, egui context and renderer.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::anyhow;
use egui::{FullOutput, RawInput, ViewportId};
use snap_core::{Error, Rect, Result};
use winit::{event_loop::ActiveEventLoop, window::Window};

use crate::gpu::Gpu;

pub(crate) struct WindowState {
    pub window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    needs_reconfigure: bool,
    renderer: egui_wgpu::Renderer,
    pub ctx: egui::Context,
    pub egui_state: egui_winit::State,
    viewport_info: egui::ViewportInfo,
    first_input: bool,

    pub tag: String,
    pub cover: Option<Rect>,
    pub cover_dirty: bool,
    pub cover_attempts: u8,
    pub tool_style: bool,
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
    pub want_visible: bool,
    pub activate: bool,
    pub opened_at: Instant,
}

impl WindowState {
    pub fn new(gpu: &Gpu, window: Arc<Window>, surface: wgpu::Surface<'static>, setup: Setup, ctx: egui::Context) -> Result<Self> {
        let size = window.inner_size();
        let caps = surface.get_capabilities(&gpu.adapter);
        let format = egui_wgpu::preferred_framebuffer_format(&caps.formats).map_err(|e| Error::Other(anyhow!("surface format: {e}")))?;
        let alpha_mode = if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::Opaque) {
            wgpu::CompositeAlphaMode::Opaque
        } else {
            caps.alpha_modes.first().copied().unwrap_or_default()
        };
        let mut config = surface
            .get_default_config(&gpu.adapter, size.width.max(1), size.height.max(1))
            .ok_or_else(|| Error::Other(anyhow!("surface is not supported by the selected GPU adapter")))?;
        config.format = format;
        config.alpha_mode = alpha_mode;
        config.present_mode = wgpu::PresentMode::AutoVsync;
        config.desired_maximum_frame_latency = 1;
        surface.configure(&gpu.device, &config);

        let renderer = egui_wgpu::Renderer::new(&gpu.device, format, egui_wgpu::RendererOptions::default());
        let egui_state = egui_winit::State::new(
            ctx.clone(),
            ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(gpu.device.limits().max_texture_dimension_2d as usize),
        );

        Ok(Self {
            window,
            surface,
            config,
            needs_reconfigure: false,
            renderer,
            ctx,
            egui_state,
            viewport_info: egui::ViewportInfo::default(),
            first_input: true,
            tag: setup.tag,
            cover: setup.cover,
            cover_dirty: setup.cover.is_some(),
            cover_attempts: 0,
            tool_style: setup.tool_style,
            want_visible: setup.want_visible,
            activate: setup.activate,
            first_pass_done: false,
            shown: false,
            pending_passthrough: None,
            opened_at: setup.opened_at,
            first_frame: None,
            repaint_at: None,
        })
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

    fn reconfigure(&mut self, gpu: &Gpu) {
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&gpu.device, &self.config);
        self.needs_reconfigure = false;
    }

    /// Render one egui frame. Returns true when the frame could not be
    /// presented and a new one should be rendered right away.
    pub fn paint(&mut self, gpu: &Gpu, mut out: FullOutput) -> bool {
        for (id, deltas) in out.textures_delta.set.drain() {
            for delta in deltas {
                self.renderer.update_texture(&gpu.device, &gpu.queue, id, &delta);
            }
        }
        let retry = self.render(gpu, &mut out);
        for id in out.textures_delta.free.drain() {
            self.renderer.free_texture(&id);
        }
        retry
    }

    fn render(&mut self, gpu: &Gpu, out: &mut FullOutput) -> bool {
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return false;
        }
        if self.needs_reconfigure || size.width != self.config.width || size.height != self.config.height {
            self.reconfigure(gpu);
        }

        let primitives = self.ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
        let screen =
            egui_wgpu::ScreenDescriptor { size_in_pixels: [self.config.width, self.config.height], pixels_per_point: out.pixels_per_point };
        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("snap-shell frame") });
        let extra = self.renderer.update_buffers(&gpu.device, &gpu.queue, &mut encoder, &primitives, &screen);

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                self.needs_reconfigure = true;
                frame
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.reconfigure(gpu);
                return true;
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                match gpu.instance.create_surface(self.window.clone()) {
                    Ok(surface) => self.surface = surface,
                    Err(e) => tracing::error!("recreating lost surface failed: {e}"),
                }
                self.reconfigure(gpu);
                return true;
            }
            wgpu::CurrentSurfaceTexture::Occluded | wgpu::CurrentSurfaceTexture::Timeout => return false,
            wgpu::CurrentSurfaceTexture::Validation => {
                tracing::warn!("surface texture acquisition failed validation");
                return false;
            }
        };

        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let clear = self.ctx.global_style().visuals.panel_fill.to_normalized_gamma_f32();
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("snap-shell egui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: clear[0] as f64, g: clear[1] as f64, b: clear[2] as f64, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.renderer.render(&mut pass.forget_lifetime(), &primitives, &screen);
        }
        gpu.queue.submit(extra.into_iter().chain([encoder.finish()]));
        self.window.pre_present_notify();
        gpu.queue.present(frame);

        self.first_frame.get_or_insert_with(|| self.opened_at.elapsed());
        false
    }
}
