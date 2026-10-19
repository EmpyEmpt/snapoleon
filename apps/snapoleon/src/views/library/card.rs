//! One grid card: 16:10 thumbnail, app glyph, title, relative time, hover quick actions.

use egui::{
    Color32, CornerRadius, FontId, Id, Modifiers, Pos2, Rect, Role, Sense, Stroke, StrokeKind, TextureHandle, Ui, Vec2, WidgetInfo,
};
use snap_core::{CaptureKind, t};
use snap_history::Entry;
use snap_ui::{
    Palette, icon,
    text::{body_strong_style, caption_style},
    widgets::{tooltip, util},
};

use super::{format, paint};

pub const MIN_WIDTH: f32 = 216.0;
pub const GAP: f32 = 16.0;
const FOOTER: f32 = 56.0;
const RADIUS: f32 = 10.0;
const ACTION: f32 = 26.0;

pub fn thumb_height(width: f32) -> f32 {
    (width * 10.0 / 16.0).round()
}

pub fn height(width: f32) -> f32 {
    thumb_height(width) + FOOTER
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quick {
    Copy,
    Edit,
    Pin,
    Favorite,
    More,
}

pub enum Event {
    None,
    /// A click with the modifiers held at the time (selection logic lives in the view).
    Click(Modifiers),
    Open,
    Context(Pos2),
    /// A quick action; the rect is the button, for anchoring a menu.
    Quick(Quick, Rect),
}

pub struct Look {
    pub selected: bool,
    /// Several captures are selected: cards show their checkbox all the time.
    pub selecting: bool,
    /// Keep the quick actions visible (their menu is open).
    pub pinned_actions: bool,
    pub now_ms: i64,
}

pub fn show(ui: &mut Ui, rect: Rect, entry: &Entry, tex: Option<&TextureHandle>, look: &Look) -> Event {
    let p = Palette::of(ui);
    let id = ui.scope_id().with(("library-card", &entry.id.0));
    let resp = ui.interact(rect, id, Sense::CLICK);
    let hover = util::hover_anim(ui, &resp);
    let sel = util::anim(ui.ctx(), id.with("sel"), look.selected, util::Motion::HOVER);
    let pointer_inside = resp.contains_pointer() || look.pinned_actions;
    let recording = entry.kind == CaptureKind::Recording;

    let thumb = Rect::from_min_size(rect.min + Vec2::splat(1.0), Vec2::new(rect.width() - 2.0, thumb_height(rect.width()) - 1.0));
    let top_round = CornerRadius { nw: 9, ne: 9, sw: 0, se: 0 };
    let painter = ui.painter();
    painter.rect_filled(rect, RADIUS, p.surface);
    painter.rect_filled(thumb, top_round, p.surface_2);
    match tex {
        Some(tex) => paint::paint_texture(ui, tex, thumb, paint::cover_uv(tex.size_vec2(), thumb.size()), top_round),
        None => {
            let glyph = if recording { icon::FILM_STRIP } else { icon::IMAGE };
            util::paint_icon(ui, thumb.center(), glyph, 28.0, p.text_faint);
        }
    }
    let painter = ui.painter();
    painter.hline(rect.x_range().shrink(1.0), thumb.bottom() + 0.5, Stroke::new(1.0, p.border));

    // Footer: app tile, title, "app · time".
    let footer_top = thumb.bottom() + 1.0;
    let tile = Rect::from_min_size(Pos2::new(rect.left() + 12.0, footer_top + (FOOTER - 1.0 - 28.0) / 2.0), Vec2::splat(28.0));
    paint::app_tile(ui, &p, tile, entry);
    let text_left = tile.right() + 10.0;
    let text_w = rect.right() - 12.0 - text_left;
    let title = paint::title_of(entry);
    snap_ui::ensure_cjk(ui.ctx(), &title);
    resp.widget_info(|| WidgetInfo::selected(Role::Button, true, look.selected, &title));
    let title_g = paint::elided(ui, &title, body_strong_style().resolve(ui.style()), p.text, text_w);
    let when = format::relative(look.now_ms, entry.created_ms);
    let caption = match paint::app_of(entry) {
        Some(app) if app != title => format!("{app} · {when}"),
        _ => when,
    };
    let caption_g = paint::elided(ui, &caption, caption_style().resolve(ui.style()), p.text_dim, text_w);
    let block_h = title_g.size().y + 2.0 + caption_g.size().y;
    let mut y = tile.center().y - block_h / 2.0;
    ui.painter().galley(util::snap(ui, Pos2::new(text_left, y)), title_g.clone(), p.text);
    y += title_g.size().y + 2.0;
    ui.painter().galley(util::snap(ui, Pos2::new(text_left, y)), caption_g, p.text_dim);

    // Border last so thumbnails never overpaint it.
    let border = util::lerp_color(util::lerp_color(p.border, p.border_strong(), hover), p.accent, sel);
    ui.painter().rect_stroke(rect, RADIUS, Stroke::new(1.0 + sel, border), StrokeKind::Inside);

    if recording {
        duration_badge(ui, thumb, entry.duration_ms.unwrap_or(0));
    }

    let mut event = Event::None;
    if resp.double_clicked() {
        event = Event::Open;
    } else if resp.clicked() {
        event = Event::Click(ui.input(|i| i.modifiers));
    } else if resp.secondary_clicked() {
        event = Event::Context(resp.interact_pointer_pos().unwrap_or(rect.center()));
    }

    // Top-left: selection check and favorite star.
    let mut x = thumb.left() + 8.0;
    if pointer_inside || look.selecting || look.selected {
        let check = Rect::from_min_size(Pos2::new(x, thumb.top() + 8.0), Vec2::splat(22.0));
        let check_resp = ui.interact(check, id.with("check"), Sense::CLICK);
        let painter = ui.painter();
        if look.selected {
            painter.circle_filled(check.center(), 11.0, p.accent);
            util::paint_icon(ui, check.center(), icon::CHECK, 13.0, p.on_accent);
        } else {
            let wash = Color32::from_black_alpha(if check_resp.hovered() { 140 } else { 100 });
            painter.circle(check.center(), 10.5, wash, Stroke::new(1.5, Color32::from_white_alpha(210)));
        }
        if check_resp.clicked() {
            event = Event::Click(Modifiers::COMMAND);
        }
        x += 22.0 + 6.0;
    }
    if entry.favorite {
        let badge = Rect::from_min_size(Pos2::new(x, thumb.top() + 8.0), Vec2::splat(22.0));
        ui.painter().circle_filled(badge.center(), 11.0, Color32::from_black_alpha(110));
        paint::star(ui.painter(), badge.center(), 6.5, Some(p.gold), p.gold);
    }

    if pointer_inside && let Some(quick) = quick_actions(ui, &p, id, thumb, entry) {
        event = quick;
    }
    event
}

fn duration_badge(ui: &Ui, thumb: Rect, duration_ms: u64) {
    let galley = ui.painter().layout_no_wrap(format::duration(duration_ms), FontId::monospace(11.0), Color32::WHITE);
    let size = Vec2::new(galley.size().x + 12.0 + 14.0, 20.0);
    let rect = Rect::from_min_size(Pos2::new(thumb.right() - 8.0 - size.x, thumb.bottom() - 8.0 - size.y), size);
    ui.painter().rect_filled(rect, 10.0, Color32::from_black_alpha(150));
    util::paint_icon(ui, Pos2::new(rect.left() + 11.0, rect.center().y), icon::PLAY, 10.0, Color32::WHITE);
    ui.painter().galley(util::snap(ui, Pos2::new(rect.left() + 20.0, rect.center().y - galley.size().y / 2.0)), galley, Color32::WHITE);
}

/// The hover pill: copy, edit, pin, favorite, more.
fn quick_actions(ui: &mut Ui, p: &Palette, id: Id, thumb: Rect, entry: &Entry) -> Option<Event> {
    const ORDER: [Quick; 5] = [Quick::Copy, Quick::Edit, Quick::Pin, Quick::Favorite, Quick::More];
    let pill = Rect::from_min_size(
        Pos2::new(thumb.right() - 8.0 - (ACTION * 5.0 + 2.0 * 4.0 + 8.0), thumb.top() + 8.0),
        Vec2::new(ACTION * 5.0 + 2.0 * 4.0 + 8.0, ACTION + 8.0),
    );
    let painter = ui.painter();
    painter.add(p.shadow_small().as_shape(pill, 17.0));
    painter.rect_filled(pill, 17.0, p.elevated);
    painter.rect_stroke(pill, 17.0, Stroke::new(1.0, p.border), StrokeKind::Inside);

    let mut out = None;
    for (n, quick) in ORDER.into_iter().enumerate() {
        let rect = Rect::from_min_size(Pos2::new(pill.left() + 4.0 + n as f32 * (ACTION + 2.0), pill.top() + 4.0), Vec2::splat(ACTION));
        let resp = ui.interact(rect, id.with(("quick", n)), Sense::CLICK);
        let hover = util::hover_anim(ui, &resp);
        ui.painter().circle_filled(rect.center(), ACTION / 2.0, p.hover().gamma_multiply(hover));
        let fg = util::lerp_color(p.text_dim, p.text, hover);
        let (glyph, tip, hotkey) = match quick {
            Quick::Copy => (icon::COPY, t("library.action.copy"), Some("CmdOrCtrl+C")),
            Quick::Edit => (icon::PENCIL_SIMPLE, t("library.action.edit"), Some("Enter")),
            Quick::Pin => (icon::PUSH_PIN, t("library.action.pin"), Some("P")),
            Quick::Favorite => {
                (icon::STAR, if entry.favorite { t("library.action.unfavorite") } else { t("library.action.favorite") }, Some("F"))
            }
            Quick::More => (icon::DOTS_THREE, t("library.action.more"), None),
        };
        if quick == Quick::Favorite {
            let star_fg = if entry.favorite { p.gold } else { fg };
            paint::star(ui.painter(), rect.center(), 7.0, entry.favorite.then_some(p.gold), star_fg);
        } else {
            util::paint_icon(ui, rect.center(), glyph, 16.0, fg);
        }
        if tooltip(resp.clone(), &tip, hotkey).clicked() {
            out = Some(Event::Quick(quick, rect));
        }
    }
    out
}
