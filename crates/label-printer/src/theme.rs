//! Look and feel: a dark palette, rounded cards, accent-filled primary
//! buttons, slide toggles and Phosphor icons. The accent is thermal orange.

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Frame, Margin, RichText, Sense, Stroke, StrokeKind, Ui,
    Vec2,
};

pub use egui_phosphor::regular as icon;

pub const BG: Color32 = Color32::from_rgb(0x16, 0x17, 0x1B);
pub const CARD: Color32 = Color32::from_rgb(0x1F, 0x21, 0x27);
pub const BORDER: Color32 = Color32::from_rgb(0x30, 0x33, 0x3C);
pub const TEXT: Color32 = Color32::from_rgb(0xF3, 0xF4, 0xF6);
pub const MUTED: Color32 = Color32::from_rgb(0x9C, 0xA3, 0xAF);
pub const GOOD: Color32 = Color32::from_rgb(0x4A, 0xDE, 0x80);
pub const BAD: Color32 = Color32::from_rgb(0xF8, 0x71, 0x71);
pub const WARN: Color32 = Color32::from_rgb(0xFB, 0xBF, 0x24);
pub const ACCENT: Color32 = Color32::from_rgb(0xEA, 0x58, 0x0C);
pub const ON_ACCENT: Color32 = Color32::WHITE;
/// Selected card: a dark tint of the accent.
pub const SELECTED: Color32 = Color32::from_rgb(0x34, 0x22, 0x1A);
const WIDGET: Color32 = Color32::from_rgb(0x2A, 0x2D, 0x35);
const WIDGET_HOVER: Color32 = Color32::from_rgb(0x34, 0x38, 0x42);

/// Fonts, spacing and colours for the whole app.
pub fn setup(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);

    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = CARD;
    v.extreme_bg_color = Color32::from_rgb(0x12, 0x13, 0x16);
    v.selection.bg_fill = Color32::from_rgb(0x7A, 0x34, 0x12);
    v.selection.stroke = Stroke::new(1.0, TEXT);
    v.hyperlink_color = ACCENT;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.inactive.weak_bg_fill = WIDGET;
    v.widgets.inactive.bg_fill = WIDGET;
    v.widgets.hovered.weak_bg_fill = WIDGET_HOVER;
    v.widgets.hovered.bg_fill = WIDGET_HOVER;
    v.widgets.active.weak_bg_fill = ACCENT;
    v.widgets.active.fg_stroke.color = ON_ACCENT;
    v.slider_trailing_fill = true;
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(7);
    }
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.button_padding = Vec2::new(10.0, 5.0);
        s.spacing.item_spacing = Vec2::new(8.0, 6.0);
        s.spacing.slider_width = 150.0;
    });
}

/// A rounded card; `selected` draws the accent outline and tint.
pub fn card<R>(ui: &mut Ui, selected: bool, add: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::new()
        .fill(if selected { SELECTED } else { CARD })
        .corner_radius(10)
        .stroke(Stroke::new(1.0, if selected { ACCENT } else { BORDER }))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// Section title above a card.
pub fn section(ui: &mut Ui, title: &str) {
    ui.add_space(14.0);
    ui.label(RichText::new(title).size(15.0).color(TEXT));
    ui.add_space(6.0);
}

/// Small grey explanatory text.
pub fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(11.5).color(MUTED));
}

/// The primary action.
pub fn accent_button(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).color(ON_ACCENT)).fill(ACCENT))
}

/// A large primary action.
pub fn big_accent_button(ui: &mut Ui, text: &str, enabled: bool) -> egui::Response {
    let button = egui::Button::new(RichText::new(text).size(16.0).color(ON_ACCENT))
        .fill(if enabled { ACCENT } else { WIDGET })
        .min_size(Vec2::new(150.0, 38.0));
    ui.add_enabled(enabled, button)
}

/// Icon-only button with a tooltip (the tooltip doubles as its accessible name).
pub fn icon_button(ui: &mut Ui, glyph: &str, tooltip: &str) -> egui::Response {
    let response = ui
        .button(RichText::new(glyph).size(15.0))
        .on_hover_text(tooltip);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tooltip));
    response
}

/// A slide toggle followed by its label; clicking either flips it.
pub fn toggle_row(ui: &mut Ui, on: &mut bool, label: &str) -> egui::Response {
    ui.horizontal(|ui| {
        let mut r = toggle(ui, on, label);
        let text = ui.add(egui::Label::new(RichText::new(label).color(TEXT)).sense(Sense::click()));
        if text.clicked() {
            *on = !*on;
            r.mark_changed();
        }
        r | text
    })
    .inner
}

fn toggle(ui: &mut Ui, on: &mut bool, label: &str) -> egui::Response {
    let size = Vec2::new(40.0, 22.0);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let enabled = ui.is_enabled();
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, *on, label)
    });
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(response.id, *on);
        let bg = Color32::from_rgb(0x3A, 0x3F, 0x4B).lerp_to_gamma(ACCENT, t);
        let painter = ui.painter();
        painter.rect_filled(rect, rect.height() / 2.0, bg);
        if response.hovered() {
            painter.rect_stroke(
                rect,
                rect.height() / 2.0,
                Stroke::new(1.0, Color32::from_white_alpha(60)),
                StrokeKind::Inside,
            );
        }
        let r = rect.height() / 2.0 - 3.0;
        let x = egui::lerp((rect.left() + 3.0 + r)..=(rect.right() - 3.0 - r), t);
        painter.circle_filled(egui::pos2(x, rect.center().y), r, Color32::WHITE);
    }
    response
}

/// Read-only pill with an icon, same height as a button.
pub fn chip(ui: &mut Ui, glyph: &str, text: &str, color: Color32) {
    let galley =
        ui.painter()
            .layout_no_wrap(format!("{glyph} {text}"), FontId::proportional(12.5), color);
    let size = Vec2::new(
        galley.size().x + 16.0,
        galley.size().y + 2.0 * ui.spacing().button_padding.y,
    );
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter()
        .rect(rect, 6, BG, Stroke::new(1.0, BORDER), StrokeKind::Inside);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, color);
}

/// A row of mutually exclusive buttons. Returns true when the value changed.
pub fn segmented<T: Copy + PartialEq>(ui: &mut Ui, value: &mut T, choices: &[(T, &str)]) -> bool {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for &(choice, label) in choices {
            let selected = *value == choice;
            let text = RichText::new(label).color(if selected { ON_ACCENT } else { TEXT });
            let button = egui::Button::new(text).fill(if selected { ACCENT } else { WIDGET });
            if ui.add(button).clicked() && !selected {
                *value = choice;
                changed = true;
            }
        }
    });
    changed
}

/// A filled status dot, with the state also given in words by the caller.
pub fn dot(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.5, color);
}
