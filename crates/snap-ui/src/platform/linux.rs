use super::{ExtraKeys, run};

pub fn system_dark() -> Option<bool> {
    if let Some(scheme) = run("gsettings", &["get", "org.gnome.desktop.interface", "color-scheme"]) {
        return Some(scheme.contains("dark"));
    }
    let theme = std::env::var("GTK_THEME").ok()?;
    Some(theme.to_lowercase().contains("dark"))
}

pub fn reduce_motion() -> bool {
    run("gsettings", &["get", "org.gnome.desktop.interface", "enable-animations"]).is_some_and(|s| s == "false")
}

pub fn extra_keys_down() -> ExtraKeys {
    ExtraKeys::default()
}
