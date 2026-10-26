//! Placement of popups. An egui popup cannot leave its native window, so every popup is fitted into it: it hangs
//! below its anchor, or above it when there is more room there, never beside or over the anchor, and its body
//! scrolls when it is taller than the room on that side.

use egui::{Context, Popup, Rect, RectAlign, ScrollArea, Ui, pos2};

use super::title_bar::TITLE_BAR_HEIGHT;

/// Distance a popup keeps from the window edges.
const EDGE: f32 = 8.0;
/// The least height a popup gets, even from an anchor at the very edge of a tiny window.
const MIN_HEIGHT: f32 = 96.0;

/// The part of the window a popup hanging off `anchor` may cover: the window less a margin, and less the title bar
/// unless the anchor itself sits in it.
pub fn popup_bounds(ctx: &Context, anchor: Rect) -> Rect {
    let window = ctx.content_rect();
    let inner = window.shrink(EDGE);
    let below_title = window.top() + TITLE_BAR_HEIGHT;
    if anchor.top() >= below_title { Rect::from_min_max(pos2(inner.left(), inner.top().max(below_title)), inner.max) } else { inner }
}

/// Where a popup `want_h` tall goes when it hangs `gap` away from `anchor`, and the most height it may take there.
/// It goes below unless it does not fit there and there is more room above.
pub fn fit(ctx: &Context, anchor: Rect, gap: f32, want_h: f32) -> (RectAlign, f32) {
    let bounds = popup_bounds(ctx, anchor);
    let below = bounds.bottom() - anchor.bottom() - gap;
    let above = anchor.top() - bounds.top() - gap;
    if want_h <= below || below >= above {
        (RectAlign::BOTTOM_START, below.max(MIN_HEIGHT))
    } else {
        (RectAlign::TOP_START, above.max(MIN_HEIGHT))
    }
}

/// [`fit`] applied to `popup`: it opens below or above its anchor only, `gap` away. Returns the popup and the most
/// height it may take, frame included.
pub fn fitted(popup: Popup<'_>, gap: f32, want_h: f32) -> (Popup<'_>, f32) {
    let popup = popup.gap(gap);
    let Some(anchor) = popup.get_anchor_rect() else { return (popup, f32::INFINITY) };
    let (align, max_h) = fit(popup.ctx(), anchor, gap, want_h);
    (popup.align(align).align_alternatives(&[]), max_h)
}

/// A popup body that scrolls once it is taller than `max_h`. The scrolled minimum equals the maximum: a popup's area
/// is only as tall as it was last frame (or egui's 400 pt default on the sizing pass of each opening), and a scroll
/// area never grows past what is left of it, so the body would keep that height even with room for more. Shorter
/// content still shrinks the body to fit.
pub fn scroll_body<R>(ui: &mut Ui, max_h: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let max_h = max_h.max(0.0);
    ScrollArea::vertical().max_height(max_h).min_scrolled_height(max_h).auto_shrink([false, true]).show(ui, add).inner
}
