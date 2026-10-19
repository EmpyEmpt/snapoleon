//! Exporting a guide (Markdown folder, HTML file, MP4 slideshow), copying it as Markdown text and
//! rewriting it with the AI. Everything runs on worker threads and reports back as view messages;
//! long exports show the app's progress pill.

use std::path::{Path, PathBuf};

use snap_core::{Result, t, tf};
use snap_engine::{
    Engine,
    steps::{self, Guide},
};

use crate::{events::AppEvent, views::ViewCx};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Markdown,
    Html,
    Video,
}

impl Kind {
    fn progress_label(self) -> String {
        t(match self {
            Kind::Markdown => "steps.export.writing_markdown",
            Kind::Html => "steps.export.writing_html",
            Kind::Video => "steps.export.rendering_video",
        })
        .into_owned()
    }
}

/// A finished export.
pub struct Written {
    pub kind: Kind,
    pub path: PathBuf,
}

/// New wording from the AI, matched to steps by id.
pub struct Rewrite {
    pub title: String,
    pub intro: String,
    pub captions: Vec<(u64, String)>,
}

/// Ask where to put the file, then write it. Resolves to `None` when the dialog was dismissed.
pub fn export(cx: &ViewCx, kind: Kind, guide: Guide, secs_per_step: u32) {
    let proxy = cx.proxy.clone();
    cx.spawn(move || -> Result<Option<Written>> {
        let stem = file_stem(&guide.title);
        let target = match kind {
            Kind::Markdown => rfd::FileDialog::new().set_title(t("steps.export.folder_dialog")).pick_folder(),
            Kind::Html => rfd::FileDialog::new().set_file_name(format!("{stem}.html")).add_filter("HTML", &["html"]).save_file(),
            Kind::Video => rfd::FileDialog::new().set_file_name(format!("{stem}.mp4")).add_filter("MP4", &["mp4"]).save_file(),
        };
        let Some(target) = target else { return Ok(None) };
        proxy.send(AppEvent::JobProgress { label: kind.progress_label(), frac: None });
        let written = match kind {
            Kind::Markdown => steps::export_markdown(&guide, &target),
            Kind::Html => steps::export_html(&guide, &target).map(|()| target),
            Kind::Video => steps::export_video(&guide, &target, secs_per_step).map(|()| target),
        };
        proxy.send(AppEvent::JobProgress { label: String::new(), frac: None });
        written.map(|path| Some(Written { kind, path }))
    });
}

/// Rewrite the title, intro and captions with the configured AI.
pub fn rewrite(cx: &ViewCx, engine: Engine, mut guide: Guide, ids: Vec<u64>) {
    cx.spawn(move || -> Result<Rewrite> {
        steps::ai_howto(&engine, &mut guide)?;
        let captions = ids.into_iter().zip(guide.steps.into_iter().map(|s| s.caption)).collect();
        Ok(Rewrite { title: guide.title, intro: guide.intro, captions })
    });
}

/// The guide as Markdown text: a heading, the intro and the numbered captions. Images stay in the
/// exported files.
pub fn markdown_text(title: &str, intro: &str, captions: &[&str]) -> String {
    let mut out = String::new();
    if !title.trim().is_empty() {
        out.push_str(&format!("# {}\n\n", title.trim()));
    }
    if !intro.trim().is_empty() {
        out.push_str(intro.trim());
        out.push_str("\n\n");
    }
    for (i, caption) in captions.iter().enumerate() {
        let caption =
            if caption.trim().is_empty() { tf("steps.step_n", &[("n", &(i + 1).to_string())]) } else { caption.trim().to_owned() };
        out.push_str(&format!("{}. {}\n", i + 1, caption));
    }
    out
}

/// A file name from the guide title; a generic one when the title is empty.
fn file_stem(title: &str) -> String {
    let cleaned: String = title.trim().chars().map(|c| if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_') { c } else { '_' }).collect();
    if cleaned.is_empty() { t("steps.file_name").into_owned() } else { cleaned }
}

/// The file's name, for the "Saved ..." toast.
pub fn display_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}
