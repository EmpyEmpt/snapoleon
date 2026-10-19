//! Small helpers: ids, codes, hashing, MIME sniffing and formatting.

use std::fmt::Write as _;

use hmac::{Hmac, KeyInit, Mac};
use rand::Rng;
use sha2::{Digest, Sha256};

/// Short-code alphabet without look-alike characters (no 0/o, 1/l/i).
const CODE_ALPHABET: &[u8] = b"23456789abcdefghjkmnpqrstuvwxyz";
const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

pub const HOUR_MS: i64 = 3_600_000;
pub const DAY_MS: i64 = 24 * HOUR_MS;

fn random_from(alphabet: &[u8], len: usize) -> String {
    let mut rng = rand::rng();
    (0..len).map(|_| alphabet[rng.random_range(0..alphabet.len())] as char).collect()
}

/// A short, human-typeable code for items and links.
pub fn new_code(len: usize) -> String {
    random_from(CODE_ALPHABET, len)
}

/// Opaque identifier for database rows.
pub fn new_id() -> String {
    let mut bytes = [0u8; 8];
    rand::rng().fill(&mut bytes);
    hex(&bytes)
}

/// Long random secret (tokens, session ids, delete secrets).
pub fn new_secret(len: usize) -> String {
    random_from(BASE62, len)
}

pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

pub fn hmac_hex(key: &[u8], msg: &[u8]) -> String {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(msg);
    hex(&mac.finalize().into_bytes())
}

/// Constant-time string comparison.
pub fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = (a.len() ^ b.len()) as u8;
    for i in 0..a.len().max(b.len()) {
        diff |= a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0);
    }
    diff == 0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    Video,
}

#[derive(Debug, Clone, Copy)]
pub struct Sniffed {
    pub mime: &'static str,
    pub kind: MediaKind,
}

/// Identify an upload from its leading bytes. The client-supplied content type
/// is never trusted, and only formats that are safe to serve inline are allowed.
pub fn sniff(head: &[u8]) -> Option<Sniffed> {
    let img = |mime| Some(Sniffed { mime, kind: MediaKind::Image });
    let vid = |mime| Some(Sniffed { mime, kind: MediaKind::Video });
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        img("image/png")
    } else if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        img("image/jpeg")
    } else if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        img("image/gif")
    } else if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        img("image/webp")
    } else if head.starts_with(b"BM") && head.len() > 14 {
        img("image/bmp")
    } else if head.len() >= 12 && &head[4..8] == b"ftyp" {
        vid("video/mp4")
    } else if head.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        vid("video/webm")
    } else {
        None
    }
}

pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{n} B") } else { format!("{v:.1} {}", UNITS[u]) }
}

/// `2026-10-19 14:05` in UTC.
pub fn format_ms(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).map(|d| d.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default()
}

/// RFC 3339 timestamp for `<time datetime>` and meta tags.
pub fn iso_ms(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)).unwrap_or_default()
}

/// Strip path components and control characters from a client file name.
pub fn clean_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = base.chars().filter(|c| !c.is_control() && !matches!(c, '"' | '<' | '>' | ':' | '|' | '?' | '*')).collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    cleaned.chars().take(120).collect()
}

/// Truncate to `max` characters without splitting a code point.
pub fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Percent-encode everything except unreserved characters.
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// File extension for a served MIME type.
pub fn sniff_ext(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        _ => "bin",
    }
}
