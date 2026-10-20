<p align="center">
  <img src="assets/brand/snapoleon-128.png" width="96" alt="Snapoleon logo">
</p>

# Snapoleon

Screenshots, screen recording, OCR and sharing in one small app, written entirely in Rust.

Snapoleon aims for the simplicity of the Windows Snipping Tool on the default path and the depth of ShareX
behind it. Press one hotkey, drag a region, and an instant toolbar appears. Pipelines, upload destinations,
a command line interface and an MCP server are there when you want them and out of the way when you do not.

- 100% Rust. The interface is egui on our own winit shell (OpenGL, wgpu on macOS). No webview, no Electron, no JavaScript runtime.
- Small and quiet. No async runtime in the client, blocking I/O on worker threads, and heavy features are
  optional or downloaded on demand.
- Private by default. Nothing leaves your machine unless you configure a destination. AI features default to
  local endpoints (Ollama, LM Studio) and can be switched off entirely by policy.
- Self-hostable links. Run `snapoleon-server` (a single binary, or build the included Dockerfile) for short links with
  expiry, passwords, view counts and link previews that look right in chat apps.

Status: version 0.1, pre-release. Nothing has been released yet: there are no published binaries and no
package-manager listings. Build from source for now. See the platform table below for what has actually been tested.

## Features

**Capture**
- Region, window, full screen, multi-monitor, and UI element capture with snapping.
- Frozen-screen overlay with crosshair, pixel loupe and HEX/RGB readout, aspect lock, arrow-key nudging.
- Scrolling capture, step-by-step capture sessions, delayed capture, color picker and pixel measure.
- HDR tone mapping and readable downscaling for high-DPI screens.

**Annotate**
- Non-destructive editor: arrow, line, shapes, pen, highlighter, text, callouts, step badges, spotlight,
  magnifier, stamps, measure, image paste, crop, undo and redo.
- Redaction: pixelate, blur, solid, plus automatic detection of sensitive data.
- Beautify presets (backgrounds, padding, shadows), export to a target size, `.snap` project files you can re-edit.
- Combine, diff and pinned always-on-top images.

**Recording**
- MP4, GIF and WebP. Optional microphone. Cursor highlight, click ripples and keystroke overlay.
- Video trimming and export to a size target (for example a chat app upload limit).

**Text and codes**
- OCR using the OS engine where available (Windows.Media.Ocr, Apple Vision), tesseract, or a built-in fallback.
- Copy as text or table, translate, search.
- QR and barcode decoding and QR generation.

**Privacy**
- Detection of emails, card numbers (with checksums), keys, tokens and local ID formats, mapped to word boxes.
- Metadata stripping and optional forced redaction before upload.
- Secrets live in the OS keyring, never in settings files.

**Library**
- Searchable history (SQLite with full-text search over OCR text, titles, app and URL), filters, favorites,
  retention rules, re-edit from the library.

**Share**
- S3-compatible stores (AWS, Cloudflare R2, MinIO, Backblaze B2, Yandex Cloud and others), FTP and FTPS
  servers, WebDAV, local or synced folders, Imgur, Telegram, Yandex Disk, Dropbox, Google Drive, OneDrive, Box.
- ShareX `.sxcu` custom uploader import, URL shorteners, link formatting (Markdown, HTML, BBCode).

**Automation**
- Pipelines: ordered steps run after a capture (annotate, redact, OCR, upload, copy, and so on), bound to hotkeys.
- Command line: `snapoleon capture`, `record`, `ocr`, `qr`, `upload`, `pin`, `edit`, `history`, `pipeline`.
  See [docs/CLI.md](docs/CLI.md).
- MCP server over stdio (`snapoleon mcp`) so AI assistants can list monitors and windows, capture, run OCR,
  decode codes and search history.
- `snapoleon://` URL scheme.

**Team server**
- `snapoleon-server`: uploads, short links with expiry and passwords, view counts, link previews, workspaces and
  tokens, OIDC single sign-on, retention, webhooks. Dockerfile, systemd unit and docker-compose included
  under `server/snapoleon-server/deploy/`.

**Enterprise**
- Managed policy: disable uploads, recording, AI or history; force a destination list; force redaction before
  upload; set retention and language. Windows Group Policy (ADMX/ADML), macOS configuration profiles,
  `policy.toml` on every platform. See [docs/POLICY.md](docs/POLICY.md).
- Per-machine MSI, per-user no-admin installer, and a portable zip.

## Platform support

| Platform | Status |
|---|---|
| Windows 10 1809+ and Windows 11 (x64, arm64) | Primary target. Developed and tested on Windows 11 x64 (single monitor). The arm64 build compiles and packages in CI but has not been run. |
| macOS 12.3+ (Apple Silicon and Intel, universal build) | Implemented. Builds and packages in CI; not yet run on a real Mac. Expect rough edges. |
| Linux (X11 and Wayland; x86_64, aarch64) | Implemented. Run under Xvfb and WSL only; not yet tested on a real desktop session. Expect rough edges. |

Linux notes: Wayland capture and global shortcuts depend on the XDG desktop portals and on your compositor.
Recording needs `ffmpeg` on Linux. OCR uses `tesseract` when it is installed and otherwise the built-in engine,
whose model is downloaded on first use. Details are in [docs/INSTALL.md](docs/INSTALL.md).

## Install

No release has been published yet. When one is, its files will be on the
[GitHub releases page](https://github.com/EmpyEmpt/snapoleon/releases) with a `SHA256SUMS` file, and the
release workflow (`.github/workflows/release.yml`) builds these packages:

| Platform | Package |
|---|---|
| Windows | NSIS installer (per-user, no admin), MSI (per-machine, x64 only), portable zip |
| macOS | `.dmg` with a universal app |
| Linux | AppImage, `.deb`, tarball |

Snapoleon is not listed in any package manager yet: not winget, Scoop, Chocolatey, Homebrew, the AUR or Flathub.
Manifest templates for those channels are in `packaging/` and are rendered for each release, but they have not
been submitted. Builds are not code-signed yet, so Windows SmartScreen and macOS Gatekeeper will warn.
Full instructions are in [docs/INSTALL.md](docs/INSTALL.md).

## Build from source

Requirements: Rust 1.90 or newer (edition 2024) and a C toolchain.

```sh
git clone https://github.com/EmpyEmpt/snapoleon
cd snapoleon
cargo run --release -p snapoleon
```

Linux build dependencies (Ubuntu 24.04 package names; PipeWire 1.0 or newer is required for screen capture):

```sh
sudo apt install libxcb1-dev libxcb-randr0-dev libxcb-shm0-dev libxkbcommon-dev \
  libpipewire-0.3-dev libspa-0.2-dev libdbus-1-dev libegl1-mesa-dev libwayland-dev \
  libasound2-dev libx11-dev libxi-dev libxtst-dev libgbm-dev clang pkg-config nasm
```

Windows needs the MSVC build tools and `nasm` on PATH. macOS needs the Xcode command line tools and `nasm`
(`brew install nasm`).

The team server builds with `cargo build --release -p snapoleon-server`, or run the container:
`docker build -f server/snapoleon-server/deploy/Dockerfile -t snapoleon-server .`

To build installers, install `cargo-packager` and run it with the config in `packaging/`:

```sh
cargo install cargo-packager --locked
cargo build --release -p snapoleon
cargo packager --config packaging/Packager.toml --release --formats nsis   # or wix, app, dmg, deb, appimage
```

## Project layout

| Path | Role |
|---|---|
| `crates/snap-core` | Shared types: geometry, settings, pipelines, hotkeys, i18n, IPC messages, naming, policy |
| `crates/snap-capture` | Monitors, windows, elements, screen/window/region capture |
| `crates/snap-image` | Pure image operations: downscale, blur, beautify, stitch, diff, encode, size targeting |
| `crates/snap-annotate` | Non-destructive annotation document, rasterizer, undo, `.snap` files |
| `crates/snap-ocr` | OCR backends, tables, QR and barcode decode/encode |
| `crates/snap-redact` | Sensitive-data detection mapped to word boxes |
| `crates/snap-upload` | Destinations, OAuth, `.sxcu` import, shorteners, team-server client |
| `crates/snap-history` | SQLite + FTS5 library, thumbnails, retention |
| `crates/snap-platform` | Hotkeys, tray, clipboard, notifications, single-instance IPC, autostart, keyring, URL scheme, policy |
| `crates/snap-record` | Screen recording, audio, overlays, trim, export-to-size |
| `crates/snap-ai` | OpenAI-compatible client (local by default): alt text, titles, translation, smart redaction |
| `crates/snap-ui` | egui design system: tokens, fonts, icons, widgets |
| `crates/snap-shell` | Multi-window winit + egui runtime (OpenGL via glutin, or wgpu) that keeps running with no windows open |
| `crates/snap-engine` | Headless orchestration shared by the GUI, CLI and MCP server |
| `apps/snapoleon` | The binary: GUI, CLI subcommands, MCP server |
| `server/snapoleon-server` | Self-hosted team server |
| `locales/` | Translations, one folder per language |
| `packaging/` | Installers, package-manager manifests, policy templates |
| `docs/` | Spec, design system, install, policy and CLI documentation |

See [docs/SPEC.md](docs/SPEC.md) for the architecture and [docs/DESIGN.md](docs/DESIGN.md) for the design system.

## Contributing

Bug reports, platform testing on macOS and Linux, and translations are especially welcome. Start with
[CONTRIBUTING.md](CONTRIBUTING.md).

## License

Dual-licensed under either of

- MIT license ([LICENSE-MIT](LICENSE-MIT))
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))

at your option. Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion
in Snapoleon by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.

## Credits

- [Inter](https://github.com/rsms/inter) by Rasmus Andersson and the Inter Project Authors, SIL Open Font License 1.1
  (`assets/fonts/Inter-LICENSE.txt`).
- [JetBrains Mono](https://github.com/JetBrains/JetBrainsMono) by the JetBrains Mono Project Authors, SIL Open
  Font License 1.1 (`assets/fonts/JetBrainsMono-OFL.txt`).
- [Phosphor Icons](https://phosphoricons.com), MIT License (`crates/snap-ui/assets/Phosphor-LICENSE.txt`).
- Built on [egui](https://github.com/emilk/egui), [winit](https://github.com/rust-windowing/winit),
  [glutin](https://github.com/rust-windowing/glutin), [wgpu](https://github.com/gfx-rs/wgpu) and many other open-source crates.
- Snapoleon is an independent, clean-room implementation. ShareX, Snipping Tool, CleanShot X, Snagit, Shottr,
  Flameshot and others are the work of their respective authors and are mentioned only as comparisons.
