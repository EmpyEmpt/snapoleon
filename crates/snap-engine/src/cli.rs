//! Command line. `snapoleon` with no subcommand starts the tray app; GUI-bound
//! subcommands forward to the running instance over IPC; the rest run
//! headless right here.
//!
//! Exit codes: 0 success, 1 failure, 2 usage error (bad arguments).

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use snap_core::{
    Rect,
    hotkey::Action,
    ipc::{AppWindow, Request, parse_url},
    settings::{ImageFormat, LinkFormat, VideoFormat},
    t,
};

mod headless;
mod system;

pub use headless::run;
pub use system::install_cli;

#[derive(Debug, Parser)]
#[command(name = "snapoleon", version, about = "Screenshots, recording, OCR and sharing")]
pub struct Cli {
    /// Start in the tray without showing a window (used by autostart).
    #[arg(long)]
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
    /// Quit the running app.
    Quit,
    /// Run the MCP server on stdio.
    Mcp,
    /// Take a screenshot. Without a target the selection overlay opens.
    Capture(CaptureArgs),
    /// Record the screen. Without a target the recorder opens.
    Record(RecordArgs),
    /// Recognise text in an image.
    Ocr(OcrArgs),
    /// Decode or generate QR codes.
    Qr {
        #[command(subcommand)]
        action: QrCommand,
    },
    /// Upload a file and print its link.
    Upload(UploadArgs),
    /// Search the capture history.
    History {
        #[command(subcommand)]
        action: HistoryCommand,
    },
    /// List or run pipelines.
    Pipeline {
        #[command(subcommand)]
        action: PipelineCommand,
    },
    /// List monitors or windows, with the indexes, ids and rectangles the capture options take.
    List {
        #[command(subcommand)]
        what: ListCommand,
    },
    /// Check the installation: version, paths, the running app and the privacy permissions.
    #[command(alias = "permissions")]
    Doctor {
        /// Print the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Put a `snapoleon` command on PATH that runs this installation (macOS and Linux).
    InstallCli {
        /// Folder to put it in (default: /usr/local/bin on macOS, ~/.local/bin on Linux).
        #[arg(long, value_name = "DIR")]
        dir: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Default, Args)]
#[command(group(clap::ArgGroup::new("target").args(["region", "window", "monitor", "all", "element"])))]
pub struct CaptureArgs {
    /// Capture a rectangle of the desktop, in physical pixels.
    #[arg(long, value_name = "X,Y,W,H", value_parser = parse_region)]
    pub region: Option<Rect>,
    /// Capture the window whose title contains this text.
    #[arg(long, value_name = "TITLE")]
    pub window: Option<String>,
    /// Capture one monitor (0 is the primary).
    #[arg(long, value_name = "N")]
    pub monitor: Option<usize>,
    /// Capture all monitors as one image.
    #[arg(long)]
    pub all: bool,
    /// Capture the UI element under the cursor.
    #[arg(long)]
    pub element: bool,
    /// Run this pipeline instead of the plain save.
    #[arg(long, value_name = "ID")]
    pub pipeline: Option<String>,
    /// Write the image here ("-" for stdout).
    #[arg(long, short, value_name = "PATH")]
    pub out: Option<PathBuf>,
    /// Image format (default: from --out, else from settings).
    #[arg(long, value_enum)]
    pub format: Option<ImageFormatArg>,
    /// Also put the image on the clipboard.
    #[arg(long)]
    pub copy: bool,
    /// Wait this many seconds before capturing.
    #[arg(long, value_name = "SECONDS")]
    pub delay: Option<u32>,
    /// Include the mouse cursor.
    #[arg(long)]
    pub cursor: bool,
    /// Print the outcome as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone, Default, Args)]
#[command(group(clap::ArgGroup::new("target").args(["region", "window", "monitor"])))]
pub struct RecordArgs {
    /// Record a rectangle of the desktop, in physical pixels.
    #[arg(long, value_name = "X,Y,W,H", value_parser = parse_region)]
    pub region: Option<Rect>,
    /// Record the window whose title contains this text.
    #[arg(long, value_name = "TITLE")]
    pub window: Option<String>,
    /// Record one monitor (0 is the primary).
    #[arg(long, value_name = "N")]
    pub monitor: Option<usize>,
    /// Stop after this many seconds (default: until Enter or Ctrl+C).
    #[arg(long, value_name = "SECONDS")]
    pub duration: Option<f32>,
    /// Output format (default: from --out, else from settings).
    #[arg(long, value_enum)]
    pub format: Option<VideoFormatArg>,
    /// Write the video here.
    #[arg(long, short, value_name = "PATH")]
    pub out: Option<PathBuf>,
}

#[derive(Debug, Clone, Args)]
#[command(group(clap::ArgGroup::new("output").args(["markdown", "tsv", "json"])))]
pub struct OcrArgs {
    /// Image file, or "-" to read the image from stdin.
    pub input: String,
    /// Print a table as Markdown.
    #[arg(long)]
    pub markdown: bool,
    /// Print a table as tab-separated values.
    #[arg(long)]
    pub tsv: bool,
    /// Print lines, words and boxes as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone, Subcommand)]
pub enum QrCommand {
    /// Read the codes in an image.
    Decode {
        /// Image file, or "-" to read the image from stdin.
        input: String,
        /// Print format, text and position as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Draw a QR code as a PNG.
    Encode {
        text: String,
        /// Write the image here ("-" for stdout).
        #[arg(long, short, value_name = "PATH")]
        out: PathBuf,
        /// Pixels per module.
        #[arg(long, value_name = "PX", default_value_t = 8)]
        size: u32,
    },
}

#[derive(Debug, Clone, Args)]
pub struct UploadArgs {
    pub file: PathBuf,
    /// Destination id (default: the default destination).
    #[arg(long, short, value_name = "ID")]
    pub destination: Option<String>,
    /// How to print the link (default: from settings).
    #[arg(long, value_enum)]
    pub format: Option<LinkFormatArg>,
    /// Also copy the printed link to the clipboard.
    #[arg(long)]
    pub copy: bool,
}

#[derive(Debug, Clone, Subcommand)]
pub enum HistoryCommand {
    /// Search by text, app, window title, URL or tag; without a query, list the latest.
    Search {
        query: Vec<String>,
        /// Print entries as JSON.
        #[arg(long)]
        json: bool,
        /// Most results to return.
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum PipelineCommand {
    /// List the configured pipelines.
    List {
        /// Print pipelines as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Run a pipeline over an image file.
    Run {
        id: String,
        image: PathBuf,
        /// Print the outcome as JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum ListCommand {
    /// The connected displays: index, name, rectangle in physical pixels, scale factor.
    Monitors {
        /// Print monitors as JSON.
        #[arg(long)]
        json: bool,
    },
    /// The top-level windows, front to back: id, app, title, rectangle in physical pixels.
    Windows {
        /// Print windows as JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ImageFormatArg {
    Png,
    #[value(alias = "jpeg")]
    Jpg,
    Webp,
}

impl From<ImageFormatArg> for ImageFormat {
    fn from(f: ImageFormatArg) -> Self {
        match f {
            ImageFormatArg::Png => ImageFormat::Png,
            ImageFormatArg::Jpg => ImageFormat::Jpeg,
            ImageFormatArg::Webp => ImageFormat::Webp,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum VideoFormatArg {
    Mp4,
    Gif,
    Webp,
}

impl From<VideoFormatArg> for VideoFormat {
    fn from(f: VideoFormatArg) -> Self {
        match f {
            VideoFormatArg::Mp4 => VideoFormat::Mp4,
            VideoFormatArg::Gif => VideoFormat::Gif,
            VideoFormatArg::Webp => VideoFormat::Webp,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LinkFormatArg {
    Url,
    Markdown,
    Html,
    Bbcode,
}

impl From<LinkFormatArg> for LinkFormat {
    fn from(f: LinkFormatArg) -> Self {
        match f {
            LinkFormatArg::Url => LinkFormat::Url,
            LinkFormatArg::Markdown => LinkFormat::Markdown,
            LinkFormatArg::Html => LinkFormat::Html,
            LinkFormatArg::Bbcode => LinkFormat::BbCode,
        }
    }
}

fn parse_region(s: &str) -> std::result::Result<Rect, String> {
    Rect::parse(s).ok_or_else(|| t("cli.error.bad_region").into_owned())
}

/// Where a parsed command line should go.
#[derive(Debug, Clone, PartialEq)]
pub enum Dispatch {
    /// Start the GUI (or forward to the running one); `None` = just show it.
    Gui(Option<Request>),
    /// Run [`run`] in this process and exit with its code.
    Headless,
    /// A `snapoleon://` link that names no action a link may start: nothing runs, the app is not even launched.
    RefusedLink,
}

pub fn dispatch(cli: &Cli) -> Dispatch {
    match &cli.command {
        None => Dispatch::Gui(None),
        Some(Command::Url { url }) => parse_url(url).map_or(Dispatch::RefusedLink, |request| Dispatch::Gui(Some(request))),
        Some(Command::Edit { path }) => Dispatch::Gui(Some(Request::Edit { path: absolute(path) })),
        Some(Command::Pin { path }) => Dispatch::Gui(Some(Request::Pin { path: absolute(path) })),
        Some(Command::Settings) => Dispatch::Gui(Some(Request::ShowWindow { window: AppWindow::Settings })),
        Some(Command::Quit) => Dispatch::Gui(Some(Request::Quit)),
        Some(Command::Capture(a)) => capture_dispatch(a),
        Some(Command::Record(a)) => record_dispatch(a),
        Some(
            Command::Mcp
            | Command::Ocr(_)
            | Command::Qr { .. }
            | Command::Upload(_)
            | Command::History { .. }
            | Command::Pipeline { .. }
            | Command::List { .. }
            | Command::Doctor { .. }
            | Command::InstallCli { .. },
        ) => Dispatch::Headless,
    }
}

/// The running app has its own working directory, so a relative path from this command line means nothing to it.
fn absolute(path: &str) -> String {
    std::path::absolute(path).map_or_else(|_| path.to_owned(), |p| p.display().to_string())
}

/// A capture runs headless when it names its target and no pipeline (so there is nothing interactive left to do),
/// or when it only makes sense headless (`--out`, `--copy`, `--format`, `--json`). Everything else is the app's job.
fn capture_dispatch(a: &CaptureArgs) -> Dispatch {
    let explicit = a.region.is_some() || a.window.is_some() || a.monitor.is_some() || a.all || a.element;
    let headless_flags = a.out.is_some() || a.copy || a.format.is_some() || a.json;
    if a.pipeline.is_none() && (explicit || headless_flags) {
        return Dispatch::Headless;
    }
    let action = if a.region.is_some() {
        Action::CaptureRegion
    } else if a.window.is_some() {
        Action::CaptureWindow
    } else if a.monitor.is_some() {
        Action::CaptureMonitor
    } else if a.all {
        Action::CaptureAllMonitors
    } else if a.element {
        Action::CaptureElement
    } else if a.delay.is_some() {
        Action::CaptureDelayed
    } else {
        Action::CaptureRegion
    };
    Dispatch::Gui(Some(Request::Action {
        action,
        pipeline: a.pipeline.clone(),
        region: a.region,
        window_title: a.window.clone(),
        monitor: a.monitor,
    }))
}

fn record_dispatch(a: &RecordArgs) -> Dispatch {
    if a.region.is_some() || a.window.is_some() || a.monitor.is_some() || a.out.is_some() || a.duration.is_some() {
        return Dispatch::Headless;
    }
    let action = if a.format == Some(VideoFormatArg::Gif) { Action::RecordGif } else { Action::RecordRegion };
    Dispatch::Gui(Some(Request::Action { action, pipeline: None, region: None, window_title: None, monitor: None }))
}

// --- parsing with localised help ------------------------------------------------------------------------------------

/// The clap command with help texts in the active language (`cli.cmd.*` / `cli.arg.*` keys; missing keys keep the
/// built-in English). Load the language first (`snap_core::i18n::set_language`).
pub fn command() -> clap::Command {
    let cmd = Cli::command();
    let cmd = match lookup("cli.about") {
        Some(about) => cmd.about(about),
        None => cmd,
    };
    let cmd = match lookup("cli.after_help") {
        Some(text) => cmd.after_help(text),
        None => cmd,
    };
    localize(cmd, "")
}

/// Parse `std::env::args` with localised help and usage errors; exits on `--help`, `--version` and bad input (code 2).
/// A bare URL or file path works without a subcommand, see [`normalize`].
pub fn parse() -> Cli {
    let matches = command().get_matches_from(normalize(std::env::args_os()));
    Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit())
}

/// Parse an explicit argument list (a second instance forwarding its arguments); never exits.
pub fn parse_from<I, T>(args: I) -> std::result::Result<Cli, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let matches = command().try_get_matches_from(normalize(args.into_iter().map(Into::into)))?;
    Cli::from_arg_matches(&matches)
}

/// Desktop launchers hand over the thing that was opened as a bare argument (`snapoleon %U` in the Linux desktop
/// entry, a file dropped on the executable). A first operand that is not a subcommand becomes `url <operand>` when
/// it is a `snapoleon://` URL and `edit <operand>` when it names an existing path; further operands are dropped, since the app opens one
/// thing per launch.
fn normalize(args: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
    let mut args: Vec<OsString> = args.into_iter().collect();
    let Some(at) = args.iter().skip(1).position(|a| !a.to_string_lossy().starts_with('-')).map(|p| p + 1) else {
        return args;
    };
    let word = args[at].to_string_lossy().into_owned();
    let command = command();
    if word == "help" || command.get_subcommands().any(|s| s.get_name() == word || s.get_all_aliases().any(|a| a == word)) {
        return args;
    }
    let scheme = format!("{}:", snap_core::URL_SCHEME);
    let subcommand = if word.get(..scheme.len()).is_some_and(|start| start.eq_ignore_ascii_case(&scheme)) {
        "url"
    } else if Path::new(&word).exists() {
        "edit"
    } else {
        // Not ours to guess: clap reports the unknown subcommand.
        return args;
    };
    args.truncate(at + 1);
    args.insert(at, subcommand.into());
    args
}

fn lookup(key: &str) -> Option<String> {
    let text = t(key);
    (text != key).then(|| text.into_owned())
}

/// Apply translations by path: command `history search` reads `cli.cmd.history.search`, its argument `limit`
/// reads `cli.arg.history.search.limit`.
fn localize(mut cmd: clap::Command, path: &str) -> clap::Command {
    let dotted = |prefix: &str, id: &str| if path.is_empty() { format!("{prefix}.{id}") } else { format!("{prefix}.{path}.{id}") };
    if !path.is_empty()
        && let Some(about) = lookup(&format!("cli.cmd.{path}"))
    {
        cmd = cmd.about(about);
    }
    let ids: Vec<String> = cmd.get_arguments().map(|a| a.get_id().to_string()).collect();
    for id in ids {
        if let Some(help) = lookup(&dotted("cli.arg", &id)) {
            cmd = cmd.mut_arg(id, |a| a.help(help));
        }
    }
    let subs: Vec<String> = cmd.get_subcommands().map(|s| s.get_name().to_string()).collect();
    for name in subs {
        let child = if path.is_empty() { name.clone() } else { format!("{path}.{name}") };
        cmd = cmd.mut_subcommand(name, |s| localize(s, &child));
    }
    cmd
}
