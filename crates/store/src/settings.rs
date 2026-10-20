//! Settings the user chose: the default price rule, the market density and the last game-data check.

use profit_market::book::PriceRule;
use rusqlite::{OptionalExtension, params};

use crate::error::Result;
use crate::migrate::Store;
use crate::{bad_code, plans};

/// How many market orders the user expects to see per item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Density {
    Sparse,
    Normal,
    Dense,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub default_price_rule: PriceRule,
    pub density: Density,
    /// Unix time of the last game-data check; `None` until one has run.
    pub last_sde_check: Option<i64>,
}

const TABLE: &str = "settings";

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

impl Store {
    /// The saved settings, or the defaults when none were ever saved.
    pub fn settings(&mut self) -> Result<Settings> {
        let tx = self.conn.transaction()?;
        let raw: Option<(i64, i64, Option<i64>)> = tx
            .query_row(
                "SELECT default_price_rule, density, last_sde_check FROM settings WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let settings = match raw {
            None => Settings {
                default_price_rule: PriceRule::SellFivePercent,
                density: Density::Normal,
                last_sde_check: None,
            },
            Some((rule, density, last_sde_check)) => Settings {
                default_price_rule: plans::price_rule_from_code(rule)
                    .ok_or_else(|| bad_code(TABLE, 1, "default_price_rule", rule))?,
                density: density_from_code(density)
                    .ok_or_else(|| bad_code(TABLE, 1, "density", density))?,
                last_sde_check,
            },
        };
        tx.commit()?;
        Ok(settings)
    }

    pub fn save_settings(&mut self, settings: &Settings) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO settings (id, default_price_rule, density, last_sde_check) \
             VALUES (1, ?1, ?2, ?3) \
             ON CONFLICT (id) DO UPDATE SET default_price_rule = ?1, density = ?2, \
                last_sde_check = ?3",
            params![
                plans::price_rule_code(settings.default_price_rule),
                density_code(settings.density),
                settings.last_sde_check
            ],
        )?;
        tx.commit()?;
        Ok(())
    }
}
