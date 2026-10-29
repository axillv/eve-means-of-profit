//! Drawing the Net worth chart: the stacked areas of the shown parts, the total line, the axis,
//! the hover guide and tooltip, and the line naming the carried characters.

use egui::epaint::PathStroke;
use egui::{
    Align2, Color32, FontId, Painter, Pos2, Rect, Sense, Shape, Stroke, Ui, WidgetInfo, WidgetType,
    pos2, vec2,
};
use mop_core::money::Money;

use crate::Shared;
use crate::build::inputs::price_text;
use crate::build::prices::age_text;
use crate::net_worth::Choices;
use crate::net_worth::chart::{self, ChartError, Dot, Layer, Model};
use crate::net_worth::data::ValuedPoint;
use crate::net_worth::view::{self, View};
use crate::orders::rows::utc_text;

/// The height of the chart and its label margins, in points.
const HEIGHT: f32 = 220.0;
/// The room left of the plot for the y labels, in points.
const Y_LABEL_WIDTH: f32 = 56.0;
/// The room below the plot for the x labels, in points.
const X_LABEL_HEIGHT: f32 = 20.0;
/// The gap between an axis label and the plot, in points.
const LABEL_GAP: f32 = 4.0;
/// The size of the axis labels, in points.
const AXIS_SIZE: f32 = 11.0;
/// The width of the total line, in points.
const LINE_WIDTH: f32 = 2.0;
/// The radius of the dot of a one-point segment, in points.
const DOT_RADIUS: f32 = 3.0;
/// The radius of the hollow mark of a partial day, in points.
const PARTIAL_RADIUS: f32 = 4.0;
/// The width of a partial day's hollow mark, in points.
const PARTIAL_WIDTH: f32 = 1.5;
/// How many steps the pixel fractions are worked out in: a finer grid than the pixels need.
const FRACTION_STEPS: i128 = 10_000;

/// Draws the chart and the carried-character lines under it. Nothing is drawn without a point.
pub fn show(ui: &mut Ui, cx: &Shared, view: &View, choices: &Choices) {
    let Some(shown) = view.shown() else {
        return;
    };
    if shown.loaded.points.is_empty() {
        return;
    }
    let model = match chart::build(&shown.loaded.points, &choices.shown, choices.show_plex) {
        Ok(model) => model,
        Err(ChartError::Overflow) => {
            ui.colored_label(
                cx.style.mute,
                "Could not draw the chart: the sum is too large",
            );
            return;
        }
    };

    let (rect, _) = ui.allocate_exact_size(vec2(chart_width(ui), HEIGHT), Sense::hover());
    let plot = Rect::from_min_max(
        pos2(rect.min.x + Y_LABEL_WIDTH, rect.min.y),
        pos2(rect.max.x, rect.max.y - X_LABEL_HEIGHT),
    );
    let painter = ui.painter();
    draw_axis(painter, plot, rect, cx, &model);
    draw_areas(painter, plot, cx, &model);
    draw_total(painter, plot, cx, &model);

    let response = ui.interact(plot, ui.id().with("net_worth_chart"), Sense::hover());
    response
        .widget_info(|| WidgetInfo::labeled(WidgetType::Other, ui.is_enabled(), "Net worth chart"));
    if let Some(tip) = hover(ui, painter, plot, cx, &model, &shown.loaded.points, choices) {
        response.on_hover_text(tip);
    }

    if let Some(latest) = shown.loaded.points.last() {
        for (key, age) in chart::carried_names(latest) {
            let name = chart::character_name(&key, cx.session.characters());
            ui.colored_label(
                cx.style.mute,
                format!("{name}: last data {} ago", age_text(age)),
            );
        }
    }
}

/// The plot's width: the panel's, less the room a floating scroll bar needs at the right edge.
/// The dock's scroll bars float over the panel's content, so a chart drawn to the edge would put
/// its last days under the bar, where they take no hover.
fn chart_width(ui: &Ui) -> f32 {
    let scroll = &ui.spacing().scroll;
    let room = scroll.bar_width + 2.0 * scroll.bar_inner_margin;
    (ui.available_width() - room).max(0.0)
}

/// The grid lines and the labels of both axes.
fn draw_axis(painter: &Painter, plot: Rect, rect: Rect, cx: &Shared, model: &Model) {
    let font = FontId::proportional(AXIS_SIZE);
    for (tick, label) in chart::axis_ticks(model.top) {
        let y = y_of(tick, model.top, plot);
        painter.hline(plot.min.x..=plot.max.x, y, Stroke::new(1.0, cx.style.line));
        painter.text(
            pos2(plot.min.x - LABEL_GAP, y),
            Align2::RIGHT_CENTER,
            label,
            font.clone(),
            cx.style.mute,
        );
    }
    painter.text(
        pos2(rect.min.x, plot.max.y + LABEL_GAP),
        Align2::LEFT_TOP,
        day_text(model.first_at),
        font.clone(),
        cx.style.mute,
    );
    painter.text(
        pos2(plot.max.x, plot.max.y + LABEL_GAP),
        Align2::RIGHT_TOP,
        day_text(model.last_at),
        font,
        cx.style.mute,
    );
}

/// One filled band per layer for every joined pair of dots, the bottom layer strongest.
fn draw_areas(painter: &Painter, plot: Rect, cx: &Shared, model: &Model) {
    let color = cx.style.text;
    for (index, layer) in model.layers.iter().enumerate() {
        let alpha = layer_alpha(index);
        let fill = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha);
        let below = index
            .checked_sub(1)
            .and_then(|below| model.layers.get(below))
            .copied();
        for segment in &model.segments {
            let Some(dots) = model.dots.get(segment.clone()) else {
                continue;
            };
            for pair in dots.windows(2) {
                let [before, after] = pair else {
                    continue;
                };
                let lower = below.map_or(Money::ZERO, |below| top_of(before, below));
                let lower_after = below.map_or(Money::ZERO, |below| top_of(after, below));
                let x_before = x_of(before.at, model, plot);
                let x_after = x_of(after.at, model, plot);
                let quad = vec![
                    pos2(x_before, y_of(lower, model.top, plot)),
                    pos2(x_after, y_of(lower_after, model.top, plot)),
                    pos2(x_after, y_of(top_of(after, *layer), model.top, plot)),
                    pos2(x_before, y_of(top_of(before, *layer), model.top, plot)),
                ];
                painter.add(Shape::convex_polygon(quad, fill, PathStroke::NONE));
            }
        }
    }
}

/// The total's line through each segment, its lone dots, and the hollow marks of partial days.
fn draw_total(painter: &Painter, plot: Rect, cx: &Shared, model: &Model) {
    for segment in &model.segments {
        let Some(dots) = model.dots.get(segment.clone()) else {
            continue;
        };
        if let [dot] = dots {
            let at = pos2(x_of(dot.at, model, plot), y_of(dot.total, model.top, plot));
            painter.circle_filled(at, DOT_RADIUS, cx.style.accent);
            continue;
        }
        let points: Vec<Pos2> = dots
            .iter()
            .map(|dot| pos2(x_of(dot.at, model, plot), y_of(dot.total, model.top, plot)))
            .collect();
        painter.add(Shape::line(
            points,
            Stroke::new(LINE_WIDTH, cx.style.accent),
        ));
    }
    for dot in &model.dots {
        if dot.partial {
            let at = pos2(x_of(dot.at, model, plot), y_of(dot.total, model.top, plot));
            painter.circle_stroke(
                at,
                PARTIAL_RADIUS,
                Stroke::new(PARTIAL_WIDTH, cx.style.stale),
            );
        }
    }
}

/// The guide over the nearest dot, and the hover text of that dot's point.
fn hover(
    ui: &Ui,
    painter: &Painter,
    plot: Rect,
    cx: &Shared,
    model: &Model,
    points: &[ValuedPoint],
    choices: &Choices,
) -> Option<String> {
    let pointer = ui.ctx().pointer_hover_pos()?;
    if !plot.contains(pointer) {
        return None;
    }
    let (index, dot) = model
        .dots
        .iter()
        .enumerate()
        .min_by(|(_, left), (_, right)| {
            let left = (x_of(left.at, model, plot) - pointer.x).abs();
            let right = (x_of(right.at, model, plot) - pointer.x).abs();
            left.total_cmp(&right)
        })?;
    painter.vline(
        x_of(dot.at, model, plot),
        plot.y_range(),
        Stroke::new(1.0, cx.style.accent),
    );
    let point = points.get(index)?;
    let basis = view::basis_text(price_text(choices.price), point.source);
    Some(chart::hover_text(
        point,
        &choices.shown,
        choices.show_plex,
        &basis,
        &|key| chart::character_name(key, cx.session.characters()),
    ))
}

/// One layer's top at a dot, or zero when the layer is not in its stack.
fn top_of(dot: &Dot, layer: Layer) -> Money {
    dot.tops
        .iter()
        .find(|(held, _)| *held == layer)
        .map_or(Money::ZERO, |(_, top)| *top)
}

/// The opacity of the layer at `index`, counting from the bottom.
fn layer_alpha(index: usize) -> u8 {
    let fade = 20usize.saturating_mul(index);
    u8::try_from(200usize.saturating_sub(fade).max(40)).unwrap_or(40)
}

/// Where a point's moment sits across the plot. One moment, or a range of none, sits in the middle.
fn x_of(at: i64, model: &Model, plot: Rect) -> f32 {
    let span = model.last_at.saturating_sub(model.first_at);
    if span <= 0 {
        return plot.center().x;
    }
    let offset = at.saturating_sub(model.first_at);
    plot.min.x + fraction(offset, span) * plot.width()
}

/// Where an amount sits up the plot, from zero at the foot to `top` at the head. A zero top keeps
/// every amount at the foot.
fn y_of(amount: Money, top: Money, plot: Rect) -> f32 {
    let top = top.ten_thousandths();
    if top <= 0 {
        return plot.max.y;
    }
    let ratio = fraction(amount.ten_thousandths(), top);
    plot.max.y - ratio * plot.height()
}

/// `value / top` from 0 to 1, worked out in whole steps so no money becomes a float.
fn fraction(value: i64, top: i64) -> f32 {
    if top <= 0 {
        return 0.0;
    }
    let steps = i128::from(value.max(0)) * FRACTION_STEPS / i128::from(top);
    let steps = u16::try_from(steps.min(FRACTION_STEPS)).unwrap_or(10_000);
    f32::from(steps) / 10_000.0
}

/// A moment's UTC day, such as `2001-09-09`.
fn day_text(at: i64) -> String {
    utc_text(at)
        .split_once(' ')
        .map_or_else(|| utc_text(at), |(day, _)| day.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mop_core::net_worth::Part;

    fn plot() -> Rect {
        Rect::from_min_size(pos2(10.0, 20.0), vec2(120.0, 200.0))
    }

    fn isk(text: &str) -> Money {
        Money::parse_isk(text).expect("a test amount")
    }

    fn model(first_at: i64, last_at: i64) -> Model {
        Model {
            dots: Vec::new(),
            segments: Vec::new(),
            layers: vec![Layer::Part(Part::Wallet)],
            top: Money::ZERO,
            first_at,
            last_at,
        }
    }

    #[test]
    fn a_lone_moment_sits_in_the_middle_and_a_range_spreads_across_the_plot() {
        let plot = plot();
        let lone = model(7, 7);
        assert!((x_of(7, &lone, plot) - plot.center().x).abs() < 0.01);
        let range = model(100, 200);
        assert!((x_of(100, &range, plot) - plot.min.x).abs() < 0.01);
        assert!((x_of(200, &range, plot) - plot.max.x).abs() < 0.01);
        assert!((x_of(150, &range, plot) - plot.center().x).abs() < 0.01);
    }

    #[test]
    fn zero_is_at_the_foot_and_the_top_is_at_the_head() {
        let plot = plot();
        assert!((y_of(Money::ZERO, isk("12,050,000.00"), plot) - plot.max.y).abs() < 0.01);
        assert!((y_of(isk("12,050,000.00"), isk("12,050,000.00"), plot) - plot.min.y).abs() < 0.01);
        assert!(
            (y_of(isk("6,025,000.00"), isk("12,050,000.00"), plot) - plot.center().y).abs() < 0.01
        );
        assert!((y_of(isk("1,000.00"), Money::ZERO, plot) - plot.max.y).abs() < 0.01);
    }

    #[test]
    fn the_bottom_layer_is_the_strongest_and_the_weakest_stops_at_forty() {
        assert_eq!(layer_alpha(0), 200);
        assert_eq!(layer_alpha(1), 180);
        assert_eq!(layer_alpha(8), 40);
        assert_eq!(layer_alpha(100), 40);
    }

    #[test]
    fn a_day_is_written_as_its_date_alone() {
        assert_eq!(day_text(1_000_000_000), "2001-09-09");
        assert_eq!(day_text(0), "1970-01-01");
    }
}
