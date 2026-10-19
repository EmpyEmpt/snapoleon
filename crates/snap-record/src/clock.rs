//! Recording clock that stops counting while paused, so output has no gaps.

use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// 100 ns units, the timestamp resolution used by Media Foundation and our sinks.
pub(crate) type Hns = i64;

pub(crate) const HNS_PER_SEC: i64 = 10_000_000;

pub(crate) fn hns_from(d: Duration) -> Hns {
    (d.as_nanos() / 100) as Hns
}

struct State {
    started: Instant,
    paused_total: Duration,
    paused_at: Option<Instant>,
}

pub(crate) struct PauseClock {
    state: Mutex<State>,
}

impl PauseClock {
    pub fn new() -> Self {
        Self { state: Mutex::new(State { started: Instant::now(), paused_total: Duration::ZERO, paused_at: None }) }
    }

    /// Start counting from zero again (the recorder calls this once all devices are open).
    pub fn restart(&self) {
        *self.state.lock() = State { started: Instant::now(), paused_total: Duration::ZERO, paused_at: None };
    }

    /// Active (un-paused) time since the clock started.
    pub fn elapsed(&self) -> Duration {
        let s = self.state.lock();
        let end = s.paused_at.unwrap_or_else(Instant::now);
        end.saturating_duration_since(s.started).saturating_sub(s.paused_total)
    }

    pub fn pause(&self) {
        let mut s = self.state.lock();
        if s.paused_at.is_none() {
            s.paused_at = Some(Instant::now());
        }
    }

    pub fn resume(&self) {
        let mut s = self.state.lock();
        if let Some(at) = s.paused_at.take() {
            s.paused_total += at.elapsed();
        }
    }

    pub fn is_paused(&self) -> bool {
        self.state.lock().paused_at.is_some()
    }
}
