//! Imgur anonymous upload with an application Client-ID.

use snap_core::{Error, Result, t};

use crate::{
    Uploader, UploadRequest, UploadResult,
    http::{Call, Progress},
    multipart::Multipart,
    util::{is_video, mime_or_octet},
};

const PROVIDER: &str = "Imgur";
const API: &str = "https://api.imgur.com/3";

pub(crate) struct Imgur {
    pub client_id: String,
}

impl Imgur {
    fn auth(&self) -> String {
        format!("Client-ID {}", self.client_id.trim())
    }
}

impl Uploader for Imgur {
    fn upload(&self, req: &UploadRequest, progress: &dyn Fn(u64, u64)) -> Result<UploadResult> {
        let field = if is_video(req.mime) { "video" } else { "image" };
        let mut form = Multipart::form().text("type", "file");
        if let Some(title) = req.title.filter(|t| !t.is_empty()) {
            form = form.text("title", title);
        }
        let form = form.file(field, req.file_name, mime_or_octet(req.mime), req.data);
        let total = req.data.len() as u64;
        let resp = Call::post(format!("{API}/{}", if field == "video" { "upload" } else { "image" }))
            .header("Authorization", self.auth())
            .multipart(form)
            .progress(Progress::new(progress, total))
            .send()?
            .check(PROVIDER)?;

        let json = resp.json()?;
        let data = &json["data"];
        let link = data["link"].as_str().ok_or_else(|| Error::Network(t("upload.error.no_link").into_owned()))?;
        let id = data["id"].as_str().unwrap_or_default();
        let delete_url = data["deletehash"].as_str().filter(|h| !h.is_empty()).map(|h| format!("https://imgur.com/delete/{h}"));
        // `<id>m.<ext>` is Imgur's 320 px medium thumbnail.
        let thumbnail_url = (!id.is_empty() && !is_video(req.mime)).then(|| {
            let ext = link.rsplit_once('.').map_or("png", |(_, e)| e);
            format!("https://i.imgur.com/{id}m.{ext}")
        });
        progress(total, total);
        Ok(UploadResult { url: link.to_string(), thumbnail_url, delete_url, provider: PROVIDER.into() })
    }

    fn test(&self) -> Result<()> {
        // The credits endpoint is free and only needs a valid Client-ID.
        Call::get(format!("{API}/credits")).header("Authorization", self.auth()).send()?.check(PROVIDER).map(|_| ())
    }
}
