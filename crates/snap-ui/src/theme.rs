//! Design tokens (docs/DESIGN.md) and the egui `Style` built from them.

use std::sync::Arc;

use egui::{
    Color32, Context, CornerRadius, Margin, Stroke, Style, Theme, ThemePreference, Ui, Vec2,
    epaint::Shadow,
    style::{HandleShape, ScrollStyle, WidgetVisuals},
};
use snap_core::settings::{General, Theme as ThemeSetting};

use crate::{fonts, platform, text};

/// Spacing scale (4-pt grid).
pub mod space {
    pub const XS: f32 = 4.0;
    pub const SM: f32 = 8.0;
    pub const MD: f32 = 12.0;
    pub const LG: f32 = 16.0;
    pub const XL: f32 = 24.0;
    pub const XXL: f32 = 32.0;
}

/// Corner radii.
pub mod radius {
    pub const CONTROL: f32 = 6.0;
    pub const CARD: f32 = 10.0;
    pub const WINDOW: f32 = 14.0;
    /// Large enough to always resolve to a full pill.
    pub const PILL: f32 = 255.0;
}

/// Control heights: default and compact.
pub const CONTROL_HEIGHT: f32 = 32.0;
pub const CONTROL_HEIGHT_COMPACT: f32 = 28.0;

const BRAND_ACCENT_DARK: Color32 = hex(0x6B7FFF);
const BRAND_ACCENT_LIGHT: Color32 = hex(0x3D5AFE);

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

pub(crate) fn with_alpha(c: Color32, alpha: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (alpha * 255.0).round() as u8)
}

/// Which theme to install and the accent to use with it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThemeSpec {
    pub dark: bool,
    pub accent: Color32,
}

impl Default for ThemeSpec {
    fn default() -> Self {
        Self::brand(true)
    }
}

impl ThemeSpec {
    /// The brand "Imperial blue" for the given mode.
    pub fn brand(dark: bool) -> Self {
        Self {
            dark,
            accent: if dark {
                BRAND_ACCENT_DARK
            } else {
                BRAND_ACCENT_LIGHT
            },
        }
    }

    /// Resolves `Theme::System` through the OS (dark when the OS cannot tell) and parses the `#RRGGBB` accent.
    pub fn from_settings(g: &General) -> Self {
        Self::resolve(g, platform::system_dark().unwrap_or(true))
    }

    /// Like [`Self::from_settings`] with a caller-provided system preference (e.g. from `ctx.system_theme()`).
    pub fn resolve(g: &General, system_dark: bool) -> Self {
        let dark = match g.theme {
            ThemeSetting::Dark => true,
            ThemeSetting::Light => false,
            ThemeSetting::System => system_dark,
        };
        Self {
            dark,
            accent: parse_hex(&g.accent).unwrap_or(Self::brand(dark).accent),
        }
    }

    pub fn with_accent(mut self, accent: Color32) -> Self {
        self.accent = accent;
        self
    }
}

/// Parses `#RRGGBB` / `RRGGBB`.
pub fn parse_hex(s: &str) -> Option<Color32> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 || !s.is_ascii() {
        return None;
    }
    u32::from_str_radix(s, 16).ok().map(hex)
}

/// Semantic tone for badges, toasts and status text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Tone {
    #[default]
    Neutral,
    Accent,
    Gold,
    Success,
    Warning,
    Danger,
}

/// Semantic colors for the active theme.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub dark: bool,
    pub bg: Color32,
    pub surface: Color32,
    pub surface_2: Color32,
    pub elevated: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub on_accent: Color32,
    pub gold: Color32,
    pub danger: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub scrim: Color32,
}

impl Palette {
    pub fn new(dark: bool, accent: Color32) -> Self {
        let on_accent = if luminance(accent) > 0.62 {
            hex(0x0E0F13)
        } else {
            Color32::WHITE
        };
        let scrim = with_alpha(hex(0x08090C), 0.55);
        if dark {
            Self {
                dark,
                bg: hex(0x0E0F13),
                surface: hex(0x16181E),
                surface_2: hex(0x1D2028),
                elevated: hex(0x242833),
                border: hex(0x2C303B),
                text: hex(0xECEDEF),
                text_dim: hex(0x9AA0AC),
                text_faint: hex(0x5D6270),
                accent,
                accent_soft: with_alpha(accent, 0.16),
                on_accent,
                gold: hex(0xE3B341),
                danger: hex(0xFF5A5F),
                success: hex(0x30C57A),
                warning: hex(0xF5A524),
                scrim,
            }
        } else {
            Self {
                dark,
                bg: hex(0xF7F7F5),
                surface: hex(0xFFFFFF),
                surface_2: hex(0xF0F0EC),
                elevated: hex(0xFFFFFF),
                border: hex(0xE3E2DC),
                text: hex(0x17181C),
                text_dim: hex(0x62646B),
                text_faint: hex(0xA3A49E),
                accent,
                accent_soft: with_alpha(accent, 0.12),
                on_accent,
                gold: hex(0xB8860B),
                danger: hex(0xE5484D),
                success: hex(0x2A9D62),
                warning: hex(0xC77A00),
                scrim,
            }
        }
    }

    /// The palette of the theme a `Ui` is currently styled with.
    pub fn of(ui: &Ui) -> Self {
        Self::from_style(ui.style())
    }

    pub(crate) fn from_style(style: &Style) -> Self {
        Self::new(
            style.visuals.dark_mode,
            style.visuals.selection.stroke.color,
        )
    }

    /// Translucent wash for hovered rows and ghost buttons.
    pub fn hover(&self) -> Color32 {
        with_alpha(self.text, if self.dark { 0.07 } else { 0.05 })
    }

    /// Translucent wash for pressed controls.
    pub fn pressed(&self) -> Color32 {
        with_alpha(self.text, if self.dark { 0.12 } else { 0.09 })
    }

    /// A border that reads on hover.
    pub fn border_strong(&self) -> Color32 {
        self.border.lerp_to_gamma(self.text_faint, 0.65)
    }

    /// Selection highlight inside text fields.
    pub fn text_selection(&self) -> Color32 {
        with_alpha(self.accent, if self.dark { 0.34 } else { 0.26 })
    }

    pub fn tone(&self, tone: Tone) -> Color32 {
        match tone {
            Tone::Neutral => self.text_dim,
            Tone::Accent => self.accent,
            Tone::Gold => self.gold,
            Tone::Success => self.success,
            Tone::Warning => self.warning,
            Tone::Danger => self.danger,
        }
    }

    /// Soft background that goes with [`Self::tone`].
    pub fn tone_soft(&self, tone: Tone) -> Color32 {
        match tone {
            Tone::Neutral => self.surface_2,
            t => with_alpha(self.tone(t), if self.dark { 0.16 } else { 0.12 }),
        }
    }

    /// Shadow for floating things: toolbars, menus, pins, toasts.
    pub fn shadow(&self) -> Shadow {
        Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(if self.dark { 89 } else { 36 }),
        }
    }

    /// A tighter shadow for small raised parts such as a selected segment.
    pub fn shadow_small(&self) -> Shadow {
        Shadow {
            offset: [0, 1],
            blur: 3,
            spread: 0,
            color: Color32::from_black_alpha(if self.dark { 80 } else { 28 }),
        }
    }
}

/// Relative luminance (WCAG) of an sRGB color.
fn luminance(c: Color32) -> f32 {
    let lin = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// The palette active on `ctx` (what [`install`] last set for the current theme).
pub fn palette(ctx: &Context) -> Palette {
    Palette::from_style(&ctx.global_style())
}

/// Installs fonts, text styles, spacing and widget visuals for both themes, then selects `theme`.
///
/// Cheap to call again when the theme or accent changes; fonts are only loaded once per context.
pub fn install(ctx: &Context, theme: &ThemeSpec) {
    fonts::install(ctx);
    let reduce_motion = platform::reduce_motion();
    ctx.set_style_of(Theme::Dark, style_for(true, theme.accent, reduce_motion));
    ctx.set_style_of(Theme::Light, style_for(false, theme.accent, reduce_motion));
    ctx.set_theme(if theme.dark {
        ThemePreference::Dark
    } else {
        ThemePreference::Light
    });
}

/// The complete egui style for one theme. Apply it to a single `Ui` with `ui.set_style(...)` to render a
/// differently themed region (the gallery does this).
pub fn style_for(dark: bool, accent: Color32, reduce_motion: bool) -> Arc<Style> {
    let p = Palette::new(dark, accent);
    let mut style = Style {
        animation_time: if reduce_motion { 0.0 } else { 0.1 },
        ..Style::default()
    };
    style.explanation_tooltips = false;
    style.url_in_tooltip = true;
    text::apply_text_styles(&mut style);
    style.drag_value_text_style = text::mono();

    let spacing = &mut style.spacing;
    spacing.item_spacing = Vec2::new(space::SM, space::SM);
    spacing.window_margin = Margin::same(space::LG as i8);
    spacing.button_padding = Vec2::new(space::MD, 6.0);
    spacing.menu_margin = Margin::same(6);
    spacing.indent = space::LG;
    spacing.interact_size = Vec2::new(40.0, CONTROL_HEIGHT);
    spacing.slider_width = 160.0;
    spacing.slider_rail_height = 4.0;
    spacing.combo_width = 140.0;
    spacing.text_edit_width = 200.0;
    spacing.icon_width = 16.0;
    spacing.icon_width_inner = 8.0;
    spacing.icon_spacing = space::SM;
    spacing.tooltip_width = 360.0;
    spacing.menu_width = 260.0;
    spacing.menu_spacing = 2.0;
    spacing.combo_height = 280.0;
    spacing.indent_ends_with_horizontal_line = false;
    spacing.scroll = ScrollStyle {
        floating: true,
        bar_width: 8.0,
        floating_width: 3.0,
        floating_allocated_width: 0.0,
        handle_min_length: 28.0,
        bar_inner_margin: 2.0,
        bar_outer_margin: 2.0,
        dormant_background_opacity: 0.0,
        active_background_opacity: 0.0,
        interact_background_opacity: 0.12,
        dormant_handle_opacity: 0.0,
        active_handle_opacity: 0.45,
        interact_handle_opacity: 0.8,
        foreground_color: false,
        ..ScrollStyle::floating()
    };

    let interaction = &mut style.interaction;
    interaction.tooltip_delay = 0.45;
    interaction.selectable_labels = false;
    interaction.resize_grab_radius_side = 5.0;
    interaction.resize_grab_radius_corner = 10.0;

    apply_visuals(&mut style.visuals, &p);
    Arc::new(style)
}

fn apply_visuals(v: &mut egui::Visuals, p: &Palette) {
    let control = CornerRadius::same(radius::CONTROL as u8);
    let widget = |bg: Color32, stroke: Stroke, fg: Color32| WidgetVisuals {
        bg_fill: bg,
        weak_bg_fill: bg,
        bg_stroke: stroke,
        corner_radius: control,
        fg_stroke: Stroke::new(1.0, fg),
        expansion: 0.0,
    };
    let hairline = Stroke::new(1.0, p.border);
    let over = |wash: Color32| composite(p.surface_2, wash);

    v.dark_mode = p.dark;
    v.override_text_color = None;
    v.weak_text_color = Some(p.text_dim);
    v.hyperlink_color = p.accent;
    v.faint_bg_color = with_alpha(p.text, 0.03);
    v.extreme_bg_color = p.surface_2;
    v.text_edit_bg_color = Some(p.surface_2);
    v.code_bg_color = p.surface_2;
    v.warn_fg_color = p.warning;
    v.error_fg_color = p.danger;
    v.panel_fill = p.bg;
    v.window_fill = p.elevated;
    v.window_stroke = hairline;
    v.window_corner_radius = CornerRadius::same(radius::WINDOW as u8);
    v.menu_corner_radius = CornerRadius::same(radius::CARD as u8);
    v.window_shadow = p.shadow();
    v.popup_shadow = p.shadow();
    v.window_highlight_topmost = false;
    v.resize_corner_size = 10.0;
    v.slider_trailing_fill = true;
    v.handle_shape = HandleShape::Circle;
    v.indent_has_left_vline = false;
    v.striped = false;
    v.disabled_alpha = 0.45;

    v.selection.bg_fill = p.text_selection();
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.text_cursor.stroke = Stroke::new(1.5, p.accent);

    v.widgets.noninteractive = widget(p.surface, hairline, p.text);
    v.widgets.inactive = widget(p.surface_2, hairline, p.text);
    v.widgets.hovered = widget(over(p.hover()), Stroke::new(1.0, p.border_strong()), p.text);
    v.widgets.active = widget(over(p.pressed()), Stroke::new(1.0, p.accent), p.text);
    v.widgets.open = widget(over(p.hover()), Stroke::new(1.0, p.border_strong()), p.text);
}

/// `top` painted over an opaque `base`.
pub(crate) fn composite(base: Color32, top: Color32) -> Color32 {
    let a = top.a() as f32 / 255.0;
    // `top` is premultiplied, so its channels already carry the alpha.
    let mix = |b: u8, t: u8| (b as f32 * (1.0 - a) + t as f32).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(
        mix(base.r(), top.r()),
        mix(base.g(), top.g()),
        mix(base.b(), top.b()),
    )
}
