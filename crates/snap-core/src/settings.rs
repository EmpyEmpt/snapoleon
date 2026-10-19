//! User settings, persisted as TOML at `paths::settings_file()`.
//!
//! Every field has a default so old/partial files always load. Secrets never
//! live here: destinations hold a `secret_ref` resolved through the OS keyring
//! by `snap-platform::secrets`.

use std::{
    io::Write,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use serde::{Deserialize, Serialize};

use crate::{Result, hotkey::HotkeyBinding, pipeline::Pipeline};

/// Set when the settings file exists but could not be read. The file is the user's only copy of their configuration,
/// so for the rest of the session nothing may write over it.
static READ_ONLY: AtomicBool = AtomicBool::new(false);

/// True when the settings file was unreadable at startup and is being left untouched.
pub fn is_read_only() -> bool {
    READ_ONLY.load(Ordering::Relaxed)
}

/// The settings file was read successfully (for instance after the user repaired it): saving works again.
pub fn clear_read_only() {
    READ_ONLY.store(false, Ordering::Relaxed);
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub general: General,
    pub capture: CaptureSettings,
    pub editor: EditorSettings,
    pub hotkeys: Vec<HotkeyBinding>,
    pub pipelines: Vec<Pipeline>,
    /// Pipeline id run after a capture when the binding names none.
    pub default_pipeline: String,
    pub destinations: Vec<Destination>,
    pub default_destination: Option<String>,
    pub links: LinkSettings,
    pub recording: RecordingSettings,
    pub history: HistorySettings,
    pub privacy: PrivacySettings,
    pub ocr: OcrSettings,
    pub translate: TranslateSettings,
    pub ai: AiSettings,
    pub pins: PinSettings,
    pub beautify_presets: Vec<BeautifyPreset>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            general: General::default(),
            capture: CaptureSettings::default(),
            editor: EditorSettings::default(),
            hotkeys: crate::hotkey::default_bindings(),
            pipelines: crate::pipeline::default_pipelines(),
            default_pipeline: "annotate".into(),
            destinations: Vec::new(),
            default_destination: None,
            links: LinkSettings::default(),
            recording: RecordingSettings::default(),
            history: HistorySettings::default(),
            privacy: PrivacySettings::default(),
            ocr: OcrSettings::default(),
            translate: TranslateSettings::default(),
            ai: AiSettings::default(),
            pins: PinSettings::default(),
            beautify_presets: default_beautify_presets(),
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Result<Settings> {
        match std::fs::read_to_string(path) {
            Ok(s) => toml::from_str(&s).map_err(|e| crate::Error::invalid(format!("{}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Load from the default location and apply managed policy on top.
    pub fn load_default() -> Settings {
        let mut s = Settings::load(&crate::paths::settings_file()).unwrap_or_else(|e| {
            tracing::warn!("settings unreadable, using defaults and leaving the file untouched: {e}");
            READ_ONLY.store(true, Ordering::Relaxed);
            Settings::default()
        });
        crate::policy::Policy::load().apply(&mut s);
        s
    }

    /// Atomic write: a synced temp file renamed over the target, with the previous file kept as `.bak`.
    /// Does nothing while the settings are read-only (see [`is_read_only`]).
    pub fn save(&self, path: &Path) -> Result<()> {
        if is_read_only() {
            tracing::debug!("settings are read-only this session; not saving");
            return Ok(());
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| crate::Error::invalid(e.to_string()))?;
        let tmp = path.with_extension("toml.tmp");
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        if path.exists() {
            std::fs::copy(path, path.with_extension("toml.bak"))?;
        }
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn pipeline(&self, id: &str) -> Option<&Pipeline> {
        self.pipelines.iter().find(|p| p.id == id)
    }
    pub fn destination(&self, id: &str) -> Option<&Destination> {
        self.destinations.iter().find(|d| d.id == id)
    }
    pub fn beautify_preset(&self, name: &str) -> Option<&BeautifyPreset> {
        self.beautify_presets.iter().find(|p| p.name == name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    System,
    Dark,
    Light,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// BCP-47-ish code matching a `locales/<lang>` dir; empty = system.
    pub language: String,
    pub theme: Theme,
    /// `#RRGGBB`; empty = brand accent.
    pub accent: String,
    pub launch_at_login: bool,
    pub show_notifications: bool,
    pub play_sounds: bool,
    pub tray_click: crate::hotkey::Action,
    pub first_run_done: bool,
    /// Disable UI animations regardless of the OS preference.
    pub reduce_motion: bool,
}

impl General {
    /// The user's accent override, if set and valid.
    pub fn accent_color(&self) -> Option<[u8; 3]> {
        parse_hex_color(&self.accent).map(|c| [c[0], c[1], c[2]])
    }
}

impl Default for General {
    fn default() -> Self {
        Self {
            language: String::new(),
            theme: Theme::System,
            accent: String::new(),
            launch_at_login: true,
            show_notifications: true,
            play_sounds: false,
            tray_click: crate::hotkey::Action::CaptureRegion,
            first_run_done: false,
            reduce_motion: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFormat {
    #[default]
    Png,
    Jpeg,
    Webp,
}

impl ImageFormat {
    pub fn ext(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Webp => "webp",
        }
    }
    pub fn mime(self) -> &'static str {
        match self {
            ImageFormat::Png => "image/png",
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Webp => "image/webp",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HdrMode {
    /// Tone-map when the monitor reports HDR.
    #[default]
    Auto,
    Off,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureSettings {
    pub format: ImageFormat,
    pub jpeg_quality: u8,
    pub webp_quality: u8,
    /// Losslessly optimise PNGs before save/upload.
    pub optimize_png: bool,
    pub include_cursor: bool,
    pub hide_desktop_icons: bool,
    pub delay_secs: u32,
    pub show_magnifier: bool,
    pub magnifier_zoom: u32,
    pub show_crosshair: bool,
    /// Dim outside the selection in the overlay.
    pub dim_outside: bool,
    /// Snap selection to windows/elements under the cursor.
    pub snap_to_windows: bool,
    pub save_dir: String,
    /// See `naming.rs` for tokens.
    pub filename_template: String,
    pub hdr: HdrMode,
    /// Capture window with transparent rounded corners/shadow where the OS allows.
    pub window_shadow: bool,
    /// Remember the last region for `CaptureLastRegion`.
    pub last_region: Option<crate::geom::Rect>,
}

impl Default for CaptureSettings {
    fn default() -> Self {
        Self {
            format: ImageFormat::Png,
            jpeg_quality: 90,
            webp_quality: 90,
            optimize_png: false,
            include_cursor: false,
            hide_desktop_icons: false,
            delay_secs: 3,
            show_magnifier: true,
            magnifier_zoom: 8,
            show_crosshair: true,
            dim_outside: true,
            snap_to_windows: true,
            save_dir: String::new(),
            filename_template: "Snapoleon {yyyy}-{MM}-{dd} {HH}.{mm}.{ss}".into(),
            hdr: HdrMode::Auto,
            window_shadow: false,
            last_region: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorSettings {
    /// `#RRGGBBAA` palette shown in the editor.
    pub palette: Vec<String>,
    pub stroke_width: f32,
    pub font_size: f32,
    /// Keep strokes/text legible when exporting downscaled.
    pub readable_downscale: bool,
    /// Max export width (0 = original).
    pub max_export_width: u32,
    pub last_tool: String,
    pub step_badge_start: u32,
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            palette: ["#FF3B30FF", "#FF9500FF", "#FFCC00FF", "#34C759FF", "#0A84FFFF", "#5E5CE6FF", "#FFFFFFFF", "#1C1C1EFF"]
                .map(String::from)
                .to_vec(),
            stroke_width: 4.0,
            font_size: 24.0,
            readable_downscale: true,
            max_export_width: 0,
            last_tool: "arrow".into(),
            step_badge_start: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Destination {
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub kind: DestinationKind,
    /// Keyring entry holding the secret (token / password / secret key).
    #[serde(default)]
    pub secret_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DestinationKind {
    /// Any S3-compatible store (AWS, R2, Yandex Cloud, Selectel, VK Cloud, MinIO, B2, Wasabi).
    S3 {
        endpoint: String,
        region: String,
        bucket: String,
        access_key_id: String,
        #[serde(default)]
        path_style: bool,
        /// Key prefix, may contain naming tokens.
        #[serde(default)]
        prefix: String,
        /// Public base (own domain / CDN). Empty = endpoint URL.
        #[serde(default)]
        public_base_url: String,
        /// Random object-name length (0 = keep file name).
        #[serde(default = "default_code_len")]
        random_code_len: u8,
        #[serde(default)]
        acl_public_read: bool,
    },
    Ftp {
        host: String,
        port: u16,
        user: String,
        #[serde(default)]
        secure: bool,
        #[serde(default)]
        dir: String,
        public_base_url: String,
    },
    WebDav {
        url: String,
        user: String,
        #[serde(default)]
        public_base_url: String,
    },
    /// Copy into a (synced) folder and build a URL from a base.
    LocalFolder {
        dir: String,
        public_base_url: String,
    },
    Imgur {
        client_id: String,
    },
    /// Bot token in secret; posts the file to a chat/channel.
    Telegram {
        chat_id: String,
    },
    YandexDisk {
        folder: String,
        #[serde(default = "yes")]
        publish: bool,
    },
    Dropbox {
        folder: String,
    },
    GoogleDrive {
        #[serde(default)]
        folder_id: String,
    },
    OneDrive {
        folder: String,
    },
    Box {
        #[serde(default)]
        folder_id: String,
        #[serde(default)]
        client_id: String,
    },
    /// ShareX `.sxcu`-compatible custom uploader.
    CustomHttp(CustomUploader),
    /// Self-hosted `snapoleon-server` (team mode).
    SnapServer {
        url: String,
    },
}

fn default_code_len() -> u8 {
    8
}
fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomUploader {
    pub request_method: String,
    pub request_url: String,
    pub parameters: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    /// `multipart_form_data` | `binary` | `form_url_encoded` | `json`
    pub body: String,
    /// Raw body template for `json` / `xml` bodies (ShareX `Data`).
    pub data: String,
    pub arguments: Vec<(String, String)>,
    pub file_form_name: String,
    /// ShareX syntax, e.g. `{json:data.link}`, `{regex:...}`, `{response}`.
    pub url: String,
    pub thumbnail_url: String,
    pub deletion_url: String,
    pub error_message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkFormat {
    #[default]
    Url,
    Markdown,
    Html,
    BbCode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Shortener {
    None,
    /// Use the team server's short links.
    SnapServer,
    Yourls {
        url: String,
        #[serde(default)]
        secret_ref: Option<String>,
    },
    Shlink {
        url: String,
        #[serde(default)]
        secret_ref: Option<String>,
    },
    IsGd,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LinkSettings {
    pub copy_format: LinkFormat,
    pub shortener: Shortener,
    /// Default expiry for server links, in hours (0 = never).
    pub expiry_hours: u32,
    /// Tell me when someone views or comments on a link on a team server.
    pub notify_events: bool,
}

impl Default for LinkSettings {
    fn default() -> Self {
        Self { copy_format: LinkFormat::Url, shortener: Shortener::None, expiry_hours: 0, notify_events: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoFormat {
    #[default]
    Mp4,
    Gif,
    Webp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EncoderPref {
    #[default]
    Auto,
    Hardware,
    Software,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordingSettings {
    pub format: VideoFormat,
    pub fps: u32,
    pub gif_fps: u32,
    /// Bits per second for MP4 (0 = auto from resolution).
    pub bitrate: u32,
    pub system_audio: bool,
    pub microphone: bool,
    /// Device name; empty = default.
    pub mic_device: String,
    pub show_cursor: bool,
    pub highlight_clicks: bool,
    pub show_keystrokes: bool,
    pub countdown_secs: u32,
    pub encoder: EncoderPref,
    pub max_gif_width: u32,
    /// Path to an ffmpeg binary used where no native encoder exists.
    pub ffmpeg_path: String,
}

impl Default for RecordingSettings {
    fn default() -> Self {
        Self {
            format: VideoFormat::Mp4,
            fps: 30,
            gif_fps: 15,
            bitrate: 0,
            system_audio: true,
            microphone: false,
            mic_device: String::new(),
            show_cursor: true,
            highlight_clicks: true,
            show_keystrokes: false,
            countdown_secs: 3,
            encoder: EncoderPref::Auto,
            max_gif_width: 960,
            ffmpeg_path: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HistorySettings {
    pub enabled: bool,
    /// Days to keep (0 = forever).
    pub retention_days: u32,
    /// Index OCR text for search (runs in background after capture).
    pub index_text: bool,
}

impl Default for HistorySettings {
    fn default() -> Self {
        Self { enabled: true, retention_days: 0, index_text: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedactKind {
    Email,
    Phone,
    CreditCard,
    Iban,
    /// Russian taxpayer id (ИНН), checksum-validated.
    RuInn,
    /// Russian pension id (СНИЛС), checksum-validated.
    RuSnils,
    RuPassport,
    UsSsn,
    IpAddress,
    /// API keys, tokens, JWTs, private-key blocks.
    Secret,
    Url,
}

impl RedactKind {
    pub const ALL: &'static [RedactKind] = &[
        RedactKind::Email,
        RedactKind::Phone,
        RedactKind::CreditCard,
        RedactKind::Iban,
        RedactKind::RuInn,
        RedactKind::RuSnils,
        RedactKind::RuPassport,
        RedactKind::UsSsn,
        RedactKind::IpAddress,
        RedactKind::Secret,
        RedactKind::Url,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedactStyle {
    #[default]
    Pixelate,
    Solid,
    Blur,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrivacySettings {
    pub redact_kinds: Vec<RedactKind>,
    pub redact_style: RedactStyle,
    /// Never embed EXIF/text chunks in exported images.
    pub strip_metadata: bool,
    /// Store window title / URL context in history.
    pub record_context: bool,
}

impl Default for PrivacySettings {
    fn default() -> Self {
        Self {
            redact_kinds: vec![
                RedactKind::Email,
                RedactKind::Phone,
                RedactKind::CreditCard,
                RedactKind::Iban,
                RedactKind::RuInn,
                RedactKind::RuSnils,
                RedactKind::RuPassport,
                RedactKind::Secret,
            ],
            redact_style: RedactStyle::Pixelate,
            strip_metadata: true,
            record_context: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrEngine {
    /// OS engine (Windows.Media.Ocr / Apple Vision), else bundled fallback.
    #[default]
    Auto,
    Native,
    Ocrs,
    Tesseract,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OcrSettings {
    pub engine: OcrEngine,
    /// Preferred languages (BCP-47). Empty = user profile languages.
    pub languages: Vec<String>,
    pub tesseract_path: String,
}

impl Default for OcrSettings {
    fn default() -> Self {
        Self { engine: OcrEngine::Auto, languages: Vec::new(), tesseract_path: String::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TranslateProvider {
    None,
    LibreTranslate {
        url: String,
    },
    DeepL {
        #[serde(default)]
        free_api: bool,
    },
    /// Use the AI endpoint from `AiSettings`.
    Ai,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TranslateSettings {
    pub provider: TranslateProvider,
    pub target_lang: String,
    pub secret_ref: Option<String>,
}

impl Default for TranslateSettings {
    fn default() -> Self {
        Self { provider: TranslateProvider::Ai, target_lang: "en".into(), secret_ref: None }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiSettings {
    pub enabled: bool,
    /// OpenAI-compatible base URL. Default targets a local Ollama.
    pub endpoint: String,
    pub model: String,
    pub vision_model: String,
    pub secret_ref: Option<String>,
    /// Name saved files and shared links after what the capture shows.
    pub smart_names: bool,
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: "http://127.0.0.1:11434/v1".into(),
            model: "qwen3:8b".into(),
            vision_model: "qwen2.5vl:7b".into(),
            secret_ref: None,
            smart_names: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PinSettings {
    pub opacity: f32,
    pub shadow: bool,
    pub border_color: String,
}

impl Default for PinSettings {
    fn default() -> Self {
        Self { opacity: 1.0, shadow: true, border_color: "#3D5AFEFF".into() }
    }
}

/// Beautify = put the capture on a background (CleanShot-style).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BeautifyPreset {
    pub name: String,
    pub background: Background,
    pub padding: u32,
    pub corner_radius: u32,
    pub shadow: u32,
    /// `None` = keep natural size; `Some((16, 9))` etc.
    pub aspect: Option<(u32, u32)>,
    pub frame: FrameStyle,
}

impl Default for BeautifyPreset {
    fn default() -> Self {
        Self {
            name: "Default".into(),
            background: Background::Gradient { from: "#5E5CE6".into(), to: "#0A84FF".into(), angle_deg: 135.0 },
            padding: 64,
            corner_radius: 12,
            shadow: 24,
            aspect: None,
            frame: FrameStyle::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Background {
    Transparent,
    Solid {
        color: String,
    },
    Gradient {
        from: String,
        to: String,
        angle_deg: f32,
    },
    /// Blurred, enlarged copy of the capture itself.
    BlurredCapture,
    Image {
        path: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameStyle {
    #[default]
    None,
    MacWindow,
    MacWindowDark,
    Browser,
    BrowserDark,
    Phone,
}

pub fn default_beautify_presets() -> Vec<BeautifyPreset> {
    vec![
        BeautifyPreset::default(),
        BeautifyPreset {
            name: "Sunset".into(),
            background: Background::Gradient { from: "#FF9A62".into(), to: "#E8467C".into(), angle_deg: 120.0 },
            ..BeautifyPreset::default()
        },
        BeautifyPreset {
            name: "Graphite".into(),
            background: Background::Solid { color: "#1C1C1E".into() },
            frame: FrameStyle::MacWindowDark,
            ..BeautifyPreset::default()
        },
        BeautifyPreset {
            name: "Social 16:9".into(),
            background: Background::BlurredCapture,
            aspect: Some((16, 9)),
            ..BeautifyPreset::default()
        },
        BeautifyPreset {
            name: "Browser".into(),
            background: Background::Gradient { from: "#E0E7FF".into(), to: "#F5F3FF".into(), angle_deg: 160.0 },
            frame: FrameStyle::Browser,
            ..BeautifyPreset::default()
        },
    ]
}

/// Parse `#RGB`, `#RRGGBB` or `#RRGGBBAA` into straight RGBA.
pub fn parse_hex_color(s: &str) -> Option<[u8; 4]> {
    let h = s.trim().trim_start_matches('#');
    let p = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    match h.len() {
        3 => {
            let c: Vec<u8> = h.chars().map(|c| c.to_digit(16).map(|d| d as u8 * 17)).collect::<Option<_>>()?;
            Some([c[0], c[1], c[2], 255])
        }
        6 => Some([p(0)?, p(2)?, p(4)?, 255]),
        8 => Some([p(0)?, p(2)?, p(4)?, p(6)?]),
        _ => None,
    }
}

pub fn to_hex_color(c: [u8; 4]) -> String {
    if c[3] == 255 {
        format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
    } else {
        format!("#{:02X}{:02X}{:02X}{:02X}", c[0], c[1], c[2], c[3])
    }
}
