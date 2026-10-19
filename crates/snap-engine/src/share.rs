//! Settings persistence, saving, uploading, shortening and link copying.

use std::{
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};

use snap_core::{
    Capture, CaptureId, Error, PROJECT_EXT, Result, RgbaImage, Settings,
    naming::{self, NameContext},
    now_ms, paths,
    policy::Policy,
    settings::{Destination, DestinationKind, ImageFormat, LinkFormat, RedactStyle, Shortener},
    t, tf,
};
use snap_history::LinkRecord;
use snap_upload::UploadRequest;

use crate::{Document, Engine, Link, SaveOptions, meta, recording, redact};

/// File names stop here; the OS limit is 255 bytes and we leave room for `" (99).webp"`.
const MAX_NAME_CHARS: usize = 150;

// --- settings ---------------------------------------------------------------------------------------------------

pub(crate) fn update_settings(engine: &Engine, f: impl FnOnce(&mut Settings)) -> Result<()> {
    let policy = Policy::load();
    let (edited, language_changed) = {
        let mut current = engine.inner.settings.write();
        let mut edited = current.clone();
        f(&mut edited);
        policy.apply(&mut edited);
        let language_changed = edited.general.language != current.general.language;
        *current = edited.clone();
        (edited, language_changed)
    };
    *engine.inner.policy.write() = policy.clone();
    after_settings_change(engine, &edited, language_changed);
    unmanaged(&edited, &policy)?.save(&paths::settings_file())
}

pub(crate) fn reload_settings(engine: &Engine) -> Result<()> {
    let fresh = Settings::load(&paths::settings_file())?;
    snap_core::settings::clear_read_only();
    let policy = Policy::load();
    let mut fresh = fresh;
    policy.apply(&mut fresh);
    let language_changed = std::mem::replace(&mut *engine.inner.settings.write(), fresh.clone()).general.language != fresh.general.language;
    *engine.inner.policy.write() = policy;
    after_settings_change(engine, &fresh, language_changed);
    Ok(())
}

fn after_settings_change(engine: &Engine, s: &Settings, language_changed: bool) {
    recording::apply_runtime_settings(s);
    if language_changed {
        snap_core::i18n::set_language(&s.general.language);
    }
    if s.history.enabled && engine.history().is_none() {
        match snap_history::History::open_default() {
            Ok(h) => *engine.inner.history.write() = Some(std::sync::Arc::new(h)),
            Err(e) => tracing::warn!("history unavailable: {e}"),
        }
    }
}

/// What goes to disk: fields that managed policy overrides keep their on-disk value, so lifting the policy later
/// gives the user their own configuration back instead of the forced one.
fn unmanaged(edited: &Settings, policy: &Policy) -> Result<Settings> {
    if !policy.is_managed() || snap_core::settings::is_read_only() {
        return Ok(edited.clone());
    }
    let disk = Settings::load(&paths::settings_file())?;
    let mut out = edited.clone();
    if policy.destinations.is_some() || policy.disable_uploads {
        out.destinations = disk.destinations.clone();
        out.default_destination = disk.default_destination.clone();
    }
    if policy.default_destination.is_some() {
        out.default_destination = disk.default_destination;
    }
    if policy.disable_ai {
        out.ai.enabled = disk.ai.enabled;
        out.translate.provider = disk.translate.provider;
    }
    if policy.disable_history {
        out.history.enabled = disk.history.enabled;
    }
    if policy.force_redact_before_upload {
        out.privacy.strip_metadata = disk.privacy.strip_metadata;
        out.privacy.redact_kinds = disk.privacy.redact_kinds;
    }
    if policy.retention_days.is_some() {
        out.history.retention_days = disk.history.retention_days;
    }
    if policy.language.is_some() {
        out.general.language = disk.general.language;
    }
    Ok(out)
}

// --- saving -----------------------------------------------------------------------------------------------------

pub(crate) fn save(engine: &Engine, img: &RgbaImage, cap: &Capture, opts: &SaveOptions) -> Result<PathBuf> {
    let s = engine.settings();
    let (path, fmt) = match &opts.path {
        Some(path) => {
            let fmt = snap_image::encode::format_from_path(path).or(opts.format).unwrap_or(s.capture.format);
            let path = if snap_image::encode::format_from_path(path).is_some() { path.clone() } else { with_appended_ext(path, fmt) };
            (path, fmt)
        }
        None => {
            let fmt = opts.format.unwrap_or(s.capture.format);
            let dir = opts.dir.clone().unwrap_or_else(|| paths::save_dir(&s.capture.save_dir));
            let template = opts.template.as_deref().unwrap_or(&s.capture.filename_template);
            (reserve_path(&dir, &file_stem(engine, template, cap), fmt)?, fmt)
        }
    };
    let saved = snap_image::encode::save(img, &path, fmt, quality(&s, fmt), s.capture.optimize_png);
    if saved.is_err() && opts.path.is_none() {
        // Do not leave the empty file that reserved the name.
        let _ = std::fs::remove_file(&path);
    }
    saved?;
    Ok(path)
}

fn with_appended_ext(path: &Path, fmt: ImageFormat) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{}", fmt.ext()));
    PathBuf::from(name)
}

/// Expand the naming template for `cap`; never empty and safe as a file name on every OS.
fn file_stem(engine: &Engine, template: &str, cap: &Capture) -> String {
    let n = engine.inner.counter.fetch_add(1, Ordering::Relaxed) + 1;
    let name = naming::expand(template, &NameContext::for_capture(cap, n));
    let name: String = name.chars().take(MAX_NAME_CHARS).collect();
    // Windows silently drops trailing dots and spaces, which would make the exists() check below lie.
    let name = name.trim_end_matches(['.', ' ']);
    if name.is_empty() { snap_core::APP_NAME.to_string() } else { name.to_string() }
}

/// Claim `dir/stem.ext`, or `dir/stem (2).ext` and so on when that exists already. The name is taken by creating the
/// file, so two saves in the same second (threads, or the CLI beside the app) can never choose the same one.
fn reserve_path(dir: &Path, stem: &str, fmt: ImageFormat) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    for n in 1.. {
        let name = if n == 1 { format!("{stem}.{}", fmt.ext()) } else { format!("{stem} ({n}).{}", fmt.ext()) };
        let path = dir.join(name);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
    }
    unreachable!("an unused name exists")
}

fn quality(s: &Settings, fmt: ImageFormat) -> u8 {
    match fmt {
        ImageFormat::Jpeg => s.capture.jpeg_quality,
        ImageFormat::Webp => s.capture.webp_quality,
        ImageFormat::Png => 100,
    }
}

pub(crate) fn save_project(engine: &Engine, doc: &Document, id: &CaptureId) -> Result<PathBuf> {
    let dir = paths::data_dir().join("projects");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{id}.{PROJECT_EXT}"));
    snap_annotate::save_project(doc, &path)?;
    if let Some(history) = engine.history()
        && let Ok(Some(entry)) = history.get(id)
        && let Err(e) = history.set_file(id, entry.file.as_deref(), Some(&path))
    {
        tracing::warn!("cannot link project {} in history: {e}", path.display());
    }
    Ok(path)
}

// --- uploading --------------------------------------------------------------------------------------------------

// Every upload (pipeline step, Library, editor, video export, CLI, MCP) enters through `upload_file` or `upload_image`,
// and both prepare the data the same way before `finish_upload` sends it: forced redaction under policy, then
// metadata stripping. Nothing else may hand bytes to an uploader.

pub(crate) fn upload_file(
    engine: &Engine,
    path: &Path,
    dest: Option<&str>,
    id: Option<&CaptureId>,
    progress: &dyn Fn(u64, u64),
    shorten: bool,
) -> Result<Link> {
    check_uploads_allowed(engine)?;
    let bytes = std::fs::read(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => Error::NotFound(path.display().to_string()),
        _ => Error::Io(e),
    })?;
    let ext = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    let bytes = prepare_file(engine, bytes)?;
    let name = path.file_name().map_or_else(|| "file".to_string(), |n| n.to_string_lossy().into_owned());
    finish_upload(engine, &bytes, &name, mime_for(&ext), None, dest, id, progress, shorten)
}

pub(crate) fn upload_image(
    engine: &Engine,
    img: &RgbaImage,
    cap: &Capture,
    dest: Option<&str>,
    progress: &dyn Fn(u64, u64),
    shorten: bool,
) -> Result<Link> {
    check_uploads_allowed(engine)?;
    let s = engine.settings();
    let forced = engine.policy().force_redact_before_upload;
    let fmt = s.capture.format;
    let bytes = if forced {
        let mut covered = img.clone();
        redact::auto_redact(engine, &mut covered, &[], Some(RedactStyle::Solid))?;
        snap_image::encode::encode(&covered, fmt, quality(&s, fmt), s.capture.optimize_png)?
    } else {
        snap_image::encode::encode(img, fmt, quality(&s, fmt), s.capture.optimize_png)?
    };
    let name = format!("{}.{}", file_stem(engine, &s.capture.filename_template, cap), fmt.ext());
    // The window title is text the redaction never saw, and context the user may have turned off.
    let title = cap.context.window_title.as_deref().filter(|_| s.privacy.record_context && !forced);
    finish_upload(engine, &bytes, &name, fmt.mime(), title, dest, Some(&cap.id), progress, shorten)
}

fn check_uploads_allowed(engine: &Engine) -> Result<()> {
    if engine.uploads_allowed() { Ok(()) } else { Err(Error::Policy(t("engine.error.uploads_disabled").into_owned())) }
}

/// The bytes of a file as they may leave the machine. Under `ForceRedactBeforeUpload` only raster images qualify: they
/// are decoded, covered and re-encoded; anything else cannot be redacted and is refused. Then metadata is stripped.
fn prepare_file(engine: &Engine, bytes: Vec<u8>) -> Result<Vec<u8>> {
    let s = engine.settings();
    let bytes = if engine.policy().force_redact_before_upload {
        let format = meta::image_format(&bytes).ok_or_else(|| Error::Policy(t("engine.error.redact_unsupported").into_owned()))?;
        let mut img = snap_image::encode::decode(&bytes)?;
        redact::auto_redact(engine, &mut img, &[], Some(RedactStyle::Solid))?;
        snap_image::encode::encode(&img, format, quality(&s, format), s.capture.optimize_png)?
    } else {
        bytes
    };
    if s.privacy.strip_metadata { meta::strip(bytes) } else { Ok(bytes) }
}

#[allow(clippy::too_many_arguments)]
fn finish_upload(
    engine: &Engine,
    data: &[u8],
    name: &str,
    mime: &str,
    title: Option<&str>,
    dest: Option<&str>,
    id: Option<&CaptureId>,
    progress: &dyn Fn(u64, u64),
    shorten: bool,
) -> Result<Link> {
    let destination = resolve_destination(engine, dest)?;
    let expiry = engine.settings().links.expiry_hours;
    let uploader = snap_upload::uploader_for(&destination, &engine.secrets())?;
    let req = UploadRequest { data, file_name: name, mime, title, expiry_hours: (expiry > 0).then_some(expiry), password: None };
    let done = uploader.upload(&req, progress)?;
    let mut link = Link {
        url: done.url,
        short_url: None,
        delete_url: done.delete_url,
        thumbnail_url: done.thumbnail_url,
        destination: destination.id.clone(),
    };
    if shorten && let Err(e) = shorten_link(engine, &mut link) {
        tracing::warn!("link shortener failed, keeping the full link: {e}");
    }
    if let Some(id) = id {
        record_link(engine, id, &link);
    }
    Ok(link)
}

/// The destination to use: the named one, the default, or the only one configured.
fn resolve_destination(engine: &Engine, id: Option<&str>) -> Result<Destination> {
    check_uploads_allowed(engine)?;
    let s = engine.settings();
    let found = match id.or(s.default_destination.as_deref()) {
        Some(id) => Some(s.destination(id).ok_or_else(|| Error::NotFound(tf("engine.error.unknown_destination", &[("id", id)])))?),
        None => (s.destinations.len() == 1).then(|| &s.destinations[0]),
    };
    found.cloned().ok_or_else(|| Error::NotFound(t("engine.error.no_destination").into_owned()))
}

/// Add the short link to `link` per `settings.links.shortener`; a no-op when none is configured.
pub(crate) fn shorten_link(engine: &Engine, link: &mut Link) -> Result<()> {
    let s = engine.settings();
    if link.short_url.is_some() || s.links.shortener == Shortener::None {
        return Ok(());
    }
    let server = snap_server_url(&s, &link.destination);
    link.short_url = Some(snap_upload::shorten(&link.url, &s.links.shortener, server.as_deref(), &*engine.secrets())?);
    Ok(())
}

/// Team server base URL for the SnapServer shortener: the destination the file went to, else the first such destination.
fn snap_server_url(s: &Settings, destination: &str) -> Option<String> {
    let url_of = |d: &Destination| match &d.kind {
        DestinationKind::SnapServer { url } => Some(url.clone()),
        _ => None,
    };
    s.destination(destination).and_then(url_of).or_else(|| s.destinations.iter().find_map(url_of))
}

fn record_link(engine: &Engine, id: &CaptureId, link: &Link) {
    let Some(history) = engine.history() else { return };
    let record = |url: &str| LinkRecord {
        url: url.to_string(),
        destination: link.destination.clone(),
        delete_url: link.delete_url.clone(),
        created_ms: now_ms(),
    };
    for url in std::iter::once(link.url.as_str()).chain(link.short_url.as_deref()) {
        match history.add_link(id, &record(url)) {
            Ok(()) => engine.history_changed(),
            // The capture has no history row (history off, or not recorded yet): nothing to attach the link to.
            Err(Error::NotFound(_)) => {}
            Err(e) => tracing::warn!("cannot record link in history: {e}"),
        }
    }
}

fn mime_for(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" | "jpe" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "pdf" => "application/pdf",
        "txt" | "md" | "log" => "text/plain",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "zip" => "application/zip",
        "snap" => "application/x-snapoleon",
        _ => "application/octet-stream",
    }
}

// --- links ------------------------------------------------------------------------------------------------------

pub(crate) fn copy_link(engine: &Engine, link: &Link, format: Option<LinkFormat>, title: Option<&str>, image: bool) -> Result<String> {
    let format = format.unwrap_or(engine.settings().links.copy_format);
    let text = format_link(link, format, title, image);
    match format {
        LinkFormat::Html => snap_platform::clipboard::set_html(&text, link.best())?,
        _ => snap_platform::clipboard::set_text(&text)?,
    }
    Ok(text)
}

/// The link in the requested markup. Images embed a direct image URL when the host gave one (or the URL is a plain
/// image file); otherwise, and for `Url`, the shareable link is used as a hyperlink.
pub(crate) fn format_link(link: &Link, format: LinkFormat, title: Option<&str>, image: bool) -> String {
    let embed = image
        .then(|| link.thumbnail_url.as_deref().or_else(|| is_image_url(&link.url).then_some(link.url.as_str())))
        .flatten()
        .filter(|_| format != LinkFormat::Url);
    match embed {
        Some(url) => snap_upload::format_link(url, format, title, true),
        None => snap_upload::format_link(link.best(), format, title, false),
    }
}

fn is_image_url(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url).to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".webp", ".gif", ".bmp"].iter().any(|ext| path.ends_with(ext))
}
