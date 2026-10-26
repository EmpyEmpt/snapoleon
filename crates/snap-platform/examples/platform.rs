//! Smoke test for snap-platform. Run on the real desktop:
//!
//! ```text
//! cargo run -p snap-platform --example platform [-- --scheme --reveal]
//! ```
//!
//! `--scheme` registers the URL scheme / `.snap` association (then removes them
//! again), `--reveal` opens Explorer/Finder on the output folder. Output files go
//! to `target/smoke/snap-platform/`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use image::{Rgba, RgbaImage};
use snap_core::hotkey::{Action, HotkeyBinding};
use snap_core::ipc::{Request, Response};
use snap_platform::{autostart, clipboard, console, dpi, hotkeys, ipc, notify, open, permissions, policy, scheme, secrets, sound, tray};

fn out_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/smoke/snap-platform");
    std::fs::create_dir_all(&dir).expect("create smoke dir");
    std::path::absolute(dir).expect("absolute path")
}

fn test_image() -> RgbaImage {
    RgbaImage::from_fn(96, 64, |x, y| {
        let a = if (x / 8 + y / 8) % 2 == 0 { 255 } else { 140 };
        Rgba([(x * 255 / 96) as u8, (y * 255 / 64) as u8, 200, a])
    })
}

fn step(name: &str, result: Result<String, String>) {
    match result {
        Ok(detail) => println!("[ ok ] {name}: {detail}"),
        Err(e) => println!("[FAIL] {name}: {e}"),
    }
}

/// Pump the thread's Win32 message queue (what the winit main thread does in the app).
fn pump(duration: Duration) {
    let end = Instant::now() + duration;
    #[cfg(windows)]
    {
        use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage};
        while Instant::now() < end {
            let mut msg = MSG::default();
            // SAFETY: standard message loop on the current thread.
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    #[cfg(not(windows))]
    std::thread::sleep(end.saturating_duration_since(Instant::now()));
}

/// Press and release a chord with SendInput (Windows only).
#[cfg(windows)]
fn press_chord(keys: &[u16]) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY,
    };
    let make = |vk: u16, flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    };
    let mut inputs: Vec<INPUT> = keys.iter().map(|k| make(*k, KEYBD_EVENT_FLAGS(0))).collect();
    inputs.extend(keys.iter().rev().map(|k| make(*k, KEYEVENTF_KEYUP)));
    // SAFETY: `inputs` is a valid array of INPUT structs.
    unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let with_scheme = args.iter().any(|a| a == "--scheme");
    let with_reveal = args.iter().any(|a| a == "--reveal");
    let dir = out_dir();

    dpi::init();
    console::attach();
    #[cfg(windows)]
    {
        use windows::Win32::UI::HiDpi::{
            AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetThreadDpiAwarenessContext,
        };
        // SAFETY: plain queries, no pointers.
        let per_monitor_v2 =
            unsafe { AreDpiAwarenessContextsEqual(GetThreadDpiAwarenessContext(), DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).as_bool() };
        step("dpi", if per_monitor_v2 { Ok("per-monitor v2".into()) } else { Err("not per-monitor v2".into()) });
    }
    #[cfg(feature = "egui")]
    {
        let m = egui::Modifiers { ctrl: true, shift: true, ..egui::Modifiers::NONE };
        step(
            "from_egui",
            Ok(format!("{:?} / {:?}", hotkeys::from_egui(m, egui::Key::Num4), hotkeys::from_egui(egui::Modifiers::NONE, egui::Key::F9))),
        );
    }

    // hotkey parsing / display, in English and Russian
    for accel in ["Ctrl+Shift+4", "Cmd+Shift+2", "CmdOrCtrl+Alt+PrtSc", "Win+Num5", "F13", "PrintScreen"] {
        step(&format!("hotkey parse {accel:?}"), hotkeys::validate(accel).map(|_| hotkeys::display(accel)).map_err(|e| e.to_string()));
    }
    for lang in ["en", "ru"] {
        snap_core::i18n::set_language(lang);
        for bad in ["Ctrl+Foo", "A", "Ctrl+Ctrl+A", "Ctrl+", "Ctrl+A+B"] {
            step(
                &format!("hotkey reject {bad:?} ({lang})"),
                match hotkeys::validate(bad) {
                    Err(e) => Ok(e.to_string()),
                    Ok(()) => Err("accepted".into()),
                },
            );
        }
    }
    snap_core::i18n::set_language("en");

    // hotkeys: register, then fire it with a synthetic key press
    let fired = Arc::new(AtomicUsize::new(0));
    let hotkey_ok = {
        let cb_fired = fired.clone();
        match hotkeys::HotkeyManager::new(move |ev| {
            if let hotkeys::HotkeyEvent::Pressed(i) = ev {
                cb_fired.store(i + 1, Ordering::SeqCst);
            }
        }) {
            Err(e) => {
                step("hotkey manager", Err(e.to_string()));
                None
            }
            Ok(mut mgr) => {
                let bindings = vec![
                    HotkeyBinding { accel: "Ctrl+Alt+Shift+F11".into(), action: Action::CaptureRegion, pipeline: None, enabled: true },
                    HotkeyBinding { accel: "Ctrl+Alt+Shift+F11".into(), action: Action::CaptureWindow, pipeline: None, enabled: true },
                    HotkeyBinding { accel: "Ctrl+Bogus".into(), action: Action::OpenLibrary, pipeline: None, enabled: true },
                ];
                for (i, r) in mgr.register(&bindings) {
                    step(
                        &format!("hotkey register #{i} ({})", bindings[i].accel),
                        r.map(|_| "registered".into()).map_err(|e| e.to_string()),
                    );
                }
                #[cfg(windows)]
                {
                    press_chord(&[0x11, 0x12, 0x10, 0x7A]); // Ctrl+Alt+Shift+F11
                    pump(Duration::from_millis(600));
                    let got = fired.load(Ordering::SeqCst);
                    step("hotkey event", if got == 1 { Ok("Pressed(0) delivered".into()) } else { Err(format!("got {got}")) });
                    fired.store(0, Ordering::SeqCst);
                    mgr.pause(true);
                    press_chord(&[0x11, 0x12, 0x10, 0x7A]);
                    pump(Duration::from_millis(400));
                    let got = fired.load(Ordering::SeqCst);
                    step("hotkey paused", if got == 0 { Ok("no event while paused".into()) } else { Err(format!("got {got}")) });
                    mgr.pause(false);
                }
                Some(mgr)
            }
        }
    };

    // tray (needs the message pump of this thread)
    let clicks = Arc::new(AtomicUsize::new(0));
    let tray_result = {
        let clicks = clicks.clone();
        let items = vec![
            tray::TrayItem::new("capture", "Capture region"),
            tray::TrayItem::new("rec", "Recording").checked(true),
            tray::TrayItem::separator(),
            tray::TrayItem::submenu(
                "More & settings",
                vec![
                    tray::TrayItem::new("lib", "Library"),
                    tray::TrayItem::new("off", "Disabled item").disabled(),
                    tray::TrayItem::submenu("Deep", vec![tray::TrayItem::new("deep", "Nested")]),
                ],
            ),
            tray::TrayItem::new("quit", "Quit"),
        ];
        tray::Tray::new("Snapoleon smoke", &test_image(), items, move |_| {
            clicks.fetch_add(1, Ordering::SeqCst);
        })
    };
    match tray_result {
        Ok(mut t) => {
            pump(Duration::from_millis(300));
            t.set_tooltip("Snapoleon smoke (updated)");
            t.set_icon(&test_image());
            t.set_items(vec![tray::TrayItem::new("only", "Rebuilt menu").checked(false)]);
            pump(Duration::from_millis(300));
            step("tray", Ok("icon shown, menu rebuilt, tooltip and icon updated".into()));
        }
        Err(e) => step("tray", Err(e.to_string())),
    }

    // clipboard
    step(
        "clipboard text",
        clipboard::set_text("Snapoleon привет").map_err(|e| e.to_string()).and_then(|_| {
            clipboard::get_text()
                .filter(|t| t == "Snapoleon привет")
                .map(|t| format!("round trip {t:?}"))
                .ok_or("read back mismatch".into())
        }),
    );
    let img = test_image();
    step(
        "clipboard image",
        clipboard::set_image(&img).map_err(|e| e.to_string()).and_then(|_| {
            let back = clipboard::get_image().ok_or("no image on clipboard")?;
            back.save(dir.join("clipboard-roundtrip.png")).map_err(|e| e.to_string())?;
            let exact = back.dimensions() == img.dimensions() && back.pixels().zip(img.pixels()).all(|(a, b)| a == b);
            Ok(format!("{}x{} read back, pixel-exact incl. alpha: {exact}", back.width(), back.height()))
        }),
    );
    step(
        "clipboard html",
        clipboard::set_html("<b>Snapoleon</b> <i>html</i>", "Snapoleon html")
            .map(|_| "set with plain-text alternative".into())
            .map_err(|e| e.to_string()),
    );
    let file = dir.join("copied-file.txt");
    std::fs::write(&file, "file on the clipboard").expect("write file");
    step(
        "clipboard files",
        clipboard::set_files(std::slice::from_ref(&file)).map_err(|e| e.to_string()).and_then(|_| {
            let listed = arboard::Clipboard::new().and_then(|mut c| c.get().file_list()).map_err(|e| e.to_string())?;
            Ok(format!("file list on clipboard: {listed:?}"))
        }),
    );

    // notification
    let icon_png = dir.join("toast-image.png");
    img.save(&icon_png).expect("save image");
    step("notify app icon", notify::set_app_icon(&img).map(|_| "registered".into()).map_err(|e| e.to_string()));
    step(
        "notify toast",
        notify::show(&notify::OsNotice {
            title: "Snapoleon smoke test".into(),
            body: "Screenshot saved \u{2714}".into(),
            image: Some(icon_png.clone()),
            ..notify::OsNotice::default()
        })
        .map(|_| "toast handed to the OS".into())
        .map_err(|e| e.to_string()),
    );

    // ipc: serve, ping ourselves, single-instance check
    match ipc::serve(|req| match req {
        Request::Ping => Response { ok: true, text: Some("pong".into()), ..Default::default() },
        other => Response::err(format!("unexpected {other:?}")),
    }) {
        Err(e) => step("ipc serve", Err(e.to_string())),
        Ok(server) => {
            step("ipc ping", ipc::send(&Request::Ping).map(|r| format!("{r:?}")).map_err(|e| e.to_string()));
            step("ipc instance_running", if ipc::instance_running() { Ok("true".into()) } else { Err("false".into()) });
            step(
                "ipc second server",
                match ipc::serve(|_| Response::ok()) {
                    Err(e) if ipc::is_already_running(&e) => Ok(format!("rejected as expected: {e}")),
                    Err(e) => Err(format!("wrong error: {e}")),
                    Ok(_) => Err("second server started".into()),
                },
            );
            let parallel: Vec<_> = (0..8).map(|_| std::thread::spawn(|| ipc::send(&Request::Ping).is_ok_and(|r| r.ok))).collect();
            let ok = parallel.into_iter().map(|h| h.join().unwrap_or(false)).filter(|ok| *ok).count();
            step("ipc 8 parallel pings", if ok == 8 { Ok("8/8".into()) } else { Err(format!("{ok}/8")) });
            drop(server);
            step("ipc after shutdown", if ipc::instance_running() { Err("still running".into()) } else { Ok("endpoint released".into()) });
            step(
                "ipc send without server",
                match ipc::send(&Request::Ping) {
                    Err(snap_core::Error::NotFound(m)) => Ok(format!("NotFound({m})")),
                    other => Err(format!("{other:?}")),
                },
            );
        }
    }

    // secrets
    let store = secrets::store();
    step(
        "secrets round trip",
        (|| {
            store.set("smoke:key", "s3cr\u{e9}t \u{2603}").map_err(|e| e.to_string())?;
            let got = store.get("smoke:key");
            if got.as_deref() != Some("s3cr\u{e9}t \u{2603}") {
                return Err(format!("get returned {got:?}"));
            }
            store.delete("smoke:key").map_err(|e| e.to_string())?;
            if store.get("smoke:key").is_some() {
                return Err("still present after delete".into());
            }
            store.delete("smoke:key").map_err(|e| format!("second delete failed: {e}"))?;
            Ok("set/get/delete ok (OS credential store)".into())
        })(),
    );
    step("secrets backend", secrets::KeyringStore::new().map(|_| "KeyringStore available".into()).map_err(|e| e.to_string()));

    // autostart
    let before = autostart::get();
    step(
        "autostart on",
        autostart::set(true)
            .map_err(|e| e.to_string())
            .and_then(|_| if autostart::get() { Ok("enabled".into()) } else { Err("get() is false".into()) }),
    );
    step(
        "autostart off",
        autostart::set(false)
            .map_err(|e| e.to_string())
            .and_then(|_| if !autostart::get() { Ok("disabled".into()) } else { Err("get() is true".into()) }),
    );
    if before {
        let _ = autostart::set(true);
    }

    // policy: Windows policy keys are admin-writable only, so this reads whatever is configured.
    policy::load();
    step("policy", Ok(format!("pairs={:?} -> {:?}", policy::read_pairs(), snap_core::policy::PLATFORM_POLICY.get())));

    // permissions
    let perms: Vec<_> = permissions::Perm::ALL.iter().map(|p| (*p, permissions::granted(*p))).collect();
    step("permissions", Ok(format!("{perms:?}")));

    // open
    step(
        "open url (rejects file:)",
        match open::url("file:///C:/Windows/System32/calc.exe") {
            Err(snap_core::Error::Invalid(m)) => Ok(m),
            other => Err(format!("{other:?}")),
        },
    );
    if with_reveal {
        step("open reveal", open::reveal(&file).map(|_| "explorer asked to select the file".into()).map_err(|e| e.to_string()));
    }

    // scheme
    if with_scheme {
        step("scheme register", scheme::register().map(|_| "registered".into()).map_err(|e| e.to_string()));
    }

    // sound
    sound::shutter();
    pump(Duration::from_millis(400));
    step("sound", Ok("shutter() returned immediately".into()));

    drop(hotkey_ok);
    println!("done; files in {}", dir.display());
}
