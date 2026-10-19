//! PII and secret detection for auto-redaction.
//!
//! [`find`] scans plain text. [`locate`] maps matches back onto OCR word boxes
//! so the caller can cover them. Wherever a format has a checksum (cards, IBAN,
//! ИНН, СНИЛС, SSN) the checksum is enforced, which keeps false positives low.

mod detect;
mod locate;
mod ocr;
mod validate;

use std::cmp::Reverse;

use snap_core::settings::RedactKind;

use detect::Span;

pub use locate::{Hit, LineWords, locate, locate_substrings};

/// One detected span. `start` and `end` are byte offsets into the scanned text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub kind: RedactKind,
    pub start: usize,
    pub end: usize,
    pub text: String,
}

/// Finds every span of the requested kinds in `text`.
///
/// Overlaps are resolved longest-first, so a URL that carries a token wins over
/// the token alone. The result is sorted by `start` and never overlaps.
pub fn find(text: &str, kinds: &[RedactKind]) -> Vec<Match> {
    if text.is_empty() || kinds.is_empty() {
        return Vec::new();
    }
    let norm = ocr::fold_digits(text);
    let mut cands: Vec<(RedactKind, Span)> = kinds
        .iter()
        .flat_map(|&k| {
            detect::detect(k, text, &norm)
                .into_iter()
                .map(move |s| (k, s))
        })
        .collect();
    cands.sort_by_key(|&(k, (s, e))| (Reverse(e - s), rank(k), s));

    let mut taken = vec![false; text.len()];
    let mut chosen = Vec::new();
    for (kind, (start, end)) in cands {
        if taken[start..end].iter().any(|&t| t) {
            continue;
        }
        taken[start..end].fill(true);
        chosen.push(Match {
            kind,
            start,
            end,
            text: text[start..end].to_owned(),
        });
    }
    chosen.sort_by_key(|m| m.start);
    chosen
}

/// Localized display name for a kind, e.g. "Payment card".
pub fn kind_label(kind: RedactKind) -> String {
    let key = match kind {
        RedactKind::Email => "redact.kind.email",
        RedactKind::Phone => "redact.kind.phone",
        RedactKind::CreditCard => "redact.kind.credit_card",
        RedactKind::Iban => "redact.kind.iban",
        RedactKind::RuInn => "redact.kind.ru_inn",
        RedactKind::RuSnils => "redact.kind.ru_snils",
        RedactKind::RuPassport => "redact.kind.ru_passport",
        RedactKind::UsSsn => "redact.kind.us_ssn",
        RedactKind::IpAddress => "redact.kind.ip_address",
        RedactKind::Secret => "redact.kind.secret",
        RedactKind::Url => "redact.kind.url",
    };
    snap_core::t(key).into_owned()
}

/// Tie-break for candidates of equal length: lower rank wins. More specific
/// kinds (secrets, links) rank above generic number shapes.
fn rank(kind: RedactKind) -> u8 {
    match kind {
        RedactKind::Secret => 0,
        RedactKind::Url => 1,
        RedactKind::Email => 2,
        RedactKind::Iban => 3,
        RedactKind::CreditCard => 4,
        RedactKind::Phone => 5,
        RedactKind::RuSnils => 6,
        RedactKind::RuInn => 7,
        RedactKind::RuPassport => 8,
        RedactKind::UsSsn => 9,
        RedactKind::IpAddress => 10,
    }
}
