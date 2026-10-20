//! Snapoleon: screenshots, recording, OCR and sharing.
//!
//! `snapoleon` with no arguments starts the tray app (single instance). Headless subcommands run
//! right here; GUI-bound ones go to the running instance over IPC, or start it.

// A GUI-subsystem binary in release builds; `console::attach` gives CLI output back its terminal.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod actions;
mod app;
mod commands;
mod dev;
mod events;
mod hotkeys;
mod ipc;
mod jobs;
mod logging;
mod notices;
mod routing;
mod server_events;
mod theme;
mod tray;
mod views;

use std::process::ExitCode;

use snap_core::{APP_ID, Settings, i18n, policy::Policy};
use snap_engine::{
    Engine,
    cli::{self, Cli, Dispatch},
};
use snap_shell::ShellConfig;

use crate::app::{App, Startup};

fn main() -> ExitCode {
    // Portable dev data has to be in place before the first path lookup.
    dev::prepare();
    snap_platform::dpi::init();
    snap_platform::console::attach();

    // The language comes from the settings and decides the help text, so they are read before the arguments.
    let settings = load_settings();
    let cli = cli::parse();
    match cli::dispatch(&cli) {
        Dispatch::Headless => run_headless(cli),
        Dispatch::Gui(request) => run_gui(request, cli.background, settings),
        Dispatch::RefusedLink => {
            eprintln!("{}", snap_core::t("app.ipc.link_refused"));
            ExitCode::FAILURE
        }
    }
}

fn run_headless(cli: Cli) -> ExitCode {
    // Standard error carries warnings only: `snapoleon mcp` keeps standard output for the protocol, and the other
    // commands print their result there.
    init_logging(logging::Sink::Console);
    match cli::run(cli) {
        Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
        Err(e) => {
            eprintln!("{}", notices::error_text(&e));
            ExitCode::FAILURE
        }
    }
}

fn run_gui(request: Option<snap_core::ipc::Request>, background: bool, mut settings: Settings) -> ExitCode {
    let dev = dev::Dev::from_env();
    // The dev harness never talks to (or replaces) the installed app.
    if dev.is_none() && snap_platform::ipc::instance_running() {
        init_logging(logging::Sink::Console);
        return if ipc::forward(request, background) { ExitCode::SUCCESS } else { ExitCode::FAILURE };
    }
    // Nothing runs that could be told to quit: do not start the app only to end it.
    if dev.is_none() && request == Some(snap_core::ipc::Request::Quit) {
        return ExitCode::SUCCESS;
    }
    init_logging(logging::Sink::Application);

    let policy = Policy::load();
    if let Some(dev) = &dev {
        dev::apply_overrides(&mut settings, dev);
        i18n::set_language(&settings.general.language);
    }
    theme::apply(&settings.general, dev.as_ref().and_then(|d| d.dark));

    let engine = Engine::new(settings);
    let mut config = ShellConfig::new(APP_ID);
    config.accessory = true;
    config.theme_init = Box::new(theme::init);
    let app = App::new(Startup { engine, policy, request, background, dev });
    match snap_shell::run(config, app) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e}");
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// Starts logging, then repeats what `load_settings` could only say before there was a log.
fn init_logging(sink: logging::Sink) {
    logging::init(sink);
    if snap_core::settings::is_read_only() {
        tracing::warn!(
            "settings file {} is unreadable; using defaults and leaving it untouched",
            snap_core::paths::settings_file().display()
        );
    }
}

/// Policy, then settings (policy applied on top), then the UI language.
fn load_settings() -> Settings {
    snap_platform::policy::load();
    let settings = Settings::load_default();
    i18n::set_language(&settings.general.language);
    settings
}
