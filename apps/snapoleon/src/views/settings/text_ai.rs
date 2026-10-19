//! Text and AI: OCR engines and languages, translation, the AI endpoint.

use egui::Ui;
use snap_core::{
    settings::{AiSettings, OcrEngine, OcrSettings, TranslateProvider, TranslateSettings},
    tf,
};
use snap_ui::{Tone, icon, space, text, widgets};

use super::kit::{self, Msg, Pick, Rows, SecretSlot, field_mono, managed_banner, opts, s, wrap};
use crate::views::ViewCx;

#[derive(Default)]
pub struct State {
    translate_secret: SecretSlot,
    ai_secret: SecretSlot,
    testing: bool,
    ai_test: Option<Result<String, String>>,
    /// Set by the "check again" button; the window re-runs the probe.
    pub reprobe: bool,
}

impl State {
    pub fn on_msg(&mut self, msg: Msg) -> Option<Msg> {
        match msg {
            Msg::AiTested(result) => {
                self.testing = false;
                self.ai_test = Some(result);
                None
            }
            other => Some(other),
        }
    }
}

/// Languages offered as chips next to whatever the system reports, as (BCP-47 code, native name).
const LANGUAGES: [(&str, &str); 13] = [
    ("en", "English"),
    ("ru", "Русский"),
    ("uk", "Українська"),
    ("de", "Deutsch"),
    ("fr", "Français"),
    ("es", "Español"),
    ("it", "Italiano"),
    ("pt", "Português"),
    ("pl", "Polski"),
    ("tr", "Türkçe"),
    ("zh", "中文"),
    ("ja", "日本語"),
    ("ko", "한국어"),
];

#[derive(Clone, Copy, PartialEq)]
enum Provider {
    None,
    Libre,
    DeepL,
    Ai,
}

impl Provider {
    fn of(p: &TranslateProvider) -> Self {
        match p {
            TranslateProvider::None => Provider::None,
            TranslateProvider::LibreTranslate { .. } => Provider::Libre,
            TranslateProvider::DeepL { .. } => Provider::DeepL,
            TranslateProvider::Ai => Provider::Ai,
        }
    }

    fn fresh(self) -> TranslateProvider {
        match self {
            Provider::None => TranslateProvider::None,
            Provider::Libre => TranslateProvider::LibreTranslate { url: "https://libretranslate.com".into() },
            Provider::DeepL => TranslateProvider::DeepL { free_api: true },
            Provider::Ai => TranslateProvider::Ai,
        }
    }
}

pub fn show(ui: &mut Ui, rc: &mut Rows, st: &mut State, cx: &mut ViewCx, set: &mut snap_core::Settings) {
    // The language chips carry native names, including the CJK ones.
    snap_ui::ensure_cjk(ui.ctx(), "中文日本語한국어");
    ocr(ui, rc, st, cx, &mut set.ocr);
    translate(ui, rc, st, cx, &mut set.translate);
    ai(ui, rc, st, cx, &mut set.ai);
}

fn ocr(ui: &mut Ui, rc: &mut Rows, st: &mut State, cx: &mut ViewCx, o: &mut OcrSettings) {
    let engines = opts(&[
        (OcrEngine::Auto, "settings.text_ai.engine.auto"),
        (OcrEngine::Native, "settings.text_ai.engine.native"),
        (OcrEngine::Tesseract, "settings.text_ai.engine.tesseract"),
        (OcrEngine::Ocrs, "settings.text_ai.engine.ocrs"),
    ]);
    let statuses: Option<Vec<(OcrEngine, bool, String)>> =
        rc.env.probe.as_ref().map(|p| p.ocr.iter().map(|e| (e.engine, e.available, e.note.clone())).collect());
    let tesseract_langs: Vec<String> = rc.env.probe.as_ref().map(|p| p.ocr_tesseract_langs.clone()).unwrap_or_default();
    let mut languages: Vec<(String, String)> =
        LANGUAGES.iter().map(|(code, name)| ((*code).to_owned(), format!("{name} ({code})"))).collect();
    let extra = rc.env.probe.iter().flat_map(|p| p.ocr_native_langs.iter()).chain(o.languages.iter());
    for code in extra {
        // "en-US" from the system is already covered by the "en" chip.
        let primary = code.split(['-', '_']).next().unwrap_or(code);
        if !languages.iter().any(|(c, _)| c.eq_ignore_ascii_case(code) || c.eq_ignore_ascii_case(primary)) {
            languages.push((code.clone(), code.clone()));
        }
    }

    rc.section(ui, &s("text_ai.section.ocr"), |ui, rc| {
        rc.choice(ui, "text_ai.ocr_engine", &mut o.engine, &engines);
        rc.stacked(ui, false, "text_ai.ocr_languages", |ui| {
            wrap(ui, |ui| {
                for (code, label) in &languages {
                    let on = o.languages.iter().any(|c| c.eq_ignore_ascii_case(code));
                    if widgets::chip(ui, label, on).clicked() {
                        if on {
                            o.languages.retain(|c| !c.eq_ignore_ascii_case(code));
                        } else {
                            o.languages.push(code.clone());
                        }
                    }
                }
            });
            if o.languages.is_empty() {
                kit::note(ui, &s("text_ai.ocr_languages.system"));
            }
            if !tesseract_langs.is_empty() && matches!(o.engine, OcrEngine::Tesseract | OcrEngine::Auto) {
                kit::note(ui, &tf("settings.text_ai.tesseract_installed", &[("langs", &tesseract_langs.join(", "))]));
            }
        });
        let hint = s("text_ai.tesseract_hint");
        rc.row(ui, "text_ai.tesseract_path", |ui| {
            kit::path_field(ui, cx, &mut o.tesseract_path, &hint, Pick::TesseractPath, 320.0);
        });
        rc.stacked(ui, false, "text_ai.engines", |ui| match &statuses {
            None => {
                ui.horizontal(|ui| {
                    widgets::spinner(ui);
                    ui.label(text::dim(s("probing")));
                });
            }
            Some(list) => {
                for (engine, available, note) in list {
                    ui.horizontal(|ui| {
                        let key = match engine {
                            OcrEngine::Native => "native",
                            OcrEngine::Tesseract => "tesseract",
                            OcrEngine::Ocrs => "ocrs",
                            OcrEngine::Auto => "auto",
                        };
                        ui.label(text::body_strong(s(&format!("text_ai.engine.{key}"))));
                        let (label, tone) =
                            if *available { (s("text_ai.available"), Tone::Success) } else { (s("text_ai.unavailable"), Tone::Warning) };
                        widgets::badge(ui, &label, tone);
                    });
                    if !note.is_empty() {
                        ui.add(egui::Label::new(text::caption(note.as_str())).wrap());
                    }
                    ui.add_space(space::XS);
                }
                if widgets::ghost_button(ui, &s("text_ai.check_again")).clicked() {
                    st.reprobe = true;
                }
            }
        });
    });
}

fn translate(ui: &mut Ui, rc: &mut Rows, st: &mut State, cx: &mut ViewCx, t: &mut TranslateSettings) {
    let providers = opts(&[
        (Provider::None, "settings.text_ai.provider.none"),
        (Provider::Libre, "settings.text_ai.provider.libre"),
        (Provider::DeepL, "settings.text_ai.provider.deepl"),
        (Provider::Ai, "settings.text_ai.provider.ai"),
    ]);
    let mut targets: Vec<(String, String)> =
        LANGUAGES.iter().map(|(code, name)| ((*code).to_owned(), format!("{name} ({code})"))).collect();
    if !targets.iter().any(|(c, _)| *c == t.target_lang) {
        targets.push((t.target_lang.clone(), t.target_lang.clone()));
    }

    let locked = rc.env.policy.disable_ai;
    ui.add_enabled_ui(!locked, |ui| {
        rc.section(ui, &s("text_ai.section.translate"), |ui, rc| {
            let before = Provider::of(&t.provider);
            let mut provider = before;
            rc.choice(ui, "text_ai.provider", &mut provider, &providers);
            if provider != before {
                t.provider = provider.fresh();
                st.translate_secret.reset();
            }
            rc.choice(ui, "text_ai.target_lang", &mut t.target_lang, &targets);
            match &mut t.provider {
                TranslateProvider::LibreTranslate { url } => {
                    rc.row(ui, "text_ai.libre_url", |ui| field_mono(ui, url, "https://libretranslate.example.com", 320.0));
                    st.translate_secret.row(ui, rc, "text_ai.translate_key", cx, &mut t.secret_ref, "translate:key");
                }
                TranslateProvider::DeepL { free_api } => {
                    rc.toggle(ui, "text_ai.deepl_free", free_api);
                    st.translate_secret.row(ui, rc, "text_ai.translate_key", cx, &mut t.secret_ref, "translate:key");
                }
                TranslateProvider::Ai | TranslateProvider::None => {}
            }
        });
    });
}

fn ai(ui: &mut Ui, rc: &mut Rows, st: &mut State, cx: &mut ViewCx, a: &mut AiSettings) {
    let locked = rc.env.policy.disable_ai;
    if locked {
        managed_banner(ui, &s("text_ai.ai_disabled"));
    }
    ui.add_enabled_ui(!locked, |ui| {
        rc.section(ui, &s("text_ai.section.ai"), |ui, rc| {
            rc.toggle(ui, "text_ai.ai_enabled", &mut a.enabled);
            rc.toggle(ui, "text_ai.smart_names", &mut a.smart_names);
            rc.text(ui, "text_ai.endpoint", &mut a.endpoint, "http://127.0.0.1:11434/v1", true, 320.0);
            rc.text(ui, "text_ai.model", &mut a.model, "qwen3:8b", true, 220.0);
            rc.text(ui, "text_ai.vision_model", &mut a.vision_model, "qwen2.5vl:7b", true, 220.0);
            st.ai_secret.row(ui, rc, "text_ai.ai_key", cx, &mut a.secret_ref, "ai:key");
            rc.row(ui, "text_ai.test", |ui| {
                if st.testing {
                    widgets::spinner(ui);
                    ui.label(text::dim(s("text_ai.testing")));
                } else if widgets::secondary_button(ui, &s("text_ai.test.button")).clicked() {
                    st.testing = true;
                    st.ai_test = None;
                    let settings = a.clone();
                    let secrets = cx.engine.secrets();
                    cx.spawn(move || {
                        Msg::AiTested(snap_ai::Ai::new(&settings, &*secrets).test().map_err(|e| crate::notices::error_text(&e)))
                    });
                }
            });
            match &st.ai_test {
                Some(Ok(message)) => rc.content(ui, |ui| status_line(ui, true, message)),
                Some(Err(message)) => rc.content(ui, |ui| status_line(ui, false, message)),
                None => {}
            }
            rc.content(ui, |ui| {
                ui.horizontal_top(|ui| {
                    ui.label(text::dim(icon::SHIELD_CHECK.to_string()));
                    ui.add(egui::Label::new(text::caption(s("text_ai.privacy_note"))).wrap());
                });
            });
        });
    });
}

fn status_line(ui: &mut Ui, ok: bool, message: &str) {
    let tone = if ok { Tone::Success } else { Tone::Danger };
    let p = snap_ui::Palette::of(ui);
    let glyph = if ok { icon::CHECK_CIRCLE } else { icon::WARNING_CIRCLE };
    ui.horizontal_top(|ui| {
        ui.label(egui::RichText::new(glyph).color(p.tone(tone)));
        ui.add(egui::Label::new(text::dim(message)).wrap());
    });
}
