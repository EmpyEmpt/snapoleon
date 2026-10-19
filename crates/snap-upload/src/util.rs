//! Small string helpers shared by the destinations.

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~');

/// Percent-encode a single URL component (RFC 3986 unreserved set stays).
pub(crate) fn pct(s: &str) -> String {
    utf8_percent_encode(s, UNRESERVED).to_string()
}

/// Percent-encode every segment of a `/`-separated path, keeping the slashes.
pub(crate) fn pct_path(s: &str) -> String {
    s.split('/').map(pct).collect::<Vec<_>>().join("/")
}

/// `base` + `/` + `rel` with exactly one slash between them.
pub(crate) fn join_url(base: &str, rel: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), rel.trim_start_matches('/'))
}

/// Make a user-supplied file name safe for use as one path component, or as a
/// relative path when it contains `/` (empty and `..` segments are dropped).
pub(crate) fn safe_relative_path(name: &str) -> String {
    let parts: Vec<String> = name
        .split(['/', '\\'])
        .map(|seg| seg.chars().filter(|c| !c.is_control() && !matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|')).collect::<String>())
        .map(|seg| seg.trim().trim_end_matches('.').to_string())
        .filter(|seg| !seg.is_empty() && seg != "..")
        .collect();
    if parts.is_empty() { "file".into() } else { parts.join("/") }
}

/// Last path component.
pub(crate) fn base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// `(stem, ".ext")` of a file name; the extension is empty when absent.
pub(crate) fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() && !name[i..].contains('/') => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

pub(crate) fn mime_or_octet(mime: &str) -> &str {
    if mime.trim().is_empty() { "application/octet-stream" } else { mime }
}

pub(crate) fn is_image(mime: &str) -> bool {
    mime.starts_with("image/")
}

pub(crate) fn is_video(mime: &str) -> bool {
    mime.starts_with("video/")
}

pub(crate) fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{bytes} B") } else { format!("{v:.0} {}", UNITS[u]) }
}

/// Escape text for HTML element content and double-quoted attributes.
pub(crate) fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}
