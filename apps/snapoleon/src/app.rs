//! The application: window registry, event routing and command routing.
//!
//! Everything here runs on the UI thread. Worker threads, the tray, hotkeys and the IPC server
//! reach it through `AppEvent`s; views reach it through `Command`s.

use std::{
    any::Any,
    collections::{HashMap, HashSet},
    panic::AssertUnwindSafe,
};

use egui::Context;
use snap_core::{Settings, paths, policy::Policy, t, tf};
use snap_engine::{Engine, Notice, NoticeKind};
use snap_platform::tray::Tray;
use snap_shell::{OsRequest, Shell, ShellApp, WindowKey};
use snap_ui::Toasts;

use crate::{
    commands::Command,
    dev::Dev,
    events::{AppEvent, HistoryChanged},
    hotkeys::Hotkeys,
    ipc,
    notices::Progress,
    theme,
    views::{self, OpenCx, View, ViewCx},
};

/// One registered window.
pub struct Slot {
    /// `None` only while the view is being called.
    view: Option<Box<dyn View>>,
    pub toasts: Toasts,
    /// Whether app-level notices and the progress pill may appear in this window.
    toast_host: bool,
    theme_gen: u64,
}

impl Slot {
    pub fn new(view: Box<dyn View>, toast_host: bool) -> Self {
        Self { view: Some(view), toasts: Toasts::new(), toast_host, theme_gen: theme::generation() }
    }
}

/// Commands behind toast buttons, keyed by the action id the toast reports back.
#[derive(Default)]
pub struct ToastActions {
    next: u64,
    map: HashMap<String, Command>,
}

impl ToastActions {
    pub fn register(&mut self, command: Command) -> String {
        self.next += 1;
        let id = format!("toast-action-{}", self.next);
        self.map.insert(id.clone(), command);
        id
    }

    fn take(&mut self, id: &str) -> Option<Command> {
        self.map.remove(id)
    }
}

/// What `main` hands the app.
pub struct Startup {
    pub engine: Engine,
    pub policy: Policy,
    /// The request this launch was started with (`snapoleon edit x.png`).
    pub request: Option<snap_core::ipc::Request>,
    /// Autostart: go to the tray without showing a window.
    pub background: bool,
    pub dev: Option<Dev>,
}

pub struct App {
    pub(crate) engine: Engine,
    pub(crate) settings: Settings,
    pub(crate) policy: Policy,
    pub(crate) slots: HashMap<WindowKey, Slot>,
    /// Commands queued by the current callback; drained by [`App::flush`].
    pub(crate) cmds: Vec<Command>,
    pub(crate) toast_actions: ToastActions,
    /// The focused app window that can show toasts.
    pub(crate) focused: Option<WindowKey>,
    /// The app window that last had focus; the progress pill lives there.
    pub(crate) host: Option<WindowKey>,
    pub(crate) progress: Option<Progress>,
    pub(crate) jobs_running: usize,
    /// The pin windows, for the tray's pin items.
    pub(crate) pins: HashSet<WindowKey>,
    /// Windows whose view panicked since the user was last told.
    crashed: Vec<WindowKey>,
    pub(crate) hotkeys: Option<Hotkeys>,
    pub(crate) tray: Option<Tray>,
    pub(crate) ipc: Option<snap_platform::ipc::Server>,
    pub(crate) recording: bool,
    pub(crate) quit_pending: bool,
    /// No tray to live in (dev harness, tray failed): the app ends with its last window.
    pub(crate) quit_when_empty: bool,
    pub(crate) startup: Option<Startup>,
    pub(crate) dev: Option<Dev>,
}

impl App {
    pub fn new(startup: Startup) -> Self {
        Self {
            settings: startup.engine.settings(),
            engine: startup.engine.clone(),
            policy: startup.policy.clone(),
            slots: HashMap::new(),
            cmds: Vec::new(),
            toast_actions: ToastActions::default(),
            focused: None,
            host: None,
            progress: None,
            jobs_running: 0,
            pins: HashSet::new(),
            crashed: Vec::new(),
            hotkeys: None,
            tray: None,
            ipc: None,
            recording: false,
            quit_pending: false,
            quit_when_empty: false,
            dev: startup.dev.clone(),
            startup: Some(startup),
        }
    }

    pub(crate) fn open_cx<'a, 'b>(&'a mut self, sh: &'a mut Shell<'b, AppEvent>) -> OpenCx<'a, 'b> {
        let proxy = sh.proxy();
        OpenCx { shell: sh, engine: &self.engine, settings: &self.settings, proxy, slots: &mut self.slots, opened: Vec::new() }
    }

    /// Run a view's `open` entry point; returns the windows it opened.
    pub(crate) fn open_windows(&mut self, sh: &mut Shell<AppEvent>, open: impl FnOnce(&mut OpenCx)) -> Vec<WindowKey> {
        let mut cx = self.open_cx(sh);
        open(&mut cx);
        cx.opened
    }

    /// [`App::open_windows`] for callers that do not need the window keys.
    pub(crate) fn open_with(&mut self, sh: &mut Shell<AppEvent>, open: impl FnOnce(&mut OpenCx)) {
        self.open_windows(sh, open);
    }

    /// Call into a window's view with a fresh `ViewCx`. `None` when the window has no view, or when the view
    /// panicked: that window is closed and the user is told, but the app lives on.
    fn with_view<R>(&mut self, sh: &mut Shell<AppEvent>, win: WindowKey, call: impl FnOnce(&mut dyn View, &mut ViewCx) -> R) -> Option<R> {
        let proxy = sh.proxy();
        let slot = self.slots.get_mut(&win)?;
        let mut view = slot.view.take()?;
        let out = {
            let mut cx = ViewCx {
                shell: sh,
                win,
                engine: &self.engine,
                settings: &self.settings,
                cmds: &mut self.cmds,
                toasts: &mut slot.toasts,
                proxy,
                toast_actions: &mut self.toast_actions,
            };
            std::panic::catch_unwind(AssertUnwindSafe(|| call(view.as_mut(), &mut cx)))
        };
        // The view goes back even after a panic: closing the window calls its `closed`, which ends what it owns
        // (an overlay session, a recording).
        slot.view = Some(view);
        match out {
            Ok(out) => Some(out),
            Err(_) => {
                tracing::error!("a view panicked; closing its window");
                self.crashed.push(win);
                sh.close(win);
                None
            }
        }
    }

    /// Process queued commands until none are left.
    pub(crate) fn flush(&mut self, sh: &mut Shell<AppEvent>) {
        for _ in 0..16 {
            let cmds = std::mem::take(&mut self.cmds);
            if cmds.is_empty() {
                return;
            }
            for cmd in cmds {
                self.run_command(sh, cmd);
            }
        }
        tracing::warn!("command queue still busy after 16 rounds; leaving the rest for the next event");
    }

    // ----- commands -----

    // ----- notices, progress, errors -----

    // ----- settings, tray, hotkeys -----

    /// Re-read the settings and apply everything that depends on them.
    pub(crate) fn reload_settings(&mut self, sh: &mut Shell<AppEvent>) {
        let old = std::mem::replace(&mut self.settings, self.engine.settings());
        let general = &self.settings.general;
        if general.language != old.general.language {
            snap_core::i18n::set_language(&general.language);
            sh.refresh_app_menu();
        }
        theme::apply(general, self.dev.as_ref().and_then(|d| d.dark));
        sh.proxy().repaint_all();
        if old.hotkeys != self.settings.hotkeys {
            self.apply_hotkeys(sh);
        }
        if old.general.launch_at_login != self.settings.general.launch_at_login {
            self.sync_autostart();
        }
        if old.general.instant_overlay != self.settings.general.instant_overlay && self.dev.is_none() && self.tray.is_some() {
            let on = self.settings.general.instant_overlay;
            self.open_with(sh, |cx| views::overlay::set_instant(cx, on));
        }
        self.refresh_tray();
    }

    pub(crate) fn sync_autostart(&self) {
        if paths::is_portable() || self.dev.is_some() {
            return;
        }
        let want = self.settings.general.launch_at_login;
        if snap_platform::autostart::get() != want
            && let Err(e) = snap_platform::autostart::set(want)
        {
            tracing::warn!("autostart: {e}");
        }
    }

    // ----- lifecycle -----

    pub(crate) fn quit(&mut self, sh: &mut Shell<AppEvent>) {
        // Finish a running recording first so its file is kept.
        if self.recording && !sh.windows_with_tag(views::recorder::TAG).is_empty() {
            self.quit_pending = true;
            self.cmds.push(Command::StopRecording);
        } else {
            self.shutdown(sh);
        }
    }

    pub(crate) fn shutdown(&mut self, sh: &mut Shell<AppEvent>) {
        for win in self.slots.keys().copied().collect::<Vec<_>>() {
            sh.close(win);
        }
        self.tray = None;
        self.hotkeys = None;
        self.ipc = None;
        // A postponed restore of the desktop icons would not get to run.
        #[cfg(target_os = "macos")]
        snap_capture::restore_desktop_icons();
        sh.quit();
    }

    fn start(&mut self, sh: &mut Shell<AppEvent>) {
        let Some(startup) = self.startup.take() else { return };
        let proxy = sh.proxy();
        let dev = startup.dev.is_some();
        self.quit_when_empty = dev;
        self.engine.set_history_listener({
            let proxy = proxy.clone();
            move || proxy.send(AppEvent::HistoryChanged)
        });

        // Single instance: the first process to bind the IPC endpoint is the app.
        if !dev {
            match ipc::serve(proxy.clone()) {
                Ok(server) => self.ipc = Some(server),
                Err(e) if snap_platform::ipc::is_already_running(&e) => {
                    tracing::info!("another instance won the race; forwarding");
                    ipc::forward(startup.request, startup.background);
                    sh.quit();
                    return;
                }
                Err(e) => tracing::warn!("single-instance IPC unavailable: {e}"),
            }
            self.start_hotkeys(sh);
            self.start_tray(sh);
            self.sync_autostart();
            let icon = snap_ui::app_icon_rgba(256);
            if let Err(e) = snap_platform::notify::set_app_icon(&icon) {
                tracing::debug!("notification icon: {e}");
            }
            self.purge_history();
            crate::server_events::start(self.engine.clone(), proxy.clone());
            if !paths::is_portable() {
                // Every start, not just the first: the executable may have moved since (an update, a new install
                // folder). Nothing is written when the registration is already right.
                std::thread::spawn(|| {
                    if let Err(e) = snap_platform::scheme::register() {
                        tracing::warn!("url scheme: {e}");
                    }
                });
            }
        }

        if snap_core::settings::is_read_only() {
            let path = paths::settings_file().display().to_string();
            let notice = Notice {
                kind: NoticeKind::Warning,
                title: t("app.notice.settings_unreadable").into_owned(),
                body: tf("app.notice.settings_unreadable_body", &[("path", &path)]),
                ..Notice::default()
            };
            self.notice(sh, notice);
        }

        if dev {
            self.dev_start(sh);
        } else if !self.settings.general.first_run_done {
            self.cmds.push(Command::OpenOnboarding);
        } else if startup.request.is_none() && !startup.background && !sh.launched_to_open() {
            // A launch to open a document or a link (macOS) is not a plain launch: that request follows instead.
            self.cmds.push(Command::OpenLibrary);
        }
        if let Some(request) = startup.request {
            self.handle_request(sh, request, None);
        }
        self.flush(sh);
        if !dev && self.tray.is_some() && self.settings.general.instant_overlay {
            // Last, so the first window the user asked for is not kept waiting for the GPU: the overlay's covers
            // open hidden now, and the first PrintScreen is as fast as every later one. Without a tray the app ends
            // with its last window, which hidden covers would prevent.
            self.open_with(sh, views::overlay::prewarm);
        }
    }

    /// Apply the retention policy off the UI thread.
    fn purge_history(&self) {
        let days = self.settings.history.retention_days;
        if days == 0 {
            return;
        }
        if let Some(history) = self.engine.history() {
            std::thread::spawn(move || match history.purge(days) {
                Ok(0) => {}
                Ok(n) => tracing::info!("history retention removed {n} entries"),
                Err(e) => tracing::warn!("history retention: {e}"),
            });
        }
    }
}

impl ShellApp<AppEvent> for App {
    fn init(&mut self, sh: &mut Shell<AppEvent>) {
        self.start(sh);
    }

    fn event(&mut self, sh: &mut Shell<AppEvent>, event: AppEvent) {
        match event {
            AppEvent::Hotkey(index) => self.on_hotkey(sh, index),
            AppEvent::Tray(event) => self.on_tray(sh, event),
            AppEvent::Ipc(request, reply) => self.handle_request(sh, request, Some(reply)),
            AppEvent::Command(command) => self.cmds.push(command),
            AppEvent::JobProgress { label, frac } => {
                self.on_job_progress(label, frac);
                if let Some(host) = self.host {
                    sh.repaint(host);
                }
            }
            AppEvent::JobDone(done) => self.on_job_done(sh, done),
            AppEvent::Notice(notice) => self.notice(sh, notice),
            AppEvent::SettingsChanged => self.reload_settings(sh),
            AppEvent::HistoryChanged => self.history_changed(sh),
            AppEvent::ViewMsg { win, msg } => self.deliver(sh, win, msg),
            AppEvent::DevShot => self.dev_shot(sh),
        }
        self.flush(sh);
    }

    fn ui(&mut self, sh: &mut Shell<AppEvent>, win: WindowKey, ctx: &Context) {
        let Some(slot) = self.slots.get_mut(&win) else { return };
        if slot.toast_host {
            if ctx.input(|i| i.viewport().focused).unwrap_or(false) {
                if self.focused != Some(win) && theme::follow_system(&self.settings.general, self.dev.as_ref().and_then(|d| d.dark)) {
                    sh.proxy().repaint_all();
                }
                self.focused = Some(win);
                self.host = Some(win);
            } else if self.focused == Some(win) {
                self.focused = None;
            }
        }
        theme::sync(ctx, &mut slot.theme_gen);

        self.with_view(sh, win, |view, cx| view.ui(cx, ctx));

        if self.host == Some(win) {
            self.draw_progress(ctx);
        }
        if let Some(slot) = self.slots.get_mut(&win) {
            for id in slot.toasts.show(ctx) {
                if let Some(command) = self.toast_actions.take(&id) {
                    self.cmds.push(command);
                }
            }
        }
        self.flush(sh);
    }

    fn close_requested(&mut self, sh: &mut Shell<AppEvent>, win: WindowKey) -> bool {
        let allowed = self.with_view(sh, win, |view, cx| view.close_requested(cx)).unwrap_or(true);
        self.flush(sh);
        allowed
    }

    fn os_request(&mut self, sh: &mut Shell<AppEvent>, request: OsRequest) {
        self.handle_os_request(sh, request);
        self.flush(sh);
    }

    fn closed(&mut self, sh: &mut Shell<AppEvent>, win: WindowKey) {
        if self.focused == Some(win) {
            self.focused = None;
        }
        if self.host == Some(win) {
            self.host = None;
        }
        if self.pins.remove(&win) {
            self.refresh_tray();
        }
        if let Some(mut slot) = self.slots.remove(&win)
            && let Some(mut view) = slot.view.take()
        {
            let mut cx = ViewCx {
                proxy: sh.proxy(),
                shell: sh,
                win,
                engine: &self.engine,
                settings: &self.settings,
                cmds: &mut self.cmds,
                toasts: &mut slot.toasts,
                toast_actions: &mut self.toast_actions,
            };
            if std::panic::catch_unwind(AssertUnwindSafe(|| view.closed(&mut cx))).is_err() {
                tracing::error!("a view panicked while closing");
            }
        }
        if let Some(at) = self.crashed.iter().position(|crashed| *crashed == win) {
            self.crashed.swap_remove(at);
            let notice = Notice { kind: NoticeKind::Error, title: t("app.error.crashed").into_owned(), ..Notice::default() };
            self.notice(sh, notice);
        }
        self.flush(sh);
        if self.quit_when_empty && self.slots.is_empty() {
            sh.quit();
        }
    }
}

impl App {
    fn deliver(&mut self, sh: &mut Shell<AppEvent>, win: WindowKey, msg: Box<dyn Any + Send>) {
        if self.with_view(sh, win, |view, cx| view.on_msg(cx, msg)).is_some() {
            sh.repaint(win);
        }
    }

    /// Something landed in the history: the tray's recent list and every open window (the Library) catch up.
    pub(crate) fn history_changed(&mut self, sh: &mut Shell<AppEvent>) {
        self.refresh_tray();
        for win in self.slots.keys().copied().collect::<Vec<_>>() {
            self.deliver(sh, win, Box::new(HistoryChanged));
        }
    }
}
