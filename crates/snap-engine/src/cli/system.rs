//! Commands about the installation rather than about images: `list`, `doctor` and `install-cli`.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use snap_core::{Error, Result, paths, t, tf};

use super::{ListCommand, headless::print_json};
use crate::agent;

// --- list -----------------------------------------------------------------------------------------------------------

pub(super) fn list_cmd(what: ListCommand) -> Result<i32> {
    match what {
        ListCommand::Monitors { json } => {
            let monitors = agent::monitors_json()?;
            if json {
                print_json(&monitors)?;
            } else {
                for m in &monitors {
                    let primary = if m["primary"] == true { t("cli.list.primary") } else { "".into() };
                    println!("{}\t{}\t{}\t{}x\t{primary}", m["index"], text(&m["name"]), rect(m), m["scale_factor"]);
                }
            }
        }
        ListCommand::Windows { json } => {
            let windows = agent::windows_json()?;
            if json {
                print_json(&windows)?;
            } else {
                for w in &windows {
                    println!("{}\t{}\t{}\t{}", w["id"], text(&w["app"]), text(&w["title"]), rect(w));
                }
            }
            #[cfg(target_os = "macos")]
            if !agent::can_read_screen() {
                let host = agent::host_app().unwrap_or_else(|| t("engine.agent.unknown_host").into_owned());
                eprintln!("{}", tf("cli.list.no_titles", &[("host", &host)]));
            }
        }
    }
    Ok(0)
}

fn text(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

/// `x,y,w,h`, the form `--region` takes.
fn rect(v: &Value) -> String {
    format!("{},{},{},{}", v["x"], v["y"], v["width"], v["height"])
}

// --- doctor ---------------------------------------------------------------------------------------------------------

pub(super) fn doctor_cmd(json: bool) -> Result<i32> {
    let report = report();
    if json {
        print_json(&report)?;
    } else {
        print_report(&report);
    }
    Ok(0)
}

/// Everything worth knowing when something does not work, as stable JSON keys.
fn report() -> Value {
    let exe = std::env::current_exe().and_then(std::fs::canonicalize).ok();
    let app = match agent::app_status() {
        Some(app) => json!({ "running": true, "version": app.version, "pid": app.pid, "exe": app.exe, "permissions": app.permissions }),
        None => json!({ "running": snap_platform::ipc::instance_running() }),
    };
    #[cfg(target_os = "macos")]
    let (bundle, host_app) = (agent::app_bundle(), agent::host_app());
    #[cfg(not(target_os = "macos"))]
    let (bundle, host_app): (Option<PathBuf>, Option<String>) = (None, None);
    let signature = bundle.as_deref().or(exe.as_deref()).and_then(designated_requirement);
    json!({
        "version": snap_core::VERSION,
        "os": std::env::consts::OS,
        "os_version": os_version(),
        "arch": std::env::consts::ARCH,
        "exe": exe,
        "bundle": bundle,
        "signature": signature,
        "host_app": host_app,
        "portable": paths::is_portable(),
        "on_path": on_path(),
        "paths": {
            "settings": paths::settings_file(),
            "data": paths::data_dir(),
            "history": paths::history_db(),
            "log": paths::logs_dir().join("snapoleon.log"),
        },
        "permissions": agent::permissions(),
        "app": app,
    })
}

fn print_report(r: &Value) {
    let yes_no = |v: &Value| if v == true { t("cli.doctor.yes") } else { t("cli.doctor.no") };
    let mut rows: Vec<(String, String)> = vec![
        (t("cli.doctor.version").into_owned(), format!("{} ({})", text(&r["version"]), platform(r))),
        (t("cli.doctor.exe").into_owned(), text(&r["exe"]).to_owned()),
    ];
    for (key, value) in [("bundle", &r["bundle"]), ("signature", &r["signature"]), ("host_app", &r["host_app"])] {
        if let Some(value) = value.as_str() {
            rows.push((t(&format!("cli.doctor.{key}")).into_owned(), value.to_owned()));
        }
    }
    rows.push((t("cli.doctor.on_path").into_owned(), r["on_path"].as_str().map_or_else(|| t("cli.doctor.no").into_owned(), str::to_owned)));
    rows.push((t("cli.doctor.portable").into_owned(), yes_no(&r["portable"]).into_owned()));
    for key in ["settings", "data", "history", "log"] {
        rows.push((t(&format!("cli.doctor.path.{key}")).into_owned(), text(&r["paths"][key]).to_owned()));
    }
    let app = &r["app"];
    let app_line = match (app["running"] == true, app["version"].as_str()) {
        (true, Some(version)) => tf("cli.doctor.app_running", &[("pid", &app["pid"].to_string()), ("version", version)]),
        (true, None) => t("cli.doctor.app_unknown").into_owned(),
        (false, _) => t("cli.doctor.app_stopped").into_owned(),
    };
    rows.push((t("cli.doctor.app").into_owned(), app_line));
    print_rows(&rows);

    println!();
    if cfg!(target_os = "macos") {
        let this = t("cli.doctor.this_process");
        let column =
            [this.chars().count(), t("cli.doctor.yes").chars().count(), t("cli.doctor.no").chars().count()].into_iter().max().unwrap_or(0)
                + 2;
        let mut rows = vec![(t("cli.doctor.permission").into_owned(), format!("{this:<column$}{}", t("cli.doctor.app_column")))];
        for key in ["screen_recording", "accessibility", "input_monitoring", "microphone"] {
            let app = app["permissions"].get(key).map_or_else(|| "-".to_owned(), |v| yes_no(v).into_owned());
            rows.push((t(&format!("cli.doctor.perm.{key}")).into_owned(), format!("{:<column$}{app}", yes_no(&r["permissions"][key]))));
        }
        print_rows(&rows);
        for hint in hints(r) {
            println!();
            println!("{hint}");
        }
    } else {
        println!("{}", t("cli.doctor.no_permissions"));
    }
}

/// "macos 27.0.1, aarch64": the parts that are known.
fn platform(r: &Value) -> String {
    let os = [text(&r["os"]), text(&r["os_version"])].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
    format!("{os}, {}", text(&r["arch"]))
}

/// Label and value columns, the labels padded to the longest one.
fn print_rows(rows: &[(String, String)]) {
    let width = rows.iter().map(|(label, _)| label.chars().count()).max().unwrap_or(0) + 2;
    for (label, value) in rows {
        let pad = width - label.chars().count();
        println!("{label}{:pad$}{value}", "");
    }
}

/// What to do about the problems the report shows (macOS only: elsewhere there are no permissions to miss).
fn hints(r: &Value) -> Vec<String> {
    let mut hints = Vec::new();
    let app = &r["app"];
    let granted = |p: &Value, key: &str| p[key] == true;
    if app["running"] != true {
        hints.push(t("cli.doctor.hint.start_app").into_owned());
    } else if granted(&app["permissions"], "needs_relaunch") {
        hints.push(t("cli.doctor.hint.app_relaunch").into_owned());
    } else if app["permissions"].is_object() && !granted(&app["permissions"], "screen_recording") {
        hints.push(t("cli.doctor.hint.app_screen").into_owned());
    }
    if r["bundle"].is_null() {
        hints.push(t("cli.doctor.hint.bare_binary").into_owned());
    }
    if r["on_path"].is_null() {
        hints.push(t("cli.doctor.hint.install_cli").into_owned());
    }
    hints
}

/// The code-signing requirement macOS keys privacy grants to; `None` elsewhere or when unsigned.
fn designated_requirement(path: &Path) -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let out = std::process::Command::new("codesign").args(["-d", "-r-"]).arg(path).output().ok()?;
    let text = [out.stdout, out.stderr].concat();
    String::from_utf8_lossy(&text).lines().find_map(|l| l.strip_prefix("designated => ").map(str::to_owned))
}

fn os_version() -> Option<String> {
    if cfg!(target_os = "macos") {
        let out = std::process::Command::new("sw_vers").arg("-productVersion").output().ok()?;
        return Some(String::from_utf8_lossy(&out.stdout).trim().to_owned()).filter(|v| !v.is_empty());
    }
    if cfg!(target_os = "linux") {
        let release = std::fs::read_to_string("/etc/os-release").ok()?;
        return release.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=")).map(|v| v.trim_matches('"').to_owned());
    }
    None
}

/// The first `snapoleon` on PATH.
fn on_path() -> Option<PathBuf> {
    let name = if cfg!(windows) { "snapoleon.exe" } else { "snapoleon" };
    std::env::split_paths(&std::env::var_os("PATH")?).map(|dir| dir.join(name)).find(|p| p.is_file())
}

// --- install-cli ----------------------------------------------------------------------------------------------------

/// Marks a script written by [`install_cli`], so a later run may replace it.
const SHIM_MARKER: &str = "# Installed by snapoleon install-cli.";

/// Put a `snapoleon` command into `dir` (default: `/usr/local/bin` on macOS, `~/.local/bin` on Linux) that runs this
/// installation, and return its path. It is a two-line shell script, not a symlink: macOS knows an app by the path its
/// executable was started from, so a symlinked binary runs without its bundle (no bundle id, no Info.plist, and its
/// own portable-mode guess), while `exec` of the real path keeps all of that. On macOS a folder this user may not
/// write to is written through the administrator prompt. Windows installers put the app on PATH themselves.
pub fn install_cli(dir: Option<&Path>) -> Result<PathBuf> {
    if cfg!(windows) {
        return Err(Error::unsupported(t("cli.install.windows")));
    }
    let exe = this_installation()?;
    #[cfg(target_os = "macos")]
    if on_temporary_mount(&exe) {
        return Err(Error::invalid(t("cli.install.temporary")));
    }
    let dir = match dir {
        Some(dir) => dir.to_path_buf(),
        None => default_dir()?,
    };
    let target = dir.join("snapoleon");
    // A symlink is replaced (that is the broken kind), a script of ours is rewritten, anything else is left alone.
    if target.symlink_metadata().is_ok_and(|m| m.file_type().is_file()) && !is_shim(&target) {
        if std::fs::canonicalize(&target).is_ok_and(|p| p == exe) {
            // A package manager put the binary itself there (a .deb's /usr/bin): it already works.
            return Ok(target);
        }
        return Err(Error::invalid(tf("cli.install.exists", &[("path", &target.display().to_string())])));
    }
    let script = format!("#!/bin/sh\n{SHIM_MARKER}\nexec {} \"$@\"\n", sh_quote(&exe));
    match write_script(&dir, &target, &script) {
        Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::PermissionDenied && cfg!(target_os = "macos") => {
            write_script_as_admin(&dir, &target, &script)?;
        }
        other => other?,
    }
    let on_path = std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d == dir));
    if !on_path {
        eprintln!("{}", tf("cli.install.not_on_path", &[("dir", &dir.display().to_string())]));
    }
    Ok(target)
}

/// The `snapoleon` command that runs this installation, if there is one: the script [`install_cli`] writes, a package
/// manager's wrapper (the Homebrew cask's) or the executable itself, in a folder on PATH or in [`install_cli`]'s default
/// folder. That folder counts because an app the system started sees only the system's PATH, not the shell's.
pub fn installed_cli() -> Option<PathBuf> {
    let exe = this_installation().ok()?;
    let name = if cfg!(windows) { "snapoleon.exe" } else { "snapoleon" };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if !cfg!(windows) {
        dirs.extend(default_dir().ok());
    }
    if cfg!(target_os = "macos") {
        // Where Homebrew puts its commands on Apple silicon (the cask's wrapper among them).
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
    }
    let target = exe.display().to_string();
    dirs.into_iter().map(|dir| dir.join(name)).find(|candidate| {
        let runs_exe = std::fs::canonicalize(candidate).is_ok_and(|real| real == exe);
        // A wrapper script is a few lines that name the executable; anything bigger is some other program.
        let small = std::fs::metadata(candidate).is_ok_and(|m| m.is_file() && m.len() < 4096);
        runs_exe || (small && std::fs::read_to_string(candidate).is_ok_and(|script| script.contains(&target)))
    })
}

/// What a `snapoleon` command should run: this executable, resolved. An AppImage runs from a mount that is gone after
/// it exits; the image itself is what to call.
fn this_installation() -> Result<PathBuf> {
    match std::env::var_os("APPIMAGE") {
        Some(image) => Ok(PathBuf::from(image)),
        None => Ok(std::env::current_exe().and_then(std::fs::canonicalize)?),
    }
}

/// Where [`install_cli`] puts the command unless told otherwise: `/usr/local/bin` on macOS, `~/.local/bin` on Linux.
fn default_dir() -> Result<PathBuf> {
    if cfg!(target_os = "macos") {
        return Ok(PathBuf::from("/usr/local/bin"));
    }
    Ok(PathBuf::from(std::env::var_os("HOME").ok_or_else(|| Error::NotFound("HOME".into()))?).join(".local/bin"))
}

/// A disk image and App Translocation's copy (an app opened from Downloads without being moved) are read-only mounts
/// that are gone after an eject or a restart, so a shim that execs into them stops working.
#[cfg(target_os = "macos")]
fn on_temporary_mount(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    if path.to_string_lossy().contains("/AppTranslocation/") {
        return true;
    }
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: a NUL-terminated path and a zeroed out-struct that statfs fills in.
    let mut fs: libc::statfs = unsafe { std::mem::zeroed() };
    unsafe { libc::statfs(c.as_ptr(), &mut fs) == 0 && fs.f_flags & libc::MNT_RDONLY as u32 != 0 }
}

fn is_shim(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|s| s.contains(SHIM_MARKER))
}

/// Single-quoted for `sh`.
fn sh_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

#[cfg(unix)]
fn write_script(dir: &Path, target: &Path, script: &str) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(".snapoleon.install");
    std::fs::write(&tmp, script)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    // Renaming over the old file also replaces a symlink (a Homebrew `binary` link) instead of writing through it.
    std::fs::rename(&tmp, target)?;
    Ok(())
}

#[cfg(not(unix))]
fn write_script(_dir: &Path, _target: &Path, _script: &str) -> Result<()> {
    Err(Error::unsupported(t("cli.install.windows")))
}

/// The same through `osascript`'s administrator prompt, for `/usr/local/bin`, which belongs to root on a fresh Mac.
fn write_script_as_admin(dir: &Path, target: &Path, script: &str) -> Result<()> {
    let tmp = std::env::temp_dir().join(format!("snapoleon-install-{}", std::process::id()));
    std::fs::write(&tmp, script)?;
    let shell = format!(
        "mkdir -p {dir} && rm -f {target} && install -m 0755 {tmp} {target}",
        dir = sh_quote(dir),
        target = sh_quote(target),
        tmp = sh_quote(&tmp)
    );
    let apple_script = format!("do shell script \"{}\" with administrator privileges", shell.replace('\\', "\\\\").replace('"', "\\\""));
    let out = std::process::Command::new("osascript").args(["-e", &apple_script]).output();
    let _ = std::fs::remove_file(&tmp);
    let out = out?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    // -128 is "User canceled." in the password prompt.
    if stderr.contains("-128") {
        return Err(Error::Cancelled);
    }
    Err(Error::Other(anyhow::anyhow!("{}", stderr.trim())))
}
