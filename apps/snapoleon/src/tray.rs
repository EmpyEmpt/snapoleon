//! The tray icon and its menu.

use chrono::{Local, TimeZone};
use snap_core::{CaptureKind, Result, Settings, hotkey::Action, t};
use snap_history::Entry;
use snap_platform::{
    hotkeys,
    tray::{Tray, TrayEvent, TrayItem},
};
use snap_shell::{Proxy, Shell};

use crate::{app::App, commands::Command, events::AppEvent};

const ICON_SIZE: u32 = 32;
const RECENT_LIMIT: usize = 8;
const LABEL_LIMIT: usize = 38;

/// What a tray menu item stands for.
pub enum MenuAction {
    Action(Action),
    Recent(String),
    Library,
    Settings,
    Quit,
}

impl MenuAction {
    pub fn parse(id: &str) -> Option<MenuAction> {
        match id {
            "library" => Some(MenuAction::Library),
            "settings" => Some(MenuAction::Settings),
            "quit" => Some(MenuAction::Quit),
            _ => match id.split_once(':')? {
                ("action", key) => Action::from_key(key).map(MenuAction::Action),
                ("recent", id) => Some(MenuAction::Recent(id.to_string())),
                _ => None,
            },
        }
    }
}

pub fn create(proxy: Proxy<AppEvent>, settings: &Settings, recent: &[Entry], recording: bool) -> Result<Tray> {
    Tray::new(
        &tooltip(recording),
        &snap_ui::tray_icon_rgba(ICON_SIZE, recording),
        menu(settings, recent, recording),
        move |event: TrayEvent| {
            proxy.send(AppEvent::Tray(event));
        },
    )
}

/// Rebuild after settings, history or the recording state changed.
pub fn refresh(tray: &mut Tray, settings: &Settings, recent: &[Entry], recording: bool) {
    tray.set_items(menu(settings, recent, recording));
    tray.set_icon(&snap_ui::tray_icon_rgba(ICON_SIZE, recording));
    tray.set_tooltip(&tooltip(recording));
}

pub fn tooltip(recording: bool) -> String {
    t(if recording { "app.tray.tooltip_recording" } else { "app.tray.tooltip" }).into_owned()
}

pub fn recent_limit() -> usize {
    RECENT_LIMIT
}

fn menu(settings: &Settings, recent: &[Entry], recording: bool) -> Vec<TrayItem> {
    let action = |a: Action| {
        let label = t(&format!("action.{}", a.key()));
        TrayItem::new(format!("action:{}", a.key()), with_accelerator(settings, a, &label))
    };
    let mut items = Vec::new();
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
        TrayItem::separator(),
        recent_menu(recent),
        TrayItem::new("library", t("action.open_library")),
        TrayItem::new("settings", t("action.open_settings")),
        TrayItem::separator(),
        TrayItem::new("quit", t("app.tray.quit")),
    ]);
    items
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

/// Windows menus right-align text after a tab as the shortcut column; other platforms get the plain label.
fn with_accelerator(settings: &Settings, action: Action, label: &str) -> String {
    if !cfg!(windows) {
        return label.to_string();
    }
    match settings.hotkeys.iter().find(|b| b.enabled && b.action == action) {
        Some(binding) => format!("{label}\t{}", hotkeys::display(&binding.accel)),
        None => label.to_string(),
    }
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
        if let Some(tray) = &mut self.tray {
            refresh(tray, &self.settings, &recent, self.recording);
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
                Some(MenuAction::Library) => self.cmds.push(Command::OpenLibrary),
                Some(MenuAction::Settings) => self.cmds.push(Command::OpenSettings { page: None }),
                Some(MenuAction::Quit) => self.cmds.push(Command::Quit),
                None => tracing::debug!("unknown tray item {id}"),
            },
        }
    }

    /// A recent capture from the tray: its project or image in the editor, else the Library.
    pub(crate) fn open_recent(&mut self, id: &str) {
        let entry = self.engine.history().and_then(|h| h.get(&snap_core::CaptureId(id.to_string())).ok().flatten());
        let command = match entry {
            Some(e) => match (e.project, e.file) {
                (Some(path), _) | (None, Some(path)) if e.kind != CaptureKind::Recording => Command::EditFile(path),
                (_, Some(path)) => Command::OpenPath(path),
                _ => Command::OpenLibrary,
            },
            None => Command::OpenLibrary,
        };
        self.cmds.push(command);
    }

    pub(crate) fn start_tray(&mut self, sh: &mut Shell<AppEvent>) {
        let recent = self.recent();
        match create(sh.proxy(), &self.settings, &recent, self.recording) {
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
