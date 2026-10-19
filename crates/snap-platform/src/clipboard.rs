//! Clipboard access.
//!
//! All operations run on one dedicated thread that owns a long-lived
//! `arboard::Clipboard`. On X11 and Wayland the application that set the
//! clipboard must keep serving it, so the instance has to outlive the call;
//! on Windows and macOS the same thread simply serialises access.

use std::borrow::Cow;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::OnceLock;

use crossbeam_channel::{Sender, bounded, unbounded};
use image::RgbaImage;
use snap_core::{Error, Result};

use crate::util::other;

type Job = Box<dyn FnOnce(&mut Option<arboard::Clipboard>) + Send>;

fn worker() -> &'static Sender<Job> {
    static WORKER: OnceLock<Sender<Job>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = unbounded::<Job>();
        std::thread::Builder::new()
            .name("snap-clipboard".into())
            .spawn(move || {
                let mut clipboard = None;
                for job in rx {
                    if catch_unwind(AssertUnwindSafe(|| job(&mut clipboard))).is_err() {
                        clipboard = None;
                    }
                }
            })
            .expect("spawn clipboard thread");
        tx
    })
}

fn run<R: Send + 'static>(f: impl FnOnce(&mut arboard::Clipboard) -> Result<R, arboard::Error> + Send + 'static) -> Result<R> {
    let (tx, rx) = bounded(1);
    let job: Job = Box::new(move |slot| {
        if slot.is_none() {
            match arboard::Clipboard::new() {
                Ok(c) => *slot = Some(c),
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            }
        }
        if let Some(clipboard) = slot.as_mut() {
            let _ = tx.send(f(clipboard));
        }
    });
    worker().send(job).map_err(|_| other("clipboard thread stopped"))?;
    rx.recv().map_err(|_| other("clipboard operation failed"))?.map_err(convert)
}

fn convert(e: arboard::Error) -> Error {
    match e {
        arboard::Error::ClipboardNotSupported => Error::unsupported("clipboard format not supported here"),
        arboard::Error::ContentNotAvailable => Error::NotFound("clipboard content".into()),
        arboard::Error::ConversionFailure => Error::invalid("clipboard content could not be converted"),
        other_err => other(other_err),
    }
}

pub fn set_text(text: &str) -> Result<()> {
    let text = text.to_owned();
    run(move |c| c.set_text(text))
}

pub fn set_image(img: &RgbaImage) -> Result<()> {
    let data = arboard::ImageData { width: img.width() as usize, height: img.height() as usize, bytes: Cow::Owned(img.as_raw().clone()) };
    run(move |c| c.set_image(data))
}

/// Place HTML on the clipboard; `alt` is the plain-text fallback for targets
/// that do not understand HTML.
pub fn set_html(html: &str, alt: &str) -> Result<()> {
    let (html, alt) = (html.to_owned(), alt.to_owned());
    run(move |c| c.set_html(html, Some(alt)))
}

/// Place files on the clipboard so they can be pasted into a file manager
/// (`CF_HDROP` on Windows, file URLs on macOS, `text/uri-list` on Linux).
pub fn set_files(paths: &[PathBuf]) -> Result<()> {
    if paths.is_empty() {
        return Err(Error::invalid("no files to copy"));
    }
    let paths: Vec<PathBuf> = paths.iter().map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone())).collect();
    run(move |c| c.set().file_list(&paths))
}

pub fn get_image() -> Option<RgbaImage> {
    let data = run(|c| c.get_image()).ok()?;
    RgbaImage::from_raw(data.width as u32, data.height as u32, data.bytes.into_owned())
}

pub fn get_text() -> Option<String> {
    run(|c| c.get_text()).ok()
}
