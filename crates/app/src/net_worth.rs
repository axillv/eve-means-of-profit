//! The Net worth page: what the owned items and the wallets are worth, and how that changes. The
//! page task fills the panels; [`show`] draws one panel's body.

use std::collections::BTreeSet;

use egui::{ComboBox, Ui};
use mop_core::net_worth::Part;
use mop_market::book::PriceRule;
use mop_store::{NetWorthSettings, StoreError};

use crate::Shared;
use crate::build::inputs::{PRICE_RULES, price_text};
use crate::dock;
use crate::status::Notice;

pub mod chart;
pub mod data;
pub mod record;
pub mod summary;
pub mod view;

/// The setting holding the chart's range code.
const RANGE_SETTING: &str = "net_worth.range";
/// The setting holding whether the PLEX vault counts: `1` or `0`.
const PLEX_SETTING: &str = "net_worth.plex";

const SECONDS_PER_DAY: i64 = 86_400;

/// How far back the chart reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Range {
    Days7,
    #[default]
    Days30,
    Days90,
    Year,
    All,
}

impl Range {
    /// Every range, in the order the dropdown lists them.
    pub const ALL: [Range; 5] = [
        Self::Days7,
        Self::Days30,
        Self::Days90,
        Self::Year,
        Self::All,
    ];

    /// The range's name in the dropdown.
    pub fn label(self) -> &'static str {
        match self {
            Self::Days7 => "7 days",
            Self::Days30 => "30 days",
            Self::Days90 => "90 days",
            Self::Year => "1 year",
            Self::All => "All",
        }
    }

    /// The range as the chart's axis reads it.
    pub fn over_text(self) -> &'static str {
        match self {
            Self::Days7 => "7 days",
            Self::Days30 => "30 days",
            Self::Days90 => "90 days",
            Self::Year => "1 year",
            Self::All => "all time",
        }
    }

    /// The range's saved code.
    pub fn code(self) -> &'static str {
        match self {
            Self::Days7 => "7d",
            Self::Days30 => "30d",
            Self::Days90 => "90d",
            Self::Year => "1y",
            Self::All => "all",
        }
    }

    /// The range a saved code names; an unknown code names none.
    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|range| range.code() == code)
    }

    /// The first moment the range reaches before `now` (Unix seconds); `All` and an overflow give 0.
    pub fn from(self, now: i64) -> i64 {
        let days = match self {
            Self::Days7 => 7,
            Self::Days30 => 30,
            Self::Days90 => 90,
            Self::Year => 365,
            Self::All => return 0,
        };
        now.checked_sub(days * SECONDS_PER_DAY).unwrap_or(0)
    }
}

/// What the Net worth panel shows and saves at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choices {
    pub range: Range,
    pub price: PriceRule,
    pub shown: BTreeSet<Part>,
    pub show_plex: bool,
}

impl Default for Choices {
    fn default() -> Self {
        Self {
            range: Range::default(),
            price: PriceRule::SellFivePercent,
            shown: Part::ALL.into_iter().collect(),
            show_plex: true,
        }
    }
}

#[derive(Debug, Default)]
pub struct NetWorthPage {
    pub choices: Choices,
    /// The loaded points and the headline of the picked range.
    pub view: view::View,
    /// Whether the first drawn frame has read the saved choices.
    read: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetWorthPanel {
    NetWorth,
    ByCharacter,
    RightNow,
}

impl NetWorthPanel {
    pub fn title(self) -> &'static str {
        match self {
            Self::NetWorth => "Net worth",
            Self::ByCharacter => "By character",
            Self::RightNow => "Right now",
        }
    }
}

/// Draws the body of one panel of this page.
pub fn show(page: &mut NetWorthPage, panel: NetWorthPanel, ui: &mut Ui, cx: &mut Shared) {
    if !page.read {
        page.read = true;
        match read_choices(cx) {
            Ok(choices) => page.choices = choices,
            Err(error) => cx.notices.push(Notice::error(format!(
                "Could not read the net worth settings: {error}"
            ))),
        }
    }
    page.view
        .update(cx, &page.choices, ui.ctx().cumulative_frame_nr());
    if panel != NetWorthPanel::NetWorth {
        dock::placeholder(ui, cx);
        return;
    }
    controls(page, ui, cx);
    view::show_headline(ui, cx, &page.view, &page.choices);
}

/// Reads the saved choices; any failed read leaves the defaults.
fn read_choices(cx: &mut Shared) -> Result<Choices, StoreError> {
    let mut choices = Choices::default();
    if let Some(range) = cx
        .store
        .setting(RANGE_SETTING)?
        .as_deref()
        .and_then(Range::from_code)
    {
        choices.range = range;
    }
    choices.price = cx.store.value_price()?;
    choices.shown = cx.store.net_worth_settings()?.shown;
    // An unset or unknown text keeps the vault on.
    choices.show_plex = cx.store.setting(PLEX_SETTING)?.as_deref() != Some("0");
    Ok(choices)
}

/// One line of controls: the range and price dropdowns and the part ticks.
fn controls(page: &mut NetWorthPage, ui: &mut Ui, cx: &mut Shared) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Range:");
        let ranges: Vec<(Range, String)> = Range::ALL
            .into_iter()
            .map(|range| (range, range.label().to_owned()))
            .collect();
        if let Some(range) = choose(ui, "net worth range", page.choices.range, &ranges) {
            save_range(page, cx, range);
        }
        ui.label("Price:");
        let prices: Vec<(PriceRule, String)> = PRICE_RULES
            .into_iter()
            .map(|rule| (rule, price_text(rule).to_owned()))
            .collect();
        if let Some(price) = choose(ui, "net worth price", page.choices.price, &prices) {
            save_price(page, cx, price);
        }
        ui.label("Show:");
        for part in Part::ALL {
            let mut ticked = page.choices.shown.contains(&part);
            if ui.checkbox(&mut ticked, part_label(part)).changed() {
                save_part(page, cx, part, ticked);
            }
        }
        let mut plex = page.choices.show_plex;
        if ui.checkbox(&mut plex, "PLEX vault").changed() {
            save_plex(page, cx, plex);
        }
    });
}

/// The part's name in the ticks.
fn part_label(part: Part) -> &'static str {
    match part {
        Part::Wallet => "Wallet",
        Part::Assets => "Assets",
        Part::SellOrders => "Sell orders",
        Part::BuyEscrow => "Buy escrow",
        Part::IndustryJobs => "Industry jobs",
        Part::ContractCollateral => "Contract collateral",
        Part::Contracts => "Contracts",
        Part::Implants => "Implants",
    }
}

/// Shows a choice as a dropdown. Returns the option picked this frame, if any.
fn choose<T: Copy + PartialEq>(
    ui: &mut Ui,
    salt: &str,
    current: T,
    options: &[(T, String)],
) -> Option<T> {
    let shown = options
        .iter()
        .find(|(value, _)| *value == current)
        .map_or_else(
            || "not allowed for this step".to_owned(),
            |(_, text)| text.clone(),
        );
    let mut picked = None;
    ComboBox::from_id_salt(salt)
        .selected_text(shown)
        .show_ui(ui, |ui| {
            for (value, text) in options {
                if ui
                    .selectable_label(*value == current, text.as_str())
                    .clicked()
                {
                    picked = Some(*value);
                }
            }
        });
    picked
}

fn save_range(page: &mut NetWorthPage, cx: &mut Shared, range: Range) {
    match cx.store.set_setting(RANGE_SETTING, range.code()) {
        Ok(()) => page.choices.range = range,
        Err(error) => save_error(cx, &error),
    }
}

fn save_price(page: &mut NetWorthPage, cx: &mut Shared, price: PriceRule) {
    match cx.store.set_value_price(price) {
        Ok(()) => page.choices.price = price,
        Err(error) => save_error(cx, &error),
    }
}

/// Saves the part ticks, keeping the chart's basis as it is stored.
fn save_part(page: &mut NetWorthPage, cx: &mut Shared, part: Part, ticked: bool) {
    let mut shown = page.choices.shown.clone();
    if ticked {
        shown.insert(part);
    } else {
        shown.remove(&part);
    }
    let basis = match cx.store.net_worth_settings() {
        Ok(settings) => settings.basis,
        Err(error) => {
            save_error(cx, &error);
            return;
        }
    };
    let settings = NetWorthSettings {
        basis,
        shown: shown.clone(),
    };
    match cx.store.save_net_worth_settings(&settings) {
        Ok(()) => page.choices.shown = shown,
        Err(error) => save_error(cx, &error),
    }
}

fn save_plex(page: &mut NetWorthPage, cx: &mut Shared, plex: bool) {
    let value = if plex { "1" } else { "0" };
    match cx.store.set_setting(PLEX_SETTING, value) {
        Ok(()) => page.choices.show_plex = plex,
        Err(error) => save_error(cx, &error),
    }
}

fn save_error(cx: &mut Shared, error: &StoreError) {
    cx.notices.push(Notice::error(format!(
        "Could not save the net worth settings: {error}"
    )));
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_000_000_000;

    #[test]
    fn every_range_has_a_label_over_text_and_code() {
        let labels = ["7 days", "30 days", "90 days", "1 year", "All"];
        let over = ["7 days", "30 days", "90 days", "1 year", "all time"];
        let codes = ["7d", "30d", "90d", "1y", "all"];
        assert_eq!(Range::ALL.len(), labels.len());
        for (index, range) in Range::ALL.into_iter().enumerate() {
            assert_eq!(range.label(), labels[index]);
            assert_eq!(range.over_text(), over[index]);
            assert_eq!(range.code(), codes[index]);
            assert_eq!(Range::from_code(codes[index]), Some(range));
        }
        assert_eq!(Range::from_code("2d"), None);
        assert_eq!(Range::from_code(""), None);
    }

    #[test]
    fn a_range_starts_the_asked_days_before_now_and_an_overflow_is_zero() {
        assert_eq!(Range::Days7.from(NOW), NOW - 604_800);
        assert_eq!(Range::Days30.from(NOW), NOW - 2_592_000);
        assert_eq!(Range::Days90.from(NOW), NOW - 7_776_000);
        assert_eq!(Range::Year.from(NOW), NOW - 31_536_000);
        assert_eq!(Range::All.from(NOW), 0);
        assert_eq!(Range::Days7.from(i64::MIN), 0);
    }

    #[test]
    fn the_default_choices_are_thirty_days_every_part_and_the_vault() {
        let choices = Choices::default();
        assert_eq!(choices.range, Range::Days30);
        assert_eq!(choices.price, PriceRule::SellFivePercent);
        assert_eq!(choices.shown, Part::ALL.into_iter().collect());
        assert!(choices.show_plex);
    }
}
