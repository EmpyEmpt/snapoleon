//! Chinese, Japanese and Korean glyphs from a font the OS already ships.
//!
//! The UI fonts carry no CJK, and bundling one would add many megabytes. Instead a window that is about to
//! show such text calls [`ensure_cjk`]: the first time a script turns up, a system font for it is read
//! and appended to the font chains of that window's context. A window without CJK text never pays for it,
//! and the memory goes back when the window closes.

use std::path::PathBuf;

use egui::{Context, FontData, FontFamily, Id};

use crate::fonts;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Script {
    Kana,
    Hangul,
    Han,
}

impl Script {
    /// Kana and Hangul come first in the chain so Han characters in Japanese or Korean text take that
    /// language's glyph shapes.
    const CHAIN_ORDER: [Script; 3] = [Script::Kana, Script::Hangul, Script::Han];

    fn of(c: char) -> Option<Script> {
        match c as u32 {
            0x3040..=0x30FF | 0x31F0..=0x31FF => Some(Script::Kana),
            0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF => Some(Script::Hangul),
            0x3000..=0x303F | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF | 0x20000..=0x2FA1F => {
                Some(Script::Han)
            }
            _ => None,
        }
    }

    fn bit(self) -> u8 {
        1 << self as u8
    }

    fn font_name(self) -> String {
        format!("System-{self:?}")
    }

    /// `fc-match` pattern that prefers a font covering the language.
    #[cfg(not(any(windows, target_os = "macos")))]
    fn pattern(self) -> &'static str {
        match self {
            Script::Kana => "sans-serif:lang=ja",
            Script::Hangul => "sans-serif:lang=ko",
            Script::Han => "sans-serif:lang=zh-cn",
        }
    }

    /// Installed font files, best first, as (path, face index in the collection).
    #[cfg(any(windows, target_os = "macos"))]
    fn candidates(self) -> Vec<(PathBuf, u32)> {
        #[cfg(windows)]
        let dir = PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into())).join("Fonts");
        #[cfg(windows)]
        let files: &[&str] = match self {
            Script::Kana => &["YuGothM.ttc", "meiryo.ttc", "msgothic.ttc"],
            Script::Hangul => &["malgun.ttf", "gulim.ttc"],
            Script::Han => &["msyh.ttc", "simsun.ttc", "YuGothM.ttc"],
        };
        #[cfg(target_os = "macos")]
        let dir = PathBuf::from("/System/Library/Fonts");
        #[cfg(target_os = "macos")]
        let files: &[&str] = match self {
            Script::Kana => &["ヒラギノ角ゴシック W3.ttc", "Hiragino Sans GB.ttc"],
            Script::Hangul => &["AppleSDGothicNeo.ttc"],
            Script::Han => &["PingFang.ttc", "Hiragino Sans GB.ttc", "STHeiti Light.ttc"],
        };
        files.iter().map(|f| (dir.join(f), 0)).collect()
    }

    /// Installed font files, best first, as (path, face index in the collection).
    #[cfg(not(any(windows, target_os = "macos")))]
    fn candidates(self) -> Vec<(PathBuf, u32)> {
        let out = std::process::Command::new("fc-match")
            .args(["-f", "%{file}\n%{index}", self.pattern()])
            .output()
            .ok()
            .filter(|o| o.status.success());
        let Some(out) = out else { return Vec::new() };
        let text = String::from_utf8_lossy(&out.stdout);
        let mut lines = text.lines();
        let file = lines.next().map(PathBuf::from);
        let index = lines.next().and_then(|i| i.trim().parse().ok()).unwrap_or(0);
        file.map(|f| vec![(f, index)]).unwrap_or_default()
    }

    fn load(self) -> Option<FontData> {
        let (path, index) = self.candidates().into_iter().find(|(p, _)| p.is_file())?;
        let bytes = std::fs::read(path).ok()?;
        let mut data = FontData::from_owned(bytes);
        data.index = index;
        Some(data)
    }
}

/// Makes sure `ctx` can draw the CJK scripts present in `text`. Cheap when `text` has none or the scripts
/// are already loaded; call it every frame with the text about to be shown.
pub fn ensure_cjk(ctx: &Context, text: &str) {
    let id = Id::unique("snap-ui/cjk-fonts");
    let have: u8 = ctx.data(|d| d.get_temp(id)).unwrap_or(0);
    let wanted = text.chars().filter_map(Script::of).fold(0u8, |acc, s| acc | s.bit());
    if wanted & !have == 0 {
        return;
    }
    // Remember the attempt even when no font is installed, so a missing font is looked for only once.
    let now = have | wanted;
    ctx.data_mut(|d| d.insert_temp(id, now));

    let mut defs = fonts::definitions();
    let mut added = Vec::new();
    for script in Script::CHAIN_ORDER.into_iter().filter(|s| now & s.bit() != 0) {
        if let Some(data) = script.load() {
            let name = script.font_name();
            defs.font_data.insert(name.clone(), data.into());
            added.push(name);
        }
    }
    if added.is_empty() {
        return;
    }
    for family in [FontFamily::Proportional, FontFamily::Monospace, fonts::semibold(), fonts::bold()] {
        defs.families.entry(family).or_default().extend(added.iter().cloned());
    }
    ctx.set_fonts(defs);
    ctx.request_repaint();
}
