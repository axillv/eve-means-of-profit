//! The Price history panel: the readings kept for each market, how long they are kept and how much
//! room they take, with the settings that change them. The sizes are estimates; the footer shows
//! the file's real size.

use egui::{Button, Checkbox, ComboBox, Id, ScrollArea, TextEdit, Ui};
use mop_core::price_history::{Retention, RetentionError, estimated_bytes, readings_kept};
use mop_market::fuzzwork::FORGE;
use mop_store::StoreError;
use mop_store::price_history::{HistoryMarket, HistorySettings, MarketKind, MarketUsage};

use super::store_text;
use crate::Shared;
use crate::table::Table;
use crate::table::model::{Cell, Column, ColumnKind, Row};

const MUTE: &str = "One reading a day is kept for each market, for as long as its rule says. Sizes are estimates; the footer shows the real file size.";
const KEPT_FOR_GOOD: &str =
    "Kept for good: these prices cannot be fetched again, so no rule or limit above thins them.";
const NO_VALUE: &str = "—";
/// The most market rows the table shows before it scrolls.
const TABLE_ROWS: usize = 6;
/// The panel reads the history again after this many seconds, so new readings show up.
const RELOAD_SECS: i64 = 10;
/// The forecast looks this many days ahead.
const YEAR_DAYS: u32 = 365;

/// The unit of the size limit. Decimal, as the sizes are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Unit {
    #[default]
    Kb,
    Mb,
    Gb,
}

impl Unit {
    fn bytes(self) -> u64 {
        match self {
            Self::Kb => 1_000,
            Self::Mb => 1_000_000,
            Self::Gb => 1_000_000_000,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Kb => "kB",
            Self::Mb => "MB",
            Self::Gb => "GB",
        }
    }
}

/// What the panel reads from the store, kept between frames for [`RELOAD_SECS`].
#[derive(Debug, Clone, Default)]
struct Snapshot {
    markets: Vec<HistoryMarket>,
    usage: Vec<MarketUsage>,
    auto_refresh: bool,
    database: u64,
    /// The saved net worth price copies: how many, and how many price rows they hold.
    copies: (u64, u64),
    at: i64,
}

/// What the panel keeps between frames, in the window's temporary memory.
#[derive(Debug, Clone, Default)]
struct Panel {
    snapshot: Option<Snapshot>,
    selected: Option<i64>,
    /// The market the rule fields below were filled in for.
    form_for: Option<i64>,
    daily: String,
    weekly: String,
    limit: String,
    unit: Unit,
    limit_filled: bool,
    confirm_remove: Option<i64>,
    /// The line under the form: `true` for an error.
    message: Option<(bool, String)>,
}

pub(super) fn show(ui: &mut Ui, cx: &mut Shared) {
    let id = Id::new("setup price history panel");
    let mut panel = ui
        .data(|data| data.get_temp::<Panel>(id))
        .unwrap_or_default();
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| draw(&mut panel, ui, cx));
    ui.data_mut(|data| data.insert_temp(id, panel));
}

/// Formats a size in decimal units: `<n> B` under 1 kB, then one decimal, rounded down.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [(u64, &str); 3] = [(1_000, "kB"), (1_000_000, "MB"), (1_000_000_000, "GB")];
    if bytes < 1_000 {
        return format!("{bytes} B");
    }
    let (unit, label) = UNITS
        .iter()
        .rev()
        .find(|(unit, _)| bytes >= *unit)
        .copied()
        .unwrap_or(UNITS[0]);
    let tenths = u128::from(bytes) * 10 / u128::from(unit);
    format!("{}.{} {label}", tenths / 10, tenths % 10)
}

fn draw(panel: &mut Panel, ui: &mut Ui, cx: &mut Shared) {
    ui.colored_label(cx.style.mute, MUTE);
    let now = (cx.clock)();
    let stale = panel.snapshot.as_ref().is_none_or(|snapshot| {
        now.checked_sub(snapshot.at)
            .is_none_or(|age| age >= RELOAD_SECS)
    });
    if stale && let Err(error) = load(panel, cx, now) {
        ui.colored_label(cx.style.neg, store_text(&error));
        return;
    }
    let Some(snapshot) = panel.snapshot.clone() else {
        return;
    };

    let rows: Vec<Row> = snapshot
        .markets
        .iter()
        .map(|market| row(market, usage_of(&snapshot, market.id)))
        .collect();
    let selected = panel.selected.and_then(|id| u64::try_from(id).ok());
    let response = Table::new(Id::new(super::SetupPanel::PriceHistory.title()), &columns())
        .selected(selected)
        .fit_rows(TABLE_ROWS)
        .show_with(
            ui,
            &cx.style,
            cx.density,
            &rows,
            None,
            &mut |_: &mut Ui, _: u64, _: &'static str| false,
        );
    if let Some(key) = response.clicked
        && let Ok(id) = i64::try_from(key)
    {
        panel.selected = Some(id);
    }

    if let Some((error, text)) = &panel.message {
        let color = if *error { cx.style.neg } else { cx.style.mute };
        ui.colored_label(color, text.as_str());
    }
    let selected_market = panel
        .selected
        .and_then(|id| snapshot.markets.iter().find(|market| market.id == id));
    match selected_market {
        Some(market) => rule_form(panel, ui, cx, market, &snapshot),
        None => {
            ui.weak("Click a market to change how long it is kept.");
        }
    }

    ui.add_space(8.0);
    limit_form(panel, ui, cx);
    let mut auto = snapshot.auto_refresh;
    if ui
        .add(Checkbox::new(&mut auto, "Refresh prices in the background"))
        .changed()
    {
        change_settings(panel, cx, |settings| settings.auto_refresh = auto);
    }

    let total = snapshot
        .usage
        .iter()
        .try_fold(0u64, |sum, usage| sum.checked_add(usage.estimated_bytes));
    let total = total.map_or(NO_VALUE.to_owned(), format_bytes);
    ui.label(format!(
        "Total {total} estimated · database file {}",
        format_bytes(snapshot.database)
    ));
    ui.label(net_worth_prices_text(snapshot.copies.0, snapshot.copies.1));
    ui.colored_label(cx.style.mute, KEPT_FOR_GOOD);
}

/// How many net worth price copies are kept, and about how much room they take.
fn net_worth_prices_text(copies: u64, rows: u64) -> String {
    if copies == 0 {
        return "Net worth prices: none yet".to_owned();
    }
    let copy = if copies == 1 { "copy" } else { "copies" };
    let row = if rows == 1 { "row" } else { "rows" };
    let size = estimated_bytes(rows, 1).map_or(NO_VALUE.to_owned(), format_bytes);
    format!("Net worth prices: {copies} {copy}, {rows} {row}, about {size}")
}

/// Reads the markets, their usage and the settings from the store.
fn load(panel: &mut Panel, cx: &mut Shared, now: i64) -> Result<(), StoreError> {
    let markets = cx.store.history_markets()?;
    let usage = cx.store.history_usage()?;
    let settings = cx.store.history_settings()?;
    let database = cx.store.database_bytes()?;
    let copies = cx.store.point_prices_size()?;
    if !panel.limit_filled {
        panel.limit_filled = true;
        if let Some(bytes) = settings.limit_bytes {
            let unit = [Unit::Gb, Unit::Mb, Unit::Kb]
                .into_iter()
                .find(|unit| bytes % unit.bytes() == 0)
                .unwrap_or(Unit::Kb);
            panel.unit = unit;
            panel.limit = (bytes / unit.bytes()).to_string();
        }
    }
    panel.snapshot = Some(Snapshot {
        markets,
        usage,
        auto_refresh: settings.auto_refresh,
        database,
        copies,
        at: now,
    });
    Ok(())
}

fn usage_of(snapshot: &Snapshot, market: i64) -> Option<&MarketUsage> {
    snapshot.usage.iter().find(|usage| usage.market == market)
}

fn columns() -> Vec<Column> {
    vec![
        column("market", "Market", 96.0),
        column("kept", "Kept", 78.0),
        column("since", "Since", 84.0),
        column("daily", "Daily for", 76.0),
        column("weekly", "Then weekly for", 108.0),
        column("size", "Size", 70.0),
        column("on", "On", 36.0),
    ]
}

fn column(id: &'static str, label: &str, width: f32) -> Column {
    Column {
        id,
        label: label.to_owned(),
        kind: ColumnKind::Text,
        width,
    }
}

fn row(market: &HistoryMarket, usage: Option<&MarketUsage>) -> Row {
    let days = usage.map_or(0, |usage| usage.days);
    let since = usage
        .and_then(|usage| usage.first_day)
        .and_then(date_text)
        .unwrap_or_else(|| NO_VALUE.to_owned());
    let weekly = market
        .retention
        .total_days
        .map_or("forever".to_owned(), |days| format!("{days} days"));
    Row {
        key: u64::try_from(market.id).unwrap_or_default(),
        depth: 0,
        cells: vec![
            Cell::text(market_name(market)),
            Cell::text(readings_text(days)),
            Cell::text(since),
            Cell::text(format!("{} days", market.retention.daily_days)),
            Cell::text(weekly),
            Cell::text(format_bytes(usage.map_or(0, |usage| usage.estimated_bytes))),
            Cell::text(if market.enabled { "✓" } else { "" }),
        ],
    }
}

fn market_name(market: &HistoryMarket) -> String {
    if market.kind == MarketKind::Region && market.place == u64::from(FORGE) {
        "The Forge".to_owned()
    } else {
        market.place.to_string()
    }
}

fn readings_text(days: u32) -> String {
    if days == 1 {
        "1 reading".to_owned()
    } else {
        format!("{days} readings")
    }
}

/// The day number as `YYYY-MM-DD`, counted from 1970-01-01. `None` when the day is out of range.
fn date_text(day: i64) -> Option<String> {
    // Civil date from a day count, after Howard Hinnant's algorithm.
    let z = day.checked_add(719_468)?;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

/// The form under the table for the selected market: how long its readings are kept.
fn rule_form(
    panel: &mut Panel,
    ui: &mut Ui,
    cx: &mut Shared,
    market: &HistoryMarket,
    snapshot: &Snapshot,
) {
    if panel.form_for != Some(market.id) {
        panel.form_for = Some(market.id);
        panel.daily = market.retention.daily_days.to_string();
        panel.weekly = market
            .retention
            .total_days
            .map_or("forever".to_owned(), |days| days.to_string());
        panel.confirm_remove = None;
    }
    let mut save = false;
    ui.horizontal_wrapped(|ui| {
        ui.label("Daily for");
        ui.add(TextEdit::singleline(&mut panel.daily).desired_width(60.0));
        ui.label("days   Then weekly for");
        ui.add(TextEdit::singleline(&mut panel.weekly).desired_width(80.0));
        ui.label("days");
        save = ui.add(Button::new("Save rule")).clicked();
    });
    if save {
        save_rule(panel, cx, market.id);
    }

    let name = market_name(market);
    let readings = readings_kept(&market.retention, YEAR_DAYS);
    let rows_per_reading = usage_of(snapshot, market.id)
        .and_then(|usage| usage.rows.checked_div(u64::from(usage.days)))
        .unwrap_or(0);
    match estimated_bytes(rows_per_reading, u64::from(readings)) {
        Ok(bytes) => {
            ui.label(format!(
                "Forecast for {name}: about {} after 1 year",
                format_bytes(bytes)
            ));
            ui.weak("an estimate, from the rows of an average reading")
        }
        Err(error) => ui.colored_label(cx.style.neg, error.to_string()),
    };

    ui.horizontal_wrapped(|ui| {
        let mut remove = false;
        if panel.confirm_remove == Some(market.id) {
            ui.label(format!("Remove {name} and all its readings?"));
            remove = ui.add(Button::new("Remove")).clicked();
            if ui.add(Button::new("Keep")).clicked() {
                panel.confirm_remove = None;
            }
        } else if ui.add(Button::new("Remove market")).clicked() {
            panel.confirm_remove = Some(market.id);
        }
        if remove {
            remove_market(panel, cx, market.id, &name);
        }
    });
}

/// Saves the daily and weekly days of a market. A bad pair keeps the old values.
fn save_rule(panel: &mut Panel, cx: &mut Shared, market: i64) {
    let Ok(daily) = panel.daily.trim().parse::<u32>() else {
        panel.message = Some((true, "Daily days must be a whole number.".to_owned()));
        return;
    };
    let weekly = match panel.weekly.trim() {
        "forever" => None,
        text => match text.parse::<u32>() {
            Ok(days) => Some(days),
            Err(_) => {
                panel.message = Some((
                    true,
                    "The weekly days must be a whole number, or forever.".to_owned(),
                ));
                return;
            }
        },
    };
    let retention = match Retention::new(daily, weekly) {
        Ok(retention) => retention,
        Err(RetentionError::ZeroDaily) => {
            panel.message = Some((true, "Daily days must be 1 or more".to_owned()));
            return;
        }
        Err(RetentionError::TotalBelowDaily) => {
            panel.message = Some((
                true,
                "The weekly part cannot be shorter than the daily part".to_owned(),
            ));
            return;
        }
        Err(error) => {
            panel.message = Some((true, error.to_string()));
            return;
        }
    };
    match cx.store.set_retention(market, retention) {
        Ok(outcome) => {
            panel.message = Some((false, format!("Dropped {} days", outcome.days_dropped)));
        }
        Err(error) => panel.message = Some((true, store_text(&error))),
    }
    panel.snapshot = None;
}

/// The size limit: a number and a unit, or nothing for no limit. Saving it thins at once.
fn limit_form(panel: &mut Panel, ui: &mut Ui, cx: &mut Shared) {
    let mut save = false;
    ui.horizontal_wrapped(|ui| {
        let caption = ui.label("Keep at most");
        ui.add(
            TextEdit::singleline(&mut panel.limit)
                .hint_text("no limit")
                .desired_width(80.0),
        )
        .labelled_by(caption.id);
        ComboBox::from_id_salt("setup price history unit")
            .selected_text(panel.unit.label())
            .show_ui(ui, |ui| {
                for unit in [Unit::Kb, Unit::Mb, Unit::Gb] {
                    ui.selectable_value(&mut panel.unit, unit, unit.label());
                }
            });
        save = ui.add(Button::new("Save limit")).clicked();
    });
    if !save {
        return;
    }
    let text = panel.limit.trim().to_owned();
    let limit = if text.is_empty() {
        None
    } else {
        let Ok(amount) = text.parse::<u64>() else {
            panel.message = Some((true, "Keep at most must be a whole number.".to_owned()));
            return;
        };
        let Some(bytes) = amount.checked_mul(panel.unit.bytes()) else {
            panel.message = Some((true, "Keep at most is too large.".to_owned()));
            return;
        };
        Some(bytes)
    };
    let saved = change_settings(panel, cx, |settings| settings.limit_bytes = limit);
    if let (true, Some(bytes)) = (saved, limit) {
        match cx.store.enforce_limit(bytes) {
            Ok(dropped) => panel.message = Some((false, format!("Dropped {dropped} days"))),
            Err(error) => panel.message = Some((true, store_text(&error))),
        }
    }
    panel.snapshot = None;
}

/// Changes the saved history settings. Returns whether the change was saved.
fn change_settings(
    panel: &mut Panel,
    cx: &mut Shared,
    change: impl FnOnce(&mut HistorySettings),
) -> bool {
    let saved = cx.store.history_settings().and_then(|mut settings| {
        change(&mut settings);
        cx.store.save_history_settings(&settings)
    });
    panel.snapshot = None;
    match saved {
        Ok(()) => true,
        Err(error) => {
            panel.message = Some((true, store_text(&error)));
            false
        }
    }
}

/// Deletes a market and all its readings, once the player has confirmed it.
fn remove_market(panel: &mut Panel, cx: &mut Shared, market: i64, name: &str) {
    match cx.store.forget_market(market) {
        Ok(()) => {
            panel.message = Some((false, format!("Removed {name}")));
            panel.selected = None;
        }
        Err(error) => panel.message = Some((true, store_text(&error))),
    }
    panel.confirm_remove = None;
    panel.snapshot = None;
}

#[cfg(test)]
mod tests {
    use super::net_worth_prices_text;

    #[test]
    fn the_net_worth_line_counts_the_copies_and_their_rows() {
        assert_eq!(net_worth_prices_text(0, 0), "Net worth prices: none yet");
        assert_eq!(
            net_worth_prices_text(1, 1),
            "Net worth prices: 1 copy, 1 row, about 100 B"
        );
        assert_eq!(
            net_worth_prices_text(2, 3),
            "Net worth prices: 2 copies, 3 rows, about 300 B"
        );
        assert_eq!(
            net_worth_prices_text(3, 1000),
            "Net worth prices: 3 copies, 1000 rows, about 100.0 kB"
        );
        assert_eq!(
            net_worth_prices_text(1, u64::MAX),
            "Net worth prices: 1 copy, 18446744073709551615 rows, about —"
        );
    }
}
