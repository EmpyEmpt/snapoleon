# Renderer: OpenGL (glutin + egui_glow) versus wgpu

`snap-shell` draws with OpenGL on Windows and Linux and with wgpu on macOS. Windows carries both backends and falls back
to wgpu at run time when OpenGL is not usable. `crates/snap-shell/build.rs` decides which backends are compiled in;
`crates/snap-shell/src/render/` holds them behind the `Graphics` and `Surface` traits, with `Gfx` and `Target` enums
dispatching between them. How to pick one: `docs/APP.md`, "Renderer".

| build | backends |
|---|---|
| Windows, default | OpenGL first, wgpu as the fallback |
| Windows, `--no-default-features --features wgpu` (`snapoleon`) | wgpu only |
| Linux, default | OpenGL only (Mesa always has llvmpipe) |
| Linux with the `wgpu` feature | wgpu only |
| macOS | wgpu (Metal) only |

OpenGL: one WGL/EGL/GLX context shared by all windows and made current on each window's surface before it paints; every
window has its own `egui_glow::Painter`. Display and context come up with the first window (WGL needs a window to load its
extensions), so there is no GPU start-up thread. A context older than OpenGL 3.0 (ES 2.0) counts as unusable.

## Fallback

If the first window's OpenGL display, context or painter cannot be made, the window is dropped, the reason is logged and
the window is made again for wgpu (the OpenGL pixel format already set on the old window is not reused). Checked on
Windows with `SNAPOLEON_RENDERER=glow-fail` (context creation fails on purpose) and with `GLUTIN_WGL_OPENGL_DLL` pointing to a
missing DLL (a real failure from glutin): both log `falling back to wgpu`, the dev views render, and the tray with its
overlay pool works (overlay ready in 29-42 ms). `SNAPOLEON_RENDERER=glow` plus the missing DLL does not fall back and
fails to open the window, as intended. A real 1.1-only driver (Basic Display Adapter) was not available to test; that
case goes through the version check, which is not exercised.

## Measurements

Windows 11, AMD iGPU (wgpu picks Vulkan there), release profile (`opt-level = "z"`, fat LTO), 2560x1440 monitor, portable
data dir, `general.instant_overlay = true` (one pre-opened cover), PrintScreen then Escape driven with `keybd_event`.

| | OpenGL only | wgpu only | both (default on Windows) |
|---|---|---|---|
| `snapoleon.exe` | 24,356,352 B | 27,905,024 B | 28,005,376 B |
| `snapoleon.exe`, xz -9 | 7,252,124 B | 8,270,436 B | 8,305,748 B |
| private bytes, tray idle, cover pool open | 103 MB | 294 MB | (OpenGL) 103 MB |
| private bytes after 10 overlay sessions | 194 MB (before the thread fix) | 358 MB | |
| overlay ready, after the hotkey (10 sessions) | 27-38 ms (render 8 ms) | 30-44 ms (render 10 ms) | |
| covers opened hidden at start-up | 136 ms | 625 ms | |

Shipping both backends costs about 3.65 MB raw and 1.05 MB xz over OpenGL alone, and is within 0.1 MB of wgpu alone: on
Windows the size gain of OpenGL disappears and the memory and start-up gain stays. The fallback itself (forced with
`glow-fail`): 319 MB idle with the pool on wgpu.

Pool cost, OpenGL: tray idle 103 MB with the pool, 4 MB without it (`instant_overlay = false`), so the setting's
description says about 100 MB.

## Memory creep with OpenGL

Private bytes grew about 1.3 MB per overlay session (194 MB after 10, 218 MB after 30, 234 MB after 40) with OpenGL while wgpu
stayed flat. Not a leak in our heap (a counting allocator showed the Rust heap flat at 17.7 MB), not textures (egui_glow
deletes a freed texture at once; no GDI, USER or handle growth either). Cause: an OpenGL driver keeps about 90 KB for every
thread created while it is loaded. Spawning 64 empty threads per session added 5.8 MB per session. The overlay path created
about 14 threads per session: the snapshot upload (up to 8), the BGRA conversion (up to 8, 4K only), the window
enumeration next to the capture, and the element query worker.

Fix: the pixel loops (`overlay::upload`, `gdi::bgra_to_rgba_opaque`, `hdr::tone_map`, `snap_image::util::par_rows`) and the
capture joins (`compose::freeze`, `capture_window`, `par_map`) run on the shared `rayon` pool (already linked through
ocrs), and the overlay's element query worker is one thread for the process. After the change 40 sessions stay between
170 and 191 MB, oscillating, not climbing; the thread count no longer grows.

## Visuals

`SNAPOLEON_DEV=library|editor|settings|overlay|pin|recorder` with `SNAPOLEON_DEV_SHOT`, compared by eye and by pixel
difference. The settings window is identical apart from a hover state; text, gamma, rounded corners and the pill-shaped
recorder match. The remaining differences come from the demo screenshot taken at start-up (different desktop content).
Both use a non-sRGB framebuffer and egui's shader-side gamma, so blending is the same.

## Caveats

- Only the Windows build was run. The Linux target type-checks; the macOS target uses wgpu and is unchanged.
- Without the `glow` feature or on macOS the OpenGL code is not built at all.
