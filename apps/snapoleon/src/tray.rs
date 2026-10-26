//! The tray icon and its menu.

use chrono::{Local, TimeZone};
use image::RgbaImage;
use snap_core::{CaptureKind, Result, Settings, hotkey::Action, t};
use snap_engine::{Notice, NoticeKind};
use snap_history::Entry;
use snap_platform::{
    permissions::{self, Perm},
    tray::{Tray, TrayEvent, TrayItem},
};
use snap_shell::{Proxy, Shell};

use crate::{
    app::App,
    commands::Command,
    events::AppEvent,
    notices::error_text,
    views::{colors::ColorToolInput, settings::SettingsPage},
};

#[cfg(not(target_os = "macos"))]
const ICON_SIZE: u32 = 32;
/// macOS: the glyph is 18 pt on the 22 pt status item, drawn at @2x; the menu bar scales it for @1x screens.
#[cfg(target_os = "macos")]
const ICON_CANVAS: u32 = 44;
#[cfg(target_os = "macos")]
const ICON_GLYPH: u32 = 36;
const RECENT_LIMIT: usize = 8;
const LABEL_LIMIT: usize = 38;
/// How long the latest capture gets its quick actions at the top of the menu.
const FRESH_MS: i64 = 10 * 60 * 1000;

/// What a tray menu item stands for.
pub enum MenuAction {
    Action(Action),
    Recent(String),
    /// Copy a capture's image (a recording's file) to the clipboard.
    CopyCapture(String),
    /// Copy a capture's latest link, formatted per `settings.links.copy_format`.
    CopyLink(String),
    Library,
    Settings,
    About,
    ClosePins,
    UnlockPins,
    Combine,
    Compare,
    Colors,
    /// Shown while Screen Recording is off (macOS), without which no capture works.
    AllowScreenRecording,
    Quit,
}

impl MenuAction {
    pub fn parse(id: &str) -> Option<MenuAction> {
        match id {
            "library" => Some(MenuAction::Library),
            "settings" => Some(MenuAction::Settings),
            "about" => Some(MenuAction::About),
            "close_pins" => Some(MenuAction::ClosePins),
            "unlock_pins" => Some(MenuAction::UnlockPins),
            "combine" => Some(MenuAction::Combine),
            "compare" => Some(MenuAction::Compare),
            "colors" => Some(MenuAction::Colors),
            "allow_screen_recording" => Some(MenuAction::AllowScreenRecording),
            "quit" => Some(MenuAction::Quit),
            _ => match id.split_once(':')? {
                ("action", key) => Action::from_key(key).map(MenuAction::Action),
                ("recent", id) => Some(MenuAction::Recent(id.to_string())),
                ("copy_capture", id) => Some(MenuAction::CopyCapture(id.to_string())),
                ("copy_link", id) => Some(MenuAction::CopyLink(id.to_string())),
                _ => None,
            },
        }
    }
}

/// What the tray menu depends on besides the settings.
#[derive(Clone, Copy)]
pub struct State<'a> {
    pub recent: &'a [Entry],
    pub recording: bool,
    /// At least one pin is open.
    pub pins: bool,
}

pub fn create(proxy: Proxy<AppEvent>, settings: &Settings, state: State) -> Result<Tray> {
    let mut tray = Tray::new(&tooltip(state.recording), &icon(state.recording), menu(settings, state), move |event: TrayEvent| {
        proxy.send(AppEvent::Tray(event));
    })?;
    tray.set_recording(state.recording);
    Ok(tray)
}

/// Rebuild after settings, history or the recording state changed.
pub fn refresh(tray: &mut Tray, settings: &Settings, state: State) {
    tray.set_items(menu(settings, state));
    tray.set_icon(&icon(state.recording));
    tray.set_recording(state.recording);
    tray.set_tooltip(&tooltip(state.recording));
}

/// macOS: a black glyph for a template image, which the menu bar colours to match itself. Elsewhere: the glyph in
/// the taskbar's ink, with a red dot while recording.
fn icon(recording: bool) -> RgbaImage {
    #[cfg(target_os = "macos")]
    {
        let glyph = snap_ui::tray_icon_rgba_colored(ICON_GLYPH, recording, [0, 0, 0, 255]);
        let mut canvas = RgbaImage::new(ICON_CANVAS, ICON_CANVAS);
        let inset = i64::from((ICON_CANVAS - ICON_GLYPH) / 2);
        image::imageops::overlay(&mut canvas, &glyph, inset, inset);
        canvas
    }
    #[cfg(not(target_os = "macos"))]
    snap_ui::tray_icon_rgba(ICON_SIZE, recording)
}

pub fn tooltip(recording: bool) -> String {
    t(if recording { "app.tray.tooltip_recording" } else { "app.tray.tooltip" }).into_owned()
}

pub fn recent_limit() -> usize {
    RECENT_LIMIT
}

fn menu(settings: &Settings, State { recent, recording, pins }: State) -> Vec<TrayItem> {
    let action = |a: Action| {
        let accel = settings.hotkeys.iter().find(|b| b.enabled && b.action == a).map(|b| b.accel.clone());
        TrayItem::new(format!("action:{}", a.key()), t(&format!("action.{}", a.key()))).accel(accel)
    };
    let mut items = Vec::new();
    // macOS answers for Screen Recording once per process, so the item stays until the app restarts after the grant,
    // and Settings > Privacy, which it opens, offers that restart.
    if !permissions::granted(Perm::ScreenRecording) {
        items.push(TrayItem::new("allow_screen_recording", t("app.permission.allow_screen_recording")));
        items.push(TrayItem::separator());
    }
    if let Some(last) = recent.first().filter(|e| Local::now().timestamp_millis() - e.created_ms < FRESH_MS) {
        items.extend(quick_actions(last));
        items.push(TrayItem::separator());
    }
    if recording {
        items.push(action(Action::StopRecording));
        items.push(TrayItem::separator());
    }
    items.extend([
        action(Action::CaptureRegion),
        action(Action::CaptureWindow),
        action(Action::CaptureAllMonitors),
        action(Action::CaptureScrolling),
        TrayItem::separator(),
    ]);
    if !recording {
        items.extend([action(Action::RecordRegion), action(Action::RecordScreen), action(Action::RecordGif), TrayItem::separator()]);
    }
    items.extend([
        action(Action::OcrRegion),
        action(Action::DecodeCodesRegion),
        action(Action::ColorPicker),
        action(Action::PinRegion),
        tools_menu(&action),
        TrayItem::separator(),
    ]);
    if pins {
        items.extend([TrayItem::new("close_pins", t("app.tray.close_pins")), TrayItem::new("unlock_pins", t("app.tray.unlock_pins"))]);
        items.push(TrayItem::separator());
    }
    items.extend([
        recent_menu(recent),
        TrayItem::new("library", t("action.open_library")),
        TrayItem::new("settings", t("app.menu.settings")).accel(mac_accel("Cmd+Comma")),
        TrayItem::new("about", t("app.menu.about")),
        TrayItem::separator(),
        TrayItem::new("quit", t("app.tray.quit")).accel(mac_accel("Cmd+Q")),
    ]);
    items
}

/// The key equivalents every Mac menu shows for Settings and Quit. They work while the menu is open.
fn mac_accel(accel: &str) -> Option<String> {
    cfg!(target_os = "macos").then(|| accel.to_string())
}

/// Open, copy and copy the link of a capture taken a moment ago, without going through the Library.
fn quick_actions(entry: &Entry) -> Vec<TrayItem> {
    let mut items = vec![
        TrayItem::new(format!("recent:{}", entry.id), t("app.menu.open_last")),
        TrayItem::new(format!("copy_capture:{}", entry.id), t("app.menu.copy_last")),
    ];
    if !entry.links.is_empty() {
        items.push(TrayItem::new(format!("copy_link:{}", entry.id), t("app.menu.copy_last_link")));
    }
    items
}

fn tools_menu(action: &dyn Fn(Action) -> TrayItem) -> TrayItem {
    TrayItem::submenu(
        t("app.tray.tools"),
        vec![
            action(Action::Measure),
            TrayItem::new("colors", t("app.tray.colors")),
            action(Action::StepCaptureToggle),
            TrayItem::separator(),
            action(Action::OpenEditorFromClipboard),
            action(Action::PinClipboard),
            TrayItem::new("combine", t("app.tray.combine")),
            TrayItem::new("compare", t("app.tray.compare")),
        ],
    )
}

fn recent_menu(recent: &[Entry]) -> TrayItem {
    let children = if recent.is_empty() {
        vec![TrayItem::new("none", t("app.tray.no_recent")).disabled()]
    } else {
        recent.iter().map(|e| TrayItem::new(format!("recent:{}", e.id), recent_label(e))).collect()
    };
    TrayItem::submenu(t("app.tray.recent"), children)
}

fn recent_label(entry: &Entry) -> String {
    let what = entry
        .context
        .window_title
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .or(entry.context.app_name.as_deref())
        .map(str::to_string)
        .unwrap_or_else(|| snap_history::kind_label(entry.kind).into_owned());
    let what = if what.chars().count() > LABEL_LIMIT {
        format!("{}…", what.chars().take(LABEL_LIMIT).collect::<String>().trim_end())
    } else {
        what
    };
    let when = Local.timestamp_millis_opt(entry.created_ms).single().map_or_else(String::new, |time| {
        let format = if time.date_naive() == Local::now().date_naive() { "%H:%M" } else { "%d.%m %H:%M" };
        time.format(format).to_string()
    });
    format!("{what}  ·  {when}")
}

impl App {
    pub(crate) fn recent(&self) -> Vec<snap_history::Entry> {
        let query = snap_history::Query { limit: recent_limit() as u32, ..snap_history::Query::default() };
        self.engine.history().and_then(|h| h.search(&query).ok()).unwrap_or_default()
    }

    pub(crate) fn refresh_tray(&mut self) {
        if self.tray.is_none() {
            return;
        }
        let recent = self.recent();
        let state = State { recent: &recent, recording: self.recording, pins: !self.pins.is_empty() };
        if let Some(tray) = &mut self.tray {
            refresh(tray, &self.settings, state);
        }
    }

    pub(crate) fn on_tray(&mut self, sh: &mut Shell<AppEvent>, event: TrayEvent) {
        match event {
            TrayEvent::Click => {
                let action = self.settings.general.tray_click;
                self.start_action(sh, action, None);
            }
            TrayEvent::DoubleClick => {}
            TrayEvent::Menu(id) => match MenuAction::parse(&id) {
                Some(MenuAction::Action(action)) => self.start_action(sh, action, None),
                Some(MenuAction::Recent(id)) => self.open_recent(&id),
                Some(MenuAction::CopyCapture(id)) => self.copy_capture(sh, &id),
                Some(MenuAction::CopyLink(id)) => self.copy_capture_link(sh, &id),
                Some(MenuAction::Library) => self.cmds.push(Command::OpenLibrary),
                Some(MenuAction::Settings) => self.cmds.push(Command::OpenSettings { page: None }),
                Some(MenuAction::About) => self.cmds.push(Command::OpenAbout),
                Some(MenuAction::ClosePins) => self.cmds.push(Command::ClosePins),
                Some(MenuAction::UnlockPins) => self.cmds.push(Command::UnlockPins),
                Some(MenuAction::Combine) => self.cmds.push(Command::OpenCombine { images: Vec::new() }),
                Some(MenuAction::Compare) => self.cmds.push(Command::OpenDiff { a: None, b: None }),
                Some(MenuAction::Colors) => self.cmds.push(Command::OpenColorTool(ColorToolInput::Open)),
                Some(MenuAction::AllowScreenRecording) => {
                    // The system prompt while undecided, which also lists the app in System Settings; the Privacy
                    // page shows the state, the way to System Settings and the relaunch the grant needs.
                    permissions::request(Perm::ScreenRecording);
                    self.cmds.push(Command::OpenSettings { page: Some(SettingsPage::Privacy) });
                }
                Some(MenuAction::Quit) => self.cmds.push(Command::Quit),
                None => tracing::debug!("unknown tray item {id}"),
            },
        }
    }

    fn history_entry(&self, id: &str) -> Option<Entry> {
        self.engine.history().and_then(|h| h.get(&snap_core::CaptureId(id.to_string())).ok().flatten())
    }

    /// Run `work` off the UI thread; a failure becomes an error notice.
    fn in_background(sh: &mut Shell<AppEvent>, work: impl FnOnce() -> Result<()> + Send + 'static) {
        let proxy = sh.proxy();
        std::thread::spawn(move || {
            if let Err(e) = work() {
                tracing::warn!("{e}");
                let notice =
                    Notice { kind: NoticeKind::Error, title: t("common.error").into_owned(), body: error_text(&e), ..Notice::default() };
                proxy.send(AppEvent::Notice(notice));
            }
        });
    }

    /// The tray's "Copy last capture": the picture, or the file itself for a recording.
    fn copy_capture(&mut self, sh: &mut Shell<AppEvent>, id: &str) {
        let Some(entry) = self.history_entry(id) else { return };
        Self::in_background(sh, move || {
            let file = entry.file.ok_or_else(|| snap_core::Error::NotFound(t("app.menu.no_file").into_owned()))?;
            if entry.kind == CaptureKind::Recording {
                snap_platform::clipboard::set_files(&[file])
            } else {
                let (image, scale) = snap_engine::load_saved_image(&file, entry.context.monitor_name.as_deref())?;
                snap_platform::clipboard::set_image_scaled(&image, scale)
            }
        });
    }

    /// The tray's "Copy last link", in the format the Links settings ask for.
    fn copy_capture_link(&mut self, sh: &mut Shell<AppEvent>, id: &str) {
        let Some(entry) = self.history_entry(id) else { return };
        let Some(record) = entry.links.last().cloned() else { return };
        let engine = self.engine.clone();
        Self::in_background(sh, move || {
            let link = snap_engine::Link {
                url: record.url,
                short_url: None,
                delete_url: record.delete_url,
                thumbnail_url: None,
                destination: record.destination,
            };
            engine.copy_link(&link, None, entry.context.window_title.as_deref()).map(|_| ())
        });
    }

    /// A recent capture from the tray: its project or image in the editor, else the Library.
    pub(crate) fn open_recent(&mut self, id: &str) {
        let entry = self.history_entry(id);
        let command = match entry {
            Some(e) => match (e.project, e.file) {
                (Some(path), _) | (None, Some(path)) if e.kind != CaptureKind::Recording => Command::EditFile(path),
                (_, Some(path)) if crate::routing::is_video(&path) => Command::OpenVideoEditor { path },
                (_, Some(path)) => Command::OpenPath(path),
                _ => Command::OpenLibrary,
            },
            None => Command::OpenLibrary,
        };
        self.cmds.push(command);
    }

    pub(crate) fn start_tray(&mut self, sh: &mut Shell<AppEvent>) {
        let recent = self.recent();
        match create(sh.proxy(), &self.settings, State { recent: &recent, recording: self.recording, pins: false }) {
            Ok(tray) => {
                self.tray = Some(tray);
                tracing::info!("tray ready");
            }
            Err(e) => {
                // Without a tray the app would be invisible once its windows close.
                tracing::warn!("tray unavailable: {e}");
                self.quit_when_empty = true;
            }
        }
    }
}
