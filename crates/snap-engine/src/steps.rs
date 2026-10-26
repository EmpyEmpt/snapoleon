//! Step capture ("docs mode"): a session of screenshots with captions and
//! click points, exported as Markdown, a self-contained HTML guide, an MP4
//! slideshow, or rewritten into a how-to by the AI.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
    thread::JoinHandle,
};

use base64::Engine as _;
use crossbeam_channel::{Receiver, Sender};
use snap_capture::{ElementInfo, MonitorInfo, WindowInfo};
use snap_core::{CaptureContext, Error, Point, Rect, Result, RgbaImage, now_ms, settings::ImageFormat, t, tf};

use crate::{Engine, platform};

mod html;
mod render;

/// Clicks closer together than this belong to one gesture (double-click): one step, not two.
const DEBOUNCE_MS: i64 = 350;
/// Windows smaller than this (either side) are not worth cropping to; the whole monitor is kept.
const MIN_WINDOW_CROP: u32 = 64;
/// Longest element name quoted in a caption.
const MAX_NAME_CHARS: usize = 60;
/// How long the title card of the slideshow stays up.
const TITLE_CARD_MS: u64 = 3000;
const VIDEO_FPS: u32 = 30;
/// Embedded images use WebP when it is clearly smaller than PNG.
const HTML_WEBP_QUALITY: u8 = 92;

#[derive(Debug, Clone)]
pub struct StepShot {
    pub image: RgbaImage,
    pub caption: String,
    /// Click point in image pixels, drawn as a ring in exports.
    pub click: Option<Point>,
    pub context: CaptureContext,
    pub taken_at_ms: i64,
}

#[derive(Debug, Clone, Default)]
pub struct Guide {
    pub title: String,
    pub intro: String,
    pub steps: Vec<StepShot>,
}

// --- export -----------------------------------------------------------------------------------------------------

/// Writes `<dir>/<slug>.md` plus `<dir>/<slug>_images/NN.png`; returns the .md path.
pub fn export_markdown(guide: &Guide, dir: &Path) -> Result<PathBuf> {
    let slug = slug(&guide.title);
    let images_dir = format!("{slug}_images");
    std::fs::create_dir_all(dir.join(&images_dir))?;
    let digits = guide.steps.len().to_string().len().max(2);
    let mut md = String::new();
    md.push_str(&format!("# {}\n\n", display_title(guide)));
    if !guide.intro.trim().is_empty() {
        md.push_str(&format!("{}\n\n", guide.intro.trim()));
    }
    for (i, step) in guide.steps.iter().enumerate() {
        let name = format!("{:0digits$}.png", i + 1);
        snap_image::encode::save(
            &render::with_click_ring(&step.image, step.click),
            &dir.join(&images_dir).join(&name),
            ImageFormat::Png,
            100,
            false,
            None,
        )?;
        let caption = step.caption.trim();
        md.push_str(&format!("## {}. {caption}\n\n![{}]({images_dir}/{name})\n\n", i + 1, markdown_alt(caption)));
    }
    let path = dir.join(format!("{slug}.md"));
    std::fs::write(&path, md)?;
    Ok(path)
}

/// One HTML file with images inlined as data URIs.
pub fn export_html(guide: &Guide, path: &Path) -> Result<()> {
    let images =
        guide.steps.iter().map(|step| embed_image(&render::with_click_ring(&step.image, step.click))).collect::<Result<Vec<_>>>()?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, html::render(guide, &images))?;
    Ok(())
}

/// MP4 slideshow, `secs_per_step` per shot, captions burned in.
pub fn export_video(guide: &Guide, path: &Path, secs_per_step: u32) -> Result<()> {
    let Some(first) = guide.steps.first() else {
        return Err(Error::invalid(t("engine.error.empty_guide")));
    };
    let hold = u64::from(secs_per_step.max(1)) * 1000;
    let mut slides = Vec::with_capacity(guide.steps.len() + 1);
    if !guide.title.trim().is_empty() {
        let (w, h) = first.image.dimensions();
        slides.push((render::title_card(w, h, guide.title.trim(), guide.intro.trim()), TITLE_CARD_MS));
    }
    for (i, step) in guide.steps.iter().enumerate() {
        let frame = render::with_click_ring(&step.image, step.click);
        slides.push((render::caption_slide(&frame, step.caption.trim(), i + 1), hold));
    }
    snap_record::slideshow::render(&slides, path, VIDEO_FPS)
}

/// Rewrite title, intro and captions with the AI.
pub fn ai_howto(engine: &Engine, guide: &mut Guide) -> Result<()> {
    let steps: Vec<snap_ai::HowToStep> = guide
        .steps
        .iter()
        .map(|s| snap_ai::HowToStep {
            caption: s.caption.clone(),
            text: describe(&s.context),
            image: Some(render::with_click_ring(&s.image, s.click)),
        })
        .collect();
    let howto = engine.ai().howto(&steps)?;
    guide.title = howto.title;
    guide.intro = howto.intro;
    for (step, caption) in guide.steps.iter_mut().zip(howto.steps) {
        step.caption = caption;
    }
    Ok(())
}

/// "App, window title" for the AI prompt.
fn describe(cx: &CaptureContext) -> Option<String> {
    let parts: Vec<&str> = [cx.app_name.as_deref(), cx.window_title.as_deref()].into_iter().flatten().collect();
    (!parts.is_empty()).then(|| parts.join(": "))
}

fn display_title(guide: &Guide) -> String {
    let title = guide.title.trim();
    if title.is_empty() { t("engine.guide.default_title").into_owned() } else { title.to_string() }
}

/// File-name-safe, lower-case form of the title; letters of any script survive.
fn slug(title: &str) -> String {
    let mut slug = String::new();
    for c in display_title(&Guide { title: title.to_string(), ..Guide::default() }).chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug: String = slug.trim_end_matches('-').chars().take(60).collect();
    if slug.is_empty() { "guide".to_string() } else { slug }
}

fn markdown_alt(s: &str) -> String {
    s.replace('\\', "\\\\").replace('[', "\\[").replace(']', "\\]")
}

/// `data:` URI of the smaller of PNG and lossy WebP.
fn embed_image(img: &RgbaImage) -> Result<String> {
    let png = snap_image::encode::encode(img, ImageFormat::Png, 100, false, None)?;
    let webp =
        snap_image::encode::encode(img, ImageFormat::Webp, HTML_WEBP_QUALITY, false, None).ok().filter(|w| w.len() * 5 < png.len() * 4);
    let (mime, bytes) = match &webp {
        Some(w) => (ImageFormat::Webp.mime(), w),
        None => (ImageFormat::Png.mime(), &png),
    };
    Ok(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

// --- recording a session ----------------------------------------------------------------------------------------

/// A running session. Captures automatically on each mouse click where the
/// platform allows (the shot is taken just before the click lands), and on
/// demand via [`StepRecorder::snap_now`].
pub struct StepRecorder {
    shared: Arc<Shared>,
    jobs: Option<Sender<Job>>,
    worker: Option<JoinHandle<()>>,
    /// Stopped (and its thread joined) before the worker, so no click arrives after the job queue closes.
    listener: Option<platform::ClickListener>,
}

struct Shared {
    engine: Engine,
    paused: AtomicBool,
    last_click_ms: AtomicI64,
}

enum Job {
    /// A click whose screen was already grabbed on the hook thread; element lookup is under way.
    Click { at: Point, snapshot: Snapshot, elements: JoinHandle<Vec<ElementInfo>> },
    /// An explicit shot of the screen the cursor is on.
    Now,
}

/// The monitor under the pointer and the windows on the desktop at the moment of the click.
struct Snapshot {
    image: RgbaImage,
    monitor: MonitorInfo,
    windows: Vec<WindowInfo>,
    taken_at_ms: i64,
}

impl StepRecorder {
    /// `on_shot` runs on a worker thread.
    pub fn start(engine: &Engine, auto_on_click: bool, on_shot: impl Fn(StepShot) + Send + Sync + 'static) -> Result<Self> {
        let shared = Arc::new(Shared { engine: engine.clone(), paused: AtomicBool::new(false), last_click_ms: AtomicI64::new(0) });
        let (tx, rx) = crossbeam_channel::unbounded();
        let listener = if auto_on_click { listen(&shared, &tx)? } else { None };
        let worker = {
            let shared = shared.clone();
            std::thread::Builder::new().name("snap-step-shots".into()).spawn(move || work(&shared, rx, on_shot))?
        };
        Ok(Self { shared, jobs: Some(tx), worker: Some(worker), listener })
    }

    /// Take a shot of the current screen now (the shortcut for apps where clicks cannot be observed).
    pub fn snap_now(&self) {
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(Job::Now);
        }
    }

    /// Paused sessions ignore clicks; [`snap_now`](Self::snap_now) still works.
    pub fn pause(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Relaxed);
    }

    /// Stops listening and waits for the shots still being processed, so every one reaches `on_shot`.
    pub fn stop(mut self) {
        self.listener = None;
        self.jobs = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Install the click listener, or run without one where the system offers none.
fn listen(shared: &Arc<Shared>, jobs: &Sender<Job>) -> Result<Option<platform::ClickListener>> {
    let (shared, jobs) = (shared.clone(), jobs.clone());
    match platform::listen_clicks(move |at| on_click(&shared, &jobs, at)) {
        Ok(listener) => Ok(Some(listener)),
        Err(Error::Unsupported(why)) => {
            tracing::warn!("steps are not captured on click here: {why}");
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// Runs on the hook thread, before the click reaches its target on Windows: grab the screen now, think later.
fn on_click(shared: &Shared, jobs: &Sender<Job>, at: Point) {
    if shared.paused.load(Ordering::Relaxed) {
        return;
    }
    let now = now_ms();
    if now - shared.last_click_ms.swap(now, Ordering::Relaxed) < DEBOUNCE_MS || platform::is_own_window_at(at) {
        return;
    }
    // The accessibility lookup is slow; start it before the click changes the UI under the pointer.
    let elements = std::thread::spawn(move || snap_capture::elements_at(at).unwrap_or_default());
    match grab(&shared.engine, at) {
        Ok(snapshot) => {
            let _ = jobs.send(Job::Click { at, snapshot, elements });
        }
        Err(e) => tracing::warn!("step shot failed: {e}"),
    }
}

fn grab(engine: &Engine, at: Point) -> Result<Snapshot> {
    let monitors = snap_capture::monitors()?;
    let monitor = monitors
        .iter()
        .find(|m| m.rect.contains(at))
        .or_else(|| monitors.iter().find(|m| m.is_primary))
        .or(monitors.first())
        .cloned()
        .ok_or_else(|| Error::NotFound(t("engine.error.no_monitor").into_owned()))?;
    let cap = snap_capture::capture_monitor(monitor.index, &engine.capture_options())?;
    let windows = snap_capture::windows().unwrap_or_default();
    Ok(Snapshot { image: cap.image, monitor, windows, taken_at_ms: cap.taken_at_ms })
}

fn work(shared: &Shared, jobs: Receiver<Job>, on_shot: impl Fn(StepShot)) {
    for job in jobs {
        let shot = match job {
            Job::Click { at, snapshot, elements } => {
                let elements = elements.join().unwrap_or_default();
                Ok(shoot(&snapshot, Some(at), &elements))
            }
            Job::Now => {
                let at = snap_capture::cursor_position();
                grab(&shared.engine, at).map(|snapshot| shoot(&snapshot, None, &[]))
            }
        };
        match shot {
            Ok(shot) => on_shot(shot),
            Err(e) => tracing::warn!("step shot failed: {e}"),
        }
    }
}

/// Crop the snapshot to the window under the click (or the foreground window for manual shots) and caption it.
fn shoot(snap: &Snapshot, click: Option<Point>, elements: &[ElementInfo]) -> StepShot {
    let monitor_rect = snap.monitor.rect;
    let window = match click {
        Some(at) => snap.windows.iter().find(|w| !w.minimized && w.rect.contains(at)),
        None => snap.windows.iter().find(|w| !w.minimized && w.rect.intersect(monitor_rect).is_some()),
    };
    let crop = window
        .and_then(|w| w.rect.intersect(monitor_rect))
        .filter(|r| r.w >= MIN_WINDOW_CROP && r.h >= MIN_WINDOW_CROP)
        .unwrap_or(monitor_rect);
    let local: Rect = crop.relative_to(monitor_rect.origin());
    let image = image::imageops::crop_imm(&snap.image, local.x.max(0) as u32, local.y.max(0) as u32, local.w, local.h).to_image();
    let mut context = window.map(snap_capture::context_for_window).unwrap_or_default();
    context.monitor_name = Some(snap.monitor.name.clone()).filter(|n| !n.is_empty());
    StepShot {
        caption: caption(click.is_some(), elements, window, &context),
        click: click.map(|at| Point::new(at.x - crop.x, at.y - crop.y)),
        image,
        context,
        taken_at_ms: snap.taken_at_ms,
    }
}

/// `Click "Save" in Notepad`, built from the element under the pointer (smallest named one first).
fn caption(clicked: bool, elements: &[ElementInfo], window: Option<&WindowInfo>, context: &CaptureContext) -> String {
    let app = window
        .map(|w| w.app_name.clone())
        .filter(|a| !a.trim().is_empty())
        .or_else(|| context.app_name.clone())
        .or_else(|| elements.iter().find_map(|e| e.app_name.clone()))
        .filter(|a| !a.trim().is_empty());
    let app_arg = app.as_deref().unwrap_or_default();
    if !clicked {
        return match (&app, &context.window_title) {
            (Some(app), Some(title)) => tf("engine.steps.caption.view_titled", &[("app", app), ("title", title)]),
            (Some(app), None) => tf("engine.steps.caption.view", &[("app", app)]),
            _ => t("engine.steps.caption.view_screen").into_owned(),
        };
    }
    if let Some(named) = elements.iter().find(|e| !e.name.trim().is_empty()) {
        let name = tidy(&named.name);
        return if app.is_some() {
            tf("engine.steps.caption.click_named", &[("name", &name), ("app", app_arg)])
        } else {
            tf("engine.steps.caption.click_named_bare", &[("name", &name)])
        };
    }
    match (elements.first(), &app) {
        (Some(e), Some(app)) => {
            tf("engine.steps.caption.click_role", &[("role", &snap_capture::role_label(&e.role).to_lowercase()), ("app", app)])
        }
        (_, Some(app)) => tf("engine.steps.caption.click_app", &[("app", app)]),
        _ => t("engine.steps.caption.click_desktop").into_owned(),
    }
}

/// Collapse whitespace and cap the length of an accessible name.
fn tidy(name: &str) -> String {
    let flat = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MAX_NAME_CHARS {
        return flat;
    }
    let cut: String = flat.chars().take(MAX_NAME_CHARS - 1).collect();
    format!("{}\u{2026}", cut.trim_end())
}
