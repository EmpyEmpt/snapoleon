# Installing Snapoleon

Download the file for your system from the [latest release](https://github.com/EmpyEmpt/snapoleon/releases/latest),
or build from source (see the end of this page). Each release includes
`SHA256SUMS` (and `SHA256SUMS.minisig` when the release was signed with minisign). Verify a download with:

```sh
sha256sum -c SHA256SUMS --ignore-missing          # Linux
shasum -a 256 -c SHA256SUMS --ignore-missing      # macOS
Get-FileHash .\Snapoleon-*-setup.exe              # Windows PowerShell, compare by eye
```

## Windows

Requires Windows 10 1809 or newer. x64 and arm64 builds are provided.

| Package | Scope | Notes |
|---|---|---|
| `Snapoleon-<ver>-windows-<arch>-setup.exe` | per-user | NSIS installer. No administrator rights, no UAC prompt. Installs to `%LOCALAPPDATA%\Programs\Snapoleon` and registers the `snapoleon://` scheme, the `.snap` association and the Start menu shortcut under your user account (HKCU). The uninstaller removes all of that, including the autostart entry, and leaves your settings and captures alone. |
| `Snapoleon-<ver>-windows-x64.msi` | per-machine | WiX installer for IT deployment (x64 only: WiX 3 cannot build arm64) (`ALLUSERS=1`). Installs to Program Files and registers the URL scheme and file association machine-wide, so it needs an elevated session; `msiexec /qn` from a non-elevated shell fails with error 1603. There is no per-user MSI: cargo-packager's WiX template is per-machine only. Use the setup.exe when you do not have administrator rights. |
| `Snapoleon-<ver>-windows-<arch>-portable.zip` | none | Unzip anywhere. Contains a `portable` marker file. |

Package managers: not listed yet. Manifests for winget, Scoop and Chocolatey are in `packaging/` and are rendered
for each release by `packaging/render-manifests.py`, but none have been submitted.

Silent install:

```powershell
.\Snapoleon-0.2.1-windows-x64-setup.exe /S
msiexec /i Snapoleon-0.2.1-windows-x64.msi /qn
```

Unsigned builds show a SmartScreen warning ("More info", then "Run anyway"). Signed releases use Azure Trusted
Signing and do not.

### Portable mode

If a file named `portable` (or a folder named `data`) sits next to `snapoleon.exe`, Snapoleon keeps everything
beside the executable: settings, history, cache and captures all live in `data\`. Move the folder to a USB stick
and your configuration moves with it. Delete the `portable` file and the `data` folder to switch back to a normal
profile.  Options that integrate with the OS, such as autostart and the `snapoleon://` URL scheme, still write to your
user profile when you turn them on, and secrets still go to the OS keyring.

### Where data lives (non-portable)

| What | Location |
|---|---|
| Settings and data | `%APPDATA%\Snapoleon\Snapoleon\` (config) and `%LOCALAPPDATA%\Snapoleon\Snapoleon\` (data, cache) |
| Captures | `%USERPROFILE%\Pictures\Snapoleon` and `Videos\Snapoleon` |
| Machine policy | `HKLM\SOFTWARE\Policies\Snapoleon`, `%ProgramData%\Snapoleon\policy.toml` |

Exact paths come from the `directories` crate and may differ slightly; Settings, Advanced shows the real ones.

## macOS

Requires macOS 14 (Sonoma) or newer: screenshots use ScreenCaptureKit's screenshot API, which arrived in macOS 14.
The `.dmg` contains a universal binary that runs on Apple Silicon and Intel Macs.

Open the `.dmg` and drag Snapoleon to Applications. (A Homebrew cask template is in `packaging/homebrew/`; it is
not in a tap yet.) Snapoleon is a menu bar app: it has no Dock icon
(`LSUIElement`), and its controls are in the menu bar icon.

To use the command line, run `/Applications/Snapoleon.app/Contents/MacOS/snapoleon install-cli` once, or use
Settings > Advanced > Command line tool. It puts a small `snapoleon` script in `/usr/local/bin` that runs the app's
own binary. A symlink to the binary would not work properly: macOS would not see it as part of Snapoleon.app. See
[CLI.md](CLI.md#macos) for how terminal commands and the MCP server get their permissions.

### Permissions

macOS asks for permissions the first time they are needed. Grant them in System Settings, Privacy & Security:

| Permission | Needed for |
|---|---|
| Screen Recording | Any screenshot or recording. Snapoleon cannot capture other apps' windows without it. After granting, macOS asks you to quit and reopen the app. |
| Microphone | Voice narration while recording (only if you turn it on). |
| Accessibility | Snapping to buttons, fields and other UI elements. Optional. (Window titles come with Screen Recording.) |
| Input Monitoring | Keystroke overlay in recordings. Optional. |

If you rebuild or replace an unsigned copy of the app, macOS may treat it as a new app and ask again; remove the
old Snapoleon entry from the list with the minus button first.

Builds that are not notarized are blocked by Gatekeeper the first time. Open the app once, then go to System
Settings > Privacy & Security, scroll down to the message about Snapoleon and click Open Anyway (macOS 15 and newer
no longer offer Control-click > Open for this). Or remove the quarantine flag:
`xattr -dr com.apple.quarantine /Applications/Snapoleon.app`.

Data lives in `~/Library/Application Support/app.Snapoleon.Snapoleon/`. Policy: configuration profile
(`/Library/Managed Preferences/app.snapoleon.plist`) or `/Library/Application Support/Snapoleon/policy.toml`.

## Linux

x86_64 and aarch64. The prebuilt packages need a recent system: Ubuntu 24.04, Debian 13, Fedora 40 or newer, or
any distribution with PipeWire 1.0 or newer.

| Format | How |
|---|---|
| AppImage | `chmod +x Snapoleon-*.AppImage && ./Snapoleon-*.AppImage`. Needs FUSE 2 (`libfuse2t64` on Ubuntu 24.04) or run with `--appimage-extract-and-run`. |
| .deb | `sudo apt install ./Snapoleon-*-linux-x86_64.deb`. Depends on libxcb, libx11, libxkbcommon (and its X11 part), libegl1, libgbm, ALSA, PipeWire (>= 1.0), D-Bus. Recommends `tesseract-ocr` and `ffmpeg`. |
| Arch (AUR) | Not on the AUR yet. `packaging/linux/aur/PKGBUILD` builds `snapoleon-bin` from the release tarball with `makepkg -si`. |
| Flatpak | Build from `packaging/linux/flatpak/app.snapoleon.Snapoleon.yml` with `flatpak-builder`. Not yet on Flathub. |
| Tarball | `Snapoleon-<ver>-linux-<arch>.tar.gz` holds the binary, desktop file, icons and licenses. |

### Optional programs

- `ffmpeg` is required for screen recording on Linux (MP4, GIF, WebP encoding).
- `tesseract` (and language data such as `tesseract-ocr-eng`, `tesseract-ocr-rus`) is used for OCR on Linux when installed;
  without it, the built-in engine is used and downloads its model on first use.
  Without it Snapoleon falls back to a slower built-in engine whose models are downloaded on demand.

```sh
sudo apt install tesseract-ocr tesseract-ocr-eng ffmpeg      # Debian / Ubuntu
sudo pacman -S tesseract tesseract-data-eng ffmpeg           # Arch
```

### What works differently on Linux

- **Element capture and the browser address** use AT-SPI over D-Bus. Applications only publish their widgets when
  desktop accessibility is on (GNOME: Settings, Accessibility, "Always show accessibility menu" or
  `gsettings set org.gnome.desktop.interface toolkit-accessibility true`; KDE: System Settings, Accessibility). Qt and
  GTK apps then work after they are restarted. Chromium-based browsers show their page only when started with
  `--force-renderer-accessibility` (or after opening `chrome://accessibility` once), so without that no page address is
  recorded for them; Firefox needs nothing extra. Without the AT-SPI service, `capture --element` and the overlay's
  element mode report that elements are unsupported.
- **Hide desktop icons** does nothing: desktops here have no common icon layer to hide.
- **Dragging a capture out of the Library** into another program is not available; use Reveal, or Copy file and paste.
- **Play sounds** uses `pw-play`, `paplay` or `aplay`, whichever is installed.

### Building on Linux

Tested on Ubuntu 24.04 (x86_64). Ubuntu 22.04 is too old: its PipeWire is 0.3.48 and the capture backend (`xcap`)
needs 1.0 or newer. Install Rust 1.95 or newer (`rustup update stable`) and these packages:

```sh
sudo apt install build-essential pkg-config clang nasm \
  libxcb1-dev libxcb-randr0-dev libxcb-shm0-dev libxkbcommon-dev \
  libpipewire-0.3-dev libspa-0.2-dev libdbus-1-dev libegl1-mesa-dev libwayland-dev \
  libasound2-dev libx11-dev libxi-dev libxtst-dev libgbm-dev
cargo build --release -p snapoleon
```

What each group is for:

| Packages | Used by |
|---|---|
| `clang`, `libpipewire-0.3-dev`, `libspa-0.2-dev` | PipeWire bindings (bindgen needs libclang) |
| `libxcb*-dev`, `libx11-dev`, `libxi-dev`, `libxtst-dev` | screen and window capture, global hotkeys, input hooks |
| `libxkbcommon-dev`, `libwayland-dev`, `libegl1-mesa-dev`, `libgbm-dev` | window system and GPU rendering |
| `libasound2-dev` | microphone and system-audio capture |
| `libdbus-1-dev` | notifications, tray, secrets, desktop portals |
| `nasm` | assembly in the crypto and codec dependencies |

At run time the app also loads `libxkbcommon-x11-0`, `libxcursor1`, `libxi6`, `libxtst6` and `libegl1` (all pulled in
by the `.deb`). Without `libxkbcommon-x11-0` it aborts on start-up under X11.

**WSL 2 with WSLg.** The GUI runs, and `SNAPOLEON_DEV=<view>` renders every window, but WSLg cannot be captured:
its XWayland server refuses to read the root window and its compositor has no screenshot protocol or portal. For a
real capture test use a virtual X server: `Xvfb :99 -screen 0 1600x900x24 & openbox & DISPLAY=:99 snapoleon capture --monitor 0`
(with `unset WAYLAND_DISPLAY`). The tray needs a StatusNotifierItem host, which WSLg lacks.

### Wayland

Wayland does not let applications capture the screen or register global shortcuts on their own. Snapoleon uses
the XDG desktop portals:

- Screen capture asks the GNOME Shell screenshot service, then the Screenshot portal, then the wlroots
  screencopy protocol (Sway, Hyprland). The compositor may show its own picker or a permission dialog the first time.
  A compositor that offers none of them (such as WSLg's Weston) gets a clear error instead of a capture.
  Install the portal backend for your desktop: `xdg-desktop-portal-gnome`, `xdg-desktop-portal-kde`, `xdg-desktop-portal-wlr` (Sway, Hyprland uses `xdg-desktop-portal-hyprland`).
- Global hotkeys use the GlobalShortcuts portal. GNOME 48 and KDE Plasma 6 support it; older compositors and some
  wlroots-based ones do not. Where it is missing, bind a key in your compositor to `snapoleon capture`
  (the command forwards to the running instance).
- Window and element snapping and the frozen-screen overlay are limited on Wayland compared with X11 or
  Windows, because applications cannot see other windows' geometry.
- The tray icon needs a StatusNotifierItem host. KDE, XFCE, Cinnamon and others have one. GNOME needs the
  "AppIndicator and KStatusNotifierItem Support" extension. Without a host, run `snapoleon settings` or use your
  compositor's key bindings.
- Clipboard on Wayland works while Snapoleon is running; some compositors drop clipboard contents when the
  source app exits, so keep it in the tray.

On X11 everything works without portals.

### Flathub note

The manifest in this repository repackages the CI tarball so that it can be built quickly. A Flathub submission
must build from source with vendored crates (`flatpak-cargo-generator.py` against `Cargo.lock`) and no network
access during the build.

## Building installers yourself

```sh
cargo install cargo-packager --locked
cargo build --release -p snapoleon
cargo packager --config packaging/Packager.toml --release --formats nsis,wix   # Windows
cargo packager --config packaging/Packager.toml --release --formats app,dmg    # macOS
cargo packager --config packaging/Packager.toml --release --formats deb,appimage  # Linux
pwsh packaging/windows/make-portable.ps1 -Exe target/release/snapoleon.exe -Version 0.2.1   # portable zip
```

On a Mac, `packaging/macos/bundle.sh` builds a signed `Snapoleon.app` for your own use without cargo-packager; see
[MACOS.md](MACOS.md).

## Uninstalling

Uninstall with the system tool (Settings, Apps; drag the app to the trash;
`apt remove snapoleon`). User data is left in place; delete the data folders listed above to remove history and
settings. On macOS also turn off Launch at login first (or remove Snapoleon under System Settings > General > Login
Items afterwards), and remove `/usr/local/bin/snapoleon` if you installed the command line tool. For the Homebrew cask,
`brew uninstall --zap snapoleon` removes the app, its command, the login item and the data folders. Secrets saved in the OS keyring (Credential Manager, Keychain, Secret Service) are not in those folders; remove them with your OS credential tool.
