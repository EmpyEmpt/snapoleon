//! Dev harness for view development. See docs/APP.md.
//!
//! `SNAPOLEON_DEV=<view>` opens one view at startup with realistic demo data and skips everything
//! global (tray, hotkeys, IPC, autostart, URL scheme), so several dev instances can run side by
//! side and next to the installed app. Data lives in a `data` folder next to the executable
//! (portable mode) unless `SNAPOLEON_DEV_SHARED_DATA` is set.
//!
//! - `SNAPOLEON_DEV_PAGE=<page>`: settings page (`general`, `hotkeys`, ...)
//! - `SNAPOLEON_DEV_MODE=<mode>`: overlay mode (`region`, `window`, `ocr`, `color`, ...)
//! - `SNAPOLEON_DEV_VIDEO=<path>`: video for the `video` view
//! - `SNAPOLEON_DEV_THEME=light|dark`, `SNAPOLEON_DEV_LANG=en|ru`
//! - `SNAPOLEON_DEV_SHOT=<path.png>`: save a screenshot of the view's first window once it has
//!   rendered for `SNAPOLEON_DEV_SHOT_DELAY_MS` (default 1500), then quit.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use image::imageops;
use snap_capture::CaptureOptions;
use snap_core::{
    Capture, CaptureContext, CaptureKind, Point, Rect, RgbaImage, Settings,
    settings::{ImageFormat, VideoFormat},
};
use snap_engine::{Engine, RecordTarget};
use snap_history::Entry;
use snap_shell::{Shell, WindowKey};

use crate::{
    app::App,
    commands::Command,
    events::AppEvent,
    views::{
        self,
        colors::{ColorSample, ColorToolInput},
        editor::EditorInput,
        overlay::{OverlayMode, OverlayRequest},
        recorder::RecorderInput,
        scroll::ScrollInput,
        settings::SettingsPage,
    },
};

pub const VIEWS: [&str; 15] = [
    "overlay",
    "editor",
    "pin",
    "recorder",
    "video",
    "library",
    "text",
    "settings",
    "onboarding",
    "about",
    "steps",
    "combine",
    "diff",
    "scroll",
    "colors",
];

#[derive(Debug, Clone)]
pub struct Dev {
    pub view: String,
    page: Option<String>,
    mode: Option<String>,
    video: Option<PathBuf>,
    shot: Option<PathBuf>,
    shot_delay: Duration,
    pub dark: Option<bool>,
    pub lang: Option<String>,
    /// The first window the view opened; the one the screenshot is taken of.
    target: Option<WindowKey>,
}

impl Dev {
    pub fn from_env() -> Option<Dev> {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        let view = var("SNAPOLEON_DEV")?;
        Some(Dev {
            view,
            page: var("SNAPOLEON_DEV_PAGE"),
            mode: var("SNAPOLEON_DEV_MODE"),
            video: var("SNAPOLEON_DEV_VIDEO").map(PathBuf::from),
            shot: var("SNAPOLEON_DEV_SHOT").map(PathBuf::from),
            shot_delay: Duration::from_millis(var("SNAPOLEON_DEV_SHOT_DELAY_MS").and_then(|v| v.parse().ok()).unwrap_or(1500)),
            dark: var("SNAPOLEON_DEV_THEME").map(|v| !v.eq_ignore_ascii_case("light")),
            lang: var("SNAPOLEON_DEV_LANG"),
            target: None,
        })
    }
}

/// Switch to portable mode (a `data` folder next to the executable) before anything reads a path.
pub fn prepare() {
    if std::env::var_os("SNAPOLEON_DEV").is_none() || std::env::var_os("SNAPOLEON_DEV_SHARED_DATA").is_some() {
        return;
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|p| p.join("data"))) {
        let _ = std::fs::create_dir_all(dir);
    }
}

impl App {
    /// Open the requested view with demo data and schedule the screenshot.
    pub(crate) fn dev_start(&mut self, sh: &mut Shell<AppEvent>) {
        let Some(dev) = self.dev.clone() else { return };
        let screen = demo_screenshot();
        let engine = self.engine.clone();
        let crop = |i: usize| screen.as_ref().map(|s| demo_crop(s, i));
        let opened = match dev.view.as_str() {
            "overlay" => {
                let request = OverlayRequest { mode: overlay_mode(dev.mode.as_deref()), pipeline: None };
                self.open_windows(sh, |cx| views::overlay::open(cx, request))
            }
            "editor" => match screen {
                Some(image) => {
                    let mut capture = Capture::new(CaptureKind::Region, image, Rect::default(), 1.0);
                    capture.screen_rect = Rect::new(0, 0, capture.image.width(), capture.image.height());
                    capture.context = demo_context(0);
                    self.open_windows(sh, |cx| views::editor::open(cx, EditorInput::from_capture(capture)))
                }
                None => Vec::new(),
            },
            "pin" => match crop(0) {
                Some(image) => self.open_windows(sh, |cx| views::pin::open(cx, image, None)),
                None => Vec::new(),
            },
            "recorder" => {
                let input = RecorderInput { target: RecordTarget::Region(Rect::new(160, 160, 800, 450)), format: VideoFormat::Mp4 };
                self.open_windows(sh, |cx| views::recorder::open(cx, input))
            }
            "video" => {
                let path = dev.video.clone().or_else(|| demo_video(&crop));
                match path {
                    Some(path) => self.open_windows(sh, |cx| views::video::open(cx, path)),
                    None => {
                        tracing::warn!("no demo video: pass SNAPOLEON_DEV_VIDEO=<path>");
                        Vec::new()
                    }
                }
            }
            "library" => {
                seed_history(&engine, screen.as_ref());
                self.open_windows(sh, views::library::open)
            }
            "text" => self.open_windows(sh, |cx| views::text::open(cx, "Recognised text".into(), DEMO_TEXT.into())),
            "settings" => {
                let page = dev.page.as_deref().and_then(SettingsPage::from_key);
                self.open_windows(sh, |cx| views::settings::open(cx, page))
            }
            "onboarding" => self.open_windows(sh, views::onboarding::open),
            "about" => self.open_windows(sh, views::about::open),
            "steps" => self.open_windows(sh, views::steps::open),
            "combine" => {
                let images: Vec<(RgbaImage, f32)> = (0..3).filter_map(&crop).map(|image| (image, 1.0)).collect();
                self.open_windows(sh, |cx| views::combine::open(cx, images))
            }
            "diff" => {
                let a = crop(1);
                let b = a.clone().map(|mut image| {
                    let (w, h) = image.dimensions();
                    imageops::overlay(
                        &mut image,
                        &RgbaImage::from_pixel(w / 4, h / 5, image::Rgba([255, 90, 95, 255])),
                        i64::from(w / 3),
                        i64::from(h / 3),
                    );
                    image
                });
                self.open_windows(sh, |cx| views::diff::open(cx, a, b))
            }
            "scroll" => {
                let input = ScrollInput { rect: Rect::new(160, 160, 900, 600), pipeline: None };
                self.open_windows(sh, |cx| views::scroll::open(cx, input))
            }
            "colors" => {
                let sample = ColorSample { rgba: [61, 90, 254, 255], at: Point::new(640, 360) };
                self.open_windows(sh, |cx| views::colors::open(cx, ColorToolInput::Picked(sample)))
            }
            other => {
                eprintln!("unknown SNAPOLEON_DEV view '{other}'; views: {}", VIEWS.join(", "));
                sh.quit();
                return;
            }
        };
        if let Some(dev) = &mut self.dev {
            dev.target = opened.first().copied();
        }
        if opened.is_empty() {
            eprintln!("the '{}' view opened no window", dev.view);
            sh.quit();
            return;
        }
        if dev.shot.is_some() {
            let proxy = sh.proxy();
            std::thread::spawn(move || {
                std::thread::sleep(dev.shot_delay);
                proxy.send(AppEvent::DevShot);
            });
        }
    }

    /// Screenshot the dev view's window, then quit.
    pub(crate) fn dev_shot(&mut self, sh: &mut Shell<AppEvent>) {
        let Some((path, target)) = self.dev.as_ref().and_then(|d| Some((d.shot.clone()?, d.target?))) else { return };
        let proxy = sh.proxy();
        if !sh.exists(target) {
            eprintln!("dev shot: the window is gone");
            proxy.send(AppEvent::Command(Command::Quit));
            return;
        }
        // HUDs hide themselves from captures; the harness needs to see them.
        let _ = sh.set_exclude_from_capture(target, false);
        let (native, rect) = (sh.native_id(target), sh.outer_rect(target));
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(250));
            match take_shot(native, rect, &path) {
                Ok(how) => println!("dev shot: {} ({how})", path.display()),
                Err(e) => eprintln!("dev shot failed: {e}"),
            }
            proxy.send(AppEvent::Command(Command::Quit));
        });
    }
}

/// Capture the window by its native id; fall back to cropping the screen at its outer rect.
fn take_shot(native: Option<u64>, rect: Option<Rect>, path: &Path) -> snap_core::Result<&'static str> {
    let options = CaptureOptions::default();
    let by_window =
        native.and_then(|id| snap_capture::capture_window(id, &options).map_err(|e| tracing::warn!("window capture: {e}")).ok());
    let (capture, how) = match (by_window, rect) {
        (Some(capture), _) => (capture, "window capture"),
        (None, Some(rect)) => (snap_capture::capture_rect(rect, &options)?, "screen crop"),
        (None, None) => return Err(snap_core::Error::NotFound("window".into())),
    };
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    snap_image::encode::save(&capture.image, path, ImageFormat::Png, 90, false, Some(capture.scale_factor))?;
    Ok(how)
}

fn overlay_mode(name: Option<&str>) -> OverlayMode {
    match name.unwrap_or("region") {
        "window" => OverlayMode::Window,
        "element" => OverlayMode::Element,
        "ocr" => OverlayMode::Ocr,
        "codes" => OverlayMode::Codes,
        "translate" => OverlayMode::Translate,
        "pin" => OverlayMode::Pin,
        "record" => OverlayMode::Record(VideoFormat::Mp4),
        "gif" => OverlayMode::Record(VideoFormat::Gif),
        "scroll" => OverlayMode::Scroll,
        "color" => OverlayMode::ColorPicker,
        "measure" => OverlayMode::Measure,
        _ => OverlayMode::Region,
    }
}

/// A real screenshot of the primary monitor, or the picture `SNAPOLEON_DEV_SCREEN` names: README and store screenshots
/// must not carry whatever happens to be on the developer's screen, and a fixed picture makes them reproducible.
fn demo_screenshot() -> Option<RgbaImage> {
    if let Some(path) = std::env::var_os("SNAPOLEON_DEV_SCREEN") {
        return image::open(&path).map(|i| i.to_rgba8()).map_err(|e| tracing::warn!("SNAPOLEON_DEV_SCREEN: {e}")).ok();
    }
    let monitors = snap_capture::monitors().ok()?;
    let primary = monitors.iter().find(|m| m.is_primary).or(monitors.first())?;
    snap_capture::capture_monitor(primary.index, &CaptureOptions::default()).ok().map(|c| c.image)
}

/// The `i`-th of a few fixed crops of `screen`, so combine/diff/pin have different pictures.
fn demo_crop(screen: &RgbaImage, i: usize) -> RgbaImage {
    let (w, h) = screen.dimensions();
    let (cw, ch) = ((w / 2).max(1), (h / 2).max(1));
    let origins = [(0, 0), (w - cw, 0), (0, h - ch), (w - cw, h - ch), (w / 4, h / 4)];
    let (x, y) = origins[i % origins.len()];
    imageops::crop_imm(screen, x, y, cw, ch).to_image()
}

fn demo_context(i: usize) -> CaptureContext {
    const APPS: [(&str, &str); 6] = [
        ("Firefox", "Snapoleon: screenshots without the weight"),
        ("Visual Studio Code", "main.rs - snapoleon"),
        ("Telegram", "Design review"),
        ("Windows Terminal", "cargo run -p snapoleon"),
        ("Figma", "Brand kit"),
        ("Notion", "Release checklist"),
    ];
    let (app, title) = APPS[i % APPS.len()];
    CaptureContext {
        app_name: Some(app.into()),
        window_title: Some(title.into()),
        url: (app == "Firefox").then(|| "https://snapoleon.app/".into()),
        ..CaptureContext::default()
    }
}

/// A short MP4 made from the demo crops, when the platform can encode one.
fn demo_video(crop: &dyn Fn(usize) -> Option<RgbaImage>) -> Option<PathBuf> {
    let slides: Vec<(RgbaImage, u64)> = (0..4).filter_map(|i| Some((crop(i)?, 800))).collect();
    let path = std::env::temp_dir().join("snapoleon-dev-demo.mp4");
    snap_record::slideshow::render(&slides, &path, 15).map_err(|e| tracing::warn!("demo video: {e}")).ok()?;
    Some(path)
}

/// Fill an empty history with a dozen believable entries (files, thumbnails, text, tags, links).
fn seed_history(engine: &Engine, screen: Option<&RgbaImage>) {
    let (Some(history), Some(screen)) = (engine.history(), screen) else { return };
    if history.count().unwrap_or(u64::MAX) >= 8 {
        return;
    }
    let dir = snap_core::paths::default_save_dir();
    let now = snap_core::now_ms();
    let kinds = [CaptureKind::Region, CaptureKind::Window, CaptureKind::Monitor, CaptureKind::Element, CaptureKind::Scrolling];
    for i in 0..14 {
        let image = demo_crop(screen, i);
        let mut capture = Capture::new(kinds[i % kinds.len()], image, Rect::default(), 1.0);
        capture.context = demo_context(i);
        capture.taken_at_ms = now - (i as i64) * 7 * 3_600_000;
        let file = dir.join(format!("Snapoleon demo {i:02}.png"));
        let bytes = snap_image::encode::save(&capture.image, &file, ImageFormat::Png, 90, false, Some(capture.scale_factor)).unwrap_or(0);
        let mut entry = Entry::from_capture(&capture, Some(file), bytes);
        entry.thumb = snap_history::write_thumbnail(&capture.image, &capture.id).ok();
        entry.text = Some(DEMO_TEXT.lines().skip(i % 4).take(3).collect::<Vec<_>>().join(" "));
        entry.favorite = i % 5 == 0;
        entry.tags = if i % 3 == 0 { vec!["design".into(), "review".into()] } else { Vec::new() };
        if i % 4 == 1 {
            entry.links.push(snap_history::LinkRecord {
                url: format!("https://snap.example.com/{i:x}k2m9"),
                destination: "team".into(),
                delete_url: None,
                created_ms: capture.taken_at_ms,
            });
        }
        if let Err(e) = history.add(&entry) {
            tracing::warn!("demo history entry: {e}");
        }
    }
}

const DEMO_TEXT: &str = "Quarterly review: 3 launch blockers remain.\nOwner: Anna Petrova, anna@example.com\nПроверить договор до пятницы и отправить скан.\nInvoice 2024-0417, total 1 284,50 EUR\nSee https://snapoleon.app/changelog for details.";

/// The settings the harness runs with: the stored ones plus the language override.
pub fn apply_overrides(settings: &mut Settings, dev: &Dev) {
    if let Some(lang) = &dev.lang {
        settings.general.language = lang.clone();
    }
}
