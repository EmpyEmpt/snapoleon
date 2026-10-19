//! File-name / key templates.
//!
//! Tokens: `{yyyy} {yy} {MM} {dd} {HH} {mm} {ss} {ms}` date parts,
//! `{unix}` seconds, `{rand}` 8 random base62 chars, `{rand:N}`,
//! `{app}` `{title}` `{kind}` from the capture, `{n}` per-session counter,
//! `{w}` `{h}` image size. Invalid path characters are replaced with `_`.

use chrono::{DateTime, Datelike, Local, Timelike};
use rand::Rng;

use crate::capture::{Capture, CaptureKind};

pub struct NameContext<'a> {
    pub when: DateTime<Local>,
    pub app: Option<&'a str>,
    pub title: Option<&'a str>,
    pub kind: Option<CaptureKind>,
    pub counter: u32,
    pub size: Option<(u32, u32)>,
}

impl<'a> NameContext<'a> {
    pub fn now() -> Self {
        Self { when: Local::now(), app: None, title: None, kind: None, counter: 0, size: None }
    }

    pub fn for_capture(c: &'a Capture, counter: u32) -> Self {
        let when = DateTime::from_timestamp_millis(c.taken_at_ms).map(|d| d.with_timezone(&Local)).unwrap_or_else(Local::now);
        Self {
            when,
            app: c.context.app_name.as_deref(),
            title: c.context.window_title.as_deref(),
            kind: Some(c.kind),
            counter,
            size: Some((c.image.width(), c.image.height())),
        }
    }
}

pub fn random_code(len: usize) -> String {
    const A: &[u8] = b"abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::rng();
    (0..len).map(|_| A[rng.random_range(0..A.len())] as char).collect()
}

/// Expand a template. Result is a single path component (no separators)
/// unless the template itself contains `/`.
pub fn expand(template: &str, cx: &NameContext) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('}') else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let token = &rest[start + 1..start + end];
        out.push_str(&sanitize(&expand_token(token, cx).unwrap_or_else(|| format!("{{{token}}}"))));
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out.trim().to_string()
}

fn expand_token(token: &str, cx: &NameContext) -> Option<String> {
    let w = &cx.when;
    Some(match token {
        "yyyy" => format!("{:04}", w.year()),
        "yy" => format!("{:02}", w.year() % 100),
        "MM" => format!("{:02}", w.month()),
        "dd" => format!("{:02}", w.day()),
        "HH" => format!("{:02}", w.hour()),
        "mm" => format!("{:02}", w.minute()),
        "ss" => format!("{:02}", w.second()),
        "ms" => format!("{:03}", w.timestamp_subsec_millis()),
        "unix" => w.timestamp().to_string(),
        "rand" => random_code(8),
        "app" => cx.app.unwrap_or("Screen").to_string(),
        "title" => truncate(cx.title.unwrap_or(""), 60),
        "kind" => cx.kind.map(|k| format!("{k:?}").to_lowercase()).unwrap_or_default(),
        "n" => cx.counter.to_string(),
        "w" => cx.size.map(|s| s.0.to_string()).unwrap_or_default(),
        "h" => cx.size.map(|s| s.1.to_string()).unwrap_or_default(),
        t if t.starts_with("rand:") => random_code(t[5..].parse::<usize>().ok()?.clamp(1, 64)),
        _ => return None,
    })
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() { '_' } else { c }).collect()
}
