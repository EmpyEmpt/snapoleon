//! Background previews: the finished image's size and weight, a small picture of it, and one
//! thumbnail per beautify preset. They are recomputed on a worker thread shortly after the
//! document or the export settings stop changing.

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    time::{Duration, Instant},
};

use egui::{ColorImage, Context, TextureHandle, TextureOptions};
use image::RgbaImage;
use snap_annotate::{Document, RenderOptions, render};
use snap_core::settings::BeautifyPreset;
use snap_image::beautify;

use super::{
    output::{self, Estimate, ExportParams, Msg},
    session::Session,
};
use crate::views::ViewCx;

/// Longest side of the small picture of the result.
const PREVIEW_SIDE: u32 = 640;
/// Width the content of a preset thumbnail is rendered at.
const THUMB_CONTENT: f32 = 200.0;

/// One kind of background result: at most one worker at a time, started once its inputs have been still for a moment.
struct Job<K, V> {
    wanted: Option<(K, Instant)>,
    in_flight: Option<K>,
    done: Option<(K, V)>,
}

impl<K: PartialEq + Clone, V> Job<K, V> {
    fn new() -> Self {
        Self { wanted: None, in_flight: None, done: None }
    }

    /// The result, when it was made for exactly `key`.
    fn current(&self, key: &K) -> Option<&V> {
        self.done.as_ref().filter(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Make sure a result for `key` is on its way. `start` launches the worker.
    fn poll(&mut self, ctx: &Context, key: &K, delay: Duration, start: impl FnOnce()) {
        if self.current(key).is_some() || self.in_flight.as_ref() == Some(key) {
            return;
        }
        if self.in_flight.is_some() {
            // A result for older inputs is still being made; look again when it lands.
            ctx.request_repaint_after(Duration::from_millis(100));
            return;
        }
        match &self.wanted {
            Some((k, since)) if k == key => {
                let waited = since.elapsed();
                if waited >= delay {
                    self.in_flight = Some(key.clone());
                    self.wanted = None;
                    start();
                } else {
                    ctx.request_repaint_after(delay - waited);
                }
            }
            _ => {
                self.wanted = Some((key.clone(), Instant::now()));
                ctx.request_repaint_after(delay);
            }
        }
    }

    fn finish(&mut self, value: V) {
        if let Some(key) = self.in_flight.take() {
            self.done = Some((key, value));
        }
    }
}

/// What the editor currently shows, and so what is worth computing.
pub struct Wants {
    pub params: ExportParams,
    /// Also encode the image to learn its file size.
    pub bytes: bool,
    /// Make the preset thumbnails.
    pub thumbs: bool,
    /// The user is adjusting something whose preview should follow quickly.
    pub fast: bool,
}

type EstimateKey = (u64, ExportParams, bool);
/// Revision, and a stamp of the presets so that editing the list refreshes the thumbnails.
type ThumbsKey = (u64, u64);

pub struct Previews {
    estimate: Job<EstimateKey, Estimate>,
    thumbs: Job<ThumbsKey, Vec<RgbaImage>>,
    preview_texture: Option<(u64, TextureHandle)>,
    thumb_textures: Vec<TextureHandle>,
    thumb_textures_for: u64,
    generation: u64,
}

impl Previews {
    pub fn new() -> Self {
        Self {
            estimate: Job::new(),
            thumbs: Job::new(),
            preview_texture: None,
            thumb_textures: Vec::new(),
            thumb_textures_for: 0,
            generation: 0,
        }
    }

    /// The latest estimate, even when it is a little behind the document.
    pub fn latest(&self) -> Option<&Estimate> {
        self.estimate.done.as_ref().map(|(_, e)| e)
    }

    /// Whether the estimate matches the document and settings right now.
    pub fn is_current(&self, rev: u64, params: ExportParams) -> bool {
        self.estimate.done.as_ref().is_some_and(|((r, p, _), _)| *r == rev && *p == params)
    }

    /// Ask for fresh results for the current state.
    pub fn poll(&mut self, ctx: &Context, s: &Session, cx: &ViewCx, wants: Wants) {
        let Wants { params, bytes, thumbs, fast } = wants;
        let delay = Duration::from_millis(if fast { 120 } else { 450 });
        let key = (s.rev, params, bytes);
        self.estimate.poll(ctx, &key, delay, || {
            let doc = s.doc.clone();
            cx.spawn(move || {
                let image = output::render_export(&doc, &params);
                let weight = if bytes { output::encoded_len(&image, &params).ok() } else { None };
                Msg::Estimate(Estimate { size: image.dimensions(), bytes: weight, preview: output::thumbnail(&image, PREVIEW_SIDE) })
            });
        });
        if thumbs {
            let key = (s.rev, presets_stamp(&cx.settings.beautify_presets));
            self.thumbs.poll(ctx, &key, delay, || {
                let doc = s.doc.clone();
                let presets = cx.settings.beautify_presets.clone();
                cx.spawn(move || Msg::Thumbs(preset_thumbnails(&doc, &presets)));
            });
        }
    }

    pub fn on_estimate(&mut self, estimate: Estimate) {
        self.estimate.finish(estimate);
        self.generation += 1;
    }

    pub fn on_thumbs(&mut self, images: Vec<RgbaImage>) {
        self.thumbs.finish(images);
        self.generation += 1;
    }

    /// The small picture of the finished image.
    pub fn preview_texture(&mut self, ctx: &Context) -> Option<&TextureHandle> {
        let (_, estimate) = self.estimate.done.as_ref()?;
        if self.preview_texture.as_ref().is_none_or(|(g, _)| *g != self.generation) {
            self.preview_texture = Some((self.generation, texture(ctx, "editor-preview", &estimate.preview)));
        }
        self.preview_texture.as_ref().map(|(_, t)| t)
    }

    /// Thumbnails of the presets, as textures, in the presets' order.
    pub fn thumb_textures(&mut self, ctx: &Context) -> &[TextureHandle] {
        if self.thumb_textures_for != self.generation
            && let Some((_, images)) = &self.thumbs.done
        {
            self.thumb_textures = images.iter().enumerate().map(|(i, image)| texture(ctx, &format!("editor-thumb-{i}"), image)).collect();
            self.thumb_textures_for = self.generation;
        }
        &self.thumb_textures
    }
}

fn presets_stamp(presets: &[BeautifyPreset]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for p in presets {
        (&p.name, p.padding, p.corner_radius, p.shadow, p.frame as u8).hash(&mut hasher);
    }
    hasher.finish()
}

pub fn texture(ctx: &Context, name: &str, image: &RgbaImage) -> TextureHandle {
    let size = [image.width() as usize, image.height() as usize];
    ctx.load_texture(name, ColorImage::from_rgba_unmultiplied(size, image.as_raw()), TextureOptions::LINEAR)
}

/// The document without its own beautify, dressed in each preset at thumbnail size.
fn preset_thumbnails(doc: &Document, presets: &[BeautifyPreset]) -> Vec<RgbaImage> {
    let mut plain = doc.clone();
    plain.beautify = None;
    let crop = plain.crop_rect();
    let scale = (THUMB_CONTENT / crop.w.max(1) as f32).min(1.0);
    let small = render(&plain, &RenderOptions { scale, readable: true, beautify: false });
    presets
        .iter()
        .map(|preset| {
            let mut p = preset.clone();
            let k = |v: u32| (v as f32 * scale).round() as u32;
            (p.padding, p.corner_radius, p.shadow) = (k(p.padding), k(p.corner_radius), k(p.shadow));
            beautify::apply_with(&small, &p, &doc.meta.context)
        })
        .collect()
}
