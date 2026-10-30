//! The What could I build? page: feed products ranked by the profit a day they could make, with
//! the columns of the Summary, Standard and Technical levels. Four named views set the question
//! and the sort; a Blueprint filter and a skills toggle narrow the rows.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use egui::{ComboBox, Id, RichText, Ui};

use mop_core::format::group_thousands;
use mop_core::industry::TypeId;
use mop_core::money::Money;

use crate::Shared;
use crate::rail::Page;
use crate::session::Session;
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

/// One of the four named views: which question the page answers. Each keeps its own rows and
/// sorts them by its own column.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum View {
    /// The profit a day, highest first. The default.
    #[default]
    BestPerDay,
    /// The margin, highest first.
    BestMargin,
    /// The capital to start, lowest first, within the wallets.
    CheapToStart,
    /// The profit a day, only for blueprints the characters hold.
    WithMyBlueprints,
}

impl View {
    /// The views, in the order the buttons draw them.
    pub const ALL: [View; 4] = [
        View::BestPerDay,
        View::BestMargin,
        View::CheapToStart,
        View::WithMyBlueprints,
    ];

    /// The button's text.
    pub fn label(self) -> &'static str {
        match self {
            Self::BestPerDay => "Best per day",
            Self::BestMargin => "Best margin %",
            Self::CheapToStart => "Cheap to start",
            Self::WithMyBlueprints => "With my blueprints",
        }
    }

    /// The id of the table column the view's rows are sorted by.
    pub fn sort_column(self) -> &'static str {
        match self {
            Self::BestPerDay | Self::WithMyBlueprints => "profit_day",
            Self::BestMargin => "margin",
            Self::CheapToStart => "capital",
        }
    }

    /// Whether the view sorts its column largest first; `Cheap to start` wants the smallest
    /// capital first.
    pub fn descending(self) -> bool {
        match self {
            Self::BestPerDay | Self::BestMargin | Self::WithMyBlueprints => true,
            Self::CheapToStart => false,
        }
    }

    /// Whether the view keeps the row on its own, before any filter: `Cheap to start` wants the
    /// capital known and within `wallet`, `With my blueprints` an owned blueprint. The other two
    /// views keep everything the filters kept.
    fn keeps(self, row: &RankRow, wallet: Option<Money>) -> bool {
        match self {
            Self::CheapToStart => wallet
                .zip(row.capital)
                .is_some_and(|(wallet, capital)| capital <= wallet),
            Self::WithMyBlueprints => matches!(row.blueprint, Blueprint::Owned { .. }),
            Self::BestPerDay | Self::BestMargin => true,
        }
    }

    /// How the view orders two of its rows, for `sort_by`: missing values last, and ties left
    /// for the caller to keep in feed order.
    fn compare(self, left: &RankRow, right: &RankRow, share_percent: u8) -> Ordering {
        match self {
            Self::BestPerDay | Self::WithMyBlueprints => order_desc(
                profit_day(left, share_percent),
                profit_day(right, share_percent),
            ),
            Self::BestMargin => order_desc(margin_tenths(left), margin_tenths(right)),
            Self::CheapToStart => order_asc(left.capital, right.capital),
        }
    }
}

/// The ordering of a descending sort: the greater value first, `None` (a missing value) last.
fn order_desc<T: Ord>(left: Option<T>, right: Option<T>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => right.cmp(&left),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// The ordering of an ascending sort: the lesser value first, `None` still last.
fn order_asc<T: Ord>(left: Option<T>, right: Option<T>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// Which blueprints' rows the page shows: the characters' own, the ones a character could buy, or
/// every blueprint, contracts-only rows included. `All` is the default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum BlueprintFilter {
    Mine,
    Buyable,
    #[default]
    All,
}

impl BlueprintFilter {
    /// The filters, in the order the combo lists them.
    pub const ALL: [BlueprintFilter; 3] = [
        BlueprintFilter::Mine,
        BlueprintFilter::Buyable,
        BlueprintFilter::All,
    ];

    /// The combo entry's text.
    pub fn label(self) -> &'static str {
        match self {
            Self::Mine => "Mine",
            Self::Buyable => "Buyable",
            Self::All => "All",
        }
    }

    /// Whether the row's blueprint passes the filter.
    fn keeps(self, row: &RankRow) -> bool {
        match self {
            Self::Mine => matches!(row.blueprint, Blueprint::Owned { .. }),
            Self::Buyable => matches!(row.blueprint, Blueprint::Buy { .. }),
            Self::All => true,
        }
    }
}

/// What the page filters the feed with, beside the view. Nothing here is remembered between
/// runs: a fresh start is every blueprint and every row.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RankFilters {
    pub blueprint: BlueprintFilter,
    pub only_my_skills: bool,
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

/// The page's state: the feed, the level, the share, the view and filters, and the rows ticked
/// for planning. The view and the filters start over with the app, so nothing is remembered.
#[derive(Debug, Clone)]
pub struct WhatToBuildPage {
    pub rows: Vec<RankRow>,
    level: Level,
    share_percent: u8,
    ticked: BTreeSet<TypeId>,
    view: View,
    filters: RankFilters,
    /// How many view buttons have been clicked. Each click gives the table a fresh id, so the
    /// header sort the last view or a header click left is cleared and the view's own sort shows.
    view_clicks: u64,
}

impl Default for WhatToBuildPage {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            level: Level::Summary,
            share_percent: DEFAULT_SHARE_PERCENT,
            ticked: BTreeSet::new(),
            view: View::default(),
            filters: RankFilters::default(),
            view_clicks: 0,
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

/// The rows the page shows: those the `filters` and the `view` keep, in the view's order. The
/// blueprint filter and the skills toggle narrow first, then the view takes its rows and sorts
/// them: profit a day for `BestPerDay` and `WithMyBlueprints`, margin for `BestMargin`, capital
/// within `wallet` for `CheapToStart`. Missing values sort last and ties keep the feed order.
pub fn visible<'a>(
    rows: &'a [RankRow],
    filters: &RankFilters,
    view: View,
    wallet: Option<Money>,
    share_percent: u8,
) -> Vec<&'a RankRow> {
    let mut shown: Vec<&RankRow> = rows
        .iter()
        .filter(|row| filters.blueprint.keeps(row))
        .filter(|row| !filters.only_my_skills || row.needs_skills.is_none())
        .filter(|row| view.keeps(row, wallet))
        .collect();
    // A slice sorts stably, so rows the sort ties keep the order the feed gave them.
    shown.sort_by(|left, right| view.compare(left, right, share_percent));
    shown
}

/// The login characters' wallets added up, `None` when any character's wallet has not been read
/// or the total overflows. `Cheap to start` shows no rows then.
fn wallet_sum(session: &Session) -> Option<Money> {
    session
        .characters()
        .iter()
        .try_fold(Money::ZERO, |total, character| {
            let wallet = session.character_data(character.id)?.wallet.as_ref()?.data;
            total.checked_add(wallet)
        })
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

/// Draws the view buttons, which set the question the page answers and its sort.
fn draw_views(ui: &mut Ui, page: &mut WhatToBuildPage) {
    ui.horizontal(|ui| {
        for choice in View::ALL {
            if ui
                .selectable_value(&mut page.view, choice, choice.label())
                .clicked()
            {
                // The next frame draws a fresh table without the header sort this click left.
                page.view_clicks = page.view_clicks.saturating_add(1);
            }
        }
    });
}

/// Draws the Blueprint filter and the skills toggle under the view buttons.
fn draw_filters(ui: &mut Ui, page: &mut WhatToBuildPage) {
    ui.horizontal(|ui| {
        ui.label("Blueprint");
        let mut blueprint = page.filters.blueprint;
        ComboBox::from_id_salt("what to build blueprint")
            .selected_text(blueprint.label())
            .show_ui(ui, |ui| {
                for choice in BlueprintFilter::ALL {
                    ui.selectable_value(&mut blueprint, choice, choice.label());
                }
            });
        page.filters.blueprint = blueprint;
        ui.checkbox(
            &mut page.filters.only_my_skills,
            "Only what my skills allow",
        );
    });
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
    draw_views(ui, page);
    draw_filters(ui, page);

    let share = page.share_percent;
    let wallet = wallet_sum(&cx.session);
    // The rows borrow the feed, so the table gets its own rows and picks from them instead.
    let (table_rows, picks) = {
        let shown = visible(&page.rows, &page.filters, page.view, wallet, share);
        let table_rows: Vec<Row> = shown
            .iter()
            .enumerate()
            .map(|(key, row)| table_row(key, row, page.level, share))
            .collect();
        let picks: Vec<TypeId> = shown.iter().map(|row| row.type_id).collect();
        (table_rows, picks)
    };
    let columns = columns(page.level);
    // A view button gives the table a fresh id, so the sort a header click left is cleared and
    // the order of the view the buttons kept shows; a header click under the new id sorts by any
    // column until the next view button.
    let table_id = Id::new("what to build").with((page.view, page.view_clicks));
    let mut toggled = None;
    let mut draw = |ui: &mut Ui, key: u64, column: &'static str| -> bool {
        if column != "tick" {
            return false;
        }
        let type_id = usize::try_from(key)
            .ok()
            .and_then(|key| picks.get(key))
            .copied();
        let Some(type_id) = type_id else {
            return true;
        };
        let mut ticked = page.ticked.contains(&type_id);
        if ui.checkbox(&mut ticked, "").changed() {
            toggled = Some((type_id, ticked));
        }
        true
    };
    let response = Table::new(table_id, &columns)
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
        .and_then(|key| picks.get(key))
        .copied()
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

    use std::io::Write;
    use std::ops::ControlFlow;
    use std::sync::{Arc, Mutex};

    use egui::vec2;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use mop_esi::character_data::{CharacterData, Fetched};
    use mop_esi::characters::{Character, Login};
    use mop_esi::industry::IndustryData;
    use mop_market::cache::{Http, HttpResponse};
    use mop_market::error::MarketError;
    use mop_market::fuzzwork::{BytesHttp, BytesResponse};
    use mop_sde::{Fetch, FetchError};
    use mop_store::{Density, Store};
    use tempfile::TempDir;

    use crate::claims::ClaimsState;
    use crate::selection::Selection;
    use crate::session::NoEve;
    use crate::theme::Style;
    use crate::worker::Worker;

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

    /// The four rows the view and filter tests rank: A and F are owned, B is bought and C can
    /// only come from a contract. At share 5 they make 100.00, 200.00, nothing and 50.00 a day
    /// with margins of 30%, 10% and 20%, and capitals to start of 50.00, 200.00 and 100.00; C is
    /// missing every number.
    fn feed() -> Vec<RankRow> {
        let listed = |name: &str,
                      blueprint: Blueprint,
                      profit_each: &str,
                      sell_price: &str,
                      capital: &str| {
            RankRow {
                blueprint,
                profit_each: Some(money(profit_each)),
                sell_price: Some(money(sell_price)),
                sells_day: Some(20),
                capacity_day: Some(20),
                capital: Some(money(capital)),
                ..row(name)
            }
        };
        vec![
            listed(
                "A",
                Blueprint::Owned { me: 10 },
                "100.00",
                "333.3333",
                "50.00",
            ),
            listed(
                "B",
                Blueprint::Buy {
                    price: money("499,639.39"),
                },
                "200.00",
                "2,000.00",
                "200.00",
            ),
            RankRow {
                blueprint: Blueprint::ContractsOnly,
                ..row("C")
            },
            listed("F", Blueprint::Owned { me: 5 }, "50.00", "250.00", "100.00"),
        ]
    }

    fn character(id: u64) -> Character {
        Character {
            id,
            name: format!("Pilot {id}"),
            scopes: BTreeSet::new(),
            added_at: 1_000_000,
            login: Login::Ok,
        }
    }

    /// One character's saved data holding `wallet`.
    fn wallet_data(id: u64, wallet: Money) -> CharacterData {
        CharacterData {
            character_id: id,
            assets: None,
            orders: None,
            jobs: None,
            wallet: Some(Fetched {
                data: wallet,
                fetched_at: 1_000_000,
                last_modified: None,
                expires: None,
            }),
        }
    }

    /// A session with one login character per amount, each having read that wallet.
    fn session_with(wallets: &[Money]) -> Session {
        let mut session = Session::new(Box::new(NoEve));
        for (index, amount) in wallets.iter().enumerate() {
            let id = u64::try_from(index + 1).expect("a small id");
            session.put_character(character(id));
            session.replace(wallet_data(id, *amount));
        }
        session
    }

    #[test]
    fn the_four_views_name_their_label_column_and_direction() {
        assert_eq!(
            View::ALL.map(View::label),
            [
                "Best per day",
                "Best margin %",
                "Cheap to start",
                "With my blueprints"
            ]
        );
        assert_eq!(
            View::ALL.map(View::sort_column),
            ["profit_day", "margin", "capital", "profit_day"]
        );
        assert_eq!(View::ALL.map(View::descending), [true, true, false, true]);
    }

    #[test]
    fn the_filters_start_on_every_blueprint_with_the_skills_toggle_off() {
        let defaults = RankFilters::default();
        assert_eq!(defaults.blueprint, BlueprintFilter::All);
        assert!(!defaults.only_my_skills);
        assert_eq!(View::default(), View::BestPerDay, "the default view");
        assert_eq!(
            BlueprintFilter::ALL.map(BlueprintFilter::label),
            ["Mine", "Buyable", "All"]
        );
    }

    #[test]
    fn the_four_views_rank_the_feed_in_their_own_way() {
        let rows = feed();
        let names = |view: View| {
            let shown = visible(
                &rows,
                &RankFilters::default(),
                view,
                Some(money("120.00")),
                5,
            );
            shown
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(View::BestPerDay), ["B", "A", "F", "C"]);
        assert_eq!(names(View::BestMargin), ["A", "F", "B", "C"]);
        assert_eq!(names(View::CheapToStart), ["A", "F"]);
        assert_eq!(names(View::WithMyBlueprints), ["A", "F"]);
    }

    #[test]
    fn the_blueprint_filter_keeps_mine_buyable_or_every_blueprint() {
        let rows = feed();
        let names = |blueprint: BlueprintFilter| {
            let filters = RankFilters {
                blueprint,
                ..RankFilters::default()
            };
            let shown = visible(&rows, &filters, View::BestPerDay, None, 5);
            shown
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(BlueprintFilter::All),
            ["B", "A", "F", "C"],
            "All gives 4 rows"
        );
        assert_eq!(names(BlueprintFilter::Mine), ["A", "F"]);
        assert_eq!(names(BlueprintFilter::Buyable), ["B"]);
    }

    #[test]
    fn only_my_skills_leaves_the_rows_that_need_a_skill_out() {
        let mut rows = feed();
        // F is the last row of the feed.
        rows[3].needs_skills = Some("Drones V (you: III)".to_owned());
        let filters = RankFilters {
            only_my_skills: true,
            ..RankFilters::default()
        };
        let mut names: Vec<&str> = visible(&rows, &filters, View::BestPerDay, None, 5)
            .iter()
            .map(|row| row.name.as_str())
            .collect();
        names.sort_unstable();
        assert_eq!(names, ["A", "B", "C"], "F needs a skill the toggle hides");

        let shown = visible(&rows, &RankFilters::default(), View::BestPerDay, None, 5);
        assert_eq!(shown.len(), 4, "the toggle is off, so F stays");
    }

    #[test]
    fn cheap_to_start_keeps_capital_within_the_summed_wallet() {
        let session = session_with(&[money("100.00"), money("20.00")]);
        let wallet = wallet_sum(&session).expect("both wallets were read");
        assert_eq!(wallet, money("120.00"), "the two wallets add up");

        let rows = [
            RankRow {
                capital: Some(money("120.00")),
                ..row("exact")
            },
            RankRow {
                capital: Some(money("120.01")),
                ..row("over")
            },
            RankRow {
                capital: Some(money("50.00")),
                ..row("small")
            },
            row("missing"),
        ];
        let at = |wallet: Option<Money>| {
            let shown = visible(
                &rows,
                &RankFilters::default(),
                View::CheapToStart,
                wallet,
                5,
            );
            shown
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            at(Some(wallet)),
            ["small", "exact"],
            "120.00 fits, 120.01 and a missing capital do not"
        );
        assert_eq!(
            at(Some(money("119.99"))),
            ["small"],
            "under 120.00 only the 50.00 row fits"
        );
        assert!(at(None).is_empty(), "no wallet gives no rows");
    }

    #[test]
    fn a_wallet_sum_is_none_when_a_wallet_is_missing_or_the_total_overflows() {
        let mut missing = Session::new(Box::new(NoEve));
        missing.put_character(character(1));
        missing.replace(wallet_data(1, money("100.00")));
        missing.put_character(character(2));
        assert_eq!(
            wallet_sum(&missing),
            None,
            "the second wallet was never read"
        );

        let biggest = Money::from_ten_thousandths(i64::MAX);
        assert_eq!(
            wallet_sum(&session_with(&[biggest, biggest])),
            None,
            "the sum overflows"
        );
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
        let shown = visible(&rows, &RankFilters::default(), View::BestPerDay, None, 100);
        let names: Vec<&str> = shown.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, ["a", "d", "c", "b"], "the missing one is last");
    }

    #[test]
    fn equal_profits_keep_the_feed_order() {
        let rows = [worth("first", "10,000.00"), worth("second", "10,000.00")];
        let shown = visible(&rows, &RankFilters::default(), View::BestPerDay, None, 5);
        let names: Vec<&str> = shown.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, ["first", "second"]);
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
        let shown = visible(&feed, &RankFilters::default(), View::BestPerDay, None, 100);
        let rows: Vec<Row> = shown
            .iter()
            .copied()
            .enumerate()
            .map(|(key, row)| table_row(key, row, Level::Summary, 100))
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

    /// The page and the app state it draws with, so a test can click the controls of `show`.
    struct Rig {
        page: WhatToBuildPage,
        cx: Shared,
        /// The folder the store lives in, kept for as long as the rig.
        _dir: TempDir,
    }

    /// Nothing is fetched in these tests.
    struct NoNetwork;

    impl Fetch for NoNetwork {
        fn get_text(&self, _url: &str) -> Result<String, FetchError> {
            Err(FetchError::Http { status: 503 })
        }

        fn download(
            &self,
            _url: &str,
            _dest: &mut dyn Write,
            _progress: &mut dyn FnMut(u64, Option<u64>) -> ControlFlow<()>,
        ) -> Result<u64, FetchError> {
            Err(FetchError::Http { status: 503 })
        }
    }

    /// A market that never answers: no price is read in this test.
    struct NoPrices;

    impl BytesHttp for NoPrices {
        fn get_bytes(
            &self,
            _url: &str,
            _if_none_match: Option<&str>,
        ) -> Result<BytesResponse, MarketError> {
            Err(MarketError::Network("not in this test".to_owned()))
        }
    }

    /// The PLEX market that never answers either.
    struct NoPlex;

    impl Http for NoPlex {
        fn get(
            &self,
            _url: &str,
            _if_none_match: Option<&str>,
        ) -> Result<HttpResponse, MarketError> {
            Err(MarketError::Network("not in this test".to_owned()))
        }
    }

    /// The rig: a fresh store, a worker nothing asks, and one character holding `wallet`, so
    /// `Cheap to start` has a wallet to count the capital against.
    fn rig(page: WhatToBuildPage, wallet: Money) -> Rig {
        let dir = TempDir::new().expect("a temp folder");
        let store = Store::open(&dir.path().join("user.sqlite"), 1_000_000)
            .expect("the store opens")
            .store;
        Rig {
            page,
            cx: Shared {
                sde: None,
                store,
                worker: Worker::spawn(egui::Context::default(), Box::new(NoNetwork)),
                market: Arc::new(crate::build::prices::Prices::new(Box::new(NoPrices))),
                plex_http: Arc::new(NoPlex),
                copy_notices: Arc::new(Mutex::new(Vec::new())),
                style: Style::eve_dense(),
                density: Density::Normal,
                rail_icons_only: false,
                data_dir: dir.path().to_path_buf(),
                notices: Vec::new(),
                clock: Box::new(|| 1_000_000),
                session: session_with(&[wallet]),
                selection: Selection::default(),
                industry: IndustryData::default(),
                claims: ClaimsState::default(),
            },
            _dir: dir,
        }
    }

    /// Alpha, Beta and Gamma: every view orders them differently, so a click on a view button or
    /// a header changes the order visibly.
    fn trio() -> Vec<RankRow> {
        let product = |name: &str, profit_each: &str, sell: &str, capital: &str| RankRow {
            blueprint: Blueprint::Owned { me: 10 },
            profit_each: Some(money(profit_each)),
            sell_price: Some(money(sell)),
            sells_day: Some(20),
            capacity_day: Some(20),
            capital: Some(money(capital)),
            ..row(name)
        };
        vec![
            // 300.00 a day at a 30% margin, first by profit and margin, last to start.
            product("Alpha", "300.00", "1,000.00", "300.00"),
            // 100.00 a day at 20%, last by profit, first by capital.
            product("Beta", "100.00", "500.00", "10.00"),
            // 200.00 a day at a 10% margin, in the middle every way but capital.
            product("Gamma", "200.00", "20,000.00", "200.00"),
        ]
    }

    /// The names among `names`, in the order the last frame drew them, top first.
    fn ranked<'h>(harness: &'h Harness<'_, Rig>, names: &[&'h str]) -> Vec<&'h str> {
        let mut placed: Vec<(&str, f32)> = names
            .iter()
            .map(|&name| {
                let node = harness
                    .query_all_by_label(name)
                    .next()
                    .or_else(|| harness.query_all_by_value(name).next())
                    .unwrap_or_else(|| panic!("no node showing {name:?}"));
                (name, node.rect().min.y)
            })
            .collect();
        placed.sort_by(|left, right| left.1.total_cmp(&right.1));
        placed.into_iter().map(|(name, _)| name).collect()
    }

    #[test]
    fn a_view_click_clears_the_header_sort_and_a_header_click_sorts_until_the_next_view() {
        let page = WhatToBuildPage {
            rows: trio(),
            ..WhatToBuildPage::default()
        };
        let mut harness = Harness::builder()
            .with_size(vec2(1280.0, 800.0))
            .build_ui_state(
                move |ui, rig: &mut Rig| {
                    rig.cx.style.apply(ui.ctx());
                    let opened = show(ui, &mut rig.page, &mut rig.cx);
                    assert!(opened.is_none(), "the test clicks no page button");
                },
                rig(page, money("300.00")),
            );
        harness.run();
        let all = ["Alpha", "Beta", "Gamma"];
        assert_eq!(
            ranked(&harness, &all),
            ["Alpha", "Gamma", "Beta"],
            "the default view sorts by profit a day"
        );

        harness.get_by_label("Margin %").click();
        harness.run();
        assert_eq!(
            ranked(&harness, &all),
            ["Alpha", "Beta", "Gamma"],
            "a header click sorts by its column"
        );

        harness.get_by_label("Cheap to start").click();
        harness.run();
        assert_eq!(
            ranked(&harness, &all),
            ["Beta", "Gamma", "Alpha"],
            "the view clears the header sort and sorts by capital"
        );

        harness.get_by_label("Profit a day").click();
        harness.run();
        assert_eq!(
            ranked(&harness, &all),
            ["Alpha", "Gamma", "Beta"],
            "a header click sorts until the next view click"
        );

        harness.get_by_label("Best margin %").click();
        harness.run();
        assert_eq!(
            ranked(&harness, &all),
            ["Alpha", "Beta", "Gamma"],
            "the next view click clears that sort too"
        );
    }
}
