//! The look: colors, corner radii and fonts.

use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Stroke, TextStyle, Visuals};
use std::sync::Arc;

/// The two color schemes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    /// Neutral dark gray: the default, so the interface does not skew judgment of colors.
    Dark,
    Light,
}

/// The family holding only the icon font, for drawing icons at their own size.
pub fn icon_family() -> FontFamily {
    FontFamily::Name("icons".into())
}

/// Colors the app uses beyond egui's own visuals.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    pub accent: Color32,
    /// Behind the canvas, around and outside it.
    pub backdrop: Color32,
    pub ribbon: Color32,
    pub tab_bar: Color32,
    pub separator: Color32,
    pub weak_text: Color32,
    pub strong_text: Color32,
    /// Marks a setting changed from its default.
    pub changed: Color32,
    pub keytip_bg: Color32,
    pub keytip_fg: Color32,
}

pub fn palette(scheme: Scheme) -> Palette {
    match scheme {
        Scheme::Dark => Palette {
            dark: true,
            accent: Color32::from_rgb(86, 156, 230),
            backdrop: Color32::from_rgb(28, 28, 28),
            ribbon: Color32::from_rgb(44, 44, 44),
            tab_bar: Color32::from_rgb(36, 36, 36),
            separator: Color32::from_rgb(64, 64, 64),
            weak_text: Color32::from_rgb(160, 160, 160),
            strong_text: Color32::from_rgb(238, 238, 238),
            changed: Color32::from_rgb(230, 170, 70),
            keytip_bg: Color32::from_rgb(235, 235, 235),
            keytip_fg: Color32::from_rgb(20, 20, 20),
        },
        Scheme::Light => Palette {
            dark: false,
            accent: Color32::from_rgb(0, 103, 192),
            backdrop: Color32::from_rgb(200, 200, 200),
            ribbon: Color32::from_rgb(250, 250, 250),
            tab_bar: Color32::from_rgb(238, 238, 238),
            separator: Color32::from_rgb(214, 214, 214),
            weak_text: Color32::from_rgb(96, 96, 96),
            strong_text: Color32::from_rgb(20, 20, 20),
            changed: Color32::from_rgb(190, 110, 0),
            keytip_bg: Color32::from_rgb(40, 40, 40),
            keytip_fg: Color32::from_rgb(250, 250, 250),
        },
    }
}

/// Sets up both schemes' visuals and the text sizes, and chooses the scheme (or the
/// system's).
pub fn apply(ctx: &egui::Context, choice: egui::ThemePreference) {
    ctx.set_visuals_of(egui::Theme::Dark, visuals(Scheme::Dark));
    ctx.set_visuals_of(egui::Theme::Light, visuals(Scheme::Light));
    ctx.set_theme(choice);
    ctx.all_styles_mut(|style| {
        use egui::FontId;
        style.text_styles = [
            (TextStyle::Small, FontId::new(11.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(13.5, FontFamily::Proportional)),
            (TextStyle::Heading, FontId::new(17.0, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(12.5, FontFamily::Monospace)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(6.0, 5.0);
        style.spacing.button_padding = egui::vec2(7.0, 3.0);
        style.spacing.interact_size.y = 22.0;
        // Text is for reading here, not for selecting: a selectable label inside a clickable
        // row (a layer's name) would take the row's clicks and drags.
        style.interaction.selectable_labels = false;
    });
}

/// egui's visuals for a scheme.
fn visuals(scheme: Scheme) -> Visuals {
    let p = palette(scheme);
    let mut v = match scheme {
        Scheme::Dark => Visuals::dark(),
        Scheme::Light => Visuals::light(),
    };
    let (panel, window, field, faint) = match scheme {
        Scheme::Dark => (
            Color32::from_rgb(38, 38, 38),
            Color32::from_rgb(42, 42, 42),
            Color32::from_rgb(26, 26, 26),
            Color32::from_rgb(46, 46, 46),
        ),
        Scheme::Light => (
            Color32::from_rgb(243, 243, 243),
            Color32::from_rgb(250, 250, 250),
            Color32::from_rgb(255, 255, 255),
            Color32::from_rgb(235, 235, 235),
        ),
    };
    v.panel_fill = panel;
    v.window_fill = window;
    v.extreme_bg_color = field;
    v.faint_bg_color = faint;
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.accent.gamma_multiply(0.55);
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(6);
    v.window_stroke = Stroke::new(1.0, p.separator);
    let radius = CornerRadius::same(4);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = radius;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.separator);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.accent.gamma_multiply(0.8));
    v.widgets.active.bg_stroke = Stroke::new(1.0, p.accent);
    v
}

/// Installs the fonts: the system UI font where there is one (Segoe UI on Windows), egui's
/// built-in fonts as fallback, and the icon font, both as a fallback for inline icons and as
/// its own family.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let icons = include_bytes!("../../../assets/fonts/FluentSystemIcons-Regular.ttf");
    fonts.font_data.insert("fluent-icons".into(), Arc::new(FontData::from_static(icons)));

    if let Some(ui_font) = system_ui_font() {
        fonts.font_data.insert("system-ui".into(), Arc::new(FontData::from_owned(ui_font)));
        fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "system-ui".into());
    }
    fonts.families.entry(FontFamily::Proportional).or_default().push("fluent-icons".into());
    fonts.families.insert(icon_family(), vec!["fluent-icons".into()]);
    ctx.set_fonts(fonts);
}

/// The bytes of the operating system's interface font, if it can be found.
fn system_ui_font() -> Option<Vec<u8>> {
    #[cfg(windows)]
    {
        let dir = std::env::var_os("WINDIR").map(std::path::PathBuf::from)?.join("Fonts");
        std::fs::read(dir.join("segoeui.ttf")).ok()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// An icon as text of the given size, for buttons and labels.
pub fn icon(c: char, size: f32) -> egui::RichText {
    egui::RichText::new(c.to_string()).family(icon_family()).size(size)
}

/// Converts an sRGB color to linear light, as the canvas shader wants it.
pub fn linear(c: Color32) -> [f32; 4] {
    let d = |v: u8| opal_color::srgb_decode(f64::from(v) / 255.0) as f32;
    [d(c.r()), d(c.g()), d(c.b()), f32::from(c.a()) / 255.0]
}
