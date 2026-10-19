//! ShareX `.sxcu` import / export, including the legacy `$json:...$` syntax.

use regex::Regex;
use serde_json::Value;
use snap_core::{Error, Result, settings::CustomUploader, t};

/// Parse a `.sxcu` document into a display name and an uploader definition.
pub fn import(json: &str) -> Result<(String, CustomUploader)> {
    let doc: Value = serde_json::from_str(json.trim_start_matches('\u{feff}'))
        .map_err(|e| Error::invalid(format!("{}: {e}", t("upload.error.sxcu_invalid"))))?;
    let Value::Object(top) = doc else {
        return Err(Error::invalid(t("upload.error.sxcu_invalid").into_owned()));
    };
    // ShareX files use PascalCase; tolerate any casing.
    let fields: std::collections::HashMap<String, &Value> = top.iter().map(|(k, v)| (k.to_ascii_lowercase(), v)).collect();
    let text = |key: &str| fields.get(key).map(|v| value_text(v)).unwrap_or_default();

    let regex_list: Vec<String> =
        fields.get("regexlist").and_then(|v| v.as_array()).map(|a| a.iter().map(value_text).collect()).unwrap_or_default();
    let convert = |s: String| convert_legacy(&s, &regex_list);

    let name = Some(text("name")).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| t("upload.sxcu.default_name").into_owned());
    let request_url = text("requesturl");
    if request_url.trim().is_empty() {
        return Err(Error::invalid(t("upload.error.sxcu_no_url").into_owned()));
    }

    let method = Some(text("requestmethod"))
        .filter(|m| !m.is_empty())
        .or_else(|| Some(text("requesttype")).filter(|m| !m.is_empty()))
        .unwrap_or_else(|| "POST".into());
    let pairs = |key: &str| -> Vec<(String, String)> {
        match fields.get(key) {
            Some(Value::Object(o)) => o.iter().map(|(k, v)| (k.clone(), convert(value_text(v)))).collect(),
            _ => Vec::new(),
        }
    };

    let file_form_name = text("fileformname");
    let arguments = pairs("arguments");
    let body = match text("body").to_ascii_lowercase().as_str() {
        "multipartformdata" => "multipart_form_data",
        "formurlencoded" => "form_url_encoded",
        "json" => "json",
        "xml" => "xml",
        "binary" => "binary",
        "none" => "none",
        _ if !file_form_name.is_empty() => "multipart_form_data",
        _ => "",
    };
    let data = fields.get("data").map(|d| convert(value_text(d))).unwrap_or_default();

    let mut url = convert(text("url"));
    if url.is_empty() {
        url = match text("responsetype").to_ascii_lowercase().as_str() {
            "redirectionurl" => "{responseurl}".into(),
            "locationheader" => "{header:Location}".into(),
            _ => String::new(),
        };
    }

    Ok((
        name,
        CustomUploader {
            request_method: method.to_ascii_uppercase(),
            request_url: convert(request_url),
            parameters: pairs("parameters"),
            headers: pairs("headers"),
            body: body.into(),
            data,
            arguments,
            file_form_name,
            url,
            thumbnail_url: convert(text("thumbnailurl")),
            deletion_url: convert(text("deletionurl")),
            error_message: convert(text("errormessage")),
        },
    ))
}

/// Serialize as a ShareX `.sxcu` document (pretty-printed, ShareX field order).
pub fn export(name: &str, c: &CustomUploader) -> String {
    let mut fields: Vec<(&str, String)> = vec![
        ("Version", quote("14.1.0")),
        ("Name", quote(name)),
        ("DestinationType", quote("ImageUploader, TextUploader, FileUploader")),
        ("RequestMethod", quote(if c.request_method.is_empty() { "POST" } else { &c.request_method })),
        ("RequestURL", quote(&c.request_url)),
    ];
    if !c.parameters.is_empty() {
        fields.push(("Parameters", object(&c.parameters)));
    }
    if !c.headers.is_empty() {
        fields.push(("Headers", object(&c.headers)));
    }
    let sharex_body = match c.body.as_str() {
        "multipart_form_data" => Some("MultipartFormData"),
        "form_url_encoded" => Some("FormURLEncoded"),
        "json" => Some("JSON"),
        "xml" => Some("XML"),
        "binary" => Some("Binary"),
        _ => None,
    };
    if let Some(b) = sharex_body {
        fields.push(("Body", quote(b)));
    }
    if !c.arguments.is_empty() {
        fields.push(("Arguments", object(&c.arguments)));
    }
    if !c.data.is_empty() {
        fields.push(("Data", quote(&c.data)));
    }
    if !c.file_form_name.is_empty() {
        fields.push(("FileFormName", quote(&c.file_form_name)));
    }
    for (key, value) in
        [("URL", &c.url), ("ThumbnailURL", &c.thumbnail_url), ("DeletionURL", &c.deletion_url), ("ErrorMessage", &c.error_message)]
    {
        if !value.is_empty() {
            fields.push((key, quote(value)));
        }
    }

    let body = fields.iter().map(|(k, v)| format!("  \"{k}\": {v}")).collect::<Vec<_>>().join(",\n");
    format!("{{\n{body}\n}}\n")
}

fn quote(s: &str) -> String {
    Value::String(s.to_string()).to_string()
}

fn object(pairs: &[(String, String)]) -> String {
    let inner = pairs.iter().map(|(k, v)| format!("    {}: {}", quote(k), quote(v))).collect::<Vec<_>>().join(",\n");
    format!("{{\n{inner}\n  }}")
}

fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Rewrite legacy `$name:arg$` / `$name$` / `$N$` placeholders as `{name:arg}`.
fn convert_legacy(s: &str, regex_list: &[String]) -> String {
    if !s.contains('$') {
        return s.to_string();
    }
    let index = Regex::new(r"\$(\d+)\$").expect("static pattern");
    let named = Regex::new(r"\$([A-Za-z]+)(?::([^$]*))?\$").expect("static pattern");
    let listed = |n: &str| n.parse::<usize>().ok().and_then(|n| n.checked_sub(1)).and_then(|i| regex_list.get(i));
    let step = index.replace_all(s, |c: &regex::Captures| match listed(&c[1]) {
        Some(pattern) => format!("{{regex:{pattern}}}"),
        None => c[0].to_string(),
    });
    named
        .replace_all(&step, |c: &regex::Captures| {
            let name = c[1].to_ascii_lowercase();
            match c.get(2).map(|a| a.as_str()) {
                Some(arg) if name == "regex" => format!("{{regex:{}}}", listed(arg).map_or(arg, |p| p.as_str())),
                Some(arg) => format!("{{{name}:{arg}}}"),
                None => format!("{{{name}}}"),
            }
        })
        .into_owned()
}
