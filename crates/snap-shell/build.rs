//! Decides which renderer backends are compiled in, so the sources only test `renderer_gl` / `renderer_wgpu`.
//!
//! - OpenGL (glutin, egui_glow): the `glow` feature, except on macOS (OpenGL is deprecated there) and when the `wgpu`
//!   feature forces wgpu.
//! - wgpu: the `wgpu` feature, macOS, a build without `glow`, and Windows next to OpenGL: a PC with no usable OpenGL
//!   (remote desktop, a VM, the Basic Display Adapter) falls back to it at run time.
//!
//! Linux with the default features is OpenGL only: Mesa always has a software rasteriser.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(renderer_gl)");
    println!("cargo::rustc-check-cfg=cfg(renderer_wgpu)");
    println!("cargo::rerun-if-changed=build.rs");
    let has = |f: &str| std::env::var_os(format!("CARGO_FEATURE_{f}")).is_some();
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let (macos, windows) = (os == "macos", os == "windows");
    let gl = has("GLOW") && !macos && !has("WGPU");
    let wgpu = has("WGPU") || macos || !has("GLOW") || (windows && gl);
    if gl {
        println!("cargo::rustc-cfg=renderer_gl");
    }
    if wgpu {
        println!("cargo::rustc-cfg=renderer_wgpu");
    }
}
