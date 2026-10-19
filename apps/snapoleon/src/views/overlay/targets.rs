//! What the pointer can snap to: the window under it (known from the frozen snapshot) and the UI
//! elements under it (asked from the accessibility API on a worker thread, because a query can take
//! a hundred milliseconds and the overlay must never wait for it).

use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use egui::Context;
use snap_capture::{ElementInfo, Frozen, role_label};
use snap_core::{CaptureKind, Point, Rect as PxRect};

use super::ViewCx;

/// Minimum time between two accessibility queries.
const QUERY_GAP: Duration = Duration::from_millis(40);
/// The pointer must move this far (physical pixels) before a new query is worth asking.
const QUERY_MOVE: i32 = 2;

/// Something a click can select.
#[derive(Clone, Debug)]
pub struct Target {
    pub rect: PxRect,
    pub label: String,
    pub kind: CaptureKind,
}

/// The answer of one accessibility query, delivered to a window's `on_msg`.
pub struct ElementsReply {
    chain: Vec<ElementInfo>,
}

#[derive(Default)]
pub struct Targets {
    /// Elements under the last queried point, smallest first.
    chain: Vec<ElementInfo>,
    /// Which of the elements containing the pointer is selected (0 = smallest). The wheel moves it.
    level: usize,
    leaf: Option<PxRect>,
    asked: Option<(Instant, Point)>,
    worker: Option<Sender<Point>>,
    wheel: f32,
}

impl Targets {
    /// The front-most window at `at`.
    pub fn window(frozen: &Frozen, at: Point) -> Option<Target> {
        let w = frozen.window_at(at)?;
        let label = if w.title.is_empty() { w.app_name.clone() } else { w.title.clone() };
        Some(Target { rect: w.rect, label, kind: CaptureKind::Window })
    }

    /// The selected element at `at`, from the last query.
    pub fn element(&self, at: Point) -> Option<Target> {
        let mut around = self.chain.iter().filter(|e| e.rect.contains(at));
        let count = around.clone().count();
        let e = around.nth(self.level.min(count.checked_sub(1)?))?;
        let role = role_label(&e.role);
        let label = if e.name.is_empty() { role } else { format!("{role} · {}", e.name) };
        Some(Target { rect: e.rect, label, kind: CaptureKind::Element })
    }

    /// Ask for the elements at `at` unless a recent query already covers it. Repaints later when the
    /// throttle swallowed a request, so the pointer coming to rest is always answered.
    pub fn request(&mut self, cx: &ViewCx, ctx: &Context, at: Point) {
        if let Some((when, last)) = self.asked {
            if (last.x - at.x).abs().max((last.y - at.y).abs()) < QUERY_MOVE {
                return;
            }
            if when.elapsed() < QUERY_GAP {
                ctx.request_repaint_after(QUERY_GAP);
                return;
            }
        }
        let worker = self.worker.get_or_insert_with(|| spawn_worker(cx));
        if worker.send(at).is_ok() {
            self.asked = Some((Instant::now(), at));
        }
    }

    pub fn apply(&mut self, reply: ElementsReply) {
        let leaf = reply.chain.first().map(|e| e.rect);
        if leaf != self.leaf {
            self.level = 0;
        }
        self.leaf = leaf;
        self.chain = reply.chain;
    }

    /// Mouse wheel over the overlay: up selects the parent element, down goes back towards the child.
    pub fn scroll(&mut self, delta: f32) {
        self.wheel += delta;
        while self.wheel >= 1.0 {
            self.wheel -= 1.0;
            self.level = (self.level + 1).min(self.chain.len().saturating_sub(1));
        }
        while self.wheel <= -1.0 {
            self.wheel += 1.0;
            self.level = self.level.saturating_sub(1);
        }
    }
}

/// One thread that answers queries in order, skipping to the newest when several piled up.
fn spawn_worker(cx: &ViewCx) -> Sender<Point> {
    let (tx, rx) = crossbeam_channel::unbounded::<Point>();
    let handle = cx.handle();
    let spawned = std::thread::Builder::new().name("snap-overlay-elements".into()).spawn(move || {
        while let Ok(mut at) = rx.recv() {
            while let Ok(newer) = rx.try_recv() {
                at = newer;
            }
            let chain = snap_capture::elements_at(at).unwrap_or_default();
            handle.send(ElementsReply { chain });
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("cannot start the element query thread: {e}");
    }
    tx
}
