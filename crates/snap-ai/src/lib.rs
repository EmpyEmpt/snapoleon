//! Optional AI: alt text, titles, how-to generation, translation and smart
//! redaction suggestions via OpenAI-compatible endpoints (Ollama, LM Studio,
//! llama.cpp, vLLM, OpenAI, OpenRouter) or the Anthropic Messages API.

mod client;
mod encode;
mod tasks;
mod text;
mod translate;

use snap_core::{CaptureContext, Error, Result, RgbaImage, secrets::SecretStore, settings::AiSettings, t};

pub use translate::translate;

use client::Client;

/// One recorded step handed to [`Ai::howto`].
#[derive(Debug, Clone, Default)]
pub struct HowToStep {
    pub caption: String,
    pub text: Option<String>,
    pub image: Option<RgbaImage>,
}

/// A generated guide: one instruction per input step.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HowTo {
    pub title: String,
    pub intro: String,
    pub steps: Vec<String>,
}

/// AI assistant bound to the user's endpoint settings.
pub struct Ai {
    settings: AiSettings,
    client: Client,
}

impl Ai {
    /// Resolves the API key through `secrets` (`secret_ref`); local endpoints
    /// work without one.
    pub fn new(s: &AiSettings, secrets: &dyn SecretStore) -> Self {
        let key = s
            .secret_ref
            .as_deref()
            .filter(|r| !r.trim().is_empty())
            .and_then(|r| secrets.get(r))
            .map(|k| k.trim().to_owned())
            .filter(|k| !k.is_empty());
        Self { settings: s.clone(), client: Client::new(&s.endpoint, key) }
    }

    /// Enabled and configured. Does not touch the network; use [`Ai::test`] for that.
    pub fn available(&self) -> bool {
        self.settings.enabled && !self.settings.endpoint.trim().is_empty() && !self.text_model().is_empty()
    }

    /// Checks the endpoint and returns a readable status (reachable models,
    /// warning when the configured models are not among them).
    pub fn test(&self) -> Result<String> {
        self.ensure_configured()?;
        self.client.test(self.text_model(), self.vision_model())
    }

    /// One or two plain sentences describing the screenshot for screen readers.
    pub fn alt_text(&self, img: &RgbaImage, cx: &CaptureContext) -> Result<String> {
        self.ensure_available()?;
        tasks::alt_text(&self.client, self.vision_model(), img, cx)
    }

    /// A title of at most 8 words, safe to use in file names and links.
    /// Needs an image, text, or a non-empty context.
    pub fn title(&self, img: Option<&RgbaImage>, text: Option<&str>, cx: &CaptureContext) -> Result<String> {
        self.ensure_available()?;
        let model = if img.is_some() { self.vision_model() } else { self.text_model() };
        tasks::title(&self.client, model, img, text, cx)
    }

    /// Turns recorded steps into a guide with exactly one instruction per step.
    pub fn howto(&self, steps: &[HowToStep]) -> Result<HowTo> {
        self.ensure_available()?;
        let model = if steps.iter().any(|s| s.image.is_some()) { self.vision_model() } else { self.text_model() };
        tasks::howto(&self.client, model, steps)
    }

    /// Substrings of `text` (typically OCR output) that look private or secret
    /// and that pattern-based detection may miss. Every item occurs verbatim in `text`.
    pub fn suggest_redactions(&self, text: &str) -> Result<Vec<String>> {
        self.ensure_available()?;
        tasks::suggest_redactions(&self.client, self.text_model(), text)
    }

    /// Translates `text` into the language `target` (an ISO code such as `ru`).
    pub fn translate(&self, text: &str, target: &str) -> Result<String> {
        self.ensure_available()?;
        tasks::translate(&self.client, self.text_model(), text, target)
    }

    fn text_model(&self) -> &str {
        self.settings.model.trim()
    }

    fn vision_model(&self) -> &str {
        let v = self.settings.vision_model.trim();
        if v.is_empty() { self.text_model() } else { v }
    }

    fn ensure_configured(&self) -> Result<()> {
        if self.settings.endpoint.trim().is_empty() {
            return Err(Error::invalid(t("ai.error.no_endpoint")));
        }
        Ok(())
    }

    fn ensure_available(&self) -> Result<()> {
        if !self.settings.enabled {
            return Err(Error::invalid(t("ai.error.disabled")));
        }
        self.ensure_configured()?;
        if self.text_model().is_empty() {
            return Err(Error::invalid(t("ai.error.no_model")));
        }
        Ok(())
    }
}
