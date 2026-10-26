# Notes for coding agents

Snapoleon is a screenshot, recording, OCR and sharing app in Rust: egui on its own winit shell, no async runtime.
Read [CONTRIBUTING.md](CONTRIBUTING.md) for the rules and [docs/APP.md](docs/APP.md) for the app's architecture.

## Map

| Path | What |
|---|---|
| `apps/snapoleon/src/` | the binary: `main.rs` startup and CLI dispatch, `app.rs` window registry, `routing.rs` commands, `views/<view>/` one directory per window |
| `crates/snap-engine/` | headless orchestration shared by the GUI, the CLI (`src/cli*`) and the MCP server (`src/mcp*`) |
| `crates/snap-capture`, `snap-record`, `snap-ocr`, `snap-image`, `snap-annotate` | capture, recording, OCR and codes, pixel operations, the annotation document |
| `crates/snap-platform/` | hotkeys, tray, clipboard, notifications, IPC, autostart, permissions; `src/<area>` with `cfg` per OS |
| `crates/snap-shell/`, `crates/snap-ui/` | window runtime (winit + egui, OpenGL or wgpu) and the design system |
| `locales/{en,ru}/<area>.toml` | every user-visible string |
| `packaging/`, `.github/workflows/` | installers, the macOS bundle script, CI and release |

## Build and check

```sh
cargo build -p snapoleon                       # debug; dependencies are optimized, so it runs at a usable speed
cargo clippy -p <crate> --no-deps              # warnings in code you touched must not get worse
cargo fmt --all
```

Rust 1.95 or newer. Never run `cargo clean`: the warm `target/` is what keeps builds fast. There are no unit tests by
design; check a change by building, linting and looking at the result.

## Run one view

`SNAPOLEON_DEV=<view> target/debug/snapoleon` opens one window with demo data and private settings in
`target/debug/data` (log: `target/debug/data/logs/snapoleon.log`). No tray, hotkeys or IPC; it ends with its last
window. Views: `overlay editor pin recorder video library text settings onboarding about steps combine diff scroll
colors`. More variables (`SNAPOLEON_DEV_PAGE`, `_THEME`, `_LANG`, `_MODE`, `_DEMO`, `_EDITOR_*`) are in
[docs/APP.md](docs/APP.md#dev-harness).

## Look at the UI (macOS)

```sh
SNAPOLEON_DEV=settings SNAPOLEON_DEV_PAGE=hotkeys target/debug/snapoleon &
peekaboo window list --app snapoleon --json              # find the window id
peekaboo see --window-id <id> --no-elements --path /tmp/view.png
```

Then read the PNG. This works while the window is covered and needs no focus. `peekaboo see --app snapoleon --json`
lists the accessibility tree and `peekaboo click --on <id>` presses a control by name. `SNAPOLEON_DEV_SHOT` depends on
the terminal's Screen Recording permission on macOS; prefer peekaboo. An untitled 500x500 window owned by Snapoleon
in window lists is macOS's text-input helper (`TUINSWindow`), not a bug.

## The real app (macOS)

Capture, permissions, notifications, hotkeys and `snapoleon://` links only behave like the shipped app in the signed
bundle: `packaging/macos/bundle.sh --debug --install` (details in [docs/MACOS.md](docs/MACOS.md)). The app is a single
instance that owns the global hotkeys: only one agent at a time may install, launch or drive it, and do not press its
hotkeys while someone else is using the machine. `bundle.sh --reset-permissions` or `tccutil reset ScreenCapture
app.snapoleon` starts the permission flow over. App log:
`~/Library/Application Support/app.Snapoleon.Snapoleon/logs/snapoleon.log`.

## Headless checks

None of these needs a privacy permission:

```sh
B=target/debug/snapoleon
$B --help && $B doctor
$B qr encode hi -o /tmp/q.png && $B qr decode /tmp/q.png --json
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"agent","version":"0"}}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' | $B mcp
```

`$B capture --monitor 0 -o /tmp/s.png` needs Screen Recording for your terminal, or a running Snapoleon app that has
it (the capture then goes through the app; see [docs/CLI.md](docs/CLI.md#macos)).

## Rules

- Every user-visible string goes through `snap_core::t` / `tf`, with the key in `locales/en` and `locales/ru`
  (natural Russian) in the same change.
- Platform code sits behind `#[cfg(target_os = ...)]`; every public function compiles on Windows, macOS and Linux.
- No tokio in the client; blocking work runs on threads. Never block the UI thread.
- Comments explain why. No emojis anywhere.
