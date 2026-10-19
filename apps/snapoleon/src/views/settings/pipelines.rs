//! Pipelines: the list, the visual step editor and the beautify presets the Beautify step uses.

use egui::{Align, Layout, Ui, Widget};
use snap_core::{
    Settings,
    pipeline::{Pipeline, Step},
    settings::{Background, BeautifyPreset, FrameStyle, ImageFormat, LinkFormat, RedactKind, RedactStyle},
    tf,
};
use snap_ui::{
    Tone, icon, space, text,
    widgets::{self, ButtonKind, badge, card_frame},
};

use super::kit::{self, Pick, Rows, color_well, dd, dd_opt, field, field_mono, form_row, list_row, opts, s, wrap};
use crate::views::ViewCx;

#[derive(Default)]
pub struct State {
    /// The pipeline whose steps are being edited; `None` shows the list.
    selected: Option<String>,
    /// The "add step" palette is open.
    adding: bool,
    /// The expanded beautify preset.
    preset_open: Option<usize>,
}

/// Every step kind in the order the palette offers them.
const KINDS: [&str; 16] = [
    "annotate",
    "auto_redact",
    "beautify",
    "resize",
    "ocr",
    "decode_codes",
    "translate",
    "save",
    "copy_image",
    "upload",
    "shorten",
    "copy_link",
    "pin",
    "alt_text",
    "run_command",
    "notify",
];

fn kind_of(step: &Step) -> &'static str {
    match step {
        Step::Annotate => "annotate",
        Step::AutoRedact { .. } => "auto_redact",
        Step::Beautify { .. } => "beautify",
        Step::Resize { .. } => "resize",
        Step::Ocr { .. } => "ocr",
        Step::DecodeCodes { .. } => "decode_codes",
        Step::Translate { .. } => "translate",
        Step::Save { .. } => "save",
        Step::CopyImage => "copy_image",
        Step::Upload { .. } => "upload",
        Step::Shorten => "shorten",
        Step::CopyLink { .. } => "copy_link",
        Step::Pin => "pin",
        Step::AltText { .. } => "alt_text",
        Step::RunCommand { .. } => "run_command",
        Step::Notify => "notify",
    }
}

/// The localised name of a step, for summaries elsewhere (the first-run card lists a pipeline's steps).
pub fn step_title(step: &Step) -> String {
    s(&format!("pipelines.step.{}.title", kind_of(step)))
}

fn kind_icon(kind: &str) -> &'static str {
    match kind {
        "annotate" => icon::PENCIL_SIMPLE,
        "auto_redact" => icon::EYE_SLASH,
        "beautify" => icon::MAGIC_WAND,
        "resize" => icon::ARROWS_IN_SIMPLE,
        "ocr" => icon::TEXT_AA,
        "decode_codes" => icon::QR_CODE,
        "translate" => icon::TRANSLATE,
        "save" => icon::FLOPPY_DISK,
        "copy_image" => icon::COPY,
        "upload" => icon::CLOUD_ARROW_UP,
        "shorten" => icon::LINK_SIMPLE,
        "copy_link" => icon::LINK,
        "pin" => icon::PUSH_PIN,
        "alt_text" => icon::SPARKLE,
        "run_command" => icon::TERMINAL,
        _ => icon::BELL,
    }
}

fn new_step(kind: &str, set: &Settings) -> Step {
    match kind {
        "annotate" => Step::Annotate,
        "auto_redact" => Step::AutoRedact { kinds: Vec::new(), style: None },
        "beautify" => Step::Beautify { preset: set.beautify_presets.first().map(|p| p.name.clone()).unwrap_or_default() },
        "resize" => Step::Resize { max_width: Some(1920), max_height: None, readable: true },
        "ocr" => Step::Ocr { copy: true, as_markdown_table: false },
        "decode_codes" => Step::DecodeCodes { copy: true },
        "translate" => Step::Translate { target_lang: None },
        "save" => Step::Save { dir: None, template: None, format: None },
        "copy_image" => Step::CopyImage,
        "upload" => Step::Upload { destination: None },
        "shorten" => Step::Shorten,
        "copy_link" => Step::CopyLink { format: None },
        "pin" => Step::Pin,
        "alt_text" => Step::AltText { copy: true },
        "run_command" => Step::RunCommand { program: String::new(), args: Vec::new() },
        _ => Step::Notify,
    }
}

/// The drop-down values the step editors share.
struct Lookups {
    presets: Vec<String>,
    destinations: Vec<(String, String)>,
    formats: Vec<(ImageFormat, String)>,
    link_formats: Vec<(LinkFormat, String)>,
    styles: Vec<(RedactStyle, String)>,
}

impl Lookups {
    fn new(set: &Settings) -> Self {
        Self {
            presets: set.beautify_presets.iter().map(|p| p.name.clone()).collect(),
            destinations: set.destinations.iter().map(|d| (d.id.clone(), d.name.clone())).collect(),
            formats: opts(&[
                (ImageFormat::Png, "settings.capture.format.png"),
                (ImageFormat::Jpeg, "settings.capture.format.jpeg"),
                (ImageFormat::Webp, "settings.capture.format.webp"),
            ]),
            link_formats: link_format_options(),
            styles: style_options(),
        }
    }
}

pub fn link_format_options() -> Vec<(LinkFormat, String)> {
    opts(&[
        (LinkFormat::Url, "settings.links.format.url"),
        (LinkFormat::Markdown, "settings.links.format.markdown"),
        (LinkFormat::Html, "settings.links.format.html"),
        (LinkFormat::BbCode, "settings.links.format.bbcode"),
    ])
}

pub fn style_options() -> Vec<(RedactStyle, String)> {
    opts(&[
        (RedactStyle::Pixelate, "settings.privacy.style.pixelate"),
        (RedactStyle::Solid, "settings.privacy.style.solid"),
        (RedactStyle::Blur, "settings.privacy.style.blur"),
    ])
}

pub fn show(ui: &mut Ui, rc: &mut Rows, st: &mut State, cx: &mut ViewCx, set: &mut Settings) {
    if rc.searching() {
        defaults(ui, rc, set);
        return;
    }
    if let Some(id) = st.selected.clone() {
        if set.pipelines.iter().any(|p| p.id == id) {
            editor(ui, st, cx, set, &id);
            return;
        }
        st.selected = None;
    }
    overview(ui, rc, st, cx, set);
}

fn defaults(ui: &mut Ui, rc: &mut Rows, set: &mut Settings) {
    let names: Vec<(String, String)> = set.pipelines.iter().map(|p| (p.id.clone(), p.name.clone())).collect();
    rc.section(ui, &s("pipelines.section.defaults"), |ui, rc| {
        rc.row(ui, "pipelines.default", |ui| {
            dd(ui, "default-pipeline", &mut set.default_pipeline, &names);
        });
    });
}

fn overview(ui: &mut Ui, rc: &mut Rows, st: &mut State, cx: &mut ViewCx, set: &mut Settings) {
    ui.label(text::body_strong(s("pipelines.list")));
    ui.add_space(space::XS);
    let mut open = None;
    for p in &set.pipelines {
        let subtitle = if p.steps.is_empty() {
            s("pipelines.no_steps_short")
        } else {
            p.steps.iter().map(step_title).collect::<Vec<_>>().join(" → ")
        };
        let default = p.id == set.default_pipeline;
        let resp = list_row(ui, icon::FLOW_ARROW, &p.name, &subtitle, |ui| {
            if default {
                badge(ui, &s("pipelines.default_badge"), Tone::Accent);
            }
        });
        if resp.clicked() {
            open = Some(p.id.clone());
        }
    }
    ui.add_space(space::XS);
    if widgets::Button::new(&s("pipelines.new")).kind(ButtonKind::Primary).icon(icon::PLUS).ui(ui).clicked() {
        let id = unique_id(set, &s("pipelines.new_name"));
        set.pipelines.push(Pipeline { id: id.clone(), name: s("pipelines.new_name"), steps: Vec::new() });
        open = Some(id);
    }
    if let Some(id) = open {
        st.selected = Some(id);
        st.adding = false;
    }
    ui.add_space(space::XL);

    defaults(ui, rc, set);
    presets(ui, cx, st, set);
}

/// A stable machine id derived from a name, unique among the pipelines.
fn unique_id(set: &Settings, name: &str) -> String {
    kit::unique_slug(name, "pipeline", |id| set.pipelines.iter().any(|p| p.id == id))
}

// ---------------------------------------------------------------------------------------------------------------------
// One pipeline

enum StepOp {
    Up(usize),
    Down(usize),
    Remove(usize),
}

fn editor(ui: &mut Ui, st: &mut State, cx: &mut ViewCx, set: &mut Settings, id: &str) {
    if kit::back_header(ui, &s("pipelines.back")) {
        st.selected = None;
        return;
    }
    let lookups = Lookups::new(set);
    let Some(index) = set.pipelines.iter().position(|p| p.id == id) else { return };
    let is_default = set.default_pipeline == id;
    let only_one = set.pipelines.len() == 1;
    let mut action = HeaderAction::None;

    // Name and pipeline-level actions.
    {
        let pipeline = &mut set.pipelines[index];
        card_frame(ui, 16).show(ui, |ui| {
            ui.set_width(ui.available_width());
            form_row(ui, &s("pipelines.name"), |ui| {
                field(ui, &mut pipeline.name, &s("pipelines.name_hint"), 280.0);
            });
            form_row(ui, &s("pipelines.id"), |ui| {
                ui.label(text::mono_text(id));
                ui.label(text::caption(s("pipelines.id_hint")));
            });
            ui.add_space(space::XS);
            ui.horizontal(|ui| {
                if is_default {
                    badge(ui, &s("pipelines.default_badge"), Tone::Accent);
                } else if widgets::secondary_button(ui, &s("pipelines.make_default")).clicked() {
                    action = HeaderAction::MakeDefault;
                }
                if widgets::Button::new(&s("pipelines.duplicate")).kind(ButtonKind::Ghost).icon(icon::COPY_SIMPLE).ui(ui).clicked() {
                    action = HeaderAction::Duplicate;
                }
                ui.add_enabled_ui(!only_one, |ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if kit::confirm_button(ui, ("delete-pipeline", id), &s("pipelines.delete"), &s("pipelines.delete_confirm")) {
                            action = HeaderAction::Delete;
                        }
                    });
                });
            });
        });
    }
    match action {
        HeaderAction::None => {}
        HeaderAction::MakeDefault => set.default_pipeline = id.to_owned(),
        HeaderAction::Duplicate => {
            let mut copy = set.pipelines[index].clone();
            copy.name = tf("settings.pipelines.copy_name", &[("name", &copy.name)]);
            copy.id = unique_id(set, &copy.name);
            st.selected = Some(copy.id.clone());
            set.pipelines.insert(index + 1, copy);
            return;
        }
        HeaderAction::Delete => {
            set.pipelines.remove(index);
            if set.default_pipeline == id {
                set.default_pipeline = set.pipelines.first().map(|p| p.id.clone()).unwrap_or_default();
            }
            for b in &mut set.hotkeys {
                if b.pipeline.as_deref() == Some(id) {
                    b.pipeline = None;
                }
            }
            st.selected = None;
            return;
        }
    }

    ui.add_space(space::LG);
    ui.label(text::body_strong(s("pipelines.steps")));
    ui.add_space(space::XS);

    let mut op = None;
    let count = set.pipelines[index].steps.len();
    if count == 0 {
        card_frame(ui, 16).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(text::dim(s("pipelines.no_steps")));
        });
    }
    for i in 0..count {
        let step = &mut set.pipelines[index].steps[i];
        if let Some(found) = step_card(ui, cx, id, i, count, step, &lookups) {
            op = Some(found);
        }
    }
    match op {
        Some(StepOp::Up(i)) => set.pipelines[index].steps.swap(i, i - 1),
        Some(StepOp::Down(i)) => set.pipelines[index].steps.swap(i, i + 1),
        Some(StepOp::Remove(i)) => {
            set.pipelines[index].steps.remove(i);
        }
        None => {}
    }

    ui.add_space(space::XS);
    if st.adding {
        card_frame(ui, 16).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(text::body_strong(s("pipelines.add_step")));
            ui.add_space(space::SM);
            wrap(ui, |ui| {
                for kind in KINDS {
                    let title = s(&format!("pipelines.step.{kind}.title"));
                    if widgets::chip_with_icon(ui, kind_icon(kind), &title, false)
                        .on_hover_text(s(&format!("pipelines.step.{kind}.desc")))
                        .clicked()
                    {
                        let step = new_step(kind, set);
                        set.pipelines[index].steps.push(step);
                        st.adding = false;
                    }
                }
            });
            ui.add_space(space::SM);
            if widgets::ghost_button(ui, &snap_core::t("common.cancel")).clicked() {
                st.adding = false;
            }
        });
    } else if widgets::Button::new(&s("pipelines.add_step")).kind(ButtonKind::Secondary).icon(icon::PLUS).ui(ui).clicked() {
        st.adding = true;
    }
}

enum HeaderAction {
    None,
    MakeDefault,
    Duplicate,
    Delete,
}

/// One step as a card: number, icon, title, move and remove buttons, then its parameters.
fn step_card(ui: &mut Ui, cx: &mut ViewCx, pipeline: &str, i: usize, count: usize, step: &mut Step, lookups: &Lookups) -> Option<StepOp> {
    let kind = kind_of(step);
    let mut op = None;
    card_frame(ui, 16).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            badge(ui, &(i + 1).to_string(), Tone::Gold);
            ui.label(text::dim(kind_icon(kind).to_string()));
            ui.label(text::body_strong(s(&format!("pipelines.step.{kind}.title"))));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::icon_button(ui, icon::TRASH, &s("pipelines.step.remove"), None).clicked() {
                    op = Some(StepOp::Remove(i));
                }
                ui.add_enabled_ui(i + 1 < count, |ui| {
                    if widgets::icon_button(ui, icon::ARROW_DOWN, &s("pipelines.step.down"), None).clicked() {
                        op = Some(StepOp::Down(i));
                    }
                });
                ui.add_enabled_ui(i > 0, |ui| {
                    if widgets::icon_button(ui, icon::ARROW_UP, &s("pipelines.step.up"), None).clicked() {
                        op = Some(StepOp::Up(i));
                    }
                });
            });
        });
        ui.label(text::caption(s(&format!("pipelines.step.{kind}.desc"))));
        ui.add_space(space::XS);
        step_params(ui, cx, pipeline, i, step, lookups);
    });
    op
}

fn toggle_param(ui: &mut Ui, label_key: &str, value: &mut bool) {
    form_row(ui, &s(label_key), |ui| {
        widgets::toggle(ui, value);
    });
}

fn step_params(ui: &mut Ui, cx: &mut ViewCx, pipeline: &str, i: usize, step: &mut Step, lookups: &Lookups) {
    match step {
        Step::Annotate | Step::CopyImage | Step::Shorten | Step::Pin | Step::Notify => {}
        Step::AutoRedact { kinds, style } => {
            form_row(ui, &s("pipelines.param.kinds"), |ui| {
                wrap(ui, |ui| {
                    for kind in RedactKind::ALL {
                        let on = kinds.contains(kind);
                        if widgets::chip(ui, &snap_redact::kind_label(*kind), on).clicked() {
                            if on {
                                kinds.retain(|k| k != kind);
                            } else {
                                kinds.push(*kind);
                            }
                        }
                    }
                });
            });
            kit::note(ui, &s("pipelines.param.kinds_hint"));
            form_row(ui, &s("pipelines.param.style"), |ui| {
                dd_opt(ui, ("redact-style", pipeline, i), style, &s("pipelines.param.from_privacy"), &lookups.styles)
            });
        }
        Step::Beautify { preset } => {
            form_row(ui, &s("pipelines.param.preset"), |ui| {
                let mut names: Vec<(String, String)> = lookups.presets.iter().map(|n| (n.clone(), n.clone())).collect();
                if !names.iter().any(|(n, _)| n == preset) {
                    names.push((preset.clone(), preset.clone()));
                }
                dd(ui, ("beautify-preset", pipeline, i), preset, &names);
            });
        }
        Step::Resize { max_width, max_height, readable } => {
            for (label, value) in [("pipelines.param.max_width", max_width), ("pipelines.param.max_height", max_height)] {
                form_row(ui, &s(label), |ui| {
                    let mut n = value.unwrap_or(0);
                    widgets::number_field(ui, &mut n, 0..=16384, " px");
                    *value = (n > 0).then_some(n);
                });
            }
            kit::note(ui, &s("pipelines.param.size_hint"));
            toggle_param(ui, "pipelines.param.readable", readable);
        }
        Step::Ocr { copy, as_markdown_table } => {
            toggle_param(ui, "pipelines.param.copy_text", copy);
            toggle_param(ui, "pipelines.param.markdown_table", as_markdown_table);
        }
        Step::DecodeCodes { copy } => toggle_param(ui, "pipelines.param.copy_text", copy),
        Step::AltText { copy } => toggle_param(ui, "pipelines.param.copy_text", copy),
        Step::Translate { target_lang } => {
            form_row(ui, &s("pipelines.param.target_lang"), |ui| {
                let mut lang = target_lang.clone().unwrap_or_default();
                field_mono(ui, &mut lang, &s("pipelines.param.target_lang_hint"), 160.0);
                *target_lang = (!lang.trim().is_empty()).then(|| lang.trim().to_owned());
            });
        }
        Step::Save { dir, template, format } => {
            form_row(ui, &s("pipelines.param.folder"), |ui| {
                let mut value = dir.clone().unwrap_or_default();
                let hint = s("pipelines.param.from_capture");
                kit::path_field_ltr(ui, cx, &mut value, &hint, Pick::StepDir { pipeline: pipeline.to_owned(), step: i }, 320.0);
                *dir = (!value.trim().is_empty()).then_some(value);
            });
            form_row(ui, &s("pipelines.param.template"), |ui| {
                let mut value = template.clone().unwrap_or_default();
                field_mono(ui, &mut value, &s("pipelines.param.from_capture"), 320.0);
                *template = (!value.trim().is_empty()).then_some(value);
            });
            form_row(ui, &s("pipelines.param.format"), |ui| {
                dd_opt(ui, ("save-format", pipeline, i), format, &s("pipelines.param.from_capture"), &lookups.formats)
            });
        }
        Step::Upload { destination } => {
            form_row(ui, &s("pipelines.param.destination"), |ui| {
                dd_opt(ui, ("upload-dest", pipeline, i), destination, &s("pipelines.param.default_destination"), &lookups.destinations);
            });
            if lookups.destinations.is_empty() {
                kit::note(ui, &s("pipelines.param.no_destinations"));
            }
        }
        Step::CopyLink { format } => {
            form_row(ui, &s("pipelines.param.link_format"), |ui| {
                dd_opt(ui, ("link-format", pipeline, i), format, &s("pipelines.param.from_links"), &lookups.link_formats);
            });
        }
        Step::RunCommand { program, args } => {
            form_row(ui, &s("pipelines.param.program"), |ui| field_mono(ui, program, &s("pipelines.param.program_hint"), 320.0));
            let mut remove = None;
            for (n, arg) in args.iter_mut().enumerate() {
                form_row(ui, &if n == 0 { s("pipelines.param.arguments") } else { String::new() }, |ui| {
                    field_mono(ui, arg, "{file}", 280.0);
                    if widgets::icon_button(ui, icon::X, &s("pipelines.param.remove_arg"), None).clicked() {
                        remove = Some(n);
                    }
                });
            }
            if let Some(n) = remove {
                args.remove(n);
            }
            form_row(ui, &if args.is_empty() { s("pipelines.param.arguments") } else { String::new() }, |ui| {
                if widgets::Button::new(&s("pipelines.param.add_arg")).kind(ButtonKind::Ghost).icon(icon::PLUS).ui(ui).clicked() {
                    args.push(String::new());
                }
            });
            kit::note(ui, &s("pipelines.param.placeholders"));
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Beautify presets

#[derive(Clone, Copy, PartialEq)]
enum BgKind {
    Transparent,
    Solid,
    Gradient,
    Blurred,
    Image,
}

impl BgKind {
    fn of(bg: &Background) -> Self {
        match bg {
            Background::Transparent => Self::Transparent,
            Background::Solid { .. } => Self::Solid,
            Background::Gradient { .. } => Self::Gradient,
            Background::BlurredCapture => Self::Blurred,
            Background::Image { .. } => Self::Image,
        }
    }

    fn fresh(self) -> Background {
        match self {
            Self::Transparent => Background::Transparent,
            Self::Solid => Background::Solid { color: "#1C1C1E".into() },
            Self::Gradient => Background::Gradient { from: "#5E5CE6".into(), to: "#0A84FF".into(), angle_deg: 135.0 },
            Self::Blurred => Background::BlurredCapture,
            Self::Image => Background::Image { path: String::new() },
        }
    }
}

enum PresetOp {
    Toggle(usize),
    Duplicate(usize),
    Delete(usize),
}

fn presets(ui: &mut Ui, cx: &mut ViewCx, st: &mut State, set: &mut Settings) {
    ui.label(text::body_strong(s("pipelines.presets")));
    ui.label(text::caption(s("pipelines.presets_hint")));
    ui.add_space(space::SM);
    let backgrounds = opts(&[
        (BgKind::Transparent, "settings.pipelines.bg.transparent"),
        (BgKind::Solid, "settings.pipelines.bg.solid"),
        (BgKind::Gradient, "settings.pipelines.bg.gradient"),
        (BgKind::Blurred, "settings.pipelines.bg.blurred"),
        (BgKind::Image, "settings.pipelines.bg.image"),
    ]);
    let frames = opts(&[
        (FrameStyle::None, "settings.pipelines.frame.none"),
        (FrameStyle::MacWindow, "settings.pipelines.frame.mac_window"),
        (FrameStyle::MacWindowDark, "settings.pipelines.frame.mac_window_dark"),
        (FrameStyle::Browser, "settings.pipelines.frame.browser"),
        (FrameStyle::BrowserDark, "settings.pipelines.frame.browser_dark"),
        (FrameStyle::Phone, "settings.pipelines.frame.phone"),
    ]);
    let mut op = None;
    let total = set.beautify_presets.len();
    let mut renames: Vec<(String, String)> = Vec::new();
    for idx in 0..total {
        let open = st.preset_open == Some(idx);
        let preset = &mut set.beautify_presets[idx];
        card_frame(ui, 16).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(text::dim(icon::PALETTE.to_string()));
                ui.label(text::body_strong(preset.name.as_str()));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let chevron = if open { icon::CARET_UP } else { icon::CARET_DOWN };
                    if widgets::icon_button(ui, chevron, &s("pipelines.preset.edit"), None).clicked() {
                        op = Some(PresetOp::Toggle(idx));
                    }
                    if widgets::icon_button(ui, icon::COPY_SIMPLE, &s("pipelines.duplicate"), None).clicked() {
                        op = Some(PresetOp::Duplicate(idx));
                    }
                    ui.add_enabled_ui(total > 1, |ui| {
                        if widgets::icon_button(ui, icon::TRASH, &s("pipelines.delete"), None).clicked() {
                            op = Some(PresetOp::Delete(idx));
                        }
                    });
                });
            });
            ui.label(text::caption(preset_summary(preset)));
            if open {
                ui.add_space(space::SM);
                preset_form(ui, cx, idx, preset, &backgrounds, &frames, &mut renames);
            }
        });
    }
    // A renamed preset keeps the Beautify steps that use it working.
    for (old, new) in renames {
        for step in set.pipelines.iter_mut().flat_map(|p| p.steps.iter_mut()) {
            if let Step::Beautify { preset } = step
                && *preset == old
            {
                *preset = new.clone();
            }
        }
    }
    match op {
        Some(PresetOp::Toggle(i)) => st.preset_open = if st.preset_open == Some(i) { None } else { Some(i) },
        Some(PresetOp::Duplicate(i)) => {
            let mut copy = set.beautify_presets[i].clone();
            copy.name = tf("settings.pipelines.copy_name", &[("name", &copy.name)]);
            set.beautify_presets.insert(i + 1, copy);
            st.preset_open = Some(i + 1);
        }
        Some(PresetOp::Delete(i)) => {
            set.beautify_presets.remove(i);
            st.preset_open = None;
        }
        None => {}
    }
    ui.add_space(space::XS);
    if widgets::Button::new(&s("pipelines.preset.new")).kind(ButtonKind::Secondary).icon(icon::PLUS).ui(ui).clicked() {
        set.beautify_presets.push(BeautifyPreset { name: s("pipelines.preset.new_name"), ..BeautifyPreset::default() });
        st.preset_open = Some(set.beautify_presets.len() - 1);
    }
    ui.add_space(space::XL);
}

fn preset_summary(p: &BeautifyPreset) -> String {
    let bg = match &p.background {
        Background::Transparent => s("pipelines.bg.transparent"),
        Background::Solid { color } => color.clone(),
        Background::Gradient { from, to, .. } => format!("{from} → {to}"),
        Background::BlurredCapture => s("pipelines.bg.blurred"),
        Background::Image { path } => path.rsplit(['/', '\\']).next().unwrap_or_default().to_owned(),
    };
    format!("{bg}  ·  {} px  ·  {} px", p.padding, p.corner_radius)
}

fn preset_form(
    ui: &mut Ui,
    cx: &mut ViewCx,
    idx: usize,
    p: &mut BeautifyPreset,
    backgrounds: &[(BgKind, String)],
    frames: &[(FrameStyle, String)],
    renames: &mut Vec<(String, String)>,
) {
    form_row(ui, &s("pipelines.preset.name"), |ui| {
        let before = p.name.clone();
        field(ui, &mut p.name, "", 240.0);
        if before != p.name {
            renames.push((before, p.name.clone()));
        }
    });
    let mut kind = BgKind::of(&p.background);
    form_row(ui, &s("pipelines.preset.background"), |ui| {
        dd(ui, ("bg-kind", idx), &mut kind, backgrounds);
    });
    if kind != BgKind::of(&p.background) {
        p.background = kind.fresh();
    }
    match &mut p.background {
        Background::Solid { color } => form_row(ui, &s("pipelines.preset.color"), |ui| color_well(ui, color, [28, 28, 30, 255], false)),
        Background::Gradient { from, to, angle_deg } => {
            form_row(ui, &s("pipelines.preset.colors"), |ui| {
                color_well(ui, from, [94, 92, 230, 255], false);
                ui.label(text::dim(icon::ARROW_RIGHT));
                color_well(ui, to, [10, 132, 255, 255], false);
            });
            form_row(ui, &s("pipelines.preset.angle"), |ui| {
                widgets::number_field(ui, angle_deg, 0.0..=360.0, "°");
            });
        }
        Background::Image { path } => form_row(ui, &s("pipelines.preset.image"), |ui| {
            kit::path_field_ltr(ui, cx, path, "", Pick::BackgroundImage { preset: idx }, 320.0);
        }),
        Background::Transparent | Background::BlurredCapture => {}
    }
    form_row(ui, &s("pipelines.preset.padding"), |ui| {
        widgets::number_field(ui, &mut p.padding, 0..=512, " px");
    });
    form_row(ui, &s("pipelines.preset.radius"), |ui| {
        widgets::number_field(ui, &mut p.corner_radius, 0..=128, " px");
    });
    form_row(ui, &s("pipelines.preset.shadow"), |ui| {
        widgets::number_field(ui, &mut p.shadow, 0..=128, " px");
    });
    form_row(ui, &s("pipelines.preset.aspect"), |ui| {
        let mut aspects: Vec<(Option<(u32, u32)>, String)> = vec![(None, s("pipelines.preset.aspect_free"))];
        for (w, h) in [(16, 9), (4, 3), (3, 2), (1, 1), (9, 16)] {
            aspects.push((Some((w, h)), format!("{w}:{h}")));
        }
        if let Some((w, h)) = p.aspect
            && !aspects.iter().any(|(a, _)| *a == Some((w, h)))
        {
            aspects.push((Some((w, h)), format!("{w}:{h}")));
        }
        dd(ui, ("aspect", idx), &mut p.aspect, &aspects);
    });
    form_row(ui, &s("pipelines.preset.frame"), |ui| {
        dd(ui, ("frame", idx), &mut p.frame, frames);
    });
}
