//! The Value prices panel: the app-wide price the net worth is valued at, and the price rules for
//! one item, a group or a category.

use std::collections::BTreeMap;

use egui::{Button, ComboBox, Id, ScrollArea, Ui};
use mop_core::money::Money;
use mop_market::book::PriceRule;
use mop_sde::{Sde, SdeError};
use mop_store::settings::{RuleTarget, RuleValue};

use super::fields::text_field;
use crate::Shared;
use crate::build::inputs::{PRICE_RULES, price_text};
use crate::status::Notice;
use crate::table::Table;
use crate::table::model::{Cell, Column, ColumnKind, Row};

const NO_RULES: &str = "No rules yet.";
const NO_DATA: &str = "Load the game data to add rules.";
const HOVER: &str = "Changing this re-prices the whole net worth chart.";
const FIXED: &str = "Fixed ISK";
const BAD_PRICE: &str = "Type a price in ISK, like 29,000,000.";
const NEGATIVE: &str = "A fixed price cannot be negative.";
/// The most rule rows the table shows before it scrolls.
const TABLE_ROWS: usize = 8;

/// What a new rule is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    #[default]
    Item,
    Group,
    Category,
}

/// How a new rule prices its target: a market statistic, or a typed fixed price.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Market(PriceRule),
    Fixed,
}

impl Default for Choice {
    fn default() -> Self {
        Self::Market(PriceRule::SellFivePercent)
    }
}

/// What the panel keeps between frames, in the window's temporary memory.
#[derive(Debug, Clone, Default)]
struct Panel {
    scope: Scope,
    name: String,
    choice: Choice,
    fixed: String,
}

pub(super) fn show(ui: &mut Ui, cx: &mut Shared) {
    let id = Id::new("setup value prices panel");
    let mut panel = ui
        .data(|data| data.get_temp::<Panel>(id))
        .unwrap_or_default();
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| draw(&mut panel, ui, cx));
    ui.data_mut(|data| data.insert_temp(id, panel));
}

fn draw(panel: &mut Panel, ui: &mut Ui, cx: &mut Shared) {
    match cx.store.value_price() {
        Ok(current) => value_price(ui, cx, current),
        Err(error) => {
            ui.colored_label(
                cx.style.neg,
                format!("Could not read the value price: {error}"),
            );
        }
    }

    let now = (cx.clock)();
    let rules = match cx.store.price_rules_at(now) {
        Ok(rules) => rules,
        Err(error) => {
            ui.colored_label(cx.style.neg, format!("Could not read the rules: {error}"));
            return;
        }
    };
    ui.add_space(4.0);
    rules_table(ui, cx, &rules, now);
    add_form(panel, ui, cx);
}

/// The app-wide `Value items at` dropdown. A saved rule changes the whole net worth chart at once.
fn value_price(ui: &mut Ui, cx: &mut Shared, current: PriceRule) {
    // A setting saved before the rule list changed may still hold `WalkSell`; it reads as the
    // default's label.
    let shown = if current == PriceRule::WalkSell {
        PriceRule::SellFivePercent
    } else {
        current
    };
    let mut picked = shown;
    ui.horizontal(|ui| {
        ui.label("Value items at");
        ComboBox::from_id_salt("setup value prices app wide")
            .selected_text(price_text(shown))
            .show_ui(ui, |ui| {
                for rule in PRICE_RULES {
                    ui.selectable_value(&mut picked, rule, price_text(rule));
                }
            })
            .response
            .on_hover_text(HOVER);
    });
    if picked == shown {
        return;
    }
    if let Err(error) = cx.store.set_value_price(picked) {
        cx.notices.push(Notice::error(format!(
            "Could not save the value price: {error}"
        )));
    }
}

/// The rules table: what each rule applies to, how it prices it and a button that removes it.
fn rules_table(ui: &mut Ui, cx: &mut Shared, rules: &BTreeMap<RuleTarget, RuleValue>, now: i64) {
    if rules.is_empty() {
        ui.colored_label(cx.style.mute, NO_RULES);
        return;
    }
    let targets: Vec<RuleTarget> = rules.keys().copied().collect();
    let rows: Vec<Row> = rules
        .iter()
        .enumerate()
        .map(|(index, (target, value))| rule_row(index, *target, *value, cx.sde.as_ref()))
        .collect();
    let mut remove = None;
    {
        let mut draw_cell = |ui: &mut Ui, key: u64, column: &'static str| -> bool {
            if column != "remove" {
                return false;
            }
            if ui.add(Button::new("Remove")).clicked() {
                remove = usize::try_from(key)
                    .ok()
                    .and_then(|index| targets.get(index).copied());
            }
            true
        };
        Table::new(Id::new(super::SetupPanel::ValuePrices.title()), &columns())
            .fit_rows(TABLE_ROWS)
            .show_with(ui, &cx.style, cx.density, &rows, None, &mut draw_cell);
    }
    if let Some(target) = remove {
        match cx.store.set_price_rule(target, None, now) {
            Ok(()) => {}
            Err(error) => cx
                .notices
                .push(Notice::error(format!("Could not save the rule: {error}"))),
        }
    }
}

fn rule_row(index: usize, target: RuleTarget, value: RuleValue, sde: Option<&Sde>) -> Row {
    let name = target_name(sde, target);
    Row {
        key: u64::try_from(index).unwrap_or_default(),
        depth: 0,
        cells: vec![
            Cell::text(target_text(target, name.as_deref())),
            Cell::text(value_text(value)),
            Cell::text(""),
        ],
    }
}

/// The name the game data has for a rule's target, if it has one.
fn target_name(sde: Option<&Sde>, target: RuleTarget) -> Option<String> {
    let sde = sde?;
    match target {
        RuleTarget::Item(id) => sde.item(id).ok().flatten().map(|item| item.name),
        RuleTarget::Group(id) => sde.group_name(id).ok().flatten(),
        RuleTarget::Category(id) => sde.category_name(id).ok().flatten(),
    }
}

fn columns() -> Vec<Column> {
    vec![
        column("applies", "Applies to", 220.0),
        column("price", "Price", 190.0),
        column("remove", "", 80.0),
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

/// The form that adds a rule: what it applies to, its name and how it prices it.
fn add_form(panel: &mut Panel, ui: &mut Ui, cx: &mut Shared) {
    if cx.sde.is_none() {
        ui.add_space(4.0);
        ui.colored_label(cx.style.mute, NO_DATA);
        return;
    }
    ui.separator();
    ui.horizontal(|ui| {
        ui.radio_value(&mut panel.scope, Scope::Item, "An item");
        ui.radio_value(&mut panel.scope, Scope::Group, "A group");
        ui.radio_value(&mut panel.scope, Scope::Category, "A category");
    });
    text_field(
        ui,
        &cx.style,
        "Applies to",
        &mut panel.name,
        "Item, group or category name",
        None,
    );
    ui.horizontal(|ui| {
        let caption = ui.label("Price");
        let mut choice = panel.choice;
        ComboBox::from_id_salt("setup value prices add price")
            .selected_text(choice_text(choice))
            .show_ui(ui, |ui| {
                for rule in PRICE_RULES {
                    ui.selectable_value(&mut choice, Choice::Market(rule), price_text(rule));
                }
                ui.selectable_value(&mut choice, Choice::Fixed, FIXED);
            })
            .response
            .labelled_by(caption.id);
        panel.choice = choice;
    });
    if panel.choice == Choice::Fixed {
        text_field(
            ui,
            &cx.style,
            "Fixed price (ISK)",
            &mut panel.fixed,
            "29,000,000",
            None,
        );
    }
    let add = ui.add(Button::new("Add rule")).clicked();
    if add {
        add_rule(panel, cx);
    }
}

fn choice_text(choice: Choice) -> &'static str {
    match choice {
        Choice::Market(rule) => price_text(rule),
        Choice::Fixed => FIXED,
    }
}

/// Looks the typed name up, builds the rule and saves it. On success the name and fixed price are
/// emptied so the next rule starts clean.
fn add_rule(panel: &mut Panel, cx: &mut Shared) {
    let name = panel.name.trim().to_owned();
    let looked_up = match cx.sde.as_ref() {
        Some(sde) => lookup(sde, panel.scope, &name),
        None => return,
    };
    let target = match looked_up {
        Ok(target) => target,
        Err(error) => {
            cx.notices.push(Notice::error(format!(
                "Could not read the game data: {error}"
            )));
            return;
        }
    };
    match build_rule(target, &name, panel.scope, panel.choice, &panel.fixed) {
        Ok((target, value)) => {
            let now = (cx.clock)();
            match cx.store.set_price_rule(target, Some(value), now) {
                Ok(()) => {
                    panel.name.clear();
                    panel.fixed.clear();
                }
                Err(error) => cx
                    .notices
                    .push(Notice::error(format!("Could not save the rule: {error}"))),
            }
        }
        Err(text) => cx.notices.push(Notice::error(text)),
    }
}

/// The exact id the typed name has in the game data, as a rule target.
fn lookup(sde: &Sde, scope: Scope, name: &str) -> Result<Option<RuleTarget>, SdeError> {
    match scope {
        Scope::Item => match sde.find_exact(name)? {
            Some(id) => Ok(sde.item(id)?.map(|item| RuleTarget::Item(item.type_id))),
            None => Ok(None),
        },
        Scope::Group => Ok(sde.find_group(name)?.map(RuleTarget::Group)),
        Scope::Category => Ok(sde.find_category(name)?.map(RuleTarget::Category)),
    }
}

/// What a rule applies to, named from the game data when it has the name.
pub fn target_text(target: RuleTarget, name: Option<&str>) -> String {
    match (target, name) {
        (RuleTarget::Item(_), Some(name)) => name.to_owned(),
        (RuleTarget::Item(id), None) => format!("Item {id}"),
        (RuleTarget::Group(_), Some(name)) => format!("{name} (group)"),
        (RuleTarget::Group(id), None) => format!("Group {id}"),
        (RuleTarget::Category(_), Some(name)) => format!("{name} (category)"),
        (RuleTarget::Category(id), None) => format!("Category {id}"),
    }
}

/// How a rule prices its target: the market statistic's label, or `Fixed <price> ISK`.
pub fn value_text(value: RuleValue) -> String {
    match value {
        RuleValue::Market(rule) => price_text(rule).to_owned(),
        RuleValue::Fixed(price) => format!("Fixed {} ISK", price.format_isk()),
    }
}

/// Builds the rule the form describes. A missing target names what was not found by `scope`; a
/// fixed price must parse and not be negative.
pub fn build_rule(
    target: Option<RuleTarget>,
    name: &str,
    scope: Scope,
    price: Choice,
    fixed: &str,
) -> Result<(RuleTarget, RuleValue), String> {
    let Some(target) = target else {
        let name = name.trim();
        return Err(match scope {
            Scope::Item => format!("No item called \"{name}\"."),
            Scope::Group => format!("No group called \"{name}\"."),
            Scope::Category => format!("No category called \"{name}\"."),
        });
    };
    let value = match price {
        Choice::Market(rule) => RuleValue::Market(rule),
        Choice::Fixed => {
            let price = Money::parse_isk(fixed).map_err(|_error| BAD_PRICE.to_owned())?;
            if price < Money::ZERO {
                return Err(NEGATIVE.to_owned());
            }
            RuleValue::Fixed(price)
        }
    };
    Ok((target, value))
}

#[cfg(test)]
mod tests {
    use mop_market::book::PriceRule;
    use mop_store::settings::{RuleTarget, RuleValue};

    use super::{Choice, Scope, build_rule, target_text, value_text};

    fn isk(text: &str) -> mop_core::money::Money {
        mop_core::money::Money::parse_isk(text).expect("a price")
    }

    #[test]
    fn a_rule_value_reads_as_its_price_text() {
        assert_eq!(
            value_text(RuleValue::Market(PriceRule::BuyFivePercent)),
            "Forge buy, 5% price"
        );
        assert_eq!(
            value_text(RuleValue::Fixed(isk("29,000,000"))),
            "Fixed 29,000,000.00 ISK"
        );
    }

    #[test]
    fn a_rule_target_reads_with_its_game_data_name_or_its_id() {
        assert_eq!(
            target_text(RuleTarget::Item(11_551), Some("Capital Armor Plate")),
            "Capital Armor Plate"
        );
        assert_eq!(target_text(RuleTarget::Item(11_551), None), "Item 11551");
        assert_eq!(
            target_text(RuleTarget::Group(18), Some("Mineral")),
            "Mineral (group)"
        );
        assert_eq!(target_text(RuleTarget::Group(18), None), "Group 18");
        assert_eq!(
            target_text(RuleTarget::Category(4), Some("Material")),
            "Material (category)"
        );
        assert_eq!(target_text(RuleTarget::Category(4), None), "Category 4");
    }

    #[test]
    fn a_built_rule_keeps_its_target_and_price() {
        assert_eq!(
            build_rule(
                Some(RuleTarget::Item(11_551)),
                "Capital Armor Plate",
                Scope::Item,
                Choice::Fixed,
                "29,000,000",
            ),
            Ok((
                RuleTarget::Item(11_551),
                RuleValue::Fixed(isk("29,000,000"))
            ))
        );
        assert_eq!(
            build_rule(
                Some(RuleTarget::Group(18)),
                "Mineral",
                Scope::Group,
                Choice::Market(PriceRule::BuyFivePercent),
                "",
            ),
            Ok((
                RuleTarget::Group(18),
                RuleValue::Market(PriceRule::BuyFivePercent)
            ))
        );
        assert_eq!(
            build_rule(
                Some(RuleTarget::Category(4)),
                "Material",
                Scope::Category,
                Choice::Market(PriceRule::SellMedian),
                "",
            ),
            Ok((
                RuleTarget::Category(4),
                RuleValue::Market(PriceRule::SellMedian)
            ))
        );
    }

    #[test]
    fn a_missing_target_names_what_the_scope_looked_for() {
        assert_eq!(
            build_rule(
                None,
                "Capital Armr Plate",
                Scope::Item,
                Choice::Fixed,
                "abc"
            ),
            Err("No item called \"Capital Armr Plate\".".to_owned())
        );
        assert_eq!(
            build_rule(None, "Minrel", Scope::Group, Choice::Fixed, "abc"),
            Err("No group called \"Minrel\".".to_owned())
        );
        assert_eq!(
            build_rule(None, "Matrial", Scope::Category, Choice::Fixed, "abc"),
            Err("No category called \"Matrial\".".to_owned())
        );
    }

    #[test]
    fn a_fixed_price_must_be_a_number_and_not_negative() {
        let target = Some(RuleTarget::Item(11_551));
        assert_eq!(
            build_rule(
                target,
                "Capital Armor Plate",
                Scope::Item,
                Choice::Fixed,
                "abc"
            ),
            Err("Type a price in ISK, like 29,000,000.".to_owned())
        );
        assert_eq!(
            build_rule(
                target,
                "Capital Armor Plate",
                Scope::Item,
                Choice::Fixed,
                ""
            ),
            Err("Type a price in ISK, like 29,000,000.".to_owned())
        );
        assert_eq!(
            build_rule(
                target,
                "Capital Armor Plate",
                Scope::Item,
                Choice::Fixed,
                "-5"
            ),
            Err("A fixed price cannot be negative.".to_owned())
        );
    }

    #[test]
    fn the_panel_is_titled_value_prices() {
        assert_eq!(
            super::super::SetupPanel::ValuePrices.title(),
            "Value prices"
        );
    }
}
