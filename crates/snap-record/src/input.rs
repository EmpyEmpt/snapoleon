//! Process-wide global input feeding the recording overlays.
//!
//! macOS listens through event taps that live as long as the subscription (see `platform::macos::input`).
//! Elsewhere `rdev::listen` can never be stopped, so a single listener thread is started lazily and fans
//! events out to whoever is subscribed at the moment.

use crossbeam_channel::Receiver;
use snap_core::Result;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Button {
    Left,
    Right,
    Middle,
    Other,
}

/// A non-modifier key, reduced to what the overlays tell apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Key {
    Backspace,
    Up,
    Down,
    Left,
    Right,
    /// Every other key by its label ("Enter", "F5", "A"; letters and digits by position); "?" when unknown.
    Named(String),
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

/// Global input for one consumer. The receiver is bounded; a stalled consumer loses events, never blocks the hook.
pub(crate) struct Subscription {
    pub rx: Receiver<InputEvent>,
    /// Removes the taps when dropped.
    #[cfg(target_os = "macos")]
    _taps: crate::platform::InputTaps,
}

/// Subscribe to pointer events and, when `keys`, to keystrokes. On macOS the pointer needs no permission and
/// keystrokes need Input Monitoring; a subscription that gets nothing it asked for is a permission error.
pub(crate) fn subscribe(pointer: bool, keys: bool) -> Result<Subscription> {
    #[cfg(target_os = "macos")]
    {
        let (rx, taps) = crate::platform::listen_input(pointer, keys)?;
        Ok(Subscription { rx, _taps: taps })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (pointer, keys);
        hub::subscribe().map(|rx| Subscription { rx })
    }
}

/// Human label for a key.
pub(crate) fn key_label(key: &Key) -> String {
    match key {
        Key::Backspace => "Backspace".into(),
        Key::Up => "\u{2191}".into(),
        Key::Down => "\u{2193}".into(),
        Key::Left => "\u{2190}".into(),
        Key::Right => "\u{2192}".into(),
        Key::Named(label) => label.clone(),
    }
}

/// "Ctrl + Shift + S" style label for a shortcut.
pub(crate) fn format_shortcut(mods: Mods, key: &Key) -> String {
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

#[cfg(not(target_os = "macos"))]
mod hub {
    use std::sync::OnceLock;

    use crossbeam_channel::{Receiver, Sender, TrySendError};
    use parking_lot::Mutex;
    use rdev::EventType;
    use snap_core::{Error, Result};

    use super::{Button, InputEvent, Key, Mods};

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

    pub(super) fn subscribe() -> Result<Receiver<InputEvent>> {
        let hub = hub();
        if let Some(msg) = hub.failure.lock().clone() {
            return Err(failure_error(msg));
        }
        let (tx, rx) = crossbeam_channel::bounded(4096);
        hub.subscribers.lock().push(tx);
        Ok(rx)
    }

    fn failure_error(msg: String) -> Error {
        Error::Other(anyhow::anyhow!("global input listener unavailable: {msg}"))
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
        let mut held: Vec<rdev::Key> = Vec::new();
        let result = rdev::listen(move |event| {
            match event.event_type {
                EventType::MouseMove { x, y } => {
                    pos = (x, y);
                    broadcast(hub, InputEvent::Move { x, y });
                }
                EventType::ButtonPress(button) => {
                    let button = match button {
                        rdev::Button::Left => Button::Left,
                        rdev::Button::Right => Button::Right,
                        rdev::Button::Middle => Button::Middle,
                        rdev::Button::Unknown(_) => Button::Other,
                    };
                    broadcast(hub, InputEvent::ButtonDown { button, x: pos.0, y: pos.1 });
                }
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
                    broadcast(hub, InputEvent::KeyDown { key: key_of(key), text, mods });
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

    fn set_modifier(mods: &mut Mods, key: rdev::Key, down: bool) -> bool {
        use rdev::Key;
        match key {
            Key::ControlLeft | Key::ControlRight => mods.ctrl = down,
            Key::Alt | Key::AltGr => mods.alt = down,
            Key::ShiftLeft | Key::ShiftRight => mods.shift = down,
            Key::MetaLeft | Key::MetaRight => mods.meta = down,
            _ => return false,
        }
        true
    }

    fn key_of(key: rdev::Key) -> Key {
        use rdev::Key as K;
        let named = match key {
            K::Backspace => return Key::Backspace,
            K::UpArrow => return Key::Up,
            K::DownArrow => return Key::Down,
            K::LeftArrow => return Key::Left,
            K::RightArrow => return Key::Right,
            K::Return | K::KpReturn => "Enter",
            K::Escape => "Esc",
            K::Delete | K::KpDelete => "Del",
            K::Tab => "Tab",
            K::Space => "Space",
            K::CapsLock => "Caps Lock",
            K::Home => "Home",
            K::End => "End",
            K::PageUp => "Page Up",
            K::PageDown => "Page Down",
            K::Insert => "Ins",
            K::PrintScreen => "Print Screen",
            K::ScrollLock => "Scroll Lock",
            K::Pause => "Pause",
            K::NumLock => "Num Lock",
            K::BackQuote => "`",
            K::Minus | K::KpMinus => "-",
            K::Equal => "=",
            K::LeftBracket => "[",
            K::RightBracket => "]",
            K::SemiColon => ";",
            K::Quote => "'",
            K::BackSlash | K::IntlBackslash => "\\",
            K::Comma => ",",
            K::Dot => ".",
            K::Slash | K::KpDivide => "/",
            K::KpPlus => "+",
            K::KpMultiply => "*",
            K::Function => "Fn",
            _ => "",
        };
        if !named.is_empty() {
            return Key::Named(named.into());
        }
        // Remaining variants are KeyA.., Num0.., Kp0.., F1.. and Unknown(n).
        let dbg = format!("{key:?}");
        let label = if let Some(letter) = dbg.strip_prefix("Key") {
            letter.to_string()
        } else if let Some(digit) = dbg.strip_prefix("Num").or_else(|| dbg.strip_prefix("Kp")) {
            digit.to_string()
        } else if dbg.starts_with('F') && dbg[1..].chars().all(|c| c.is_ascii_digit()) {
            dbg
        } else {
            "?".to_string()
        };
        Key::Named(label)
    }
}
