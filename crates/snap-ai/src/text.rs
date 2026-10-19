//! Cleaning and parsing of model output.

use serde_json::Value;

/// Removes reasoning blocks and code fences, trims whitespace.
pub fn clean_reply(raw: &str) -> String {
    let mut s = raw;
    if let Some(i) = s.rfind("</think>") {
        s = &s[i + "</think>".len()..];
    }
    strip_fences(s.trim()).trim().to_owned()
}

fn strip_fences(s: &str) -> &str {
    let Some(rest) = s.strip_prefix("```") else { return s };
    let body = rest.split_once('\n').map_or("", |(_, b)| b);
    body.trim_end().strip_suffix("```").unwrap_or(body).trim_end()
}

/// First JSON object or array found in `raw`, tolerating prose and fences around it.
pub fn extract_json(raw: &str) -> Option<Value> {
    let s = clean_reply(raw);
    for (i, c) in s.char_indices() {
        if c != '{' && c != '[' {
            continue;
        }
        if let Some(Ok(v)) = serde_json::Deserializer::from_str(&s[i..]).into_iter::<Value>().next() {
            return Some(v);
        }
    }
    None
}

/// Single-line plain text: no surrounding quotes or markdown emphasis.
pub fn plain_line(s: &str) -> String {
    let joined = s.split_whitespace().collect::<Vec<_>>().join(" ");
    joined.trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | '*' | '«' | '»' | '“' | '”')).trim().to_owned()
}

/// Whitespace-collapsed paragraph(s) with quotes trimmed; keeps line breaks.
pub fn plain_text(s: &str) -> String {
    let lines: Vec<String> = s.lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).collect();
    lines.join("\n").trim_matches(|c: char| matches!(c, '"' | '“' | '”' | '«' | '»') || c.is_whitespace()).to_owned()
}

/// Title usable as a file name: at most 8 words and 80 characters, no reserved characters.
pub fn filename_title(raw: &str) -> String {
    let first = clean_reply(raw);
    let first = first.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let first = first.trim_start_matches(|c: char| c == '#' || c.is_whitespace());
    let first = first.strip_prefix("Title:").or_else(|| first.strip_prefix("title:")).unwrap_or(first);
    let cleaned: String = plain_line(first)
        .chars()
        .map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { ' ' } else { c })
        .collect();
    let words: Vec<&str> = cleaned.split_whitespace().take(8).collect();
    let mut title = words.join(" ");
    if title.chars().count() > 80 {
        title = title.chars().take(80).collect();
    }
    title.trim_matches(|c: char| c == '.' || c == ',' || c == ';' || c == '-' || c.is_whitespace()).to_owned()
}

/// UI/target language code (`ru`, `pt-BR`) as a name for prompts.
pub fn language_name(code: &str) -> String {
    let base = code.split(['-', '_']).next().unwrap_or(code).to_ascii_lowercase();
    let name = match base.as_str() {
        "en" => "English",
        "ru" => "Russian",
        "uk" => "Ukrainian",
        "be" => "Belarusian",
        "de" => "German",
        "fr" => "French",
        "es" => "Spanish",
        "it" => "Italian",
        "pt" => "Portuguese",
        "nl" => "Dutch",
        "pl" => "Polish",
        "cs" => "Czech",
        "sk" => "Slovak",
        "sv" => "Swedish",
        "da" => "Danish",
        "nb" | "no" => "Norwegian",
        "fi" => "Finnish",
        "el" => "Greek",
        "tr" => "Turkish",
        "ar" => "Arabic",
        "he" => "Hebrew",
        "hi" => "Hindi",
        "id" => "Indonesian",
        "vi" => "Vietnamese",
        "th" => "Thai",
        "ja" => "Japanese",
        "ko" => "Korean",
        "zh" => "Chinese",
        "hu" => "Hungarian",
        "ro" => "Romanian",
        "bg" => "Bulgarian",
        _ => return code.to_owned(),
    };
    name.to_owned()
}
