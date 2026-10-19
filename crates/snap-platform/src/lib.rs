//! OS integration for Snapoleon: global hotkeys, tray icon, clipboard,
//! notifications, single-instance IPC, autostart, secrets, URL scheme,
//! managed policy, permissions, open/reveal, console attach, DPI and sound.
//!
//! Every function compiles on Windows, macOS and Linux; where a backend cannot
//! exist the call returns [`snap_core::Error::Unsupported`].
//!
//! Threading: [`hotkeys::HotkeyManager`] and [`tray::Tray`] must be created on
//! the thread that pumps the OS event loop (the winit main thread, after the
//! loop has started). Everything else may be called from any thread.

pub mod autostart;
pub mod clipboard;
pub mod console;
pub mod dpi;
pub mod hotkeys;
pub mod ipc;
pub mod notify;
pub mod open;
pub mod permissions;
pub mod policy;
pub mod scheme;
pub mod secrets;
pub mod sound;
pub mod tray;

mod util;
