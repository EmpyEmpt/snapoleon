//! Localised display strings for capture data.

use snap_core::{t, tf};

use crate::MonitorInfo;

/// "Display 1 (main)" / "Display 2: DELL U2723QE", in the active language.
pub fn monitor_label(m: &MonitorInfo) -> String {
    let n = (m.index + 1).to_string();
    if m.is_primary {
        tf("capture.monitor.label_primary", &[("n", &n)])
    } else if m.name.is_empty() {
        tf("capture.monitor.label", &[("n", &n)])
    } else {
        tf("capture.monitor.label_named", &[("n", &n), ("name", &m.name)])
    }
}

/// Localised name of an [`ElementInfo::role`](crate::ElementInfo::role); unknown roles are returned as-is.
pub fn role_label(role: &str) -> String {
    let key = format!("capture.role.{}", role.replace(' ', "_"));
    let label = t(&key);
    if label == key { role.to_string() } else { label.into_owned() }
}

/// The error for a missing macOS privacy permission, as the localised sentence users read; `name_key` is the
/// `capture.permission.*` key of the permission's name.
#[cfg(target_os = "macos")]
pub(crate) fn permission_error(name_key: &str) -> snap_core::Error {
    snap_core::Error::permission(tf("capture.permission.needed", &[("what", &t(name_key))]))
}
