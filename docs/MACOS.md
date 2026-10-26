# Developing on macOS

Snapoleon needs macOS 14 Sonoma or newer, on Apple Silicon or Intel. This page is for people building it: how to
get a build that macOS treats as a real app, how to reset its privacy permissions, and how to look at the UI from a
script or an agent. Users read [INSTALL.md](INSTALL.md).

## Why a plain `cargo run` is not enough

macOS decides privacy questions (Screen Recording, Accessibility, Input Monitoring, Microphone) per app, and it
identifies the app by two things:

- **The responsible process.** A program started from Terminal, iTerm, an editor's terminal or an agent host is
  checked as part of that app. `cargo run -p snapoleon` therefore captures with *Terminal's* Screen Recording
  permission, and the prompts name Terminal. Only a copy started through LaunchServices (Finder, the Dock, `open`,
  a login item) is its own responsible process.
- **The code signature.** A grant is stored against the app's designated requirement. An ad-hoc signature is a
  requirement on the code hash, so every rebuild looks like a new app and the old switch in System Settings stops
  working. A certificate gives `identifier "app.snapoleon" and certificate leaf = ...`, which stays the same.

The bundle also carries what macOS reads from `Info.plist`: the bundle id `app.snapoleon`, the usage strings shown in
permission prompts, the `snapoleon://` scheme, `LSUIElement` (menu bar app) and `LSMinimumSystemVersion`.

So: use `cargo run` and the dev harness (`SNAPOLEON_DEV=<view>`, see [APP.md](APP.md#dev-harness)) for UI work, and
the bundle below whenever capture, permissions, notifications, hotkeys or URL handling are involved.

## Setup

```sh
xcode-select --install                     # C toolchain, codesign
brew install nasm                          # assembly in the codec and crypto dependencies
rustup update stable                       # Rust 1.95 or newer
packaging/macos/make-signing-identity.sh   # once per Mac; asks for your password to trust the certificate
```

`make-signing-identity.sh` creates a self-signed code-signing certificate, "Snapoleon Local Signing", in your login
keychain. It is only trusted on this Mac; it does not make a build distributable. If `codesign` later asks for keychain
access, choose Always Allow.

## Build, run, install

```sh
packaging/macos/bundle.sh --debug --install   # build, bundle, sign, replace /Applications/Snapoleon.app, launch
packaging/macos/bundle.sh --debug --run       # build and open target/macos/Snapoleon.app without installing
packaging/macos/bundle.sh                     # release build, bundle only
```

`bundle.sh` signs with `$SNAPOLEON_SIGN_IDENTITY`, else "Snapoleon Local Signing" when it exists, else ad-hoc (with a
warning: permissions then have to be granted again after every build). It always signs with the hardened runtime and
`packaging/macos/entitlements.plist`, like a release, and builds with `MACOSX_DEPLOYMENT_TARGET=14.0`.

`--install` quits the running app through its own IPC (`snapoleon quit`) and ends only the GUI process: `snapoleon mcp`
servers that agents keep open are left running. It then unregisters the build copy and a mounted
`/Volumes/Snapoleon` DMG from LaunchServices, so only `/Applications/Snapoleon.app` answers to `app.snapoleon`
(`open -b`, `snapoleon://` links, notifications and System Settings otherwise pick whichever copy they saw last).

The app is a single instance and owns the global hotkeys. Run one copy at a time; a second launch hands its
arguments to the first and exits.

## Permissions

Check what the app and your terminal have:

```sh
/Applications/Snapoleon.app/Contents/MacOS/snapoleon doctor
```

It prints the executable, bundle, designated requirement, the app it was started from, and a table of the four
permissions for this process and for the running app (asked over IPC).

Start over, as a new user would see it:

```sh
packaging/macos/bundle.sh --debug --install --reset-permissions
# or by hand:
tccutil reset ScreenCapture app.snapoleon
tccutil reset Accessibility app.snapoleon
tccutil reset ListenEvent app.snapoleon     # Input Monitoring
tccutil reset Microphone app.snapoleon
```

After granting Screen Recording macOS offers Quit & Reopen; capture works in the reopened app. If a switch in System
Settings is on but `doctor` says no, the grant belongs to an older signature: remove the entry with the minus button
and grant it again.

## Command line and MCP from a terminal

Headless commands (`snapoleon capture --region ...`, `snapoleon mcp`) run in the terminal's process, so on their
own they would capture with the terminal's permission. When this process may not read the screen, they hand the grab
to the running app over IPC (`Request::Grab`), starting it in the background first when it is not running and
the command runs from inside the bundle. A GUI start from a terminal (`snapoleon`, `snapoleon capture --pipeline
...`) relaunches through LaunchServices. Details: [CLI.md](CLI.md#macos).

Put a `snapoleon` command on your PATH with `snapoleon install-cli` (or Settings > Advanced > Command line tool). It
writes a two-line script that `exec`s the binary inside the bundle. Do not symlink the executable: a symlinked binary
runs without its bundle.

## Looking at the UI from a script or an agent

The app's windows can be read without focusing or clicking anything. With [peekaboo](https://github.com/steipete/Peekaboo)
(it needs Screen Recording and Accessibility itself):

```sh
peekaboo window list --app snapoleon --json             # window ids, titles and frames
peekaboo see --window-id <id> --no-elements --path out.png
peekaboo see --app snapoleon --json                      # the accessibility tree: named buttons, fields, ...
peekaboo click --on <element id>                         # drive a named control
```

`peekaboo see --window-id` works while the window is covered by others. `SNAPOLEON_DEV_SHOT` (the dev harness's own
screenshot) goes through Snapoleon's capture code and so depends on the terminal's Screen Recording permission;
peekaboo is the dependable way to look at a window on macOS.

Window lists from the window server include an untitled, invisible 500x500 window owned by Snapoleon at the bottom-left
of the main screen. It is `TUINSWindow`, the text input system's cursor accessory (the input source and Caps Lock
indicator) that macOS creates for every app with a text field. It is harmless; `snapoleon list windows` and the MCP
`list_windows` leave it out.

## Logs

| Run | Log |
|---|---|
| The installed app | `~/Library/Application Support/app.Snapoleon.Snapoleon/logs/snapoleon.log` |
| The dev harness | `target/debug/data/logs/snapoleon.log` (portable data next to the binary) |
| Headless commands | standard error only |

`log stream --predicate 'subsystem == "com.apple.TCC"' --info` shows the privacy decisions macOS makes as they happen.

## Releases

`.github/workflows/release.yml` builds a universal binary and packages it with cargo-packager. With the Apple
secrets it signs with the Developer ID (hardened runtime, entitlements) and notarizes the app and the DMG; without
them it signs ad-hoc as `app.snapoleon`, so the Info.plist is still bound. Either way the workflow checks the signature
before it publishes.
