//! Listen-only event taps for the recording overlays, on a run-loop thread of their own.
//!
//! The pointer tap needs no permission. The key tap needs Input Monitoring, so it is only made when keystrokes
//! are shown and the permission is there: trying without it would raise the system prompt in the middle of a
//! recording. Keys are read from the event itself (key code, flags and the typed text), never through the Text
//! Input Source calls that must run on the main thread. Taps are made per subscription, so a permission granted
//! while the app runs applies to the next recording.

use std::{
    ffi::c_void,
    ptr::NonNull,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::anyhow;
use crossbeam_channel::{Receiver, Sender};
use objc2_core_foundation::{CFMachPort, CFRetained, CFRunLoop, kCFRunLoopDefaultMode};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventMask, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventTapProxy,
    CGEventType, CGPreflightListenEventAccess,
};
use snap_core::{Error, Result, t};

use crate::input::{Button, InputEvent, Key, Mods};

const POINTER_EVENTS: [CGEventType; 10] = [
    CGEventType::MouseMoved,
    CGEventType::LeftMouseDown,
    CGEventType::LeftMouseUp,
    CGEventType::LeftMouseDragged,
    CGEventType::RightMouseDown,
    CGEventType::RightMouseUp,
    CGEventType::RightMouseDragged,
    CGEventType::OtherMouseDown,
    CGEventType::OtherMouseUp,
    CGEventType::OtherMouseDragged,
];

fn mask(events: &[CGEventType]) -> CGEventMask {
    events.iter().fold(0, |m, e| m | 1 << e.0)
}

/// The tap thread's run loop; dropping this ends the thread, which removes the taps.
pub(crate) struct InputTaps {
    alive: Arc<AtomicBool>,
    run_loop: RunLoop,
}

struct RunLoop(CFRetained<CFRunLoop>);

// SAFETY: only `CFRunLoopStop` is called from other threads, and Core Foundation documents it as thread safe.
unsafe impl Send for RunLoop {}

impl Drop for InputTaps {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
        self.run_loop.0.stop();
    }
}

/// What the tap callback reaches through its `user_info` pointer.
struct TapState {
    tx: Sender<InputEvent>,
    ports: Vec<CFRetained<CFMachPort>>,
}

pub(crate) fn listen_input(pointer: bool, keys: bool) -> Result<(Receiver<InputEvent>, InputTaps)> {
    let keys = keys && {
        let granted = CGPreflightListenEventAccess();
        if !granted {
            tracing::warn!("keystroke overlay off: Input Monitoring is not granted");
        }
        granted
    };
    if !pointer && !keys {
        return Err(Error::permission(t("record.error.input_permission")));
    }
    let (tx, rx) = crossbeam_channel::bounded(4096);
    let alive = Arc::new(AtomicBool::new(true));
    let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<RunLoop>>(1);
    let flag = alive.clone();
    std::thread::Builder::new().name("snap-record-input".into()).spawn(move || run(pointer, keys, tx, &flag, &ready_tx))?;
    match ready_rx.recv() {
        Ok(Ok(run_loop)) => Ok((rx, InputTaps { alive, run_loop })),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(Error::Other(anyhow!("the input tap thread exited before it was ready"))),
    }
}

fn run(pointer: bool, keys: bool, tx: Sender<InputEvent>, alive: &AtomicBool, ready: &Sender<Result<RunLoop>>) {
    let Some(run_loop) = CFRunLoop::current() else {
        let _ = ready.send(Err(Error::Other(anyhow!("no run loop for the input taps"))));
        return;
    };
    // Every access to the state goes through this pointer; it is freed after the taps are gone.
    let state = Box::into_raw(Box::new(TapState { tx, ports: Vec::new() }));
    let wanted = [(pointer, mask(&POINTER_EVENTS), "pointer"), (keys, mask(&[CGEventType::KeyDown]), "key")];
    // SAFETY: the callback and its state pointer stay valid while the taps exist, and the taps only call back
    // from this thread's run loop, never while the state is changed below. The default mode is a CF constant.
    unsafe {
        let mode = kCFRunLoopDefaultMode;
        for (_, events, name) in wanted.into_iter().filter(|w| w.0) {
            let port = CGEvent::tap_create(
                CGEventTapLocation::SessionEventTap,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::ListenOnly,
                events,
                Some(callback),
                state.cast(),
            );
            let Some(port) = port else {
                tracing::warn!("the {name} event tap could not be created");
                continue;
            };
            let source = CFMachPort::new_run_loop_source(None, Some(&port), 0);
            run_loop.add_source(source.as_deref(), mode);
            CGEvent::tap_enable(&port, true);
            (*state).ports.push(port);
        }
        if (*state).ports.is_empty() {
            drop(Box::from_raw(state));
            let _ = ready.send(Err(Error::permission(t("record.error.input_permission"))));
            return;
        }
        let _ = ready.send(Ok(RunLoop(run_loop)));
        // `stop` from the subscription's drop ends a run early; the timeout covers a stop that lands before the
        // first run starts.
        while alive.load(Ordering::Relaxed) {
            CFRunLoop::run_in_mode(mode, 1.0, false);
        }
        for port in &(*state).ports {
            CGEvent::tap_enable(port, false);
            port.invalidate();
        }
        drop(Box::from_raw(state));
    }
}

unsafe extern "C-unwind" fn callback(_: CGEventTapProxy, kind: CGEventType, event: NonNull<CGEvent>, user: *mut c_void) -> *mut CGEvent {
    // SAFETY: `user` is the tap thread's state (see `run`).
    let state = unsafe { &*user.cast::<TapState>() };
    if kind == CGEventType::TapDisabledByTimeout || kind == CGEventType::TapDisabledByUserInput {
        // The system switches a slow tap off; a listen-only one never delays anything, so switch it back on.
        for port in &state.ports {
            CGEvent::tap_enable(port, true);
        }
    // SAFETY: a real event is valid for the duration of the call.
    } else if let Some(input) = convert(kind, unsafe { event.as_ref() }) {
        let _ = state.tx.try_send(input);
    }
    event.as_ptr()
}

fn convert(kind: CGEventType, ev: &CGEvent) -> Option<InputEvent> {
    // Global points, top-left origin: the space the overlay's input scale expects.
    let at = CGEvent::location(Some(ev));
    let down = |button| Some(InputEvent::ButtonDown { button, x: at.x, y: at.y });
    match kind {
        CGEventType::MouseMoved | CGEventType::LeftMouseDragged | CGEventType::RightMouseDragged | CGEventType::OtherMouseDragged => {
            Some(InputEvent::Move { x: at.x, y: at.y })
        }
        CGEventType::LeftMouseDown => down(Button::Left),
        CGEventType::RightMouseDown => down(Button::Right),
        CGEventType::OtherMouseDown => {
            down(if CGEvent::integer_value_field(Some(ev), CGEventField::MouseEventButtonNumber) == 2 { Button::Middle } else { Button::Other })
        }
        CGEventType::LeftMouseUp | CGEventType::RightMouseUp | CGEventType::OtherMouseUp => Some(InputEvent::ButtonUp),
        CGEventType::KeyDown => {
            if CGEvent::integer_value_field(Some(ev), CGEventField::KeyboardEventAutorepeat) != 0 {
                return None;
            }
            let flags = CGEvent::flags(Some(ev));
            let mods = Mods {
                ctrl: flags.contains(CGEventFlags::MaskControl),
                alt: flags.contains(CGEventFlags::MaskAlternate),
                shift: flags.contains(CGEventFlags::MaskShift),
                meta: flags.contains(CGEventFlags::MaskCommand),
            };
            let code = CGEvent::integer_value_field(Some(ev), CGEventField::KeyboardEventKeycode) as u16;
            Some(InputEvent::KeyDown { key: key_of(code), text: typed(ev), mods })
        }
        _ => None,
    }
}

/// The text the key produced with the current layout and modifiers, unless it is a control character.
fn typed(ev: &CGEvent) -> Option<String> {
    let mut buf = [0u16; 8];
    let mut len = 0;
    // SAFETY: the buffer holds `buf.len()` UTF-16 units and `len` receives how many were written.
    unsafe { CGEvent::keyboard_get_unicode_string(Some(ev), buf.len() as _, &mut len, buf.as_mut_ptr()) };
    let text = String::from_utf16_lossy(&buf[..(len as usize).min(buf.len())]);
    text.chars().next().is_some_and(|c| !c.is_control()).then_some(text)
}

/// A virtual key code (`kVK_*`) as a key. Letters and digits by position, as on a US layout.
fn key_of(code: u16) -> Key {
    let label = match code {
        0x33 => return Key::Backspace,
        0x7E => return Key::Up,
        0x7D => return Key::Down,
        0x7B => return Key::Left,
        0x7C => return Key::Right,
        0x24 | 0x4C => "Enter",
        0x35 => "Esc",
        0x75 => "Del",
        0x30 => "Tab",
        0x31 => "Space",
        0x39 => "Caps Lock",
        0x73 => "Home",
        0x77 => "End",
        0x74 => "Page Up",
        0x79 => "Page Down",
        0x72 => "Help",
        0x47 => "Clear",
        0x3F => "Fn",
        0x32 => "`",
        0x0A => "\u{a7}",
        0x1B | 0x4E => "-",
        0x18 | 0x51 => "=",
        0x21 => "[",
        0x1E => "]",
        0x29 => ";",
        0x27 => "'",
        0x2A => "\\",
        0x2B => ",",
        0x2F | 0x41 => ".",
        0x2C | 0x4B => "/",
        0x45 => "+",
        0x43 => "*",
        0x00 => "A",
        0x0B => "B",
        0x08 => "C",
        0x02 => "D",
        0x0E => "E",
        0x03 => "F",
        0x05 => "G",
        0x04 => "H",
        0x22 => "I",
        0x26 => "J",
        0x28 => "K",
        0x25 => "L",
        0x2E => "M",
        0x2D => "N",
        0x1F => "O",
        0x23 => "P",
        0x0C => "Q",
        0x0F => "R",
        0x01 => "S",
        0x11 => "T",
        0x20 => "U",
        0x09 => "V",
        0x0D => "W",
        0x07 => "X",
        0x10 => "Y",
        0x06 => "Z",
        0x1D | 0x52 => "0",
        0x12 | 0x53 => "1",
        0x13 | 0x54 => "2",
        0x14 | 0x55 => "3",
        0x15 | 0x56 => "4",
        0x17 | 0x57 => "5",
        0x16 | 0x58 => "6",
        0x1A | 0x59 => "7",
        0x1C | 0x5B => "8",
        0x19 | 0x5C => "9",
        0x7A => "F1",
        0x78 => "F2",
        0x63 => "F3",
        0x76 => "F4",
        0x60 => "F5",
        0x61 => "F6",
        0x62 => "F7",
        0x64 => "F8",
        0x65 => "F9",
        0x6D => "F10",
        0x67 => "F11",
        0x6F => "F12",
        0x69 => "F13",
        0x6B => "F14",
        0x71 => "F15",
        0x6A => "F16",
        0x40 => "F17",
        0x4F => "F18",
        0x50 => "F19",
        0x5A => "F20",
        _ => "?",
    };
    Key::Named(label.into())
}
