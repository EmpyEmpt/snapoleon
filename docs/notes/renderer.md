# Renderer: OpenGL (glutin + egui_glow) versus wgpu

`snap-shell` draws with OpenGL on Windows and Linux and with wgpu on macOS. The `wgpu` cargo feature
(`--no-default-features --features wgpu` for `snapoleon`, or `--features wgpu` on top of the default) selects wgpu on every
OS. `crates/snap-shell/build.rs` makes the choice; `crates/snap-shell/src/render/` holds the two backends behind the
`Graphics` and `Surface` traits.

OpenGL: one WGL/EGL/GLX context shared by all windows and made current on each window's surface before it paints; every
window has its own `egui_glow::Painter`. Display and context come up with the first window (WGL needs a window to load its
extensions), so there is no GPU start-up thread.

## Measurements

Windows 11, AMD iGPU (wgpu picks Vulkan there), release profile (`opt-level = "z"`, fat LTO), 2560x1440 monitor, portable
data dir, `general.instant_overlay = true` (one pre-opened cover), PrintScreen then Escape driven with `keybd_event`.

| | OpenGL | wgpu |
|---|---|---|
| `snapoleon.exe` | 24,356,352 B (23.2 MiB) | 27,905,024 B (26.6 MiB) |
| `snapoleon.exe`, xz -9 | 7,252,124 B | 8,270,436 B |
| private bytes, tray idle, cover pool open | 103 MB | 294 MB |
| private bytes after 10 overlay sessions | 194 MB | 358 MB |
| overlay ready, after the hotkey (10 sessions) | 27-38 ms (render 8 ms) | 30-44 ms (render 10 ms) |
| covers opened hidden at start-up | 136 ms | 625 ms |

OpenGL memory keeps growing slowly with sessions (194 MB after 10, 218 MB after 30; working set 138 MB and 136 MB).

## Visuals

`SNAPOLEON_DEV=library|editor|settings|overlay|pin|recorder` with `SNAPOLEON_DEV_SHOT`, compared by eye and by pixel
difference. The settings window is identical apart from a hover state; text, gamma, rounded corners and the pill-shaped
recorder match. The remaining differences come from the demo screenshot taken at start-up (different desktop content).
Both use a non-sRGB framebuffer and egui's shader-side gamma, so blending is the same.

## Caveats

- No fallback to wgpu when OpenGL is unavailable (a bare "GDI Generic" driver, some remote sessions): the window fails to
  open and the error is logged. Build with the `wgpu` feature where that matters.
- Only the Windows build was run. The Linux target type-checks; the macOS target uses wgpu and is unchanged.
