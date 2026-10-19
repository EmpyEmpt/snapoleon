//! Prints what the design system reads from the OS and how settings resolve to a theme.
//!
//!     cargo run -p snap-ui --example probe

use snap_core::settings::{General, Theme};
use snap_ui::{Palette, ThemeSpec};

fn main() {
    println!("system dark mode: {:?}", snap_ui::system_dark());
    println!("reduce motion:    {}", snap_ui::reduce_motion());

    let system = ThemeSpec::from_settings(&General::default());
    println!(
        "default settings -> dark={} accent={:?}",
        system.dark, system.accent
    );

    let custom = General {
        theme: Theme::Light,
        accent: "#FF8800".into(),
        ..General::default()
    };
    let spec = ThemeSpec::from_settings(&custom);
    let p = Palette::new(spec.dark, spec.accent);
    println!(
        "light + #FF8800  -> dark={} accent={:?} on_accent={:?} accent_soft={:?}",
        spec.dark, spec.accent, p.on_accent, p.accent_soft
    );

    let bad = General {
        accent: "not-a-color".into(),
        theme: Theme::Dark,
        ..General::default()
    };
    println!("bad accent       -> {:?}", ThemeSpec::from_settings(&bad));

    for accel in [
        "Ctrl+Shift+4",
        "PrintScreen",
        "Cmd+Shift+4",
        "CmdOrCtrl+Alt+Left",
    ] {
        println!("{accel:>20} -> {:?}", snap_ui::widgets::accel_parts(accel));
    }
    for hex in ["#3D5AFE", "f80", "#11223344", "zz"] {
        println!("{hex:>20} -> {:?}", snap_ui::widgets::parse_color(hex));
    }
}
