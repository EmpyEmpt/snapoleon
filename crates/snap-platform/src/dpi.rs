//! Process DPI awareness.

/// Make the process per-monitor-v2 DPI aware on Windows so every coordinate in
/// the app is in physical pixels. Call before creating any window. Harmless if
/// the manifest already set it; no-op on macOS and Linux.
pub fn init() {
    #[cfg(windows)]
    {
        use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
        // SAFETY: no pointers involved. Fails with "access denied" when awareness was already set.
        if let Err(e) = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) } {
            tracing::debug!("DPI awareness already set: {e}");
        }
    }
}
