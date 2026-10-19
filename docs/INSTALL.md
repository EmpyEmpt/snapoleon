# Installing Snapoleon

Release files are on the [releases page](https://github.com/snapoleon/snapoleon/releases). Each release includes
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
| `Snapoleon-<ver>-windows-<arch>-setup.exe` | per-user | NSIS installer. No administrator rights. Installs under `%LOCALAPPDATA%`. |
| `Snapoleon-<ver>-windows-<arch>.msi` | per-machine | WiX installer for IT deployment. Installs to Program Files. |
| `Snapoleon-<ver>-windows-<arch>-portable.zip` | none | Unzip anywhere. Contains a `portable` marker file. |

Package managers:

```powershell
winget install Snapoleon.Snapoleon
scoop install snapoleon          # portable zip; settings persist in the Scoop persist folder
choco install snapoleon          # per-user installer
```

Silent install:

```powershell
.\Snapoleon-0.1.0-windows-x64-setup.exe /S
msiexec /i Snapoleon-0.1.0-windows-x64.msi /qn
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

Requires macOS 12.3 (Monterey) or newer, for ScreenCaptureKit. The `.dmg` contains a universal binary that runs
on Apple Silicon and Intel Macs.

```sh
brew install --cask snapoleon
```

or open the `.dmg` and drag Snapoleon to Applications. Snapoleon is a menu bar app: it has no Dock icon
(`LSUIElement`), and its controls are in the menu bar icon.

### Permissions

macOS asks for permissions the first time they are needed. Grant them in System Settings, Privacy & Security:

| Permission | Needed for |
|---|---|
| Screen Recording | Any screenshot or recording. Snapoleon cannot capture other apps' windows without it. After granting, macOS asks you to quit and reopen the app. |
| Microphone | Voice narration while recording (only if you turn it on). |
| Accessibility | UI element snapping and window titles. Optional. |
| Input Monitoring | Keystroke overlay in recordings. Optional. |

If you rebuild or replace an unsigned copy of the app, macOS may treat it as a new app and ask again; remove the
old Snapoleon entry from the list with the minus button first.

Unsigned or un-notarized builds are blocked by Gatekeeper. Control-click the app, choose Open, and confirm. Or:
`xattr -dr com.apple.quarantine /Applications/Snapoleon.app`.

Data lives in `~/Library/Application Support/app.Snapoleon.Snapoleon/`. Policy: configuration profile
(`/Library/Managed Preferences/app.snapoleon.plist`) or `/Library/Application Support/Snapoleon/policy.toml`.

## Linux

x86_64 and aarch64. The prebuilt packages need a recent system: Ubuntu 24.04, Debian 13, Fedora 40 or newer, or
any distribution with PipeWire 1.0 or newer.

| Format | How |
|---|---|
| AppImage | `chmod +x Snapoleon-*.AppImage && ./Snapoleon-*.AppImage`. Needs FUSE 2 (`libfuse2t64` on Ubuntu 24.04) or run with `--appimage-extract-and-run`. |
| .deb | `sudo apt install ./Snapoleon-*-linux-x86_64.deb`. Depends on libxcb, PipeWire (>= 1.0), D-Bus. Recommends `tesseract-ocr` and `ffmpeg`. |
| Arch (AUR) | `snapoleon-bin` |
| Flatpak | Build from `packaging/linux/flatpak/app.snapoleon.Snapoleon.yml` with `flatpak-builder`. Not yet on Flathub. |
| Tarball | `Snapoleon-<ver>-linux-<arch>.tar.gz` holds the binary, desktop file, icons and licenses. |

### Optional programs

- `ffmpeg` is required for screen recording on Linux (MP4, GIF, WebP encoding).
- `tesseract` (and language data such as `tesseract-ocr-eng`, `tesseract-ocr-rus`) is used for OCR on Linux.
  Without it Snapoleon falls back to a slower built-in engine whose models are downloaded on demand.

```sh
sudo apt install tesseract-ocr tesseract-ocr-eng ffmpeg      # Debian / Ubuntu
sudo pacman -S tesseract tesseract-data-eng ffmpeg           # Arch
```

### Wayland

Wayland does not let applications capture the screen or register global shortcuts on their own. Snapoleon uses
the XDG desktop portals:

- Screen capture goes through the ScreenCast and Screenshot portals. The compositor may show its own picker or a
  permission dialog the first time. Install the portal backend for your desktop: `xdg-desktop-portal-gnome`,
  `xdg-desktop-portal-kde`, `xdg-desktop-portal-wlr` (Sway, Hyprland uses `xdg-desktop-portal-hyprland`).
- Global hotkeys use the GlobalShortcuts portal. GNOME 48 and KDE Plasma 6 support it; older compositors and some
  wlroots-based ones do not. Where it is missing, bind a key in your compositor to `snapoleon capture region`
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
pwsh packaging/windows/make-portable.ps1 -Exe target/release/snapoleon.exe -Version 0.1.0   # portable zip
```

## Uninstalling

Uninstall with the system tool (Settings, Apps; `winget uninstall Snapoleon.Snapoleon`; drag the app to the trash;
`apt remove snapoleon`). User data is left in place; delete the data folders listed above to remove history and
settings. Secrets saved in the OS keyring (Credential Manager, Keychain, Secret Service) are not in those folders; remove them with your OS credential tool.
