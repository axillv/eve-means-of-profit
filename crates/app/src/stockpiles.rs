//! The Stockpiles page: the list of stockpiles, what one holds against its targets and the plans'
//! claims, and what is short.

mod buy;
mod contents;
mod edit;
mod import;
mod list;

pub use import::import_file;

use std::collections::BTreeMap;

use egui::Ui;
use mop_core::buylist::LinePrice;
use mop_core::industry::TypeId;
use mop_core::money::Money;
use mop_core::reserve::{self, ClaimedLine, Claims, ShortMode};
use mop_core::stock::{CharacterId, LocationId, ResolvedAsset, resolve};
use mop_core::stockpile::{Held, Stock, Stockpile, StockpileError};
use mop_esi::places::FIRST_STRUCTURE_ID;
use mop_market::book::PriceRule;
use mop_sde::Sde;

use crate::Shared;
use crate::build::prices::{Held as PricesHeld, Live, line_price, live};
use crate::build::state::{push_error, refresh_claims};
use crate::item_panel;
use crate::session::Session;

const FIRST_SYSTEM: LocationId = 30_000_000;
const LAST_SYSTEM: LocationId = 32_999_999;
const FIRST_STATION: LocationId = 60_000_000;
const LAST_STATION: LocationId = 63_999_999;

/// The Stockpiles page state: the list's dialogs, the buy window and the jEve import.
#[derive(Debug, Default)]
pub struct StockpilesPage {
    pub(crate) list: list::State,
    pub(crate) buy: buy::State,
    pub(crate) edit: edit::State,
    pub(crate) import: import::State,
}

/// One panel of the Stockpiles page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StockpilesPanel {
    Stockpiles,
    Contents,
    Item,
}

impl StockpilesPanel {
    pub fn title(self) -> &'static str {
        match self {
            Self::Stockpiles => "Stockpiles",
            Self::Contents => "Contents",
            Self::Item => "Item",
        }
    }
}

impl StockpilesPage {
    /// Selects the stockpile with `stockpile_id`. An id the store does not hold clears the
    /// selection.
    pub fn open(&mut self, stockpile_id: i64, cx: &mut Shared) {
        self.buy.close();
        self.list.reset();
        match cx.store.stockpile(stockpile_id) {
            Ok(Some(_)) => cx.selection.stockpile = Some(stockpile_id),
            Ok(None) => cx.selection.stockpile = None,
            Err(error) => {
                cx.selection.stockpile = None;
                push_error(
                    &mut cx.notices,
                    format!("Could not open the stockpile: {error}"),
                );
            }
        }
    }

    /// Opens the stockpile editor on `stockpile_id`, keeping it selected.
    pub fn edit(&mut self, stockpile_id: i64, cx: &mut Shared) {
        self.open(stockpile_id, cx);
        self.edit.open(stockpile_id, cx);
    }

    pub(crate) fn open_import(&mut self) {
        self.import.start();
    }
}

impl StockpilesPage {
    /// Draws the body of one panel of this page.
    pub fn show(&mut self, panel: StockpilesPanel, ui: &mut Ui, cx: &mut Shared) {
        match panel {
            StockpilesPanel::Stockpiles => list::show(self, ui, cx),
            StockpilesPanel::Contents => contents::show(self, ui, cx),
            StockpilesPanel::Item => item_panel::show(ui, cx),
        }
    }

    /// Draws the page's windows once a frame, after the dock.
    pub fn show_windows(&mut self, ui: &mut Ui, cx: &mut Shared) {
        self.edit.show(ui, cx);
        self.import.show(ui, cx);
    }
}

/// Draws the body of one panel of this page.
pub fn show(page: &mut StockpilesPage, panel: StockpilesPanel, ui: &mut Ui, cx: &mut Shared) {
    page.show(panel, ui, cx);
}

/// Everything the panels read, taken from the session and the store once a frame. The claims are
/// brought up to date first, so every reader of `claims` sees the live reservations.
pub(crate) struct Snapshot {
    pub(crate) all: BTreeMap<i64, Stockpile>,
    pub(crate) ordered: Vec<Stockpile>,
    pub(crate) plan_names: BTreeMap<i64, String>,
    pub(crate) assets: Vec<ResolvedAsset>,
    pub(crate) sell_orders: Vec<Held>,
    pub(crate) buy_orders: Vec<Held>,
    pub(crate) jobs: Vec<Held>,
    pub(crate) claims: Claims,
    pub(crate) modes: BTreeMap<i64, ShortMode>,
    pub(crate) price_rule: PriceRule,
    pub(crate) now: i64,
}

impl Snapshot {
    pub(crate) fn stock(&self) -> Stock<'_> {
        Stock {
            assets: &self.assets,
            sell_orders: &self.sell_orders,
            buy_orders: &self.buy_orders,
            jobs: &self.jobs,
        }
    }

    pub(crate) fn mode(&self, id: i64) -> ShortMode {
        self.modes.get(&id).copied().unwrap_or_default()
    }
}

/// Reads the session's stock, the store's stockpiles and the live claims. A store that cannot be
/// read leaves that part empty and reports it once.
pub(crate) fn snapshot(cx: &mut Shared) -> Snapshot {
    refresh_claims(cx);
    let now = (cx.clock)();
    let assets = resolve(&cx.session.stock_characters()).assets;
    let mut sell_orders = Vec::new();
    let mut buy_orders = Vec::new();
    for character in cx.session.characters() {
        let Some(data) = cx.session.character_data(character.id) else {
            continue;
        };
        let Some(orders) = data.orders.as_ref() else {
            continue;
        };
        let Ok(character_id) = CharacterId::try_from(character.id) else {
            continue;
        };
        for order in &orders.data {
            let held = Held {
                character_id,
                type_id: order.type_id,
                place: order.location_id,
                units: order.volume_remain,
            };
            if order.is_buy {
                buy_orders.push(held);
            } else {
                sell_orders.push(held);
            }
        }
    }
    let jobs = match cx.store.job_assignments() {
        Ok(assignments) => reserve::unassigned(&cx.claims.running, &assignments),
        Err(error) => {
            push_error(
                &mut cx.notices,
                format!("Could not read the job assignments: {error}"),
            );
            Vec::new()
        }
    };
    let ordered = match cx.store.stockpiles() {
        Ok(list) => list,
        Err(error) => {
            push_error(
                &mut cx.notices,
                format!("Could not read the stockpiles: {error}"),
            );
            Vec::new()
        }
    };
    let mut modes = BTreeMap::new();
    for stockpile in &ordered {
        match cx.store.short_mode(stockpile.id) {
            Ok(mode) => {
                modes.insert(stockpile.id, mode);
            }
            Err(error) => push_error(
                &mut cx.notices,
                format!("Could not read the short mode: {error}"),
            ),
        }
    }
    let plan_names = match cx.store.plans() {
        Ok(plans) => plans.into_iter().map(|plan| (plan.id, plan.name)).collect(),
        Err(error) => {
            push_error(
                &mut cx.notices,
                format!("Could not read the plans: {error}"),
            );
            BTreeMap::new()
        }
    };
    let price_rule = match cx.store.settings() {
        Ok(settings) => settings.default_price_rule,
        Err(error) => {
            push_error(
                &mut cx.notices,
                format!("Could not read the settings: {error}"),
            );
            PriceRule::SellFivePercent
        }
    };
    let all = ordered
        .iter()
        .map(|stockpile| (stockpile.id, stockpile.clone()))
        .collect();
    Snapshot {
        all,
        ordered,
        plan_names,
        assets,
        sell_orders,
        buy_orders,
        jobs,
        claims: cx.claims.claims.clone(),
        modes,
        price_rule,
        now,
    }
}

/// One stockpile's lines with the claims of the plans.
pub(crate) fn claimed_lines(
    snapshot: &Snapshot,
    id: i64,
    mode: ShortMode,
    sde: &Sde,
    session: &Session,
) -> Result<Vec<ClaimedLine>, StockpileError> {
    let stock = snapshot.stock();
    let locate = |place: LocationId| locate(sde, session, place);
    reserve::claimed_lines(id, &snapshot.all, &stock, &locate, &snapshot.claims, mode)
}

/// Where a place is: the system and region of a station or system from the game data, or of a
/// structure from its entry. Anything else has no place.
pub(crate) fn locate(sde: &Sde, session: &Session, place: LocationId) -> Option<(u32, u32)> {
    let system = if (FIRST_SYSTEM..=LAST_SYSTEM).contains(&place) {
        u32::try_from(place).ok()
    } else if (FIRST_STATION..=LAST_STATION).contains(&place) {
        match sde.station(place) {
            Ok(station) => station.map(|station| station.system_id),
            Err(_) => None,
        }
    } else if place >= FIRST_STRUCTURE_ID {
        session
            .structures()
            .get(&place)
            .and_then(|entry| entry.system_id)
    } else {
        None
    };
    let solar = match sde.system(system?) {
        Ok(solar) => solar?,
        Err(_) => return None,
    };
    Some((solar.id, solar.region_id))
}

/// The cost of buying `units` of `type_id` at `rule`: `None` without a full price or for no units.
pub(crate) fn short_value(
    type_id: TypeId,
    units: u64,
    rule: PriceRule,
    held: &PricesHeld,
) -> Option<Money> {
    if units == 0 {
        return None;
    }
    let quote = live(held, type_id, rule);
    match line_price(quote.as_ref(), None, units) {
        Ok(LinePrice::Priced { total, .. }) => Some(total),
        Ok(LinePrice::Thin { .. } | LinePrice::Missing) | Err(_) => None,
    }
}

/// The live price of `type_id` under `rule`, if the file has one.
pub(crate) fn live_price(type_id: TypeId, rule: PriceRule, held: &PricesHeld) -> Option<Live> {
    live(held, type_id, rule)
}

/// A count with thousands grouped, such as `1,250`.
pub(crate) fn count_text(value: u64) -> String {
    i64::try_from(value).map_or_else(|_| value.to_string(), mop_core::format::group_thousands)
}

/// Sums the values of the rows with something short, and counts the ones without a price.
pub(crate) fn short_total(
    lines: &[ClaimedLine],
    rule: PriceRule,
    held: &PricesHeld,
) -> (Money, usize) {
    let mut total = Money::ZERO;
    let mut incomplete = 0_usize;
    for line in lines.iter().filter(|line| line.short > 0) {
        match short_value(line.line.type_id, line.short, rule, held) {
            Some(value) => match total.checked_add(value) {
                Some(sum) => total = sum,
                None => incomplete += 1,
            },
            None => incomplete += 1,
        }
    }
    (total, incomplete)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_place_is_a_station_system_or_a_structure_entry() {
        let dir = tempfile::TempDir::new().expect("a temp folder");
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../sde/tests/fixtures/sde-3607705");
        let db = dir.path().join("sde.sqlite");
        mop_sde::import(&source, &db).expect("the fixture imports");
        let sde = Sde::open(&db).expect("the fixture opens");
        let session = Session::new(Box::new(crate::session::NoEve));

        let jita = locate(&sde, &session, 60_003_760).expect("Jita has a system");
        assert_eq!(jita.0, 30_000_142);
        assert_eq!(jita.1, 10_000_002);
        assert_eq!(
            locate(&sde, &session, 30_000_142),
            Some((30_000_142, 10_000_002))
        );
        assert_eq!(locate(&sde, &session, 99), None);
    }
}
