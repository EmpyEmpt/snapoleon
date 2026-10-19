//! Picks the renderer backend once, so the sources only test `renderer_wgpu` / `renderer_glow`.
//!
//! wgpu is used when the `wgpu` feature is on, on macOS (OpenGL is deprecated there) and when the `glow` feature is
//! off; otherwise OpenGL through glutin and egui_glow.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(renderer_wgpu)");
    println!("cargo::rustc-check-cfg=cfg(renderer_glow)");
    println!("cargo::rerun-if-changed=build.rs");
    let wgpu = std::env::var_os("CARGO_FEATURE_WGPU").is_some()
        || std::env::var_os("CARGO_FEATURE_GLOW").is_none()
        || std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "macos");
    println!("cargo::rustc-cfg={}", if wgpu { "renderer_wgpu" } else { "renderer_glow" });
}
