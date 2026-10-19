//! ShareX-compatible custom HTTP uploader.
//!
//! For `json` / `xml` bodies [`CustomUploader::data`] holds the raw body template (ShareX's `Data`);
//! without it a JSON object is built from the arguments.

pub mod sxcu;
pub(crate) mod template;

use snap_core::{Error, Result, settings::CustomUploader, t, tf};
use template::{Escape, ResponseView, Vars, expand};

use crate::{
    Body, UploadRequest, UploadResult, Uploader,
    http::{Call, Progress},
    multipart::Multipart,
    util::mime_or_octet,
};

const PROVIDER: &str = "Custom uploader";

pub(crate) struct Custom {
    pub cfg: CustomUploader,
    /// Optional secret from the keyring, available as `{secret}`.
    pub secret: String,
}

#[derive(PartialEq, Eq)]
enum BodyKind {
    None,
    Multipart,
    Binary,
    Form,
    Json,
    Xml,
}

impl Custom {
    fn method(&self) -> String {
        let m = self.cfg.request_method.trim();
        if m.is_empty() { "POST".into() } else { m.to_ascii_uppercase() }
    }

    fn body_kind(&self) -> BodyKind {
        let norm: String = self.cfg.body.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_lowercase();
        match norm.as_str() {
            "multipartformdata" | "multipart" => BodyKind::Multipart,
            "binary" => BodyKind::Binary,
            "formurlencoded" | "form" => BodyKind::Form,
            "json" => BodyKind::Json,
            "xml" => BodyKind::Xml,
            "none" => BodyKind::None,
            _ if !self.cfg.file_form_name.trim().is_empty() => BodyKind::Multipart,
            _ if matches!(self.method().as_str(), "POST" | "PUT" | "PATCH") => BodyKind::Multipart,
            _ => BodyKind::None,
        }
    }

    fn vars<'a>(&'a self, req: &'a UploadRequest) -> Result<Vars<'a>> {
        let input = if req.mime.starts_with("text/") {
            String::from_utf8_lossy(&req.body.bytes()?).into_owned()
        } else {
            req.file_name.to_string()
        };
        Ok(Vars {
            file_name: req.file_name,
            input,
            mime: mime_or_octet(req.mime),
            title: req.title.unwrap_or_default(),
            secret: &self.secret,
            file: req.body,
        })
    }

    fn request_url(&self, vars: &Vars) -> Result<String> {
        let raw = expand(&self.cfg.request_url, vars, None, Escape::None);
        let mut url = url::Url::parse(raw.trim()).map_err(|_| Error::invalid(tf("upload.error.bad_url", &[("url", &raw)])))?;
        if !self.cfg.parameters.is_empty() {
            let mut q = url.query_pairs_mut();
            for (k, v) in &self.cfg.parameters {
                q.append_pair(&expand(k, vars, None, Escape::None), &expand(v, vars, None, Escape::None));
            }
        }
        Ok(url.to_string())
    }
}

impl Uploader for Custom {
    fn upload(&self, req: &UploadRequest, progress: &dyn Fn(u64, u64)) -> Result<UploadResult> {
        let vars = self.vars(req)?;
        let ex = |s: &str| expand(s, &vars, None, Escape::None);
        let method = self.method();
        let kind = self.body_kind();
        let total = req.body.size();

        let mut call = Call::new(&method, self.request_url(&vars)?).same_origin_redirects();
        let mut user_content_type = None;
        for (k, v) in &self.cfg.headers {
            let value = ex(v);
            match k.to_ascii_lowercase().as_str() {
                "content-length" | "host" => {}
                "content-type" => user_content_type = Some(value),
                _ => call = call.header(k, value),
            }
        }

        let field = if self.cfg.file_form_name.trim().is_empty() { "file" } else { self.cfg.file_form_name.trim() };
        call = match kind {
            BodyKind::None => call,
            BodyKind::Multipart => {
                let mut form = Multipart::form();
                for (k, v) in self.cfg.arguments.iter() {
                    form = form.text(&ex(k), &ex(v));
                }
                call.multipart(form.file(field, req.file_name, mime_or_octet(req.mime), req.body)).progress(Progress::new(progress, total))
            }
            BodyKind::Binary => {
                let ct = user_content_type.unwrap_or_else(|| mime_or_octet(req.mime).to_string());
                call.body(req.body, &ct).progress(Progress::new(progress, total))
            }
            BodyKind::Form => {
                let pairs: Vec<(String, String)> = self.cfg.arguments.iter().map(|(k, v)| (ex(k), ex(v))).collect();
                let refs: Vec<(&str, &str)> = pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
                call.form(&refs)
            }
            BodyKind::Json | BodyKind::Xml => {
                let is_json = kind == BodyKind::Json;
                let escape = if is_json { Escape::Json } else { Escape::Xml };
                let raw = Some(&self.cfg.data).filter(|d| !d.is_empty());
                let body = match raw {
                    Some(template) => expand(template, &vars, None, escape),
                    None => {
                        let object: serde_json::Map<String, serde_json::Value> =
                            self.cfg.arguments.iter().map(|(k, v)| (ex(k), serde_json::Value::String(ex(v)))).collect();
                        serde_json::Value::Object(object).to_string()
                    }
                };
                let default_ct = if is_json { "application/json" } else { "application/xml" };
                call.owned(body.into_bytes(), user_content_type.as_deref().unwrap_or(default_ct))
            }
        };

        let resp = call.send()?;
        progress(total, total);
        let text = resp.text();
        let view = ResponseView::new(&text, &resp.url, &resp.headers);
        let rx = |s: &str| expand(s, &vars, Some(&view), Escape::None).trim().to_string();

        let message = rx(&self.cfg.error_message);
        if !message.is_empty() {
            return Err(Error::Network(message));
        }
        if !resp.is_success() {
            return Err(resp.error(PROVIDER));
        }

        let url = if self.cfg.url.trim().is_empty() { text.trim().to_string() } else { rx(&self.cfg.url) };
        if url.is_empty() {
            return Err(Error::Network(t("upload.error.custom_no_url").into_owned()));
        }
        let non_empty = |s: String| (!s.is_empty()).then_some(s);
        Ok(UploadResult {
            url,
            thumbnail_url: non_empty(rx(&self.cfg.thumbnail_url)),
            delete_url: non_empty(rx(&self.cfg.deletion_url)),
            provider: PROVIDER.into(),
        })
    }

    /// Checks that the endpoint is configured and reachable without uploading
    /// anything: any HTTP answer, even an error status, proves the host responds.
    fn test(&self) -> Result<()> {
        let dummy = UploadRequest {
            body: Body::Bytes(b""),
            file_name: "test.png",
            mime: "image/png",
            title: None,
            expiry_hours: None,
            password: None,
        };
        let vars = self.vars(&dummy)?;
        let url = self.request_url(&vars)?;
        let parsed = url::Url::parse(&url).map_err(|_| Error::invalid(tf("upload.error.bad_url", &[("url", &url)])))?;
        Call::new("HEAD", parsed.origin().ascii_serialization()).send().map(|_| ())
    }
}
