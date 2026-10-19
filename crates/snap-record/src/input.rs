//! Process-wide global input listener feeding the recording overlays.
//!
//! `rdev::listen` can never be stopped, so a single listener thread is started
//! lazily and fans events out to whoever is subscribed at the moment.

use std::sync::OnceLock;

use crossbeam_channel::{Receiver, Sender, TrySendError};
use parking_lot::Mutex;
use rdev::{Button, EventType, Key};
use snap_core::{Error, Result};

#[derive(Debug, Clone)]
pub(crate) enum InputEvent {
    Move {
        x: f64,
        y: f64,
    },
    ButtonDown {
        button: Button,
        x: f64,
        y: f64,
    },
    ButtonUp,
    /// A non-modifier key went down. `text` is the character it produced (layout aware).
    KeyDown {
        key: Key,
        text: Option<String>,
        mods: Mods,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

impl Mods {
    pub fn any_command(self) -> bool {
        self.ctrl || self.alt || self.meta
    }
}

struct Hub {
    subscribers: Mutex<Vec<Sender<InputEvent>>>,
    failure: Mutex<Option<String>>,
}

fn hub() -> &'static Hub {
    static HUB: OnceLock<Hub> = OnceLock::new();
    static STARTED: OnceLock<()> = OnceLock::new();
    let hub = HUB.get_or_init(|| Hub { subscribers: Mutex::new(Vec::new()), failure: Mutex::new(None) });
    STARTED.get_or_init(|| {
        let spawned = std::thread::Builder::new().name("snap-record-input".into()).spawn(move || listen(hub));
        if let Err(e) = spawned {
            *hub.failure.lock() = Some(e.to_string());
        }
    });
    hub
}

/// Subscribe to global input. The receiver is bounded; a stalled consumer loses events, never blocks the hook.
pub(crate) fn subscribe() -> Result<Receiver<InputEvent>> {
    let hub = hub();
    if let Some(msg) = hub.failure.lock().clone() {
        return Err(failure_error(msg));
    }
    let (tx, rx) = crossbeam_channel::bounded(4096);
    hub.subscribers.lock().push(tx);
    Ok(rx)
}

fn failure_error(msg: String) -> Error {
    if cfg!(target_os = "macos") {
        Error::permission(snap_core::t("record.error.input_permission"))
    } else {
        Error::Other(anyhow::anyhow!("global input listener unavailable: {msg}"))
    }
}

fn broadcast(hub: &Hub, ev: InputEvent) {
    hub.subscribers.lock().retain(|tx| match tx.try_send(ev.clone()) {
        Ok(()) | Err(TrySendError::Full(_)) => true,
        Err(TrySendError::Disconnected(_)) => false,
    });
}

fn listen(hub: &'static Hub) {
    let mut mods = Mods::default();
    let mut pos = (0.0f64, 0.0f64);
    let mut held: Vec<Key> = Vec::new();
    let result = rdev::listen(move |event| {
        match event.event_type {
            EventType::MouseMove { x, y } => {
                pos = (x, y);
                broadcast(hub, InputEvent::Move { x, y });
            }
            EventType::ButtonPress(button) => broadcast(hub, InputEvent::ButtonDown { button, x: pos.0, y: pos.1 }),
            EventType::ButtonRelease(_) => broadcast(hub, InputEvent::ButtonUp),
            EventType::KeyPress(key) => {
                if set_modifier(&mut mods, key, true) {
                    return;
                }
                if held.contains(&key) {
                    return; // auto-repeat
                }
                held.push(key);
                let text = event.name.filter(|s| s.chars().next().is_some_and(|c| !c.is_control()));
                broadcast(hub, InputEvent::KeyDown { key, text, mods });
            }
            EventType::KeyRelease(key) => {
                set_modifier(&mut mods, key, false);
                held.retain(|k| *k != key);
            }
            EventType::Wheel { .. } => {}
        }
    });
    let msg = match result {
        Ok(()) => "listener exited".to_string(),
        Err(e) => format!("{e:?}"),
    };
    tracing::warn!("global input listener stopped: {msg}");
    *hub.failure.lock() = Some(msg);
    hub.subscribers.lock().clear();
}

fn set_modifier(mods: &mut Mods, key: Key, down: bool) -> bool {
    match key {
        Key::ControlLeft | Key::ControlRight => mods.ctrl = down,
        Key::Alt | Key::AltGr => mods.alt = down,
        Key::ShiftLeft | Key::ShiftRight => mods.shift = down,
        Key::MetaLeft | Key::MetaRight => mods.meta = down,
        _ => return false,
    }
    true
}

/// Human label for a non-text key.
pub(crate) fn key_label(key: Key) -> String {
    let named = match key {
        Key::Return | Key::KpReturn => "Enter",
        Key::Escape => "Esc",
        Key::Backspace => "Backspace",
        Key::Delete | Key::KpDelete => "Del",
        Key::Tab => "Tab",
        Key::Space => "Space",
        Key::CapsLock => "Caps Lock",
        Key::Home => "Home",
        Key::End => "End",
        Key::PageUp => "Page Up",
        Key::PageDown => "Page Down",
        Key::Insert => "Ins",
        Key::PrintScreen => "Print Screen",
        Key::ScrollLock => "Scroll Lock",
        Key::Pause => "Pause",
        Key::NumLock => "Num Lock",
        Key::UpArrow => "\u{2191}",
        Key::DownArrow => "\u{2193}",
        Key::LeftArrow => "\u{2190}",
        Key::RightArrow => "\u{2192}",
        Key::BackQuote => "`",
        Key::Minus | Key::KpMinus => "-",
        Key::Equal => "=",
        Key::LeftBracket => "[",
        Key::RightBracket => "]",
        Key::SemiColon => ";",
        Key::Quote => "'",
        Key::BackSlash | Key::IntlBackslash => "\\",
        Key::Comma => ",",
        Key::Dot => ".",
        Key::Slash | Key::KpDivide => "/",
        Key::KpPlus => "+",
        Key::KpMultiply => "*",
        Key::Function => "Fn",
        _ => "",
    };
    if !named.is_empty() {
        return named.to_string();
    }
    // Remaining variants are KeyA.., Num0.., Kp0.., F1.. and Unknown(n).
    let dbg = format!("{key:?}");
    if let Some(letter) = dbg.strip_prefix("Key") {
        return letter.to_string();
    }
    if let Some(digit) = dbg.strip_prefix("Num").or_else(|| dbg.strip_prefix("Kp")) {
        return digit.to_string();
    }
    if dbg.starts_with('F') && dbg[1..].chars().all(|c| c.is_ascii_digit()) {
        return dbg;
    }
    "?".to_string()
}

/// "Ctrl + Shift + S" style label for a shortcut.
pub(crate) fn format_shortcut(mods: Mods, key: Key) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(5);
    if mods.ctrl {
        parts.push("Ctrl".into());
    }
    if mods.alt {
        parts.push(if cfg!(target_os = "macos") { "Option" } else { "Alt" }.into());
    }
    if mods.shift {
        parts.push("Shift".into());
    }
    if mods.meta {
        parts.push(
            if cfg!(target_os = "macos") {
                "Cmd"
            } else if cfg!(windows) {
                "Win"
            } else {
                "Super"
            }
            .into(),
        );
    }
    parts.push(key_label(key));
    parts.join(" + ")
}
