//! After-capture pipelines: a named, ordered list of steps bound to hotkeys,
//! CLI invocations and the tray. Data only — `snap-engine` executes them.

use serde::{Deserialize, Serialize};

use crate::settings::{ImageFormat, LinkFormat, RedactKind, RedactStyle};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pipeline {
    /// Stable identifier referenced by hotkeys / CLI (`--pipeline <id>`).
    pub id: String,
    pub name: String,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum Step {
    /// Open the editor and wait for the user (interactive; skipped headless).
    Annotate,
    /// OCR + PII detection, then cover matches.
    AutoRedact {
        #[serde(default)]
        kinds: Vec<RedactKind>,
        #[serde(default)]
        style: Option<RedactStyle>,
    },
    /// Apply a beautify preset by name (background, padding, frame...).
    Beautify { preset: String },
    /// Downscale to fit; `readable` keeps strokes/text legible.
    Resize {
        max_width: Option<u32>,
        max_height: Option<u32>,
        #[serde(default = "yes")]
        readable: bool,
    },
    /// Recognise text; optionally put it on the clipboard.
    Ocr {
        #[serde(default = "yes")]
        copy: bool,
        #[serde(default)]
        as_markdown_table: bool,
    },
    /// Decode QR / barcodes.
    DecodeCodes {
        #[serde(default = "yes")]
        copy: bool,
    },
    Translate { target_lang: Option<String> },
    /// Write to disk. `None` fields fall back to settings.
    Save {
        dir: Option<String>,
        template: Option<String>,
        format: Option<ImageFormat>,
    },
    CopyImage,
    /// Upload to a destination id (`None` = default destination).
    Upload { destination: Option<String> },
    Shorten,
    CopyLink { format: Option<LinkFormat> },
    Pin,
    /// Generate alt text / title with the configured AI endpoint.
    AltText {
        #[serde(default = "yes")]
        copy: bool,
    },
    /// Run an external program; `{file}`, `{url}`, `{text}` are substituted.
    RunCommand { program: String, args: Vec<String> },
    Notify,
}

fn yes() -> bool {
    true
}

impl Step {
    pub fn is_interactive(&self) -> bool {
        matches!(self, Step::Annotate | Step::Pin)
    }
}

/// Pipelines shipped by default. The user can edit or delete them.
pub fn default_pipelines() -> Vec<Pipeline> {
    vec![
        Pipeline {
            id: "quick".into(),
            name: "Quick: copy + save".into(),
            steps: vec![Step::CopyImage, Step::Save { dir: None, template: None, format: None }, Step::Notify],
        },
        Pipeline {
            id: "annotate".into(),
            name: "Annotate".into(),
            steps: vec![Step::Annotate],
        },
        Pipeline {
            id: "share".into(),
            name: "Redact + upload + copy Markdown".into(),
            steps: vec![
                Step::AutoRedact { kinds: vec![], style: None },
                Step::Save { dir: None, template: None, format: None },
                Step::Upload { destination: None },
                Step::CopyLink { format: Some(LinkFormat::Markdown) },
                Step::Notify,
            ],
        },
        Pipeline {
            id: "ocr".into(),
            name: "Copy text (OCR)".into(),
            steps: vec![Step::Ocr { copy: true, as_markdown_table: false }, Step::Notify],
        },
        Pipeline {
            id: "pin".into(),
            name: "Pin to screen".into(),
            steps: vec![Step::Pin],
        },
    ]
}
