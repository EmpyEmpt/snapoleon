//! Localised display strings for capture data.

use snap_core::{Error, t, tf};

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

/// User-facing text for a capture error. Permission errors get an explanatory, localised sentence.
pub fn error_message(e: &Error) -> String {
    match e {
        Error::Permission(what) => {
            let key = match what.as_ref() {
                "Screen Recording" => "capture.permission.screen_recording",
                "Accessibility" => "capture.permission.accessibility",
                _ => return tf("capture.permission.needed", &[("what", what)]),
            };
            tf("capture.permission.needed", &[("what", &t(key))])
        }
        other => other.to_string(),
    }
}
