//! `.snap` project files: a zip with `doc.json`, `base.png` and `assets/<shape id>.png`.

use std::fs::File;
use std::io::{BufWriter, Read, Seek, Write};
use std::path::Path;
use std::sync::Arc;

use image::RgbaImage;
use serde::Serialize;
use serde_json::Value;
use snap_core::settings::ImageFormat;
use snap_core::{Error, Result, tf};
use snap_image::encode;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::model::{Document, ShapeKind};

/// Format version written by this build. Bump it when `doc.json` changes shape and add the
/// matching step to [`migrate`].
const VERSION: u32 = 1;

/// Upper bound for any single entry, to keep a corrupt archive from exhausting memory.
const MAX_ENTRY: u64 = 1 << 30;

#[derive(Serialize)]
struct Envelope<'a> {
    version: u32,
    document: &'a Document,
}

fn bad(reason: impl ToString) -> Error {
    Error::Invalid(tf("annotate.error.damaged", &[("reason", &reason.to_string())]))
}

fn png(img: &RgbaImage) -> Result<Vec<u8>> {
    encode::encode(img, ImageFormat::Png, 100, false)
}

/// Writes the project atomically (temporary file, then rename).
pub fn save_project(doc: &Document, path: &Path) -> Result<()> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.tmp"));
    let written = write_archive(doc, &tmp).and_then(|()| std::fs::rename(&tmp, path).map_err(Error::from));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

fn write_archive(doc: &Document, tmp: &Path) -> Result<()> {
    let mut zip = ZipWriter::new(BufWriter::new(File::create(tmp)?));
    let packed = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    // PNG data is already compressed; storing it avoids burning CPU for nothing.
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    let json = serde_json::to_vec(&Envelope { version: VERSION, document: doc }).map_err(bad)?;
    put(&mut zip, "doc.json", packed, &json)?;
    put(&mut zip, "base.png", stored, &png(&doc.base)?)?;
    for shape in &doc.shapes {
        if let ShapeKind::Image { image, .. } = &shape.kind {
            put(&mut zip, &format!("assets/{}.png", shape.id), stored, &png(image)?)?;
        }
    }
    let mut out = zip.finish().map_err(bad)?;
    out.flush()?;
    out.get_ref().sync_all()?;
    Ok(())
}

fn put<W: Write + Seek>(zip: &mut ZipWriter<W>, name: &str, options: SimpleFileOptions, data: &[u8]) -> Result<()> {
    zip.start_file(name, options).map_err(bad)?;
    zip.write_all(data)?;
    Ok(())
}

pub fn load_project(path: &Path) -> Result<Document> {
    let mut zip =
        ZipArchive::new(File::open(path)?).map_err(|_| Error::Invalid(snap_core::t("annotate.error.not_a_project").into_owned()))?;

    let json: Value = serde_json::from_slice(&read_entry(&mut zip, "doc.json")?).map_err(bad)?;
    let version = json.get("version").and_then(Value::as_u64).ok_or_else(|| bad("version"))? as u32;
    let json = migrate(json, version)?;
    let document = json.get("document").cloned().ok_or_else(|| bad("document"))?;
    let mut doc: Document = serde_json::from_value(document).map_err(bad)?;

    doc.base = Arc::new(encode::decode(&read_entry(&mut zip, "base.png")?)?);
    for shape in &mut doc.shapes {
        if let ShapeKind::Image { image, .. } = &mut shape.kind {
            *image = Arc::new(encode::decode(&read_entry(&mut zip, &format!("assets/{}.png", shape.id))?)?);
        }
    }
    doc.fix_next_id();
    Ok(doc)
}

/// Upgrades a parsed `doc.json` from `version` to [`VERSION`].
fn migrate(json: Value, version: u32) -> Result<Value> {
    match version {
        VERSION => Ok(json),
        v if v > VERSION => Err(Error::Invalid(tf("annotate.error.newer", &[("version", &v.to_string())]))),
        v => Err(bad(format!("version {v}"))),
    }
}

fn read_entry<R: Read + Seek>(zip: &mut ZipArchive<R>, name: &str) -> Result<Vec<u8>> {
    let entry = zip.by_name(name).map_err(|_| Error::Invalid(tf("annotate.error.missing", &[("name", name)])))?;
    let mut data = Vec::new();
    entry.take(MAX_ENTRY).read_to_end(&mut data)?;
    Ok(data)
}
