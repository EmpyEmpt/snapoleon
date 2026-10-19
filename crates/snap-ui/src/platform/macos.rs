use super::{ExtraKeys, run};

pub fn system_dark() -> Option<bool> {
    // The key is absent in light mode, so a failed read means light.
    Some(
        run("defaults", &["read", "-g", "AppleInterfaceStyle"])
            .is_some_and(|s| s.eq_ignore_ascii_case("dark")),
    )
}

pub fn reduce_motion() -> bool {
    run(
        "defaults",
        &["read", "com.apple.universalaccess", "reduceMotion"],
    )
    .is_some_and(|s| s == "1")
}

pub fn extra_keys_down() -> ExtraKeys {
    ExtraKeys::default()
}
