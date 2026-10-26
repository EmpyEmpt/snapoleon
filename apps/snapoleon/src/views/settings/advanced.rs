//! Advanced: policy status, portable mode, paths, command line and MCP snippets, backup and reset.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use egui::Ui;
use snap_core::{Settings, paths, policy::Policy, tf};
use snap_ui::{Palette, ToastKind, Tone, icon, radius, space, text, widgets};

use super::kit::{Msg, Rows, managed_badge, s};
use snap_engine::{Notice, NoticeKind};

use crate::{commands::Command, events::AppEvent, views::ViewCx};

/// How long a lookup of the `snapoleon` command stays good: the Install button, or a terminal, may add it meanwhile.
const PROGRAM_TTL: Duration = Duration::from_secs(3);

#[derive(Default)]
pub struct State {
    program: Option<(Program, Instant)>,
}

/// What the snippets call.
#[derive(Clone)]
struct Program {
    /// This executable's full path, for configuration files read by programs that do not search the shell's PATH.
    exe: String,
    /// What to type in a shell: `snapoleon` when the command is installed, the full path otherwise.
    command: String,
}

impl Program {
    fn find() -> Self {
        let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "snapoleon".into());
        let command = if snap_engine::cli::installed_cli().is_some() {
            "snapoleon".to_string()
        } else if exe.contains(' ') {
            format!("\"{exe}\"")
        } else {
            exe.clone()
        };
        Self { exe, command }
    }
}

impl State {
    pub fn on_msg(&mut self, cx: &mut ViewCx, set: &mut Settings, msg: Msg) -> Option<Msg> {
        match msg {
            Msg::SettingsExported(Ok(path)) => {
                cx.toast(ToastKind::Success, tf("settings.advanced.exported", &[("path", &path.display().to_string())]));
            }
            Msg::SettingsImported(Ok(loaded)) => {
                *set = *loaded;
                set.general.first_run_done = true;
                cx.toast(ToastKind::Success, s("advanced.imported"));
            }
            Msg::SettingsExported(Err(e)) | Msg::SettingsImported(Err(e)) => {
                if !e.is_empty() {
                    cx.toast(ToastKind::Error, e);
                }
            }
            other => return Some(other),
        }
        None
    }
}

pub fn show(ui: &mut Ui, rc: &mut Rows, st: &mut State, cx: &mut ViewCx, set: &mut Settings) {
    let program = match &st.program {
        Some((program, at)) if at.elapsed() <= PROGRAM_TTL => program.clone(),
        _ => {
            let program = Program::find();
            st.program = Some((program.clone(), Instant::now()));
            program
        }
    };

    policy(ui, rc);
    installation(ui, rc, cx);
    command_line(ui, rc, cx, &program);
    backup(ui, rc, cx, set);
}

// ---------------------------------------------------------------------------------------------------------------------

/// Everything the administrator pinned, as (label, value).
fn policy_items(p: &Policy) -> Vec<(String, String)> {
    let on = s("advanced.policy.on");
    let mut items = Vec::new();
    let mut flag = |set: bool, key: &str, value: &str| {
        if set {
            items.push((s(&format!("advanced.policy.{key}")), value.to_owned()));
        }
    };
    flag(p.disable_uploads, "uploads", &s("advanced.policy.off"));
    flag(p.disable_recording, "recording", &s("advanced.policy.off"));
    flag(p.disable_ai, "ai", &s("advanced.policy.off"));
    flag(p.disable_history, "history", &s("advanced.policy.off"));
    flag(p.force_redact_before_upload, "redact", &on);
    if let Some(days) = p.retention_days {
        items.push((s("advanced.policy.retention"), tf("settings.advanced.policy.days", &[("n", &days.to_string())])));
    }
    if let Some(lang) = &p.language {
        items.push((s("advanced.policy.language"), lang.clone()));
    }
    if let Some(list) = &p.destinations {
        items.push((s("advanced.policy.destinations"), list.len().to_string()));
    }
    if let Some(id) = &p.default_destination {
        items.push((s("advanced.policy.default_destination"), id.clone()));
    }
    items
}

fn policy(ui: &mut Ui, rc: &mut Rows) {
    let items = policy_items(&rc.env.policy);
    let managed = rc.env.policy.is_managed();
    rc.section(ui, &s("advanced.section.policy"), |ui, rc| {
        rc.row(ui, "advanced.policy", |ui| {
            if managed {
                managed_badge(ui);
            } else {
                widgets::badge(ui, &s("advanced.policy.none"), Tone::Neutral);
            }
        });
        for (label, value) in &items {
            rc.row_raw(ui, false, label, "", |ui| {
                managed_badge(ui);
                ui.label(text::mono_text(value.as_str()));
            });
        }
    });
}

fn installation(ui: &mut Ui, rc: &mut Rows, cx: &mut ViewCx) {
    let portable = rc.env.portable;
    rc.section(ui, &s("advanced.section.install"), |ui, rc| {
        rc.row(ui, "advanced.portable", |ui| {
            let (label, tone) =
                if portable { (s("advanced.portable.yes"), Tone::Accent) } else { (s("advanced.portable.no"), Tone::Neutral) };
            widgets::badge(ui, &label, tone);
        });
        path_row(ui, rc, cx, "advanced.path.settings", &paths::settings_file(), true);
        path_row(ui, rc, cx, "advanced.path.data", &paths::data_dir(), false);
        path_row(ui, rc, cx, "advanced.path.history", &paths::history_db(), true);
        path_row(ui, rc, cx, "advanced.path.logs", &paths::logs_dir(), false);
        path_row(ui, rc, cx, "advanced.path.models", &paths::models_dir(), false);
        path_row(ui, rc, cx, "advanced.path.locales", &paths::user_locales_dir(), false);
    });
}

/// A row with a path in the description and Copy and Open buttons. A file is revealed, a folder opened.
fn path_row(ui: &mut Ui, rc: &mut Rows, cx: &mut ViewCx, key: &str, path: &Path, is_file: bool) {
    let shown = path.display().to_string();
    let (label, _) = super::kit::texts(key);
    rc.row_raw(ui, false, &label, &shown, |ui| {
        let open = if is_file { s("advanced.show") } else { s("open") };
        if widgets::secondary_button(ui, &open).clicked() {
            let buf: PathBuf = path.to_path_buf();
            if is_file {
                cx.push(Command::Reveal(buf));
            } else {
                // The folder may not exist yet on a fresh install.
                let _ = std::fs::create_dir_all(&buf);
                cx.push(Command::OpenPath(buf));
            }
        }
        if widgets::icon_button(ui, icon::COPY, &s("copy_path"), None).clicked() {
            cx.push(Command::CopyText(shown.clone()));
        }
    });
}

fn command_line(ui: &mut Ui, rc: &mut Rows, cx: &mut ViewCx, program: &Program) {
    let command = &program.command;
    // A window every copy of the OS has open, so the example works as written.
    let window = if cfg!(target_os = "macos") {
        "Finder"
    } else if cfg!(windows) {
        "Notepad"
    } else {
        "Terminal"
    };
    let cli = [
        "capture --monitor 0 --out screenshot.png",
        &format!("capture --window \"{window}\" --pipeline share"),
        "ocr screenshot.png",
        "upload screenshot.png",
        "edit screenshot.png",
        "pin screenshot.png",
        "history search invoice",
    ]
    .map(|args| format!("{command} {args}"))
    .join("\n");
    // Desktop apps start MCP servers without the shell's PATH, so their configuration names the executable itself.
    let mcp_json = serde_json::json!({ "mcpServers": { "snapoleon": { "command": program.exe, "args": ["mcp"] } } });
    let mcp = serde_json::to_string_pretty(&mcp_json).unwrap_or_default();
    let claude_code = format!("claude mcp add snapoleon -- {command} mcp");

    rc.section(ui, &s("advanced.section.cli"), |ui, rc| {
        if cfg!(not(windows)) {
            rc.row(ui, "advanced.install_cli", |ui| {
                if widgets::secondary_button(ui, &s("advanced.install_cli.button")).clicked() {
                    install_cli(cx);
                }
            });
        }
        rc.stacked(ui, false, "advanced.cli", |ui| snippet(ui, cx, &cli));
        rc.stacked(ui, false, "advanced.mcp_desktop", |ui| snippet(ui, cx, &mcp));
        rc.stacked(ui, false, "advanced.mcp_code", |ui| snippet(ui, cx, &claude_code));
    });
}

/// `snapoleon install-cli` on a worker (it may wait for the macOS administrator prompt); the result is a notice.
fn install_cli(cx: &ViewCx) {
    let proxy = cx.proxy.clone();
    std::thread::spawn(move || {
        let notice = match snap_engine::cli::install_cli(None) {
            Ok(path) => Notice {
                kind: NoticeKind::Success,
                title: s("advanced.install_cli.done"),
                body: path.display().to_string(),
                ..Notice::default()
            },
            Err(snap_core::Error::Cancelled) => return,
            Err(e) => Notice {
                kind: NoticeKind::Error,
                title: s("advanced.install_cli.failed"),
                body: crate::notices::error_text(&e),
                ..Notice::default()
            },
        };
        proxy.send(AppEvent::Notice(notice));
    });
}

/// Code in a framed block with a Copy button. Long lines scroll inside the block instead of widening the page.
fn snippet(ui: &mut Ui, cx: &mut ViewCx, code: &str) {
    let p = Palette::of(ui);
    egui::Frame::new().fill(p.surface_2).stroke(egui::Stroke::new(1.0, p.border)).corner_radius(radius::CARD).inner_margin(12).show(
        ui,
        |ui| {
            ui.set_width(ui.available_width());
            // The button goes first, at the right edge, so the code gets exactly the room that is left.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                if widgets::icon_button(ui, icon::COPY, &s("copy_snippet"), None).clicked() {
                    cx.push(Command::CopyText(code.to_owned()));
                    cx.toast(ToastKind::Success, snap_core::t("common.copied"));
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                    egui::ScrollArea::horizontal().id_salt(code).auto_shrink([false, true]).show(ui, |ui| {
                        ui.add(egui::Label::new(text::mono_text(code)).wrap_mode(egui::TextWrapMode::Extend).selectable(true));
                    });
                });
            });
        },
    );
}

fn backup(ui: &mut Ui, rc: &mut Rows, cx: &mut ViewCx, set: &mut Settings) {
    rc.section(ui, &s("advanced.section.backup"), |ui, rc| {
        rc.row(ui, "advanced.export", |ui| {
            if widgets::secondary_button(ui, &s("advanced.export.button")).clicked() {
                let current = cx.settings.clone();
                cx.spawn(move || {
                    let Some(path) =
                        rfd::FileDialog::new().set_file_name("snapoleon-settings.toml").add_filter("TOML", &["toml"]).save_file()
                    else {
                        return Msg::SettingsExported(Err(String::new()));
                    };
                    Msg::SettingsExported(current.save(&path).map(|()| path).map_err(|e| crate::notices::error_text(&e)))
                });
            }
        });
        rc.row(ui, "advanced.import", |ui| {
            if widgets::secondary_button(ui, &s("advanced.import.button")).clicked() {
                cx.spawn(|| {
                    let Some(path) = rfd::FileDialog::new().add_filter("TOML", &["toml"]).pick_file() else {
                        return Msg::SettingsImported(Err(String::new()));
                    };
                    Msg::SettingsImported(Settings::load(&path).map(Box::new).map_err(|e| crate::notices::error_text(&e)))
                });
            }
        });
        rc.row(ui, "advanced.reset", |ui| {
            if super::kit::confirm_button(ui, "reset-settings", &s("advanced.reset.button"), &s("advanced.reset.confirm")) {
                let keep = (set.destinations.clone(), set.default_destination.clone());
                *set = Settings::default();
                (set.destinations, set.default_destination) = keep;
                set.general.first_run_done = true;
            }
        });
    });
    ui.add_space(space::XS);
}
