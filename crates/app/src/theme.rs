//! The look of the app: one bundle of colour tokens, "EVE, dense", passed by reference.
//!
//! [`Style::apply`] maps the tokens onto egui's visuals. [`Style::row_fill`] gives the background
//! of one table row. [`contrast`] is the WCAG ratio the design note's tests check.

use egui::{
    Color32, Context, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Theme, Visuals, vec2,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub bg: Color32,
    pub panel: Color32,
    pub row: Color32,
    pub line: Color32,
    pub sel: Color32,
    pub text: Color32,
    pub mute: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub stale: Color32,
    pub pos: Color32,
    pub neg: Color32,
    /// Corner radius of sheets (windows and panels).
    pub radius: u8,
    /// Corner radius of controls.
    pub control_radius: u8,
    /// Width of the page rail, in points.
    pub rail_width: f32,
}

impl Style {
    /// The dark "EVE, dense" bundle, the only one for now.
    pub fn eve_dense() -> Self {
        Self {
            bg: Color32::from_rgb(0x13, 0x14, 0x16),
            panel: Color32::from_rgb(0x1a, 0x1b, 0x1e),
            row: Color32::from_rgb(0x21, 0x23, 0x27),
            line: Color32::from_rgb(0x2c, 0x2e, 0x33),
            sel: Color32::from_rgb(0x2a, 0x24, 0x18),
            text: Color32::from_rgb(0xd2, 0xd0, 0xcb),
            mute: Color32::from_rgb(0x8d, 0x8d, 0x8a),
            accent: Color32::from_rgb(0xd6, 0xa2, 0x4a),
            on_accent: Color32::from_rgb(0x1a, 0x14, 0x05),
            stale: Color32::from_rgb(0xb0, 0x8a, 0x4a),
            pos: Color32::from_rgb(0x86, 0xa9, 0x82),
            neg: Color32::from_rgb(0xc4, 0x77, 0x68),
            radius: 12,
            control_radius: 8,
            rail_width: 80.0,
        }
    }

    /// Maps the tokens onto egui's dark visuals. Called once, when the app starts.
    pub fn apply(&self, ctx: &Context) {
        let mut style = (*ctx.global_style()).clone();
        style.visuals = Visuals::dark();
        let visuals = &mut style.visuals;
        visuals.override_text_color = Some(self.text);
        visuals.panel_fill = self.panel;
        visuals.window_fill = self.panel;
        visuals.window_stroke = Stroke::new(1.0, self.line);
        visuals.window_corner_radius = CornerRadius::same(self.radius);
        visuals.extreme_bg_color = self.bg;
        visuals.faint_bg_color = self.row;
        visuals.hyperlink_color = self.accent;
        visuals.selection.bg_fill = self.sel;
        visuals.selection.stroke = Stroke::new(1.0, self.accent);
        let control = CornerRadius::same(self.control_radius);
        let widgets = &mut visuals.widgets;
        widgets.noninteractive.bg_fill = self.panel;
        widgets.noninteractive.weak_bg_fill = self.panel;
        widgets.noninteractive.bg_stroke = Stroke::new(1.0, self.line);
        widgets.noninteractive.fg_stroke = Stroke::new(1.0, self.text);
        widgets.noninteractive.corner_radius = control;
        widgets.inactive.bg_fill = self.row;
        widgets.inactive.weak_bg_fill = self.row;
        widgets.inactive.bg_stroke = Stroke::new(1.0, self.line);
        widgets.inactive.fg_stroke = Stroke::new(1.0, self.text);
        widgets.inactive.corner_radius = control;
        widgets.hovered.bg_fill = self.line;
        widgets.hovered.weak_bg_fill = self.line;
        widgets.hovered.bg_stroke = Stroke::new(1.0, self.accent);
        widgets.hovered.fg_stroke = Stroke::new(1.0, self.text);
        widgets.hovered.corner_radius = control;
        widgets.active.bg_fill = self.sel;
        widgets.active.weak_bg_fill = self.sel;
        widgets.active.bg_stroke = Stroke::new(1.0, self.accent);
        widgets.active.fg_stroke = Stroke::new(1.0, self.text);
        widgets.active.corner_radius = control;
        style.text_styles = [
            (
                TextStyle::Small,
                FontId::new(11.0, FontFamily::Proportional),
            ),
            (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
            (
                TextStyle::Button,
                FontId::new(14.0, FontFamily::Proportional),
            ),
            (
                TextStyle::Monospace,
                FontId::new(13.0, FontFamily::Monospace),
            ),
            (
                TextStyle::Heading,
                FontId::new(18.0, FontFamily::Proportional),
            ),
        ]
        .into();
        style.spacing.interact_size.y = 24.0;
        style.spacing.button_padding = vec2(8.0, 4.0);
        ctx.set_theme(Theme::Dark);
        ctx.set_global_style(style);
    }

    /// The background of a table row: selected, then hovered, then zebra, else the page.
    pub fn row_fill(&self, zebra: bool, hovered: bool, selected: bool) -> Color32 {
        if selected {
            self.sel
        } else if hovered {
            self.line
        } else if zebra {
            self.row
        } else {
            self.bg
        }
    }
}

/// The WCAG contrast ratio between two colours, from 1 to 21, from their relative luminance.
pub fn contrast(a: Color32, b: Color32) -> f64 {
    let (x, y) = (luminance(a), luminance(b));
    let (light, dark) = if x > y { (x, y) } else { (y, x) };
    (light + 0.05) / (dark + 0.05)
}

/// The WCAG relative luminance of a colour, from 0 (black) to 1 (white).
fn luminance(color: Color32) -> f64 {
    0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
}

/// One sRGB channel, with the gamma curve removed.
fn linear(channel: u8) -> f64 {
    let value = f64::from(channel) / 255.0;
    if value <= 0.03928 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}
