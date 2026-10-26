//! What the command line and the MCP server share as processes outside the app: captures that go through the
//! running app when this process may not read the screen, the privacy-permission report, and the window list as
//! agents should see it.
//!
//! macOS checks Screen Recording against the *responsible* app. For `snapoleon capture` or `snapoleon mcp` started
//! from a terminal or an agent host (Terminal, iTerm, an editor, Claude) that is the host, not Snapoleon, so the
//! user's grant to Snapoleon does not count there. Such a process hands the grab to the running app
//! ([`Request::Grab`]), starting it in the background when it is not running, and names the host in its error when
//! neither is possible.

use std::path::Path;

use serde_json::{Value, json};
use snap_core::{
    Result,
    ipc::{AppStatus, GrabInfo, GrabTarget, Permissions, Request, Response},
    settings::ImageFormat,
};
use snap_platform::permissions::{self, Perm};

use crate::{CaptureRequest, Engine};

/// [`Engine::capture`] for a process outside the app.
pub fn capture(engine: &Engine, request: &CaptureRequest) -> Result<snap_core::Capture> {
    #[cfg(target_os = "macos")]
    if !permissions::granted(Perm::ScreenRecording)
        && let Some(target) = mac::grab_target(request)
    {
        return mac::through_app(engine, target);
    }
    engine.capture(request)
}

/// The app's half of [`Request::Grab`], run on a worker thread: capture, write the PNG to `out`, describe it.
pub fn grab(engine: &Engine, target: GrabTarget, cursor: bool, out: &Path) -> Response {
    let mut settings = engine.settings();
    let with_cursor;
    let engine = if settings.capture.include_cursor == cursor {
        engine
    } else {
        settings.capture.include_cursor = cursor;
        with_cursor = Engine::with_parts(settings, engine.secrets(), None);
        &with_cursor
    };
    let request = match target {
        GrabTarget::Monitor { index } => CaptureRequest::Monitor(index),
        GrabTarget::AllMonitors => CaptureRequest::AllMonitors,
        GrabTarget::Window { id } => CaptureRequest::Window(Some(id)),
        GrabTarget::WindowTitled { title } => CaptureRequest::WindowTitled(title),
        GrabTarget::Region { rect } => CaptureRequest::Rect(rect),
        GrabTarget::ElementAtCursor => CaptureRequest::ElementAtCursor,
    };
    let grabbed = engine.capture(&request).and_then(|cap| {
        snap_image::encode::save(&cap.image, out, ImageFormat::Png, 100, false)?;
        Ok(cap)
    });
    match grabbed {
        Ok(cap) => Response {
            ok: true,
            path: Some(out.display().to_string()),
            grab: Some(GrabInfo {
                kind: cap.kind,
                screen_rect: cap.screen_rect,
                scale_factor: cap.scale_factor,
                context: cap.context,
                tone_mapped: cap.tone_mapped,
            }),
            ..Response::default()
        },
        Err(e) => Response::err(crate::exec::error_text(&e)),
    }
}

/// The privacy permissions of this process.
pub fn permissions() -> Permissions {
    Permissions {
        screen_recording: permissions::granted(Perm::ScreenRecording),
        accessibility: permissions::granted(Perm::Accessibility),
        input_monitoring: permissions::granted(Perm::InputMonitoring),
        microphone: permissions::granted(Perm::Microphone),
        needs_relaunch: permissions::needs_relaunch(),
    }
}

/// The app's answer to [`Request::Status`].
pub fn status() -> Response {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    let status = AppStatus { version: snap_core::VERSION.to_owned(), pid: std::process::id(), exe, permissions: permissions() };
    Response { ok: true, status: Some(status), ..Response::default() }
}

/// The running app's status; `None` when no app runs or it does not answer (an older version).
pub(crate) fn app_status() -> Option<AppStatus> {
    if !snap_platform::ipc::instance_running() {
        return None;
    }
    snap_platform::ipc::send(&Request::Status).ok().and_then(|r| r.status)
}

/// The monitors as the CLI and the MCP server report them.
pub(crate) fn monitors_json() -> Result<Vec<Value>> {
    Ok(snap_capture::monitors()?
        .iter()
        .map(|m| {
            json!({
                "index": m.index,
                "name": m.name,
                "x": m.rect.x, "y": m.rect.y, "width": m.rect.w, "height": m.rect.h,
                "scale_factor": m.scale_factor,
                "primary": m.is_primary,
                "hdr": m.hdr,
                "refresh_hz": m.refresh_hz,
            })
        })
        .collect())
}

/// The windows, front to back, as the CLI and the MCP server report them. Snapoleon's own untitled windows are left
/// out: on macOS the text input system gives every app with a text field an invisible 500x500 cursor-accessory
/// window (`TUINSWindow`), which window lists report as an untitled Snapoleon window and which is never worth a capture.
pub(crate) fn windows_json() -> Result<Vec<Value>> {
    Ok(snap_capture::windows()?
        .iter()
        .filter(|w| !(w.title.is_empty() && w.app_name.eq_ignore_ascii_case(snap_core::APP_NAME)))
        .map(|w| {
            json!({
                "id": w.id,
                "title": w.title,
                "app": w.app_name,
                "pid": w.pid,
                "x": w.rect.x, "y": w.rect.y, "width": w.rect.w, "height": w.rect.h,
                "z": w.z,
                "minimized": w.minimized,
            })
        })
        .collect())
}

/// Whether this process may read window titles and pixels. Without it macOS lists windows with empty titles.
pub(crate) fn can_read_screen() -> bool {
    permissions::granted(Perm::ScreenRecording)
}

#[cfg(target_os = "macos")]
pub use mac::{app_bundle, start_app};
#[cfg(target_os = "macos")]
pub(crate) use mac::{host_app, host_permission_error};

#[cfg(target_os = "macos")]
mod mac {
    use std::{
        ffi::OsString,
        os::unix::ffi::OsStringExt,
        path::{Path, PathBuf},
        process::Command,
        time::{Duration, Instant},
    };

    use snap_core::{
        Capture, CaptureId, Error, Result,
        ipc::{GrabTarget, Request},
        t, tf,
    };

    use crate::{CaptureRequest, Engine};

    /// How long a freshly started app may take to answer on its IPC endpoint.
    const APP_START_TIMEOUT: Duration = Duration::from_secs(15);
    const APP_START_POLL: Duration = Duration::from_millis(200);
    /// Process trees are shallow; this only stops a loop on a corrupt parent chain.
    const MAX_ANCESTORS: usize = 32;

    /// The grab the app can do for `request`. The clipboard and files need no permission, and the other requests are
    /// never made from outside the app.
    pub(super) fn grab_target(request: &CaptureRequest) -> Option<GrabTarget> {
        Some(match request {
            CaptureRequest::Monitor(index) => GrabTarget::Monitor { index: *index },
            CaptureRequest::AllMonitors => GrabTarget::AllMonitors,
            CaptureRequest::Window(Some(id)) => GrabTarget::Window { id: *id },
            CaptureRequest::WindowTitled(title) => GrabTarget::WindowTitled { title: title.clone() },
            CaptureRequest::Rect(rect) => GrabTarget::Region { rect: *rect },
            CaptureRequest::ElementAtCursor => GrabTarget::ElementAtCursor,
            _ => return None,
        })
    }

    /// Let the running app (started now if need be) grab the pixels, and read them back from the file it wrote.
    pub(super) fn through_app(engine: &Engine, target: GrabTarget) -> Result<Capture> {
        if !snap_platform::ipc::instance_running() && !start_app() {
            return Err(host_permission_error());
        }
        let out = std::env::temp_dir().join(format!("snapoleon-grab-{}.png", CaptureId::new()));
        let request = Request::Grab { target, cursor: engine.settings().capture.include_cursor, out: out.display().to_string() };
        let response = snap_platform::ipc::send(&request);
        let image = match &response {
            Ok(r) if r.ok => Some(snap_image::encode::load(&out)),
            _ => None,
        };
        let _ = std::fs::remove_file(&out);
        let response = response?;
        match (response.grab, image) {
            (Some(info), Some(image)) => {
                let mut cap = Capture::new(info.kind, image?, info.screen_rect, info.scale_factor);
                cap.context = info.context;
                cap.tone_mapped = info.tone_mapped;
                Ok(cap)
            }
            // The app's own error (it lacks the permission itself, no such window, ...) is already in words.
            _ => Err(Error::Other(anyhow::anyhow!(response.error.unwrap_or_else(|| t("engine.agent.app_failed").into_owned())))),
        }
    }

    /// The `.app` this executable runs from, symlinks resolved; `None` for a bare binary (`cargo build`).
    pub fn app_bundle() -> Option<PathBuf> {
        let exe = std::env::current_exe().and_then(std::fs::canonicalize).ok()?;
        let macos = exe.parent()?;
        let contents = macos.parent()?;
        let bundle = contents.parent()?;
        (macos.file_name()? == "MacOS" && contents.file_name()? == "Contents" && bundle.extension()? == "app").then(|| bundle.to_path_buf())
    }

    /// Start this installation's app in the tray through LaunchServices, so it is its own responsible process, and
    /// wait until it answers on its IPC endpoint. Only an app bundle can be started this way.
    pub fn start_app() -> bool {
        let Some(bundle) = app_bundle() else {
            return false;
        };
        let opened =
            Command::new("open").args(["-g", "-a"]).arg(&bundle).args(["--args", snap_platform::autostart::BACKGROUND_ARG]).status();
        if !matches!(opened, Ok(status) if status.success()) {
            tracing::warn!("could not start {}: {opened:?}", bundle.display());
            return false;
        }
        let deadline = Instant::now() + APP_START_TIMEOUT;
        while Instant::now() < deadline {
            if snap_platform::ipc::instance_running() {
                return true;
            }
            std::thread::sleep(APP_START_POLL);
        }
        tracing::warn!("{} did not answer within {APP_START_TIMEOUT:?}", bundle.display());
        false
    }

    /// The permission error for a process outside the app, naming the app macOS actually asks.
    pub(crate) fn host_permission_error() -> Error {
        let host = host_app().unwrap_or_else(|| t("engine.agent.unknown_host").into_owned());
        Error::permission(tf("engine.agent.host_permission", &[("host", &host)]))
    }

    /// The app macOS holds responsible for this process: the outermost `.app` among its ancestors (Terminal, iTerm,
    /// Visual Studio Code for its integrated terminal, Claude). `None` with no app above it (ssh, launchd).
    pub(crate) fn host_app() -> Option<String> {
        let mut pid = std::os::unix::process::parent_id() as i32;
        for _ in 0..MAX_ANCESTORS {
            if pid <= 1 {
                return None;
            }
            if let Some(name) = process_path(pid).as_deref().and_then(outermost_app) {
                return Some(name);
            }
            pid = parent_of(pid)?;
        }
        None
    }

    fn process_path(pid: i32) -> Option<PathBuf> {
        let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        // SAFETY: the buffer is as large as the size passed; the call writes at most that many bytes.
        let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
        (n > 0).then(|| {
            buf.truncate(n as usize);
            PathBuf::from(OsString::from_vec(buf))
        })
    }

    fn parent_of(pid: i32) -> Option<i32> {
        // SAFETY: proc_bsdinfo is plain old data, so all zeroes is a valid value.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        // SAFETY: the buffer is a proc_bsdinfo of the size passed, which is what PROC_PIDTBSDINFO fills.
        let n = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
        (n == size).then_some(info.pbi_ppid as i32)
    }

    fn outermost_app(path: &Path) -> Option<String> {
        let app = path.ancestors().filter(|p| p.extension().is_some_and(|e| e == "app")).last()?;
        Some(app.file_stem()?.to_string_lossy().into_owned())
    }
}
