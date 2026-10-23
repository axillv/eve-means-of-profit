//! Reads Fuzzwork's Forge summary prices once and prints a few of them for Tritanium.
//!
//! Run with `cargo run -p mop-market --example forge`.

use std::error::Error;
use std::io::{self, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use mop_market::fuzzwork::{FORGE, FuzzworkMarket, Side, Stat};
use mop_market::http::UreqHttp;

const TRITANIUM: u32 = 34;
const STATS: [(Stat, &str); 4] = [
    (Stat::Best, "best"),
    (Stat::FivePercent, "five percent"),
    (Stat::WeightedAverage, "weighted average"),
    (Stat::Median, "median"),
];

fn main() -> Result<(), Box<dyn Error>> {
    let mut out = io::stdout();
    let now = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())?;
    writeln!(out, "requested at Unix {now}")?;
    let mut market = FuzzworkMarket::new(UreqHttp::new(), FORGE);
    let refresh = market.refresh(now)?;
    writeln!(out, "refresh: {refresh:?}")?;
    writeln!(out, "Forge rows: {}", market.rows().count())?;
    writeln!(out, "published at Unix {:?}", market.published_at())?;
    writeln!(out, "orderSet: {:?}", market.order_set())?;
    for side in [Side::Sell, Side::Buy] {
        for (stat, label) in STATS {
            let price = market
                .price(TRITANIUM, side, stat)
                .map_or_else(|| "none".to_owned(), |price| price.to_plain());
            writeln!(out, "Tritanium {side:?} {label}: {price}")?;
        }
    }
    Ok(())
}
