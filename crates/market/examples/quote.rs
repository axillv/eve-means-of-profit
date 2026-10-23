//! Prices a few items at Jita under each rule, from live ESI data.
//!
//! Run with `cargo run -p mop-market --example quote`.

use std::error::Error;
use std::io::{self, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use mop_market::book::{PriceRule, Quote, quote};
use mop_market::cache::Market;
use mop_market::http::UreqHttp;

const JITA: u64 = 60003760;
const QTY: u64 = 10_000;
const ITEMS: [(u32, &str); 2] = [(11481, "R.A.M.- Robotics"), (34, "Tritanium")];
const RULES: [(PriceRule, &str); 3] = [
    (PriceRule::WalkSell, "walk sell"),
    (PriceRule::LowestSell, "lowest sell"),
    (PriceRule::HighestBuy, "highest buy"),
];

fn main() -> Result<(), Box<dyn Error>> {
    let mut out = io::stdout();
    let now = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())?;
    writeln!(out, "requested at Unix {now}, {QTY} units each")?;
    let mut market = Market::new(UreqHttp::new());
    for (type_id, name) in ITEMS {
        let book = market.book(type_id, now)?;
        writeln!(
            out,
            "{name} ({type_id}): book last modified at Unix {}, from cache: {}",
            book.last_modified, book.from_cache
        )?;
        for (rule, label) in RULES {
            writeln!(
                out,
                "  {label:<12} {}",
                describe(quote(book.orders, JITA, rule, QTY)?)
            )?;
        }
    }
    Ok(())
}

fn describe(quote: Quote) -> String {
    match quote {
        Quote::Priced {
            total,
            per_unit,
            available,
        } => format!(
            "total {} ISK, per unit {} ISK, {available} units for sale",
            total.to_plain(),
            per_unit.to_plain()
        ),
        Quote::Thin {
            wanted,
            available,
            total_for_available,
        } => format!(
            "thin: {available} of {wanted} units for sale, those cost {} ISK",
            total_for_available.to_plain()
        ),
        Quote::NoOrders => "no orders".to_owned(),
    }
}
