//! macOS: a listen-only `CGEventTap` on its own run-loop thread. Needs the Input Monitoring (or Accessibility) permission.

use std::thread::JoinHandle;

use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes};
use core_graphics::event::{CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType, CallbackResult};
use snap_core::{Error, Point, Result, t};

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

/// Event locations are in points; the capture crate works in physical pixels, scaled per monitor.
fn to_pixels(x: f64, y: f64) -> Point {
    let monitors = snap_capture::monitors().unwrap_or_default();
    monitors
        .iter()
        .find_map(|m| {
            let s = f64::from(m.scale_factor);
            let (left, top) = (f64::from(m.rect.x) / s, f64::from(m.rect.y) / s);
            let inside = x >= left && y >= top && x < left + f64::from(m.rect.w) / s && y < top + f64::from(m.rect.h) / s;
            inside.then(|| Point::new((x * s).round() as i32, (y * s).round() as i32))
        })
        .unwrap_or_else(|| Point::new(x.round() as i32, y.round() as i32))
}

pub(crate) fn is_own_window_at(_p: Point) -> bool {
    false
}
