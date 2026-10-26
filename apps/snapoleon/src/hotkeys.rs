//! Global hotkeys: owns the platform `HotkeyManager` and maps its events to bindings.

use std::sync::atomic::{AtomicBool, Ordering};

use snap_core::{
    Error, Result,
    hotkey::{Action, HotkeyBinding},
    t,
};
use snap_engine::{Notice, NoticeKind};
use snap_platform::hotkeys::{self, HotkeyEvent, HotkeyManager};
use snap_shell::{Proxy, Shell};

use crate::{
    app::App,
    commands::Command,
    events::AppEvent,
    views::{OpenCx, countdown, overlay},
};

/// A binding the OS refused, for the conflict notice. `reason` names the shortcut itself.
pub struct Conflict {
    pub accel: String,
    pub reason: String,
}

pub struct Hotkeys {
    manager: HotkeyManager,
    /// The bindings as last registered; `AppEvent::Hotkey(i)` indexes into this.
    bindings: Vec<HotkeyBinding>,
    /// Indices into `bindings` the OS refused at the last `apply`.
    refused: Vec<usize>,
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
        Ok(Self { manager, bindings: Vec::new(), refused: Vec::new(), escape: false })
    }

    /// Replace every registration. Returns the bindings that could not be registered.
    pub fn apply(&mut self, bindings: &[HotkeyBinding]) -> Vec<Conflict> {
        self.bindings = bindings.to_vec();
        let refused: Vec<(usize, Error)> =
            self.manager.register(bindings).into_iter().filter_map(|(index, result)| Some((index, result.err()?))).collect();
        self.refused = refused.iter().map(|(index, _)| *index).collect();
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

    /// Registered bindings the OS itself also reacts to (see `hotkeys::system_conflict`). Refused ones are left out:
    /// their conflict notice already carries the same explanation.
    pub fn system_conflicts(&self) -> Vec<hotkeys::SystemConflict> {
        self.bindings
            .iter()
            .enumerate()
            .filter(|(index, b)| b.enabled && !self.refused.contains(index))
            .filter_map(|(_, b)| hotkeys::system_conflict(&b.accel))
            .collect()
    }

    fn grab_escape(&mut self, on: bool) {
        if std::mem::replace(&mut self.escape, on) == on {
            return;
        }
        if let Err(e) = self.manager.grab_escape(on) {
            tracing::warn!("cannot take Escape while the overlay or a count-down is up: {e}");
        }
        ESCAPE_HELD.store(self.manager.escape_grabbed(), Ordering::Relaxed);
    }
}

/// The OS holds Escape for us (see `Hotkeys::grab_escape`); false without global hotkeys.
static ESCAPE_HELD: AtomicBool = AtomicBool::new(false);

/// Whether a press of Escape reaches the app right now, whatever has the keyboard focus. Views that cannot take the
/// focus, like the count-down tile, offer Escape only then.
pub(crate) fn escape_held() -> bool {
    ESCAPE_HELD.load(Ordering::Relaxed)
}

/// Escape pressed anywhere while it was grabbed: end the overlay session and drop a delayed capture's count-down.
fn escape(cx: &mut OpenCx) {
    overlay::cancel(cx);
    countdown::cancel(cx);
}

/// The hotkeys that take a running overlay session down: every capture, and every action that opens the overlay.
fn ends_session(action: Action) -> bool {
    action.is_capture()
        || matches!(
            action,
            Action::RecordRegion
                | Action::RecordGif
                | Action::OcrRegion
                | Action::DecodeCodesRegion
                | Action::TranslateRegion
                | Action::ColorPicker
                | Action::Measure
                | Action::PinRegion
        )
}

fn conflicts(bindings: &[HotkeyBinding], refused: impl IntoIterator<Item = (usize, Error)>) -> Vec<Conflict> {
    refused
        .into_iter()
        .map(|(index, error)| {
            let accel = bindings[index].accel.clone();
            // When the OS holds the key for a feature of its own, saying which and how to free it beats "in use".
            let reason = hotkeys::system_conflict(&accel).map(|system| system.brief).unwrap_or_else(|| match error {
                Error::Invalid(reason) => reason,
                other => other.to_string(),
            });
            Conflict { accel, reason }
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
                body: conflict.reason,
                ..Notice::default()
            };
            self.notice(sh, notice);
        }
    }

    /// Once per launch: later changes are made on the Hotkeys settings page or the onboarding card, which show the
    /// same warning next to the shortcut.
    fn report_system_conflicts(&mut self, sh: &mut Shell<AppEvent>) {
        let Some(hotkeys) = &self.hotkeys else { return };
        for conflict in hotkeys.system_conflicts() {
            tracing::warn!("hotkey is also a system shortcut: {}", conflict.message);
            let notice = Notice {
                kind: NoticeKind::Warning,
                title: t("app.hotkey.system_title").into_owned(),
                body: conflict.brief,
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
            // A capture or overlay hotkey during a session takes it down: it is the one key that always reaches the
            // app, whatever has the keyboard focus. Only the user's own key press does this; a delayed capture's
            // count-down, IPC and links that arrive mid-session are dropped.
            Some(binding) if ends_session(binding.action) && overlay::active() => self.open_with(sh, overlay::cancel),
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
                self.report_system_conflicts(sh);
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
