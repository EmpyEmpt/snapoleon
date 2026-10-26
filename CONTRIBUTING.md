# Contributing to Snapoleon

Thanks for helping. This document covers the development setup, the rules the codebase relies on, and how to add
a language.

## Setup

1. Install Rust 1.90 or newer (`rustup update stable`). The workspace uses edition 2024.
2. Linux only: install the build dependencies listed in the [README](README.md#build-from-source).
   PipeWire 1.0 or newer is required.
3. Build and run:

   ```sh
   cargo run -p snapoleon            # debug build of the app (dependencies are optimized, see Cargo.toml)
   cargo check --workspace
   cargo clippy --workspace --all-targets
   cargo fmt --all
   ```

`apps/snapoleon` is the default workspace member, so plain `cargo run` starts the app.

For the in-app development harness (how to open individual windows and views without going through the whole
capture flow), see [docs/APP.md](docs/APP.md).

## Architecture in one minute

The workspace is layered. Library crates (`snap-image`, `snap-annotate`, `snap-ocr`, ...) are plain Rust with no
UI. `snap-engine` orchestrates them headlessly and is shared by the GUI, the CLI and the MCP server.
`snap-shell` is the window runtime, `snap-ui` the egui design system, and `apps/snapoleon` ties it together.
Read [docs/SPEC.md](docs/SPEC.md) before changing a crate's public API and [docs/DESIGN.md](docs/DESIGN.md)
before touching UI.

Hard rules from the spec:

- 100% Rust in the client. No webview, no Electron, no JS runtime. The server renders plain HTML.
- No tokio in the client. Use blocking I/O on worker threads (`std::thread`, `crossbeam-channel`). HTTP is `ureq`
  with rustls.
- Heavy or rare features go behind cargo features or are downloaded on demand.
- Platform code lives in `src/platform/{windows,macos,linux}.rs` (or similar) behind `#[cfg(target_os = ...)]`.
  Every public function must compile on all three operating systems and return `Error::Unsupported` where a
  backend is missing.
- Clean-room: do not copy code or look at the source of closed or incompatibly licensed competitors. Features
  come from public descriptions.

## Internationalization

Every user-visible string goes through `snap_core::t("area.key")`. Do not hard-code text in the UI, in
notifications, in errors shown to the user, or in tray menus.

- Strings live in `locales/<lang>/<area>.toml`, for example `locales/en/capture.toml`.
- Each crate or area owns its own files. Do not edit another area's locale file; add keys in yours.
- English (`en`) is the source language. Russian (`ru`) is maintained alongside it. A new key must be added to
  `en` and `ru` in the same change.
- Each file's tables flatten into dotted keys (`[editor.tool]` and `arrow = "Arrow"` give `editor.tool.arrow`).
  Users can also drop extra `locales/<code>/*.toml` files into the `locales` folder of their data directory to
  test a translation without rebuilding.
- Keys are stable identifiers (`capture.toolbar.copy`), not sentences. Use placeholders for dynamic values
  instead of concatenating translated fragments.
- Where macOS has its own words for a thing ("menu bar" for the tray, "Finder", "Keychain"), add the variant under
  a `macos.` prefix, for example `[macos.settings.general.tray_click]`, in `en` and `ru`. On macOS it replaces the
  plain key; the code keeps asking for the plain key.

### Adding a language

Community translations are a goal of the project.

1. Copy the folder `locales/en` to `locales/<code>`, where `<code>` is a lowercase BCP 47 code (`de`, `pt-br`).
2. Translate the values in every `.toml` file. Do not change the keys or the placeholders (`{name}`).
3. Keep strings short; toolbar buttons and tabs have little room. Check them in the running app with the
   language set in Settings, General.
4. Set the native name of the language in `locales/<code>/common.toml` (`[meta]` table, `language_name`). Languages are
   embedded from the `locales/` folder at build time, so no code change is needed; the language then appears in
   Settings, General.
5. Untranslated keys fall back to English, so a partial translation is useful and welcome.

## Coding conventions

- `rustfmt.toml` sets `max_width = 140` and `use_small_heuristics = "Max"`. Run `cargo fmt --all` before pushing.
- `cargo clippy --workspace --all-targets` should not get worse. CI reports clippy warnings without failing,
  but new warnings will be questioned in review.
- Prefer plain functions and data over trait hierarchies. Avoid new dependencies unless they pay for themselves;
  look at what is already in `[workspace.dependencies]` first.
- Errors: library crates return `snap_core::Result`; use `anyhow` only in the binary. Never panic on user input.
  The release profile uses `panic = "abort"`.
- Do not store secrets in settings files. Use the `SecretStore` (OS keyring) and keep a `secret_ref` instead.
- Comments explain why, not what. Keep public items documented.
- Do not use emojis in code, comments, commit messages or docs.

## Tests

There are no unit tests in this project, by design. Verify your change with:

- `cargo check` and `cargo clippy` for the affected crates, and for other platforms where you touched `cfg` code
  (`cargo check --target x86_64-unknown-linux-gnu` if you have the target installed).
- Small runnable examples in `examples/*.rs` inside the crate you changed, for example
  `cargo run -p snap-ocr --example ocr_file -- path/to/image.png`. Add one when a change is easiest to judge by
  looking at its output. Examples are small, self-contained and documented at the top.
- Running the app and using the feature, via the dev harness in [docs/APP.md](docs/APP.md).

Describe in the pull request what you ran and what you saw.

## Pull requests

- Keep a change focused; separate refactors from behavior changes.
- Mention the platforms you tested on. macOS and Linux testing from contributors is especially valuable.
- Packaging changes under `packaging/` and `.github/` are checked by the release workflow only on tags; say
  how you validated them.
- By submitting a contribution you agree to license it under the project's dual MIT / Apache-2.0 license.
