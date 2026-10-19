//! The active theme, shared by every window. `ShellConfig::theme_init` installs it into each new
//! egui context; when the settings change, every window re-installs it before its next frame.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use parking_lot::RwLock;
use snap_core::settings::{General, Theme};
use snap_ui::ThemeSpec;

static CURRENT: RwLock<Option<ThemeSpec>> = RwLock::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(1);
/// The user's "reduce motion" setting; the OS preference is handled inside `snap_ui::install`.
static REDUCE_MOTION: AtomicBool = AtomicBool::new(false);

/// Resolve the theme from the settings (`dark_override` is the dev harness knob) and publish it.
pub fn apply(general: &General, dark_override: Option<bool>) {
    let mut spec = ThemeSpec::from_settings(general);
    if let Some(dark) = dark_override {
        let accent = snap_ui::parse_hex(&general.accent).unwrap_or(ThemeSpec::brand(dark).accent);
        spec = ThemeSpec { dark, accent };
    }
    *CURRENT.write() = Some(spec);
    REDUCE_MOTION.store(general.reduce_motion, Ordering::SeqCst);
    GENERATION.fetch_add(1, Ordering::SeqCst);
}

/// Follow the OS light/dark switch when the theme is "system". Cheap enough to call whenever a window
/// gains focus; windows pick the change up before their next frame. Returns whether the theme changed.
pub fn follow_system(general: &General, dark_override: Option<bool>) -> bool {
    if general.theme != Theme::System || dark_override.is_some() {
        return false;
    }
    let spec = ThemeSpec::from_settings(general);
    let changed = CURRENT.read().is_none_or(|current| current != spec);
    if changed {
        *CURRENT.write() = Some(spec);
        GENERATION.fetch_add(1, Ordering::SeqCst);
    }
    changed
}

/// For `ShellConfig::theme_init`: a brand-new context.
pub fn init(ctx: &egui::Context) {
    let spec = (*CURRENT.read()).unwrap_or_default();
    snap_ui::install(ctx, &spec);
    if REDUCE_MOTION.load(Ordering::SeqCst) {
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
    }
}

/// Re-install the theme if the settings changed since `applied` was recorded.
pub fn sync(ctx: &egui::Context, applied: &mut u64) {
    let generation = GENERATION.load(Ordering::SeqCst);
    if *applied != generation {
        *applied = generation;
        init(ctx);
    }
}

/// The generation a context created right now starts at.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::SeqCst)
}
