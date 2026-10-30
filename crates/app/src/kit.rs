//! The widget kit: the round-14 look's buttons and tags. Pages use it instead of styling egui
//! widgets themselves, so amber keeps meaning "act here" everywhere.

use std::sync::Arc;

use egui::{
    Color32, CornerRadius, FontId, Galley, Rect, Response, RichText, Sense, Stroke, StrokeKind,
    TextStyle, TextWrapMode, Ui, WidgetInfo, WidgetText, WidgetType, vec2,
};

use crate::theme::Style;

/// The three kinds of button the mock-up draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// A plain grey button.
    Plain,
    /// An amber-outlined button that marks the one action of a page.
    Act,
    /// A filled amber main button.
    Primary,
}

/// The colours of one button: its fill, its outline and its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colors {
    pub fill: Color32,
    pub stroke: Color32,
    pub text: Color32,
}

/// The colours of a button of `kind`: grey, amber-outlined or filled amber.
pub fn button_colors(style: &Style, kind: ButtonKind) -> Colors {
    match kind {
        ButtonKind::Plain => Colors {
            fill: style.row,
            stroke: style.line,
            text: style.text,
        },
        ButtonKind::Act => Colors {
            fill: Color32::TRANSPARENT,
            stroke: style.accent,
            text: style.accent,
        },
        ButtonKind::Primary => Colors {
            fill: style.accent,
            stroke: style.accent,
            text: style.on_accent,
        },
    }
}

/// Draws `text` as a button of `kind`: the theme's interact height, 12 px side padding, the
/// control corner radius and a one point outline.
pub fn add_button(ui: &mut Ui, style: &Style, text: &str, kind: ButtonKind) -> Response {
    let height = ui.spacing().interact_size.y;
    let font = TextStyle::Button.resolve(ui.style());
    draw(ui, style, text, kind, height, 12.0, font)
}

/// Draws `text` as a small button of `kind`, 24 px tall with 9 px side padding, in a scope so the
/// caller's spacing stays untouched.
pub fn add_small_button(ui: &mut Ui, style: &Style, text: &str, kind: ButtonKind) -> Response {
    let height = style.sizes.small_button;
    ui.scope(|ui| {
        let font = FontId::proportional(13.0);
        draw(ui, style, text, kind, height, 9.0, font)
    })
    .inner
}

/// One button, at `height` with `pad` points at either side of its text.
fn draw(
    ui: &mut Ui,
    style: &Style,
    text: &str,
    kind: ButtonKind,
    height: f32,
    pad: f32,
    font: FontId,
) -> Response {
    let colors = button_colors(style, kind);
    let galley = text_galley(ui, text, colors.text, font, kind == ButtonKind::Primary);
    let size = vec2(galley.size().x + 2.0 * pad, height);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    // The kit keeps egui's accent outline on hover; the resting outline is the kind's own.
    let stroke = if response.hovered() {
        ui.visuals().widgets.hovered.bg_stroke
    } else {
        Stroke::new(1.0, colors.stroke)
    };
    ui.painter().rect(
        rect,
        CornerRadius::same(style.control_radius),
        colors.fill,
        stroke,
        StrokeKind::Inside,
    );
    paint_centered(ui, rect, galley, colors.text);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), text));
    response
}

/// The four tones of a tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Neutral context.
    Plain,
    /// The stage a row is at.
    Stage,
    /// A warning.
    Warn,
    /// Something bad.
    Bad,
}

/// A tag's outline and text colours for `tone`.
pub fn tag_colors(style: &Style, tone: Tone) -> (Color32, Color32) {
    match tone {
        Tone::Plain => (style.tick, style.mute),
        Tone::Stage => (style.tick, style.text),
        Tone::Warn => (style.stale, style.stale),
        Tone::Bad => (style.neg, style.neg),
    }
}

/// Draws `text` as a round 18 px tag of `tone`, with an outline and no fill. A tag hovers but does
/// not click.
pub fn tag(ui: &mut Ui, style: &Style, text: &str, tone: Tone) -> Response {
    let (outline, color) = tag_colors(style, tone);
    let galley = text_galley(ui, text, color, FontId::proportional(12.0), false);
    let size = vec2(galley.size().x + 14.0, 18.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect(
        rect,
        CornerRadius::same(9),
        Color32::TRANSPARENT,
        Stroke::new(1.0, outline),
        StrokeKind::Inside,
    );
    paint_centered(ui, rect, galley, color);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, ui.is_enabled(), text));
    response
}

/// Lays `text` out on one line, strong for the main button.
fn text_galley(ui: &Ui, text: &str, color: Color32, font: FontId, strong: bool) -> Arc<Galley> {
    let rich = RichText::new(text).font(font.clone()).color(color);
    let rich = if strong { rich.strong() } else { rich };
    WidgetText::from(rich).into_galley(ui, Some(TextWrapMode::Extend), f32::INFINITY, font)
}

/// Paints `galley` in the middle of `rect`.
fn paint_centered(ui: &Ui, rect: Rect, galley: Arc<Galley>, color: Color32) {
    let pos = rect.center() - galley.size() / 2.0;
    ui.painter().galley(pos, galley, color);
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;

    fn button_harness(kind: ButtonKind, clicks: Rc<Cell<usize>>) -> Harness<'static> {
        let style = Style::eve_dense();
        Harness::builder()
            .with_size(vec2(400.0, 200.0))
            .build_ui(move |ui| {
                style.apply(ui.ctx());
                if add_button(ui, &style, "Copy multibuy", kind).clicked() {
                    clicks.set(clicks.get() + 1);
                }
            })
    }

    fn small_harness(clicks: Rc<Cell<usize>>) -> Harness<'static> {
        let style = Style::eve_dense();
        Harness::builder()
            .with_size(vec2(400.0, 200.0))
            .build_ui(move |ui| {
                style.apply(ui.ctx());
                if add_small_button(ui, &style, "Sell…", ButtonKind::Plain).clicked() {
                    clicks.set(clicks.get() + 1);
                }
            })
    }

    fn tag_harness(clicks: Rc<Cell<usize>>) -> Harness<'static> {
        let style = Style::eve_dense();
        Harness::builder()
            .with_size(vec2(400.0, 200.0))
            .build_ui(move |ui| {
                style.apply(ui.ctx());
                if tag(ui, &style, "beaten", Tone::Plain).clicked() {
                    clicks.set(clicks.get() + 1);
                }
            })
    }

    #[test]
    fn the_button_kinds_map_onto_the_theme_colours() {
        let style = Style::eve_dense();
        assert_eq!(
            button_colors(&style, ButtonKind::Plain),
            Colors {
                fill: style.row,
                stroke: style.line,
                text: style.text,
            }
        );
        assert_eq!(
            button_colors(&style, ButtonKind::Act),
            Colors {
                fill: Color32::TRANSPARENT,
                stroke: style.accent,
                text: style.accent,
            }
        );
        assert_eq!(
            button_colors(&style, ButtonKind::Primary),
            Colors {
                fill: style.accent,
                stroke: style.accent,
                text: style.on_accent,
            }
        );
    }

    #[test]
    fn the_tag_tones_map_onto_the_theme_colours() {
        let style = Style::eve_dense();
        assert_eq!(tag_colors(&style, Tone::Plain), (style.tick, style.mute));
        assert_eq!(tag_colors(&style, Tone::Stage), (style.tick, style.text));
        assert_eq!(tag_colors(&style, Tone::Warn), (style.stale, style.stale));
        assert_eq!(tag_colors(&style, Tone::Bad), (style.neg, style.neg));
    }

    #[test]
    fn the_buttons_and_the_tag_have_the_mock_ups_heights() {
        for kind in [ButtonKind::Plain, ButtonKind::Act, ButtonKind::Primary] {
            let mut harness = button_harness(kind, Rc::new(Cell::new(0)));
            harness.run_steps(2);
            let height = harness.get_by_label("Copy multibuy").rect().height();
            assert_eq!(height, 28.0);
        }

        let mut harness = small_harness(Rc::new(Cell::new(0)));
        harness.run_steps(2);
        assert_eq!(harness.get_by_label("Sell…").rect().height(), 24.0);

        let mut harness = tag_harness(Rc::new(Cell::new(0)));
        harness.run_steps(2);
        assert_eq!(harness.get_by_label("beaten").rect().height(), 18.0);
    }

    #[test]
    fn a_main_button_is_clickable_and_a_tag_is_not() {
        let clicks = Rc::new(Cell::new(0));
        let mut harness = button_harness(ButtonKind::Primary, Rc::clone(&clicks));
        harness.run_steps(2);
        assert!(harness.query_by_label("Copy multibuy").is_some());
        harness.get_by_label("Copy multibuy").click();
        harness.run_steps(2);
        assert_eq!(clicks.get(), 1);

        let clicks = Rc::new(Cell::new(0));
        let mut harness = tag_harness(Rc::clone(&clicks));
        harness.run_steps(2);
        harness.get_by_label("beaten").click();
        harness.run_steps(2);
        assert_eq!(clicks.get(), 0);
    }

    #[test]
    fn a_small_button_does_not_shrink_the_next_button() {
        let style = Style::eve_dense();
        let mut harness = Harness::builder()
            .with_size(vec2(400.0, 200.0))
            .build_ui(move |ui| {
                style.apply(ui.ctx());
                add_small_button(ui, &style, "Sell…", ButtonKind::Plain);
                add_button(ui, &style, "Copy multibuy", ButtonKind::Plain);
            });
        harness.run_steps(2);
        assert_eq!(harness.get_by_label("Sell…").rect().height(), 24.0);
        assert_eq!(harness.get_by_label("Copy multibuy").rect().height(), 28.0);
    }
}
