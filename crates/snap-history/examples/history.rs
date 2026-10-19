//! Smoke run for snap-history. Builds a database and thumbnails under
//! `target/smoke/snap-history`, exercises the public API and asserts every
//! result, so a regression exits non-zero.
//!
//! Run with `cargo run -p snap-history --example history`.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use image::{Rgba, RgbaImage};
use snap_core::{Capture, CaptureContext, CaptureId, CaptureKind, Error, now_ms};
use snap_history::{Entry, History, LinkRecord, Query, kind_label, write_thumbnail};

const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 86_400_000;

struct Sample {
    kind: CaptureKind,
    app: &'static str,
    title: &'static str,
    text: &'static str,
    age_ms: i64,
    color: [u8; 3],
}

fn add_sample(db: &History, dir: &Path, s: &Sample) -> Result<CaptureId> {
    let [r, g, b] = s.color;
    let image = RgbaImage::from_pixel(1280, 800, Rgba([r, g, b, 255]));
    let mut capture = Capture::from_image(s.kind, image);
    capture.context = CaptureContext { app_name: Some(s.app.into()), window_title: Some(s.title.into()), ..Default::default() };
    capture.taken_at_ms = now_ms() - s.age_ms;

    let file = dir.join(format!("{}.png", capture.id));
    capture.image.save(&file).context("saving sample image")?;
    let bytes = fs::metadata(&file)?.len();
    let mut entry = Entry::from_capture(&capture, Some(file), bytes);
    entry.text = Some(s.text.into());
    entry.thumb = Some(write_thumbnail(&capture.image, &capture.id)?);
    db.add(&entry)?;
    Ok(capture.id)
}

fn show(label: &str, got: &[Entry]) {
    println!("ok  {label}: {} result(s)", got.len());
    for e in got {
        let app = e.context.app_name.as_deref().unwrap_or("-");
        let text = e.text.as_deref().unwrap_or("");
        println!("      {} [{}] {app} | {text}", e.id, kind_label(e.kind));
    }
}

/// Assert that `got` holds exactly `want`, in order.
fn check(label: &str, got: &[Entry], want: &[&CaptureId]) -> Result<()> {
    let got_ids: Vec<&CaptureId> = got.iter().map(|e| &e.id).collect();
    ensure!(got_ids == want, "{label}: expected {want:?}, got {got_ids:?}");
    show(label, got);
    Ok(())
}

fn query(text: &str) -> Query {
    Query { text: text.into(), ..Default::default() }
}

fn main() -> Result<()> {
    let out: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/smoke/snap-history");
    fs::create_dir_all(&out)?;
    let db_path = out.join("history.sqlite");
    for suffix in ["", "-wal", "-shm"] {
        let _ = fs::remove_file(out.join(format!("history.sqlite{suffix}")));
    }

    let db = History::open(&db_path)?;
    let samples = [
        Sample {
            kind: CaptureKind::Window,
            app: "Firefox",
            title: "Личный кабинет — Счета",
            text: "Счёт за электричество: итого 1 240 рублей",
            age_ms: HOUR_MS,
            color: [200, 80, 60],
        },
        Sample {
            kind: CaptureKind::Region,
            app: "Visual Studio Code",
            title: "main.rs - snapoleon",
            text: "fn capture_region(rect: Rect) -> Result<Capture>",
            age_ms: 2 * HOUR_MS,
            color: [40, 90, 200],
        },
        Sample {
            kind: CaptureKind::Window,
            app: "Snipping Tool",
            title: "Notes",
            text: "Quarterly roadmap meeting notes",
            age_ms: 60 * DAY_MS,
            color: [90, 160, 90],
        },
    ];
    let ids: Vec<CaptureId> = samples.iter().map(|s| add_sample(&db, &out, s)).collect::<Result<_>>()?;
    let (ru, code, old) = (&ids[0], &ids[1], &ids[2]);

    // Reopening exercises the migration check and WAL persistence.
    drop(db);
    let db = History::open(&db_path)?;
    ensure!(db.count()? == 3, "expected 3 rows after reopen");
    println!("ok  reopened database, {} rows", db.count()?);

    check("recent (empty query)", &db.search(&Query::default())?, &[ru, code, old])?;
    check("ru prefix 'элект'", &db.search(&query("элект"))?, &[ru])?;
    check("ru prefix, uppercase 'РУБ'", &db.search(&query("РУБ"))?, &[ru])?;
    check("en prefix 'captur'", &db.search(&query("captur"))?, &[code])?;
    check("en phrase words 'fn capt'", &db.search(&query("fn capt"))?, &[code])?;
    check("words are ANDed 'fn electr'", &db.search(&query("fn electr"))?, &[])?;

    let by_app = Query { app: Some("visual studio code".into()), ..Default::default() };
    check("app filter, case-insensitive", &db.search(&by_app)?, &[code])?;
    let by_kind = Query { kind: Some(CaptureKind::Window), ..Default::default() };
    check("kind filter = window", &db.search(&by_kind)?, &[ru, old])?;
    let page = Query { limit: 1, offset: 1, ..Default::default() };
    check("pagination limit 1 offset 1", &db.search(&page)?, &[code])?;
    let since = Query { from_ms: Some(now_ms() - DAY_MS), ..Default::default() };
    check("date filter (last 24 h)", &db.search(&since)?, &[ru, code])?;

    let apps = db.apps()?;
    println!("ok  apps: {apps:?}");
    ensure!(apps.contains(&("Firefox".to_string(), 1)), "apps() missing Firefox");

    // Edits must keep the FTS index in step (update triggers).
    db.set_text(code, "Scrolling demo")?;
    check("after set_text: 'captur' gone", &db.search(&query("captur"))?, &[])?;
    check("after set_text: 'scroll' found", &db.search(&query("scroll"))?, &[code])?;

    db.set_tags(old, &["Archive".into(), "  ".into(), "archive".into()])?;
    ensure!(db.get(old)?.map(|e| e.tags) == Some(vec!["Archive".to_string()]), "tags not normalised");
    check("tag 'archive' found", &db.search(&query("archive"))?, &[old])?;

    db.add_link(
        ru,
        &LinkRecord { url: "https://example.com/s/abc".into(), destination: "self-hosted".into(), delete_url: None, created_ms: now_ms() },
    )?;
    ensure!(db.get(ru)?.is_some_and(|e| e.links.len() == 1), "link not stored");
    println!("ok  link attached to {ru}");

    db.set_favorite(code, true)?;
    let favs = Query { favorites: true, ..Default::default() };
    check("favorites only", &db.search(&favs)?, &[code])?;

    let missing = CaptureId("does-not-exist".into());
    ensure!(matches!(db.set_text(&missing, "x"), Err(Error::NotFound(_))), "missing id must be NotFound");

    // Retention: only the 60-day-old capture goes. Its image file stays.
    let old_thumb = db.get(old)?.and_then(|e| e.thumb).context("old entry has no thumbnail")?;
    ensure!(old_thumb.exists(), "thumbnail should exist before purge");
    let purged = db.purge(30)?;
    ensure!(purged == 1, "purge(30) should remove 1 entry, removed {purged}");
    ensure!(!old_thumb.exists(), "purge should delete the thumbnail");
    ensure!(db.count()? == 2, "2 rows should remain after purge");
    println!("ok  purge(30) removed {purged} entry, thumbnail {}", old_thumb.display());

    // Deleting with delete_files removes the image; without it, only the thumbnail goes.
    let code_file = db.get(code)?.and_then(|e| e.file).context("code entry has no file")?;
    db.delete(code, true)?;
    ensure!(!code_file.exists(), "delete(_, true) should remove the image");
    check("after delete: 'scrolling' gone", &db.search(&query("scrolling"))?, &[])?;

    let ru_file = db.get(ru)?.and_then(|e| e.file).context("ru entry has no file")?;
    let ru_thumb = db.get(ru)?.and_then(|e| e.thumb).context("ru entry has no thumbnail")?;
    db.delete(ru, false)?;
    ensure!(ru_file.exists(), "delete(_, false) must keep the image");
    ensure!(!ru_thumb.exists(), "delete should remove the thumbnail");
    ensure!(db.count()? == 0, "database should be empty");
    ensure!(db.get(ru)?.is_none(), "deleted entry still present");
    println!("ok  delete: image kept = {}, thumbnail removed", ru_file.display());

    println!("all snap-history smoke checks passed");
    Ok(())
}
