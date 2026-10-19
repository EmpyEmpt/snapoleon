//! Cover windows that stay open, hidden, between overlay sessions.
//!
//! Creating a borderless window and its GPU surface and drawing its first frame takes longer than the
//! 100 ms the overlay has to appear in. So the covers are opened once, at start-up (or by the first session),
//! and kept: a session only freezes the screen, hands each cover its monitor's snapshot (a [`Seat`]) and asks the
//! shell to present it. When the session ends the covers are hidden again and release the snapshot.
//!
//! The pool follows the monitor layout: a session whose monitors differ from the pooled covers' rects (a monitor
//! was plugged in, the resolution changed) replaces them. Without a pool (the dev harness, which must end with its
//! last window) covers are closed with the session, as they would be without any of this.
//!
//! Everything here runs on the UI thread.

use std::{cell::RefCell, rc::Rc};

use snap_core::{Rect as PxRect, t};
use snap_shell::{WindowKey, WindowSpec};

use super::{OverlayView, TAG, session::Shared};
use crate::views::OpenCx;

/// What a cover needs from the running session: the session and which of its monitors this cover is over.
#[derive(Clone)]
pub struct Seat {
    pub session: Shared,
    pub monitor: usize,
}

/// A cover's link to the session it currently serves; `None` while it rests.
pub type Slot = Rc<RefCell<Option<Seat>>>;

#[derive(Clone)]
pub struct Cover {
    pub key: WindowKey,
    pub rect: PxRect,
    pub slot: Slot,
}

#[derive(Default)]
struct Pool {
    /// Covers survive their session (hidden) instead of being closed.
    enabled: bool,
    covers: Vec<Cover>,
    /// A session is running; one overlay at a time.
    active: bool,
}

thread_local! {
    static POOL: RefCell<Pool> = RefCell::new(Pool::default());
}

/// Keep covers between sessions from now on. Called once by the app when it runs as the tray app.
pub fn enable() {
    POOL.with_borrow_mut(|pool| pool.enabled = true);
}

pub fn enabled() -> bool {
    POOL.with_borrow(|pool| pool.enabled)
}

/// Whether an overlay session is running.
pub fn active() -> bool {
    POOL.with_borrow(|pool| pool.active)
}

pub fn set_active(active: bool) {
    POOL.with_borrow_mut(|pool| pool.active = active);
}

/// The covers over `rects`, in that order: the pooled ones when they match, new ones otherwise. New covers
/// start visible when `visible` (a session is about to use them) and join the pool when it is enabled.
pub fn covers(cx: &mut OpenCx, rects: &[PxRect], visible: bool) -> Vec<Cover> {
    if let Some(found) = reuse(rects) {
        return found;
    }
    for stale in POOL.with_borrow_mut(|pool| std::mem::take(&mut pool.covers)) {
        cx.shell.close(stale.key);
    }
    let fresh: Vec<Cover> = rects
        .iter()
        .map(|&rect| {
            let slot = Slot::default();
            let spec = WindowSpec {
                title: t("overlay.title").into_owned(),
                tag: TAG.into(),
                exclude_from_capture: true,
                visible,
                ..WindowSpec::cover(rect)
            };
            let key = cx.open(spec, OverlayView::new(slot.clone()));
            Cover { key, rect, slot }
        })
        .collect();
    if enabled() {
        POOL.with_borrow_mut(|pool| pool.covers = fresh.clone());
    }
    fresh
}

/// The pooled covers over exactly these monitor rects, in this order.
fn reuse(rects: &[PxRect]) -> Option<Vec<Cover>> {
    POOL.with_borrow(|pool| {
        if pool.covers.len() != rects.len() {
            return None;
        }
        rects.iter().map(|r| pool.covers.iter().find(|c| c.rect == *r).cloned()).collect()
    })
}

/// Whether [`covers`] would reuse windows that are already open and hidden.
pub fn ready_for(rects: &[PxRect]) -> bool {
    reuse(rects).is_some()
}

/// The session is over: every cover lets go of it.
pub fn release(session: &Shared) {
    let slots: Vec<Slot> = POOL.with_borrow(|pool| pool.covers.iter().map(|c| c.slot.clone()).collect());
    for slot in slots {
        let mine = slot.borrow().as_ref().is_some_and(|seat| Rc::ptr_eq(&seat.session, session));
        if mine {
            slot.borrow_mut().take();
        }
    }
    set_active(false);
}

/// A cover window is gone (closed by the OS or replaced): it leaves the pool.
pub fn forget(key: WindowKey) {
    POOL.with_borrow_mut(|pool| pool.covers.retain(|c| c.key != key));
}
