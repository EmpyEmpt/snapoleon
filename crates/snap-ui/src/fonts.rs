//! Inter (UI), JetBrains Mono (numbers, code) and Phosphor (icons, merged as a fallback).

use egui::{Context, FontData, FontDefinitions, FontFamily, Id};

static INTER_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
static INTER_SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf");
static INTER_BOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-Bold.ttf");
static JETBRAINS_MONO: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Medium.ttf");
static PHOSPHOR: &[u8] = include_bytes!("../assets/Phosphor.ttf");

/// Family for semibold UI text (`body_strong`, `title`).
pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}

/// Family for bold UI text (`headline`).
pub fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}

pub fn definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    for (name, bytes) in [
        ("Inter", INTER_REGULAR),
        ("Inter-SemiBold", INTER_SEMIBOLD),
        ("Inter-Bold", INTER_BOLD),
        ("JetBrainsMono", JETBRAINS_MONO),
        ("Phosphor", PHOSPHOR),
    ] {
        fonts.font_data.insert(name.into(), FontData::from_static(bytes).into());
    }

    // egui's stock Latin fonts are replaced; its icon/emoji fonts stay as last-resort fallbacks.
    let fallbacks: Vec<String> = fonts
        .families
        .get(&FontFamily::Proportional)
        .into_iter()
        .flatten()
        .filter(|n| !matches!(n.as_str(), "Ubuntu-Light" | "Hack"))
        .cloned()
        .collect();
    let chain = |primary: &[&str]| -> Vec<String> { primary.iter().map(|s| s.to_string()).chain(fallbacks.iter().cloned()).collect() };
    fonts.families.insert(FontFamily::Proportional, chain(&["Inter", "Phosphor"]));
    fonts.families.insert(FontFamily::Monospace, chain(&["JetBrainsMono", "Inter", "Phosphor"]));
    fonts.families.insert(semibold(), chain(&["Inter-SemiBold", "Phosphor"]));
    fonts.families.insert(bold(), chain(&["Inter-Bold", "Phosphor"]));
    fonts
}

#[derive(Clone, Copy)]
struct Installed;

/// Loads the fonts once per context.
pub fn install(ctx: &Context) {
    let id = Id::unique("snap-ui/fonts");
    if ctx.data(|d| d.get_temp::<Installed>(id)).is_none() {
        ctx.set_fonts(definitions());
        ctx.data_mut(|d| d.insert_temp(id, Installed));
    }
}
