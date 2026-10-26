//! Global hotkeys: owns the platform `HotkeyManager` and maps its events to bindings.

use snap_core::{Error, Result, hotkey::HotkeyBinding, t};
use snap_engine::{Notice, NoticeKind};
use snap_platform::hotkeys::{self, HotkeyEvent, HotkeyManager};
use snap_shell::{Proxy, Shell};

use crate::{app::App, events::AppEvent};

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
}

impl Hotkeys {
    /// Must be called on the UI thread after the event loop started.
    pub fn new(proxy: Proxy<AppEvent>) -> Result<Self> {
        let manager = HotkeyManager::new(move |event| {
            if let HotkeyEvent::Pressed(index) = event {
                proxy.send(AppEvent::Hotkey(index));
            }
        })?;
        Ok(Self { manager, bindings: Vec::new(), refused: Vec::new() })
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
}

fn conflicts(bindings: &[HotkeyBinding], refused: impl IntoIterator<Item = (usize, Error)>) -> Vec<Conflict> {
    refused
        .into_iter()
        .map(|(index, error)| {
            let accel = bindings[index].accel.clone();
            // When the OS holds the key for a feature of its own, saying which and how to free it beats "in use".
            let reason = hotkeys::system_conflict(&accel).map(|system| system.message).unwrap_or_else(|| match error {
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
                body: conflict.message,
                ..Notice::default()
            };
            self.notice(sh, notice);
        }
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
