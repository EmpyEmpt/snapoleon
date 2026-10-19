//! Small helpers: ids, codes, hashing, MIME sniffing and formatting.

use std::{fmt::Write as _, net::IpAddr};

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

/// Whether `ip` may be contacted on behalf of a user-supplied URL: public unicast
/// addresses only (no loopback, private, link-local, CGNAT, metadata, ULA or mapped forms).
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_unspecified()
                || v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                || o[0] == 0
                || o[0] >= 240
                || (o[0] == 100 && (64..128).contains(&o[1]))
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
                || (o[0] == 198 && (o[1] == 18 || o[1] == 19)))
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            // 64:ff9b::/96 (NAT64) embeds an IPv4 address in the last 32 bits.
            if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
                let [a, b] = [s[6].to_be_bytes(), s[7].to_be_bytes()];
                return is_public_ip(IpAddr::V4(std::net::Ipv4Addr::new(a[0], a[1], b[0], b[1])));
            }
            !(v6.is_unspecified()
                || v6.is_loopback()
                || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00
                || (s[0] & 0xffc0) == 0xfe80
                || (s[0] == 0x2001 && s[1] == 0x0db8))
        }
    }
}

/// An IP network such as `10.0.0.0/8`; a bare address means a single host.
#[derive(Debug, Clone, Copy)]
pub struct Cidr {
    net: IpAddr,
    prefix: u8,
}

impl Cidr {
    pub fn contains(&self, ip: IpAddr) -> bool {
        fn bits(ip: IpAddr) -> (u128, u8) {
            match ip.to_canonical() {
                IpAddr::V4(v) => (u128::from(u32::from(v)), 32),
                IpAddr::V6(v) => (u128::from(v), 128),
            }
        }
        let ((net, width), (ip, ip_width)) = (bits(self.net), bits(ip));
        if width != ip_width {
            return false;
        }
        let shift = u32::from(width - self.prefix);
        shift >= 128 || net >> shift == ip >> shift
    }
}

impl std::str::FromStr for Cidr {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (addr, prefix) = s.trim().split_once('/').map_or((s.trim(), None), |(a, p)| (a, Some(p)));
        let net: IpAddr = addr.parse().map_err(|_| format!("invalid network: {s}"))?;
        let max = if net.to_canonical().is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(p) => p.parse::<u8>().ok().filter(|p| *p <= max).ok_or_else(|| format!("invalid network: {s}"))?,
            None => max,
        };
        Ok(Self { net, prefix })
    }
}

impl<'de> serde::Deserialize<'de> for Cidr {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?.parse().map_err(serde::de::Error::custom)
    }
}
