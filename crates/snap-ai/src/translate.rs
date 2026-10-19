//! Translation providers: LibreTranslate, DeepL and the configured AI endpoint.

use serde_json::{Value, json};
use snap_core::{
    Error, Result,
    secrets::SecretStore,
    settings::{AiSettings, TranslateProvider, TranslateSettings},
    t,
};

use crate::{Ai, client::Client};

/// Translates `text` into `target` using the provider chosen in `settings`.
/// An empty `target` falls back to `settings.target_lang`.
pub fn translate(
    text: &str,
    target: &str,
    settings: &TranslateSettings,
    ai: &AiSettings,
    secrets: &dyn SecretStore,
) -> Result<String> {
    let target = if target.trim().is_empty() { settings.target_lang.trim() } else { target.trim() };
    if target.is_empty() {
        return Err(Error::invalid(t("ai.error.no_target")));
    }
    if text.trim().is_empty() {
        return Ok(String::new());
    }
    let key = settings
        .secret_ref
        .as_deref()
        .filter(|r| !r.trim().is_empty())
        .and_then(|r| secrets.get(r))
        .map(|k| k.trim().to_owned())
        .filter(|k| !k.is_empty());
    match &settings.provider {
        TranslateProvider::None => Err(Error::invalid(t("ai.error.translate_none"))),
        TranslateProvider::Ai => Ai::new(ai, secrets).translate(text, target),
        TranslateProvider::LibreTranslate { url } => libre(url, text, target, key),
        TranslateProvider::DeepL { free_api } => deepl(*free_api, text, target, key),
    }
}

fn libre(url: &str, text: &str, target: &str, key: Option<String>) -> Result<String> {
    let base = url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err(Error::invalid(t("ai.error.no_endpoint")));
    }
    let endpoint = if base.ends_with("/translate") { base.to_owned() } else { format!("{base}/translate") };
    let mut body = json!({"q": text, "source": "auto", "target": libre_code(target), "format": "text"});
    if let Some(key) = key {
        body["api_key"] = Value::String(key);
    }
    let reply = Client::new("", None).post_service_json(&endpoint, &[], &body)?;
    reply
        .get("translatedText")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| Error::Network(t("ai.error.bad_response").into_owned()))
}

fn deepl(free: bool, text: &str, target: &str, key: Option<String>) -> Result<String> {
    let key = key.ok_or_else(|| Error::invalid(t("ai.error.no_key")))?;
    let host = if free { "api-free.deepl.com" } else { "api.deepl.com" };
    let body = json!({"text": [text], "target_lang": deepl_code(target)});
    let reply = Client::new("", None).post_service_json(
        &format!("https://{host}/v2/translate"),
        &[("Authorization", format!("DeepL-Auth-Key {key}"))],
        &body,
    )?;
    reply
        .pointer("/translations/0/text")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| Error::Network(t("ai.error.bad_response").into_owned()))
}

/// LibreTranslate uses lowercase base codes (`pt`, `zh`), with `zt` for Traditional Chinese.
fn libre_code(code: &str) -> String {
    let lower = code.to_ascii_lowercase().replace('_', "-");
    if matches!(lower.as_str(), "zh-tw" | "zh-hant") {
        "zt".into()
    } else {
        lower.split('-').next().unwrap_or("").to_owned()
    }
}

/// DeepL wants uppercase codes and rejects bare `EN` / `PT` as targets.
fn deepl_code(code: &str) -> String {
    let upper = code.to_ascii_uppercase().replace('_', "-");
    match upper.as_str() {
        "EN" => "EN-US".into(),
        "PT" => "PT-BR".into(),
        "ZH-TW" | "ZH-HANT" => "ZH-HANT".into(),
        "ZH-CN" | "ZH-HANS" => "ZH-HANS".into(),
        _ => upper,
    }
}
