//! The MCP tools: schemas and implementations.

use std::{collections::VecDeque, path::Path};

use base64::Engine as _;
use serde_json::{Map, Value, json};
use snap_core::{Capture, CaptureKind, Error, Rect, Result, RgbaImage, t, tf};
use snap_history::Query;

use crate::{CaptureRequest, Engine};

/// Screenshots are shown to the model no larger than this on the long side (what vision models take at full quality).
const MAX_IMAGE_SIDE: u32 = 1568;
/// Captures kept in memory so later calls can refer to them by id.
const KEPT_CAPTURES: usize = 4;
const SNIPPET_CHARS: usize = 240;
const MAX_RESULTS: u32 = 50;
const DEFAULT_RESULTS: u32 = 10;

/// What a tool hands back: content blocks plus optional structured data.
pub(super) struct Output {
    content: Vec<Value>,
    structured: Option<Value>,
}

impl Output {
    pub(super) fn text(text: impl Into<String>) -> Self {
        Self { content: vec![json!({ "type": "text", "text": text.into() })], structured: None }
    }

    /// Text content mirroring structured data, as the protocol recommends for clients without structured support.
    fn data(value: Value) -> Self {
        let text = serde_json::to_string_pretty(&value).unwrap_or_default();
        Self { content: vec![json!({ "type": "text", "text": text })], structured: Some(value) }
    }

    pub(super) fn into_result(self, is_error: bool) -> Value {
        let mut result = json!({ "content": self.content, "isError": is_error });
        if let Some(structured) = self.structured {
            result["structuredContent"] = structured;
        }
        result
    }
}

// --- definitions ----------------------------------------------------------------------------------------------------

const TOOLS: [&str; 9] = [
    "list_monitors",
    "list_windows",
    "capture_screen",
    "capture_window",
    "capture_region",
    "ocr_image",
    "decode_codes",
    "search_history",
    "get_capture",
];

pub(super) fn exists(name: &str) -> bool {
    TOOLS.contains(&name)
}

fn tool(name: &str, title: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": { "type": "object", "properties": properties, "required": required, "additionalProperties": false },
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false },
    })
}

pub(super) fn definitions() -> Vec<Value> {
    let source = json!({
        "path": { "type": "string", "description": "Path of an image file on this computer." },
        "capture_id": { "type": "string", "description": "Id returned by a capture tool in this session, or a history id." },
    });
    vec![
        tool(
            "list_monitors",
            "List monitors",
            "List the connected displays with their index, size and position in physical pixels.",
            json!({}),
            &[],
        ),
        tool(
            "list_windows",
            "List windows",
            "List the visible top-level windows, front to back, with id, title, app and position in physical pixels.",
            json!({}),
            &[],
        ),
        tool(
            "capture_screen",
            "Capture screen",
            "Take a screenshot of one monitor. Returns the image (downscaled to at most 1568 px on the long side) and a capture_id.",
            json!({ "monitor": { "type": "integer", "minimum": 0, "description": "Monitor index from list_monitors. Defaults to the primary monitor." } }),
            &[],
        ),
        tool(
            "capture_window",
            "Capture window",
            "Take a screenshot of one window, even when other windows cover it. Give the window's title (a case-insensitive substring) or its id from list_windows.",
            json!({
                "title": { "type": "string", "description": "Part of the window title or app name." },
                "id": { "type": "integer", "minimum": 0, "description": "Window id from list_windows." },
            }),
            &[],
        ),
        tool(
            "capture_region",
            "Capture region",
            "Take a screenshot of a rectangle of the desktop, in physical pixels with the origin at the top-left of the primary monitor.",
            json!({
                "x": { "type": "integer", "description": "Left edge." },
                "y": { "type": "integer", "description": "Top edge." },
                "w": { "type": "integer", "minimum": 1, "description": "Width." },
                "h": { "type": "integer", "minimum": 1, "description": "Height." },
            }),
            &["x", "y", "w", "h"],
        ),
        tool(
            "ocr_image",
            "Read text from an image",
            "Recognise the text in an image file or a previous capture.",
            {
                let mut p = source.as_object().cloned().unwrap_or_default();
                p.insert(
                    "format".into(),
                    json!({
                        "type": "string",
                        "enum": ["text", "markdown_table", "tsv", "lines"],
                        "description": "text (default), a table as markdown_table or tsv, or lines with bounding boxes in image pixels.",
                    }),
                );
                Value::Object(p)
            },
            &[],
        ),
        tool(
            "decode_codes",
            "Decode QR codes and barcodes",
            "Find and decode QR codes and barcodes in an image file or a previous capture.",
            source,
            &[],
        ),
        tool(
            "search_history",
            "Search capture history",
            "Search the user's earlier screenshots by the text on them, the app, the window title, the URL or tags. An empty query lists the latest.",
            json!({
                "query": { "type": "string", "description": "Words to look for; each matches as a prefix." },
                "limit": { "type": "integer", "minimum": 1, "maximum": MAX_RESULTS, "description": "Most results to return (default 10)." },
            }),
            &[],
        ),
        tool(
            "get_capture",
            "Get a capture",
            "Return a capture from this session or from the history, as an image with its details.",
            json!({ "id": { "type": "string", "description": "A capture_id or a history id." } }),
            &["id"],
        ),
    ]
}

// --- implementation -------------------------------------------------------------------------------------------------

pub(super) struct Tools {
    engine: Engine,
    /// Most recent first.
    captures: VecDeque<Capture>,
}

impl Tools {
    pub(super) fn new(engine: Engine) -> Self {
        Self { engine, captures: VecDeque::new() }
    }

    pub(super) fn call(&mut self, name: &str, args: &Value) -> Result<Output> {
        match name {
            "list_monitors" => list_monitors(),
            "list_windows" => list_windows(),
            "capture_screen" => {
                let monitor = match args.get("monitor").and_then(Value::as_u64) {
                    Some(m) => m as usize,
                    None => snap_capture::monitors()?.iter().find(|m| m.is_primary).map_or(0, |m| m.index),
                };
                self.capture(&CaptureRequest::Monitor(monitor))
            }
            "capture_window" => {
                let request = match (args.get("id").and_then(Value::as_u64), args.get("title").and_then(Value::as_str)) {
                    (Some(id), _) => CaptureRequest::Window(Some(id)),
                    (None, Some(title)) if !title.trim().is_empty() => CaptureRequest::WindowTitled(title.to_string()),
                    _ => return Err(Error::invalid(t("mcp.error.window_needs_target"))),
                };
                self.capture(&request)
            }
            "capture_region" => {
                let get = |key: &str| {
                    args.get(key).and_then(Value::as_i64).ok_or_else(|| Error::invalid(tf("mcp.error.missing_argument", &[("name", key)])))
                };
                let (x, y, w, h) = (get("x")?, get("y")?, get("w")?, get("h")?);
                let rect = Rect::parse(&format!("{x},{y},{w},{h}")).ok_or_else(|| Error::invalid(t("mcp.error.empty_region")))?;
                self.capture(&CaptureRequest::Rect(rect))
            }
            "ocr_image" => self.ocr_image(args),
            "decode_codes" => self.decode_codes(args),
            "search_history" => self.search_history(args),
            "get_capture" => self.get_capture(args),
            other => Err(Error::NotFound(other.to_string())),
        }
    }

    fn capture(&mut self, request: &CaptureRequest) -> Result<Output> {
        let cap = self.engine.capture(request)?;
        let (shown, scale) = shrink(&cap.image);
        let details = json!({
            "capture_id": cap.id,
            "kind": kind_name(cap.kind),
            "width": cap.image.width(),
            "height": cap.image.height(),
            "image_width": shown.width(),
            "image_height": shown.height(),
            "scale": scale,
            "screen_rect": rect_json(cap.screen_rect),
            "app": cap.context.app_name,
            "window_title": cap.context.window_title,
            "url": cap.context.url,
        });
        let mut output = Output::data(details);
        output.content.insert(0, image_block(&shown)?);
        self.captures.push_front(cap);
        self.captures.truncate(KEPT_CAPTURES);
        Ok(output)
    }

    /// The image named by `path` or `capture_id`.
    fn source_image(&self, args: &Value) -> Result<RgbaImage> {
        if let Some(path) = args.get("path").and_then(Value::as_str) {
            return snap_image::encode::load(Path::new(path));
        }
        let id = args.get("capture_id").and_then(Value::as_str).ok_or_else(|| Error::invalid(t("mcp.error.needs_source")))?;
        self.find_image(id)
    }

    /// A capture of this session, else a history entry's file.
    fn find_image(&self, id: &str) -> Result<RgbaImage> {
        if let Some(cap) = self.captures.iter().find(|c| c.id.0 == id) {
            return Ok(cap.image.clone());
        }
        let entry = self.history_entry(id)?;
        let file =
            entry.file.filter(|f| f.exists()).or(entry.thumb).ok_or_else(|| Error::NotFound(tf("mcp.error.no_file", &[("id", id)])))?;
        snap_image::encode::load(&file)
    }

    fn history_entry(&self, id: &str) -> Result<snap_history::Entry> {
        let history = self.engine.history().ok_or_else(|| Error::NotFound(t("mcp.error.no_history").into_owned()))?;
        history.get(&snap_core::CaptureId(id.to_string()))?.ok_or_else(|| Error::NotFound(tf("mcp.error.unknown_capture", &[("id", id)])))
    }

    fn ocr_image(&self, args: &Value) -> Result<Output> {
        let result = self.engine.ocr(&self.source_image(args)?)?;
        let text = match args.get("format").and_then(Value::as_str).unwrap_or("text") {
            "markdown_table" => result.to_markdown_table().ok_or_else(|| Error::NotFound(t("mcp.error.no_table").into_owned()))?,
            "tsv" => result.to_tsv().ok_or_else(|| Error::NotFound(t("mcp.error.no_table").into_owned()))?,
            "lines" => {
                let lines: Vec<Value> = result
                    .lines
                    .iter()
                    .map(|l| json!({ "text": l.text, "x": l.rect.x, "y": l.rect.y, "w": l.rect.w, "h": l.rect.h }))
                    .collect();
                return Ok(Output::data(json!({ "engine": result.engine, "lines": lines })));
            }
            _ => result.text(),
        };
        Ok(Output::text(if text.trim().is_empty() { t("mcp.ocr.no_text").into_owned() } else { text }))
    }

    fn decode_codes(&self, args: &Value) -> Result<Output> {
        let codes = self.engine.decode_codes(&self.source_image(args)?);
        let list: Vec<Value> = codes
            .iter()
            .map(|c| json!({ "format": c.format, "text": c.text, "rect": c.rect.map(|r| json!({ "x": r.x, "y": r.y, "w": r.w, "h": r.h })) }))
            .collect();
        Ok(Output::data(json!({ "codes": list })))
    }

    fn search_history(&self, args: &Value) -> Result<Output> {
        let history = self.engine.history().ok_or_else(|| Error::NotFound(t("mcp.error.no_history").into_owned()))?;
        let limit = args.get("limit").and_then(Value::as_u64).map_or(DEFAULT_RESULTS, |l| l.clamp(1, u64::from(MAX_RESULTS)) as u32);
        let text = args.get("query").and_then(Value::as_str).unwrap_or_default().to_string();
        let results: Vec<Value> = history.search(&Query { text, limit, ..Query::default() })?.iter().map(entry_json).collect();
        Ok(Output::data(json!({ "results": results })))
    }

    fn get_capture(&self, args: &Value) -> Result<Output> {
        let id =
            args.get("id").and_then(Value::as_str).ok_or_else(|| Error::invalid(tf("mcp.error.missing_argument", &[("name", "id")])))?;
        let session = self.captures.iter().find(|c| c.id.0 == id);
        let (image, details) = match session {
            Some(cap) => (
                cap.image.clone(),
                json!({ "capture_id": cap.id, "kind": kind_name(cap.kind), "app": cap.context.app_name, "window_title": cap.context.window_title, "url": cap.context.url }),
            ),
            None => (self.find_image(id)?, entry_json(&self.history_entry(id)?)),
        };
        let (shown, scale) = shrink(&image);
        let mut details = details;
        details["width"] = json!(image.width());
        details["height"] = json!(image.height());
        details["scale"] = json!(scale);
        let mut output = Output::data(details);
        output.content.insert(0, image_block(&shown)?);
        Ok(output)
    }
}

fn list_monitors() -> Result<Output> {
    let monitors: Vec<Value> = snap_capture::monitors()?
        .iter()
        .map(|m| {
            json!({
                "index": m.index,
                "name": m.name,
                "x": m.rect.x, "y": m.rect.y, "width": m.rect.w, "height": m.rect.h,
                "scale_factor": m.scale_factor,
                "primary": m.is_primary,
                "hdr": m.hdr,
                "refresh_hz": m.refresh_hz,
            })
        })
        .collect();
    Ok(Output::data(json!({ "monitors": monitors })))
}

fn list_windows() -> Result<Output> {
    let windows: Vec<Value> = snap_capture::windows()?
        .iter()
        .map(|w| {
            json!({
                "id": w.id,
                "title": w.title,
                "app": w.app_name,
                "pid": w.pid,
                "x": w.rect.x, "y": w.rect.y, "width": w.rect.w, "height": w.rect.h,
                "z": w.z,
                "minimized": w.minimized,
            })
        })
        .collect();
    Ok(Output::data(json!({ "windows": windows })))
}

// --- helpers --------------------------------------------------------------------------------------------------------

fn rect_json(r: Rect) -> Value {
    json!({ "x": r.x, "y": r.y, "w": r.w, "h": r.h })
}

fn kind_name(kind: CaptureKind) -> Value {
    serde_json::to_value(kind).unwrap_or(Value::Null)
}

/// The image as the model will see it, and the factor from capture pixels to shown pixels.
fn shrink(img: &RgbaImage) -> (RgbaImage, f32) {
    let longest = img.width().max(img.height());
    if longest <= MAX_IMAGE_SIDE {
        return (img.clone(), 1.0);
    }
    let shown = snap_image::resize::fit(img, Some(MAX_IMAGE_SIDE), Some(MAX_IMAGE_SIDE), true);
    let scale = shown.width() as f32 / img.width() as f32;
    (shown, scale)
}

fn image_block(img: &RgbaImage) -> Result<Value> {
    let png = snap_image::encode::encode(img, snap_core::settings::ImageFormat::Png, 100, false)?;
    Ok(json!({ "type": "image", "data": base64::engine::general_purpose::STANDARD.encode(png), "mimeType": "image/png" }))
}

fn entry_json(e: &snap_history::Entry) -> Value {
    let snippet: Option<String> = e.text.as_ref().map(|t| t.chars().take(SNIPPET_CHARS).collect());
    let mut object = Map::new();
    object.insert("id".into(), json!(e.id));
    object.insert("kind".into(), kind_name(e.kind));
    object.insert("created".into(), json!(chrono::DateTime::from_timestamp_millis(e.created_ms).map(|d| d.to_rfc3339())));
    object.insert("app".into(), json!(e.context.app_name));
    object.insert("window_title".into(), json!(e.context.window_title));
    object.insert("url".into(), json!(e.context.url));
    object.insert("file".into(), json!(e.file));
    object.insert("width".into(), json!(e.width));
    object.insert("height".into(), json!(e.height));
    object.insert("text".into(), json!(snippet));
    object.insert("links".into(), json!(e.links.iter().map(|l| &l.url).collect::<Vec<_>>()));
    object.insert("tags".into(), json!(e.tags));
    Value::Object(object)
}
