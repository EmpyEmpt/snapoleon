//! Global hotkeys: owns the platform `HotkeyManager` and maps its events to bindings.

use snap_core::{Error, Result, hotkey::HotkeyBinding, t, tf};
use snap_engine::{Notice, NoticeKind};
use snap_platform::hotkeys::{self, HotkeyEvent, HotkeyManager};
use snap_shell::{Proxy, Shell};

use crate::{
    app::App,
    commands::Command,
    events::AppEvent,
    views::{OpenCx, countdown, overlay},
};

/// A binding the OS refused, for the conflict notice.
pub struct Conflict {
    pub accel: String,
    pub reason: String,
}

impl Conflict {
    pub fn message(&self) -> String {
        tf("app.hotkey.conflict_body", &[("accel", &hotkeys::display(&self.accel)), ("reason", &self.reason)])
    }
}

pub struct Hotkeys {
    manager: HotkeyManager,
    /// The bindings as last registered; `AppEvent::Hotkey(i)` indexes into this.
    bindings: Vec<HotkeyBinding>,
    /// Escape is wanted globally (whether or not the OS granted it).
    escape: bool,
}

impl Hotkeys {
    /// Must be called on the UI thread after the event loop started.
    pub fn new(proxy: Proxy<AppEvent>) -> Result<Self> {
        let manager = HotkeyManager::new(move |event| match event {
            HotkeyEvent::Pressed(index) => proxy.send(AppEvent::Hotkey(index)),
            HotkeyEvent::Escape => proxy.send(AppEvent::Command(Command::Open(Box::new(escape)))),
            HotkeyEvent::Released(_) => {}
        })?;
        Ok(Self { manager, bindings: Vec::new(), escape: false })
    }

    /// Replace every registration. Returns the bindings that could not be registered.
    pub fn apply(&mut self, bindings: &[HotkeyBinding]) -> Vec<Conflict> {
        self.bindings = bindings.to_vec();
        let refused = self.manager.register(bindings).into_iter().filter_map(|(index, result)| Some((index, result.err()?)));
        conflicts(bindings, refused)
    }

    pub fn binding(&self, index: usize) -> Option<&HotkeyBinding> {
        self.bindings.get(index)
    }

    /// Whether `bindings` is what is registered right now.
    pub fn is_current(&self, bindings: &[HotkeyBinding]) -> bool {
        self.bindings == bindings
    }

    /// Pause while a hotkey recorder field is focused. Bindings changed while paused are registered when it ends;
    /// the ones the OS then refuses are returned.
    pub fn pause(&mut self, paused: bool) -> Vec<Conflict> {
        conflicts(&self.bindings, self.manager.pause(paused))
    }

    fn grab_escape(&mut self, on: bool) {
        if std::mem::replace(&mut self.escape, on) == on {
            return;
        }
        if let Err(e) = self.manager.grab_escape(on) {
            tracing::warn!("cannot take Escape while the overlay or a count-down is up: {e}");
        }
    }
}

/// Escape pressed anywhere while it was grabbed: end the overlay session and drop a delayed capture's count-down.
fn escape(cx: &mut OpenCx) {
    overlay::cancel(cx);
    countdown::cancel(cx);
}

fn conflicts(bindings: &[HotkeyBinding], refused: impl IntoIterator<Item = (usize, Error)>) -> Vec<Conflict> {
    refused
        .into_iter()
        .map(|(index, error)| {
            let reason = match error {
                Error::Invalid(reason) => reason,
                other => other.to_string(),
            };
            Conflict { accel: bindings[index].accel.clone(), reason }
        })
        .collect()
}

impl App {
    pub(crate) fn apply_hotkeys(&mut self, sh: &mut Shell<AppEvent>) {
        let Some(hotkeys) = &mut self.hotkeys else { return };
        if hotkeys.is_current(&self.settings.hotkeys) {
            return;
        }
        let conflicts = hotkeys.apply(&self.settings.hotkeys);
        self.report_conflicts(sh, conflicts);
    }

    pub(crate) fn pause_hotkeys(&mut self, sh: &mut Shell<AppEvent>, paused: bool) {
        let Some(hotkeys) = &mut self.hotkeys else { return };
        let conflicts = hotkeys.pause(paused);
        self.report_conflicts(sh, conflicts);
    }

    fn report_conflicts(&mut self, sh: &mut Shell<AppEvent>, conflicts: Vec<Conflict>) {
        for conflict in conflicts {
            tracing::warn!("hotkey {} not registered: {}", conflict.accel, conflict.reason);
            let notice = Notice {
                kind: NoticeKind::Warning,
                title: t("app.hotkey.conflict_title").into_owned(),
                body: conflict.message(),
                ..Notice::default()
            };
            self.notice(sh, notice);
        }
    }

    /// Grab Escape globally exactly while an overlay session or a delayed capture's count-down is up: the overlay's
    /// own Escape needs the keyboard focus, which macOS may give to a system alert, and the count-down never has it.
    pub(crate) fn sync_escape(&mut self, sh: &Shell<AppEvent>) {
        let Some(hotkeys) = &mut self.hotkeys else { return };
        hotkeys.grab_escape(overlay::active() || !sh.windows_with_tag(countdown::TAG).is_empty());
    }

    pub(crate) fn on_hotkey(&mut self, sh: &mut Shell<AppEvent>, index: usize) {
        let binding = self.hotkeys.as_ref().and_then(|h| h.binding(index)).cloned();
        match binding {
            Some(binding) => self.start_action(sh, binding.action, binding.pipeline),
            None => tracing::debug!("hotkey {index} has no binding"),
        }
    }

    pub(crate) fn start_hotkeys(&mut self, sh: &mut Shell<AppEvent>) {
        match Hotkeys::new(sh.proxy()) {
            Ok(hotkeys) => {
                self.hotkeys = Some(hotkeys);
                self.apply_hotkeys(sh);
                tracing::info!("hotkeys ready");
            }
            Err(e) => {
                tracing::warn!("global hotkeys unavailable: {e}");
                self.notice(
                    sh,
                    Notice {
                        kind: NoticeKind::Warning,
                        title: t("app.hotkey.unavailable").into_owned(),
                        body: e.to_string(),
                        ..Notice::default()
                    },
                );
            }
        }
    }
}
