//! Per-thread COM initialisation (MTA).

use std::cell::RefCell;

use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

struct ComGuard(bool);

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

thread_local! {
    static COM: RefCell<Option<ComGuard>> = const { RefCell::new(None) };
}

/// Join the MTA on this thread. If the thread is already an STA the call fails harmlessly and the
/// existing apartment is used (everything we call is free-threaded or agile).
pub fn ensure() {
    COM.with(|c| {
        c.borrow_mut().get_or_insert_with(|| ComGuard(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok()));
    });
}
