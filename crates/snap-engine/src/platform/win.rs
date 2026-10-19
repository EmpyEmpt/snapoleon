//! Windows: a low-level mouse hook (`WH_MOUSE_LL`) on a dedicated thread with its own message loop.

use std::{cell::RefCell, panic::AssertUnwindSafe, thread::JoinHandle};

use snap_core::{Error, Point, Result};
use windows::Win32::{
    Foundation::{LPARAM, LRESULT, POINT, WPARAM},
    System::Threading::GetCurrentThreadId,
    UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, GetWindowThreadProcessId, MSG, MSLLHOOKSTRUCT, PostThreadMessageW,
        SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_QUIT, WindowFromPoint,
    },
};

type Handler = Box<dyn Fn(Point)>;

thread_local! {
    /// The hook procedure runs on the thread that installed the hook, so the handler can live in thread-local storage.
    static HANDLER: RefCell<Option<Handler>> = const { RefCell::new(None) };
}

pub(crate) struct Hook {
    thread_id: u32,
    join: Option<JoinHandle<()>>,
}

impl Hook {
    pub(crate) fn start(on_click: Box<dyn Fn(Point) + Send>) -> Result<Hook> {
        let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<u32>>(1);
        let join = std::thread::Builder::new().name("snap-click-hook".into()).spawn(move || run_hook(on_click, ready_tx))?;
        match ready_rx.recv() {
            Ok(Ok(thread_id)) => Ok(Hook { thread_id, join: Some(join) }),
            Ok(Err(e)) => {
                let _ = join.join();
                Err(e)
            }
            Err(_) => Err(Error::Other(anyhow::anyhow!("the mouse hook thread exited before it was ready"))),
        }
    }
}

impl Drop for Hook {
    fn drop(&mut self) {
        // Ends the message loop; the thread then removes the hook itself.
        // SAFETY: plain thread-message post to a thread id we own.
        let _ = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn run_hook(on_click: Box<dyn Fn(Point) + Send>, ready: crossbeam_channel::Sender<Result<u32>>) {
    HANDLER.with(|h| *h.borrow_mut() = Some(on_click));
    // SAFETY: the procedure below only touches thread-local state and forwards to the next hook.
    let hook = match unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(hook_proc), None, 0) } {
        Ok(hook) => hook,
        Err(e) => {
            let _ = ready.send(Err(Error::Other(anyhow::anyhow!("cannot install the mouse hook: {e}"))));
            return;
        }
    };
    // SAFETY: trivial getter.
    let _ = ready.send(Ok(unsafe { GetCurrentThreadId() }));
    let mut msg = MSG::default();
    // SAFETY: standard message loop; GetMessageW returns false on WM_QUIT.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = UnhookWindowsHookEx(hook);
    }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && wparam.0 as u32 == WM_LBUTTONDOWN {
        // SAFETY: for WH_MOUSE_LL, lparam points at a MSLLHOOKSTRUCT valid for the duration of the call.
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        let at = Point::new(info.pt.x, info.pt.y);
        // A panic must not unwind across the FFI boundary.
        let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
            HANDLER.with(|h| {
                if let Some(handler) = h.borrow().as_ref() {
                    handler(at);
                }
            })
        }));
    }
    // SAFETY: forwarding the unchanged arguments.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

pub(crate) fn is_own_window_at(p: Point) -> bool {
    // SAFETY: plain queries with valid arguments.
    unsafe {
        let hwnd = WindowFromPoint(POINT { x: p.x, y: p.y });
        if hwnd.0.is_null() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        pid == std::process::id()
    }
}
