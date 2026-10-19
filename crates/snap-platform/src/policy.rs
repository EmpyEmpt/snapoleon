//! Managed (IT-admin) policy from the OS.
//!
//! - Windows: `REG_SZ` / `REG_DWORD` values under `HKLM\SOFTWARE\Policies\Snapoleon`
//!   and `HKCU\SOFTWARE\Policies\Snapoleon`; machine policy wins over user policy.
//! - macOS: managed preferences `/Library/Managed Preferences/app.snapoleon.plist`
//!   (and the per-user copy), read through `plutil`.
//! - Linux: nothing here; `snap_core::policy::Policy::load` reads `/etc/snapoleon/policy.toml`.

use snap_core::policy::{PLATFORM_POLICY, Policy};

/// Read the OS policy store and publish it as `snap_core::policy::PLATFORM_POLICY`.
/// Call once at startup, before `Policy::load()`. Does nothing when no policy is set.
pub fn load() {
    let pairs = read_pairs();
    if pairs.is_empty() {
        return;
    }
    let policy = Policy::from_pairs(&pairs);
    if PLATFORM_POLICY.set(policy).is_err() {
        tracing::debug!("platform policy was already loaded");
    }
}

/// Raw key/value pairs of the OS policy store, lowest priority first
/// (the shape `Policy::from_pairs` expects: later pairs win).
pub fn read_pairs() -> Vec<(String, String)> {
    imp::read_pairs()
}

#[cfg(windows)]
mod imp {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RegType};
    use winreg::{HKEY, RegKey};

    const KEY: &str = r"SOFTWARE\Policies\Snapoleon";

    pub fn read_pairs() -> Vec<(String, String)> {
        // Per-user values first so that machine-wide policy overrides them.
        let mut pairs = read_hive(HKEY_CURRENT_USER);
        pairs.extend(read_hive(HKEY_LOCAL_MACHINE));
        pairs
    }

    fn read_hive(hive: HKEY) -> Vec<(String, String)> {
        let Ok(key) = RegKey::predef(hive).open_subkey(KEY) else {
            return Vec::new();
        };
        key.enum_values()
            .flatten()
            .filter_map(|(name, value)| match value.vtype {
                RegType::REG_SZ | RegType::REG_EXPAND_SZ => key.get_value::<String, _>(&name).ok().map(|v| (name, v)),
                RegType::REG_DWORD => key.get_value::<u32, _>(&name).ok().map(|v| (name, v.to_string())),
                _ => None,
            })
            .collect()
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::path::PathBuf;
    use std::process::Command;

    use snap_core::APP_ID;

    pub fn read_pairs() -> Vec<(String, String)> {
        let base = PathBuf::from("/Library/Managed Preferences");
        // Asked of the OS, not read from `$USER`: the user this policy restrains controls their environment.
        let user = Command::new("/usr/bin/id")
            .arg("-un")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty());
        // Per-user managed prefs override machine-wide ones.
        let files = [Some(base.join(format!("{APP_ID}.plist"))), user.map(|user| base.join(user).join(format!("{APP_ID}.plist")))];
        files.iter().flatten().filter(|f| f.is_file()).flat_map(|f| read_plist(f)).collect()
    }

    /// `plutil -convert json` turns any plist flavour into JSON on stdout.
    fn read_plist(path: &std::path::Path) -> Vec<(String, String)> {
        let Ok(out) = Command::new("plutil").args(["-convert", "json", "-o", "-"]).arg(path).output() else {
            return Vec::new();
        };
        if !out.status.success() {
            return Vec::new();
        }
        let Ok(serde_json::Value::Object(map)) = serde_json::from_slice(&out.stdout) else {
            return Vec::new();
        };
        map.into_iter()
            .filter_map(|(k, v)| {
                let text = match v {
                    serde_json::Value::String(s) => s,
                    serde_json::Value::Bool(b) => if b { "1" } else { "0" }.to_string(),
                    serde_json::Value::Number(n) => n.to_string(),
                    _ => return None,
                };
                Some((k, text))
            })
            .collect()
    }
}

#[cfg(target_os = "linux")]
mod imp {
    pub fn read_pairs() -> Vec<(String, String)> {
        Vec::new()
    }
}
