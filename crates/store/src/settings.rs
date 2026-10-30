//! Settings the user chose: the default price rule, the market density and the last game-data
//! check, plus the dated price rules by item, group or category. The settings are named values in
//! the user database, with an open list of other named values beside them.

use std::collections::BTreeMap;

use mop_core::industry::TypeId;
use mop_core::money::Money;
use mop_market::book::PriceRule;
use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{Result, StoreError};
use crate::migrate::Store;
use crate::plans;
use crate::{bad_code, from_stored};

/// How many market orders the user expects to see per item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Density {
    Sparse,
    Normal,
    Dense,
}

/// The player's interface size, as a percentage of the normal size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceSize {
    P100,
    P125,
    P150,
    P175,
    P200,
}

impl InterfaceSize {
    pub const ALL: [InterfaceSize; 5] = [
        InterfaceSize::P100,
        InterfaceSize::P125,
        InterfaceSize::P150,
        InterfaceSize::P175,
        InterfaceSize::P200,
    ];

    /// The size as a percentage: 100, 125, 150, 175 or 200.
    pub fn percent(self) -> u16 {
        match self {
            InterfaceSize::P100 => 100,
            InterfaceSize::P125 => 125,
            InterfaceSize::P150 => 150,
            InterfaceSize::P175 => 175,
            InterfaceSize::P200 => 200,
        }
    }

    /// The size for a percentage, or `None` when it is not one of the five sizes.
    pub fn from_percent(percent: i64) -> Option<InterfaceSize> {
        InterfaceSize::ALL
            .into_iter()
            .find(|size| i64::from(size.percent()) == percent)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub default_price_rule: PriceRule,
    pub density: Density,
    pub interface_size: InterfaceSize,
    pub rail_icons_only: bool,
    /// Unix time of the last game-data check; `None` until one has run.
    pub last_sde_check: Option<i64>,
}

/// What a price rule is about: one item, every item in a group, or every group in a category.
///
/// Ordered items first, then groups, then categories, each by id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RuleTarget {
    Item(TypeId),
    Group(u32),
    Category(u32),
}

impl RuleTarget {
    /// The target kind and id the database stores it under.
    fn stored(self) -> (i64, i64) {
        match self {
            RuleTarget::Item(id) => (0, i64::from(id)),
            RuleTarget::Group(id) => (1, i64::from(id)),
            RuleTarget::Category(id) => (2, i64::from(id)),
        }
    }
}

/// What a price rule says: a market statistic, or a fixed price in ISK.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleValue {
    Market(PriceRule),
    Fixed(Money),
}

/// What priced an item: its own rule, its group's, its category's, or the app-wide price.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PricedBy {
    AppWide,
    Item,
    Group(u32),
    Category(u32),
}

/// The rule that prices one item and what set it: the item's own rule, else its group's, else
/// its category's, else the app-wide `Value items at` price. The most specific rule wins; only
/// the target kind decides, never when its change was made.
pub fn resolve(
    rules: &BTreeMap<RuleTarget, RuleValue>,
    app_wide: PriceRule,
    type_id: TypeId,
    group_id: u32,
    category_id: u32,
) -> (RuleValue, PricedBy) {
    for (target, priced_by) in [
        (RuleTarget::Item(type_id), PricedBy::Item),
        (RuleTarget::Group(group_id), PricedBy::Group(group_id)),
        (
            RuleTarget::Category(category_id),
            PricedBy::Category(category_id),
        ),
    ] {
        if let Some(value) = rules.get(&target) {
            return (*value, priced_by);
        }
    }
    (RuleValue::Market(app_wide), PricedBy::AppWide)
}

/// One change of a price rule: the rule it is about, what it became (`None` means the rule was
/// removed) and the Unix time the change was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriceRuleChange {
    pub target: RuleTarget,
    pub value: Option<RuleValue>,
    pub at: i64,
}

const TABLE: &str = "price_rule_changes";

/// Every change, in time order and then by target.
const READ_ALL_SQL: &str = "SELECT target_kind, target_id, at, kind, market_rule, fixed \
     FROM price_rule_changes ORDER BY at, target_kind, target_id";

/// Every change up to a time, by target and then time, so that reading the rows in order leaves
/// each target at its latest change.
const READ_AT_SQL: &str = "SELECT target_kind, target_id, at, kind, market_rule, fixed \
     FROM price_rule_changes WHERE at <= ?1 ORDER BY target_kind, target_id, at";

/// The target a stored (kind, id) pair names; a kind that names no target is bad data.
fn target_from_stored(kind: i64, id: i64) -> Result<RuleTarget> {
    let id: u32 = from_stored(id, TABLE, kind, "target_id")?;
    match kind {
        0 => Ok(RuleTarget::Item(id)),
        1 => Ok(RuleTarget::Group(id)),
        2 => Ok(RuleTarget::Category(id)),
        _ => Err(bad_code(TABLE, kind, "target_kind", kind)),
    }
}

/// A change whose `kind` names a value the matching column does not hold.
fn empty_value_column(at: i64, column: &str) -> StoreError {
    StoreError::BadData(format!(
        "{TABLE} change at {at}: kind names a value but column {column} is empty"
    ))
}

/// The value a stored (kind, market_rule, fixed) triple names; a `None` kind is a removal.
fn value_from_stored(
    kind: Option<i64>,
    market_rule: Option<i64>,
    fixed: Option<i64>,
    at: i64,
) -> Result<Option<RuleValue>> {
    let Some(kind) = kind else {
        return Ok(None);
    };
    match kind {
        0 => {
            let code = market_rule.ok_or_else(|| empty_value_column(at, "market_rule"))?;
            let rule = plans::price_rule_from_code(code)
                .ok_or_else(|| bad_code(TABLE, at, "market_rule", code))?;
            Ok(Some(RuleValue::Market(rule)))
        }
        1 => {
            let fixed = fixed.ok_or_else(|| empty_value_column(at, "fixed"))?;
            Ok(Some(RuleValue::Fixed(Money::from_ten_thousandths(fixed))))
        }
        _ => Err(bad_code(TABLE, at, "kind", kind)),
    }
}

/// The changes one of the `READ_*_SQL` queries returns, in the order the query lists them.
fn read_changes(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<PriceRuleChange>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params, |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, Option<i64>>(3)?,
            row.get::<_, Option<i64>>(4)?,
            row.get::<_, Option<i64>>(5)?,
        ))
    })?;
    let mut changes = Vec::new();
    for (kind, id, at, value_kind, market_rule, fixed) in
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    {
        changes.push(PriceRuleChange {
            target: target_from_stored(kind, id)?,
            value: value_from_stored(value_kind, market_rule, fixed, at)?,
            at,
        });
    }
    Ok(changes)
}

const KEY_PRICE_RULE: &str = "default_price_rule";
const KEY_VALUE_PRICE: &str = "value_price";
const KEY_DENSITY: &str = "density";
const KEY_INTERFACE_SIZE: &str = "interface_size";
const KEY_RAIL_ICONS_ONLY: &str = "rail_icons_only";
const KEY_LAST_SDE_CHECK: &str = "last_sde_check";
const MAX_KEY_LEN: usize = 64;
const MAX_VALUE_LEN: usize = 4_096;

fn density_code(density: Density) -> i64 {
    match density {
        Density::Sparse => 0,
        Density::Normal => 1,
        Density::Dense => 2,
    }
}

fn density_from_code(code: i64) -> Option<Density> {
    match code {
        0 => Some(Density::Sparse),
        1 => Some(Density::Normal),
        2 => Some(Density::Dense),
        _ => None,
    }
}

/// A setting name: 1 to 64 characters from `a-z`, `0-9`, `_` and `.`.
fn check_key(key: &str) -> Result<()> {
    let valid = (1..=MAX_KEY_LEN).contains(&key.len())
        && key.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'.')
        });
    if valid {
        return Ok(());
    }
    Err(StoreError::BadData(format!(
        "{key:?} is not a setting name: 1 to 64 characters from a-z, 0-9, _ and ."
    )))
}

fn bad_value(key: &str, text: &str) -> StoreError {
    StoreError::BadData(format!(
        "setting {key} has the value {text:?}, which is not valid"
    ))
}

fn read_setting(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT value FROM app_settings WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()?)
}

/// Every saved setting whose key starts with `prefix`, in key order. The match is exact: case
/// counts, and `_` and `%` are plain characters.
pub(crate) fn settings_with_prefix(
    conn: &Connection,
    prefix: &str,
) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT key, value FROM app_settings \
         WHERE substr(key, 1, length(?1)) = ?1 ORDER BY key",
    )?;
    let rows = stmt.query_map(params![prefix], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn write_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO app_settings (key, value) VALUES (?1, ?2) \
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

fn clear_setting_in(conn: &Connection, key: &str) -> Result<()> {
    conn.execute("DELETE FROM app_settings WHERE key = ?1", params![key])?;
    Ok(())
}

/// A stored number, read back from its decimal text.
fn stored_number(key: &str, text: &str) -> Result<i64> {
    text.parse().ok().ok_or_else(|| bad_value(key, text))
}

impl Store {
    /// The saved value under `key`, or `None` when none was ever saved.
    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        check_key(key)?;
        read_setting(&self.conn, key)
    }

    /// Saves `value` under `key`, replacing an earlier value. An empty value is kept as empty.
    pub fn set_setting(&mut self, key: &str, value: &str) -> Result<()> {
        check_key(key)?;
        if value.len() > MAX_VALUE_LEN {
            return Err(StoreError::TooLarge);
        }
        write_setting(&self.conn, key, value)
    }

    /// Removes the value under `key`. Removing a key that was never saved is not an error.
    pub fn clear_setting(&mut self, key: &str) -> Result<()> {
        check_key(key)?;
        clear_setting_in(&self.conn, key)
    }

    /// The saved settings, or the defaults when none were ever saved. The default density is
    /// [`Density::Sparse`] (Roomy); a saved density, even [`Density::Normal`], wins over it.
    pub fn settings(&mut self) -> Result<Settings> {
        let tx = self.conn.transaction()?;
        let default_price_rule = match read_setting(&tx, KEY_PRICE_RULE)? {
            None => PriceRule::SellFivePercent,
            Some(text) => {
                let code = stored_number(KEY_PRICE_RULE, &text)?;
                plans::price_rule_from_code(code).ok_or_else(|| bad_value(KEY_PRICE_RULE, &text))?
            }
        };
        let density = match read_setting(&tx, KEY_DENSITY)? {
            None => Density::Sparse,
            Some(text) => {
                let code = stored_number(KEY_DENSITY, &text)?;
                density_from_code(code).ok_or_else(|| bad_value(KEY_DENSITY, &text))?
            }
        };
        let interface_size = match read_setting(&tx, KEY_INTERFACE_SIZE)? {
            None => InterfaceSize::P100,
            Some(text) => {
                let percent = stored_number(KEY_INTERFACE_SIZE, &text)?;
                InterfaceSize::from_percent(percent)
                    .ok_or_else(|| bad_value(KEY_INTERFACE_SIZE, &text))?
            }
        };
        let rail_icons_only = match read_setting(&tx, KEY_RAIL_ICONS_ONLY)? {
            None => false,
            Some(text) => match text.as_str() {
                "1" => true,
                "0" => false,
                _ => return Err(bad_value(KEY_RAIL_ICONS_ONLY, &text)),
            },
        };
        let last_sde_check = match read_setting(&tx, KEY_LAST_SDE_CHECK)? {
            None => None,
            Some(text) => Some(stored_number(KEY_LAST_SDE_CHECK, &text)?),
        };
        tx.commit()?;
        Ok(Settings {
            default_price_rule,
            density,
            interface_size,
            rail_icons_only,
            last_sde_check,
        })
    }

    /// Saves the settings in one transaction. A `last_sde_check` of `None` removes the saved time.
    pub fn save_settings(&mut self, settings: &Settings) -> Result<()> {
        let tx = self.conn.transaction()?;
        write_setting(
            &tx,
            KEY_PRICE_RULE,
            &plans::price_rule_code(settings.default_price_rule).to_string(),
        )?;
        write_setting(
            &tx,
            KEY_DENSITY,
            &density_code(settings.density).to_string(),
        )?;
        write_setting(
            &tx,
            KEY_INTERFACE_SIZE,
            &settings.interface_size.percent().to_string(),
        )?;
        let rail = if settings.rail_icons_only { "1" } else { "0" };
        write_setting(&tx, KEY_RAIL_ICONS_ONLY, rail)?;
        match settings.last_sde_check {
            Some(at) => write_setting(&tx, KEY_LAST_SDE_CHECK, &at.to_string())?,
            None => clear_setting_in(&tx, KEY_LAST_SDE_CHECK)?,
        }
        tx.commit()?;
        Ok(())
    }

    /// The app-wide `Value items at` price: what a point with no rule over it is worth per
    /// unit. Unset: [`PriceRule::SellFivePercent`]. A saved value that is not a known
    /// [`plans::price_rule_code`] is the same bad-value error as a bad `default_price_rule`.
    pub fn value_price(&self) -> Result<PriceRule> {
        match read_setting(&self.conn, KEY_VALUE_PRICE)? {
            None => Ok(PriceRule::SellFivePercent),
            Some(text) => {
                let code = stored_number(KEY_VALUE_PRICE, &text)?;
                plans::price_rule_from_code(code).ok_or_else(|| bad_value(KEY_VALUE_PRICE, &text))
            }
        }
    }

    /// Saves the app-wide `Value items at` price, replacing an earlier one. Not dated:
    /// switching it re-prices the whole net worth chart.
    pub fn set_value_price(&mut self, rule: PriceRule) -> Result<()> {
        write_setting(
            &self.conn,
            KEY_VALUE_PRICE,
            &plans::price_rule_code(rule).to_string(),
        )
    }

    /// Records what the price rule for `target` became at the Unix time `at`. A change for the
    /// same target at the same `at` replaces the earlier one; other changes are never deleted, so
    /// a net worth point can use the rules that stood on its day. A `None` value removes the rule
    /// from that time on. A negative fixed price fails the table's check and nothing is written.
    pub fn set_price_rule(
        &mut self,
        target: RuleTarget,
        value: Option<RuleValue>,
        at: i64,
    ) -> Result<()> {
        let (kind, market_rule, fixed) = match value {
            None => (None, None, None),
            Some(RuleValue::Market(rule)) => (Some(0), Some(plans::price_rule_code(rule)), None),
            Some(RuleValue::Fixed(price)) => (Some(1), None, Some(price.ten_thousandths())),
        };
        let (target_kind, target_id) = target.stored();
        self.conn.execute(
            "INSERT INTO price_rule_changes \
                (target_kind, target_id, at, kind, market_rule, fixed) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT (target_kind, target_id, at) DO UPDATE SET \
                kind = excluded.kind, \
                market_rule = excluded.market_rule, \
                fixed = excluded.fixed",
            params![target_kind, target_id, at, kind, market_rule, fixed],
        )?;
        Ok(())
    }

    /// The rules in force at the Unix time `at`: for each target, its latest change at or before
    /// `at`, leaving out targets whose latest such change was a removal.
    pub fn price_rules_at(&self, at: i64) -> Result<BTreeMap<RuleTarget, RuleValue>> {
        let mut rules = BTreeMap::new();
        for change in read_changes(&self.conn, READ_AT_SQL, params![at])? {
            match change.value {
                Some(value) => {
                    rules.insert(change.target, value);
                }
                None => {
                    rules.remove(&change.target);
                }
            }
        }
        Ok(rules)
    }

    /// Every change, in time order and then by target.
    pub fn price_rule_changes(&self) -> Result<Vec<PriceRuleChange>> {
        read_changes(&self.conn, READ_ALL_SQL, [])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_match_is_exact_and_in_key_order() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE app_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO app_settings (key, value) VALUES
                 ('invention.skill.2', '3'),
                 ('invention.skill.10', '4'),
                 ('invention.skillX3', '5'),
                 ('INVENTION.SKILL.4', '1'),
                 ('invention.attempts', '7');",
        )
        .unwrap();
        let found = settings_with_prefix(&conn, "invention.skill.").unwrap();
        assert_eq!(
            found,
            vec![
                ("invention.skill.10".to_string(), "4".to_string()),
                ("invention.skill.2".to_string(), "3".to_string()),
            ]
        );
    }

    fn open_temp() -> (tempfile::TempDir, std::path::PathBuf, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        let store = Store::open(&path, 1_000).expect("open").store;
        (dir, path, store)
    }

    /// A whole ISK amount, as the database counts it.
    fn isk(amount: i64) -> Money {
        Money::from_ten_thousandths(amount * 10_000)
    }

    #[test]
    fn targets_order_items_then_groups_then_categories_by_id() {
        let mut targets = [
            RuleTarget::Category(1),
            RuleTarget::Group(10),
            RuleTarget::Item(9),
            RuleTarget::Category(4),
            RuleTarget::Group(2),
            RuleTarget::Item(1),
        ];
        targets.sort();
        assert_eq!(
            targets,
            [
                RuleTarget::Item(1),
                RuleTarget::Item(9),
                RuleTarget::Group(2),
                RuleTarget::Group(10),
                RuleTarget::Category(1),
                RuleTarget::Category(4),
            ]
        );
    }

    #[test]
    fn fixed_item_rule_follows_its_change_times() {
        let (_dir, _path, mut store) = open_temp();
        let plate = RuleTarget::Item(11_551);
        store
            .set_price_rule(
                plate,
                Some(RuleValue::Fixed(isk(29_000_000))),
                1_000_000_000,
            )
            .expect("first");
        store
            .set_price_rule(
                plate,
                Some(RuleValue::Fixed(isk(31_000_000))),
                1_001_728_000,
            )
            .expect("second");
        assert!(
            store
                .price_rules_at(999_999_999)
                .expect("before")
                .is_empty()
        );
        assert_eq!(
            store.price_rules_at(1_000_864_000).expect("between"),
            BTreeMap::from([(plate, RuleValue::Fixed(isk(29_000_000)))])
        );
        assert_eq!(
            store.price_rules_at(1_001_728_000).expect("after"),
            BTreeMap::from([(plate, RuleValue::Fixed(isk(31_000_000)))])
        );
        assert_eq!(store.price_rule_changes().expect("changes").len(), 2);
    }

    #[test]
    fn category_rule_is_removed_later_and_every_change_survives_reopen() {
        let (_dir, path, mut store) = open_temp();
        let plate = RuleTarget::Item(11_551);
        let material = RuleTarget::Category(4);
        store
            .set_price_rule(
                plate,
                Some(RuleValue::Fixed(isk(29_000_000))),
                1_000_000_000,
            )
            .expect("item");
        store
            .set_price_rule(
                material,
                Some(RuleValue::Market(PriceRule::BuyFivePercent)),
                1_000_000_000,
            )
            .expect("category");
        store
            .set_price_rule(material, None, 1_000_086_400)
            .expect("removal");
        store
            .set_price_rule(
                plate,
                Some(RuleValue::Fixed(isk(31_000_000))),
                1_001_728_000,
            )
            .expect("item update");
        assert_eq!(
            store.price_rules_at(1_000_000_000).expect("at"),
            BTreeMap::from([
                (plate, RuleValue::Fixed(isk(29_000_000))),
                (material, RuleValue::Market(PriceRule::BuyFivePercent)),
            ])
        );
        assert_eq!(
            store.price_rules_at(1_000_086_400).expect("after removal"),
            BTreeMap::from([(plate, RuleValue::Fixed(isk(29_000_000)))])
        );
        let expected = vec![
            PriceRuleChange {
                target: plate,
                value: Some(RuleValue::Fixed(isk(29_000_000))),
                at: 1_000_000_000,
            },
            PriceRuleChange {
                target: material,
                value: Some(RuleValue::Market(PriceRule::BuyFivePercent)),
                at: 1_000_000_000,
            },
            PriceRuleChange {
                target: material,
                value: None,
                at: 1_000_086_400,
            },
            PriceRuleChange {
                target: plate,
                value: Some(RuleValue::Fixed(isk(31_000_000))),
                at: 1_001_728_000,
            },
        ];
        assert_eq!(store.price_rule_changes().expect("changes"), expected);
        drop(store);
        let reopened = Store::open(&path, 2_000).expect("reopen").store;
        assert_eq!(reopened.price_rule_changes().expect("changes"), expected);
    }

    #[test]
    fn a_change_at_the_same_time_is_replaced_and_walk_sell_is_kept() {
        let (_dir, _path, mut store) = open_temp();
        let minerals = RuleTarget::Group(1_002);
        store
            .set_price_rule(minerals, Some(RuleValue::Fixed(isk(1))), 1_000_000_000)
            .expect("fixed");
        store
            .set_price_rule(
                minerals,
                Some(RuleValue::Market(PriceRule::SellMedian)),
                1_000_000_000,
            )
            .expect("market");
        let changes = store.price_rule_changes().expect("changes");
        assert_eq!(changes.len(), 1);
        assert_eq!(
            changes[0].value,
            Some(RuleValue::Market(PriceRule::SellMedian))
        );
        assert_eq!(
            store.price_rules_at(1_000_000_000).expect("at"),
            BTreeMap::from([(minerals, RuleValue::Market(PriceRule::SellMedian))])
        );
        store
            .set_price_rule(
                minerals,
                Some(RuleValue::Market(PriceRule::WalkSell)),
                1_000_086_400,
            )
            .expect("walk sell");
        assert_eq!(
            store.price_rules_at(1_000_086_400).expect("walk sell"),
            BTreeMap::from([(minerals, RuleValue::Market(PriceRule::WalkSell))])
        );
    }

    #[test]
    fn a_negative_fixed_price_is_refused_and_keeps_no_change() {
        let (_dir, _path, mut store) = open_temp();
        let plate = RuleTarget::Item(11_551);
        store
            .set_price_rule(
                plate,
                Some(RuleValue::Fixed(isk(29_000_000))),
                1_000_000_000,
            )
            .expect("kept");
        let before = store.price_rule_changes().expect("changes");
        assert!(matches!(
            store.set_price_rule(
                plate,
                Some(RuleValue::Fixed(Money::from_ten_thousandths(-1))),
                1_000_086_400,
            ),
            Err(StoreError::Sqlite(_))
        ));
        assert_eq!(store.price_rule_changes().expect("changes"), before);
        store
            .set_price_rule(
                plate,
                Some(RuleValue::Market(PriceRule::HighestBuy)),
                1_000_086_400,
            )
            .expect("good change at the same time");
        assert_eq!(store.price_rule_changes().expect("changes").len(), 2);
    }

    /// The acceptance rules: an item, a group, a category and the app-wide fallback.
    fn acceptance_rules() -> BTreeMap<RuleTarget, RuleValue> {
        BTreeMap::from([
            (RuleTarget::Item(11_551), RuleValue::Fixed(isk(29_000_000))),
            (
                RuleTarget::Group(18),
                RuleValue::Market(PriceRule::BuyFivePercent),
            ),
            (
                RuleTarget::Category(4),
                RuleValue::Market(PriceRule::SellMedian),
            ),
        ])
    }

    #[test]
    fn resolve_picks_the_most_specific_rule_or_the_app_wide_price() {
        let rules = acceptance_rules();
        // Capital Armor Plate: its own rule wins over its group's and its category's.
        assert_eq!(
            resolve(&rules, PriceRule::SellFivePercent, 11_551, 873, 17),
            (RuleValue::Fixed(isk(29_000_000)), PricedBy::Item)
        );
        // Tritanium: no item rule, so the group's wins over the category's.
        assert_eq!(
            resolve(&rules, PriceRule::SellFivePercent, 34, 18, 4),
            (
                RuleValue::Market(PriceRule::BuyFivePercent),
                PricedBy::Group(18)
            )
        );
        // A group-427 item in category 4: no item or group rule, so the category's wins.
        assert_eq!(
            resolve(&rules, PriceRule::SellFivePercent, 25_232, 427, 4),
            (
                RuleValue::Market(PriceRule::SellMedian),
                PricedBy::Category(4)
            )
        );
        // PAU: no rule over it, so the app-wide price prices it.
        assert_eq!(
            resolve(&rules, PriceRule::HighestBuy, 12_612, 100, 18),
            (RuleValue::Market(PriceRule::HighestBuy), PricedBy::AppWide)
        );
    }

    #[test]
    fn an_item_rule_beats_its_group_even_when_the_group_rule_is_later() {
        let (_dir, _path, mut store) = open_temp();
        store
            .set_price_rule(
                RuleTarget::Item(11_551),
                Some(RuleValue::Fixed(isk(29_000_000))),
                1_000_000_000,
            )
            .expect("item");
        store
            .set_price_rule(
                RuleTarget::Group(873),
                Some(RuleValue::Market(PriceRule::WalkSell)),
                1_001_728_000,
            )
            .expect("group later");
        let rules = store.price_rules_at(1_001_728_000).expect("at");
        assert_eq!(
            resolve(&rules, PriceRule::SellFivePercent, 11_551, 873, 17),
            (RuleValue::Fixed(isk(29_000_000)), PricedBy::Item)
        );
    }

    #[test]
    fn value_price_is_sell_five_percent_until_saved_and_survives_reopen() {
        let (_dir, path, mut store) = open_temp();
        assert_eq!(
            store.value_price().expect("default"),
            PriceRule::SellFivePercent
        );
        store.set_value_price(PriceRule::BuyMedian).expect("save");
        assert_eq!(store.value_price().expect("saved"), PriceRule::BuyMedian);
        drop(store);
        let reopened = Store::open(&path, 2_000).expect("reopen").store;
        assert_eq!(
            reopened.value_price().expect("reopened"),
            PriceRule::BuyMedian
        );
    }

    #[test]
    fn a_value_price_that_is_not_a_known_code_is_bad_data() {
        let (_dir, _path, mut store) = open_temp();
        store.set_setting("value_price", "x").expect("save");
        assert!(matches!(store.value_price(), Err(StoreError::BadData(_))));
        // A number that names no rule is the same bad-code error.
        store.set_setting("value_price", "9").expect("save");
        assert!(matches!(store.value_price(), Err(StoreError::BadData(_))));
    }
}
