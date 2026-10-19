//! Accelerator strings (`Ctrl+Shift+4`, `Cmd+Shift+2`, `Alt+PrintScreen`, ...)
//! to OS-independent key codes, and back to human-readable labels.

use global_hotkey::hotkey::{CMD_OR_CTRL, Code, HotKey, Modifiers};
use snap_core::{Error, Result, tf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Accel {
    pub mods: Modifiers,
    pub key: Code,
}

impl Accel {
    pub fn parse(text: &str) -> Result<Accel> {
        let bad = |key: &str, token: &str| Error::invalid(tf(key, &[("accel", text), ("token", token)]));
        let mut mods = Modifiers::empty();
        let mut key = None;
        for token in text.split('+') {
            let token = token.trim();
            if token.is_empty() {
                return Err(bad("platform.hotkey.empty_token", token));
            }
            if let Some(m) = parse_modifier(token) {
                if mods.contains(m) {
                    return Err(bad("platform.hotkey.duplicate_modifier", token));
                }
                mods |= m;
            } else if let Some(code) = parse_key(token) {
                if key.replace(code).is_some() {
                    return Err(bad("platform.hotkey.two_keys", token));
                }
            } else {
                return Err(bad("platform.hotkey.unknown_key", token));
            }
        }
        let key = key.ok_or_else(|| bad("platform.hotkey.no_key", text))?;
        if is_typing_key(key) && !mods.intersects(Modifiers::CONTROL | Modifiers::ALT | Modifiers::META) {
            return Err(bad("platform.hotkey.needs_modifier", text));
        }
        Ok(Accel { mods, key })
    }

    pub fn hotkey(self) -> HotKey {
        HotKey::new(Some(self.mods), self.key)
    }

    /// Label shown to the user: `⇧⌘4` on macOS, `Ctrl+Shift+4` elsewhere.
    pub fn label(self) -> String {
        if cfg!(target_os = "macos") {
            let mut s = String::new();
            for (m, glyph) in [(Modifiers::CONTROL, "⌃"), (Modifiers::ALT, "⌥"), (Modifiers::SHIFT, "⇧"), (Modifiers::META, "⌘")] {
                if self.mods.contains(m) {
                    s.push_str(glyph);
                }
            }
            s.push_str(&key_label(self.key, true));
            s
        } else {
            let win = if cfg!(windows) { "Win" } else { "Super" };
            let mut parts = Vec::new();
            for (m, name) in [(Modifiers::CONTROL, "Ctrl"), (Modifiers::ALT, "Alt"), (Modifiers::SHIFT, "Shift"), (Modifiers::META, win)] {
                if self.mods.contains(m) {
                    parts.push(name.to_string());
                }
            }
            parts.push(key_label(self.key, false));
            parts.join("+")
        }
    }
}

fn parse_modifier(token: &str) -> Option<Modifiers> {
    Some(match token.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Modifiers::CONTROL,
        "alt" | "option" | "opt" => Modifiers::ALT,
        "shift" => Modifiers::SHIFT,
        "cmd" | "command" | "super" | "win" | "windows" | "meta" => Modifiers::META,
        "cmdorctrl" | "commandorcontrol" | "ctrlorcmd" | "controlorcommand" => CMD_OR_CTRL,
        _ => return None,
    })
}

/// Keys that are not tied to typing and may be bound without a modifier.
fn is_typing_key(key: Code) -> bool {
    function_key_index(key).is_none()
        && !matches!(
            key,
            Code::PrintScreen | Code::ScrollLock | Code::Pause | Code::Insert | Code::Delete | Code::Home | Code::End | Code::PageUp | Code::PageDown
        )
}

const LETTERS: [Code; 26] = [
    Code::KeyA, Code::KeyB, Code::KeyC, Code::KeyD, Code::KeyE, Code::KeyF, Code::KeyG, Code::KeyH, Code::KeyI,
    Code::KeyJ, Code::KeyK, Code::KeyL, Code::KeyM, Code::KeyN, Code::KeyO, Code::KeyP, Code::KeyQ, Code::KeyR,
    Code::KeyS, Code::KeyT, Code::KeyU, Code::KeyV, Code::KeyW, Code::KeyX, Code::KeyY, Code::KeyZ,
];

const DIGITS: [Code; 10] = [
    Code::Digit0, Code::Digit1, Code::Digit2, Code::Digit3, Code::Digit4,
    Code::Digit5, Code::Digit6, Code::Digit7, Code::Digit8, Code::Digit9,
];

const NUMPAD_DIGITS: [Code; 10] = [
    Code::Numpad0, Code::Numpad1, Code::Numpad2, Code::Numpad3, Code::Numpad4,
    Code::Numpad5, Code::Numpad6, Code::Numpad7, Code::Numpad8, Code::Numpad9,
];

const FUNCTION_KEYS: [Code; 24] = [
    Code::F1, Code::F2, Code::F3, Code::F4, Code::F5, Code::F6, Code::F7, Code::F8, Code::F9, Code::F10, Code::F11, Code::F12,
    Code::F13, Code::F14, Code::F15, Code::F16, Code::F17, Code::F18, Code::F19, Code::F20, Code::F21, Code::F22, Code::F23, Code::F24,
];

/// Canonical name, accepted lower-case aliases.
const NAMED: &[(Code, &str, &[&str])] = &[
    (Code::Space, "Space", &["spacebar"]),
    (Code::Tab, "Tab", &[]),
    (Code::Enter, "Enter", &["return"]),
    (Code::Escape, "Esc", &["escape"]),
    (Code::Backspace, "Backspace", &["bksp"]),
    (Code::Delete, "Delete", &["del"]),
    (Code::Insert, "Insert", &["ins"]),
    (Code::Home, "Home", &[]),
    (Code::End, "End", &[]),
    (Code::PageUp, "PageUp", &["pgup"]),
    (Code::PageDown, "PageDown", &["pgdn", "pgdown"]),
    (Code::ArrowUp, "Up", &["arrowup"]),
    (Code::ArrowDown, "Down", &["arrowdown"]),
    (Code::ArrowLeft, "Left", &["arrowleft"]),
    (Code::ArrowRight, "Right", &["arrowright"]),
    (Code::PrintScreen, "PrintScreen", &["prtsc", "prtscn", "prtscr", "printscr", "print", "sysrq"]),
    (Code::ScrollLock, "ScrollLock", &[]),
    (Code::Pause, "Pause", &["break"]),
    (Code::Comma, ",", &["comma"]),
    (Code::Period, ".", &["period", "dot"]),
    (Code::Slash, "/", &["slash"]),
    (Code::Backslash, "\\", &["backslash"]),
    (Code::Semicolon, ";", &["semicolon"]),
    (Code::Quote, "'", &["quote", "apostrophe"]),
    (Code::Backquote, "`", &["backquote", "grave", "tilde", "~"]),
    (Code::Minus, "-", &["minus", "dash"]),
    (Code::Equal, "=", &["equal", "equals", "plus"]),
    (Code::BracketLeft, "[", &["bracketleft", "lbracket"]),
    (Code::BracketRight, "]", &["bracketright", "rbracket"]),
    (Code::NumpadAdd, "NumAdd", &["numpadadd"]),
    (Code::NumpadSubtract, "NumSub", &["numpadsubtract", "numsubtract", "num-"]),
    (Code::NumpadMultiply, "NumMul", &["numpadmultiply", "nummultiply", "num*"]),
    (Code::NumpadDivide, "NumDiv", &["numpaddivide", "numdivide", "num/"]),
    (Code::NumpadDecimal, "NumDecimal", &["numpaddecimal", "num."]),
    (Code::NumpadEnter, "NumEnter", &["numpadenter"]),
];

fn function_key_index(key: Code) -> Option<usize> {
    FUNCTION_KEYS.iter().position(|k| *k == key)
}

fn parse_key(token: &str) -> Option<Code> {
    let lower = token.to_ascii_lowercase();
    let mut chars = lower.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_lowercase() {
            return Some(LETTERS[c as usize - 'a' as usize]);
        }
        if c.is_ascii_digit() {
            return Some(DIGITS[c as usize - '0' as usize]);
        }
    }
    if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<usize>().ok()) {
        return (1..=24).contains(&n).then(|| FUNCTION_KEYS[n - 1]);
    }
    for prefix in ["numpad", "num"] {
        if let Some(n) = lower.strip_prefix(prefix).and_then(|n| n.parse::<usize>().ok()) {
            return NUMPAD_DIGITS.get(n).copied();
        }
    }
    NAMED
        .iter()
        .find(|(_, name, aliases)| name.to_ascii_lowercase() == lower || aliases.contains(&lower.as_str()))
        .map(|(code, _, _)| *code)
}

fn key_label(key: Code, mac: bool) -> String {
    if let Some(i) = LETTERS.iter().position(|k| *k == key) {
        return char::from(b'A' + i as u8).to_string();
    }
    if let Some(i) = DIGITS.iter().position(|k| *k == key) {
        return i.to_string();
    }
    if let Some(i) = NUMPAD_DIGITS.iter().position(|k| *k == key) {
        return format!("Num{i}");
    }
    if let Some(i) = function_key_index(key) {
        return format!("F{}", i + 1);
    }
    if mac {
        let glyph = match key {
            Code::Enter => Some("↩"),
            Code::Escape => Some("⎋"),
            Code::Tab => Some("⇥"),
            Code::Backspace => Some("⌫"),
            Code::Delete => Some("⌦"),
            Code::ArrowUp => Some("↑"),
            Code::ArrowDown => Some("↓"),
            Code::ArrowLeft => Some("←"),
            Code::ArrowRight => Some("→"),
            Code::Home => Some("↖"),
            Code::End => Some("↘"),
            Code::PageUp => Some("⇞"),
            Code::PageDown => Some("⇟"),
            _ => None,
        };
        if let Some(g) = glyph {
            return g.into();
        }
    }
    NAMED.iter().find(|(c, _, _)| *c == key).map(|(_, name, _)| (*name).to_string()).unwrap_or_else(|| key.to_string())
}

/// XDG shortcuts trigger (`CTRL+SHIFT+4`, `Print`) for the GlobalShortcuts portal.
#[cfg(all(target_os = "linux", feature = "wayland-portal"))]
pub(crate) fn xdg_trigger(a: Accel) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (m, name) in [(Modifiers::CONTROL, "CTRL"), (Modifiers::ALT, "ALT"), (Modifiers::SHIFT, "SHIFT"), (Modifiers::META, "LOGO")] {
        if a.mods.contains(m) {
            parts.push(name.into());
        }
    }
    parts.push(xdg_keysym(a.key));
    parts.join("+")
}

#[cfg(all(target_os = "linux", feature = "wayland-portal"))]
fn xdg_keysym(key: Code) -> String {
    if let Some(i) = LETTERS.iter().position(|k| *k == key) {
        return char::from(b'a' + i as u8).to_string();
    }
    if let Some(i) = DIGITS.iter().position(|k| *k == key) {
        return i.to_string();
    }
    if let Some(i) = NUMPAD_DIGITS.iter().position(|k| *k == key) {
        return format!("KP_{i}");
    }
    if let Some(i) = function_key_index(key) {
        return format!("F{}", i + 1);
    }
    match key {
        Code::Space => "space",
        Code::Tab => "Tab",
        Code::Enter => "Return",
        Code::Escape => "Escape",
        Code::Backspace => "BackSpace",
        Code::Delete => "Delete",
        Code::Insert => "Insert",
        Code::Home => "Home",
        Code::End => "End",
        Code::PageUp => "Prior",
        Code::PageDown => "Next",
        Code::ArrowUp => "Up",
        Code::ArrowDown => "Down",
        Code::ArrowLeft => "Left",
        Code::ArrowRight => "Right",
        Code::PrintScreen => "Print",
        Code::ScrollLock => "Scroll_Lock",
        Code::Pause => "Pause",
        Code::Comma => "comma",
        Code::Period => "period",
        Code::Slash => "slash",
        Code::Backslash => "backslash",
        Code::Semicolon => "semicolon",
        Code::Quote => "apostrophe",
        Code::Backquote => "grave",
        Code::Minus => "minus",
        Code::Equal => "equal",
        Code::BracketLeft => "bracketleft",
        Code::BracketRight => "bracketright",
        Code::NumpadAdd => "KP_Add",
        Code::NumpadSubtract => "KP_Subtract",
        Code::NumpadMultiply => "KP_Multiply",
        Code::NumpadDivide => "KP_Divide",
        Code::NumpadDecimal => "KP_Decimal",
        Code::NumpadEnter => "KP_Enter",
        _ => "",
    }
    .into()
}
