//! RustCraft's visual theme: dark charcoal surfaces, one ember accent,
//! Inter typography, rounded panels.

use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame, Margin, RichText, Stroke,
    TextStyle, Vec2,
};

pub const BG: Color32 = Color32::from_rgb(0x15, 0x16, 0x19);
pub const SIDEBAR: Color32 = Color32::from_rgb(0x1a, 0x1b, 0x1f);
pub const CARD: Color32 = Color32::from_rgb(0x21, 0x23, 0x28);
pub const CARD_RAISED: Color32 = Color32::from_rgb(0x29, 0x2b, 0x31);
pub const BORDER: Color32 = Color32::from_rgb(0x30, 0x32, 0x39);
pub const TEXT: Color32 = Color32::from_rgb(0xec, 0xec, 0xef);
pub const MUTED: Color32 = Color32::from_rgb(0xa2, 0xa6, 0xaf);
pub const ACCENT: Color32 = Color32::from_rgb(0xe8, 0x74, 0x3b);
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0xf2, 0x8a, 0x55);
pub const SUCCESS: Color32 = Color32::from_rgb(0x55, 0xb8, 0x82);
pub const WARNING: Color32 = Color32::from_rgb(0xe3, 0xab, 0x45);
pub const DANGER: Color32 = Color32::from_rgb(0xe4, 0x5f, 0x63);

pub const SEMIBOLD: &str = "semibold";

pub fn semibold() -> FontFamily {
    FontFamily::Name(SEMIBOLD.into())
}

pub fn install(ctx: &egui::Context, reduced_motion: bool) {
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("Inter".into(), Arc::new(FontData::from_static(include_bytes!("../assets/fonts/Inter-Regular.ttf"))));
    fonts.font_data.insert(
        "Inter-SemiBold".into(),
        Arc::new(FontData::from_static(include_bytes!("../assets/fonts/Inter-SemiBold.ttf"))),
    );
    let fallbacks = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "Inter".into());
    fonts.families.insert(semibold(), std::iter::once("Inter-SemiBold".to_string()).chain(fallbacks).collect());
    ctx.set_fonts(fonts);

    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, FontId::new(24.0, semibold())),
            (TextStyle::Body, FontId::new(14.5, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(14.5, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(12.5, FontFamily::Monospace)),
        ]
        .into();
        style.spacing.item_spacing = Vec2::new(10.0, 10.0);
        style.spacing.button_padding = Vec2::new(14.0, 7.0);
        style.spacing.interact_size.y = 30.0;
        style.spacing.slider_width = 220.0;
        style.animation_time = if reduced_motion { 0.0 } else { 0.12 };

        let v = &mut style.visuals;
        v.dark_mode = true;
        v.override_text_color = None;
        v.panel_fill = BG;
        v.window_fill = CARD;
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.window_corner_radius = CornerRadius::same(14);
        v.menu_corner_radius = CornerRadius::same(10);
        v.extreme_bg_color = Color32::from_rgb(0x12, 0x13, 0x15);
        v.faint_bg_color = CARD;
        v.code_bg_color = Color32::from_rgb(0x12, 0x13, 0x15);
        v.hyperlink_color = ACCENT_HOVER;
        v.warn_fg_color = WARNING;
        v.error_fg_color = DANGER;
        v.selection.bg_fill = ACCENT.linear_multiply(0.45);
        v.selection.stroke = Stroke::new(1.0, ACCENT);

        let radius = CornerRadius::same(8);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = CARD;
        w.noninteractive.weak_bg_fill = CARD;
        w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        w.noninteractive.corner_radius = radius;
        for (state, fill) in [
            (&mut w.inactive, CARD_RAISED),
            (&mut w.hovered, Color32::from_rgb(0x33, 0x35, 0x3d)),
            (&mut w.active, Color32::from_rgb(0x3a, 0x3c, 0x45)),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.corner_radius = radius;
            state.fg_stroke = Stroke::new(1.0, TEXT);
            state.bg_stroke = Stroke::NONE;
            state.expansion = 0.0;
        }
        w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x44, 0x46, 0x4f));
        w.active.bg_stroke = Stroke::new(1.0, ACCENT);
        w.open = w.active;
    });
}

pub fn card() -> Frame {
    Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(18))
}

/// The large accent call-to-action button.
pub fn primary(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_owned()).family(semibold()).size(17.0).color(Color32::WHITE))
        .fill(ACCENT)
        .corner_radius(CornerRadius::same(10))
        .min_size(Vec2::new(180.0, 46.0))
}

pub fn secondary(text: &str) -> egui::Button<'static> {
    egui::Button::new(text.to_owned()).corner_radius(CornerRadius::same(8))
}

pub fn danger(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_owned()).color(Color32::WHITE)).fill(DANGER.linear_multiply(0.85))
}

/// A pill label. Painted directly so it keeps its size in any layout.
pub fn badge(ui: &mut egui::Ui, text: &str, color: Color32) {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::proportional(11.5), color);
    let (rect, _) = ui.allocate_exact_size(galley.size() + Vec2::new(16.0, 6.0), egui::Sense::hover());
    ui.painter().rect(
        rect,
        CornerRadius::same(255),
        color.linear_multiply(0.16),
        Stroke::new(1.0, color.linear_multiply(0.5)),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
}

pub fn title(ui: &mut egui::Ui, text: &str, subtitle: &str) {
    ui.label(RichText::new(text).heading().color(TEXT));
    if !subtitle.is_empty() {
        ui.label(RichText::new(subtitle).color(MUTED));
    }
    ui.add_space(8.0);
}

pub fn section(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).family(semibold()).size(15.5).color(TEXT));
}

pub fn muted(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).color(MUTED)
}
