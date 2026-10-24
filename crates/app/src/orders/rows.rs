//! The data of the Orders table: every market order of the login characters, with how it stands
//! against The Forge's best price. Nothing here draws or asks the network.

use mop_core::format::group_thousands;
use mop_core::industry::TypeId;
use mop_core::money::Money;
use mop_market::fuzzwork::{Side, Stat};

use crate::Shared;
use crate::build::prices::age_text;
use crate::build::state::type_name;

/// An order whose character's data is older than this many seconds shows its age amber.
pub(crate) const STALE_SECS: i64 = 600;

/// One market order, as the table shows it.
pub(crate) struct Entry {
    pub type_id: TypeId,
    pub sell: bool,
    pub corporation: bool,
    pub item: String,
    pub character: String,
    pub location: String,
    pub price: Money,
    pub left: u64,
    pub total: u64,
    pub issued: i64,
    /// Seconds until the order ends; zero or less when it has ended.
    pub expires_in: Option<i64>,
    pub escrow: Option<Money>,
    /// The Forge's best price on the order's side, when the file has the type.
    pub forge_best: Option<Money>,
    /// How far the order is behind `forge_best`, when it is behind it.
    pub beaten_by: Option<Money>,
    /// Seconds since the character's orders were fetched.
    pub age: Option<i64>,
}

/// Every order of every login character, sells first, then by item name.
pub(crate) fn entries(cx: &mut Shared, now: i64) -> Vec<Entry> {
    let held = cx.market.held();
    let sde = cx.sde.as_ref();
    let mut entries = Vec::new();
    for character in cx.session.characters() {
        let Some(fetched) = cx
            .session
            .character_data(character.id)
            .and_then(|data| data.orders.as_ref())
        else {
            continue;
        };
        let age = now.checked_sub(fetched.fetched_at);
        for order in &fetched.data {
            let side = if order.is_buy { Side::Buy } else { Side::Sell };
            let forge_best = held
                .rows
                .get(&(order.type_id, side))
                .map(|row| row.price(side, Stat::Best));
            let beaten_by = forge_best.and_then(|best| {
                let (worse, gap) = if order.is_buy {
                    (order.price < best, best.checked_sub(order.price))
                } else {
                    (order.price > best, order.price.checked_sub(best))
                };
                if worse { gap } else { None }
            });
            entries.push(Entry {
                type_id: order.type_id,
                sell: !order.is_buy,
                corporation: order.is_corporation,
                item: type_name(order.type_id, sde, &mut cx.notices),
                character: character.name.clone(),
                location: cx.session.place_label(order.location_id, sde),
                price: order.price,
                left: order.volume_remain,
                total: order.volume_total,
                issued: order.issued,
                expires_in: order.expires.checked_sub(now),
                escrow: order.escrow,
                forge_best,
                beaten_by,
                age,
            });
        }
    }
    entries.sort_by(|left, right| {
        right
            .sell
            .cmp(&left.sell)
            .then_with(|| left.item.to_lowercase().cmp(&right.item.to_lowercase()))
            .then_with(|| left.corporation.cmp(&right.corporation))
    });
    entries
}

/// The footer line for the orders shown: their count, the sell value left and the escrow.
/// Both sums are kept in `i128`; a sum too large for the ISK it shows reads `too large`.
pub(crate) fn footer(shown: &[&Entry]) -> String {
    let sell = shown
        .iter()
        .filter(|entry| entry.sell)
        .try_fold(0i128, |sum, entry| {
            let value =
                i128::from(entry.price.ten_thousandths()).checked_mul(i128::from(entry.left))?;
            sum.checked_add(value)
        });
    let escrow = shown
        .iter()
        .filter(|entry| !entry.sell)
        .try_fold(0i128, |sum, entry| {
            let value = i128::from(entry.escrow.unwrap_or(Money::ZERO).ten_thousandths());
            sum.checked_add(value)
        });
    format!(
        "{} orders · sell {} · escrow {}",
        shown.len(),
        isk_text(sell),
        isk_text(escrow),
    )
}

/// An amount in ten-thousandths of ISK, shown as ISK; `too large` when it does not fit.
fn isk_text(amount: Option<i128>) -> String {
    amount
        .and_then(|amount| i64::try_from(amount).ok())
        .map_or_else(
            || "too large".to_owned(),
            |amount| Money::from_ten_thousandths(amount).format_isk(),
        )
}

/// A count with thousands grouped, such as `1,234`.
pub(crate) fn grouped(value: u64) -> String {
    i64::try_from(value).map_or_else(|_| value.to_string(), group_thousands)
}

/// A UTC time as EVE shows it, such as `2026-10-19 14:05`.
pub(crate) fn utc_text(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let within = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        within / 3_600,
        within % 3_600 / 60,
    )
}

/// The year, month and day of a count of days since 1970-01-01 (Howard Hinnant's civil calendar).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// The text shown in a cell of at most `max` characters: a longer text is cut and ends in `…`.
pub(crate) fn short(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// The text of an order's time left: `in 3d`, or `expired`.
pub(crate) fn expires_text(expires_in: Option<i64>) -> String {
    match expires_in {
        Some(seconds) if seconds > 0 => format!("in {}", age_text(seconds)),
        Some(_) => "expired".to_owned(),
        None => "—".to_owned(),
    }
}

/// The text of a data age, such as `2m`.
pub(crate) fn age_label(age: Option<i64>) -> String {
    age.map_or_else(|| "—".to_owned(), age_text)
}
