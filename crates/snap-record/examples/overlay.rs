//! Smoke test for the input overlays: opens its own window, records it, and injects
//! mouse clicks and keystrokes into it, so the clicks/keys never reach other apps.
//! Keys are only sent while this window really is the foreground window.
//!
//! `cargo run -p snap-record --example overlay` (Windows only).

#[cfg(windows)]
fn main() -> snap_core::Result<()> {
    use std::{ptr::null, time::Duration};

    use rdev::{Button, EventType, Key, simulate};
    use snap_core::settings::{EncoderPref, VideoFormat};
    use snap_record::{RecordOptions, RecordTarget};
    use windows::{
        Win32::{
            Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
            System::{
                LibraryLoader::GetModuleHandleW,
                Threading::{AttachThreadInput, GetCurrentThreadId},
            },
            UI::{
                Input::KeyboardAndMouse::SetFocus,
                WindowsAndMessaging::{
                    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetForegroundWindow, GetMessageW, GetWindowRect,
                    GetWindowThreadProcessId, MSG, PostMessageW, RegisterClassW, SW_SHOW, SetForegroundWindow, ShowWindow,
                    TranslateMessage, WM_CLOSE, WM_DESTROY, WNDCLASSW, WS_EX_TOPMOST, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
                },
            },
        },
        core::w,
    };

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        if msg == WM_DESTROY {
            // SAFETY: ends this thread's message loop.
            unsafe { windows::Win32::UI::WindowsAndMessaging::PostQuitMessage(0) };
            return LRESULT(0);
        }
        // SAFETY: default handling for every other message.
        unsafe { DefWindowProcW(hwnd, msg, w, l) }
    }

    // SAFETY: first call in the process, before any window exists.
    unsafe {
        use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    tracing_subscriber::fmt().with_env_filter("snap_record=debug").init();
    snap_record::set_ffmpeg_enabled(false);

    // SAFETY: standard window class + window creation on this thread.
    let hwnd = unsafe {
        let hinst = GetModuleHandleW(None).map_err(|e| snap_core::Error::Other(e.into()))?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: hinst.into(),
            lpszClassName: w!("SnapRecordDemo"),
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(6usize as *mut _), // COLOR_WINDOW + 1
            ..Default::default()
        };
        RegisterClassW(&class);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST,
            w!("SnapRecordDemo"),
            w!("snap-record overlay demo"),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            300,
            200,
            900,
            600,
            None,
            None,
            Some(hinst.into()),
            Some(null()),
        )
        .map_err(|e| snap_core::Error::Other(e.into()))?;
        let _ = ShowWindow(hwnd, SW_SHOW);
        hwnd
    };
    let raw = hwnd.0 as usize;

    let worker = std::thread::spawn(move || -> snap_core::Result<()> {
        let hwnd = HWND(raw as *mut _);
        std::thread::sleep(Duration::from_millis(500));
        // Take the foreground (attach to the current foreground thread's input queue first).
        // SAFETY: documented Win32 calls on valid handles.
        let focused = unsafe {
            let fg = GetForegroundWindow();
            let fg_thread = GetWindowThreadProcessId(fg, None);
            let me = GetCurrentThreadId();
            let _ = AttachThreadInput(me, fg_thread, true);
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(Some(hwnd));
            let _ = AttachThreadInput(me, fg_thread, false);
            GetForegroundWindow() == hwnd
        };
        println!("window is foreground: {focused}");
        let mut rect = RECT::default();
        // SAFETY: valid window handle, out-parameter.
        unsafe { GetWindowRect(hwnd, &mut rect) }.map_err(|e| snap_core::Error::Other(e.into()))?;
        let (cx, cy) = ((rect.left + rect.right) as f64 / 2.0, (rect.top + rect.bottom) as f64 / 2.0);

        let out = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/smoke/snap-record/overlay.mp4"));
        let rec = snap_record::start(RecordOptions {
            target: RecordTarget::Window(raw as u64),
            format: VideoFormat::Mp4,
            fps: 30,
            bitrate: 0,
            system_audio: false,
            microphone: false,
            mic_device: None,
            show_cursor: true,
            highlight_clicks: true,
            show_keystrokes: true,
            encoder: EncoderPref::Auto,
            max_gif_width: 800,
            output: out.clone(),
            ffmpeg: None,
            exclude_windows: Vec::new(),
            hide_desktop_icons: false,
        })?;

        let pause = |ms| std::thread::sleep(Duration::from_millis(ms));
        let click = |x: f64, y: f64, button: Button| {
            let _ = simulate(&EventType::MouseMove { x, y });
            pause(120);
            let _ = simulate(&EventType::ButtonPress(button));
            pause(80);
            let _ = simulate(&EventType::ButtonRelease(button));
            pause(250);
        };
        click(cx - 200.0, cy - 100.0, Button::Left);
        click(cx + 150.0, cy + 50.0, Button::Right);
        if focused {
            for key in [Key::KeyH, Key::KeyE, Key::KeyL, Key::KeyL, Key::KeyO] {
                let _ = simulate(&EventType::KeyPress(key));
                let _ = simulate(&EventType::KeyRelease(key));
                pause(90);
            }
            let _ = simulate(&EventType::KeyPress(Key::ControlLeft));
            let _ = simulate(&EventType::KeyPress(Key::ShiftLeft));
            let _ = simulate(&EventType::KeyPress(Key::KeyS));
            let _ = simulate(&EventType::KeyRelease(Key::KeyS));
            let _ = simulate(&EventType::KeyRelease(Key::ShiftLeft));
            let _ = simulate(&EventType::KeyRelease(Key::ControlLeft));
            pause(300);
            let _ = simulate(&EventType::KeyPress(Key::Return));
            let _ = simulate(&EventType::KeyRelease(Key::Return));
        }
        pause(900);
        let r = rec.stop()?;
        println!("recorded {} ms, {}x{}, {} bytes -> {}", r.duration_ms, r.width, r.height, r.bytes, r.path.display());
        // SAFETY: closes our own window, ending the message loop.
        unsafe {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        Ok(())
    });

    let mut msg = MSG::default();
    // SAFETY: standard message loop for the window created above.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    worker.join().expect("scenario thread panicked")
}

#[cfg(not(windows))]
fn main() {
    println!("this smoke test drives a Win32 window and only runs on Windows");
}
