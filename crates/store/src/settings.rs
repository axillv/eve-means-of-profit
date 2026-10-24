//! Settings the user chose: the default price rule, the market density and the last game-data check.
//! They are named values in the user database, with an open list of other named values beside them.

use mop_market::book::PriceRule;
use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{Result, StoreError};
use crate::migrate::Store;
use crate::plans;

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

const KEY_PRICE_RULE: &str = "default_price_rule";
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

    /// The saved settings, or the defaults when none were ever saved.
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
            None => Density::Normal,
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
}
