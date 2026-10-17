//! Readable light/dark themes, comfortable spacing and UI scale.
use eframe::egui::{self, Color32, FontFamily, FontId, Stroke, TextStyle};

pub fn apply(ctx: &egui::Context, dark: bool, scale: f32) {
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    if dark {
        v.panel_fill = Color32::from_rgb(28, 31, 37);
        v.window_fill = Color32::from_rgb(36, 40, 47);
        v.extreme_bg_color = Color32::from_rgb(20, 22, 27);
        v.faint_bg_color = Color32::from_rgb(34, 38, 45);
        v.selection.bg_fill = Color32::from_rgb(40, 82, 140);
        v.selection.stroke = Stroke::new(1.0_f32, Color32::from_rgb(215, 230, 255));
        v.widgets.inactive.weak_bg_fill = Color32::from_rgb(52, 58, 68);
        v.widgets.inactive.bg_fill = Color32::from_rgb(52, 58, 68);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(68, 76, 90);
        v.widgets.hovered.bg_fill = Color32::from_rgb(68, 76, 90);
        for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active] {
            w.fg_stroke.color = Color32::from_gray(232);
        }
        v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(110, 122, 142));
    } else {
        v.panel_fill = Color32::from_rgb(241, 243, 247);
        v.window_fill = Color32::from_rgb(252, 252, 254);
        v.extreme_bg_color = Color32::WHITE;
        v.faint_bg_color = Color32::from_rgb(246, 248, 252);
        // light-blue selection with dark-blue text: selected tabs/rows stay readable
        v.selection.bg_fill = Color32::from_rgb(190, 217, 252);
        v.selection.stroke = Stroke::new(1.0_f32, Color32::from_rgb(20, 55, 130));
        v.widgets.inactive.weak_bg_fill = Color32::from_rgb(226, 231, 239);
        v.widgets.inactive.bg_fill = Color32::from_rgb(226, 231, 239);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(210, 220, 236);
        v.widgets.hovered.bg_fill = Color32::from_rgb(210, 220, 236);
        v.widgets.active.weak_bg_fill = Color32::from_rgb(190, 205, 228);
        v.widgets.active.bg_fill = Color32::from_rgb(190, 205, 228);
        for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active] {
            w.fg_stroke.color = Color32::from_rgb(24, 30, 42);
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(205, 211, 222));
        v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(138, 150, 170));
        v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(70, 110, 180));
    }
    v.window_shadow.spread = 6;
    ctx.set_visuals(v);

    let mut s = (*ctx.style()).clone();
    let prop = FontFamily::Proportional;
    s.text_styles = [
        (TextStyle::Heading, FontId::new(20.0, prop.clone())),
        (TextStyle::Body, FontId::new(15.0, prop.clone())),
        (TextStyle::Button, FontId::new(15.0, prop.clone())),
        (TextStyle::Small, FontId::new(12.5, prop)),
        (TextStyle::Monospace, FontId::new(14.5, FontFamily::Monospace)),
    ]
    .into();
    // static labels must not swallow clicks meant for table rows (text boxes stay selectable)
    s.interaction.selectable_labels = false;
    s.spacing.item_spacing = egui::vec2(8.0, 6.0);
    s.spacing.button_padding = egui::vec2(10.0, 5.0);
    s.spacing.interact_size.y = 26.0;
    s.spacing.window_margin = egui::Margin::same(12);
    s.spacing.menu_margin = egui::Margin::same(8);
    ctx.set_style(s);
    ctx.set_zoom_factor(scale.clamp(0.8, 1.7));
}
