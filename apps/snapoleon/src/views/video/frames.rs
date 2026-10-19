//! Decoding pictures for the editor: the preview frame, the thumbnail strip and a small LRU cache of
//! frame textures. Decoding runs on worker threads; results come back as `Msg`s.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use crossbeam_channel::Sender;
use egui::TextureHandle;
use snap_core::RgbaImage;
use snap_record::edit;

use super::Msg;
use crate::views::ViewHandle;

/// Widest preview picture requested; wider sources are scaled down by the decoder.
const PREVIEW_WIDTH: u32 = 1280;
/// Thumbnails are decoded at this width and drawn smaller.
const THUMB_WIDTH: u32 = 240;
/// Frame textures kept for scrubbing back and forth.
const CACHE_FRAMES: usize = 32;

/// The preview worker: send a timestamp, get `Msg::Frame` back. The view keeps one request in flight,
/// so the worker never has a backlog. It ends when the sender is dropped with the view.
pub fn spawn_preview(path: PathBuf, handle: ViewHandle) -> Sender<u64> {
    let (tx, rx) = crossbeam_channel::unbounded::<u64>();
    let worker = std::thread::Builder::new().name("snap-video-preview".into()).spawn(move || {
        while let Ok(ms) = rx.recv() {
            let image = edit::frame_at(&path, ms, PREVIEW_WIDTH);
            handle.send(Msg::Frame { ms, image });
        }
    });
    if let Err(e) = worker {
        tracing::error!("cannot start the preview worker: {e}");
    }
    tx
}

/// Decode `count` evenly spaced thumbnails, one `Msg::Thumb` each. Stops early once `current` no longer
/// holds `generation` (the strip was laid out again).
pub fn spawn_thumbnails(path: &Path, handle: ViewHandle, duration_ms: u64, count: usize, generation: u64, current: Arc<AtomicU64>) {
    let path = path.to_path_buf();
    let worker = std::thread::Builder::new().name("snap-video-thumbs".into()).spawn(move || {
        for index in 0..count {
            if current.load(Ordering::Relaxed) != generation {
                return;
            }
            let at = (duration_ms as f64 * (index as f64 + 0.5) / count as f64) as u64;
            match edit::frame_at(&path, at, THUMB_WIDTH) {
                Ok(image) => handle.send(Msg::Thumb { generation, index, image }),
                Err(e) => tracing::debug!("thumbnail {index}: {e}"),
            }
        }
    });
    if let Err(e) = worker {
        tracing::error!("cannot start the thumbnail worker: {e}");
    }
}

/// Recently shown frames, most recent first.
#[derive(Default)]
pub struct FrameCache {
    entries: Vec<(u64, TextureHandle)>,
}

impl FrameCache {
    pub fn get(&mut self, key: u64) -> Option<TextureHandle> {
        let at = self.entries.iter().position(|(k, _)| *k == key)?;
        let entry = self.entries.remove(at);
        let texture = entry.1.clone();
        self.entries.insert(0, entry);
        Some(texture)
    }

    pub fn put(&mut self, key: u64, texture: TextureHandle) {
        self.entries.retain(|(k, _)| *k != key);
        self.entries.insert(0, (key, texture));
        self.entries.truncate(CACHE_FRAMES);
    }
}

pub fn color_image(image: &RgbaImage) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], image.as_raw())
}
