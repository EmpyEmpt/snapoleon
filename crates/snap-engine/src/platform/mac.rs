//! macOS: a listen-only `CGEventTap` on its own run-loop thread. Needs the Input Monitoring (or Accessibility) permission.

use std::thread::JoinHandle;

use core_foundation::{
    base::{CFType, TCFType},
    dictionary::{CFDictionary, CFDictionaryRef},
    number::CFNumber,
    runloop::{CFRunLoop, kCFRunLoopCommonModes},
    string::CFString,
};
use core_graphics::{
    event::{CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType, CallbackResult},
    geometry::{CGPoint, CGRect},
    window::{
        copy_window_info, kCGNullWindowID, kCGWindowAlpha, kCGWindowBounds, kCGWindowLayer, kCGWindowListOptionOnScreenOnly,
        kCGWindowOwnerPID,
    },
};
use snap_core::{Error, Point, Result, geom::PointLayout, t};

pub(crate) struct Hook {
    run_loop: CFRunLoop,
    join: Option<JoinHandle<()>>,
}

impl Hook {
    pub(crate) fn start(on_click: Box<dyn Fn(Point) + Send>) -> Result<Hook> {
        let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<CFRunLoop>>(1);
        let join = std::thread::Builder::new().name("snap-click-tap".into()).spawn(move || {
            let tap = CGEventTap::new(
                CGEventTapLocation::Session,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::ListenOnly,
                vec![CGEventType::LeftMouseDown],
                move |_proxy, _kind, event| {
                    let at = event.location();
                    on_click(to_pixels(at.x, at.y));
                    CallbackResult::Keep
                },
            );
            let source = tap.as_ref().ok().and_then(|tap| tap.mach_port().create_runloop_source(0).ok());
            let (Ok(tap), Some(source)) = (tap, source) else {
                let _ = ready_tx.send(Err(Error::permission(t("engine.error.input_permission").into_owned())));
                return;
            };
            let run_loop = CFRunLoop::get_current();
            // SAFETY: kCFRunLoopCommonModes is a constant CoreFoundation string.
            run_loop.add_source(&source, unsafe { kCFRunLoopCommonModes });
            tap.enable();
            let _ = ready_tx.send(Ok(run_loop));
            CFRunLoop::run_current();
        })?;
        match ready_rx.recv() {
            Ok(Ok(run_loop)) => Ok(Hook { run_loop, join: Some(join) }),
            Ok(Err(e)) => {
                let _ = join.join();
                Err(e)
            }
            Err(_) => Err(Error::Other(anyhow::anyhow!("the event tap thread exited before it was ready"))),
        }
    }
}

impl Drop for Hook {
    fn drop(&mut self) {
        self.run_loop.stop();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Event locations are in points; the capture crate works in virtual-desktop pixels.
fn to_pixels(x: f64, y: f64) -> Point {
    PointLayout::current().to_pixels(x, y)
}

/// One pass over the window list, front to back: the first visible window under `p` decides. The menu bar and status
/// items belong to other processes, so clicks on them count.
pub(crate) fn is_own_window_at(p: Point) -> bool {
    let (x, y) = PointLayout::current().to_points(p);
    let at = CGPoint::new(x, y);
    let Some(list) = copy_window_info(kCGWindowListOptionOnScreenOnly, kCGNullWindowID) else { return false };
    // SAFETY: the keys are constant CoreGraphics strings.
    let (pid_key, layer_key, bounds_key, alpha_key) = unsafe {
        (
            CFString::wrap_under_get_rule(kCGWindowOwnerPID),
            CFString::wrap_under_get_rule(kCGWindowLayer),
            CFString::wrap_under_get_rule(kCGWindowBounds),
            CFString::wrap_under_get_rule(kCGWindowAlpha),
        )
    };
    let owner = list.iter().find_map(|item| {
        // SAFETY: every entry of the window list is a dictionary keyed by strings; the list keeps it alive.
        let w: CFDictionary<CFString, CFType> = unsafe { CFDictionary::wrap_under_get_rule(*item as CFDictionaryRef) };
        let number = |key: &CFString| w.find(key).and_then(|v| v.downcast::<CFNumber>());
        let visible = number(&layer_key).and_then(|n| n.to_i64()).is_some_and(|l| l >= 0)
            && number(&alpha_key).and_then(|n| n.to_f64()).is_none_or(|a| a > 0.0);
        let bounds = w.find(&bounds_key).and_then(|v| v.downcast::<CFDictionary>()).and_then(|d| CGRect::from_dict_representation(&d));
        (visible && bounds.is_some_and(|b| b.contains(&at))).then(|| number(&pid_key).and_then(|n| n.to_i64()))
    });
    owner.flatten() == Some(i64::from(std::process::id()))
}
