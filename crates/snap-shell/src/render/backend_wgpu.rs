//! wgpu backend: one instance/adapter/device/queue shared by every window, one surface and `egui_wgpu::Renderer` each.

use std::{sync::Arc, thread::JoinHandle};

use anyhow::anyhow;
use egui::FullOutput;
use snap_core::{Error, Result};
use winit::{
    event_loop::{ActiveEventLoop, OwnedDisplayHandle},
    window::{Window, WindowAttributes},
};

use super::{Graphics, Paint, Surface};

const VENDOR_AMD: u32 = 0x1002;

#[derive(Clone)]
struct Gpu {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

/// DX12 on Windows, Metal on macOS, Vulkan and GL on Linux. Windows also
/// enables Vulkan, but only as a per-adapter fallback (see [`rank`]).
fn backends() -> wgpu::Backends {
    wgpu::Backends::from_env().unwrap_or(if cfg!(windows) {
        wgpu::Backends::DX12 | wgpu::Backends::VULKAN
    } else if cfg!(target_os = "macos") {
        wgpu::Backends::METAL
    } else {
        wgpu::Backends::VULKAN | wgpu::Backends::GL
    })
}

impl Gpu {
    /// Create the instance and pick an adapter without a surface, so this can
    /// run on a worker thread before the first window exists.
    fn create(display: OwnedDisplayHandle) -> Result<Self> {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = backends();
        desc.flags = wgpu::InstanceFlags::from_build_config().with_env();
        desc.backend_options = wgpu::BackendOptions::from_env_or_default();
        desc.display = Some(Box::new(display));
        Self::with_instance(wgpu::Instance::new(desc), None)
    }

    /// Pick an adapter and open a device on it. `surface` restricts the choice
    /// to adapters that can present to it.
    fn with_instance(instance: wgpu::Instance, surface: Option<&wgpu::Surface<'_>>) -> Result<Self> {
        let adapter = pick_adapter(&instance, surface)?;
        let info = adapter.get_info();
        tracing::debug!(name = %info.name, backend = ?info.backend, "gpu adapter selected");

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("snap-shell"),
            required_limits: wgpu::Limits::default().using_resolution(adapter.limits()).using_alignment(adapter.limits()),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            ..Default::default()
        }))
        .map_err(|e| Error::Other(anyhow!("gpu device request failed: {e}")))?;

        device.on_uncaptured_error(Arc::new(|e| tracing::error!("wgpu error: {e}")));
        device.set_device_lost_callback(|reason, msg| tracing::error!(?reason, "gpu device lost: {msg}"));
        Ok(Self { instance, adapter, device, queue })
    }
}

/// Lower is better: low-power adapters first (unless `WGPU_POWER_PREF=high`),
/// software rasterizers last. Between backends of the same GPU, DX12 wins on
/// Windows except on AMD, whose DX12 driver keeps a background thread at 100%
/// CPU after egui's first draw (verified on Radeon iGPUs); Vulkan is idle there.
fn rank(info: &wgpu::AdapterInfo, high_performance: bool) -> (u8, u8) {
    use wgpu::{Backend, DeviceType};
    let kind = match (info.device_type, high_performance) {
        (DeviceType::IntegratedGpu, false) | (DeviceType::DiscreteGpu, true) => 0,
        (DeviceType::DiscreteGpu, false) | (DeviceType::IntegratedGpu, true) => 1,
        (DeviceType::VirtualGpu, _) => 2,
        (DeviceType::Other, _) => 3,
        (DeviceType::Cpu, _) => 4,
    };
    let prefer_vulkan = cfg!(windows) && info.vendor == VENDOR_AMD;
    let backend = match info.backend {
        Backend::Vulkan if prefer_vulkan => 0,
        Backend::Dx12 if !prefer_vulkan => 0,
        Backend::Metal => 0,
        Backend::Vulkan | Backend::Dx12 => 1,
        _ => 2,
    };
    (kind, backend)
}

fn pick_adapter(instance: &wgpu::Instance, surface: Option<&wgpu::Surface<'_>>) -> Result<wgpu::Adapter> {
    let power = wgpu::PowerPreference::from_env().unwrap_or(wgpu::PowerPreference::LowPower);
    let high_performance = power == wgpu::PowerPreference::HighPerformance;

    let mut adapters = pollster::block_on(instance.enumerate_adapters(backends()));
    adapters.retain(|a| surface.is_none_or(|s| a.is_surface_supported(s)));
    adapters.sort_by_key(|a| rank(&a.get_info(), high_performance));
    if let Some(adapter) = adapters.into_iter().next() {
        return Ok(adapter);
    }

    for force_fallback_adapter in [false, true] {
        let options = wgpu::RequestAdapterOptions {
            power_preference: power,
            force_fallback_adapter,
            compatible_surface: surface,
            apply_limit_buckets: false,
        };
        if let Ok(adapter) = pollster::block_on(instance.request_adapter(&options)) {
            return Ok(adapter);
        }
    }
    Err(Error::Other(anyhow!("no usable GPU adapter found")))
}

/// The GPU is created on a worker thread at start-up so the first window does not pay for adapter and device
/// initialisation.
enum Slot {
    Pending(JoinHandle<Result<Gpu>>),
    Ready(Gpu),
    Absent,
}

pub(crate) struct Gfx {
    display: OwnedDisplayHandle,
    slot: Slot,
    /// One clone per live [`Target`]: the count says whether anything still uses the device.
    users: Arc<()>,
}

impl Gfx {
    /// Wait for the start-up GPU initialisation, or retry it from scratch.
    fn gpu(&mut self) -> Result<&Gpu> {
        self.slot = match std::mem::replace(&mut self.slot, Slot::Absent) {
            Slot::Pending(handle) => match handle.join() {
                Ok(Ok(gpu)) => Slot::Ready(gpu),
                Ok(Err(e)) => {
                    tracing::warn!("gpu start-up failed: {e}");
                    Slot::Absent
                }
                Err(_) => {
                    tracing::warn!("gpu start-up thread panicked");
                    Slot::Absent
                }
            },
            other => other,
        };
        if matches!(self.slot, Slot::Absent) {
            self.slot = Slot::Ready(Gpu::create(self.display.clone())?);
        }
        match &self.slot {
            Slot::Ready(gpu) => Ok(gpu),
            _ => Err(Error::Other(anyhow!("no GPU available"))),
        }
    }
}

impl Graphics for Gfx {
    type Target = Target;

    fn start(display: OwnedDisplayHandle) -> Self {
        let worker = {
            let display = display.clone();
            std::thread::Builder::new().name("snap-shell-gpu".into()).spawn(move || Gpu::create(display))
        };
        let slot = match worker {
            Ok(handle) => Slot::Pending(handle),
            Err(e) => {
                tracing::warn!("gpu start-up thread failed to spawn: {e}");
                Slot::Absent
            }
        };
        Self { display, slot, users: Arc::new(()) }
    }

    fn create_window(&mut self, el: &ActiveEventLoop, attrs: WindowAttributes) -> Result<Window> {
        el.create_window(attrs).map_err(|e| Error::Other(anyhow!(e)))
    }

    fn create_target(&mut self, window: &Arc<Window>) -> Result<Target> {
        let mut gpu = self.gpu()?.clone();
        let mut surface = gpu.instance.create_surface(window.clone()).map_err(|e| Error::Other(anyhow!(e)))?;
        if !gpu.adapter.is_surface_supported(&surface) {
            // The adapter picked before any window existed cannot present here; only possible to repair while
            // nothing else uses the device.
            if Arc::strong_count(&self.users) > 1 {
                return Err(Error::Other(anyhow!("surface is not supported by the shared GPU adapter")));
            }
            gpu = Gpu::with_instance(gpu.instance.clone(), Some(&surface))?;
            surface = gpu.instance.create_surface(window.clone()).map_err(|e| Error::Other(anyhow!(e)))?;
            self.slot = Slot::Ready(gpu.clone());
        }
        Target::new(gpu, window.clone(), surface, self.users.clone())
    }
}

pub(crate) struct Target {
    gpu: Gpu,
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    needs_reconfigure: bool,
    renderer: egui_wgpu::Renderer,
    _user: Arc<()>,
}

impl Target {
    fn new(gpu: Gpu, window: Arc<Window>, surface: wgpu::Surface<'static>, user: Arc<()>) -> Result<Self> {
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
        Ok(Self { gpu, window, surface, config, needs_reconfigure: false, renderer, _user: user })
    }

    fn reconfigure(&mut self) {
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.gpu.device, &self.config);
        self.needs_reconfigure = false;
    }

    fn render(&mut self, ctx: &egui::Context, out: &mut FullOutput) -> Paint {
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Paint::Skipped;
        }
        if self.needs_reconfigure || size.width != self.config.width || size.height != self.config.height {
            self.reconfigure();
        }

        let gpu = self.gpu.clone();
        let primitives = ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
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
                self.reconfigure();
                return Paint::Retry;
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                match gpu.instance.create_surface(self.window.clone()) {
                    Ok(surface) => self.surface = surface,
                    Err(e) => tracing::error!("recreating lost surface failed: {e}"),
                }
                self.reconfigure();
                return Paint::Retry;
            }
            wgpu::CurrentSurfaceTexture::Occluded | wgpu::CurrentSurfaceTexture::Timeout => return Paint::Skipped,
            wgpu::CurrentSurfaceTexture::Validation => {
                tracing::warn!("surface texture acquisition failed validation");
                return Paint::Skipped;
            }
        };

        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let clear = ctx.global_style().visuals.panel_fill.to_normalized_gamma_f32();
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
        Paint::Presented
    }
}

impl Surface for Target {
    fn max_texture_side(&self) -> usize {
        self.gpu.device.limits().max_texture_dimension_2d as usize
    }

    fn paint(&mut self, ctx: &egui::Context, mut out: FullOutput) -> Paint {
        for (id, deltas) in out.textures_delta.set.drain() {
            for delta in deltas {
                self.renderer.update_texture(&self.gpu.device, &self.gpu.queue, id, &delta);
            }
        }
        let result = self.render(ctx, &mut out);
        for id in out.textures_delta.free.drain() {
            self.renderer.free_texture(&id);
        }
        result
    }
}
