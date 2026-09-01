use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle,
    Visuals,
};

pub const INK: Color32 = Color32::from_rgb(31, 38, 45);
pub const MUTED: Color32 = Color32::from_rgb(92, 103, 114);
pub const CANVAS: Color32 = Color32::from_rgb(236, 239, 241);
pub const PANEL: Color32 = Color32::from_rgb(247, 248, 249);
pub const SURFACE: Color32 = Color32::from_rgb(255, 255, 255);
pub const LINE: Color32 = Color32::from_rgb(187, 194, 200);
pub const FOCUS: Color32 = Color32::from_rgb(51, 95, 125);
pub const FOCUS_SOFT: Color32 = Color32::from_rgb(228, 236, 241);
pub const NEGATIVE: Color32 = Color32::from_rgb(126, 58, 58);
pub const CAUTION: Color32 = Color32::from_rgb(104, 82, 48);

pub fn install(context: &egui::Context, dark: bool) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "convalgen-noto-sans".to_owned(),
        Arc::new(FontData::from_static(ttf_noto_sans::REGULAR)),
    );
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("convalgen-noto-sans".to_owned());
    }
    context.set_fonts(fonts);

    let mut style = (*context.style()).clone();
    style.visuals = if dark {
        let mut visuals = Visuals::dark();
        visuals.panel_fill = Color32::from_rgb(29, 33, 37);
        visuals.window_fill = Color32::from_rgb(34, 39, 44);
        visuals.faint_bg_color = Color32::from_rgb(42, 47, 52);
        visuals.extreme_bg_color = Color32::from_rgb(24, 28, 32);
        visuals.selection.bg_fill = Color32::from_rgb(58, 91, 113);
        visuals
    } else {
        let mut visuals = Visuals::light();
        visuals.panel_fill = PANEL;
        visuals.window_fill = SURFACE;
        visuals.faint_bg_color = CANVAS;
        visuals.extreme_bg_color = SURFACE;
        visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, INK);
        visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, INK);
        visuals.widgets.hovered.bg_fill = FOCUS_SOFT;
        visuals.widgets.active.bg_fill = Color32::from_rgb(214, 225, 232);
        visuals.selection.bg_fill = FOCUS;
        visuals.selection.stroke = Stroke::new(1.0_f32, Color32::WHITE);
        visuals
    };
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.interact_size.y = 28.0;
    style.spacing.scroll.floating = false;
    style.spacing.scroll.bar_width = 9.0;
    style.spacing.scroll.handle_min_length = 32.0;
    style.visuals.widgets.noninteractive.corner_radius = CornerRadius::same(3);
    style.visuals.widgets.inactive.corner_radius = CornerRadius::same(3);
    style.visuals.widgets.hovered.corner_radius = CornerRadius::same(3);
    style.visuals.widgets.active.corner_radius = CornerRadius::same(3);
    style.text_styles.insert(
        TextStyle::Heading,
        FontId::new(20.0, FontFamily::Proportional),
    );
    style
        .text_styles
        .insert(TextStyle::Body, FontId::new(13.5, FontFamily::Proportional));
    style.text_styles.insert(
        TextStyle::Monospace,
        FontId::new(12.5, FontFamily::Monospace),
    );
    style.text_styles.insert(
        TextStyle::Small,
        FontId::new(11.0, FontFamily::Proportional),
    );
    context.set_style(style);
}

pub fn section() -> egui::Frame {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0_f32, LINE))
        .corner_radius(CornerRadius::same(3))
        .inner_margin(12.0)
}
