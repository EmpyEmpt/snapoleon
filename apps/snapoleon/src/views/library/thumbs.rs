//! Lazily decoded thumbnails: a small worker pool decodes, the UI thread keeps a bounded texture cache.

use std::{collections::HashMap, path::PathBuf, thread};

use crossbeam_channel::{Receiver, Sender, unbounded};
use egui::{ColorImage, Context, TextureHandle, TextureOptions};
use snap_core::CaptureKind;
use snap_history::Entry;

use crate::views::ViewHandle;

/// Textures kept alive; the visible grid needs a few dozen, the rest is scroll-back.
const CACHE: usize = 240;
const WORKERS: usize = 3;
/// Longest side of a thumbnail decoded from the original file when history has none.
const FALLBACK_EDGE: u32 = 320;

/// A decoded thumbnail, delivered to the view's `on_msg`. `image` is `None` when nothing could be decoded.
pub struct Loaded {
    pub id: String,
    pub image: Option<ColorImage>,
}

struct Job {
    id: String,
    thumb: Option<PathBuf>,
    file: Option<PathBuf>,
    video: bool,
}

enum Slot {
    Pending,
    Missing,
    Ready(TextureHandle),
}

pub struct Thumbs {
    jobs: Sender<Job>,
    /// Held until [`Self::start`] hands it to the workers.
    queue: Option<Receiver<Job>>,
    slots: HashMap<String, (Slot, u64)>,
    frame: u64,
}

impl Thumbs {
    pub fn new() -> Self {
        let (jobs, queue) = unbounded();
        Self { jobs, queue: Some(queue), slots: HashMap::new(), frame: 0 }
    }

    /// Start the decode workers; they deliver [`Loaded`] to `handle` and end when this cache is dropped.
    pub fn start(&mut self, handle: &ViewHandle) {
        let Some(queue) = self.queue.take() else { return };
        for n in 0..WORKERS {
            let (queue, handle) = (queue.clone(), handle.clone());
            let spawned = thread::Builder::new().name(format!("snap-thumbs-{n}")).spawn(move || work(queue, handle));
            if let Err(e) = spawned {
                tracing::error!("cannot start a thumbnail worker: {e}");
            }
        }
    }

    /// Advance the usage clock; call once per frame before the first [`Self::get`].
    pub fn begin_frame(&mut self) {
        self.frame += 1;
    }

    /// The texture for `entry`, queueing a decode the first time it is asked for.
    pub fn get(&mut self, entry: &Entry) -> Option<&TextureHandle> {
        let frame = self.frame;
        let key = &entry.id.0;
        if !self.slots.contains_key(key) {
            let job =
                Job { id: key.clone(), thumb: entry.thumb.clone(), file: entry.file.clone(), video: entry.kind == CaptureKind::Recording };
            let queued = self.jobs.send(job).is_ok();
            self.slots.insert(key.clone(), (if queued { Slot::Pending } else { Slot::Missing }, frame));
        }
        let (slot, used) = self.slots.get_mut(key)?;
        *used = frame;
        match slot {
            Slot::Ready(tex) => Some(&*tex),
            _ => None,
        }
    }

    pub fn loaded(&mut self, ctx: &Context, loaded: Loaded) {
        let slot = match loaded.image {
            Some(image) => Slot::Ready(ctx.load_texture(format!("thumb-{}", loaded.id), image, TextureOptions::LINEAR)),
            None => Slot::Missing,
        };
        self.slots.insert(loaded.id, (slot, self.frame));
    }

    /// Forget a thumbnail so it is decoded again (the file behind it changed).
    pub fn forget(&mut self, id: &str) {
        self.slots.remove(id);
    }

    /// Drop the least recently shown textures once the cache is over budget.
    pub fn end_frame(&mut self) {
        if self.slots.len() <= CACHE {
            return;
        }
        let mut by_age: Vec<(u64, String)> = self.slots.iter().map(|(id, (_, used))| (*used, id.clone())).collect();
        by_age.sort_unstable();
        let excess = self.slots.len() - CACHE * 3 / 4;
        for (used, id) in by_age.into_iter().take(excess) {
            // Never evict what the current frame is drawing.
            if used < self.frame {
                self.slots.remove(&id);
            }
        }
    }
}

fn work(queue: Receiver<Job>, handle: ViewHandle) {
    while let Ok(job) = queue.recv() {
        let image = decode(&job);
        handle.send(Loaded { id: job.id, image });
    }
}

fn decode(job: &Job) -> Option<ColorImage> {
    let rgba = job.thumb.as_deref().and_then(|p| image::open(p).ok()).map(|i| i.to_rgba8()).or_else(|| fallback(job))?;
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

/// History has no usable thumbnail: derive one from the capture itself (or from a video's first frame).
fn fallback(job: &Job) -> Option<image::RgbaImage> {
    let file = job.file.as_deref()?;
    if job.video {
        return snap_record::edit::frame_at(file, 0, FALLBACK_EDGE).ok();
    }
    let full = snap_image::encode::load(file).ok()?;
    Some(snap_image::resize::thumbnail(&full, FALLBACK_EDGE))
}
