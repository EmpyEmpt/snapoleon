use std::time::Instant;

use egui_winit::accesskit_winit;
use winit::event_loop::EventLoopProxy;

use crate::{OsRequest, WindowKey, window::EditAction};

/// Everything the event loop can be woken with.
pub(crate) enum Msg<E> {
    User(E),
    Repaint {
        key: WindowKey,
        at: Instant,
    },
    RepaintAll,
    Hook(AppHook),
    /// A screen reader or another accessibility client asked a window for its tree, acted on it, or went away.
    AccessKit(accesskit_winit::Event),
}

impl<E> From<accesskit_winit::Event> for Msg<E> {
    fn from(event: accesskit_winit::Event) -> Self {
        Msg::AccessKit(event)
    }
}

/// What the OS integration (the macOS app delegate and app menu) hands the loop.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) enum AppHook {
    Request(OsRequest),
    /// An Edit menu command for the window with the keyboard focus.
    Edit(EditAction),
}

/// Thread-safe handle to the running shell; wakes the loop from any thread.
pub struct Proxy<E: Send + 'static> {
    inner: EventLoopProxy<Msg<E>>,
}

impl<E: Send + 'static> Clone for Proxy<E> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl<E: Send + 'static> Proxy<E> {
    pub(crate) fn new(inner: EventLoopProxy<Msg<E>>) -> Self {
        Self { inner }
    }

    /// Deliver a user event to [`ShellApp::event`](crate::ShellApp::event).
    /// Silently dropped once the event loop has exited.
    pub fn send(&self, e: E) {
        let _ = self.inner.send_event(Msg::User(e));
    }

    /// Repaint every window.
    pub fn repaint_all(&self) {
        let _ = self.inner.send_event(Msg::RepaintAll);
    }

    /// The loop's own proxy, for the AccessKit adapters.
    pub(crate) fn event_loop(&self) -> EventLoopProxy<Msg<E>> {
        self.inner.clone()
    }

    pub(crate) fn repaint_at(&self, key: WindowKey, at: Instant) {
        let _ = self.inner.send_event(Msg::Repaint { key, at });
    }
}
