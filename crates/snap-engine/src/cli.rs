//! Command line. `snapoleon` with no subcommand starts the tray app; GUI-bound
//! subcommands forward to the running instance over IPC; the rest run
//! headless right here.

use clap::{Parser, Subcommand};
use snap_core::{Result, ipc::Request};

#[derive(Debug, Parser)]
#[command(name = "snapoleon", version, about = "Screenshots, recording, OCR and sharing")]
pub struct Cli {
    /// Start in the tray without showing a window (used by autostart).
    #[arg(long, global = true)]
    pub background: bool,
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Owned by snap-engine; extend freely.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Handle a snapoleon:// URL.
    Url { url: String },
    /// Open an image or .snap project in the editor.
    Edit { path: String },
    /// Pin an image to the screen.
    Pin { path: String },
    /// Open settings.
    Settings,
    /// Run the MCP server on stdio.
    Mcp,
}

/// Where a parsed command line should go.
#[derive(Debug, Clone, PartialEq)]
pub enum Dispatch {
    /// Start the GUI (or forward to the running one); `None` = just show it.
    Gui(Option<Request>),
    /// Run [`run`] in this process and exit with its code.
    Headless,
}

pub fn dispatch(cli: &Cli) -> Dispatch {
    match &cli.command {
        None => Dispatch::Gui(None),
        Some(Command::Mcp) => Dispatch::Headless,
        Some(Command::Url { url }) => Dispatch::Gui(snap_core::ipc::parse_url(url)),
        Some(Command::Edit { path }) => Dispatch::Gui(Some(Request::Edit { path: path.clone() })),
        Some(Command::Pin { path }) => Dispatch::Gui(Some(Request::Pin { path: path.clone() })),
        Some(Command::Settings) => Dispatch::Gui(Some(Request::ShowWindow { window: snap_core::ipc::AppWindow::Settings })),
    }
}

/// Headless execution; returns the process exit code.
pub fn run(cli: Cli) -> Result<i32> {
    let _ = cli;
    Err(crate::todo())
}
