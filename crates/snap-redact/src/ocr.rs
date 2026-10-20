//! OCR tolerance: letters that OCR engines commonly confuse with digits are
//! folded to digits, but only inside whitespace-delimited tokens that are
//! mostly digits, so ordinary words are never touched.

/// Returns `text` with OCR-confusable letters folded to digits in digit-dominated tokens.
///
/// Every substitution maps one ASCII character to one ASCII character, so byte
/// offsets in the result are identical to offsets in `text`.
pub fn fold_digits(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for piece in text.split_inclusive(char::is_whitespace) {
        let body = piece.trim_end_matches(char::is_whitespace);
        let digits = body.bytes().filter(u8::is_ascii_digit).count();
        let dominated = digits > 0 && digits * 2 >= body.chars().count();
        if dominated {
            out.extend(body.chars().map(fold));
        } else {
            out.push_str(body);
        }
        out.push_str(&piece[body.len()..]);
    }
    out
}

fn fold(c: char) -> char {
    match c {
        'O' | 'o' => '0',
        'l' | 'I' | '|' | '!' => '1',
        'S' => '5',
        'B' => '8',
        other => other,
    }
}
