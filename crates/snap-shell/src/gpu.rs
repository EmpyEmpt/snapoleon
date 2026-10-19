//! The single wgpu instance/adapter/device/queue shared by every window.

use std::sync::Arc;

use anyhow::anyhow;
use snap_core::{Error, Result};
use winit::event_loop::OwnedDisplayHandle;

const VENDOR_AMD: u32 = 0x1002;

#[derive(Clone)]
pub(crate) struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
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
    pub fn create(display: OwnedDisplayHandle) -> Result<Self> {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = backends();
        desc.flags = wgpu::InstanceFlags::from_build_config().with_env();
        desc.backend_options = wgpu::BackendOptions::from_env_or_default();
        desc.display = Some(Box::new(display));
        Self::with_instance(wgpu::Instance::new(desc), None)
    }

    /// Pick an adapter and open a device on it. `surface` restricts the choice
    /// to adapters that can present to it.
    pub fn with_instance(instance: wgpu::Instance, surface: Option<&wgpu::Surface<'_>>) -> Result<Self> {
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
