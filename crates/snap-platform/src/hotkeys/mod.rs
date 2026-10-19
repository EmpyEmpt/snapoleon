//! Global hotkeys.
//!
//! Windows, macOS and X11 use `global-hotkey`. On Wayland the
//! `org.freedesktop.portal.GlobalShortcuts` portal is used when available;
//! otherwise [`HotkeyManager::new`] fails with `Unsupported` and a hint to bind
//! `snapoleon capture region` in the desktop's own shortcut settings.
//!
//! # Threading
//!
//! `global-hotkey` needs a thread that pumps OS messages: on Windows the
//! manager owns a hidden window whose `WM_HOTKEY` messages are delivered to the
//! creating thread, on macOS the Carbon event handler lives on the main run
//! loop. Create the [`HotkeyManager`] on the main/UI thread **after** the winit
//! event loop has started (for example in `ApplicationHandler::resumed`), and
//! keep it on that thread - the type is intentionally not `Send`. The callback
//! itself runs on a background thread owned by the manager.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::RecvTimeoutError;
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use parking_lot::Mutex;
use snap_core::hotkey::HotkeyBinding;
use snap_core::{Error, Result, tf};

mod accel;
#[cfg(all(target_os = "linux", feature = "wayland-portal"))]
mod portal;

use accel::Accel;

/// Event for the binding at this index of the slice given to [`HotkeyManager::register`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    Pressed(usize),
    Released(usize),
}

/// Check that an accelerator string can be parsed and bound.
pub fn validate(accel: &str) -> Result<()> {
    Accel::parse(accel).map(|_| ())
}

/// Human-readable form of an accelerator: `⇧⌘4` on macOS, `Ctrl+Shift+4` elsewhere.
/// Unparseable input is returned unchanged.
pub fn display(accel: &str) -> String {
    Accel::parse(accel).map(Accel::label).unwrap_or_else(|_| accel.trim().to_string())
}

/// Accelerator string for an egui key press, for the hotkey recorder widget.
/// `None` for keys that have no accelerator name. The Windows/Super key is not
/// reported by egui, so it cannot be recorded here.
#[cfg(feature = "egui")]
pub fn from_egui(mods: egui::Modifiers, key: egui::Key) -> Option<String> {
    use egui::Key;
    let name: String = match key {
        Key::Num0 => "0".into(),
        Key::Num1 => "1".into(),
        Key::Num2 => "2".into(),
        Key::Num3 => "3".into(),
        Key::Num4 => "4".into(),
        Key::Num5 => "5".into(),
        Key::Num6 => "6".into(),
        Key::Num7 => "7".into(),
        Key::Num8 => "8".into(),
        Key::Num9 => "9".into(),
        Key::ArrowUp => "Up".into(),
        Key::ArrowDown => "Down".into(),
        Key::ArrowLeft => "Left".into(),
        Key::ArrowRight => "Right".into(),
        Key::Escape => "Esc".into(),
        Key::Tab => "Tab".into(),
        Key::Backspace => "Backspace".into(),
        Key::Enter => "Enter".into(),
        Key::Space => "Space".into(),
        Key::Insert => "Insert".into(),
        Key::Delete => "Delete".into(),
        Key::Home => "Home".into(),
        Key::End => "End".into(),
        Key::PageUp => "PageUp".into(),
        Key::PageDown => "PageDown".into(),
        Key::Pause => "Pause".into(),
        Key::Comma => ",".into(),
        Key::Period => ".".into(),
        Key::Slash => "/".into(),
        Key::Backslash => "\\".into(),
        Key::Semicolon => ";".into(),
        Key::Quote => "'".into(),
        Key::Backtick => "`".into(),
        Key::Minus => "-".into(),
        Key::Equals | Key::Plus => "=".into(),
        Key::OpenBracket => "[".into(),
        Key::CloseBracket => "]".into(),
        other => {
            let n = other.name();
            let is_letter = n.len() == 1 && n.chars().all(|c| c.is_ascii_alphabetic());
            let is_function = n.strip_prefix('F').and_then(|d| d.parse::<u8>().ok()).is_some_and(|d| (1..=24).contains(&d));
            if !(is_letter || is_function) {
                return None;
            }
            n.to_string()
        }
    };
    let mut parts: Vec<&str> = Vec::new();
    if mods.ctrl {
        parts.push("Ctrl");
    }
    if mods.alt {
        parts.push("Alt");
    }
    if mods.shift {
        parts.push("Shift");
    }
    if mods.mac_cmd {
        parts.push("Cmd");
    }
    parts.push(&name);
    Some(parts.join("+"))
}

/// Only one manager may exist at a time: `global-hotkey` routes every event
/// through a single process-wide channel.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// State shared with the event thread.
struct Routes {
    /// OS hotkey id -> binding index.
    ids: Mutex<HashMap<u32, usize>>,
    paused: AtomicBool,
}

enum Backend {
    Native(Native),
    #[cfg(all(target_os = "linux", feature = "wayland-portal"))]
    Portal(portal::Portal),
}

struct Native {
    manager: GlobalHotKeyManager,
    stop: Arc<AtomicBool>,
    poller: Option<JoinHandle<()>>,
}

pub struct HotkeyManager {
    backend: Backend,
    routes: Arc<Routes>,
    /// Hotkeys currently registered with the OS (native backend), kept for pause/resume.
    bound: Vec<(HotKey, usize)>,
    paused: bool,
}

impl HotkeyManager {
    /// Create the manager. See the module docs for the threading requirements.
    ///
    /// `on_event` is invoked from a background thread owned by the manager.
    pub fn new(on_event: impl Fn(HotkeyEvent) + Send + 'static) -> Result<Self> {
        if ACTIVE.swap(true, Ordering::SeqCst) {
            return Err(Error::invalid("a HotkeyManager already exists"));
        }
        let routes = Arc::new(Routes { ids: Mutex::new(HashMap::new()), paused: AtomicBool::new(false) });
        match Self::backend(on_event, &routes) {
            Ok(backend) => Ok(Self { backend, routes, bound: Vec::new(), paused: false }),
            Err(e) => {
                ACTIVE.store(false, Ordering::SeqCst);
                Err(e)
            }
        }
    }

    fn backend(on_event: impl Fn(HotkeyEvent) + Send + 'static, routes: &Arc<Routes>) -> Result<Backend> {
        #[cfg(target_os = "linux")]
        if is_wayland() {
            #[cfg(feature = "wayland-portal")]
            return portal::Portal::new(on_event, routes.clone()).map(Backend::Portal);
            #[cfg(not(feature = "wayland-portal"))]
            return Err(wayland_unsupported(""));
        }
        let manager = GlobalHotKeyManager::new().map_err(|e| crate::util::other(format_args!("global hotkeys: {e}")))?;
        let stop = Arc::new(AtomicBool::new(false));
        let poller = spawn_poller(on_event, routes.clone(), stop.clone())?;
        Ok(Backend::Native(Native { manager, stop, poller: Some(poller) }))
    }

    /// Replace all registrations with `bindings`. Disabled bindings are skipped.
    /// Returns one result per attempted binding, keyed by its index in `bindings`;
    /// a conflict with another application is reported as `Err` for that binding only.
    pub fn register(&mut self, bindings: &[HotkeyBinding]) -> Vec<(usize, Result<()>)> {
        self.unregister_all();
        let mut results: Vec<(usize, Result<()>)> = Vec::new();
        let mut parsed: Vec<(usize, Accel)> = Vec::new();
        let mut seen: HashMap<u32, usize> = HashMap::new();
        for (i, b) in bindings.iter().enumerate().filter(|(_, b)| b.enabled) {
            match Accel::parse(&b.accel) {
                Err(e) => results.push((i, Err(e))),
                Ok(a) => match seen.insert(a.hotkey().id(), i) {
                    Some(first) => results.push((
                        i,
                        Err(Error::invalid(tf("platform.hotkey.duplicate", &[("accel", &a.label()), ("n", &(first + 1).to_string())]))),
                    )),
                    None => parsed.push((i, a)),
                },
            }
        }

        match &mut self.backend {
            Backend::Native(native) => {
                for (i, a) in parsed {
                    let hk = a.hotkey();
                    let r = if self.paused { Ok(()) } else { native.manager.register(hk).map_err(|e| native_error(a, e)) };
                    if r.is_ok() {
                        self.routes.ids.lock().insert(hk.id(), i);
                        self.bound.push((hk, i));
                    }
                    results.push((i, r));
                }
            }
            #[cfg(all(target_os = "linux", feature = "wayland-portal"))]
            Backend::Portal(p) => {
                let outcome = p.bind(&parsed);
                for (i, _) in &parsed {
                    results.push((*i, outcome.as_ref().map(|_| ()).map_err(crate::util::other)));
                }
            }
        }
        results.sort_by_key(|(i, _)| *i);
        results
    }

    pub fn unregister_all(&mut self) {
        match &mut self.backend {
            Backend::Native(native) => {
                if !self.paused {
                    for (hk, _) in &self.bound {
                        if let Err(e) = native.manager.unregister(*hk) {
                            tracing::debug!("unregister hotkey: {e}");
                        }
                    }
                }
            }
            #[cfg(all(target_os = "linux", feature = "wayland-portal"))]
            Backend::Portal(p) => p.reset(),
        }
        self.bound.clear();
        self.routes.ids.lock().clear();
    }

    /// Suspend (or resume) all hotkeys, e.g. while the hotkey recorder has focus.
    /// On native backends the OS registrations are released while paused so the
    /// keys reach the focused window. Registrations made while paused are only
    /// attempted on resume: the bindings the OS refuses then are returned by
    /// their index in the slice given to [`HotkeyManager::register`].
    pub fn pause(&mut self, paused: bool) -> Vec<(usize, Error)> {
        let mut refused = Vec::new();
        if self.paused == paused {
            return refused;
        }
        self.paused = paused;
        self.routes.paused.store(paused, Ordering::SeqCst);
        match &self.backend {
            Backend::Native(native) => {
                for (hk, index) in &self.bound {
                    if paused {
                        if let Err(e) = native.manager.unregister(*hk) {
                            tracing::warn!("unregistering hotkey {hk}: {e}");
                        }
                    } else if let Err(e) = native.manager.register(*hk) {
                        tracing::warn!("re-registering hotkey {hk}: {e}");
                        refused.push((*index, native_error(Accel { mods: hk.mods, key: hk.key }, e)));
                    }
                }
            }
            // The portal keeps its binding; events are dropped while `routes.paused` is set.
            #[cfg(all(target_os = "linux", feature = "wayland-portal"))]
            Backend::Portal(_) => {}
        }
        refused
    }
}

impl Drop for HotkeyManager {
    fn drop(&mut self) {
        self.unregister_all();
        match &mut self.backend {
            Backend::Native(native) => {
                native.stop.store(true, Ordering::SeqCst);
                if let Some(t) = native.poller.take() {
                    let _ = t.join();
                }
            }
            #[cfg(all(target_os = "linux", feature = "wayland-portal"))]
            Backend::Portal(_) => {}
        }
        ACTIVE.store(false, Ordering::SeqCst);
    }
}

fn native_error(a: Accel, e: global_hotkey::Error) -> Error {
    match e {
        global_hotkey::Error::AlreadyRegistered(_) => Error::invalid(tf("platform.hotkey.in_use", &[("accel", &a.label())])),
        other => crate::util::other(format_args!("{}: {other}", a.label())),
    }
}

fn spawn_poller(on_event: impl Fn(HotkeyEvent) + Send + 'static, routes: Arc<Routes>, stop: Arc<AtomicBool>) -> Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("snap-hotkeys".into())
        .spawn(move || {
            let rx = GlobalHotKeyEvent::receiver();
            while !stop.load(Ordering::SeqCst) {
                match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(ev) => {
                        if routes.paused.load(Ordering::SeqCst) {
                            continue;
                        }
                        let Some(index) = routes.ids.lock().get(&ev.id).copied() else { continue };
                        on_event(match ev.state {
                            HotKeyState::Pressed => HotkeyEvent::Pressed(index),
                            HotKeyState::Released => HotkeyEvent::Released(index),
                        });
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .map_err(Error::from)
}

#[cfg(target_os = "linux")]
fn is_wayland() -> bool {
    std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v.eq_ignore_ascii_case("wayland")) || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

#[cfg(target_os = "linux")]
pub(crate) fn wayland_unsupported(reason: &str) -> Error {
    let hint = snap_core::t("platform.hotkey.wayland_hint");
    if reason.is_empty() { Error::unsupported(hint.into_owned()) } else { Error::unsupported(format!("{reason} {hint}")) }
}
