//! Headless execution of the CLI commands: results go to stdout in a machine-friendly form, progress and
//! diagnostics to stderr.

use std::{
    io::{IsTerminal, Read, Write},
    path::Path,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use serde_json::json;
use snap_core::{
    CaptureKind, Error, Result, RgbaImage, Settings,
    pipeline::Step,
    settings::{ImageFormat, VideoFormat},
    t, tf,
};
use snap_history::{History, Query};

use super::{CaptureArgs, Cli, Command, HistoryCommand, OcrArgs, PipelineCommand, QrCommand, RecordArgs, UploadArgs, system};
use crate::{
    Annotated, CaptureRequest, Engine, Notice, NoticeKind, Outcome, PipelineUi, RecordTarget, SaveOptions, agent, capture, exec, share,
};

/// How often the recording loop looks at its stop conditions.
const POLL: Duration = Duration::from_millis(100);

/// Headless execution; returns the process exit code: 0 success, 1 failure, 2 usage error.
pub fn run(cli: Cli) -> Result<i32> {
    let Some(command) = cli.command else {
        eprintln!("{}", error_line(&Error::invalid(t("cli.error.no_command"))));
        return Ok(2);
    };
    snap_platform::policy::load();
    let settings = Settings::load_default();
    snap_core::i18n::set_language(&settings.general.language);
    match execute(command, settings) {
        Ok(code) => Ok(code),
        Err(e) => {
            eprintln!("{}", error_line(&e));
            Ok(if matches!(e, Error::Invalid(_)) { 2 } else { 1 })
        }
    }
}

pub(super) fn error_line(e: &Error) -> String {
    tf("cli.error.line", &[("message", &exec::error_text(e))])
}

fn execute(command: Command, settings: Settings) -> Result<i32> {
    match command {
        Command::Capture(args) => capture_cmd(args, settings),
        Command::Record(args) => record_cmd(args, settings),
        Command::Ocr(args) => ocr_cmd(args, &settings),
        Command::Qr { action } => qr_cmd(action),
        Command::Upload(args) => upload_cmd(args, settings),
        Command::History { action } => history_cmd(action),
        Command::Pipeline { action } => pipeline_cmd(action, settings),
        Command::Mcp => {
            crate::mcp::serve_stdio(Engine::new(settings))?;
            Ok(0)
        }
        Command::List { what } => system::list_cmd(what),
        Command::Doctor { json } => system::doctor_cmd(json),
        Command::InstallCli { dir } => {
            println!("{}", system::install_cli(dir.as_deref())?.display());
            Ok(0)
        }
        Command::Url { .. } | Command::Edit { .. } | Command::Pin { .. } | Command::Settings | Command::Quit => {
            eprintln!("{}", error_line(&Error::unsupported(t("cli.error.needs_app").into_owned())));
            Ok(1)
        }
    }
}

// --- UI -------------------------------------------------------------------------------------------------------------

/// Pipelines run from the terminal: notices and progress on stderr, text results on stdout.
struct CliUi {
    progress_on_tty: bool,
}

impl CliUi {
    fn new() -> Self {
        Self { progress_on_tty: std::io::stderr().is_terminal() }
    }
}

impl PipelineUi for CliUi {
    fn annotate(&self, cap: &snap_core::Capture) -> Result<Option<Annotated>> {
        Ok(Some(Annotated { image: cap.image.clone(), document: None }))
    }
    fn pin(&self, _img: &RgbaImage, _cap: &snap_core::Capture) {}
    fn notify(&self, n: &Notice) {
        // Failures are reported once, by `run`, as the final error line.
        if n.kind != NoticeKind::Error {
            eprintln!("{}: {}", n.title, n.body);
        }
    }
    fn progress(&self, label: &str, frac: Option<f32>) {
        if !self.progress_on_tty {
            return;
        }
        let line = match (label.is_empty(), frac) {
            (true, _) => String::new(),
            (false, Some(f)) => format!("{label} {:.0}%", f * 100.0),
            (false, None) => format!("{label}\u{2026}"),
        };
        eprint!("\r{line:<48}\r");
    }
    fn show_text(&self, _title: &str, text: &str) {
        println!("{text}");
    }
}

// --- capture --------------------------------------------------------------------------------------------------------

fn capture_cmd(args: CaptureArgs, mut settings: Settings) -> Result<i32> {
    let request = if let Some(r) = args.region {
        CaptureRequest::Rect(r)
    } else if let Some(title) = &args.window {
        CaptureRequest::WindowTitled(title.clone())
    } else if let Some(n) = args.monitor {
        CaptureRequest::Monitor(n)
    } else if args.all {
        CaptureRequest::AllMonitors
    } else if args.element {
        CaptureRequest::ElementAtCursor
    } else {
        return Err(Error::invalid(t("cli.error.need_target")));
    };
    if args.cursor {
        settings.capture.include_cursor = true;
    }
    let engine = Engine::new(settings);
    if let Some(secs) = args.delay {
        std::thread::sleep(Duration::from_secs(u64::from(secs)));
    }
    let cap = agent::capture(&engine, &request)?;
    let format: Option<ImageFormat> = args.format.map(Into::into);

    if args.out.as_deref() == Some(Path::new("-")) {
        let settings = engine.settings();
        let fmt = format.unwrap_or(settings.capture.format);
        let quality = if fmt == ImageFormat::Webp { settings.capture.webp_quality } else { settings.capture.jpeg_quality };
        write_stdout(&snap_image::encode::encode(&cap.image, fmt, quality, settings.capture.optimize_png)?)?;
        return Ok(0);
    }

    let steps = match &args.pipeline {
        Some(id) => engine
            .settings()
            .pipeline(id)
            .map(|p| p.steps.clone())
            .ok_or_else(|| Error::NotFound(tf("engine.error.no_pipeline", &[("id", id)])))?,
        None => {
            let mut steps = Vec::new();
            if args.copy {
                steps.push(Step::CopyImage);
            }
            steps.push(Step::Save { dir: None, template: None, format });
            steps
        }
    };
    let (width, height, screen_rect) = (cap.image.width(), cap.image.height(), cap.screen_rect);
    let save = SaveOptions { path: args.out, format, ..SaveOptions::default() };
    let outcome = engine.run_steps_with_save(cap, &steps, &save, &CliUi::new())?;
    if args.json {
        let mut value = outcome_json(&outcome);
        value["width"] = json!(width);
        value["height"] = json!(height);
        value["screen_rect"] = json!({ "x": screen_rect.x, "y": screen_rect.y, "w": screen_rect.w, "h": screen_rect.h });
        print_json(&value)?;
    } else {
        print_outcome(&outcome);
    }
    Ok(0)
}

pub(super) fn print_json(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value).map_err(|e| Error::Other(e.into()))?);
    Ok(())
}

/// The shareable result on stdout: the link if there is one, else the file.
fn print_outcome(o: &Outcome) {
    if let Some(link) = &o.link {
        println!("{}", link.best());
    } else if let Some(file) = &o.file {
        println!("{}", file.display());
    }
}

fn outcome_json(o: &Outcome) -> serde_json::Value {
    json!({
        "id": o.id,
        "file": o.file,
        "project": o.project,
        "url": o.link.as_ref().map(|l| &l.url),
        "short_url": o.link.as_ref().and_then(|l| l.short_url.as_ref()),
        "delete_url": o.link.as_ref().and_then(|l| l.delete_url.as_ref()),
        "text": o.text,
        "codes": o.codes,
        "alt_text": o.alt_text,
        "cancelled": o.cancelled,
    })
}

// --- record ---------------------------------------------------------------------------------------------------------

/// Set by Ctrl+C. The handler can only be installed once per process.
fn interrupted() -> Arc<AtomicBool> {
    static FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    FLAG.get_or_init(|| {
        let flag = Arc::new(AtomicBool::new(false));
        let handler_flag = flag.clone();
        if let Err(e) = ctrlc::set_handler(move || handler_flag.store(true, Ordering::Relaxed)) {
            tracing::warn!("Ctrl+C will not stop the recording gracefully: {e}");
        }
        flag
    })
    .clone()
}

fn record_cmd(args: RecordArgs, settings: Settings) -> Result<i32> {
    snap_platform::dpi::init();
    let engine = Engine::new(settings);
    if !engine.recording_allowed() {
        return Err(Error::Policy(t("engine.error.recording_disabled").into_owned()));
    }
    // Unlike a screenshot, a recording cannot be handed to the app: it would be the app's recording, not this command's.
    #[cfg(target_os = "macos")]
    if !agent::can_read_screen() {
        return Err(agent::host_permission_error());
    }
    let target = if let Some(r) = args.region {
        RecordTarget::Region(r)
    } else if let Some(title) = &args.window {
        RecordTarget::Window(capture::find_window(title)?.id)
    } else if let Some(n) = args.monitor {
        RecordTarget::Monitor(n)
    } else {
        return Err(Error::invalid(t("cli.error.need_target")));
    };
    let format = args
        .format
        .map(Into::into)
        .or_else(|| args.out.as_deref().and_then(video_format_from_path))
        .unwrap_or(engine.settings().recording.format);
    let mut options = engine.record_options(target, format);
    if let Some(out) = args.out {
        options.output = out;
    }
    let context = snap_capture::foreground_window().map(|w| snap_capture::context_for_window(&w)).unwrap_or_default();

    let interrupted = interrupted();
    let recorder = snap_record::start(options)?;
    eprintln!("{}", t(if args.duration.is_some() { "cli.record.started_timed" } else { "cli.record.started" }));
    let enter = Arc::new(AtomicBool::new(false));
    if args.duration.is_none() {
        let enter = enter.clone();
        std::thread::spawn(move || {
            // EOF (stdin closed or not a terminal) must not count as a request to stop.
            if matches!(std::io::stdin().read_line(&mut String::new()), Ok(n) if n > 0) {
                enter.store(true, Ordering::Relaxed);
            }
        });
    }
    let started = Instant::now();
    let limit = args.duration.map(|s| Duration::from_secs_f32(s.max(0.0)));
    while !interrupted.load(Ordering::Relaxed) && !enter.load(Ordering::Relaxed) && limit.is_none_or(|l| started.elapsed() < l) {
        if let Some(failure) = recorder.error() {
            recorder.cancel();
            return Err(Error::Other(anyhow::anyhow!(failure)));
        }
        std::thread::sleep(POLL);
    }
    let recording = recorder.stop()?;
    if let Err(e) = engine.add_recording(&recording, context) {
        tracing::warn!("recording not added to history: {e}");
    }
    println!("{}", recording.path.display());
    Ok(0)
}

fn video_format_from_path(path: &Path) -> Option<VideoFormat> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "mp4" => Some(VideoFormat::Mp4),
        "gif" => Some(VideoFormat::Gif),
        "webp" => Some(VideoFormat::Webp),
        _ => None,
    }
}

// --- ocr and codes --------------------------------------------------------------------------------------------------

fn load_image(input: &str) -> Result<RgbaImage> {
    if input != "-" {
        return snap_image::encode::load(Path::new(input));
    }
    let mut bytes = Vec::new();
    std::io::stdin().lock().read_to_end(&mut bytes)?;
    snap_image::encode::decode(&bytes)
}

fn ocr_cmd(args: OcrArgs, settings: &Settings) -> Result<i32> {
    let result = snap_ocr::recognize(&load_image(&args.input)?, &settings.ocr)?;
    if args.json {
        let mut value = serde_json::to_value(&result).map_err(|e| Error::Other(e.into()))?;
        value["text"] = json!(result.text());
        print_json(&value)?;
    } else if args.markdown || args.tsv {
        let table = if args.markdown { result.to_markdown_table() } else { result.to_tsv() };
        let Some(table) = table else {
            eprintln!("{}", t("cli.ocr.no_table"));
            return Ok(1);
        };
        println!("{table}");
    } else {
        let text = result.text();
        if !text.trim().is_empty() {
            println!("{text}");
        }
    }
    Ok(0)
}

fn qr_cmd(action: QrCommand) -> Result<i32> {
    match action {
        QrCommand::Decode { input, json } => {
            let codes = snap_ocr::codes::decode(&load_image(&input)?);
            if codes.is_empty() {
                eprintln!("{}", t("cli.qr.none_found"));
                return Ok(1);
            }
            if json {
                print_json(&codes)?;
            } else {
                for code in &codes {
                    println!("{}", code.text);
                }
            }
            Ok(0)
        }
        QrCommand::Encode { text, out, size } => {
            let img = snap_ocr::codes::try_qr(&text, size.clamp(1, 64))?;
            if out == Path::new("-") {
                write_stdout(&snap_image::encode::encode(&img, ImageFormat::Png, 100, false)?)?;
            } else {
                snap_image::encode::save(&img, &out, ImageFormat::Png, 100, false)?;
                println!("{}", out.display());
            }
            Ok(0)
        }
    }
}

fn write_stdout(bytes: &[u8]) -> Result<()> {
    let mut out = std::io::stdout().lock();
    out.write_all(bytes)?;
    out.flush()?;
    Ok(())
}

// --- upload ---------------------------------------------------------------------------------------------------------

fn upload_cmd(args: UploadArgs, settings: Settings) -> Result<i32> {
    let is_video = args
        .file
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "mp4" | "webm" | "mov" | "mkv"));
    let is_image = !is_video
        && matches!(args.file.extension().and_then(|e| e.to_str()), Some(e) if ["png", "jpg", "jpeg", "webp", "gif", "bmp"].contains(&e.to_ascii_lowercase().as_str()));
    let format = args.format.map(Into::into).unwrap_or(settings.links.copy_format);
    let engine = Engine::new(settings);
    let steps = [Step::Upload { destination: args.destination }, Step::Shorten];
    let kind = if is_video { CaptureKind::Recording } else { CaptureKind::File };
    let outcome = engine.run_file(&args.file, kind, &steps, &CliUi::new())?;
    let link = outcome.link.ok_or_else(|| Error::NotFound(t("engine.error.no_link").into_owned()))?;
    let title = args.file.file_stem().map(|s| s.to_string_lossy().into_owned());
    let text = share::format_link(&link, format, title.as_deref(), is_image);
    println!("{text}");
    if args.copy {
        snap_platform::clipboard::set_text(&text)?;
    }
    Ok(0)
}

// --- history and pipelines ------------------------------------------------------------------------------------------

fn history_cmd(action: HistoryCommand) -> Result<i32> {
    let HistoryCommand::Search { query, json, limit } = action;
    let entries = History::open_default()?.search(&Query { text: query.join(" "), limit, ..Query::default() })?;
    if json {
        print_json(&entries)?;
        return Ok(0);
    }
    for e in &entries {
        let when = chrono::DateTime::from_timestamp_millis(e.created_ms)
            .map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default();
        let kind = serde_json::to_value(e.kind).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
        let file = e.file.as_ref().map(|f| f.display().to_string()).unwrap_or_default();
        println!(
            "{}\t{when}\t{kind}\t{}\t{}\t{file}",
            e.id,
            e.context.app_name.as_deref().unwrap_or_default(),
            e.context.window_title.as_deref().unwrap_or_default()
        );
    }
    Ok(0)
}

fn pipeline_cmd(action: PipelineCommand, settings: Settings) -> Result<i32> {
    match action {
        PipelineCommand::List { json } => {
            if json {
                print_json(&settings.pipelines)?;
            } else {
                for p in &settings.pipelines {
                    println!("{}\t{}\t{}", p.id, p.name, p.steps.iter().map(step_name).collect::<Vec<_>>().join(","));
                }
            }
            Ok(0)
        }
        PipelineCommand::Run { id, image, json } => {
            let steps = settings
                .pipeline(&id)
                .map(|p| p.steps.clone())
                .ok_or_else(|| Error::NotFound(tf("engine.error.no_pipeline", &[("id", &id)])))?;
            let engine = Engine::new(settings);
            let outcome = engine.run_file(&image, CaptureKind::File, &steps, &CliUi::new())?;
            if json {
                print_json(&outcome_json(&outcome))?;
            } else {
                print_outcome(&outcome);
            }
            Ok(0)
        }
    }
}

/// The serde tag of a step (`auto_redact`, `copy_link`, ...).
fn step_name(step: &Step) -> String {
    serde_json::to_value(step).ok().and_then(|v| v["step"].as_str().map(str::to_owned)).unwrap_or_default()
}
