//! Readable light/dark themes, comfortable spacing and UI scale.
use eframe::egui::{self, Color32, FontFamily, FontId, Stroke, TextStyle};
use serde::{Deserialize, Serialize};

/// How big the interface is drawn on each screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScaleMode {
    /// follow the display scaling the operating system sets for each screen
    #[default]
    Follow,
    /// the same size in pixels on every screen (display scaling is ignored)
    Fixed,
    /// fit each screen: the layout looks the way it does on a 1920×1080 screen at 100%
    Fit,
}

impl ScaleMode {
    pub const ALL: [ScaleMode; 3] = [ScaleMode::Follow, ScaleMode::Fixed, ScaleMode::Fit];

    pub fn label(self) -> &'static str {
        match self {
            ScaleMode::Follow => "Follow each screen's display scaling",
            ScaleMode::Fixed => "Same size in pixels on every screen",
            ScaleMode::Fit => "Fit each screen like 1920×1080",
        }
    }
}

/// The screen height the Fit mode keeps the layout of.
const FIT_HEIGHT: f32 = 1080.0;

fn scale_id() -> egui::Id {
    egui::Id::new("radiomaster_ui_scale")
}

/// Set the zoom for the screen the main window is on now. Called every frame: it only changes
/// anything when the window moved to a screen with other scaling or the settings changed.
pub fn fit_zoom(ctx: &egui::Context, mode: ScaleMode) {
    let mut scale = ctx.data(|d| d.get_temp::<f32>(scale_id())).unwrap_or(1.0);
    // Ctrl + / Ctrl − zoomed since the last frame: keep that (for this session) instead of undoing it
    let last = ctx.data(|d| d.get_temp::<f32>(last_zoom_id()));
    if let Some(last) = last.filter(|l| (ctx.zoom_factor() - l).abs() > 0.005 && *l > 0.0) {
        scale = (scale * ctx.zoom_factor() / last).clamp(0.5, 3.0);
        ctx.data_mut(|d| d.insert_temp(scale_id(), scale));
    }
    let (native, monitor) = ctx.input(|i| (i.viewport().native_pixels_per_point.unwrap_or(1.0), i.viewport().monitor_size));
    let native = if native.is_finite() && native > 0.0 { native } else { 1.0 };
    let ppp = match (mode, monitor) {
        (ScaleMode::Follow, _) => native * scale,
        (ScaleMode::Fixed, _) => scale,
        // (the monitor size is given in points at the current zoom)
        (ScaleMode::Fit, Some(m)) if m.y > 100.0 => (m.y * ctx.pixels_per_point()).round() / FIT_HEIGHT * scale,
        (ScaleMode::Fit, _) => native * scale,
    };
    let zoom = (ppp / native).clamp(0.4, 3.0);
    if (ctx.zoom_factor() - zoom).abs() > 0.005 {
        ctx.set_zoom_factor(zoom);
    }
    ctx.data_mut(|d| d.insert_temp(last_zoom_id(), zoom));
}

fn last_zoom_id() -> egui::Id {
    egui::Id::new("radiomaster_ui_zoom")
}

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
    // the zoom itself is set by `fit_zoom`, which also knows the screen
    ctx.data_mut(|d| d.insert_temp(scale_id(), scale.clamp(0.8, 1.7)));
}

/// Fonts from the computer that cover what egui's built-in fonts don't (symbols, Chinese,
/// Japanese, Korean, Arabic, Hindi…), so track titles and translations don't show as squares.
/// They are only used for characters the built-in fonts lack.
pub fn install_fallback_fonts(ctx: &egui::Context) {
    #[cfg(windows)]
    let candidates: &[(&str, u32)] = &[
        ("C:\\Windows\\Fonts\\seguisym.ttf", 0),
        ("C:\\Windows\\Fonts\\seguiemj.ttf", 0),
        ("C:\\Windows\\Fonts\\msyh.ttc", 0),
        ("C:\\Windows\\Fonts\\YuGothR.ttc", 0),
        ("C:\\Windows\\Fonts\\malgun.ttf", 0),
        ("C:\\Windows\\Fonts\\Nirmala.ttf", 0),
        ("C:\\Windows\\Fonts\\arial.ttf", 0),
    ];
    #[cfg(target_os = "macos")]
    let candidates: &[(&str, u32)] = &[
        ("/System/Library/Fonts/Apple Symbols.ttf", 0),
        ("/System/Library/Fonts/PingFang.ttc", 0),
        ("/System/Library/Fonts/Hiragino Sans GB.ttc", 0),
        ("/System/Library/Fonts/AppleSDGothicNeo.ttc", 0),
        ("/System/Library/Fonts/Supplemental/Arial Unicode.ttf", 0),
    ];
    #[cfg(all(unix, not(target_os = "macos")))]
    let candidates: &[(&str, u32)] = &[
        ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", 0),
        ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 0),
        ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 0),
        ("/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc", 0),
        ("/usr/share/fonts/truetype/noto/NotoSansSymbols2-Regular.ttf", 0),
    ];
    let mut defs = egui::FontDefinitions::default();
    let mut added = 0;
    for (i, (path, index)) in candidates.iter().enumerate() {
        let Ok(bytes) = std::fs::read(path) else { continue };
        // a font egui can't read would stop the program: check it first
        if ab_glyph::FontRef::try_from_slice_and_index(&bytes, *index).is_err() {
            continue;
        }
        let name = format!("system{i}");
        let mut fd = egui::FontData::from_owned(bytes);
        fd.index = *index;
        defs.font_data.insert(name.clone(), std::sync::Arc::new(fd));
        for fam in [FontFamily::Proportional, FontFamily::Monospace] {
            defs.families.entry(fam).or_default().push(name.clone());
        }
        added += 1;
    }
    if added > 0 {
        ctx.set_fonts(defs);
    }
}
