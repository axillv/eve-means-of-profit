//! The What could I build? page: feed products ranked by the profit a day they could make, with
//! the columns of the Summary, Standard and Technical levels.

use std::collections::BTreeSet;

use egui::{ComboBox, Id, RichText, Ui};

use mop_core::format::group_thousands;
use mop_core::industry::TypeId;
use mop_core::money::Money;

use crate::Shared;
use crate::rail::Page;
use crate::table::Table;
use crate::table::model::{Cell, Column, ColumnKind, Row};

/// The seconds in a day and an hour, for the job time text.
const HOUR_SECS: u64 = 3_600;
const DAY_SECS: u64 = 86_400;
/// The share of the daily sales a player takes, when the page starts.
const DEFAULT_SHARE_PERCENT: u8 = 5;

/// How many columns of the table are shown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Level {
    #[default]
    Summary,
    Standard,
    Technical,
}

impl Level {
    pub const ALL: [Level; 3] = [Level::Summary, Level::Standard, Level::Technical];

    pub fn label(self) -> &'static str {
        match self {
            Self::Summary => "Summary",
            Self::Standard => "Standard",
            Self::Technical => "Technical",
        }
    }
}

/// The blueprint a product's row would build from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blueprint {
    Owned { me: u8 },
    Buy { price: Money },
    ContractsOnly,
}

/// One product of the feed: what it is, how it is built, and every number the table can show.
#[derive(Debug, Clone, PartialEq)]
pub struct RankRow {
    pub type_id: TypeId,
    pub name: String,
    pub blueprint: Blueprint,
    pub warnings: Vec<String>,
    pub profit_each: Option<Money>,
    pub sell_price: Option<Money>,
    pub build_cost: Option<Money>,
    pub capital: Option<Money>,
    pub material_cost: Option<Money>,
    pub job_cost: Option<Money>,
    pub invention_cost: Option<Money>,
    pub fees: Option<Money>,
    pub middle: Option<Money>,
    pub spread: Option<Money>,
    pub sells_day: Option<u64>,
    pub capacity_day: Option<u64>,
    pub job_seconds: Option<u64>,
    pub sell_orders: Option<u64>,
    pub needs_skills: Option<String>,
}

/// The page's state: the feed, the level, the share and the rows ticked for planning.
#[derive(Debug, Clone)]
pub struct WhatToBuildPage {
    pub rows: Vec<RankRow>,
    level: Level,
    share_percent: u8,
    ticked: BTreeSet<TypeId>,
}

impl Default for WhatToBuildPage {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            level: Level::Summary,
            share_percent: DEFAULT_SHARE_PERCENT,
            ticked: BTreeSet::new(),
        }
    }
}

/// The profit a day the row's product could make at `share` percent of its daily sales, never more
/// than the day's capacity. `None` unless the profit each, the sales and the capacity are known.
pub fn profit_day(row: &RankRow, share: u8) -> Option<Money> {
    let profit_each = row.profit_each?;
    let sells = row.sells_day?;
    let capacity = row.capacity_day?;
    let units = u128::from(sells)
        .checked_mul(u128::from(share))?
        .checked_div(100)?
        .min(u128::from(capacity));
    profit_each.checked_mul_qty(u64::try_from(units).ok()?)
}

/// The profit each as tenths of a percent of the sell price, rounded half away from zero. `None`
/// when the sell price is not above zero.
pub fn margin_tenths(row: &RankRow) -> Option<i32> {
    let profit = i128::from(row.profit_each?.ten_thousandths());
    let sell = i128::from(row.sell_price?.ten_thousandths());
    if sell <= 0 {
        return None;
    }
    let numerator = profit.checked_mul(1000)?;
    i32::try_from(divide_round_half_away(numerator, sell)?).ok()
}

/// `numerator / denominator`, rounded to the nearest whole, halves away from zero.
fn divide_round_half_away(numerator: i128, denominator: i128) -> Option<i128> {
    let quotient = numerator.checked_div(denominator)?;
    let remainder = numerator.checked_rem(denominator)?.checked_abs()?;
    let twice = remainder.checked_mul(2)?;
    if twice < denominator.checked_abs()? {
        return Some(quotient);
    }
    let step = if (numerator < 0) == (denominator < 0) {
        1
    } else {
        -1
    };
    quotient.checked_add(step)
}

/// The row indices in the default order: profit a day highest first, missing last, ties in feed
/// order.
fn default_order(rows: &[RankRow], share: u8) -> Vec<usize> {
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by(|&left, &right| {
        match (
            rows.get(left).and_then(|row| profit_day(row, share)),
            rows.get(right).and_then(|row| profit_day(row, share)),
        ) {
            (Some(left), Some(right)) => right.cmp(&left),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
    });
    order
}

fn column(id: &'static str, label: &str, kind: ColumnKind, width: f32) -> Column {
    Column {
        id,
        label: label.to_owned(),
        kind,
        width,
    }
}

/// The columns of `level`, in the order the table draws them.
pub fn columns(level: Level) -> Vec<Column> {
    let mut columns = vec![
        column("tick", "", ColumnKind::Text, 32.0),
        column("item", "Item", ColumnKind::Text, 220.0),
        column("profit_day", "Profit a day", ColumnKind::Money, 120.0),
        column("margin", "Margin %", ColumnKind::Number, 90.0),
        column("sells_day", "Sells a day", ColumnKind::Number, 100.0),
        column("blueprint", "Blueprint", ColumnKind::Text, 150.0),
        column("warnings", "Warnings", ColumnKind::Text, 200.0),
    ];
    if matches!(level, Level::Standard | Level::Technical) {
        columns.extend([
            column("profit_each", "Profit each", ColumnKind::Money, 120.0),
            column("build_cost", "Build cost", ColumnKind::Money, 120.0),
            column("sell_price", "Sell price", ColumnKind::Money, 120.0),
            column("job_time", "Job time", ColumnKind::Number, 100.0),
            column("sell_orders", "Sell orders", ColumnKind::Number, 100.0),
            column("needs_skills", "Needs skills", ColumnKind::Text, 160.0),
            column("capital", "Capital to start", ColumnKind::Money, 140.0),
        ]);
    }
    if matches!(level, Level::Technical) {
        columns.extend([
            column("material_cost", "Material cost", ColumnKind::Money, 130.0),
            column("job_cost", "Job cost", ColumnKind::Money, 110.0),
            column("invention_cost", "Invention cost", ColumnKind::Money, 130.0),
            column("fees", "Fees", ColumnKind::Money, 100.0),
            column("middle", "30-day middle", ColumnKind::Money, 130.0),
            column("spread", "Buy/sell spread", ColumnKind::Money, 140.0),
        ]);
    }
    columns
}

fn money_cell(value: Option<Money>) -> Cell {
    value.map_or_else(Cell::missing, Cell::money)
}

fn count_cell(value: Option<u64>) -> Cell {
    value.map_or_else(Cell::missing, |count| {
        Cell::int(
            i128::from(count),
            group_thousands(i64::try_from(count).unwrap_or(i64::MAX)),
        )
    })
}

fn blueprint_text(blueprint: &Blueprint) -> String {
    match blueprint {
        Blueprint::Owned { me } => format!("Owned ME {me}"),
        Blueprint::Buy { price } => format!("Buy {}", price.format_isk()),
        Blueprint::ContractsOnly => "Contracts only".to_owned(),
    }
}

fn blueprint_cell(blueprint: &Blueprint) -> Cell {
    Cell::text(blueprint_text(blueprint))
}

/// The warnings of a row joined into one cell, or a dash when there are none.
fn warnings_cell(warnings: &[String]) -> Cell {
    if warnings.is_empty() {
        Cell::missing()
    } else {
        Cell::text(warnings.join(" · "))
    }
}

/// A job time as `2 h 5 m`, or `1 d 0 h` from a day on.
fn job_time_text(seconds: u64) -> String {
    let days = seconds / DAY_SECS;
    if days > 0 {
        format!("{days} d {} h", (seconds % DAY_SECS) / HOUR_SECS)
    } else {
        format!("{} h {} m", seconds / HOUR_SECS, (seconds % HOUR_SECS) / 60)
    }
}

fn job_time_cell(seconds: Option<u64>) -> Cell {
    seconds.map_or_else(Cell::missing, |seconds| {
        Cell::int(i128::from(seconds), job_time_text(seconds))
    })
}

/// Tenths of a percent as `30.3%`.
fn margin_text(tenths: i32) -> String {
    let tenths = i64::from(tenths);
    let sign = if tenths < 0 { "-" } else { "" };
    let magnitude = tenths.abs();
    format!("{sign}{}.{}%", magnitude / 10, magnitude % 10)
}

fn margin_cell(row: &RankRow) -> Cell {
    margin_tenths(row).map_or_else(Cell::missing, |tenths| {
        Cell::int(i128::from(tenths), margin_text(tenths))
    })
}

/// The cells of `row` in the order of [`columns`] for `level`.
fn cells(row: &RankRow, level: Level, share: u8) -> Vec<Cell> {
    let mut cells = vec![
        Cell::text(""),
        Cell::text(&row.name),
        money_cell(profit_day(row, share)),
        margin_cell(row),
        count_cell(row.sells_day),
        blueprint_cell(&row.blueprint),
        warnings_cell(&row.warnings),
    ];
    if matches!(level, Level::Standard | Level::Technical) {
        cells.extend([
            money_cell(row.profit_each),
            money_cell(row.build_cost),
            money_cell(row.sell_price),
            job_time_cell(row.job_seconds),
            count_cell(row.sell_orders),
            row.needs_skills
                .as_deref()
                .map_or_else(Cell::missing, Cell::text),
            money_cell(row.capital),
        ]);
    }
    if matches!(level, Level::Technical) {
        cells.extend([
            money_cell(row.material_cost),
            money_cell(row.job_cost),
            money_cell(row.invention_cost),
            money_cell(row.fees),
            money_cell(row.middle),
            money_cell(row.spread),
        ]);
    }
    cells
}

fn table_row(key: usize, row: &RankRow, level: Level, share: u8) -> Row {
    Row {
        key: u64::try_from(key).unwrap_or(u64::MAX),
        depth: 0,
        cells: cells(row, level, share),
    }
}

/// Draws the page. Returns the page a button asks for, when one was clicked.
pub fn show(ui: &mut Ui, page: &mut WhatToBuildPage, cx: &mut Shared) -> Option<Page> {
    ui.heading("What could I build?");
    ui.horizontal(|ui| {
        ui.label("Columns");
        let mut level = page.level;
        ComboBox::from_id_salt("what to build columns")
            .selected_text(level.label())
            .show_ui(ui, |ui| {
                for choice in Level::ALL {
                    ui.selectable_value(&mut level, choice, choice.label());
                }
            });
        page.level = level;
    });

    let share = page.share_percent;
    let order = default_order(&page.rows, share);
    let table_rows: Vec<Row> = order
        .iter()
        .enumerate()
        .filter_map(|(key, &index)| {
            page.rows
                .get(index)
                .map(|row| table_row(key, row, page.level, share))
        })
        .collect();
    let columns = columns(page.level);
    let mut toggled = None;
    let mut draw = |ui: &mut Ui, key: u64, column: &'static str| -> bool {
        if column != "tick" {
            return false;
        }
        let row = usize::try_from(key)
            .ok()
            .and_then(|key| order.get(key))
            .and_then(|&index| page.rows.get(index));
        let Some(row) = row else {
            return true;
        };
        let mut ticked = page.ticked.contains(&row.type_id);
        if ui.checkbox(&mut ticked, "").changed() {
            toggled = Some((row.type_id, ticked));
        }
        true
    };
    let response = Table::new(Id::new("what to build"), &columns)
        .fit_rows(table_rows.len())
        .show_with(ui, &cx.style, cx.density, &table_rows, None, &mut draw);

    if let Some((type_id, ticked)) = toggled {
        if ticked {
            page.ticked.insert(type_id);
        } else {
            page.ticked.remove(&type_id);
        }
    }
    if let Some(type_id) = response
        .clicked
        .and_then(|key| usize::try_from(key).ok())
        .and_then(|key| order.get(key))
        .and_then(|&index| page.rows.get(index))
        .map(|row| row.type_id)
    {
        cx.selection.item = Some(type_id);
    }
    if table_rows.is_empty() {
        return empty_state(ui, cx);
    }
    None
}

/// The line under the empty table: how to add a character, or that there is nothing to rank yet.
fn empty_state(ui: &mut Ui, cx: &Shared) -> Option<Page> {
    if cx.session.characters().is_empty() {
        ui.label(
            RichText::new("Add a character to use your skills, blueprints and slots.")
                .color(cx.style.text),
        );
        return ui
            .button("Add a character")
            .clicked()
            .then_some(Page::Characters);
    }
    ui.label(RichText::new("No products to rank yet.").color(cx.style.mute));
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    use egui::vec2;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use mop_store::Density;

    use crate::theme::Style;

    fn money(amount: &str) -> Money {
        Money::parse_isk(amount).expect("an amount")
    }

    fn row(name: &str) -> RankRow {
        RankRow {
            type_id: 2_456,
            name: name.to_owned(),
            blueprint: Blueprint::Owned { me: 10 },
            warnings: Vec::new(),
            profit_each: None,
            sell_price: None,
            build_cost: None,
            capital: None,
            material_cost: None,
            job_cost: None,
            invention_cost: None,
            fees: None,
            middle: None,
            spread: None,
            sells_day: None,
            capacity_day: None,
            job_seconds: None,
            sell_orders: None,
            needs_skills: None,
        }
    }

    /// A row whose profit a day is `amount` at any share with one unit a day.
    fn worth(name: &str, amount: &str) -> RankRow {
        RankRow {
            profit_each: Some(money(amount)),
            sells_day: Some(1),
            capacity_day: Some(1),
            ..row(name)
        }
    }

    #[test]
    fn the_three_levels_grow_the_columns_from_seven_to_twenty() {
        let summary = columns(Level::Summary);
        let standard = columns(Level::Standard);
        let technical = columns(Level::Technical);
        assert_eq!(summary.len(), 7);
        assert_eq!(standard.len(), 14);
        assert_eq!(technical.len(), 20);
        let ids = |columns: &[Column]| -> Vec<&'static str> {
            columns.iter().map(|column| column.id).collect()
        };
        assert_eq!(
            ids(&summary),
            [
                "tick",
                "item",
                "profit_day",
                "margin",
                "sells_day",
                "blueprint",
                "warnings",
            ]
        );
        assert_eq!(
            ids(&standard)[7..],
            [
                "profit_each",
                "build_cost",
                "sell_price",
                "job_time",
                "sell_orders",
                "needs_skills",
                "capital",
            ]
        );
        assert_eq!(
            ids(&technical)[14..],
            [
                "material_cost",
                "job_cost",
                "invention_cost",
                "fees",
                "middle",
                "spread",
            ]
        );
        assert_eq!(summary[0].label, "");
        assert_eq!(summary[1].label, "Item");
        assert_eq!(summary[2].label, "Profit a day");
        assert_eq!(summary[3].label, "Margin %");
        assert_eq!(summary[4].label, "Sells a day");
        assert_eq!(summary[5].label, "Blueprint");
        assert_eq!(summary[6].label, "Warnings");
        assert_eq!(standard[7].label, "Profit each");
        assert_eq!(standard[10].label, "Job time");
        assert_eq!(standard[13].label, "Capital to start");
        assert_eq!(technical[18].label, "30-day middle");
        assert_eq!(technical[19].label, "Buy/sell spread");
    }

    #[test]
    fn a_standard_row_shows_its_profit_margin_sales_time_blueprint_and_warnings() {
        let row = RankRow {
            warnings: vec!["Thin market".to_owned(), "Spike".to_owned()],
            profit_each: Some(money("100.00")),
            sell_price: Some(money("330.15")),
            sells_day: Some(6_900),
            capacity_day: Some(2_415),
            job_seconds: Some(7_500),
            sell_orders: Some(49),
            ..row("Hobgoblin II")
        };
        let summary = cells(&row, Level::Summary, 5);
        assert_eq!(summary[0].text, "");
        assert_eq!(summary[1].text, "Hobgoblin II");
        assert_eq!(summary[2].text, "34,500.00");
        assert_eq!(summary[3].text, "30.3%");
        assert_eq!(summary[4].text, "6,900");
        assert_eq!(summary[5].text, "Owned ME 10");
        assert_eq!(summary[6].text, "Thin market · Spike");

        let standard = cells(&row, Level::Standard, 5);
        assert_eq!(standard[7].text, "100.00");
        assert_eq!(standard[9].text, "330.15");
        assert_eq!(standard[10].text, "2 h 5 m");
        assert_eq!(standard[11].text, "49");
        assert_eq!(standard[12].text, "—");
    }

    #[test]
    fn a_missing_value_is_a_dash_and_the_blueprint_says_how_to_get_it() {
        let missing = cells(&row("Thing"), Level::Technical, 5);
        assert_eq!(missing.len(), 20);
        assert_eq!(missing[0].text, "");
        for index in [2, 3, 4, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19] {
            assert_eq!(missing[index].text, "—", "cell {index} is a dash");
        }
        assert_eq!(missing[5].text, "Owned ME 10");

        let bought = RankRow {
            blueprint: Blueprint::Buy {
                price: money("499,639.39"),
            },
            ..row("Bought")
        };
        assert_eq!(cells(&bought, Level::Summary, 5)[5].text, "Buy 499,639.39");

        let contracts = RankRow {
            blueprint: Blueprint::ContractsOnly,
            ..row("Contracts")
        };
        assert_eq!(
            cells(&contracts, Level::Summary, 5)[5].text,
            "Contracts only"
        );
    }

    #[test]
    fn profit_a_day_takes_the_share_and_stops_at_the_capacity() {
        let row = RankRow {
            profit_each: Some(money("100.00")),
            sells_day: Some(6_900),
            capacity_day: Some(2_415),
            ..row("Hobgoblin II")
        };
        assert_eq!(profit_day(&row, 5), Some(money("34,500.00")));
        assert_eq!(
            profit_day(
                &RankRow {
                    capacity_day: Some(100),
                    ..row.clone()
                },
                5
            ),
            Some(money("10,000.00"))
        );
        assert_eq!(profit_day(&row, 0), Some(Money::ZERO));
        let no_capacity = RankRow {
            capacity_day: None,
            ..row.clone()
        };
        assert_eq!(profit_day(&no_capacity, 5), None);
        let no_sales = RankRow {
            sells_day: None,
            ..row.clone()
        };
        assert_eq!(profit_day(&no_sales, 5), None);
        // The share rounds down before the capacity is applied.
        let odd = RankRow {
            sells_day: Some(101),
            capacity_day: Some(101),
            ..row
        };
        assert_eq!(profit_day(&odd, 5), Some(money("500.00")));
    }

    #[test]
    fn margin_is_tenths_of_a_percent_of_the_sell_price() {
        let row = RankRow {
            profit_each: Some(money("100.00")),
            sell_price: Some(money("330.15")),
            ..row("Hobgoblin II")
        };
        assert_eq!(margin_tenths(&row), Some(303));
        assert_eq!(margin_text(303), "30.3%");
        assert_eq!(margin_text(-303), "-30.3%");
        assert_eq!(margin_text(5), "0.5%");
        assert_eq!(margin_text(0), "0.0%");
        let free = RankRow {
            sell_price: Some(Money::ZERO),
            ..row.clone()
        };
        assert_eq!(margin_tenths(&free), None);
        let no_price = RankRow {
            sell_price: None,
            ..row
        };
        assert_eq!(margin_tenths(&no_price), None);
    }

    #[test]
    fn the_default_order_puts_the_biggest_profit_first_and_missing_last() {
        let rows = [
            worth("a", "30,000.00"),
            RankRow {
                capacity_day: None,
                ..worth("b", "50,000.00")
            },
            worth("c", "10,000.00"),
            worth("d", "20,000.00"),
        ];
        assert_eq!(default_order(&rows, 100), [0, 3, 2, 1]);
        let order: Vec<&str> = default_order(&rows, 100)
            .iter()
            .filter_map(|&index| rows.get(index).map(|row| row.name.as_str()))
            .collect();
        assert_eq!(order, ["a", "d", "c", "b"]);
    }

    #[test]
    fn equal_profits_keep_the_feed_order() {
        let rows = [worth("first", "10,000.00"), worth("second", "10,000.00")];
        assert_eq!(default_order(&rows, 5), [0, 1]);
    }

    #[test]
    fn clicking_a_header_sorts_the_table_by_that_column() {
        // Profit a day falls from Alpha to Gamma; margin rises.
        let feed = [
            RankRow {
                profit_each: Some(money("30.00")),
                sell_price: Some(money("1000.00")),
                sells_day: Some(1),
                capacity_day: Some(1),
                ..row("Alpha")
            },
            RankRow {
                profit_each: Some(money("20.00")),
                sell_price: Some(money("100.00")),
                sells_day: Some(1),
                capacity_day: Some(1),
                ..row("Beta")
            },
            RankRow {
                profit_each: Some(money("10.00")),
                sell_price: Some(money("200.00")),
                sells_day: Some(1),
                capacity_day: Some(1),
                ..row("Gamma")
            },
        ];
        let order = default_order(&feed, 100);
        let rows: Vec<Row> = order
            .iter()
            .enumerate()
            .filter_map(|(key, &index)| {
                feed.get(index)
                    .map(|row| table_row(key, row, Level::Summary, 100))
            })
            .collect();
        let columns = columns(Level::Summary);
        let style = Style::eve_dense();
        let mut harness = Harness::builder()
            .with_size(vec2(1280.0, 800.0))
            .build_ui_state(
                move |ui, seen: &mut Vec<u64>| {
                    style.apply(ui.ctx());
                    let mut frame = Vec::new();
                    let mut draw = |_: &mut Ui, key: u64, column: &'static str| -> bool {
                        if column == "item" {
                            frame.push(key);
                        }
                        false
                    };
                    Table::new(Id::new("what to build"), &columns).show_with(
                        ui,
                        &style,
                        Density::Dense,
                        &rows,
                        None,
                        &mut draw,
                    );
                    *seen = frame;
                },
                Vec::new(),
            );
        harness.run();
        assert_eq!(harness.state(), &vec![0, 1, 2], "profit a day first");

        harness.get_by_label("Margin %").click();
        harness.run();
        assert_eq!(harness.state(), &vec![1, 2, 0], "margin % highest first");
    }

    #[test]
    fn a_job_time_reads_in_hours_and_minutes_or_days_and_hours() {
        assert_eq!(job_time_text(7_500), "2 h 5 m");
        assert_eq!(job_time_text(DAY_SECS), "1 d 0 h");
        assert_eq!(job_time_text(0), "0 h 0 m");
    }
}
