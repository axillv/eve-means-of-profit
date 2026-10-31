//! The widget kit: the round-14 look's buttons and tags. Pages use it instead of styling egui
//! widgets themselves, so amber keeps meaning "act here" everywhere.

use std::sync::Arc;

use egui::{
    Align, Color32, CornerRadius, FontId, Frame, Galley, Layout, Rect, Response, RichText, Sense,
    Stroke, StrokeKind, TextStyle, TextWrapMode, Ui, UiBuilder, WidgetInfo, WidgetText, WidgetType,
    vec2,
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

/// The page frame: no fill, the page margins around `add`, laid out downwards.
pub fn page<R>(ui: &mut Ui, style: &Style, add: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::new()
        .inner_margin(vec2(style.sizes.page_margin_x, style.sizes.page_margin_y))
        .show(ui, |ui| ui.vertical(add).inner)
        .inner
}

/// The title row of a page: the title and the line under it.
#[derive(Debug, Clone, Copy)]
pub struct Header<'a> {
    /// The page title.
    pub title: &'a str,
    /// The grey line beside the title, e.g. when the page was refreshed.
    pub sub: Option<&'a str>,
}

/// Draws the page title row with `actions` at its right end, then the gap under it.
pub fn page_header(ui: &mut Ui, style: &Style, header: Header<'_>, actions: impl FnOnce(&mut Ui)) {
    // The row and its gap are exact: neither inserts the layout's item spacing.
    let spacing = ui.spacing().item_spacing;
    ui.spacing_mut().item_spacing.y = 0.0;
    let (rect, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), style.sizes.page_header),
        Sense::hover(),
    );
    {
        let mut row = ui.new_child(
            UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::left_to_right(Align::Center)),
        );
        row.spacing_mut().item_spacing.x = 14.0;
        row.label(RichText::new(header.title).heading().strong());
        if let Some(sub) = header.sub {
            row.label(RichText::new(sub).color(style.mute));
        }
        row.with_layout(Layout::right_to_left(Align::Center), |ui| actions(ui));
    }
    ui.add_space(style.sizes.page_header_gap);
    ui.spacing_mut().item_spacing.y = spacing.y;
}

/// The title row of a sheet: the title and the grey count after it.
#[derive(Debug, Clone, Copy)]
pub struct SheetHeader<'a> {
    /// The card title.
    pub title: &'a str,
    /// The grey words after the title, e.g. "2 ready to deliver".
    pub count: Option<&'a str>,
}

/// Draws the sheet title row with `right` content at its right end, then the line under it.
pub fn sheet_header(
    ui: &mut Ui,
    style: &Style,
    header: SheetHeader<'_>,
    right: impl FnOnce(&mut Ui),
) {
    // The row and the line are exact: neither inserts the layout's item spacing.
    let spacing = ui.spacing().item_spacing;
    ui.spacing_mut().item_spacing.y = 0.0;
    sheet_title_row(ui, style, header, right);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(
        rect.left()..=rect.right(),
        rect.center().y,
        Stroke::new(1.0, style.line),
    );
    ui.spacing_mut().item_spacing.y = spacing.y;
}

/// A card: the sheet frame with its title row and `body` under it.
pub fn sheet<R>(
    ui: &mut Ui,
    style: &Style,
    header: SheetHeader<'_>,
    right: impl FnOnce(&mut Ui),
    body: impl FnOnce(&mut Ui) -> R,
) -> R {
    sheet_frame(style)
        .show(ui, |ui| {
            ui.vertical(|ui| {
                sheet_header(ui, style, header, right);
                body(ui)
            })
            .inner
        })
        .inner
}

/// A card with no body: only its title row, without the line under it.
pub fn folded_sheet(
    ui: &mut Ui,
    style: &Style,
    header: SheetHeader<'_>,
    right: impl FnOnce(&mut Ui),
) {
    sheet_frame(style).show(ui, |ui| {
        ui.vertical(|ui| {
            sheet_title_row(ui, style, header, right);
        });
    });
}

/// The frame every card sits in: panel fill, a one point outline and the sheet radius.
fn sheet_frame(style: &Style) -> Frame {
    Frame::new()
        .fill(style.panel)
        .stroke(Stroke::new(1.0, style.line))
        .corner_radius(CornerRadius::same(style.radius))
}

/// The 38 px title row of a sheet, inset by the sheet padding, without a line under it.
fn sheet_title_row(
    ui: &mut Ui,
    style: &Style,
    header: SheetHeader<'_>,
    right: impl FnOnce(&mut Ui),
) {
    let (rect, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), style.sizes.sheet_header),
        Sense::hover(),
    );
    let inner = rect.shrink2(vec2(style.sizes.sheet_pad, 0.0));
    let mut row = ui.new_child(
        UiBuilder::new()
            .max_rect(inner)
            .layout(Layout::left_to_right(Align::Center)),
    );
    row.spacing_mut().item_spacing.x = 10.0;
    row.label(RichText::new(header.title).strong());
    if let Some(count) = header.count {
        row.label(RichText::new(count).color(style.mute));
    }
    row.with_layout(Layout::right_to_left(Align::Center), |ui| right(ui));
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

    fn page_harness() -> (Harness<'static>, Rc<Cell<Rect>>) {
        let style = Style::eve_dense();
        let top = Rc::new(Cell::new(Rect::NOTHING));
        let top_out = Rc::clone(&top);
        let harness = Harness::builder()
            .with_size(vec2(800.0, 400.0))
            .build_ui(move |ui| {
                style.apply(ui.ctx());
                top_out.set(ui.available_rect_before_wrap());
                page(ui, &style, |ui| {
                    ui.label("first");
                });
            });
        (harness, top)
    }

    #[test]
    fn the_page_insets_its_content_by_the_page_margins() {
        let (mut harness, top) = page_harness();
        harness.run_steps(2);
        let rect = harness.get_by_label("first").rect();
        assert_eq!(rect.left(), top.get().left() + 16.0);
        assert_eq!(rect.top(), top.get().top() + 12.0);
    }

    #[test]
    fn the_page_header_draws_its_parts_and_its_gap() {
        let style = Style::eve_dense();
        let top = Rc::new(Cell::new(Rect::NOTHING));
        let top_out = Rc::clone(&top);
        let mut harness = Harness::builder()
            .with_size(vec2(800.0, 400.0))
            .build_ui(move |ui| {
                style.apply(ui.ctx());
                top_out.set(ui.available_rect_before_wrap());
                page_header(
                    ui,
                    &style,
                    Header {
                        title: "Today",
                        sub: Some("Refreshed 18:30 EVE time, 12 min ago"),
                    },
                    |ui| {
                        add_button(ui, &style, "Refresh all", ButtonKind::Act);
                    },
                );
                ui.label("after");
            });
        harness.run_steps(2);

        assert!(harness.query_by_label("Today").is_some());
        assert!(
            harness
                .query_by_label("Refreshed 18:30 EVE time, 12 min ago")
                .is_some()
        );
        assert!(harness.query_by_label("Refresh all").is_some());
        assert_eq!(
            harness.get_by_label("after").rect().top(),
            top.get().top() + 44.0
        );
        assert_eq!(
            harness.get_by_label("Refresh all").rect().right(),
            top.get().right()
        );
    }

    #[test]
    fn a_sheet_draws_its_header_and_body_at_the_mock_up_offsets() {
        let style = Style::eve_dense();
        let top = Rc::new(Cell::new(Rect::NOTHING));
        let top_out = Rc::clone(&top);
        let mut harness = Harness::builder()
            .with_size(vec2(800.0, 400.0))
            .build_ui(move |ui| {
                style.apply(ui.ctx());
                top_out.set(ui.available_rect_before_wrap());
                sheet(
                    ui,
                    &style,
                    SheetHeader {
                        title: "Jobs",
                        count: Some("2 ready to deliver"),
                    },
                    |_ui| {},
                    |ui| {
                        ui.label("body");
                    },
                );
            });
        harness.run_steps(2);

        assert!(harness.query_by_label("Jobs").is_some());
        assert!(harness.query_by_label("2 ready to deliver").is_some());
        assert_eq!(
            harness.get_by_label("body").rect().top(),
            top.get().top() + 40.0
        );
        assert_eq!(
            harness.get_by_label("body").rect().left(),
            top.get().left() + 1.0
        );
        assert_eq!(
            harness.get_by_label("Jobs").rect().left(),
            top.get().left() + 1.0 + 14.0
        );
    }

    #[test]
    fn a_folded_sheet_is_only_its_title_row() {
        let style = Style::eve_dense();
        let height = Rc::new(Cell::new(0.0));
        let height_out = Rc::clone(&height);
        let mut harness = Harness::builder()
            .with_size(vec2(800.0, 400.0))
            .build_ui(move |ui| {
                style.apply(ui.ctx());
                let response = ui.scope(|ui| {
                    folded_sheet(
                        ui,
                        &style,
                        SheetHeader {
                            title: "Orders",
                            count: Some("nothing beaten"),
                        },
                        |_ui| {},
                    );
                });
                height_out.set(response.response.rect.height());
            });
        harness.run_steps(2);

        assert!(harness.query_by_label("Orders").is_some());
        assert!(harness.query_by_label("nothing beaten").is_some());
        assert_eq!(height.get(), 40.0);
    }
}
