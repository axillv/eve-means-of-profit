//! The Net worth page's By character and Right now tables: what each character was worth and
//! what is in the latest point. The builders here are pure; [`show_by_character`] and
//! [`show_right_now`] draw them.

use std::collections::BTreeMap;
use std::fmt;

use egui::{Id, Ui};
use mop_core::money::Money;
use mop_core::net_worth::{Part, STALE_AFTER};
use mop_esi::characters::Character;

use crate::Shared;
use crate::build::inputs::price_text;
use crate::build::prices::age_text;
use crate::net_worth::Choices;
use crate::net_worth::chart::{Layer, character_name};
use crate::net_worth::data::{MemberValue, ValuedPoint};
use crate::net_worth::view::{View, percent_text};
use crate::status::Notice;
use crate::table::Table;
use crate::table::model::{Cell, Column, ColumnKind, Row};

/// The parts that get their own column in the By character table, in part order.
const OWN_PARTS: [Part; 5] = [
    Part::Wallet,
    Part::Assets,
    Part::SellOrders,
    Part::BuyEscrow,
    Part::IndustryJobs,
];

/// The parts added together into the `Other` column, in part order.
const OTHER_PARTS: [Part; 3] = [Part::ContractCollateral, Part::Contracts, Part::Implants];

/// A pure builder's failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableError {
    Overflow,
}

impl fmt::Display for TableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Overflow => f.write_str("net worth is too large to add up"),
        }
    }
}

impl std::error::Error for TableError {}

impl From<std::num::TryFromIntError> for TableError {
    fn from(_error: std::num::TryFromIntError) -> Self {
        Self::Overflow
    }
}

/// A drawn table: its columns, rows, footer and the rows whose age is stale.
#[derive(Debug, Clone, PartialEq)]
pub struct Built {
    pub columns: Vec<Column>,
    pub rows: Vec<Row>,
    pub footer: Vec<Cell>,
    /// The age in seconds of each row that is older than a day, by row key.
    pub stale: BTreeMap<u64, i64>,
}

/// One money column: its head and the member parts it adds up.
struct MoneyColumn {
    id: &'static str,
    label: String,
    parts: Vec<Part>,
}

/// The By character table of the latest point.
pub fn by_character(
    last: &ValuedPoint,
    choices: &Choices,
    now: i64,
    login: &[Character],
) -> Result<Built, TableError> {
    let money = money_columns(choices);
    let mut columns = vec![Column {
        id: "character",
        label: "Character".to_owned(),
        kind: ColumnKind::Text,
        width: 130.0,
    }];
    columns.extend(money.iter().map(|column| Column {
        id: column.id,
        label: column.label.clone(),
        kind: ColumnKind::Money,
        width: column_width(column.id),
    }));
    columns.push(Column {
        id: "age",
        label: "Data age".to_owned(),
        kind: ColumnKind::Number,
        width: 80.0,
    });

    let mut rows = Vec::with_capacity(last.members.len());
    let mut stale = BTreeMap::new();
    let mut sums = vec![Money::ZERO; money.len()];
    for (index, (key, member)) in last.members.iter().enumerate() {
        let mut cells = Vec::with_capacity(columns.len());
        cells.push(Cell::text(character_name(key, login)));
        for (section, sum) in money.iter().zip(sums.iter_mut()) {
            let amount = member_amount(member, &section.parts)?;
            *sum = sum.checked_add(amount).ok_or(TableError::Overflow)?;
            cells.push(Cell::money(amount));
        }
        let age = now.saturating_sub(member.record_at);
        if age > STALE_AFTER {
            let row = u64::try_from(index).map_err(TableError::from)?;
            stale.insert(row, age);
        }
        cells.push(Cell::int(i128::from(age), age_text(age)));
        rows.push(Row {
            key: u64::try_from(index).map_err(TableError::from)?,
            depth: 0,
            cells,
        });
    }

    let mut footer = Vec::with_capacity(money.len() + 1);
    footer.push(Cell::text("Total"));
    footer.extend(sums.into_iter().map(Cell::money));
    Ok(Built {
        columns,
        rows,
        footer,
        stale,
    })
}

/// The Right now table of the latest point.
pub fn right_now(last: &ValuedPoint, choices: &Choices, now: i64) -> Result<Built, TableError> {
    let columns = vec![
        Column {
            id: "part",
            label: "Part".to_owned(),
            kind: ColumnKind::Text,
            width: 130.0,
        },
        Column {
            id: "value",
            label: "Value".to_owned(),
            kind: ColumnKind::Money,
            width: 120.0,
        },
        Column {
            id: "share",
            label: "Share".to_owned(),
            kind: ColumnKind::Number,
            width: 80.0,
        },
        Column {
            id: "basis",
            label: "Basis".to_owned(),
            kind: ColumnKind::Text,
            width: 160.0,
        },
        Column {
            id: "age",
            label: "Age".to_owned(),
            kind: ColumnKind::Number,
            width: 60.0,
        },
    ];
    let oldest = oldest_age(last, now);
    let mut rows = Vec::new();
    for part in Part::ALL {
        if !choices.shown.contains(&part) {
            continue;
        }
        let value = last.value.by_part.get(&part).copied().unwrap_or_default();
        if value == Money::ZERO {
            continue;
        }
        let share = share(value, last.total)?;
        let cells = vec![
            Cell::text(Layer::Part(part).label()),
            Cell::money(value),
            Cell::int(i128::from(share), percent_text(share, false)),
            Cell::text(basis(part, last, choices)),
            age_cell(oldest),
        ];
        rows.push(Row {
            key: u64::try_from(rows.len()).map_err(TableError::from)?,
            depth: 0,
            cells,
        });
    }
    if choices.show_plex && last.plex_value != Money::ZERO {
        let share = share(last.plex_value, last.total)?;
        let cells = vec![
            Cell::text(Layer::Plex.label()),
            Cell::money(last.plex_value),
            Cell::int(i128::from(share), percent_text(share, false)),
            Cell::text("PLEX market"),
            Cell::missing(),
        ];
        rows.push(Row {
            key: u64::try_from(rows.len()).map_err(TableError::from)?,
            depth: 0,
            cells,
        });
    }
    let footer = vec![
        Cell::text("Total"),
        Cell::money(last.total),
        Cell::int(10_000, percent_text(10_000, false)),
    ];
    Ok(Built {
        columns,
        rows,
        footer,
        stale: BTreeMap::new(),
    })
}

/// The money columns of the By character table, in part order: the shown parts with their own
/// column, then `Other` when one of its parts is shown, then the total of every shown part.
fn money_columns(choices: &Choices) -> Vec<MoneyColumn> {
    let mut columns: Vec<MoneyColumn> = OWN_PARTS
        .into_iter()
        .filter(|part| choices.shown.contains(part))
        .map(|part| MoneyColumn {
            id: part_id(part),
            label: Layer::Part(part).label().to_owned(),
            parts: vec![part],
        })
        .collect();
    let other: Vec<Part> = OTHER_PARTS
        .into_iter()
        .filter(|part| choices.shown.contains(part))
        .collect();
    if !other.is_empty() {
        columns.push(MoneyColumn {
            id: "other",
            label: "Other".to_owned(),
            parts: other,
        });
    }
    columns.push(MoneyColumn {
        id: "total",
        label: "Total".to_owned(),
        parts: Part::ALL
            .into_iter()
            .filter(|part| choices.shown.contains(part))
            .collect(),
    });
    columns
}

/// The table id of a part's own column.
fn part_id(part: Part) -> &'static str {
    match part {
        Part::Wallet => "wallet",
        Part::Assets => "assets",
        Part::SellOrders => "sell-orders",
        Part::BuyEscrow => "buy-escrow",
        Part::IndustryJobs => "industry-jobs",
        Part::ContractCollateral => "contract-collateral",
        Part::Contracts => "contracts",
        Part::Implants => "implants",
    }
}

/// The width of a money column.
fn column_width(id: &str) -> f32 {
    match id {
        "wallet" | "assets" | "sell-orders" | "buy-escrow" | "industry-jobs" => 120.0,
        "other" => 90.0,
        "total" => 130.0,
        _ => 110.0,
    }
}

/// One member's amount in `parts`, added with a check on every step.
fn member_amount(member: &MemberValue, parts: &[Part]) -> Result<Money, TableError> {
    let mut sum = Money::ZERO;
    for part in parts {
        let amount = member.parts.get(part).copied().unwrap_or_default();
        sum = sum.checked_add(amount).ok_or(TableError::Overflow)?;
    }
    Ok(sum)
}

/// The oldest member's age at `now`, in seconds; `None` with no members.
fn oldest_age(last: &ValuedPoint, now: i64) -> Option<i64> {
    last.members
        .values()
        .map(|member| now.saturating_sub(member.record_at))
        .max()
}

/// An age cell, missing when there is no age.
fn age_cell(age: Option<i64>) -> Cell {
    match age {
        Some(age) => Cell::int(i128::from(age), age_text(age)),
        None => Cell::missing(),
    }
}

/// `value` as a share of `total` in hundredths of a percent, halves away from zero. Zero total
/// gives zero.
fn share(value: Money, total: Money) -> Result<i64, TableError> {
    let total = i128::from(total.ten_thousandths());
    if total == 0 {
        return Ok(0);
    }
    let scaled = i128::from(value.ten_thousandths())
        .checked_mul(10_000)
        .ok_or(TableError::Overflow)?;
    let quotient = scaled / total;
    let remainder = scaled % total;
    let rounded = if remainder.unsigned_abs().saturating_mul(2) >= total.unsigned_abs() {
        quotient + scaled.signum() * total.signum()
    } else {
        quotient
    };
    i64::try_from(rounded).map_err(TableError::from)
}

/// What priced a part: jEve's own values, the part's own price, or the picked rule.
fn basis(part: Part, last: &ValuedPoint, choices: &Choices) -> String {
    if imported(last, part) {
        return "jEve".to_owned();
    }
    match part {
        Part::SellOrders => "own price".to_owned(),
        Part::BuyEscrow => "escrow".to_owned(),
        Part::Wallet => "wallet".to_owned(),
        Part::Assets
        | Part::IndustryJobs
        | Part::ContractCollateral
        | Part::Contracts
        | Part::Implants => price_text(choices.price).to_owned(),
    }
}

/// Whether every member holding a non-zero amount of `part` is imported.
fn imported(last: &ValuedPoint, part: Part) -> bool {
    last.members
        .values()
        .filter(|member| {
            member
                .parts
                .get(&part)
                .is_some_and(|amount| *amount != Money::ZERO)
        })
        .all(|member| member.imported)
}

/// The latest valued point, if the page has read any.
fn latest(view: &View) -> Option<&ValuedPoint> {
    view.shown()?.loaded.points.last()
}

/// Draws the By character panel's table.
pub fn show_by_character(ui: &mut Ui, cx: &mut Shared, view: &View, choices: &Choices) {
    let Some(last) = latest(view) else {
        return;
    };
    let now = (cx.clock)();
    let built = match by_character(last, choices, now, cx.session.characters()) {
        Ok(built) => built,
        Err(error) => {
            notice(cx, error);
            return;
        }
    };
    let style = cx.style;
    let mut draw = |ui: &mut Ui, key: u64, column: &'static str| -> bool {
        if column != "age" {
            return false;
        }
        let Some(&age) = built.stale.get(&key) else {
            return false;
        };
        ui.colored_label(style.stale, age_text(age));
        true
    };
    Table::new(Id::new("By character"), &built.columns).show_with(
        ui,
        &style,
        cx.density,
        &built.rows,
        Some(&built.footer),
        &mut draw,
    );
}

/// Draws the Right now panel's table.
pub fn show_right_now(ui: &mut Ui, cx: &mut Shared, view: &View, choices: &Choices) {
    let Some(last) = latest(view) else {
        return;
    };
    let now = (cx.clock)();
    let built = match right_now(last, choices, now) {
        Ok(built) => built,
        Err(error) => {
            notice(cx, error);
            return;
        }
    };
    Table::new(Id::new("Right now"), &built.columns).show(
        ui,
        &cx.style,
        cx.density,
        &built.rows,
        Some(&built.footer),
    );
}

/// Shows a builder's failure as an error notice.
fn notice(cx: &mut Shared, error: TableError) {
    cx.notices.push(Notice::error(format!(
        "Could not add up net worth: {error}"
    )));
}

#[cfg(test)]
mod tests {
    use super::*;
    use mop_core::net_worth::{CharacterKey, PointValue};

    const NOW: i64 = 1_552_492_184;
    const AT: i64 = 1_552_492_124;

    fn isk(text: &str) -> Money {
        Money::parse_isk(text).expect("a test price")
    }

    fn member(parts: &[(Part, &str)], imported: bool) -> MemberValue {
        MemberValue {
            parts: Part::ALL
                .into_iter()
                .map(|part| {
                    let amount = parts
                        .iter()
                        .find(|(shown, _)| *shown == part)
                        .map_or(Money::ZERO, |(_, text)| isk(text));
                    (part, amount)
                })
                .collect(),
            record_at: AT,
            imported,
        }
    }

    fn point(
        members: BTreeMap<CharacterKey, MemberValue>,
        plex_value: &str,
        total: &str,
    ) -> ValuedPoint {
        let mut by_part = Part::ALL
            .into_iter()
            .map(|part| (part, Money::ZERO))
            .collect::<BTreeMap<_, _>>();
        for member in members.values() {
            for (part, amount) in &member.parts {
                let sum = by_part.entry(*part).or_insert(Money::ZERO);
                // A helper sum may overflow only in the overflow test, which reads the members.
                if let Some(total) = sum.checked_add(*amount) {
                    *sum = total;
                }
            }
        }
        ValuedPoint {
            value: PointValue {
                at: AT,
                by_part,
                by_character: BTreeMap::new(),
                total: Money::ZERO,
                unpriced: BTreeMap::new(),
                stale: Vec::new(),
            },
            plex_units: 0,
            plex_value: isk(plex_value),
            total: isk(total),
            unpriced: BTreeMap::new(),
            source: crate::net_worth::data::pricer::PriceSource::Copy { published_at: None },
            members,
        }
    }

    fn the_member() -> ValuedPoint {
        let key = CharacterKey::Name("TEST-NAME".to_owned());
        point(
            BTreeMap::from([(
                key,
                member(
                    &[
                        (Part::Wallet, "10.00"),
                        (Part::Assets, "9.00"),
                        (Part::SellOrders, "8.00"),
                        (Part::BuyEscrow, "5.00"),
                        (Part::IndustryJobs, "7.00"),
                        (Part::ContractCollateral, "3.00"),
                        (Part::Contracts, "4.00"),
                        (Part::Implants, "0.00"),
                    ],
                    true,
                ),
            )]),
            "0.00",
            "46.00",
        )
    }

    fn ids(columns: &[Column]) -> Vec<&'static str> {
        columns.iter().map(|column| column.id).collect()
    }

    #[test]
    fn the_by_character_table_adds_the_parts_and_folds_the_rest_into_other() {
        let last = the_member();
        let choices = Choices::default();
        let table = by_character(&last, &choices, NOW, &[]).expect("the table builds");
        assert_eq!(
            ids(&table.columns),
            vec![
                "character",
                "wallet",
                "assets",
                "sell-orders",
                "buy-escrow",
                "industry-jobs",
                "other",
                "total",
                "age",
            ]
        );
        assert_eq!(table.rows.len(), 1);
        let row = &table.rows[0];
        let texts: Vec<&str> = row.cells.iter().map(|cell| cell.text.as_str()).collect();
        assert_eq!(
            texts,
            vec![
                "TEST-NAME",
                "10.00",
                "9.00",
                "8.00",
                "5.00",
                "7.00",
                "7.00",
                "46.00",
                "1m",
            ]
        );
        assert!(table.stale.is_empty());
        let footer: Vec<&str> = table.footer.iter().map(|cell| cell.text.as_str()).collect();
        assert_eq!(
            footer,
            vec![
                "Total", "10.00", "9.00", "8.00", "5.00", "7.00", "7.00", "46.00",
            ]
        );
    }

    #[test]
    fn a_column_appears_only_for_the_parts_that_are_shown() {
        let last = the_member();
        let choices = Choices {
            shown: [Part::Wallet, Part::Implants].into_iter().collect(),
            ..Choices::default()
        };
        let table = by_character(&last, &choices, NOW, &[]).expect("the table builds");
        assert_eq!(
            ids(&table.columns),
            vec!["character", "wallet", "other", "total", "age"]
        );
        let texts: Vec<&str> = table.rows[0]
            .cells
            .iter()
            .map(|cell| cell.text.as_str())
            .collect();
        assert_eq!(texts, vec!["TEST-NAME", "10.00", "0.00", "10.00", "1m"]);
    }

    #[test]
    fn the_right_now_table_lists_the_shown_parts_with_their_shares_and_basis() {
        let last = the_member();
        let choices = Choices::default();
        let table = right_now(&last, &choices, NOW).expect("the table builds");
        let rows: Vec<(&str, &str, &str, &str, &str)> = table
            .rows
            .iter()
            .map(|row| {
                (
                    row.cells[0].text.as_str(),
                    row.cells[1].text.as_str(),
                    row.cells[2].text.as_str(),
                    row.cells[3].text.as_str(),
                    row.cells[4].text.as_str(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                ("Wallet", "10.00", "21.74%", "jEve", "1m"),
                ("Assets", "9.00", "19.57%", "jEve", "1m"),
                ("Sell orders", "8.00", "17.39%", "jEve", "1m"),
                ("Buy escrow", "5.00", "10.87%", "jEve", "1m"),
                ("Industry jobs", "7.00", "15.22%", "jEve", "1m"),
                ("Contract collateral", "3.00", "6.52%", "jEve", "1m"),
                ("Contracts", "4.00", "8.70%", "jEve", "1m"),
            ]
        );
        let footer: Vec<&str> = table.footer.iter().map(|cell| cell.text.as_str()).collect();
        assert_eq!(footer, vec!["Total", "46.00", "100.00%"]);
    }

    #[test]
    fn a_non_imported_part_names_its_own_price_and_the_vault_has_none() {
        let key = CharacterKey::Name("SEEN".to_owned());
        let seen = BTreeMap::from([(
            key,
            member(
                &[
                    (Part::Wallet, "1.00"),
                    (Part::SellOrders, "2.00"),
                    (Part::BuyEscrow, "3.00"),
                    (Part::Assets, "4.00"),
                ],
                false,
            ),
        )]);
        let with_plex = point(seen, "2,600,000,000.00", "2,600,000,010.00");
        let table = right_now(&with_plex, &Choices::default(), NOW).expect("the table builds");
        let basis_of = |part: &str| {
            table
                .rows
                .iter()
                .find(|row| row.cells[0].text == part)
                .map(|row| row.cells[3].text.clone())
        };
        assert_eq!(basis_of("Wallet"), Some("wallet".to_owned()));
        assert_eq!(basis_of("Assets"), Some("Forge sell, 5% price".to_owned()));
        assert_eq!(basis_of("Sell orders"), Some("own price".to_owned()));
        assert_eq!(basis_of("Buy escrow"), Some("escrow".to_owned()));
        assert_eq!(basis_of("PLEX vault"), Some("PLEX market".to_owned()));
        let vault = table
            .rows
            .iter()
            .find(|row| row.cells[0].text == "PLEX vault")
            .expect("the vault row");
        assert_eq!(vault.cells[1].text, "2,600,000,000.00");
        assert_eq!(vault.cells[4].text, "—");
    }

    #[test]
    fn two_members_whose_wallet_overflows_the_money_range_are_an_error() {
        let huge = MemberValue {
            parts: Part::ALL
                .into_iter()
                .map(|part| {
                    let amount = if part == Part::Wallet {
                        Money::from_ten_thousandths(i64::MAX)
                    } else {
                        Money::ZERO
                    };
                    (part, amount)
                })
                .collect(),
            record_at: AT,
            imported: false,
        };
        let members = BTreeMap::from([
            (CharacterKey::Name("ONE".to_owned()), huge.clone()),
            (CharacterKey::Name("TWO".to_owned()), huge),
        ]);
        let last = point(members, "0.00", "0.00");
        assert_eq!(
            by_character(&last, &Choices::default(), NOW, &[]),
            Err(TableError::Overflow)
        );
    }

    #[test]
    fn a_member_older_than_a_day_is_stale_and_the_others_are_not() {
        let key = CharacterKey::Name("OLD".to_owned());
        let mut old = member(&[(Part::Wallet, "1.00")], false);
        old.record_at = NOW - 90_000;
        let last = point(BTreeMap::from([(key, old)]), "0.00", "1.00");
        let table = by_character(&last, &Choices::default(), NOW, &[]).expect("the table builds");
        assert_eq!(table.stale.get(&0), Some(&90_000));
    }
}
