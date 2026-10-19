//! Wire protocols: OpenAI-compatible chat completions and Anthropic Messages.

use std::time::Duration;

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use snap_core::{Error, Result, t, tf};

use crate::text::clean_reply;

const TEMPERATURE: f64 = 0.2;
const ANTHROPIC_VERSION: &str = "2023-06-01";
const ANTHROPIC_HOST: &str = "api.anthropic.com";

pub enum Part {
    Text(String),
    /// JPEG bytes.
    Image(Vec<u8>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Backend {
    OpenAi,
    Anthropic,
}

pub struct Client {
    base: String,
    backend: Backend,
    key: Option<String>,
    agent: ureq::Agent,
}

impl Client {
    pub fn new(endpoint: &str, key: Option<String>) -> Self {
        let mut base = endpoint.trim().trim_end_matches('/').to_owned();
        if let Some(stripped) = base.strip_suffix("/chat/completions") {
            base = stripped.to_owned();
        }
        let backend =
            if host_of(&base).eq_ignore_ascii_case(ANTHROPIC_HOST) { Backend::Anthropic } else { Backend::OpenAi };
        if backend == Backend::Anthropic {
            if let Some(stripped) = base.strip_suffix("/messages") {
                base = stripped.to_owned();
            }
            if !has_path(&base) {
                base.push_str("/v1");
            }
        }
        let config = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(5)))
            .timeout_send_body(Some(Duration::from_secs(120)))
            .timeout_recv_response(Some(Duration::from_secs(120)))
            .timeout_recv_body(Some(Duration::from_secs(120)))
            .http_status_as_error(false)
            .build();
        Self { base, backend, key, agent: config.into() }
    }

    /// One user turn with a system prompt; returns the cleaned reply text.
    pub fn chat(&self, model: &str, system: &str, user: &[Part], max_tokens: u32) -> Result<String> {
        let (url, body) = match self.backend {
            Backend::OpenAi => (format!("{}/chat/completions", self.base), openai_body(model, system, user, true)),
            Backend::Anthropic => {
                (format!("{}/messages", self.base), anthropic_body(model, system, user, max_tokens))
            }
        };
        tracing::debug!(%url, model, parts = user.len(), "ai request");
        let (mut status, mut text) = self.post_json(&url, &body)?;
        if status == 400 && self.backend == Backend::OpenAi && text.contains("temperature") {
            // Some hosted models only accept the default temperature.
            (status, text) = self.post_json(&url, &openai_body(model, system, user, false))?;
        }
        if !(200..300).contains(&status) {
            return Err(http_error(status, &text));
        }
        let reply = match self.backend {
            Backend::OpenAi => openai_reply(&text),
            Backend::Anthropic => anthropic_reply(&text),
        };
        let reply = clean_reply(&reply.ok_or_else(|| Error::Network(t("ai.error.bad_response").into_owned()))?);
        if reply.is_empty() {
            return Err(Error::Network(t("ai.error.empty_response").into_owned()));
        }
        Ok(reply)
    }

    /// Lists models; falls back to a 1-token completion when the server has no `/models`.
    pub fn test(&self, model: &str, vision_model: &str) -> Result<String> {
        let (status, text) = self.get(&format!("{}/models", self.base))?;
        if status == 404 || status == 405 {
            self.chat(model, "Reply with one word.", &[Part::Text("ping".into())], 1)?;
            return Ok(tf("ai.test.ok_chat", &[("endpoint", &self.base), ("model", model)]));
        }
        if !(200..300).contains(&status) {
            return Err(http_error(status, &text));
        }
        let ids = model_ids(&text);
        if ids.is_empty() {
            return Ok(tf("ai.test.ok_empty", &[("endpoint", &self.base)]));
        }
        let shown: Vec<&str> = ids.iter().take(8).map(String::as_str).collect();
        let mut list = shown.join(", ");
        if ids.len() > shown.len() {
            list.push_str(", …");
        }
        let mut out =
            tf("ai.test.ok", &[("endpoint", &self.base), ("count", &ids.len().to_string()), ("models", &list)]);
        let mut missing: Vec<&str> = [model, vision_model]
            .into_iter()
            .filter(|m| !m.is_empty() && !ids.iter().any(|id| model_matches(id, m)))
            .collect();
        missing.dedup();
        if !missing.is_empty() {
            out.push('\n');
            out.push_str(&tf("ai.test.model_missing", &[("models", &missing.join(", "))]));
        }
        Ok(out)
    }

    /// Unauthenticated-by-default JSON POST for translation services.
    pub fn post_service_json(&self, url: &str, headers: &[(&str, String)], body: &Value) -> Result<Value> {
        let mut req = self.agent.post(url);
        for (k, v) in headers {
            req = req.header(*k, v);
        }
        let mut resp = req.send_json(body).map_err(|e| unreachable_error(url, e))?;
        let status = resp.status().as_u16();
        let text = read_body(&mut resp)?;
        if !(200..300).contains(&status) {
            return Err(http_error(status, &text));
        }
        serde_json::from_str(&text).map_err(|_| Error::Network(t("ai.error.bad_response").into_owned()))
    }

    fn post_json(&self, url: &str, body: &Value) -> Result<(u16, String)> {
        let mut req = self.agent.post(url);
        for (k, v) in self.auth_headers() {
            req = req.header(k, v);
        }
        let mut resp = req.send_json(body).map_err(|e| unreachable_error(url, e))?;
        let status = resp.status().as_u16();
        Ok((status, read_body(&mut resp)?))
    }

    fn get(&self, url: &str) -> Result<(u16, String)> {
        let mut req = self.agent.get(url);
        for (k, v) in self.auth_headers() {
            req = req.header(k, v);
        }
        let mut resp = req.call().map_err(|e| unreachable_error(url, e))?;
        let status = resp.status().as_u16();
        Ok((status, read_body(&mut resp)?))
    }

    fn auth_headers(&self) -> Vec<(&'static str, String)> {
        match (self.backend, &self.key) {
            (Backend::OpenAi, Some(k)) => vec![("Authorization", format!("Bearer {k}"))],
            (Backend::OpenAi, None) => vec![],
            (Backend::Anthropic, k) => {
                let mut h = vec![("anthropic-version", ANTHROPIC_VERSION.to_owned())];
                if let Some(k) = k {
                    h.push(("x-api-key", k.clone()));
                }
                h
            }
        }
    }
}

fn read_body(resp: &mut ureq::http::Response<ureq::Body>) -> Result<String> {
    resp.body_mut().read_to_string().map_err(Error::network)
}

fn unreachable_error(url: &str, e: ureq::Error) -> Error {
    Error::Network(tf("ai.error.unreachable", &[("url", url), ("error", &e.to_string())]))
}

fn http_error(status: u16, body: &str) -> Error {
    let message = error_message(body).unwrap_or_else(|| body.chars().take(200).collect());
    Error::Network(tf("ai.error.http", &[("status", &status.to_string()), ("message", message.trim())]))
}

fn error_message(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body).ok()?;
    let e = v.get("error").unwrap_or(&v);
    match e {
        Value::String(s) => Some(s.clone()),
        Value::Object(_) => e.get("message").and_then(Value::as_str).map(str::to_owned),
        _ => v.get("message").and_then(Value::as_str).map(str::to_owned),
    }
}

fn openai_body(model: &str, system: &str, user: &[Part], with_temperature: bool) -> Value {
    let content = if user.iter().all(|p| matches!(p, Part::Text(_))) {
        let texts: Vec<&str> = user
            .iter()
            .filter_map(|p| if let Part::Text(s) = p { Some(s.as_str()) } else { None })
            .collect();
        Value::String(texts.join("\n\n"))
    } else {
        Value::Array(
            user.iter()
                .map(|p| match p {
                    Part::Text(s) => json!({"type": "text", "text": s}),
                    Part::Image(jpeg) => json!({
                        "type": "image_url",
                        "image_url": {"url": format!("data:image/jpeg;base64,{}", STANDARD.encode(jpeg))},
                    }),
                })
                .collect(),
        )
    };
    let mut body = json!({
        "model": model,
        "stream": false,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": content},
        ],
    });
    if with_temperature {
        body["temperature"] = json!(TEMPERATURE);
    }
    body
}

fn anthropic_body(model: &str, system: &str, user: &[Part], max_tokens: u32) -> Value {
    let blocks: Vec<Value> = user
        .iter()
        .map(|p| match p {
            Part::Text(s) => json!({"type": "text", "text": s}),
            Part::Image(jpeg) => json!({
                "type": "image",
                "source": {"type": "base64", "media_type": "image/jpeg", "data": STANDARD.encode(jpeg)},
            }),
        })
        .collect();
    json!({
        "model": model,
        "max_tokens": max_tokens,
        "temperature": TEMPERATURE,
        "system": system,
        "messages": [{"role": "user", "content": blocks}],
    })
}

fn openai_reply(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body).ok()?;
    match v.pointer("/choices/0/message/content")? {
        Value::String(s) => Some(s.clone()),
        Value::Array(parts) => {
            Some(parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join(""))
        }
        _ => None,
    }
}

fn anthropic_reply(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body).ok()?;
    let blocks = v.get("content")?.as_array()?;
    Some(
        blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
    )
}

fn model_ids(body: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<Value>(body) else { return Vec::new() };
    let list = v.get("data").or_else(|| v.get("models")).and_then(Value::as_array);
    list.into_iter()
        .flatten()
        .filter_map(|m| m.get("id").or_else(|| m.get("name")).and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

/// Ollama lists `qwen3:8b` and also accepts `qwen3`; LM Studio may prefix publishers.
fn model_matches(listed: &str, wanted: &str) -> bool {
    listed == wanted || listed.strip_suffix(":latest") == Some(wanted) || listed.rsplit('/').next() == Some(wanted)
}

fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or("");
    if let Some(v6) = host_port.strip_prefix('[') {
        return v6.split_once(']').map_or(v6, |(h, _)| h);
    }
    host_port.split(':').next().unwrap_or("")
}

fn has_path(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split_once('/').is_some_and(|(_, p)| !p.is_empty())
}
