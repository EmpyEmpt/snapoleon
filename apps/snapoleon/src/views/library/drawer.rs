//! The right-hand detail drawer: preview, context, text, links, tags and actions for one capture.

use egui::{
    Align, Color32, CornerRadius, FontId, Layout, Margin, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, StrokeKind, TextureHandle, Ui,
    UiBuilder, Vec2,
};
use snap_core::{CaptureKind, t};
use snap_engine::LinkAccess;
use snap_history::{Entry, LinkRecord};
use snap_ui::{
    Palette, Tone, icon,
    text::{self, body_strong_style, body_style, caption_style},
    widgets::{self, Button, ButtonKind, TextField, badge, divider, icon_button, spinner, tooltip, util},
};
use snap_upload::server::RemoteItem;

use super::{Act, format, paint};

pub const WIDTH: f32 = 340.0;
const PAD: f32 = 16.0;

#[derive(Default)]
pub struct State {
    pub tag_input: String,
    pub confirm_delete: bool,
    pub delete_files: bool,
    /// The capture whose details are on screen; a different one starts at the top.
    shown: Option<snap_core::CaptureId>,
    /// The sharing options of one of the capture's team-server links, while they are open.
    pub manage: Option<Manage>,
}

/// What the server says about a link.
pub enum Remote {
    Loading,
    Found(RemoteItem),
    /// The server does not list it: deleted there, or a short link.
    Missing,
    Failed(String),
}

/// The sharing options of one link on a team server.
pub struct Manage {
    /// The capture the link belongs to.
    pub entry: String,
    /// Index into the capture's links.
    pub link: usize,
    pub remote: Remote,
    pub password: String,
    /// Hours; empty = never.
    pub expiry: String,
    pub confirm_delete: bool,
}

impl Manage {
    pub fn new(entry: &str, link: usize, expiry_hours: u32) -> Self {
        Self {
            entry: entry.to_owned(),
            link,
            remote: Remote::Loading,
            password: String::new(),
            expiry: if expiry_hours > 0 { expiry_hours.to_string() } else { String::new() },
            confirm_delete: false,
        }
    }
}

pub struct Props<'a> {
    pub entry: &'a Entry,
    pub preview: Option<&'a TextureHandle>,
    /// The preview is still being decoded.
    pub loading: bool,
    pub file_ok: bool,
    pub now_ms: i64,
    pub recognizing: bool,
    /// Text recognition is still being prepared ([`snap_ocr::warming_up`]).
    pub ocr_warming: bool,
    pub uploading: bool,
}

pub enum Out {
    Close,
    Act(Act),
    QuickLook,
    OpenUrl(String),
    CopyText(String),
    ShowText,
    /// The "copy as" menu of the link at this index, anchored to the button.
    LinkMenu(usize, Rect),
    /// The "more" menu of the link at this index, anchored to the button.
    LinkMore(usize, Rect),
    /// Share the capture again as a new link with this password and expiry.
    ShareAgain(LinkAccess),
    /// Delete the item on the server, for the link at this index.
    DeleteRemote {
        item: String,
        link: usize,
    },
    MoreMenu(Rect),
    SetTags(Vec<String>),
    /// The user confirmed; whether to remove the files too is in the drawer state.
    Delete,
}

pub fn show(ui: &mut Ui, state: &mut State, props: &Props) -> Vec<Out> {
    let p = Palette::of(ui);
    let rect = ui.max_rect();
    ui.painter().rect_filled(rect, 0.0, p.surface);
    ui.painter().vline(rect.left() + 0.5, rect.y_range(), Stroke::new(1.0, p.border));

    let mut out = Vec::new();
    let inner = Rect::from_min_max(Pos2::new(rect.left() + 1.0 + PAD, rect.top()), Pos2::new(rect.right() - PAD, rect.bottom()));
    let mut body = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Min)));
    body.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
    let mut scroll = ScrollArea::vertical().id_salt("library-drawer").auto_shrink([false, false]);
    if state.shown.as_ref() != Some(&props.entry.id) {
        state.shown = Some(props.entry.id.clone());
        state.manage = None;
        scroll = scroll.vertical_scroll_offset(0.0);
    }
    scroll.show(&mut body, |ui| {
        ui.set_width(inner.width());
        ui.add_space(PAD);
        content(ui, &p, state, props, &mut out);
        ui.add_space(PAD);
    });
    out
}

fn content(ui: &mut Ui, p: &Palette, state: &mut State, props: &Props, out: &mut Vec<Out>) {
    let entry = props.entry;
    let recording = entry.kind == CaptureKind::Recording;
    let width = ui.available_width();

    preview(ui, p, props, width, out);
    ui.add_space(4.0);

    // Title, with the favorite toggle.
    ui.horizontal_top(|ui| {
        let title_w = width - 28.0 - 8.0;
        let galley = paint::clamped(ui, &paint::title_of(entry), text::title_style().resolve(ui.style()), p.text, title_w, 2);
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(title_w, galley.size().y), Sense::hover());
        ui.painter().galley(rect.min, galley.clone(), p.text);
        if galley.rows.len() > 1 || galley.job.text.ends_with('…') {
            resp.on_hover_text(paint::title_of(entry));
        }
        let (star_rect, star_resp) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::CLICK);
        let hover = util::hover_anim(ui, &star_resp);
        ui.painter().circle_filled(star_rect.center(), 14.0, p.hover().gamma_multiply(hover));
        let fg = if entry.favorite { p.gold } else { util::lerp_color(p.text_dim, p.text, hover) };
        paint::star(ui.painter(), star_rect.center(), 8.0, entry.favorite.then_some(p.gold), fg);
        let tip = if entry.favorite { t("library.action.unfavorite") } else { t("library.action.favorite") };
        if tooltip(star_resp, &tip, Some("F")).clicked() {
            out.push(Out::Act(Act::Favorite));
        }
    });
    let kind = snap_history::kind_label(entry.kind);
    ui.label(text::caption(format!("{kind} · {}", format::relative(props.now_ms, entry.created_ms))));
    ui.add_space(4.0);

    if !props.file_ok {
        missing_notice(ui, p);
    }
    actions(ui, props, recording, out);
    ui.add_space(4.0);
    divider(ui);
    ui.add_space(4.0);

    info(ui, p, entry, out);

    if !recording {
        ui.add_space(4.0);
        divider(ui);
        ui.add_space(4.0);
        text_block(ui, p, props, out);
    }
    if !entry.links.is_empty() {
        ui.add_space(4.0);
        divider(ui);
        ui.add_space(4.0);
        links(ui, p, state, props, out);
    }
    ui.add_space(4.0);
    divider(ui);
    ui.add_space(4.0);
    tags(ui, p, state, entry, out);
    ui.add_space(8.0);
    divider(ui);
    ui.add_space(4.0);
    delete_block(ui, p, state, out);
}

fn preview(ui: &mut Ui, p: &Palette, props: &Props, width: f32, out: &mut Vec<Out>) {
    let entry = props.entry;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, (width * 0.625).round()), Sense::CLICK);
    let hover = util::hover_anim(ui, &resp);
    let painter = ui.painter();
    painter.rect_filled(rect, 10.0, p.surface_2);
    match props.preview {
        Some(tex) => {
            let target = paint::fit(tex.size_vec2(), rect.shrink(1.0), 1.0);
            paint::paint_texture(ui, tex, target, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), CornerRadius::same(4));
        }
        None if props.loading => spinner_at(ui, rect.center()),
        None => {
            let glyph = if entry.kind == CaptureKind::Recording { icon::FILM_STRIP } else { icon::IMAGE };
            util::paint_icon(ui, rect.center(), glyph, 32.0, p.text_faint);
        }
    }
    if entry.kind == CaptureKind::Recording && props.preview.is_some() {
        let c = rect.center();
        ui.painter().circle_filled(c, 22.0, Color32::from_black_alpha(140));
        util::paint_icon(ui, c, icon::PLAY, 20.0, Color32::WHITE);
    }
    ui.painter().rect_stroke(rect, 10.0, Stroke::new(1.0, util::lerp_color(p.border, p.border_strong(), hover)), StrokeKind::Inside);

    // Close button, floating on the preview.
    let close = Rect::from_min_size(Pos2::new(rect.right() - 8.0 - 26.0, rect.top() + 8.0), Vec2::splat(26.0));
    let close_resp = ui.interact(close, resp.id.with("close"), Sense::CLICK);
    let h = util::hover_anim(ui, &close_resp);
    ui.painter().circle_filled(close.center(), 13.0, Color32::from_black_alpha((110.0 + 60.0 * h) as u8));
    // Drawn as two strokes rather than the icon glyph, which did not show on the dark disc over the preview.
    let (c, arm) = (close.center(), 4.5);
    for d in [Vec2::new(arm, arm), Vec2::new(arm, -arm)] {
        ui.painter().line_segment([c - d, c + d], Stroke::new(1.6, Color32::WHITE));
    }
    if tooltip(close_resp, &t("library.drawer.close"), Some("Esc")).clicked() {
        out.push(Out::Close);
    }
    if tooltip(resp.on_hover_cursor(egui::CursorIcon::PointingHand), &t("library.action.quick_look"), Some("Space")).clicked() {
        out.push(Out::QuickLook);
    }
}

fn spinner_at(ui: &mut Ui, center: Pos2) {
    let rect = Rect::from_center_size(center, Vec2::splat(20.0));
    ui.scope_builder(UiBuilder::new().max_rect(rect), spinner);
}

fn missing_notice(ui: &mut Ui, p: &Palette) {
    egui::Frame::new().fill(p.tone_soft(Tone::Warning)).corner_radius(8).inner_margin(Margin::symmetric(10, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::WARNING).color(p.warning));
            ui.add(egui::Label::new(RichText::new(t("library.drawer.missing")).color(p.warning).size(12.0)).wrap());
        });
    });
}

/// The primary action and the icon buttons beside it.
fn actions(ui: &mut Ui, props: &Props, recording: bool, out: &mut Vec<Out>) {
    let editable = props.file_ok || props.entry.project.is_some();
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = 28.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let label = if recording { t("library.action.edit_video") } else { t("library.action.edit") };
            let edit = ui.add_enabled(editable, Button::new(&label).kind(ButtonKind::Primary).icon(icon::PENCIL_SIMPLE));
            if tooltip(edit, "", Some("Enter")).clicked() {
                out.push(Out::Act(Act::Edit));
            }
            ui.add_space(4.0);
            ui.add_enabled_ui(props.file_ok, |ui| {
                if icon_button(ui, icon::COPY, &t("library.action.copy"), Some("CmdOrCtrl+C")).clicked() {
                    out.push(Out::Act(Act::Copy));
                }
                if !recording && icon_button(ui, icon::PUSH_PIN, &t("library.action.pin"), Some("P")).clicked() {
                    out.push(Out::Act(Act::Pin));
                }
                if props.uploading {
                    ui.add_space(6.0);
                    spinner(ui);
                    ui.add_space(6.0);
                } else if icon_button(ui, icon::CLOUD_ARROW_UP, &t("library.action.upload"), Some("U")).clicked() {
                    out.push(Out::Act(Act::Upload));
                }
            });
            if icon_button(ui, icon::FOLDER_OPEN, &t("library.action.reveal"), None).clicked() {
                out.push(Out::Act(Act::Reveal));
            }
            let more = icon_button(ui, icon::DOTS_THREE, &t("library.action.more"), None);
            if more.clicked() {
                out.push(Out::MoreMenu(more.rect));
            }
        });
    });
}

fn info_row(ui: &mut Ui, p: &Palette, label: &str, value: impl FnOnce(&mut Ui, f32)) {
    const LABEL_W: f32 = 84.0;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(LABEL_W, 18.0), Sense::hover());
        let galley = paint::elided(ui, label, caption_style().resolve(ui.style()), p.text_dim, LABEL_W);
        ui.painter().galley(util::snap(ui, Pos2::new(rect.left(), rect.center().y - galley.size().y / 2.0)), galley, p.text_dim);
        let w = ui.available_width();
        ui.vertical(|ui| {
            ui.set_width(w);
            ui.spacing_mut().item_spacing.y = 0.0;
            value(ui, w);
        });
    });
}

fn plain(ui: &mut Ui, p: &Palette, value: &str, width: f32, mono: bool) {
    let font = if mono { FontId::monospace(12.0) } else { body_style().resolve(ui.style()) };
    let galley = paint::clamped(ui, value, font, p.text, width, 2);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, galley.size().y.max(18.0)), Sense::hover());
    ui.painter().galley(util::snap(ui, Pos2::new(rect.left(), rect.center().y - galley.size().y / 2.0)), galley, p.text);
    resp.on_hover_text(value);
}

fn info(ui: &mut Ui, p: &Palette, entry: &Entry, out: &mut Vec<Out>) {
    ui.spacing_mut().item_spacing.y = 6.0;
    let ctx = &entry.context;
    let clean = |s: &Option<String>| s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
    if let Some(app) = clean(&ctx.app_name) {
        info_row(ui, p, &t("library.info.app"), |ui, w| plain(ui, p, &app, w, false));
    }
    if let Some(window) = clean(&ctx.window_title).filter(|w| paint::title_of(entry) != *w || ctx.app_name.is_none()) {
        info_row(ui, p, &t("library.info.window"), |ui, w| plain(ui, p, &window, w, false));
    }
    if let Some(url) = clean(&ctx.url) {
        info_row(ui, p, &t("library.info.url"), |ui, w| {
            let font = body_style().resolve(ui.style());
            let galley = paint::clamped(ui, &url, font, p.accent, w, 2);
            let (rect, resp) = ui.allocate_exact_size(galley.size().max(Vec2::new(w.min(40.0), 18.0)), Sense::CLICK);
            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            ui.painter().galley(util::snap(ui, rect.min), galley, p.accent);
            if resp.hovered() {
                ui.painter().hline(rect.x_range(), rect.bottom() - 1.0, Stroke::new(1.0, p.accent));
            }
            if resp.on_hover_text(&url).clicked() {
                out.push(Out::OpenUrl(url.clone()));
            }
        });
    }
    if entry.width > 0 {
        info_row(ui, p, &t("library.info.size"), |ui, w| plain(ui, p, &format!("{} × {}", entry.width, entry.height), w, true));
    }
    if entry.bytes > 0 {
        info_row(ui, p, &t("library.info.file_size"), |ui, w| plain(ui, p, &format::bytes(entry.bytes), w, true));
    }
    if let Some(ms) = entry.duration_ms {
        info_row(ui, p, &t("library.info.duration"), |ui, w| plain(ui, p, &format::duration(ms), w, true));
    }
    info_row(ui, p, &t("library.info.captured"), |ui, w| plain(ui, p, &format::date_time(entry.created_ms), w, false));
    if let Some(name) = entry.file.as_ref().and_then(|f| f.file_name()) {
        let full = entry.file.as_ref().map(|f| f.display().to_string()).unwrap_or_default();
        info_row(ui, p, &t("library.info.file"), |ui, w| {
            let galley = paint::elided(ui, &name.to_string_lossy(), FontId::monospace(12.0), p.text, w);
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, galley.size().y.max(18.0)), Sense::hover());
            ui.painter().galley(util::snap(ui, Pos2::new(rect.left(), rect.center().y - galley.size().y / 2.0)), galley, p.text);
            resp.on_hover_text(full);
        });
    }
}

fn section_header(ui: &mut Ui, p: &Palette, title: &str, trailing: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).text_style(body_strong_style()).color(p.text));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            trailing(ui);
        });
    });
}

fn text_block(ui: &mut Ui, p: &Palette, props: &Props, out: &mut Vec<Out>) {
    let entry = props.entry;
    let text = entry.text.as_deref().map(str::trim).filter(|s| !s.is_empty());
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = 28.0;
        section_header(ui, p, &t("library.text.title"), |ui| {
            if let Some(text) = text {
                if icon_button(ui, icon::ARROWS_OUT, &t("library.text.open"), None).clicked() {
                    out.push(Out::ShowText);
                }
                if icon_button(ui, icon::COPY, &t("library.text.copy"), None).clicked() {
                    out.push(Out::CopyText(text.to_owned()));
                }
            }
        });
    });
    match text {
        Some(text) => {
            snap_ui::ensure_cjk(ui.ctx(), text);
            egui::Frame::new().fill(p.surface_2).corner_radius(10).inner_margin(Margin::same(10)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let galley = paint::clamped(ui, text, body_style().resolve(ui.style()), p.text, ui.available_width(), 7);
                let (rect, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
                ui.painter().galley(util::snap(ui, rect.min), galley, p.text);
            });
        }
        None if props.recognizing => {
            ui.horizontal(|ui| {
                spinner(ui);
                ui.label(text::dim(t(if props.ocr_warming { "ocr.warmup.label" } else { "library.text.recognizing" })));
            });
            if props.ocr_warming {
                ui.label(text::caption(t("ocr.warmup.hint")));
            }
        }
        None => {
            ui.label(text::caption(t("library.text.none")));
            let button = ui.add_enabled(props.file_ok, Button::new(&t("library.text.recognize")).icon(icon::TEXT_AA));
            if button.clicked() {
                out.push(Out::Act(Act::Recognize));
            }
        }
    }
}

fn links(ui: &mut Ui, p: &Palette, state: &mut State, props: &Props, out: &mut Vec<Out>) {
    let entry = props.entry;
    section_header(ui, p, &t("library.links.title"), |ui| {
        badge(ui, &entry.links.len().to_string(), Tone::Neutral);
    });
    for (n, link) in entry.links.iter().enumerate() {
        link_row(ui, p, n, link, out);
    }
    if let Some(manage) = &mut state.manage
        && !manage_panel(ui, p, manage, props.file_ok, out)
    {
        state.manage = None;
    }
}

/// The sharing options of a team-server link: what the server knows, a new link with a password and expiry, and
/// deleting it there. The server fixes password and expiry when a file arrives, so changing them means a new link.
/// False once the user closes the panel.
fn manage_panel(ui: &mut Ui, p: &Palette, m: &mut Manage, file_ok: bool, out: &mut Vec<Out>) -> bool {
    let mut open = true;
    egui::Frame::new().fill(p.surface_2).corner_radius(10).inner_margin(Margin::same(12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new(t("library.remote.title")).text_style(body_strong_style()).color(p.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if icon_button(ui, icon::X, &t("library.drawer.close"), None).clicked() {
                    open = false;
                }
            });
        });
        match &m.remote {
            Remote::Loading => {
                ui.horizontal(|ui| {
                    spinner(ui);
                    ui.label(text::dim(t("library.remote.loading")));
                });
            }
            Remote::Failed(message) => {
                ui.add(egui::Label::new(RichText::new(message).color(p.warning).size(12.0)).wrap());
            }
            Remote::Missing => {
                ui.add(egui::Label::new(text::caption(t("library.remote.missing"))).wrap());
            }
            Remote::Found(item) => remote_facts(ui, p, item),
        }

        ui.add_space(4.0);
        ui.label(text::caption(t("library.remote.password")));
        let hint = t("library.remote.password_hint");
        ui.add(TextField::new(&mut m.password).hint(&hint).icon(icon::LOCK).password(true).width(ui.available_width()));
        ui.label(text::caption(t("library.remote.expiry")));
        let hint = t("library.remote.expiry_hint");
        ui.add(TextField::new(&mut m.expiry).hint(&hint).icon(icon::CLOCK).width(ui.available_width()));
        m.expiry.retain(|c| c.is_ascii_digit());
        let share = ui.add_enabled(file_ok, Button::new(&t("library.remote.share_again")).icon(icon::CLOUD_ARROW_UP));
        if tooltip(share, &t("library.remote.share_again_tip"), None).clicked() {
            let hours = m.expiry.parse::<u32>().unwrap_or(0);
            let password = Some(m.password.clone()).filter(|p| !p.is_empty());
            out.push(Out::ShareAgain(LinkAccess { password, expiry_hours: Some(hours) }));
        }

        if let Remote::Found(item) = &m.remote {
            ui.add_space(4.0);
            divider(ui);
            ui.add_space(4.0);
            if !m.confirm_delete {
                let delete = ui.add(Button::new(&t("library.remote.delete")).kind(ButtonKind::Ghost).icon(icon::TRASH));
                m.confirm_delete = delete.clicked();
            } else {
                ui.label(RichText::new(t("library.remote.delete_hint")).color(p.danger).size(12.0));
                ui.horizontal(|ui| {
                    if ui.add(Button::new(&t("library.delete.confirm")).kind(ButtonKind::Danger)).clicked() {
                        out.push(Out::DeleteRemote { item: item.id.clone(), link: m.link });
                    }
                    if widgets::secondary_button(ui, &t("common.cancel")).clicked() {
                        m.confirm_delete = false;
                    }
                });
            }
        }
    });
    open
}

/// Views, comments, password and expiry as the server reports them.
fn remote_facts(ui: &mut Ui, p: &Palette, item: &RemoteItem) {
    let yes_no = |on: bool| if on { t("library.remote.password_set") } else { t("library.remote.password_none") };
    let expires = match item.expires_ms {
        Some(ms) => format::date_time(ms),
        None => t("library.remote.never").into_owned(),
    };
    let facts = [
        (t("library.remote.views"), item.views.to_string()),
        (t("library.remote.comments"), item.comments.to_string()),
        (t("library.remote.password"), yes_no(item.has_password).into_owned()),
        (t("library.remote.expires"), expires),
    ];
    for (label, value) in facts {
        info_row(ui, p, &label, |ui, w| plain(ui, p, &value, w, false));
    }
}

fn link_row(ui: &mut Ui, p: &Palette, n: usize, link: &LinkRecord, out: &mut Vec<Out>) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 48.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 10.0, p.surface_2);
    painter.rect_stroke(rect, 10.0, Stroke::new(1.0, p.border), StrokeKind::Inside);
    let buttons_w = 28.0 * 3.0 + 4.0 * 2.0 + 8.0;
    let text_w = rect.width() - 12.0 - buttons_w - 4.0;
    let url = paint::elided(ui, &link.url, FontId::monospace(11.5), p.text, text_w);
    let sub = format!("{} · {}", link.destination, format::short_date(link.created_ms, snap_core::now_ms()));
    let sub = paint::elided(ui, &sub, caption_style().resolve(ui.style()), p.text_dim, text_w);
    let block = url.size().y + 2.0 + sub.size().y;
    let mut y = rect.center().y - block / 2.0;
    ui.painter().galley(util::snap(ui, Pos2::new(rect.left() + 12.0, y)), url.clone(), p.text);
    y += url.size().y + 2.0;
    ui.painter().galley(util::snap(ui, Pos2::new(rect.left() + 12.0, y)), sub, p.text_dim);
    let row_id = resp.id;
    resp.on_hover_text(&link.url);

    let slot = |k: f32| {
        Rect::from_min_size(Pos2::new(rect.right() - 8.0 - 28.0 * (3.0 - k) - 4.0 * (2.0 - k), rect.center().y - 14.0), Vec2::splat(28.0))
    };
    for (k, (glyph, tip)) in [
        (icon::COPY, t("library.links.copy_as")),
        (icon::ARROW_SQUARE_OUT, t("library.links.open")),
        (icon::DOTS_THREE, t("library.action.more")),
    ]
    .into_iter()
    .enumerate()
    {
        let r = slot(k as f32);
        let button = ui.interact(r, row_id.with(("link-button", k)), Sense::CLICK);
        let hover = util::hover_anim(ui, &button);
        ui.painter().rect_filled(r, 6.0, p.hover().gamma_multiply(hover));
        util::paint_icon(ui, r.center(), glyph, 15.0, util::lerp_color(p.text_dim, p.text, hover));
        if tooltip(button, &tip, None).clicked() {
            out.push(match k {
                0 => Out::LinkMenu(n, r),
                1 => Out::OpenUrl(link.url.clone()),
                _ => Out::LinkMore(n, r),
            });
        }
    }
}

fn tags(ui: &mut Ui, p: &Palette, state: &mut State, entry: &Entry, out: &mut Vec<Out>) {
    section_header(ui, p, &t("library.tags.title"), |_| ());
    if !entry.tags.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
            for tag in &entry.tags {
                if tag_pill(ui, p, tag) {
                    out.push(Out::SetTags(entry.tags.iter().filter(|t| *t != tag).cloned().collect()));
                }
            }
        });
    }
    let hint = t("library.tags.add");
    let field = ui.add(TextField::new(&mut state.tag_input).hint(&hint).icon(icon::TAG).width(ui.available_width()));
    if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        let tag = state.tag_input.trim().to_owned();
        state.tag_input.clear();
        if !tag.is_empty() && !entry.tags.iter().any(|t| t.eq_ignore_ascii_case(&tag)) {
            let mut all = entry.tags.clone();
            all.push(tag);
            out.push(Out::SetTags(all));
        }
        field.request_focus();
    }
}

/// A tag chip with a remove button; true when removed.
fn tag_pill(ui: &mut Ui, p: &Palette, tag: &str) -> bool {
    let galley = ui.painter().layout_no_wrap(tag.to_owned(), body_style().resolve(ui.style()), p.text);
    let size = Vec2::new(galley.size().x + 12.0 + 22.0, 24.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::CLICK);
    let close = Rect::from_center_size(Pos2::new(rect.right() - 13.0, rect.center().y), Vec2::splat(18.0));
    let hover_close = ui.rect_contains_pointer(close);
    ui.painter().rect_filled(rect, 12.0, p.accent_soft);
    ui.painter().galley(util::snap(ui, Pos2::new(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0)), galley, p.text);
    if hover_close {
        ui.painter().circle_filled(close.center(), 9.0, p.hover());
    }
    util::paint_icon(ui, close.center(), icon::X, 11.0, if hover_close { p.text } else { p.text_dim });
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && hover_close
}

fn delete_block(ui: &mut Ui, p: &Palette, state: &mut State, out: &mut Vec<Out>) {
    if !state.confirm_delete {
        let button = ui.add(Button::new(&t("library.action.delete")).kind(ButtonKind::Ghost).icon(icon::TRASH));
        if tooltip(button, "", Some("Del")).clicked() {
            state.confirm_delete = true;
        }
        return;
    }
    egui::Frame::new().fill(p.tone_soft(Tone::Danger)).corner_radius(10).inner_margin(Margin::same(12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(t("library.delete.one")).text_style(body_strong_style()).color(p.text));
        ui.label(text::caption(t("library.delete.one_hint")));
        widgets::checkbox(ui, &mut state.delete_files, &t("library.delete.files"));
        ui.horizontal(|ui| {
            if ui.add(Button::new(&t("library.delete.confirm")).kind(ButtonKind::Danger)).clicked() {
                out.push(Out::Delete);
            }
            if widgets::secondary_button(ui, &t("common.cancel")).clicked() {
                state.confirm_delete = false;
            }
        });
    });
}
