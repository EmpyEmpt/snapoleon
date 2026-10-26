# Changelog

All notable changes to Snapoleon. Versions follow [Semantic Versioning](https://semver.org).

## [0.2.0] - 2026-10-26

The macOS release: Snapoleon now runs on a Mac the way a Mac app should, and a lot of rough edges are gone on every
platform.

### macOS

- **Screenshots work.** Capture now uses ScreenCaptureKit. Before, macOS 15 and newer handed back only the
  wallpaper. Colors are correct on P3 and other wide-gamut displays, Retina captures keep their size, and setups
  with displays of different resolutions put every window and selection on the right screen.
- **Permissions stick.** The app is signed with a stable identity, so a Screen Recording grant is remembered.
  Setup shows what is granted, opens the right System Settings page and offers to reopen the app when macOS
  needs that. It picks up where it left off after macOS restarts it.
- **The capture overlay can never get stuck.** Escape, the capture shortcut again, or switching apps always closes
  it, and the app you were in gets the keyboard back. A macOS permission alert is never covered.
- **Default shortcuts no longer clash with the system's own screenshot keys.** Region capture is now ⇧⌘2, all
  screens ⇧⌘1, window ⌥⇧⌘2. Old defaults are moved automatically; shortcuts you picked yourself are kept.
- **Real notifications** with the Snapoleon icon, a thumbnail, and Open / Show in Finder buttons. Clicking one opens
  the capture.
- **Native windows**: standard title bar buttons, rounded corners, full screen, an app menu with ⌘, ⌘W ⌘M ⌘Q and
  a working Edit menu. A Dock icon appears while a window is open.
- The menu bar icon follows the light or dark menu bar, its menu shows keyboard shortcuts and your recent captures,
  and it can be opened with VoiceOver.
- Opening Snapoleon again from Finder or Spotlight shows the Library; `snapoleon://` links and `.snap` files open
  in the running app.
- Launch at login uses the system Login Items. Drag captures out of the Library into other apps. Copied images
  paste at their on-screen size. Saved images carry their resolution.
- Text recognition is ready within seconds of launch instead of taking half a minute the first time.
- Hide desktop icons no longer restarts Finder.

### Everywhere

- Dropdowns and menus stay inside their window, flip upward when there is no room, and scroll when long.
- Settings: clicking the section you are in takes you back to its top; pages remember their own scroll position;
  rows rearrange instead of overlapping in narrow windows; a new **Interface size** setting scales every window.
- Hotkeys: duplicates are refused with a clear message and a Replace button; conflicts with system shortcuts get a
  warning and a button to the system's keyboard settings.
- Escape on an open list closes only the list, not the window.
- Text annotations can have an outline, so they stay readable on busy screenshots. Annotations are kept inside
  the picture.
- A notice after an unexpected quit, with the crash report.
- Accessibility: buttons and settings have names for VoiceOver, Narrator and UI automation.
- Russian translation pass: shorter notices, localized units.

### Command line and AI agents

- New `snapoleon doctor` shows versions, paths, the running app and every permission; `snapoleon list` shows
  monitors and windows with the indexes capture options take; `capture --json`.
- On macOS, captures from a terminal or the MCP server go through the running app, so they use its permissions.
- `snapoleon install-cli`, also in Settings > Advanced, puts the `snapoleon` command on PATH on a Mac.
- MCP: full-resolution output to a file and documented coordinates.

### For developers

- Rust 1.95 or newer; macOS 14 or newer.
- `packaging/macos/bundle.sh` builds a signed app bundle that keeps its permissions across rebuilds; see
  `docs/MACOS.md`. `AGENTS.md` explains how coding agents build, run and check the UI.
- The release workflow signs the macOS app (ad-hoc without a certificate) and stamps the version into the binary.

## [0.1.0-1] - 2026-10-20

First preview build.
