//! The app's look: colours, egui style, and the small custom widgets shared
//! by every page. Colours come from the Joltux logo (teal tile, amber spark).

use eframe::egui::{
    self, Color32, CornerRadius, CursorIcon, FontId, Margin, Response, Sense, Stroke, StrokeKind,
    TextStyle, Ui, Widget,
};

pub const BG: Color32 = Color32::from_rgb(15, 21, 20);
pub const SIDEBAR: Color32 = Color32::from_rgb(10, 15, 14);
pub const SURFACE: Color32 = Color32::from_rgb(22, 31, 29);
pub const SURFACE_HI: Color32 = Color32::from_rgb(31, 43, 40);
pub const BORDER: Color32 = Color32::from_rgb(38, 52, 48);
pub const TEXT: Color32 = Color32::from_rgb(214, 226, 222);
pub const TEXT_DIM: Color32 = Color32::from_rgb(134, 154, 148);
pub const ACCENT: Color32 = Color32::from_rgb(24, 165, 142);
pub const ACCENT_BRIGHT: Color32 = Color32::from_rgb(52, 208, 180);
pub const ACCENT_DEEP: Color32 = Color32::from_rgb(10, 83, 72);
pub const AMBER: Color32 = Color32::from_rgb(255, 194, 61);
pub const DANGER: Color32 = Color32::from_rgb(236, 110, 102);
pub const BLUE: Color32 = Color32::from_rgb(110, 165, 236);

/// How long hover and selection animations take, in seconds.
pub const FAST: f32 = 0.14;

/// Every icon glyph the app draws. A test checks egui's built-in fonts have
/// them all, so none of them can show up as an empty box.
pub mod icon {
    pub const STORE: &str = "🏠";
    pub const LIBRARY: &str = "🎮";
    pub const ADD: &str = "➕";
    pub const ACCOUNT: &str = "👤";
    pub const SETTINGS: &str = "⚙";
    pub const BACK: &str = "⬅";
    pub const SEARCH: &str = "🔍";
    pub const PLAY: &str = "▶";
    pub const CLOSE: &str = "✖";
    pub const HEART: &str = "♥";
    pub const STOP: &str = "⏹";
    pub const MORE: &str = "☰";
    #[cfg(test)]
    pub const ALL: &[&str] = &[
        STORE, LIBRARY, ADD, ACCOUNT, SETTINGS, BACK, SEARCH, PLAY, CLOSE, HEART, STOP, MORE,
    ];
}

/// Installs the dark Joltux style on the whole app.
pub fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();

    style.text_styles = [
        (TextStyle::Small, FontId::proportional(12.0)),
        (TextStyle::Body, FontId::proportional(14.5)),
        (TextStyle::Button, FontId::proportional(14.5)),
        (TextStyle::Heading, FontId::proportional(22.0)),
        (TextStyle::Monospace, FontId::monospace(12.5)),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(14.0, 7.0);
    style.spacing.interact_size.y = 32.0;
    style.spacing.icon_width = 18.0;
    style.spacing.scroll.bar_width = 8.0;
    style.animation_time = FAST;

    let visuals = &mut style.visuals;
    *visuals = egui::Visuals::dark();
    visuals.panel_fill = BG;
    visuals.window_fill = SURFACE;
    visuals.window_stroke = Stroke::new(1.0_f32, BORDER);
    visuals.window_corner_radius = CornerRadius::same(12);
    visuals.menu_corner_radius = CornerRadius::same(10);
    visuals.extreme_bg_color = SIDEBAR;
    visuals.faint_bg_color = SURFACE;
    visuals.hyperlink_color = ACCENT_BRIGHT;
    visuals.selection.bg_fill = mix(SURFACE_HI, ACCENT, 0.55);
    visuals.selection.stroke = Stroke::new(1.5_f32, ACCENT_BRIGHT);

    let radius = CornerRadius::same(8);
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = SURFACE;
    widgets.noninteractive.weak_bg_fill = SURFACE;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    widgets.noninteractive.corner_radius = radius;

    // Checkbox and radio backgrounds; lighter so they stand out on cards.
    widgets.inactive.bg_fill = mix(SURFACE_HI, TEXT_DIM, 0.3);
    widgets.inactive.weak_bg_fill = SURFACE_HI;
    widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    widgets.inactive.corner_radius = radius;

    widgets.hovered.bg_fill = mix(SURFACE_HI, ACCENT, 0.25);
    widgets.hovered.weak_bg_fill = mix(SURFACE_HI, ACCENT, 0.18);
    widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    widgets.hovered.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    widgets.hovered.corner_radius = radius;
    widgets.hovered.expansion = 0.0;

    widgets.active.bg_fill = ACCENT_DEEP;
    widgets.active.weak_bg_fill = ACCENT_DEEP;
    widgets.active.bg_stroke = Stroke::new(1.0_f32, ACCENT_BRIGHT);
    widgets.active.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    widgets.active.corner_radius = radius;
    widgets.active.expansion = 0.0;

    widgets.open = widgets.inactive;
    widgets.open.bg_stroke = Stroke::new(1.0_f32, ACCENT);

    ctx.set_style(style);
}

/// Blends `a` towards `b` by `t` (0 = `a`, 1 = `b`).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    a.lerp_to_gamma(b, t.clamp(0.0, 1.0))
}

/// Starts fast and settles gently; used for page and card motion.
pub fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// A rounded panel that groups related content.
pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(16))
}

/// A single-line text box with the app's padding.
pub fn field(text: &mut String) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(text).margin(Margin::symmetric(10, 7))
}

/// A small caption above a form field or section.
pub fn caption(ui: &mut Ui, text: &str) {
    ui.label(egui::RichText::new(text).small().color(TEXT_DIM));
}

/// A card title.
pub fn section_title(ui: &mut Ui, text: &str) {
    ui.label(egui::RichText::new(text).size(16.0).color(Color32::WHITE));
}

/// A coloured status dot, vertically centred on the current row.
pub fn dot(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

/// A small tinted label, such as a platform name.
pub fn badge(ui: &mut Ui, text: &str, color: Color32) {
    let font = TextStyle::Small.resolve(ui.style());
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let (rect, _) = ui.allocate_exact_size(galley.size() + egui::vec2(12.0, 4.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(5), color.gamma_multiply(0.16));
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tone {
    Primary,
    Danger,
}

/// The app's emphasised button: filled teal for the main action on a page,
/// or an outlined red one for destructive actions.
pub struct AccentButton {
    text: String,
    tone: Tone,
}

impl AccentButton {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into(), tone: Tone::Primary }
    }

    pub fn danger(text: impl Into<String>) -> Self {
        Self { text: text.into(), tone: Tone::Danger }
    }
}

impl Widget for AccentButton {
    fn ui(self, ui: &mut Ui) -> Response {
        let font = TextStyle::Button.resolve(ui.style());
        let galley = ui.painter().layout_no_wrap(self.text, font, Color32::PLACEHOLDER);
        let size = egui::vec2(
            galley.size().x + 32.0,
            (galley.size().y + 14.0).max(ui.spacing().interact_size.y),
        );
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        if ui.is_rect_visible(rect) {
            let enabled = ui.is_enabled();
            let hover = ui
                .ctx()
                .animate_bool_with_time(response.id, enabled && response.hovered(), FAST);
            let pressed = response.is_pointer_button_down_on();
            let radius = CornerRadius::same(8);
            let (fill, stroke, text) = match (self.tone, enabled) {
                (_, false) => (SURFACE_HI, Stroke::NONE, TEXT_DIM),
                (Tone::Primary, true) => {
                    let fill = if pressed { ACCENT_DEEP } else { mix(ACCENT, ACCENT_BRIGHT, hover * 0.6) };
                    (fill, Stroke::NONE, Color32::WHITE)
                }
                (Tone::Danger, true) => (
                    DANGER.gamma_multiply(0.08 + 0.16 * hover),
                    Stroke::new(1.0_f32, mix(BORDER, DANGER, 0.4 + 0.6 * hover)),
                    DANGER,
                ),
            };
            ui.painter().rect_filled(rect, radius, fill);
            ui.painter().rect_stroke(rect, radius, stroke, StrokeKind::Inside);
            ui.painter().galley(rect.center() - galley.size() / 2.0, galley, text);
        }
        response.on_hover_cursor(CursorIcon::PointingHand)
    }
}

/// An on/off switch with a label; the knob slides when it changes.
pub fn toggle(ui: &mut Ui, on: &mut bool, label: &str) -> Response {
    ui.horizontal(|ui| {
        let (rect, mut response) = ui.allocate_exact_size(egui::vec2(34.0, 20.0), Sense::click());
        let text = ui.add(egui::Label::new(label).selectable(false).sense(Sense::click()));
        if response.clicked() || text.clicked() {
            *on = !*on;
            response.mark_changed();
        }
        let hovered = response.hovered() || text.hovered();
        if hovered {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        let t = ui.ctx().animate_bool_with_time(response.id, *on, FAST);
        let off = mix(SURFACE_HI, TEXT_DIM, if hovered { 0.45 } else { 0.3 });
        ui.painter().rect_filled(rect, CornerRadius::same(255), mix(off, ACCENT, t));
        let knob_x = egui::lerp((rect.left() + 10.0)..=(rect.right() - 10.0), t);
        ui.painter()
            .circle_filled(egui::pos2(knob_x, rect.center().y), 7.0, Color32::WHITE);
        response
    })
    .inner
}

/// A pill-shaped toggle, used for the store's category filters.
pub fn chip(ui: &mut Ui, selected: bool, label: &str) -> Response {
    let font = TextStyle::Small.resolve(ui.style());
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, Color32::PLACEHOLDER);
    let size = galley.size() + egui::vec2(24.0, 12.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let ctx = ui.ctx();
        let on = ctx.animate_bool_with_time(response.id.with("on"), selected, FAST);
        let hover = ctx.animate_bool_with_time(response.id.with("hover"), response.hovered(), FAST);
        let radius = CornerRadius::same(255);
        let fill = mix(mix(SURFACE, SURFACE_HI, hover), ACCENT, on);
        let border = mix(mix(BORDER, ACCENT, hover * 0.7), ACCENT, on);
        let text = mix(mix(TEXT_DIM, TEXT, hover), Color32::WHITE, on);
        ui.painter().rect_filled(rect, radius, fill);
        ui.painter().rect_stroke(rect, radius, Stroke::new(1.0_f32, border), StrokeKind::Inside);
        ui.painter().galley(rect.center() - galley.size() / 2.0, galley, text);
    }
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// A row of options where the highlight slides to the chosen one. Returns
/// true when the choice changed.
pub fn segmented<T: Copy + PartialEq>(
    ui: &mut Ui,
    id_salt: &str,
    current: &mut T,
    options: &[(T, &str)],
) -> bool {
    let font = TextStyle::Button.resolve(ui.style());
    let galleys: Vec<_> = options
        .iter()
        .map(|(_, label)| ui.painter().layout_no_wrap((*label).to_owned(), font.clone(), Color32::PLACEHOLDER))
        .collect();
    let pad = egui::vec2(14.0, 7.0);
    let inset = 3.0;
    let widths: Vec<f32> = galleys.iter().map(|g| g.size().x + pad.x * 2.0).collect();
    let height = galleys.iter().map(|g| g.size().y).fold(0.0, f32::max) + pad.y * 2.0;
    let total = egui::vec2(widths.iter().sum::<f32>() + inset * 2.0, height + inset * 2.0);

    let (rect, _) = ui.allocate_exact_size(total, Sense::hover());
    let id = ui.id().with(id_salt);
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(10), SIDEBAR);
    painter.rect_stroke(rect, CornerRadius::same(10), Stroke::new(1.0_f32, BORDER), StrokeKind::Inside);

    let mut left = rect.left() + inset;
    let cells: Vec<egui::Rect> = widths
        .iter()
        .map(|width| {
            let cell = egui::Rect::from_min_size(egui::pos2(left, rect.top() + inset), egui::vec2(*width, height));
            left += width;
            cell
        })
        .collect();

    // The highlight glides between cells; positions are relative to the
    // control so scrolling or resizing does not animate it.
    if let Some(selected) = options.iter().position(|(value, _)| value == current) {
        let ctx = ui.ctx();
        let x = ctx.animate_value_with_time(id.with("x"), cells[selected].left() - rect.left(), 0.18);
        let w = ctx.animate_value_with_time(id.with("w"), cells[selected].width(), 0.18);
        let pill = egui::Rect::from_min_size(egui::pos2(rect.left() + x, rect.top() + inset), egui::vec2(w, height));
        painter.rect_filled(pill, CornerRadius::same(8), ACCENT);
    }

    let mut changed = false;
    for (index, ((value, _), galley)) in options.iter().zip(galleys).enumerate() {
        let cell = cells[index];
        let response = ui
            .interact(cell, id.with(index), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        let color = if value == current {
            Color32::WHITE
        } else if response.hovered() {
            TEXT
        } else {
            TEXT_DIM
        };
        ui.painter().galley(cell.center() - galley.size() / 2.0, galley, color);
        if response.clicked() && value != current {
            *current = *value;
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_fonts_have_every_icon() {
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| apply(ctx));
        let font = FontId::proportional(14.5);
        for glyphs in icon::ALL {
            assert!(
                ctx.fonts(|fonts| fonts.has_glyphs(&font, glyphs)),
                "egui's fonts are missing {glyphs:?}"
            );
        }
    }

    #[test]
    fn easing_stays_in_range() {
        assert_eq!(ease_out(-1.0), 0.0);
        assert_eq!(ease_out(2.0), 1.0);
        assert!(ease_out(0.5) > 0.5);
    }
}
