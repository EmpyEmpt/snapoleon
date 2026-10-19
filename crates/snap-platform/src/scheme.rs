//! Per-user registration of the `snapoleon://` URL scheme and the `.snap`
//! project file association.
//!
//! - Windows: `HKCU\Software\Classes`; URLs run `"<exe>" url "%1"`, projects `"<exe>" edit "%1"`.
//! - Linux: desktop entries in `~/.local/share/applications` plus a shared-mime-info
//!   definition for `application/x-snapoleon`; `xdg-mime default` is run best-effort.
//! - macOS: nothing to do at runtime - the scheme and document type come from the
//!   app bundle's `Info.plist`.

use snap_core::Result;

/// Register the handlers for the current executable. Safe to call on every start.
pub fn register() -> Result<()> {
    imp::register()
}

#[cfg(windows)]
mod imp {
    use snap_core::{APP_NAME, PROJECT_EXT, Result, URL_SCHEME};
    use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    use crate::util::exe_path;

    const PROG_ID: &str = "Snapoleon.Project";

    pub fn register() -> Result<()> {
        let exe = exe_path()?;
        let exe = exe.display();
        let classes = RegKey::predef(HKEY_CURRENT_USER).create_subkey(r"Software\Classes")?.0;

        let scheme = classes.create_subkey(URL_SCHEME)?.0;
        scheme.set_value("", &format!("URL:{APP_NAME}"))?;
        scheme.set_value("URL Protocol", &"")?;
        scheme.create_subkey("DefaultIcon")?.0.set_value("", &format!("\"{exe}\",0"))?;
        scheme.create_subkey(r"shell\open\command")?.0.set_value("", &format!("\"{exe}\" url \"%1\""))?;

        classes.create_subkey(format!(".{PROJECT_EXT}"))?.0.set_value("", &PROG_ID)?;
        let prog = classes.create_subkey(PROG_ID)?.0;
        prog.set_value("", &format!("{APP_NAME} project"))?;
        prog.create_subkey("DefaultIcon")?.0.set_value("", &format!("\"{exe}\",0"))?;
        prog.create_subkey(r"shell\open\command")?.0.set_value("", &format!("\"{exe}\" edit \"%1\""))?;

        // SAFETY: plain notification to the shell, no pointers are passed.
        unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use snap_core::Result;

    pub fn register() -> Result<()> {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::path::Path;
    use std::process::Command;

    use snap_core::{APP_NAME, Error, PROJECT_EXT, Result, URL_SCHEME};

    use crate::util::{desktop_quote, exe_path, run_quiet};

    const MIME: &str = "application/x-snapoleon";
    const SCHEME_ENTRY: &str = "snapoleon.desktop";
    const PROJECT_ENTRY: &str = "snapoleon-project.desktop";

    pub fn register() -> Result<()> {
        let dirs = directories::BaseDirs::new().ok_or_else(|| Error::NotFound("home directory".into()))?;
        let data = dirs.data_dir();
        let apps = data.join("applications");
        let mime_root = data.join("mime");
        let exe = desktop_quote(&exe_path()?.to_string_lossy());

        std::fs::create_dir_all(&apps)?;
        std::fs::write(
            apps.join(SCHEME_ENTRY),
            format!(
                "[Desktop Entry]\nType=Application\nName={APP_NAME}\nExec={exe} url %u\nIcon=snapoleon\n\
                 Terminal=false\nNoDisplay=true\nMimeType=x-scheme-handler/{URL_SCHEME};\n"
            ),
        )?;
        std::fs::write(
            apps.join(PROJECT_ENTRY),
            format!(
                "[Desktop Entry]\nType=Application\nName={APP_NAME} project\nExec={exe} edit %f\nIcon=snapoleon\n\
                 Terminal=false\nNoDisplay=true\nMimeType={MIME};\n"
            ),
        )?;

        let packages = mime_root.join("packages");
        std::fs::create_dir_all(&packages)?;
        std::fs::write(
            packages.join("snapoleon.xml"),
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                 <mime-info xmlns=\"http://www.freedesktop.org/standards/shared-mime-info\">\n\
                 \t<mime-type type=\"{MIME}\">\n\t\t<comment>{APP_NAME} project</comment>\n\
                 \t\t<glob pattern=\"*.{PROJECT_EXT}\"/>\n\t</mime-type>\n</mime-info>\n"
            ),
        )?;

        refresh_databases(&apps, &mime_root);
        Ok(())
    }

    /// Best effort: these tools are missing on minimal systems and the entries
    /// still work after the next login.
    fn refresh_databases(apps: &Path, mime_root: &Path) {
        run_quiet(Command::new("update-mime-database").arg(mime_root));
        run_quiet(Command::new("update-desktop-database").arg(apps));
        run_quiet(Command::new("xdg-mime").args(["default", SCHEME_ENTRY, &format!("x-scheme-handler/{URL_SCHEME}")]));
        run_quiet(Command::new("xdg-mime").args(["default", PROJECT_ENTRY, MIME]));
    }
}
