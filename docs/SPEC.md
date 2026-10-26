# Snapoleon — product & architecture spec

Snapoleon is a clean-room, open-source, all-Rust screenshot / recording / OCR /
sharing tool for Windows, macOS and Linux. It reaches feature parity with AShot
(a-shot.ru) and covers the gaps against ShareX, CleanShot X, Snagit, Shottr,
Flameshot, Snipping Tool and Lightshot.

**Product shape:** Snipping-Tool simplicity in the default path (one hotkey →
select → instant toolbar), ShareX depth behind progressive disclosure
(pipelines, destinations, CLI, MCP), and things neither has (element capture,
readable downscaling, self-hosted links, local PII formats, context-searchable
history).

**Hard rules**
- 100% Rust. No webview, no Electron, no JS runtime in the client. GUI = egui
  on our own winit + OpenGL/wgpu shell. The team server renders plain server-side HTML.
- Lean: no tokio in the client. Blocking I/O on worker threads (`std::thread`,
  `crossbeam-channel`). HTTP = `ureq` + rustls. Heavy/rare features behind cargo
  features or downloaded on demand (OCR fallback models).
- Clean-room: we never look at AShot's source. Features come from public
  descriptions only.
- Every user-visible string goes through `snap_core::t("area.key")`; add keys in
  `locales/en/<area>.toml` **and** `locales/ru/<area>.toml` (each crate/area owns
  its own files; never edit another area's locale file).
- Platform code lives in `src/platform/{windows,macos,linux}.rs` (or similar)
  behind `#[cfg(target_os = ...)]`; every public function compiles on all three
  OSes, returning `Error::Unsupported` where a backend is missing.
- No unit tests. Verify with `cargo check`, `cargo clippy`, and small runnable
  examples (`examples/*.rs`) where it helps.

## Workspace

| crate | role |
|---|---|
| `snap-core` | shared types: geometry, `Capture`, `Settings`, pipelines, hotkey actions, i18n, IPC messages, naming templates, policy, `SecretStore` (DONE — read it) |
| `snap-capture` | monitors/windows/elements, screen/window/region capture, HDR tone-map, frozen snapshots |
| `snap-image` | pure image ops: readable downscale, pixelate/blur/solid, beautify, combine, stitch, diff, encode/decode, target-size export, color formats |
| `snap-annotate` | non-destructive annotation document, tiny-skia rasterizer, hit-testing, undo, `.snap` project files |
| `snap-ocr` | OCR (Windows.Media.Ocr, Apple Vision, tesseract CLI, `ocrs` fallback), tables/markdown, QR/barcode decode + QR encode |
| `snap-redact` | PII/secret detection with checksums, mapping to word boxes |
| `snap-upload` | destinations, OAuth, ShareX `.sxcu` import, shorteners, link formatting, team-server client |
| `snap-history` | SQLite+FTS5 library, thumbnails, retention |
| `snap-platform` | global hotkeys, tray, clipboard, notifications, single-instance IPC, autostart, keyring secrets, URL scheme, policy, permissions, open/reveal |
| `snap-record` | screen recording (MP4 HW/SW, GIF, WebP), audio, cursor/click/keystroke overlays, trim, export-to-size, slideshow video |
| `snap-ai` | OpenAI-compatible client (local Ollama/LM Studio by default): alt text, titles, how-to, translate, smart redaction |
| `snap-ui` | design system for egui: tokens, fonts, icons, widgets, toasts, custom title bar |
| `snap-shell` | multi-window winit + egui runtime (OpenGL or wgpu) that keeps running with zero windows (tray app) |
| `snap-engine` | headless orchestration used by GUI, CLI and MCP: capture requests, pipeline executor, scrolling capture, step capture, export |
| `apps/snapoleon` | the binary: GUI (tray, overlay, editor, pins, recorder HUD, library, settings, tools), CLI subcommands, `mcp` stdio server |
| `server/snapoleon-server` | self-hosted team server (axum): uploads, short links w/ expiry/password/delete, view counts, OG previews, workspaces, tokens, OIDC SSO, retention |

Shared fonts are in `assets/fonts/` (Inter Regular/SemiBold/Bold, JetBrains Mono
Medium; subset to Latin+Cyrillic+symbols by `tools/subset_fonts.py` from the full fonts in `assets/fonts/src/`).
They are embedded once, in `snap_core::fonts`: use those statics, never a new `include_bytes!`.

Coordinate conventions (see `snap_core::geom`): integer `Rect`/`Point` are
**physical pixels in virtual-desktop space**; `RectF`/`PointF` are image-pixel
space in documents. Images are straight-alpha sRGB `RgbaImage`. macOS lays displays
out in points; `snap_core::geom::PointLayout` turns that into the pixel space (origins
scaled by the largest display scale, so mixed-DPI displays never overlap), and every
crate converts through it.

---

## snap-capture

```rust
pub struct MonitorInfo { pub index: usize, pub id: u32, pub name: String, pub rect: Rect, pub work_area: Rect,
                         pub scale_factor: f32, pub is_primary: bool, pub hdr: bool, pub refresh_hz: f32 }
pub struct WindowInfo { pub id: u64, pub pid: u32, pub title: String, pub app_name: String, pub app_id: Option<String>,
                        pub rect: Rect /* visible frame, no invisible resize borders (DWMWA_EXTENDED_FRAME_BOUNDS) */,
                        pub z: u32 /* 0 = front-most */, pub minimized: bool }
pub struct ElementInfo { pub rect: Rect, pub name: String, pub role: String, pub app_name: Option<String>, pub window_id: Option<u64> }
pub struct CaptureOptions { pub include_cursor: bool, pub hdr: HdrMode, pub window_shadow: bool,
                            pub hide_desktop_icons: bool /* macOS: leave Finder's icons out of the capture */ }  // Default impl

pub fn monitors() -> Result<Vec<MonitorInfo>>;
pub fn windows() -> Result<Vec<WindowInfo>>;            // visible top-level, front→back, excludes our own pid & shell/desktop windows
pub fn foreground_window() -> Option<WindowInfo>;
pub fn cursor_position() -> Point;
pub fn capture_monitor(index: usize, o: &CaptureOptions) -> Result<Capture>;
pub fn capture_all(o: &CaptureOptions) -> Result<Capture>;            // composite of all monitors, physical px
pub fn capture_rect(r: Rect, o: &CaptureOptions) -> Result<Capture>;  // may span monitors (mixed DPI correct)
pub fn capture_rect_pixels(r: Rect, o: &CaptureOptions) -> Result<RgbaImage>; // pixels only, no context (scroll frames)
pub fn capture_window(id: u64, o: &CaptureOptions) -> Result<Capture>;// works when occluded where the OS allows (WGC / ScreenCaptureKit / XComposite)

/// Instant snapshot for the selection overlay: every monitor image + window list + cursor.
pub struct Frozen { pub monitors: Vec<(MonitorInfo, RgbaImage)>, pub windows: Vec<WindowInfo>, pub cursor: Point, pub taken_at_ms: i64 }
pub fn freeze(o: &CaptureOptions) -> Result<Frozen>;
impl Frozen {
    pub fn bounds(&self) -> Rect;
    pub fn crop(&self, r: Rect) -> RgbaImage;              // spans monitors
    pub fn pixel(&self, p: Point) -> Option<[u8; 4]>;
    pub fn window_at(&self, p: Point) -> Option<&WindowInfo>;
    pub fn to_capture(&self, r: Rect, kind: CaptureKind) -> Capture;
}

/// Accessibility element under a point (UIA on Windows — Chromium, Electron, Firefox, UWP, Win32;
/// AXUIElement on macOS; AT-SPI on Linux). Returns the ancestor chain smallest→largest so the
/// overlay can cycle parents with the scroll wheel.
pub fn elements_at(p: Point) -> Result<Vec<ElementInfo>>;
pub fn context_for_window(w: &WindowInfo) -> CaptureContext;  // app name, title, browser URL via accessibility where possible
pub fn context_at(p: Point) -> CaptureContext;

/// Hides desktop icons while alive (Windows: SHELLDLL_DefView list view). `None` on macOS, where
/// captures leave Finder's icon window out (`CaptureOptions::hide_desktop_icons`), and on Linux.
pub fn hide_desktop_icons() -> Option<DesktopIconsGuard>;
/// macOS: a system prompt (screen capture approval, Gatekeeper, password) is on screen; overlays must not cover it.
pub fn system_alert_on_screen() -> bool;
/// macOS: throwaway tiny capture right after Screen Recording is granted, so the "bypass the private
/// window picker" prompt appears while nothing covers it. No-op elsewhere.
pub fn prime_capture_consent();
/// Simulate wheel scroll over a point (for scrolling capture).
pub fn scroll_at(p: Point, notches: i32) -> Result<()>;
```

Backends: Windows → Windows.Graphics.Capture (`windows-capture` or direct
`windows` crate) for window capture and HDR (capture as FP16 when the monitor is
HDR and tone-map to SDR using the monitor's SDR white level; otherwise BGRA8),
DXGI desktop duplication or GDI BitBlt for fast monitor snapshots; per-monitor
DPI awareness v2 must be set (call `snap_platform::dpi::init()` early; capture
crate must also work if the process is already DPI-aware). macOS →
ScreenCaptureKit screenshots (macOS 14+; `CGWindowListCreateImage` on 12.3 and 13),
converted to sRGB, with the CGWindowList for window enumeration; Linux → `xcap`
(X11 + Wayland portal).
UIA calls run on a dedicated COM (MTA) worker thread inside the crate.

## snap-image

```rust
pub mod resize {
    pub fn resize(img: &RgbaImage, w: u32, h: u32) -> RgbaImage;           // Lanczos3, alpha-correct, fast (fast_image_resize ok)
    pub fn readable_downscale(img: &RgbaImage, w: u32, h: u32) -> RgbaImage; // keeps thin text/lines legible: scale-dependent
                                                                             // min/max morphological pre-filter on luminance edges, then
                                                                             // resample, then mild unsharp; no-op path for scale ≥ 0.8
    pub fn fit(img: &RgbaImage, max_w: Option<u32>, max_h: Option<u32>, readable: bool) -> RgbaImage;
    pub fn thumbnail(img: &RgbaImage, max_side: u32) -> RgbaImage;
}
pub mod effects {
    pub fn pixelate(img: &mut RgbaImage, r: Rect, block: u32);              // r in image px; clamps
    pub fn blur(img: &mut RgbaImage, r: Rect, radius: f32);                 // gaussian (box×3)
    pub fn fill(img: &mut RgbaImage, r: Rect, rgba: [u8; 4]);
    pub fn redact(img: &mut RgbaImage, r: Rect, style: RedactStyle);       // Pixelate is irreversible: block ≥ max(12, h/2) + noise
    pub fn gaussian_blur(img: &RgbaImage, sigma: f32) -> RgbaImage;
    pub fn dim_except(img: &mut RgbaImage, keep: &[Rect], amount: f32);     // spotlight
    pub fn drop_shadow(img: &RgbaImage, radius: u32, offset: (i32, i32), rgba: [u8; 4]) -> RgbaImage; // grows canvas
    pub fn round_corners(img: &mut RgbaImage, radius: u32);                 // anti-aliased alpha mask
}
pub mod beautify { pub fn apply(img: &RgbaImage, p: &BeautifyPreset) -> RgbaImage; pub fn draw_frame(...) }
// frames drawn procedurally (mac window traffic lights light/dark, browser chrome w/ address pill, phone bezel)
pub mod combine {
    pub enum Layout { Horizontal, Vertical, Grid { cols: u32 } }
    pub enum Align { Start, Center, End }
    pub fn combine(imgs: &[RgbaImage], layout: Layout, gap: u32, align: Align, bg: [u8; 4]) -> RgbaImage;
}
pub mod stitch {          // scrolling capture: vertical overlap detection, robust to sticky headers/footers & scrollbars
    pub struct Stitcher;  impl Stitcher {
        pub fn new() -> Self; pub fn push(&mut self, frame: &RgbaImage) -> PushOutcome; pub fn finish(self) -> RgbaImage;
        pub fn preview(&self, max_h: u32) -> RgbaImage; pub fn height(&self) -> u32; }
    pub enum PushOutcome { Appended { rows: u32 }, NoChange /* reached end */, NoOverlap /* appended blindly */ }
}
pub mod diff {
    pub enum DiffMode { Highlight, Overlay { opacity: f32 }, SideBySide, Difference }
    pub struct DiffStats { pub changed_pixels: u64, pub changed_ratio: f32, pub bbox: Option<Rect> }
    pub fn diff(a: &RgbaImage, b: &RgbaImage, mode: DiffMode, threshold: u8) -> (RgbaImage, DiffStats);
}
pub mod encode {
    pub fn encode(img: &RgbaImage, fmt: ImageFormat, quality: u8, optimize_png: bool) -> Result<Vec<u8>>; // never writes metadata
    pub fn encode_to_size(img: &RgbaImage, fmt: ImageFormat, max_bytes: usize) -> Result<(Vec<u8>, SizeFit { scale: f32, quality: u8 })>;
    pub fn decode(bytes: &[u8]) -> Result<RgbaImage>;     // honours EXIF orientation, drops metadata
    pub fn load(path: &Path) -> Result<RgbaImage>;
    pub fn save(img: &RgbaImage, path: &Path, fmt: ImageFormat, quality: u8, optimize_png: bool) -> Result<u64>; // returns bytes written
    pub fn format_from_path(path: &Path) -> Option<ImageFormat>;
}
pub mod color {
    pub fn hex(c: [u8; 4]) -> String; pub fn rgb(c) -> String; pub fn hsl(c) -> String; pub fn hsv(c) -> String; pub fn oklch(c) -> String;
    pub fn contrast_ratio(a: [u8; 4], b: [u8; 4]) -> f32;  // WCAG 2.x
    pub fn average(img: &RgbaImage, r: Rect) -> [u8; 4];
}
```
WebP lossy via the `webp` crate; PNG optimisation via `oxipng` (feature `oxipng`, default on).

## snap-annotate

```rust
pub struct Document { pub base: Arc<RgbaImage>, pub scale_factor: f32, pub crop: Option<RectF>, pub shapes: Vec<Shape>,
                      pub beautify: Option<BeautifyPreset>, pub meta: DocMeta }
pub struct DocMeta { pub capture_id: Option<CaptureId>, pub context: CaptureContext, pub created_ms: i64 }
pub struct Shape { pub id: u64, pub kind: ShapeKind, pub style: Style, pub hidden: bool }
pub struct Style { pub stroke: [u8; 4], pub fill: Option<[u8; 4]>, pub width: f32, pub opacity: f32, pub shadow: bool, pub dashed: bool }
pub enum ArrowHead { Triangle, Open, None }
pub enum ShapeKind {
    Arrow { from: PointF, to: PointF, ctrl: Option<PointF> /* curved */, head: ArrowHead, double: bool },
    Line { from: PointF, to: PointF },
    Rect { rect: RectF, radius: f32 },
    Ellipse { rect: RectF },
    Pen { points: Vec<PointF> },                      // smoothed (Catmull-Rom) on render
    Highlighter { points: Vec<PointF> },              // multiply blend, wide, translucent
    Text { pos: PointF, text: String, size: f32, bold: bool, bg: TextBg /* None|Solid|Bubble */, max_width: Option<f32> },
    Callout { rect: RectF, tail: PointF, text: String, size: f32 },
    Step { center: PointF, number: u32, radius: f32 },// numbered badge, auto-increment
    Redact { rect: RectF, style: RedactStyle },       // destructive at export
    Spotlight { rect: RectF, ellipse: bool },         // dims everything else (all spotlights combine)
    Magnifier { source: RectF, center: PointF, radius: f32 }, // zoomed circular callout of `source`
    Image { rect: RectF, image: Arc<RgbaImage> },     // pasted image / logo
    Stamp { center: PointF, size: f32, glyph: Stamp /* Check, Cross, Question, Warning, Star, Heart, ThumbsUp, Info */ },
    Measure { from: PointF, to: PointF },             // shows length in px
}
impl Document {
    pub fn new(base: RgbaImage, scale_factor: f32) -> Self;  pub fn from_capture(c: &Capture) -> Self;
    pub fn canvas_size(&self) -> (u32, u32);  pub fn next_step_number(&self) -> u32;  pub fn add(&mut self, kind: ShapeKind, style: Style) -> u64;
    pub fn get_mut(&mut self, id: u64) -> Option<&mut Shape>;  pub fn remove(&mut self, id: u64);  pub fn bring_to_front/send_to_back(id);
}
pub struct RenderOptions { pub scale: f32, pub readable: bool, pub beautify: bool }
pub fn render(doc: &Document, o: &RenderOptions) -> RgbaImage;          // export; crop applied; readable = stroke/text minimums when scale < 1
pub struct View { pub offset: PointF, pub zoom: f32, pub size: (u32, u32) } // doc px → view px: (p - offset) * zoom
pub fn render_view(doc: &Document, view: &View, base_too: bool) -> tiny_skia::Pixmap; // live editor preview at view resolution
pub fn hit_test(doc: &Document, p: PointF, tol: f32) -> Option<u64>;
pub enum Handle { Body, Start, End, Ctrl, Corner(u8), Edge(u8), Tail }
pub fn handles(shape: &Shape) -> Vec<(Handle, PointF)>;  pub fn drag(shape: &mut Shape, h: Handle, delta: PointF);  pub fn bounds(shape: &Shape) -> RectF;
pub struct Undo { /* snapshot stack of Vec<Shape> + crop; cheap because base is Arc */ }
impl Undo { pub fn checkpoint(&mut self, doc: &Document); pub fn undo(&mut self, doc: &mut Document) -> bool; pub fn redo(&mut self, doc: &mut Document) -> bool; }
pub fn save_project(doc: &Document, path: &Path) -> Result<()>;   // `.snap` = zip { doc.json, base.png, assets/<n>.png }
pub fn load_project(path: &Path) -> Result<Document>;
pub fn text_layout(text: &str, size: f32, bold: bool, max_w: Option<f32>) -> (f32, f32); // measured size, for editor caret/box
```
Text is rendered with `ab_glyph` outlines → tiny-skia paths using the shared Inter fonts.
Readable downscale: when `scale < 1`, stroke width becomes `max(w*scale, w*scale.sqrt()*0.75, 1.5)`, text
size `max(size*scale, 11.0)`, step badge radius `max(r*scale, 9.0)`, and the base image uses
`snap_image::resize::readable_downscale`.

## snap-ocr

```rust
pub struct OcrWord { pub text: String, pub rect: RectF, pub confidence: f32 }
pub struct OcrLine { pub text: String, pub rect: RectF, pub words: Vec<OcrWord> }
pub struct OcrResult { pub lines: Vec<OcrLine>, pub language: Option<String>, pub engine: String }
impl OcrResult { pub fn text(&self) -> String; /* paragraphs by vertical gaps */ pub fn to_markdown_table(&self) -> Option<String>;
                 pub fn to_tsv(&self) -> Option<String>; pub fn is_empty(&self) -> bool; }
pub fn recognize(img: &RgbaImage, s: &OcrSettings) -> Result<OcrResult>;     // blocking; upscales small text ×2 for accuracy
pub fn engines() -> Vec<EngineStatus { engine: OcrEngine, available: bool, note: String }>;
pub fn native_languages() -> Vec<String>;
pub mod codes {
    pub struct Code { pub format: String, pub text: String, pub rect: Option<RectF> }
    pub fn decode(img: &RgbaImage) -> Vec<Code>;                   // QR, DataMatrix, Aztec, PDF417, EAN/UPC, Code128/39… (rxing)
    pub fn qr(text: &str, module_px: u32) -> RgbaImage;            // QR generation (share link to phone)
}
```
Engines: Windows `Windows.Media.Ocr` (WinRT via `windows` crate; pick a language
pack per settings/user profile, run on an MTA thread); macOS Vision
`VNRecognizeTextRequest` (accurate, language correction, ru+en); Linux/any:
`tesseract` CLI with TSV output if on PATH or configured; last resort `ocrs`
(feature `ocrs`, models downloaded on first use to `paths::models_dir()`).

## snap-redact

```rust
pub struct Match { pub kind: RedactKind, pub start: usize, pub end: usize, pub text: String }   // byte offsets
pub fn find(text: &str, kinds: &[RedactKind]) -> Vec<Match>;
pub struct LineWords { pub words: Vec<(String, RectF)> }
pub struct Hit { pub kind: RedactKind, pub rect: RectF, pub text: String }
pub fn locate(lines: &[LineWords], kinds: &[RedactKind]) -> Vec<Hit>;  // joins words with spaces, maps matches back to union of word boxes (inflated 2px)
pub fn locate_substrings(lines: &[LineWords], needles: &[String]) -> Vec<Hit>;  // for AI-suggested redactions
```
Validators: Luhn (cards, 13–19 digits, IIN prefixes), IBAN mod-97, ИНН (10/12-digit checksums), СНИЛС
checksum, RU passport `NN NN NNNNNN` near context words, +7/8 RU phones and E.164/intl/US formats,
emails, IPv4/IPv6, US SSN, secrets (AWS `AKIA…`, GitHub `gh[pousr]_…`, GitLab `glpat-`, Slack `xox[abpr]-`,
Stripe `sk_live_`, OpenAI `sk-…`, Anthropic `sk-ant-…`, Google `AIza…`, JWT `eyJ…`, PEM private-key
headers, `password=`/`token:` key-value pairs, generic high-entropy ≥ 24 chars), URLs with credentials or
token query params. OCR-tolerant: allow common confusions (O/0, l/1, spaces inside digit groups).

## snap-upload

```rust
pub struct UploadRequest<'a> { pub data: &'a [u8], pub file_name: &'a str, pub mime: &'a str, pub title: Option<&'a str>,
                               pub expiry_hours: Option<u32>, pub password: Option<&'a str> }
pub struct UploadResult { pub url: String, pub thumbnail_url: Option<String>, pub delete_url: Option<String>, pub provider: String }
pub trait Uploader: Send + Sync {
    fn upload(&self, req: &UploadRequest, progress: &dyn Fn(u64, u64)) -> Result<UploadResult>;
    fn test(&self) -> Result<()>;
}
pub fn uploader_for(d: &Destination, secrets: &dyn SecretStore) -> Result<Box<dyn Uploader>>;
pub fn shorten(url: &str, s: &Shortener, server: Option<&str>, secrets: &dyn SecretStore) -> Result<String>;
pub fn format_link(url: &str, f: LinkFormat, title: Option<&str>, image: bool) -> String; // Markdown ![t](u) / <img> / [img]u[/img]
pub mod presets { pub struct S3Preset { pub name: &'static str, pub endpoint: &'static str, pub region: &'static str, pub path_style: bool }
                  pub fn s3() -> &'static [S3Preset]; }   // AWS, Cloudflare R2, Yandex Object Storage, Selectel, VK Cloud, MinIO, Backblaze B2, Wasabi, DigitalOcean Spaces
pub mod sxcu { pub fn import(json: &str) -> Result<(String, CustomUploader)>; pub fn export(name: &str, c: &CustomUploader) -> String; }
pub mod oauth { pub enum Provider { Box, Dropbox, GoogleDrive, OneDrive, YandexDisk }
                pub fn authorize(p: Provider, client_id: &str, client_secret: Option<&str>) -> Result<Tokens>;  // PKCE + loopback 127.0.0.1, opens browser
                pub fn refresh(p: Provider, client_id: &str, refresh_token: &str) -> Result<Tokens>; }
pub mod server {  // snapoleon-server client (team mode)
    pub struct Client { /* base url + token */ }
    impl Client { pub fn new(url: &str, token: &str) -> Self; pub fn me(&self) -> Result<Me>; pub fn list(&self, page: u32) -> Result<Vec<RemoteItem>>;
                  pub fn delete(&self, id: &str) -> Result<()>; pub fn events(&self, since_ms: i64) -> Result<Vec<RemoteEvent /* view, comment */>>; }
}
```
S3 uses our own SigV4 (hmac + sha2), PUT with `Content-Type`, optional `x-amz-acl`, random object names of
`random_code_len` chars, URLs built from `public_base_url` (own domain) when set. Secrets are fetched from
the `SecretStore` by `secret_ref` (S3 secret key, FTP/WebDAV password, tokens, OAuth refresh tokens as JSON).
Telegram uses Bot API `sendPhoto`/`sendDocument` (returns `t.me` link for public channels, else message
link if derivable, else file id note). Yandex Disk uses REST upload + `publish` for a public link.

## snap-history

```rust
pub struct LinkRecord { pub url: String, pub destination: String, pub delete_url: Option<String>, pub created_ms: i64 }
pub struct Entry { pub id: CaptureId, pub kind: CaptureKind, pub file: Option<PathBuf>, pub project: Option<PathBuf>, pub thumb: Option<PathBuf>,
                   pub width: u32, pub height: u32, pub bytes: u64, pub created_ms: i64, pub context: CaptureContext,
                   pub text: Option<String>, pub links: Vec<LinkRecord>, pub tags: Vec<String>, pub favorite: bool, pub duration_ms: Option<u64> }
pub struct Query { pub text: String, pub kind: Option<CaptureKind>, pub app: Option<String>, pub from_ms: Option<i64>, pub to_ms: Option<i64>,
                   pub favorites: bool, pub limit: u32, pub offset: u32 }
pub struct History;  // Send + Sync (Mutex<Connection>)
impl History {
    pub fn open(path: &Path) -> Result<Self>;  pub fn open_default() -> Result<Self>;
    pub fn add(&self, e: &Entry) -> Result<()>;  pub fn get(&self, id: &CaptureId) -> Result<Option<Entry>>;
    pub fn set_text(&self, id: &CaptureId, text: &str) -> Result<()>;  pub fn add_link(&self, id: &CaptureId, l: &LinkRecord) -> Result<()>;
    pub fn set_tags(&self, id, tags: &[String]) -> Result<()>;  pub fn set_favorite(&self, id, fav: bool) -> Result<()>;
    pub fn set_file(&self, id, file: Option<&Path>, project: Option<&Path>) -> Result<()>;
    pub fn delete(&self, id, delete_files: bool) -> Result<()>;
    pub fn search(&self, q: &Query) -> Result<Vec<Entry>>;   // FTS5 over text + title + app + url + tags, prefix matching, ranked; empty text = recent
    pub fn apps(&self) -> Result<Vec<(String, u32)>>;  pub fn count(&self) -> Result<u64>;
    pub fn purge(&self, older_than_days: u32) -> Result<u32>;
}
pub fn write_thumbnail(img: &RgbaImage, id: &CaptureId) -> Result<PathBuf>;   // 320px webp/png in paths::thumbnails_dir()
```

## snap-platform

```rust
pub mod dpi { pub fn init(); }                                    // per-monitor-v2 on Windows; no-op elsewhere
pub mod hotkeys {
    pub struct HotkeyManager;  pub enum HotkeyEvent { Pressed(usize /* binding index */), Released(usize) }
    impl HotkeyManager { pub fn new(on_event: impl Fn(HotkeyEvent) + Send + 'static) -> Result<Self>;
        pub fn register(&mut self, b: &[HotkeyBinding]) -> Vec<(usize, Result<()>)>;   // conflicts reported per binding
        pub fn unregister_all(&mut self); pub fn pause(&mut self, paused: bool); }       // pause while the hotkey recorder is focused
    pub fn validate(accel: &str) -> Result<()>;  pub fn display(accel: &str) -> String;   // "⌘⇧4" on mac, "Ctrl+Shift+4" elsewhere
    pub fn from_egui(mods: egui::Modifiers, key: egui::Key) -> Option<String>; // feature "egui"
}
pub mod tray {
    pub struct TrayItem { pub id: String, pub label: String, pub enabled: bool, pub checked: Option<bool>, pub children: Vec<TrayItem>, pub separator: bool }
    pub enum TrayEvent { Click, DoubleClick, Menu(String) }
    pub struct Tray; impl Tray { pub fn new(tooltip: &str, icon: &RgbaImage, items: Vec<TrayItem>, on_event: impl Fn(TrayEvent) + Send + Sync + 'static) -> Result<Self>;
        pub fn set_items(&mut self, items: Vec<TrayItem>); pub fn set_icon(&mut self, icon: &RgbaImage); pub fn set_tooltip(&mut self, t: &str); }
}   // tray-icon/muda on Windows+macOS (create on the main thread after the event loop starts), ksni (no GTK) on Linux
pub mod clipboard { pub fn set_image(img: &RgbaImage) -> Result<()>; pub fn set_text(t: &str) -> Result<()>; pub fn set_html(html: &str, alt: &str) -> Result<()>;
                    pub fn set_files(paths: &[PathBuf]) -> Result<()>; pub fn get_image() -> Option<RgbaImage>; pub fn get_text() -> Option<String>; }
pub mod notify { pub fn show(title: &str, body: &str, image: Option<&Path>) -> Result<()>; }  // WinRT toast w/ AUMID, NSUserNotification/osascript, notify-rust
pub mod ipc { pub struct Server; pub fn serve(handler: impl Fn(Request) -> Response + Send + Sync + 'static) -> Result<Server>;  // Err if another instance owns it
              pub fn send(req: &Request) -> Result<Response>; pub fn instance_running() -> bool; }  // named pipe \\.\pipe\snapoleon-<user> / unix socket in runtime dir
pub mod autostart { pub fn set(enabled: bool) -> Result<()>; pub fn get() -> bool; }
pub mod secrets { pub struct KeyringStore; impl SecretStore for KeyringStore {} pub fn store() -> Arc<dyn SecretStore>; }
pub mod scheme { pub fn register() -> Result<()>; }               // snapoleon:// + .snap association, per-user (HKCU / ~/.local/share/applications); macOS via Info.plist
pub mod policy { pub fn load(); }                                 // reads HKLM/HKCU\Software\Policies\Snapoleon or managed plist, sets snap_core::policy::PLATFORM_POLICY
pub mod permissions { pub enum Perm { ScreenRecording, Accessibility, InputMonitoring, Microphone }
                      pub fn granted(p: Perm) -> bool; pub fn request(p: Perm); pub fn open_settings(p: Perm); }  // real on macOS, true elsewhere
pub mod open { pub fn path(p: &Path) -> Result<()>; pub fn url(u: &str) -> Result<()>; pub fn reveal(p: &Path) -> Result<()>; }
pub mod console { pub fn attach(); }                              // Windows GUI-subsystem binary used as CLI: AttachConsole(ATTACH_PARENT_PROCESS)
pub mod sound { pub fn shutter(); }                               // tiny embedded click; non-blocking; silent fallback
```

## snap-record

```rust
pub enum RecordTarget { Region(Rect), Monitor(usize), Window(u64) }
pub struct RecordOptions { pub target: RecordTarget, pub format: VideoFormat, pub fps: u32, pub bitrate: u32, pub system_audio: bool,
    pub microphone: bool, pub mic_device: Option<String>, pub show_cursor: bool, pub highlight_clicks: bool, pub show_keystrokes: bool,
    pub encoder: EncoderPref, pub max_gif_width: u32, pub output: PathBuf, pub ffmpeg: Option<PathBuf>, pub exclude_windows: Vec<u64> }
pub struct Recorder; pub fn start(o: RecordOptions) -> Result<Recorder>;
impl Recorder { pub fn pause(&self); pub fn resume(&self); pub fn is_paused(&self) -> bool; pub fn elapsed_ms(&self) -> u64;
                pub fn audio_level(&self) -> f32; pub fn stop(self) -> Result<Recording>; pub fn cancel(self); }
pub struct Recording { pub path: PathBuf, pub duration_ms: u64, pub width: u32, pub height: u32, pub bytes: u64, pub format: VideoFormat }
pub fn encoders() -> Vec<EncoderInfo { pub name: String, pub hardware: bool }>;  pub fn mic_devices() -> Vec<String>;
pub mod edit {
    pub struct VideoInfo { pub duration_ms: u64, pub width: u32, pub height: u32, pub fps: f32, pub has_audio: bool }
    pub fn probe(p: &Path) -> Result<VideoInfo>;  pub fn frame_at(p: &Path, ms: u64, max_w: u32) -> Result<RgbaImage>;
    pub fn trim(input: &Path, output: &Path, start_ms: u64, end_ms: u64) -> Result<()>;
    pub fn export(input: &Path, output: &Path, o: &ExportOptions { format: VideoFormat, max_bytes: Option<u64>, width: Option<u32>, fps: Option<u32>, range: Option<(u64, u64)>, mute: bool }) -> Result<u64>;
}
pub mod slideshow { pub fn render(slides: &[(RgbaImage, u64 /* ms */)], output: &Path, fps: u32) -> Result<()>; }  // step-capture video export
```
Backends: Windows — WGC frames + Media Foundation SinkWriter (H.264 HW via MFT when
available, software MFT otherwise; AAC audio), WASAPI loopback (system) + capture
(mic) mixed; `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` on our HUD is the
app's job, but `exclude_windows` lets capture skip them where supported. macOS —
ScreenCaptureKit (video + system audio, mic on 15+) → AVAssetWriter (VideoToolbox
HW). Linux — ffmpeg subprocess (x11grab / PipeWire via portal + pulse monitor).
Any platform: when a native path is unavailable or for edit ops, use ffmpeg if
found (`settings.recording.ffmpeg_path`, PATH, or next to the exe). GIF: our own
encoder (`gif` + `color_quant` NeuQuant, per-frame diff rectangles, transparent
unchanged pixels). Animated WebP: `webp-animation` or equivalent. Overlays
(click ripples, keystroke HUD, cursor highlight) are composited onto frames with
tiny-skia from a global input listener (`rdev` or platform hooks).

## snap-ai

```rust
pub struct Ai; impl Ai {
    pub fn new(s: &AiSettings, secrets: &dyn SecretStore) -> Self;
    pub fn available(&self) -> bool;  pub fn test(&self) -> Result<String /* model list / ok */>;
    pub fn alt_text(&self, img: &RgbaImage, cx: &CaptureContext) -> Result<String>;
    pub fn title(&self, img: Option<&RgbaImage>, text: Option<&str>, cx: &CaptureContext) -> Result<String>;  // ≤ 8 words, for file names / links
    pub fn howto(&self, steps: &[HowToStep { caption: String, text: Option<String>, image: Option<RgbaImage> }]) -> Result<HowTo { title: String, intro: String, steps: Vec<String> }>;
    pub fn suggest_redactions(&self, text: &str) -> Result<Vec<String>>;
    pub fn translate(&self, text: &str, target: &str) -> Result<String>;
}
pub fn translate(text: &str, target: &str, t: &TranslateSettings, ai: &AiSettings, secrets: &dyn SecretStore) -> Result<String>; // LibreTranslate / DeepL / AI
```
OpenAI-compatible `/chat/completions` (images as `data:` URLs, JPEG ≤ 1568 px),
works with Ollama, LM Studio, llama.cpp server, vLLM, OpenAI, OpenRouter.
Anthropic Messages API supported when endpoint host is `api.anthropic.com`.

## snap-ui (design system — see docs/DESIGN.md)

```rust
pub fn install(ctx: &egui::Context, theme: &ThemeSpec);           // fonts (Inter, JetBrains Mono, Phosphor), visuals, spacing, text styles
pub struct ThemeSpec { pub dark: bool, pub accent: egui::Color32 } impl ThemeSpec { pub fn from_settings(g: &General) -> Self; }
pub fn palette(ctx: &egui::Context) -> Palette;                    // semantic colors for the active theme
pub use egui_phosphor::regular as icon;
pub mod widgets {  // all return egui::Response
    icon_button, tool_button(selected), primary_button, secondary_button, ghost_button, danger_button,
    toggle(&mut bool), segmented(&mut T, &[(T, &str)]), chip, kbd(accel), badge,
    card(ui, |ui|), section(ui, title, |ui|), setting_row(ui, label, desc, |ui| control), nav_item(icon, label, selected),
    search_field(&mut String, hint), text_field, number_field, slider, dropdown(id, &mut T, &[(T, &str)]),
    color_swatches(&mut [u8;4], palette: &[[u8;4]]), stroke_picker(&mut f32), hotkey_recorder(&mut String, &mut RecorderState),
    empty_state(icon, title, body), spinner, progress(f32), title_bar(ui, title, &mut TitleBarActions) /* custom chrome, drag, min/max/close */,
    floating_toolbar(ui/ctx, id, pos, |ui|) /* pill with shadow used by overlay + editor */, tooltip helpers
}
pub struct Toasts; impl Toasts { pub fn push(&mut self, kind: ToastKind, text: impl Into<String>); pub fn show(&mut self, ctx: &egui::Context); }
pub fn logo(painter: &egui::Painter, rect: egui::Rect);            // vector brand mark
pub fn app_icon_rgba(size: u32) -> RgbaImage;                      // rasterized brand mark for window/tray icons
pub fn tray_icon_rgba(size: u32, recording: bool) -> RgbaImage;
```

## snap-shell

```rust
pub type WindowKey = u64;
pub struct WindowSpec { pub title: String, pub size: Option<(f32, f32)> /* logical */, pub min_size: Option<(f32, f32)>,
    pub position: Option<Point> /* physical */, pub cover: Option<Rect> /* exact physical rect, borderless, for overlays */,
    pub decorations: bool, pub resizable: bool, pub always_on_top: bool, pub skip_taskbar: bool, pub visible: bool,
    pub tool_window: bool, pub exclude_from_capture: bool, pub rounded: bool, pub icon: Option<RgbaImage>, pub tag: String }
pub trait ShellApp<E: Send + 'static>: 'static {
    fn init(&mut self, sh: &mut Shell<E>) {}
    fn event(&mut self, sh: &mut Shell<E>, ev: E);                      // user events from any thread via proxy
    fn ui(&mut self, sh: &mut Shell<E>, win: WindowKey, ctx: &egui::Context);
    fn close_requested(&mut self, sh: &mut Shell<E>, win: WindowKey) -> bool { true }
    fn closed(&mut self, sh: &mut Shell<E>, win: WindowKey) {}
}
pub struct Shell<E>;  impl<E> Shell<E> {
    pub fn open(&mut self, spec: WindowSpec) -> WindowKey;   pub fn close(&mut self, w: WindowKey);
    pub fn exists(&self, w: WindowKey) -> bool;              pub fn set_visible(&mut self, w, v: bool);  pub fn focus(&mut self, w);
    pub fn set_title(&mut self, w, t: &str);  pub fn set_outer_rect(&mut self, w, r: Rect);  pub fn outer_rect(&self, w) -> Option<Rect>;
    pub fn set_always_on_top(&mut self, w, on: bool);  pub fn set_passthrough(&mut self, w, on: bool);  pub fn drag_window(&mut self, w);
    pub fn native_id(&self, w) -> Option<u64>;                 // HWND / NSWindow number / X11 id
    pub fn monitors(&self) -> Vec<(Rect, f32)>;               pub fn proxy(&self) -> Proxy<E>;  pub fn repaint(&self, w);
    pub fn windows_with_tag(&self, tag: &str) -> Vec<WindowKey>;  pub fn quit(&mut self);
}
#[derive(Clone)] pub struct Proxy<E>; impl<E> Proxy<E> { pub fn send(&self, e: E); pub fn repaint_all(&self); }
pub struct ShellConfig { pub app_id: String, pub accessory: bool /* macOS: no Dock icon */, pub theme_init: Box<dyn Fn(&egui::Context)> }
pub fn run<E: Send + 'static>(cfg: ShellConfig, app: impl ShellApp<E>) -> Result<()>;
```
One graphics context for all windows and, per window, a surface + renderer +
egui::Context + egui_winit::State. The renderer is OpenGL (glutin + egui_glow, one
shared context made current per window surface) on Windows and Linux and wgpu
(Metal) on macOS; the `wgpu` cargo feature selects wgpu everywhere (low-power
preference; DX12/Vulkan on Windows, Vulkan+GL on Linux). See `docs/notes/renderer.md`. The loop keeps running
with zero windows (tray). Windows open in < 50 ms (reuse device; create
hidden-then-show for overlays). Each window repaints only on input or request
(`ctx.request_repaint*` honoured per window). Fonts/theme installed into each
new context via `theme_init`. `cover` windows: borderless, exact physical
rect, topmost, no shadow, no taskbar, DPI-correct on mixed-DPI setups.

## snap-engine (built after the libraries)

```rust
pub struct Engine;  // Clone (Arc inner), Send + Sync
impl Engine {
    pub fn new(settings: Settings) -> Self;  pub fn settings(&self) -> Settings;  pub fn update_settings(&self, f: impl FnOnce(&mut Settings));
    pub fn secrets(&self) -> Arc<dyn SecretStore>;  pub fn history(&self) -> Option<&History>;
    pub fn capture(&self, req: &CaptureRequest) -> Result<Capture>;        // non-interactive: monitor/all/window/rect/last/element-at-cursor
    pub fn run(&self, cap: Capture, pipeline: &str, ui: &dyn PipelineUi) -> Result<Outcome>;  // executes Steps; records history; policy-aware
    pub fn save(&self, img: &RgbaImage, cap: &Capture, dir/template/format overrides) -> Result<PathBuf>;
    pub fn upload(&self, path_or_bytes, dest: Option<&str>) -> Result<UploadResult>;  pub fn ocr(&self, img) -> Result<OcrResult>;
    pub fn auto_redact(&self, img: &mut RgbaImage) -> Result<Vec<Hit>>;  pub fn scrolling(&self, rect: Rect, cancel: &AtomicBool, on_progress) -> Result<Capture>;
}
pub trait PipelineUi: Sync { fn annotate(&self, cap: &Capture) -> Result<Option<RgbaImage>>; fn pin(&self, img: &RgbaImage); fn notify(&self, title: &str, body: &str); fn progress(&self, label: &str, frac: Option<f32>); }
pub struct HeadlessUi;  // for CLI/MCP: annotate = pass-through, pin = no-op, notify = stderr
pub struct Outcome { pub file: Option<PathBuf>, pub url: Option<String>, pub text: Option<String>, pub codes: Vec<Code>, pub alt_text: Option<String> }
pub mod steps { /* step-capture session: shots + captions + click points → Markdown / HTML (self-contained) / MP4 slideshow, AI how-to */ }
pub mod mcp { pub fn serve_stdio(engine: Engine) -> Result<()>; }   // JSON-RPC 2.0 MCP: tools list_monitors, list_windows, capture_screen, capture_window, capture_region, ocr_image, decode_codes, search_history, get_capture
pub mod cli { pub fn run(args: Cli) -> Result<i32>; }               // clap; forwards to the running instance over IPC when it can
```

## apps/snapoleon (GUI)

`snapoleon` with no args starts the tray app (single instance; second launch
forwards its args over IPC). Subcommands: `capture`, `record`, `ocr`, `qr`,
`upload`, `pin`, `edit`, `history`, `pipeline`, `mcp`, `settings` (open), `url`
(handle a `snapoleon://` URL).

Windows/views: **Overlay** (per monitor `cover` window over a frozen snapshot:
crosshair + loupe w/ pixel grid and HEX/RGB readout, window & element snapping
with scroll-to-parent, drag/resize handles, size label, aspect lock (Shift),
arrow-key nudge, Space to move, instant toolbar: Copy, Save, Edit, Pin, Upload,
OCR, QR, Record, Scroll-capture, Redact, Cancel; color picker mode; measure mode
(px distance, element-to-element gaps)), **Editor** (tools: select, crop, arrow,
line, rect, ellipse, pen, highlighter, text, callout, step badge, redact
(pixelate/solid/blur) + auto-redact PII, spotlight, magnifier, stamp, measure,
image paste; color/stroke/fill/shadow; zoom/pan; beautify panel with presets;
export size w/ readable toggle; undo/redo; copy/save/save-as project/upload/pin;
OCR text panel; AI alt-text), **Pin** (borderless always-on-top image; drag,
wheel zoom, Ctrl+wheel opacity, double-click to edit, right-click menu, lock
click-through), **Recorder HUD** (4 thin frame windows around region + control
pill: timer, pause, mic level, stop, cancel; excluded from capture), **Video
editor** (scrubber with thumbnails, trim handles, export: MP4/GIF/WebP, size
target presets "Telegram 2 GB / email 25 MB / Discord 10 MB / custom", fps,
width, mute), **Library** (grid of thumbnails, search box with OCR/title/app/URL
search, filters by kind/app/date/favorites, detail pane with links/copy-as,
re-edit project, drag-out), **Settings** (General, Capture, Hotkeys, Pipelines
(visual step editor), Destinations (presets, test, .sxcu import), Links,
Recording, Text & AI, Privacy, History, Advanced (policy status, portable,
CLI/MCP snippets)), **Onboarding** (3 cards: hotkey, where files go, permissions
on macOS), **Text result** (OCR text editable, copy, copy as table, translate,
search web), **Step capture** (session HUD; afterwards a guide editor →
Markdown/HTML/MP4/AI how-to), **Combine**, **Diff**, **Tools** (color picker
history, measure).

## server/snapoleon-server

axum + tokio + rusqlite (bundled) + filesystem or S3 blob storage. Endpoints:
`POST /api/v1/upload` (multipart, bearer token; `expiry_hours`, `password`,
`title`), `GET /:code` (HTML viewer page with OG/Twitter meta, messenger-friendly
previews, password form when protected, view counter), `GET /:code/raw`,
`DELETE /api/v1/items/:id`, `GET /api/v1/items`, `GET /api/v1/events?since=`,
`POST /api/v1/shorten`, `GET /s/:code` (redirect), admin pages (`/admin`: users,
tokens, storage, retention, workspace settings) as server-rendered HTML with a
small CSS file, OIDC SSO login (authorization code + PKCE), local accounts
(argon2), retention sweeper, webhook notify on view/comment (Telegram/Slack
compatible JSON), comments on items. Config via `snapoleon-server.toml` + env.
Ships `deploy/` with Dockerfile, docker-compose, systemd unit, and
`examples/cloudflare-worker.js` (R2/S3 link proxy).

## Packaging & distribution

`cargo-packager` config: Windows NSIS (per-user, no admin) + MSI (WiX, per-machine
for IT) + portable zip (with `portable` marker); macOS `.app` + DMG (universal
binary, `LSUIElement`, `NSScreenCaptureUsageDescription`, URL scheme in
Info.plist); Linux AppImage + .deb + .rpm-ready + Flatpak manifest + AUR
PKGBUILD. Manifests: winget, Scoop, Chocolatey, Homebrew cask. ADMX/ADML policy
templates. GitHub Actions: CI (fmt, clippy, build on windows-x64/arm64,
macos-universal, linux-x64/arm64) and Release (signing hooks: Azure Trusted
Signing / signtool for Windows, `codesign` + `notarytool` for macOS, minisign
checksums).
