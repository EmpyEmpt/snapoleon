//! Stub for the settings view: a standard window that names its current page, so the app runs end to end.
//! Keep `TAG`, `SettingsPage` and the `open` signature: the rest of the app already calls them.

use std::any::Any;

use egui::Context;
use snap_core::t;
use snap_ui::{icon, text, widgets::empty_state};

use super::{Chrome, OpenCx, View, ViewCx, window_spec};

pub const TAG: &str = "settings";

/// A settings page. Deep links (`snapoleon settings`, the hotkey conflict toast, the tray) name one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingsPage {
    General,
    Capture,
    Hotkeys,
    Pipelines,
    Destinations,
    Links,
    Recording,
    TextAi,
    Privacy,
    History,
    Advanced,
}

impl SettingsPage {
    pub const ALL: [SettingsPage; 11] = [
        SettingsPage::General,
        SettingsPage::Capture,
        SettingsPage::Hotkeys,
        SettingsPage::Pipelines,
        SettingsPage::Destinations,
        SettingsPage::Links,
        SettingsPage::Recording,
        SettingsPage::TextAi,
        SettingsPage::Privacy,
        SettingsPage::History,
        SettingsPage::Advanced,
    ];

    /// Stable key for the dev harness (`SNAPOLEON_DEV_PAGE`) and deep links.
    pub fn key(self) -> &'static str {
        match self {
            SettingsPage::General => "general",
            SettingsPage::Capture => "capture",
            SettingsPage::Hotkeys => "hotkeys",
            SettingsPage::Pipelines => "pipelines",
            SettingsPage::Destinations => "destinations",
            SettingsPage::Links => "links",
            SettingsPage::Recording => "recording",
            SettingsPage::TextAi => "text_ai",
            SettingsPage::Privacy => "privacy",
            SettingsPage::History => "history",
            SettingsPage::Advanced => "advanced",
        }
    }

    pub fn from_key(key: &str) -> Option<SettingsPage> {
        SettingsPage::ALL.into_iter().find(|p| p.key() == key)
    }
}

/// Opens the window, or brings it forward and switches to `page` when it is already open.
/// An existing window receives the page as `on_msg(Box<SettingsPage>)`.
pub fn open(cx: &mut OpenCx, page: Option<SettingsPage>) {
    if let Some(&win) = cx.shell.windows_with_tag(TAG).first() {
        cx.focus_tagged(TAG);
        if let Some(page) = page {
            cx.post(win, page);
        }
        return;
    }
    cx.open(
        window_spec(t("app.stub.settings.title"), TAG, (920.0, 680.0), (720.0, 520.0)),
        SettingsView { page: page.unwrap_or(SettingsPage::General) },
    );
}

struct SettingsView {
    page: SettingsPage,
}

impl View for SettingsView {
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context) {
        let title = t("app.stub.settings.title");
        let page = self.page;
        Chrome::new(&title).show(cx, ctx, |ui, _cx| {
            ui.add_space(((ui.available_height() - 170.0) / 2.0).max(0.0));
            empty_state(ui, icon::GEAR, &title, &t("app.stub.body"));
            ui.vertical_centered(|ui| ui.label(text::mono_text(page.key())));
        });
    }

    fn on_msg(&mut self, _cx: &mut ViewCx, msg: Box<dyn Any + Send>) {
        if let Ok(page) = msg.downcast::<SettingsPage>() {
            self.page = *page;
        }
    }
}
