use std::time::Instant;

use winit::event_loop::EventLoopProxy;

use crate::WindowKey;

/// Everything the event loop can be woken with.
pub(crate) enum Msg<E> {
    User(E),
    Repaint { key: WindowKey, at: Instant },
    RepaintAll,
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

    pub(crate) fn repaint_at(&self, key: WindowKey, at: Instant) {
        let _ = self.inner.send_event(Msg::Repaint { key, at });
    }
}
