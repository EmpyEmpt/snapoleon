//! URL shorteners: team server, YOURLS, Shlink and is.gd.

use serde_json::{Value, json};
use snap_core::{
    Error, Result,
    secrets::{SecretStore, secret_key},
    settings::Shortener,
    t,
};

use crate::{http::Call, server::Client, util::pct};

/// Keyring key under which the team-server token is looked up for
/// [`Shortener::SnapServer`] (that variant carries no `secret_ref` of its own).
pub fn snap_server_secret_key(server_url: &str) -> String {
    secret_key("snap_server", server_url.trim().trim_end_matches('/'))
}

/// Shorten `url`. `server` is the team-server base URL, used by
/// [`Shortener::SnapServer`] only.
pub fn shorten(url: &str, s: &Shortener, server: Option<&str>, secrets: &dyn SecretStore) -> Result<String> {
    match s {
        Shortener::None => Ok(url.to_string()),
        Shortener::SnapServer => {
            let base = server.filter(|u| !u.trim().is_empty()).ok_or_else(|| Error::invalid(t("upload.error.shorten_server").into_owned()))?;
            let token = secrets.get(&snap_server_secret_key(base)).unwrap_or_default();
            Client::new(base, &token).shorten(url)
        }
        Shortener::Yourls { url: api, secret_ref } => yourls(url, api, secret_ref.as_deref().and_then(|k| secrets.get(k)).as_deref().unwrap_or_default()),
        Shortener::Shlink { url: api, secret_ref } => {
            let key = secret_ref.as_deref().and_then(|k| secrets.get(k)).ok_or_else(|| Error::permission(t("upload.error.shorten_key").into_owned()))?;
            shlink(url, api, key.trim())
        }
        Shortener::IsGd => is_gd(url),
    }
}

fn yourls(long_url: &str, api: &str, signature: &str) -> Result<String> {
    let endpoint = if api.trim_end_matches('/').ends_with(".php") { api.trim().to_string() } else { format!("{}/yourls-api.php", api.trim().trim_end_matches('/')) };
    let mut form = vec![("action", "shorturl"), ("format", "json"), ("url", long_url)];
    if !signature.trim().is_empty() {
        form.push(("signature", signature.trim()));
    }
    let resp = Call::post(endpoint).form(&form).send()?;
    let body: Value = resp.json()?;
    // An already-shortened URL answers `fail` but still carries `shorturl`.
    if let Some(short) = body["shorturl"].as_str() {
        return Ok(short.to_string());
    }
    let message = body["message"].as_str().or_else(|| body["errorMessage"].as_str()).unwrap_or("YOURLS error");
    Err(Error::Network(message.to_string()))
}

fn shlink(long_url: &str, api: &str, key: &str) -> Result<String> {
    let endpoint = format!("{}/rest/v3/short-urls", api.trim().trim_end_matches('/'));
    let resp = Call::post(endpoint).header("X-Api-Key", key).header("Accept", "application/json").json(&json!({ "longUrl": long_url, "findIfExists": true })).send()?;
    if !resp.is_success() {
        let body: Value = resp.json().unwrap_or(Value::Null);
        let detail = body["detail"].as_str().or_else(|| body["title"].as_str());
        return Err(match detail {
            Some(d) => Error::Network(d.to_string()),
            None => resp.error("Shlink"),
        });
    }
    resp.json()?["shortUrl"].as_str().map(String::from).ok_or_else(|| Error::Network(t("upload.error.no_link").into_owned()))
}

fn is_gd(long_url: &str) -> Result<String> {
    let resp = Call::get(format!("https://is.gd/create.php?format=json&logstats=0&url={}", pct(long_url))).send()?;
    let body: Value = resp.json()?;
    match body["shorturl"].as_str() {
        Some(short) => Ok(short.to_string()),
        None => Err(Error::Network(body["errormessage"].as_str().unwrap_or("is.gd error").to_string())),
    }
}
