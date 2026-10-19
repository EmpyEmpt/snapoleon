//! Telegram Bot API: `sendPhoto` / `sendDocument` / `sendVideo` / `sendAnimation`.

use serde_json::Value;
use snap_core::{Error, Result, t, tf};

use crate::{
    UploadRequest, UploadResult, Uploader,
    http::{Call, Progress},
    multipart::Multipart,
    util::{human_size, is_image, is_video, mime_or_octet},
};

const PROVIDER: &str = "Telegram";
const PHOTO_LIMIT: usize = 10 * 1024 * 1024;
/// Bot API ceiling for any uploaded file.
const FILE_LIMIT: usize = 50 * 1024 * 1024;

pub(crate) struct Telegram {
    pub token: String,
    pub chat_id: String,
}

enum Method {
    Photo,
    Document,
    Video,
    Animation,
}

impl Method {
    fn api(&self) -> &'static str {
        match self {
            Method::Photo => "sendPhoto",
            Method::Document => "sendDocument",
            Method::Video => "sendVideo",
            Method::Animation => "sendAnimation",
        }
    }
    fn field(&self) -> &'static str {
        match self {
            Method::Photo => "photo",
            Method::Document => "document",
            Method::Video => "video",
            Method::Animation => "animation",
        }
    }
}

impl Telegram {
    fn endpoint(&self, method: &str) -> String {
        format!("https://api.telegram.org/bot{}/{method}", self.token.trim())
    }

    /// Accepts `@name`, `name`, `t.me/name`, `https://t.me/name` or a numeric id.
    fn chat(&self) -> String {
        let id = self.chat_id.trim();
        let name = id.trim_start_matches("https://").trim_start_matches("http://").trim_start_matches("t.me/").trim_end_matches('/');
        if name.parse::<i64>().is_ok() { name.to_string() } else { format!("@{}", name.trim_start_matches('@')) }
    }

    fn pick_method(req: &UploadRequest) -> Method {
        let mime = req.mime;
        if mime == "image/gif" {
            Method::Animation
        } else if is_video(mime) {
            Method::Video
        } else if is_image(mime) && matches!(mime, "image/png" | "image/jpeg" | "image/webp") && req.data.len() <= PHOTO_LIMIT {
            Method::Photo
        } else {
            Method::Document
        }
    }

    fn send(&self, method: &Method, req: &UploadRequest, progress: &dyn Fn(u64, u64)) -> Result<crate::http::Response> {
        let mut form = Multipart::form().text("chat_id", &self.chat());
        if let Some(title) = req.title.filter(|t| !t.is_empty()) {
            form = form.text("caption", &title.chars().take(1024).collect::<String>());
        }
        if matches!(method, Method::Video) {
            form = form.text("supports_streaming", "true");
        }
        let form = form.file(method.field(), req.file_name, mime_or_octet(req.mime), req.data);
        Call::post(self.endpoint(method.api())).multipart(form).progress(Progress::new(progress, req.data.len() as u64)).send()
    }

    /// Public link to the posted message when it can be derived.
    fn message_link(&self, result: &Value) -> String {
        let message_id = result["message_id"].as_i64().unwrap_or_default();
        let chat = &result["chat"];
        let username = chat["username"].as_str().map(String::from).or_else(|| self.chat().strip_prefix('@').map(String::from));
        if let Some(name) = username {
            return format!("https://t.me/{name}/{message_id}");
        }
        let chat_id = chat["id"].as_i64().unwrap_or_default();
        // Private supergroups and channels have ids `-100<internal id>`.
        if let Some(internal) = chat_id.to_string().strip_prefix("-100") {
            return format!("https://t.me/c/{internal}/{message_id}");
        }
        format!("tg://openmessage?chat_id={chat_id}&message_id={message_id}")
    }
}

impl Uploader for Telegram {
    fn upload(&self, req: &UploadRequest, progress: &dyn Fn(u64, u64)) -> Result<UploadResult> {
        if req.data.len() > FILE_LIMIT {
            return Err(Error::invalid(tf(
                "upload.error.file_too_large",
                &[("provider", PROVIDER), ("limit", &human_size(FILE_LIMIT as u64))],
            )));
        }
        let method = Self::pick_method(req);
        let mut resp = self.send(&method, req, progress)?;
        // Photos have extra limits (dimensions, aspect ratio); a document always works.
        if resp.status == 400 && matches!(method, Method::Photo) {
            resp = self.send(&Method::Document, req, progress)?;
        }
        let resp = resp.check(PROVIDER)?;
        let json = resp.json()?;
        if json["ok"].as_bool() != Some(true) {
            return Err(Error::Network(json["description"].as_str().unwrap_or("Telegram API error").to_string()));
        }
        let total = req.data.len() as u64;
        progress(total, total);
        Ok(UploadResult { url: self.message_link(&json["result"]), thumbnail_url: None, delete_url: None, provider: PROVIDER.into() })
    }

    fn test(&self) -> Result<()> {
        Call::get(self.endpoint("getMe")).send()?.check(PROVIDER)?;
        let resp = Call::post(self.endpoint("getChat")).form(&[("chat_id", &self.chat())]).send()?;
        if resp.status == 400 || resp.status == 403 {
            return Err(Error::NotFound(t("upload.error.telegram_chat").into_owned()));
        }
        resp.check(PROVIDER).map(|_| ())
    }
}
