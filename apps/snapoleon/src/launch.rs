//! macOS: the app has to be its own responsible process. Started from a terminal (`snapoleon` through the PATH shim,
//! the bundle's executable run by hand, `snapoleon capture --pipeline ...`) it would be checked for Screen Recording,
//! Accessibility and the rest as part of the terminal, so the user's grants to Snapoleon would not apply and the
//! prompts would name the terminal. Such a start goes through LaunchServices instead, like a double-click.

use std::process::{Command, ExitCode};

use snap_core::{ipc::Request, t};
use snap_engine::agent::AppStart;

/// `Some(exit code)` when this launch was handed to a copy of the app started through LaunchServices; `None` to run
/// the app in this process: it was started by LaunchServices or launchd already, it is a bare binary with no bundle to
/// start (`cargo run`), or `open` could not start the bundle.
pub fn relaunch(request: Option<&Request>, background: bool) -> Option<ExitCode> {
    // launchd is the parent of everything LaunchServices starts: Finder, the Dock, Spotlight, login items, `open`.
    if std::os::unix::process::parent_id() == 1 {
        return None;
    }
    let bundle = snap_engine::agent::app_bundle()?;
    let Some(request) = request else {
        let mut open = Command::new("open");
        open.arg("-a").arg(&bundle);
        if background {
            open.args(["--args", snap_platform::autostart::BACKGROUND_ARG]);
        }
        return open.status().is_ok_and(|s| s.success()).then_some(ExitCode::SUCCESS);
    };
    // Start it in the tray, then hand the request over as a second launch would, so its answer (a path, a link,
    // recognised text) still reaches this terminal.
    match snap_engine::agent::start_app() {
        AppStart::Answered => {}
        // Nothing was started: running here is the only way left to serve the request.
        AppStart::NotOpened => return None,
        // A copy is (or may still be) starting through LaunchServices: running here too would leave a terminal-owned
        // app, or a second instance racing it for the socket.
        AppStart::NoAnswer => {
            eprintln!("{}", t("app.ipc.no_answer"));
            return Some(ExitCode::FAILURE);
        }
    }
    Some(if crate::ipc::forward(Some(request.clone()), background) { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}
