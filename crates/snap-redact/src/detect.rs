//! Candidate generators. Each returns byte spans (`start`, `end`) into the
//! text it is given. Spans may overlap; `lib.rs` resolves overlaps.
//!
//! Digit-based kinds receive the OCR-folded text; everything else receives the
//! original. Folding preserves byte offsets, so spans are valid for both.

use std::net::Ipv6Addr;
use std::sync::LazyLock;

use regex::Regex;
use snap_core::settings::RedactKind;

use crate::validate::{
    entropy, iban_valid, inn_valid, known_card_iin, luhn, snils_valid, ssn_valid,
};

pub type Span = (usize, usize);

macro_rules! regex {
    ($name:ident, $pat:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pat).expect("static regex"));
    };
}

regex!(
    EMAIL,
    r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9](?:[A-Za-z0-9\-]*[A-Za-z0-9])?(?:\.[A-Za-z0-9](?:[A-Za-z0-9\-]*[A-Za-z0-9])?)*\.[A-Za-z]{2,}"
);
regex!(
    URL,
    r#"(?:\b(?:https?|ftp|wss?)://|\bwww\.)[^\s<>"'\x60]*[^\s<>"'\x60.,;:!?)\]}]"#
);
regex!(
    PROVIDER_TOKEN,
    r"(?x)
      \b(?:AKIA|ASIA)[0-9A-Z]{16}\b
    | \bgh[pousr]_[0-9A-Za-z]{36,255}\b
    | \bglpat-[0-9A-Za-z_\-]{20,}
    | \bxox[abpr]-[0-9A-Za-z\-]{10,}
    | \b(?:sk|rk)_(?:live|test)_[0-9A-Za-z]{16,}
    | \bsk-[0-9A-Za-z_\-]{20,}
    | \bAIza[0-9A-Za-z_\-]{35}\b
    | \beyJ[0-9A-Za-z_\-]{5,}\.[0-9A-Za-z_\-]{5,}(?:\.[0-9A-Za-z_\-]*)?
    | -----BEGIN [0-9A-Z ]*PRIVATE KEY(?: BLOCK)?-----[\s\S]*?(?:-----END [0-9A-Z ]*PRIVATE KEY(?: BLOCK)?-----|\z)
    "
);
regex!(
    KEY_VALUE,
    r#"(?i)\b[a-z0-9_\-]*?(?:password|passwd|pwd|secret|token|api[_\-]?key)[a-z0-9_\-]*\s*[:=]\s*(?:"([^"\n]+)"|'([^'\n]+)'|([^\s"',;]+))"#
);
regex!(BEARER, r"(?i)\bbearer\s+([A-Za-z0-9._~+/=\-]{16,})");
regex!(GENERIC_TOKEN, r"[A-Za-z0-9+/=_\-.]{24,}");
regex!(IBAN_TOKEN, r"\b[A-Z]{2}[0-9]{2}(?: ?[A-Z0-9]{1,4})+");
regex!(SSN, r"[0-9]{3}-[0-9]{2}-[0-9]{4}");
regex!(
    SNILS_FORMATTED,
    r"^[0-9]{3}[- ][0-9]{3}[- ][0-9]{3}[ -][0-9]{2}$"
);
regex!(
    PHONE_RU,
    r"(?:\+7|8)[ \-]?\(?[0-9]{3}\)?[ \-]?[0-9]{3}[ \-]?[0-9]{2}[ \-]?[0-9]{2}"
);
regex!(
    PHONE_US,
    r"(?:\+?1[ \-.]?)?(?:\([2-9][0-9]{2}\)[ \-.]?|[2-9][0-9]{2}[ \-.])[0-9]{3}[ \-.][0-9]{4}"
);
regex!(PHONE_E164, r"\+[1-9](?:[ \-.()]?[0-9]){6,14}");
regex!(
    IPV4,
    r"(?:(?:25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9]?[0-9])\.){3}(?:25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9]?[0-9])"
);
regex!(
    IPV6,
    r"[0-9A-Fa-f]{0,4}(?::[0-9A-Fa-f]{0,4}){2,7}(?::(?:[0-9]{1,3}\.){3}[0-9]{1,3})?"
);

const PASSPORT_CONTEXT: &[&str] = &["паспорт", "серия", "passport"];
const SNILS_CONTEXT: &[&str] = &["снилс", "snils", "страхов"];
const SECRET_PARAM_SUFFIXES: &[&str] = &[
    "token",
    "key",
    "secret",
    "password",
    "passwd",
    "pwd",
    "pass",
    "signature",
    "sig",
    "auth",
    "session",
    "sessionid",
];

/// Spans of `kind` in `text` (original) and `norm` (OCR-folded).
pub fn detect(kind: RedactKind, text: &str, norm: &str) -> Vec<Span> {
    let mut spans = match kind {
        RedactKind::Email => scan(&EMAIL, text, |_, _, _| true),
        RedactKind::Url => scan(&URL, text, |_, _, _| true),
        RedactKind::Secret => secrets(text),
        RedactKind::Iban => {
            let mut v = iban(text);
            v.extend(iban(norm));
            v
        }
        RedactKind::CreditCard => digit_spans(norm, 13, 19, |d| luhn(d) && known_card_iin(d)),
        RedactKind::RuInn => digit_spans(norm, 10, 12, inn_valid),
        RedactKind::RuSnils => snils(norm, text),
        RedactKind::RuPassport => digit_spans(norm, 10, 10, |_| true)
            .into_iter()
            .filter(|&(s, e)| near_any(text, s, e, PASSPORT_CONTEXT))
            .collect(),
        RedactKind::UsSsn => scan(&SSN, norm, |t, s, e| {
            let p = &t[s..e];
            digit_bounded(t, s, e) && ssn_valid(&p[..3], &p[4..6], &p[7..])
        }),
        RedactKind::Phone => phones(norm),
        RedactKind::IpAddress => {
            let mut v = scan(&IPV4, text, ipv4_bounded);
            v.extend(scan(&IPV6, text, ipv6_bounded));
            v
        }
    };
    if is_digit_kind(kind) {
        // Digits inside an IBAN-shaped token belong to that token, even when it
        // fails mod-97; otherwise its tail reads as a card or a phone number.
        let iban_like = scan(&IBAN_TOKEN, text, |_, _, _| true);
        spans.retain(|&(s, e)| !iban_like.iter().any(|&(a, b)| a <= s && e <= b));
    }
    spans
}

fn is_digit_kind(kind: RedactKind) -> bool {
    matches!(
        kind,
        RedactKind::CreditCard
            | RedactKind::RuInn
            | RedactKind::RuSnils
            | RedactKind::RuPassport
            | RedactKind::UsSsn
            | RedactKind::Phone
    )
}

/// Leftmost matches of `re` that `accept` approves. Rejected matches are
/// skipped and the search resumes one character later.
fn scan(re: &Regex, text: &str, accept: impl Fn(&str, usize, usize) -> bool) -> Vec<Span> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos <= text.len() {
        let Some(m) = re.find_at(text, pos) else {
            break;
        };
        let step = text[m.start()..].chars().next().map_or(1, char::len_utf8);
        if accept(text, m.start(), m.end()) {
            out.push((m.start(), m.end()));
            pos = if m.end() > m.start() {
                m.end()
            } else {
                m.start() + step
            };
        } else {
            pos = m.start() + step;
        }
    }
    out
}

fn prev_char(text: &str, s: usize) -> Option<char> {
    text[..s].chars().next_back()
}

fn next_char(text: &str, e: usize) -> Option<char> {
    text[e..].chars().next()
}

fn digit_bounded(text: &str, s: usize, e: usize) -> bool {
    !prev_char(text, s).is_some_and(|c| c.is_ascii_digit())
        && !next_char(text, e).is_some_and(|c| c.is_ascii_digit())
}

fn ipv4_bounded(text: &str, s: usize, e: usize) -> bool {
    let before_ok = match prev_char(text, s) {
        Some(c) if c.is_ascii_digit() => false,
        Some('.') => !text[..s - 1]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_digit()),
        _ => true,
    };
    let after_ok = match next_char(text, e) {
        Some(c) if c.is_ascii_digit() => false,
        Some('.') => !text[e + 1..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit()),
        _ => true,
    };
    before_ok && after_ok
}

fn ipv6_bounded(text: &str, s: usize, e: usize) -> bool {
    let word = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric() || c == ':');
    !word(prev_char(text, s)) && !word(next_char(text, e)) && text[s..e].parse::<Ipv6Addr>().is_ok()
}

/// Maximal runs of digits joined by single spaces or hyphens. For each start
/// group the longest run whose digit string satisfies `ok` and has a length in
/// `min..=max` is reported. Groups are never split, so an 11-digit number
/// inside a 14-digit run is not reported.
fn digit_spans(text: &str, min: usize, max: usize, ok: impl Fn(&[u8]) -> bool) -> Vec<Span> {
    let b = text.as_bytes();
    let mut groups: Vec<Span> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            let s = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            groups.push((s, i));
        } else {
            i += 1;
        }
    }

    let mut out = Vec::new();
    let mut g0 = 0;
    while g0 < groups.len() {
        let mut g1 = g0;
        while g1 + 1 < groups.len()
            && groups[g1 + 1].0 == groups[g1].1 + 1
            && matches!(b[groups[g1].1], b' ' | b'-')
        {
            g1 += 1;
        }
        let chain = &groups[g0..=g1];
        for a in 0..chain.len() {
            let mut digits = Vec::new();
            let mut best = None;
            for (k, &(s, e)) in chain[a..].iter().enumerate() {
                digits.extend_from_slice(&b[s..e]);
                if digits.len() > max {
                    break;
                }
                if digits.len() >= min && ok(&digits) {
                    best = Some(a + k);
                }
            }
            if let Some(last) = best {
                out.push((chain[a].0, chain[last].1));
            }
        }
        g0 = g1 + 1;
    }
    out
}

/// Whether one of `words` (lowercase) occurs within 40 characters of the span.
fn near_any(text: &str, s: usize, e: usize, words: &[&str]) -> bool {
    let before: Vec<char> = text[..s].chars().rev().take(40).collect();
    let mut window: String = before.into_iter().rev().collect();
    window.extend(text[e..].chars().take(40));
    let window = window.to_lowercase();
    words.iter().any(|w| window.contains(w))
}

/// СНИЛС: formatted `NNN-NNN-NNN NN` always counts; plain 11 digits need a context word.
fn snils(norm: &str, text: &str) -> Vec<Span> {
    digit_spans(norm, 11, 11, snils_valid)
        .into_iter()
        .filter(|&(s, e)| {
            let digits = &norm[s..e];
            SNILS_FORMATTED.is_match(digits)
                || (digits.bytes().all(|c| c.is_ascii_digit())
                    && near_any(text, s, e, SNILS_CONTEXT))
        })
        .collect()
}

/// Russian and international phone formats. Candidates are tried in order of
/// specificity and a later, more generic match never overlaps an earlier one.
fn phones(norm: &str) -> Vec<Span> {
    let candidates = [
        scan(&PHONE_RU, norm, digit_bounded),
        scan(&PHONE_US, norm, digit_bounded),
        scan(&PHONE_E164, norm, |t, s, e| {
            let n = t[s..e].bytes().filter(u8::is_ascii_digit).count();
            digit_bounded(t, s, e) && (8..=15).contains(&n)
        }),
    ];
    let mut out: Vec<Span> = Vec::new();
    for list in candidates {
        for sp in list {
            if out.iter().all(|&o| o.1 <= sp.0 || sp.1 <= o.0) {
                out.push(sp);
            }
        }
    }
    out
}

/// IBAN candidates: a country code and check digits, then space-grouped
/// alphanumerics. Trailing space-separated words are trimmed until mod-97 holds.
fn iban(text: &str) -> Vec<Span> {
    let mut out = Vec::new();
    for m in IBAN_TOKEN.find_iter(text) {
        let mut tokens: Vec<Span> = Vec::new();
        let mut off = m.start();
        for part in text[m.start()..m.end()].split(' ') {
            if !part.is_empty() {
                tokens.push((off, off + part.len()));
            }
            off += part.len() + 1;
        }
        for k in (1..=tokens.len()).rev() {
            let joined: String = tokens[..k].iter().map(|&(s, e)| &text[s..e]).collect();
            if iban_valid(&joined) {
                out.push((tokens[0].0, tokens[k - 1].1));
                break;
            }
        }
    }
    out
}

/// Secret-like values: provider tokens, PEM blocks, key=value pairs, bearer
/// tokens, credential-bearing URLs, and generic high-entropy strings.
fn secrets(text: &str) -> Vec<Span> {
    let mut out = scan(&PROVIDER_TOKEN, text, |_, _, _| true);

    for caps in KEY_VALUE.captures_iter(text) {
        if let Some(v) = (1..=3).find_map(|i| caps.get(i))
            && v.as_str().len() >= 4
            && !masked(v.as_str())
        {
            out.push((v.start(), v.end()));
        }
    }
    for caps in BEARER.captures_iter(text) {
        if let Some(v) = caps.get(1) {
            out.push((v.start(), v.end()));
        }
    }
    for m in GENERIC_TOKEN.find_iter(text) {
        let t = m.as_str();
        let has = |f: fn(&u8) -> bool| t.bytes().any(|c| f(&c));
        if has(u8::is_ascii_lowercase)
            && has(u8::is_ascii_uppercase)
            && has(u8::is_ascii_digit)
            && entropy(t) >= 3.5
        {
            out.push((m.start(), m.end()));
        }
    }
    out.extend(
        scan(&URL, text, |_, _, _| true)
            .into_iter()
            .filter(|&(s, e)| url_has_secret(&text[s..e])),
    );
    out
}

/// Credentials (`user:pass@host`) or token-like query parameters.
fn url_has_secret(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    if rest[..authority_end]
        .rsplit_once('@')
        .is_some_and(|(userinfo, _)| userinfo.contains(':'))
    {
        return true;
    }
    rest.split_once('?').is_some_and(|(_, query)| {
        query.split('&').any(|kv| {
            kv.split_once('=').is_some_and(|(k, _)| {
                let k = k.to_ascii_lowercase();
                SECRET_PARAM_SUFFIXES.iter().any(|s| k.ends_with(s))
            })
        })
    })
}

fn masked(v: &str) -> bool {
    v.chars()
        .all(|c| matches!(c, '*' | '•' | '.' | '-' | '_' | 'x' | 'X'))
}
