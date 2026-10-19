//! The virtualized card grid, its empty states and the floating layers above it (selection bar, quick look, menus).

use egui::{Align2, Area, Color32, Context, CornerRadius, Id, Order, Rect, ScrollArea, Sense, Ui, Vec2};
use snap_core::{hotkey::Action, settings::LinkFormat, t, tf};
use snap_ui::{
    icon,
    widgets::{self, Button, ButtonKind, empty_state_brand, empty_state_with, kbd, spinner, toolbar_pill, util},
};

use super::{Act, LibraryView, Load, MenuKind, card, format, menu, paint};
use crate::commands::Command;
use crate::views::ViewCx;

/// Side padding of the grid and the filter bar.
pub const PAD: f32 = 16.0;
const TOP: f32 = 4.0;

impl LibraryView {
    pub(super) fn grid(&mut self, ui: &mut Ui, cx: &mut ViewCx) {
        let area = ui.available_rect_before_wrap();
        if self.entries.is_empty() {
            self.empty_state(ui, cx, area);
            return;
        }
        let now_ms = snap_core::now_ms();
        let n = self.entries.len();
        let inner_w = (area.width() - PAD * 2.0).max(card::MIN_WIDTH);
        let cols = (((inner_w + card::GAP) / (card::MIN_WIDTH + card::GAP)).floor() as usize).max(1);
        let card_w = ((inner_w - card::GAP * (cols - 1) as f32) / cols as f32).floor();
        let row_h = card::height(card_w) + card::GAP;
        let rows = n.div_ceil(cols);
        self.cols = cols;
        // Room under the last row for the loading indicator, and for the selection bar floating above the grid.
        let bottom = if self.selected.len() > 1 {
            88.0
        } else if self.more {
            56.0
        } else {
            PAD
        };
        let total_h = TOP + rows as f32 * row_h - card::GAP + bottom;
        let card_rect = |idx: usize, origin: egui::Pos2| {
            let (r, c) = (idx / cols, idx % cols);
            Rect::from_min_size(
                origin + Vec2::new(c as f32 * (card_w + card::GAP), r as f32 * row_h),
                Vec2::new(card_w, card::height(card_w)),
            )
        };

        let menu_card = self.menu.as_ref().filter(|m| matches!(m.kind, MenuKind::Entry)).map(|m| m.id.clone());
        let scroll_to = self.scroll_to.take();
        let mut scroll = ScrollArea::vertical().id_salt("library-grid").auto_shrink([false, false]);
        if std::mem::take(&mut self.scroll_top) {
            scroll = scroll.vertical_scroll_offset(0.0);
        }

        let mut events = Vec::new();
        let mut last_row = 0;
        let mut visible_rows = 1;
        let mut background_clicked = false;
        let Self { entries, thumbs, selected, more, .. } = self;
        let scrolled = scroll.show_viewport(ui, |ui, viewport| {
            let (content, background) = ui.allocate_exact_size(Vec2::new(area.width(), total_h), Sense::CLICK);
            background_clicked = background.clicked();
            let origin = content.min + Vec2::new(PAD, TOP);
            visible_rows = ((viewport.height() / row_h).floor() as usize).max(1);
            let first = ((viewport.min.y - TOP) / row_h).floor().max(0.0) as usize;
            last_row = (((viewport.max.y - TOP) / row_h).ceil().max(0.0) as usize).min(rows - 1);
            let visible = (first * cols).min(n)..((last_row + 1) * cols).min(n);
            for (idx, entry) in entries.iter().enumerate().take(visible.end).skip(visible.start) {
                let tex = thumbs.get(entry).cloned();
                let look = card::Look {
                    selected: selected.contains(&entry.id.0),
                    selecting: selected.len() > 1,
                    pinned_actions: menu_card.as_deref() == Some(entry.id.0.as_str()),
                    now_ms,
                };
                let event = card::show(ui, card_rect(idx, origin), entry, tex.as_ref(), &look);
                if !matches!(event, card::Event::None) {
                    events.push((idx, event));
                }
            }
            if let Some(idx) = scroll_to {
                ui.scroll_to_rect(card_rect(idx, origin).expand2(Vec2::new(0.0, card::GAP)), None);
            }
            if *more {
                let at = egui::pos2(content.center().x, content.bottom() - bottom / 2.0 - 4.0);
                ui.scope_builder(egui::UiBuilder::new().max_rect(Rect::from_center_size(at, Vec2::splat(20.0))), spinner);
            }
        });
        self.visible_rows = visible_rows;
        // A hairline under the filters while cards scroll beneath them.
        if scrolled.state.clamped_offset().y > 1.0 {
            let border = snap_ui::palette(ui.ctx()).border;
            ui.painter().hline(area.x_range(), area.top() + 0.5, egui::Stroke::new(1.0, border));
        }

        if self.more && !self.loading && last_row + 6 >= rows {
            self.load(cx, Load::More);
        }
        let had_event = !events.is_empty();
        for (idx, event) in events {
            self.card_event(cx, idx, event);
        }
        if background_clicked && !had_event && !ui.input(|i| i.modifiers.command || i.modifiers.shift) {
            self.clear_selection();
        }
    }

    fn card_event(&mut self, cx: &mut ViewCx, idx: usize, event: card::Event) {
        let Some(entry) = self.entries.get(idx).cloned() else { return };
        let id = entry.id.0.clone();
        match event {
            card::Event::None => {}
            card::Event::Click(mods) => self.click(&id, mods),
            card::Event::Open => {
                self.select_only(&id);
                self.run(cx, Act::Edit, vec![entry]);
            }
            card::Event::Drag(pos) => self.drag_out(cx, &id, pos),
            card::Event::Context(pos) => {
                if !self.selected.contains(&id) {
                    self.select_only(&id);
                }
                self.menu = Some(super::OpenMenu { kind: MenuKind::Entry, id, anchor: menu::Anchor::Point(pos) });
            }
            card::Event::Quick(quick, rect) => {
                let targets = self.targets_for(&id);
                match quick {
                    card::Quick::Copy => self.run(cx, Act::Copy, targets),
                    card::Quick::Edit => self.run(cx, Act::Edit, vec![entry]),
                    card::Quick::Pin => self.run(cx, Act::Pin, targets),
                    card::Quick::Favorite => self.run(cx, Act::Favorite, targets),
                    card::Quick::More => {
                        if !self.selected.contains(&id) {
                            self.select_only(&id);
                        }
                        self.toggle_menu(MenuKind::Entry, &id, menu::Anchor::Widget(rect));
                    }
                }
            }
        }
    }

    /// Drag the files of card `id` (or of the whole selection it belongs to) into another program. The OS runs the
    /// drag in a loop of its own and takes the mouse button release with it, so egui is told about the release after.
    fn drag_out(&mut self, cx: &mut ViewCx, id: &str, pos: egui::Pos2) {
        if !self.selected.contains(id) {
            self.select_only(id);
        }
        let files: Vec<std::path::PathBuf> = self.targets_for(id).into_iter().filter_map(|e| e.file).filter(|f| f.exists()).collect();
        if files.is_empty() {
            return cx.toast(snap_ui::ToastKind::Error, t("library.error.missing"));
        }
        if let Err(e) = snap_platform::drag::files(&files) {
            cx.toast_error(&e);
        }
        let event = egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() };
        cx.shell.push_input_event(cx.win, event);
    }

    /// The captures an action started from card `id` applies to: the whole selection when the card is part of it.
    pub(super) fn targets_for(&self, id: &str) -> Vec<snap_history::Entry> {
        if self.selected.contains(id) && self.selected.len() > 1 {
            self.selected_entries()
        } else {
            self.entries.iter().find(|e| e.id.0 == id).cloned().into_iter().collect()
        }
    }

    fn empty_state(&mut self, ui: &mut Ui, cx: &mut ViewCx, area: Rect) {
        if self.loading {
            ui.add_space((area.height() / 2.0 - 40.0).max(0.0));
            ui.vertical_centered(spinner);
            return;
        }
        ui.add_space(((area.height() - 220.0) / 2.0).max(0.0));
        if cx.engine.history().is_none() {
            empty_state_with(ui, icon::CLOCK_COUNTER_CLOCKWISE, &t("library.off.title"), &t("library.off.body"), |ui| {
                if widgets::secondary_button(ui, &t("library.off.action")).clicked() {
                    cx.push(Command::OpenSettings { page: Some(crate::views::settings::SettingsPage::History) });
                }
            });
        } else if self.filter.is_active() || !self.applied_search.is_empty() {
            empty_state_with(ui, icon::MAGNIFYING_GLASS, &t("library.none.title"), &t("library.none.body"), |ui| {
                if widgets::secondary_button(ui, &t("library.none.action")).clicked() {
                    self.filter = Default::default();
                    self.search.clear();
                    self.applied_search.clear();
                    self.load(cx, Load::Reset);
                }
            });
        } else {
            let body = match capture_hotkey(cx) {
                Some(key) => tf("library.empty.body", &[("key", &key)]),
                None => t("library.empty.body_nokey").into_owned(),
            };
            empty_state_brand(ui, &t("library.empty.title"), &body, |ui| {
                let capture = ui.add(Button::new(&t("action.capture_region")).kind(ButtonKind::Primary).icon(icon::SELECTION));
                if capture.clicked() {
                    cx.push(Command::StartCapture { action: Action::CaptureRegion, pipeline: None });
                }
            });
        }
    }

    // --- layers above the window ---------------------------------------------------------------------------------

    pub(super) fn overlays(&mut self, cx: &mut ViewCx, ctx: &Context) {
        if self.selected.len() > 1 {
            self.selection_bar(cx, ctx);
        }
        if self.quick_look {
            self.quick_look_layer(ctx);
        }
        self.menu_layer(cx, ctx);
    }

    fn selection_bar(&mut self, cx: &mut ViewCx, ctx: &Context) {
        let p = snap_ui::palette(ctx);
        let count = self.selected.len();
        let entries = self.selected_entries();
        let combinable = entries.iter().filter(|e| e.kind != snap_core::CaptureKind::Recording).count() >= 2;
        let all_favorite = entries.iter().all(|e| e.favorite);
        let mut act = None;
        let (mut combine, mut clear, mut delete, mut cancel) = (false, false, false, false);
        Area::new(Id::unique("library-selection-bar")).order(Order::Foreground).anchor(Align2::CENTER_BOTTOM, [0.0, -20.0]).show(
            ctx,
            |ui| {
                toolbar_pill(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.add_space(10.0);
                    if self.bulk_confirm {
                        ui.label(egui::RichText::new(format::counted("library.delete.many", count as u64)).color(p.text));
                        ui.add_space(6.0);
                        widgets::checkbox(ui, &mut self.drawer.delete_files, &t("library.delete.files"));
                        ui.add_space(6.0);
                        cancel = ui.add(Button::new(&t("common.cancel")).kind(ButtonKind::Ghost)).clicked();
                        delete = ui.add(Button::new(&t("library.delete.confirm")).kind(ButtonKind::Danger).icon(icon::TRASH)).clicked();
                        return;
                    }
                    ui.label(
                        egui::RichText::new(format::counted("library.selected", count as u64))
                            .text_style(snap_ui::text::body_strong_style())
                            .color(p.text),
                    );
                    ui.add_space(6.0);
                    widgets::toolbar_separator(ui);
                    let favorite = if all_favorite { t("library.action.unfavorite") } else { t("library.action.favorite") };
                    if ui.add(Button::new(&favorite).kind(ButtonKind::Ghost).icon(icon::STAR)).clicked() {
                        act = Some(Act::Favorite);
                    }
                    let combine_button =
                        ui.add_enabled(combinable, Button::new(&t("library.action.combine")).kind(ButtonKind::Ghost).icon(icon::STACK));
                    combine = widgets::tooltip(combine_button, &t("library.action.combine_hint"), None).clicked();
                    if ui.add(Button::new(&t("library.action.delete")).kind(ButtonKind::Ghost).icon(icon::TRASH)).clicked() {
                        act = Some(Act::Delete);
                    }
                    widgets::toolbar_separator(ui);
                    clear = widgets::icon_button(ui, icon::X, &t("library.selection.clear"), Some("Esc")).clicked();
                });
            },
        );
        if let Some(act) = act {
            self.run(cx, act, entries);
        }
        if combine {
            self.combine_selected(cx);
        }
        if delete {
            self.delete_selected(cx);
        }
        if cancel {
            self.bulk_confirm = false;
        }
        if clear {
            self.clear_selection();
        }
    }

    fn quick_look_layer(&mut self, ctx: &Context) {
        let Some(entry) = self.subject().cloned() else {
            self.quick_look = false;
            return;
        };
        let p = snap_ui::palette(ctx);
        let tex = self
            .preview
            .as_ref()
            .filter(|pv| pv.id == entry.id.0)
            .and_then(|pv| pv.tex.clone())
            .or_else(|| self.thumbs.get(&entry).cloned());
        let full = ctx.content_rect();
        let rect = Rect::from_min_max(egui::pos2(full.left(), full.top() + widgets::TITLE_BAR_HEIGHT), full.max);
        let fade = util::anim(ctx, Id::unique("library-quick-look-fade"), true, util::Motion::HOVER);
        let mut close = false;
        Area::new(Id::unique("library-quick-look")).order(Order::Foreground).fixed_pos(rect.min).show(ctx, |ui| {
            let (rect, resp) = ui.allocate_exact_size(rect.size(), Sense::CLICK);
            let scrim = Color32::from_rgba_unmultiplied(8, 9, 12, (215.0 * fade) as u8);
            ui.painter().rect_filled(rect, CornerRadius { nw: 0, ne: 0, sw: 14, se: 14 }, scrim);
            let bounds = rect.shrink2(Vec2::new(56.0, 64.0));
            match &tex {
                Some(tex) => {
                    let target = paint::fit(tex.size_vec2(), bounds, 2.0);
                    ui.painter().add(p.shadow().as_shape(target, 8.0));
                    paint::paint_texture(
                        ui,
                        tex,
                        target,
                        Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                        CornerRadius::same(8),
                    );
                }
                None => {
                    ui.scope_builder(egui::UiBuilder::new().max_rect(Rect::from_center_size(rect.center(), Vec2::splat(24.0))), spinner);
                }
            }
            // Caption pill under the image and the key hint.
            let label = format!("{}  ·  {} × {}", paint::title_of(&entry), entry.width, entry.height);
            let galley = paint::elided(ui, &label, egui::FontId::proportional(12.0), Color32::WHITE, rect.width() - 160.0);
            let pill = Rect::from_center_size(egui::pos2(rect.center().x, rect.bottom() - 28.0), galley.size() + Vec2::new(28.0, 14.0));
            ui.painter().rect_filled(pill, pill.height() / 2.0, Color32::from_black_alpha(140));
            ui.painter().galley(util::snap(ui, pill.center() - galley.size() / 2.0), galley, Color32::WHITE);
            if resp.clicked() {
                close = true;
            }
            let hint = Rect::from_min_size(egui::pos2(rect.right() - 20.0 - 44.0, rect.top() + 14.0), Vec2::new(44.0, 20.0));
            ui.scope_builder(egui::UiBuilder::new().max_rect(hint), |ui| kbd(ui, "Esc"));
        });
        if close {
            self.quick_look = false;
        }
    }

    fn menu_layer(&mut self, cx: &mut ViewCx, ctx: &Context) {
        let Some(open) = &self.menu else { return };
        let (kind, id, anchor) = (open.kind, open.id.clone(), open.anchor);
        let Some(entry) = self.entries.iter().find(|e| e.id.0 == id).cloned() else {
            self.menu = None;
            return;
        };
        let menu_id = Id::unique("library-menu");
        match kind {
            MenuKind::Entry => {
                let targets = self.targets_for(&id);
                let items = entry_items(&entry, targets.len());
                match menu::show(ctx, menu_id, anchor, &items) {
                    menu::Outcome::Open => {}
                    menu::Outcome::Closed => self.menu = None,
                    menu::Outcome::Chosen(act) => {
                        self.menu = None;
                        self.run(cx, act, targets);
                    }
                }
            }
            MenuKind::LinkMore(index) => {
                let Some(link) = entry.links.get(index).cloned() else {
                    self.menu = None;
                    return;
                };
                let server = cx
                    .settings
                    .destinations
                    .iter()
                    .any(|d| d.id == link.destination && matches!(d.kind, snap_core::settings::DestinationKind::SnapServer { .. }));
                let mut items = vec![menu::Item::new(icon::QR_CODE, t("library.links.show_qr"), LinkMore::Qr)];
                if server {
                    items.push(menu::Item::new(icon::LOCK, t("library.remote.options"), LinkMore::Options));
                }
                match menu::show(ctx, menu_id, anchor, &items) {
                    menu::Outcome::Open => {}
                    menu::Outcome::Closed => self.menu = None,
                    menu::Outcome::Chosen(choice) => {
                        self.menu = None;
                        match choice {
                            LinkMore::Qr => cx.push(Command::ShowQr(link.url)),
                            LinkMore::Options => self.open_remote(cx, &entry, index),
                        }
                    }
                }
            }
            MenuKind::Link(index) => {
                let Some(link) = entry.links.get(index).cloned() else {
                    self.menu = None;
                    return;
                };
                let formats = [
                    (icon::LINK, "library.links.format.url", LinkFormat::Url),
                    (icon::HASH, "library.links.format.markdown", LinkFormat::Markdown),
                    (icon::CODE, "library.links.format.html", LinkFormat::Html),
                    (icon::CHAT_TEXT, "library.links.format.bbcode", LinkFormat::BbCode),
                ];
                let items: Vec<_> = formats.iter().map(|(glyph, key, format)| menu::Item::new(glyph, t(key), *format)).collect();
                match menu::show(ctx, menu_id, anchor, &items) {
                    menu::Outcome::Open => {}
                    menu::Outcome::Closed => self.menu = None,
                    menu::Outcome::Chosen(format) => {
                        self.menu = None;
                        let image = entry.kind != snap_core::CaptureKind::Recording;
                        let text = snap_upload::format_link(&link.url, format, Some(&paint::title_of(&entry)), image);
                        cx.push(Command::CopyText(text));
                        cx.toast(snap_ui::ToastKind::Success, t("common.copied"));
                    }
                }
            }
        }
    }
}

/// What the "more" menu of a link offers.
#[derive(Clone, Copy)]
enum LinkMore {
    Qr,
    Options,
}

fn entry_items(entry: &snap_history::Entry, count: usize) -> Vec<menu::Item<Act>> {
    let recording = entry.kind == snap_core::CaptureKind::Recording;
    let many = count > 1;
    let mut items = vec![
        menu::Item::new(icon::PENCIL_SIMPLE, if recording { t("library.action.edit_video") } else { t("library.action.edit") }, Act::Edit)
            .hotkey("Enter")
            .enabled(!many),
    ];
    items.push(menu::Item::new(icon::COPY, t("library.action.copy"), Act::Copy).hotkey("CmdOrCtrl+C"));
    if !recording {
        items.push(menu::Item::new(icon::PUSH_PIN, t("library.action.pin"), Act::Pin).hotkey("P"));
    }
    let favorite = if entry.favorite { t("library.action.unfavorite") } else { t("library.action.favorite") };
    items.push(menu::Item::new(icon::STAR, favorite, Act::Favorite).hotkey("F"));
    items.push(menu::Item::new(icon::CLOUD_ARROW_UP, t("library.action.upload"), Act::Upload).hotkey("U").separated());
    items.push(menu::Item::new(icon::FOLDER_OPEN, t("library.action.reveal"), Act::Reveal).enabled(!many));
    items.push(menu::Item::new(icon::FILES, t("library.action.copy_file"), Act::CopyFile));
    items.push(menu::Item::new(icon::PATH, t("library.action.copy_path"), Act::CopyPath).enabled(!many));
    if !recording && !many {
        items.push(menu::Item::new(icon::TEXT_AA, t("library.text.recognize"), Act::Recognize));
    }
    items.push(menu::Item::new(icon::TRASH, t("library.action.delete"), Act::Delete).hotkey("Del").danger().separated());
    items
}

/// The configured hotkey for "capture region", as it should be displayed.
fn capture_hotkey(cx: &ViewCx) -> Option<String> {
    cx.settings
        .hotkeys
        .iter()
        .find(|b| b.enabled && b.action == Action::CaptureRegion && !b.accel.trim().is_empty())
        .map(|b| snap_platform::hotkeys::display(&b.accel))
}
