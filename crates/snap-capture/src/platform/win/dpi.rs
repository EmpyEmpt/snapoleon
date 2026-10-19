use std::sync::Once;

use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};

/// Request per-monitor-v2 awareness once. "Access denied" (already aware, e.g. via manifest) is fine.
pub fn ensure() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    });
}
