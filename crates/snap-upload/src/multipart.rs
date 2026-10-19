//! Minimal `multipart/form-data` (and `multipart/related`) body builder.
//! File payloads are referenced, never copied, so large videos stay cheap.

use rand::{Rng, distr::Alphanumeric};

use crate::body::{Body, Chunk};

pub(crate) struct Multipart<'a> {
    boundary: String,
    subtype: &'static str,
    parts: Vec<Chunk<'a>>,
    pending: Vec<u8>,
}

impl<'a> Multipart<'a> {
    pub fn form() -> Self {
        Self::with_subtype("form-data")
    }

    /// `multipart/related`, as used by Google Drive's metadata + media upload.
    pub fn related() -> Self {
        Self::with_subtype("related")
    }

    fn with_subtype(subtype: &'static str) -> Self {
        let token: String = rand::rng().sample_iter(Alphanumeric).take(24).map(char::from).collect();
        Self { boundary: format!("----SnapoleonFormBoundary{token}"), subtype, parts: Vec::new(), pending: Vec::new() }
    }

    fn open_part(&mut self, headers: &[(&str, String)]) {
        self.pending.extend_from_slice(format!("--{}\r\n", self.boundary).as_bytes());
        for (k, v) in headers {
            self.pending.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
        }
        self.pending.extend_from_slice(b"\r\n");
    }

    fn close_part(&mut self) {
        self.pending.extend_from_slice(b"\r\n");
    }

    pub fn text(mut self, name: &str, value: &str) -> Self {
        self.open_part(&[("Content-Disposition", format!("form-data; name=\"{}\"", quote(name)))]);
        self.pending.extend_from_slice(value.as_bytes());
        self.close_part();
        self
    }

    pub fn file(mut self, name: &str, file_name: &str, mime: &str, data: Body<'a>) -> Self {
        self.open_part(&[
            ("Content-Disposition", format!("form-data; name=\"{}\"; filename=\"{}\"", quote(name), quote(file_name))),
            ("Content-Type", mime.to_string()),
        ]);
        self.data(data)
    }

    /// An unnamed part with a content type (for `multipart/related`). Inline
    /// text goes through [`Multipart::inline`].
    pub fn part(mut self, content_type: &str, body: Body<'a>) -> Self {
        self.open_part(&[("Content-Type", content_type.to_string())]);
        self.data(body)
    }

    pub fn inline(mut self, content_type: &str, body: &str) -> Self {
        self.open_part(&[("Content-Type", content_type.to_string())]);
        self.pending.extend_from_slice(body.as_bytes());
        self.close_part();
        self
    }

    fn data(mut self, data: Body<'a>) -> Self {
        self.parts.push(Chunk::Owned(std::mem::take(&mut self.pending)));
        self.parts.push(Chunk::Payload(data));
        self.close_part();
        self
    }

    /// Returns the `Content-Type` header value and the body chunks.
    pub fn finish(mut self) -> (String, Vec<Chunk<'a>>) {
        self.pending.extend_from_slice(format!("--{}--\r\n", self.boundary).as_bytes());
        self.parts.push(Chunk::Owned(self.pending));
        (format!("multipart/{}; boundary={}", self.subtype, self.boundary), self.parts)
    }
}

fn quote(s: &str) -> String {
    s.chars().filter(|c| *c != '\r' && *c != '\n').collect::<String>().replace('\\', "\\\\").replace('"', "%22")
}
