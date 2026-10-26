//! The view contract.
//!
//! A view is one window's UI. The app keeps a registry `WindowKey -> Box<dyn View>`, calls
//! [`View::ui`] every frame of that window and forwards close requests and messages. Views
//! never open each other: they push a [`Command`] into [`ViewCx::cmds`] and the app routes it.
//!
//! Each view owns one directory below `views/` and exposes a `pub fn open(cx: &mut OpenCx, ...)`
//! entry point that the app calls when the matching command arrives. See docs/APP.md.

use std::{any::Any, collections::HashMap, sync::OnceLock};

use egui::{Context, Key, SystemTheme, Ui, ViewportCommand};
use snap_core::{Error, RgbaImage, Settings, t, tf};
use snap_engine::{Engine, Notice, NoticeKind};
use snap_shell::{Proxy, Shell, WindowKey, WindowSpec};
use snap_ui::{
    Toast, ToastKind, Toasts, palette, space,
    widgets::{TITLE_BAR_HEIGHT, TitleBarActions, title_bar_with},
};

use crate::{
    app::{Slot, ToastActions},
    commands::Command,
    events::AppEvent,
};

pub mod about;
pub mod colors;
pub mod combine;
pub mod countdown;
pub mod diff;
pub mod editor;
pub mod library;
pub mod onboarding;
pub mod overlay;
pub mod pin;
pub mod recorder;
pub mod scroll;
pub mod settings;
pub mod steps;
pub mod text;
pub mod video;

/// One window's UI. Everything runs on the UI thread, so a view may hold `Rc`/`RefCell`
/// (the overlay shares one session between its per-monitor windows that way).
pub trait View {
    /// Build the window's UI. Called once per frame of this window.
    fn ui(&mut self, cx: &mut ViewCx, ctx: &Context);

    /// The user asked to close the window. Return `false` to keep it open (unsaved changes).
    fn close_requested(&mut self, _cx: &mut ViewCx) -> bool {
        true
    }

    /// The window is gone. Release resources; the last chance to push commands.
    fn closed(&mut self, _cx: &mut ViewCx) {}

    /// A message posted through [`ViewHandle::send`], [`ViewCx::spawn`] or [`OpenCx::post`].
    fn on_msg(&mut self, _cx: &mut ViewCx, _msg: Box<dyn Any + Send>) {}
}

/// What a view gets on every call.
pub struct ViewCx<'a, 'b> {
    pub shell: &'a mut Shell<'b, AppEvent>,
    /// The window this call is for.
    pub win: WindowKey,
    pub engine: &'a Engine,
    /// Snapshot taken when the call started; refreshed after `AppEvent::SettingsChanged`.
    pub settings: &'a Settings,
    /// Commands processed right after the current call returns.
    pub cmds: &'a mut Vec<Command>,
    /// This window's toast stack (drawn by the app after `ui` returns).
    pub toasts: &'a mut Toasts,
    pub proxy: Proxy<AppEvent>,
    pub(crate) toast_actions: &'a mut ToastActions,
}

impl ViewCx<'_, '_> {
    /// A `Send` handle that delivers messages to this view's `on_msg` from any thread.
    pub fn handle(&self) -> ViewHandle {
        ViewHandle { proxy: self.proxy.clone(), win: self.win }
    }

    /// Run `work` on a new thread; its result comes back as `on_msg(Box<T>)` of this window.
    pub fn spawn<T: Send + 'static>(&self, work: impl FnOnce() -> T + Send + 'static) {
        let handle = self.handle();
        let spawned = std::thread::Builder::new().name("snap-view-job".into()).spawn(move || handle.send(work()));
        if let Err(e) = spawned {
            tracing::error!("cannot start a view job: {e}");
        }
    }

    pub fn push(&mut self, cmd: Command) {
        self.cmds.push(cmd);
    }

    pub fn toast(&mut self, kind: ToastKind, text: impl Into<String>) {
        self.toasts.push(kind, text);
    }

    /// A toast with one button; clicking it runs `command`.
    pub fn toast_action(&mut self, kind: ToastKind, text: impl Into<String>, label: impl Into<String>, command: Command) {
        let id = self.toast_actions.register(command);
        self.toasts.push_toast(Toast::new(kind, text).action(label, id));
    }

    /// Show an error as a toast, using the localised message where the error type has one.
    pub fn toast_error(&mut self, e: &Error) {
        self.toasts.push(ToastKind::Error, crate::notices::error_text(e));
    }

    /// A notice that is not tied to this window's toast stack (OS notification when no app window is focused).
    pub fn notify(&self, notice: Notice) {
        self.proxy.send(AppEvent::Notice(notice));
    }

    /// Persist a settings change and tell the app to re-apply it (theme, hotkeys, tray, autostart).
    pub fn update_settings(&mut self, change: impl FnOnce(&mut Settings)) {
        match self.engine.update_settings(change) {
            Ok(()) => self.proxy.send(AppEvent::SettingsChanged),
            Err(e) => self.toast_error(&e),
        }
    }

    /// Close this window without asking `close_requested`.
    pub fn close(&mut self) {
        self.shell.close(self.win);
    }
}

/// Thread-safe way into one view. Cheap to clone.
#[derive(Clone)]
pub struct ViewHandle {
    proxy: Proxy<AppEvent>,
    win: WindowKey,
}

impl ViewHandle {
    pub fn send<M: Any + Send>(&self, msg: M) {
        self.proxy.send(AppEvent::ViewMsg { win: self.win, msg: Box::new(msg) });
    }
}

/// What a view's `open` entry point gets: enough to create windows and register their views.
pub struct OpenCx<'a, 'b> {
    pub shell: &'a mut Shell<'b, AppEvent>,
    pub engine: &'a Engine,
    pub settings: &'a Settings,
    pub proxy: Proxy<AppEvent>,
    pub(crate) slots: &'a mut HashMap<WindowKey, Slot>,
    pub(crate) opened: Vec<WindowKey>,
}

impl OpenCx<'_, '_> {
    /// Open a window and register `view` for it. Returns the key even when the OS refused the
    /// window (then the view is dropped and an error notice is sent).
    pub fn open(&mut self, spec: WindowSpec, view: impl View + 'static) -> WindowKey {
        // Toasts live in ordinary app windows, not in overlays, pins or HUDs.
        let toast_host = spec.cover.is_none() && !spec.tool_window && !spec.always_on_top && !spec.skip_taskbar;
        let key = self.shell.open(spec);
        if !self.shell.exists(key) {
            self.notify(Notice { kind: NoticeKind::Error, title: t("app.error.window").into_owned(), ..Notice::default() });
            return key;
        }
        self.slots.insert(key, Slot::new(Box::new(view), toast_host));
        self.opened.push(key);
        key
    }

    /// Bring the existing window with this tag to the front. Singleton views call this first.
    pub fn focus_tagged(&mut self, tag: &str) -> bool {
        let Some(&key) = self.shell.windows_with_tag(tag).first() else { return false };
        self.shell.set_visible(key, true);
        self.shell.focus(key);
        true
    }

    /// Deliver `msg` to the view of `win` (asynchronously, on the next event-loop turn).
    pub fn post<M: Any + Send>(&self, win: WindowKey, msg: M) {
        self.proxy.send(AppEvent::ViewMsg { win, msg: Box::new(msg) });
    }

    pub fn notify(&self, notice: Notice) {
        self.proxy.send(AppEvent::Notice(notice));
    }
}

/// A file size in the user's language: "812 B", "238 KB", "1.4 MB".
pub fn file_size(n: u64) -> String {
    const UNITS: [&str; 4] = ["b", "kb", "mb", "gb"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let number = if unit == 0 || value >= 100.0 { format!("{value:.0}") } else { format!("{value:.1}") };
    tf(&format!("common.size.{}", UNITS[unit]), &[("n", &number)])
}

/// The app icon every window carries.
pub fn app_icon() -> RgbaImage {
    static ICON: OnceLock<RgbaImage> = OnceLock::new();
    ICON.get_or_init(|| snap_ui::app_icon_rgba(64)).clone()
}

/// A standard app window: the app's own title bar ([`Chrome`]), resizable, app icon. On macOS the window keeps the
/// system's traffic lights, rounded corners and shadow around that title bar; elsewhere it has no OS decorations and
/// is rounded where the OS can (Windows 11). `size` and `min` are logical pixels; the shell fits them to small screens.
pub fn window_spec(title: impl Into<String>, tag: &str, size: (f32, f32), min: (f32, f32)) -> WindowSpec {
    WindowSpec {
        title: title.into(),
        size: Some(size),
        min_size: Some(min),
        decorations: false,
        title_bar: Some(TITLE_BAR_HEIGHT),
        rounded: true,
        icon: Some(app_icon()),
        tag: tag.into(),
        ..WindowSpec::default()
    }
}

type CenterSlot<'a> = Box<dyn FnOnce(&mut Ui) + 'a>;

/// The standard window chrome: title bar, window outline, body.
///
/// ```ignore
/// Chrome::new(&t("editor.title")).show(cx, ctx, |ui, cx| { /* body */ });
/// ```
pub struct Chrome<'a> {
    title: &'a str,
    maximize: bool,
    close_on_escape: bool,
    center: Option<CenterSlot<'a>>,
}

impl<'a> Chrome<'a> {
    pub fn new(title: &'a str) -> Self {
        Self { title, maximize: true, close_on_escape: false, center: None }
    }

    /// For dialog-like windows: no maximize button (on macOS a greyed-out green one).
    pub fn no_maximize(mut self) -> Self {
        self.maximize = false;
        self
    }

    /// Escape closes the window, as it does a dialog, unless a text field or another widget has the keyboard.
    pub fn close_on_escape(mut self) -> Self {
        self.close_on_escape = true;
        self
    }

    /// Content between the title and the window buttons (the Library's search field).
    pub fn center(mut self, add: impl FnOnce(&mut Ui) + 'a) -> Self {
        self.center = Some(Box::new(add));
        self
    }

    pub fn show<R>(self, cx: &mut ViewCx, ctx: &Context, body: impl FnOnce(&mut Ui, &mut ViewCx) -> R) -> R {
        // macOS: the system's traffic lights sit over the left end of the bar (see `window_spec`).
        let native = cx.shell.title_bar_inset(cx.win);
        let mut actions = TitleBarActions {
            maximized: ctx.input(|i| i.viewport().maximized).unwrap_or(false),
            show_maximize: self.maximize,
            native_controls: native.map(|inset| if inset > 0.0 { inset + space::MD } else { 0.0 }),
            ..TitleBarActions::default()
        };
        let center = self.center;
        let title = self.title;
        // Window lists, the Window menu and assistive tools read the native title, which the bar drawn here hides:
        // keep it the one shown, also after the title changes or the language is switched.
        let title_id = egui::Id::unique("snap-native-title");
        if ctx.data(|d| d.get_temp::<String>(title_id)).as_deref() != Some(title) {
            ctx.data_mut(|d| d.insert_temp(title_id, title.to_owned()));
            cx.shell.set_title(cx.win, title);
        }
        let out = ctx.root_ui(|ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(palette(ui.ctx()).bg))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                    title_bar_with(ui, title, &mut actions, |ui| {
                        if let Some(center) = center {
                            center(ui);
                        }
                    });
                    ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                    body(ui, cx)
                })
                .inner
        });
        if self.close_on_escape && !ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_pressed(Key::Escape)) {
            actions.close = true;
        }
        actions.apply(ctx);
        if actions.double_clicked {
            cx.shell.title_bar_double_clicked(cx.win);
        }
        if native.is_some() {
            sync_native_controls(ctx, self.maximize);
        } else {
            paint_outline(ctx);
        }
        out
    }
}

/// macOS: the traffic lights are the system's. Keep the green button and their light or dark look in step with the
/// view; the window is only told when something changed. The system also draws the window's outline.
fn sync_native_controls(ctx: &Context, maximize: bool) {
    let dark = ctx.theme() == egui::Theme::Dark;
    let id = egui::Id::unique("snap-native-controls");
    let last = ctx.data(|d| d.get_temp::<(bool, bool)>(id));
    if last == Some((maximize, dark)) {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(id, (maximize, dark)));
    // A new window has all three buttons and follows the system's appearance.
    if last.map_or(!maximize, |(was, _)| was != maximize) {
        ctx.send_viewport_cmd(ViewportCommand::EnableButtons { close: true, minimized: true, maximize });
    }
    if last.is_none_or(|(_, was)| was != dark) {
        ctx.send_viewport_cmd(ViewportCommand::SetTheme(if dark { SystemTheme::Dark } else { SystemTheme::Light }));
    }
}

/// The 1 px window outline from DESIGN.md, drawn above everything.
fn paint_outline(ctx: &Context) {
    let p = palette(ctx);
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("snap-window-outline")));
    painter.rect_stroke(ctx.content_rect(), 0.0, egui::Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
}
