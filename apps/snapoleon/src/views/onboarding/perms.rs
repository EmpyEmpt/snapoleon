//! Permission status and actions shared by the onboarding card and Settings > Privacy: the live state of each
//! permission, the one action that can still change it, and the relaunch macOS needs before a Screen Recording grant
//! reaches a running app.

use std::time::{Duration, Instant};

use egui::{Context, Ui, Widget};
use snap_core::t;
use snap_engine::{Notice, NoticeKind};
use snap_platform::permissions::{self, Perm, Status};
use snap_shell::Proxy;
use snap_ui::{
    Tone, icon, text,
    widgets::{self, Button, ButtonKind, badge},
};

use crate::{commands::Command, events::AppEvent, views::ViewCx};

/// How often the status is read again while a permission list is on screen.
const POLL: Duration = Duration::from_secs(1);

/// The permission statuses, read at most once per [`POLL`] (the window repaints on every pointer move).
#[derive(Clone, Copy)]
pub struct Live {
    status: [Status; 4],
    read_at: Instant,
}

impl Live {
    pub fn read() -> Self {
        Self { status: Perm::ALL.map(permissions::status), read_at: Instant::now() }
    }

    pub fn get(&self, perm: Perm) -> Status {
        Perm::ALL.iter().position(|p| *p == perm).map_or(Status::Granted, |i| self.status[i])
    }

    /// Read again when due and keep the window waking up for the next read. Call it every frame the list is shown.
    pub fn refresh(&mut self, ctx: &Context) {
        if self.read_at.elapsed() >= POLL {
            *self = Self::read();
        }
        if self.get(Perm::ScreenRecording) == Status::Granted {
            prime_capture();
        }
        ctx.request_repaint_after(POLL);
    }
}

/// The capture backend's own one-time consent comes up now, while the user is looking at permissions, instead of
/// over the capture overlay. Once per process, off the UI thread.
fn prime_capture() {
    #[cfg(target_os = "macos")]
    {
        static PRIMED: std::sync::Once = std::sync::Once::new();
        PRIMED.call_once(|| {
            if let Err(e) = std::thread::Builder::new().name("snap-capture-consent".into()).spawn(snap_capture::prime_capture_consent) {
                tracing::warn!("cannot prime the capture consent: {e}");
            }
        });
    }
}

/// The localisation key and icon of a permission's row.
pub fn row_texts(perm: Perm) -> (&'static str, &'static str) {
    match perm {
        Perm::ScreenRecording => ("screen", icon::MONITOR),
        Perm::Accessibility => ("accessibility", icon::CURSOR),
        Perm::InputMonitoring => ("input", icon::KEYBOARD),
        Perm::Microphone => ("microphone", icon::MICROPHONE),
    }
}

/// The status and the action that fits it, added right to left: "Allowed" once granted, "Allow" (the system prompt)
/// while undecided, "Open System Settings" after that, since the prompt does not come back. `reset` adds a reset
/// button for a refused permission.
pub fn actions(ui: &mut Ui, proxy: &Proxy<AppEvent>, perm: Perm, status: Status, primary: bool, reset: bool) {
    let kind = if primary { ButtonKind::Primary } else { ButtonKind::Secondary };
    if status == Status::Granted {
        badge(ui, &t("onboarding.permissions.granted"), Tone::Success);
        return;
    }
    if status == Status::NotDetermined {
        if Button::new(&t("onboarding.permissions.allow")).kind(kind).ui(ui).clicked() {
            permissions::request(perm);
        }
        return;
    }
    if Button::new(&t("onboarding.permissions.open_settings")).kind(kind).icon(icon::ARROW_SQUARE_OUT).ui(ui).clicked() {
        permissions::open_settings(perm);
    }
    if reset && permissions::can_reset() {
        reset_button(ui, proxy, perm);
    }
}

/// "Reset": clears the user's answer and asks again. It is only offered for a permission that is not granted, so
/// nothing that works can be lost by it.
fn reset_button(ui: &mut Ui, proxy: &Proxy<AppEvent>, perm: Perm) {
    if !widgets::ghost_button(ui, &t("onboarding.permissions.reset")).on_hover_text(t("onboarding.permissions.reset_hint")).clicked() {
        return;
    }
    // tccutil takes a moment; the prompt that follows needs no particular thread.
    let proxy = proxy.clone();
    let spawned = std::thread::Builder::new().name("snap-permission-reset".into()).spawn(move || match permissions::reset(perm) {
        Ok(()) => permissions::request(perm),
        Err(e) => {
            tracing::warn!("permission reset: {e}");
            proxy.send(AppEvent::Notice(Notice {
                kind: NoticeKind::Error,
                title: t("onboarding.permissions.reset_failed").into_owned(),
                body: crate::notices::error_text(&e),
                ..Notice::default()
            }));
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("cannot reset the permission: {e}");
    }
}

/// Shown while Screen Recording was asked for and is still off: macOS applies a new grant only to an app started after
/// it, and a switch that is on but still not honoured after a relaunch is a stale entry that a reset clears. `reset`
/// adds that reset here, for lists whose rows do not carry one.
pub fn relaunch_hint(ui: &mut Ui, cx: &mut ViewCx, reset: bool) {
    ui.add(egui::Label::new(text::caption(t("onboarding.permissions.relaunch_hint"))).wrap());
    ui.horizontal(|ui| {
        // Secondary: the row's "Open System Settings" above is the step that comes first.
        if Button::new(&t("onboarding.permissions.relaunch")).icon(icon::ARROWS_CLOCKWISE).ui(ui).clicked() {
            relaunch(cx);
        }
        if reset && permissions::can_reset() {
            reset_button(ui, &cx.proxy, Perm::ScreenRecording);
        }
    });
}

/// Start a new instance and quit this one. The new one comes back where the user was: the first run resumes its
/// onboarding card by itself (from `--background`); otherwise the Settings window opens.
fn relaunch(cx: &mut ViewCx) {
    let args: &[&str] = if cx.settings.general.first_run_done { &["settings"] } else { &["--background"] };
    match permissions::relaunch(args) {
        Ok(()) => cx.push(Command::Quit),
        Err(e) => cx.toast_error(&e),
    }
}
