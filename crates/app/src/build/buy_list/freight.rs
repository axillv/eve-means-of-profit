//! The freight part of the Buy list: the services on the plan's route, their offers, and the cheapest
//! contract. Moved out of `buy_list.rs`; the bodies are unchanged.

use std::sync::Arc;

use egui::{Checkbox, ComboBox, Context, Id, RichText, Ui};
use mop_core::buylist::{BuyListError, buy_list};
use mop_core::freight::{FreightError, Packing, RateCard};
use mop_core::money::Money;
use mop_core::plan::Plan;
use mop_sde::Sde;
use mop_sde::map::StarMap;
use mop_store::PlanRecord;

use super::prices::Held;
use super::{Edit, Offer, buy_inputs, chosen, contract_line, offers};
use crate::Shared;
use crate::build::state::{report, type_name};

// `offers` calls `freight::quote`, which resolves to this module, so the core quote is re-exported here.
pub(super) use mop_core::freight::quote;

/// How long the freight services of a route are kept before the store is read again.
const SERVICES_KEPT_SECS: f64 = 1.0;
/// How long the star map is kept before the game data is read again. The map changes rarely.
const MAP_KEPT_SECS: f64 = 60.0;

/// A freight service on the plan's route: its rate card, and the jumps it charges for, or the
/// reason it has none.
#[derive(Clone)]
pub(super) struct Leg {
    pub(super) id: i64,
    pub(super) name: String,
    pub(super) card: RateCard,
    pub(super) jumps: Result<Option<u32>, String>,
}

/// The services of a route as they were read, and when.
#[derive(Clone)]
struct Legs {
    route: i64,
    read_at: f64,
    legs: Vec<Leg>,
}

/// The star map as it was read, and when.
#[derive(Clone)]
struct Map {
    read_at: f64,
    star: Option<Arc<StarMap>>,
}

/// The services of a route, read from the store at most once a second. The frames in between reuse
/// the last read, so the panel does not read the store every frame.
pub(super) fn freight_legs(ctx: &Context, route: i64, cx: &mut Shared) -> Vec<Leg> {
    let now = ctx.input(|input| input.time);
    let kept = ctx.data(|data| data.get_temp::<Legs>(legs_key()));
    if let Some(kept) = kept
        && kept.route == route
        && now - kept.read_at < SERVICES_KEPT_SECS
    {
        return kept.legs;
    }
    let star = star_map(ctx, now, cx);
    let legs = read_legs(route, star.as_deref(), cx);
    let kept = Legs {
        route,
        read_at: now,
        legs: legs.clone(),
    };
    ctx.data_mut(|data| data.insert_temp(legs_key(), kept));
    legs
}

/// The services of a route with the jumps each charges for, read from the store.
fn read_legs(route_id: i64, star: Option<&StarMap>, cx: &mut Shared) -> Vec<Leg> {
    let routes = match cx.store.routes() {
        Ok(routes) => routes,
        Err(error) => {
            report(cx, format!("Could not read the routes: {error}"));
            return Vec::new();
        }
    };
    let Some(route) = routes.into_iter().find(|route| route.id == route_id) else {
        return Vec::new();
    };
    let services = match cx.store.services(route_id) {
        Ok(services) => services,
        Err(error) => {
            report(cx, format!("Could not read the freight services: {error}"));
            return Vec::new();
        }
    };
    services
        .into_iter()
        .map(|service| {
            let jumps = match (service.jumps_override, star) {
                (Some(jumps), _) => Ok(Some(jumps)),
                (None, Some(star)) => star
                    .jumps(route.from_system, route.to_system, service.route_kind)
                    .map_err(|error| error.to_string()),
                (None, None) => Ok(None),
            };
            Leg {
                id: service.id,
                name: service.name,
                card: service.card,
                jumps,
            }
        })
        .collect()
}

/// The star map of the game data, read at most once a minute.
fn star_map(ctx: &Context, now: f64, cx: &mut Shared) -> Option<Arc<StarMap>> {
    let kept = ctx.data(|data| data.get_temp::<Map>(map_key()));
    if let Some(kept) = kept
        && now - kept.read_at < MAP_KEPT_SECS
    {
        return kept.star;
    }
    let star = match cx.sde.as_ref().map(Sde::star_map) {
        Some(Ok(star)) => Some(Arc::new(star)),
        Some(Err(error)) => {
            report(cx, format!("Could not read the map: {error}"));
            None
        }
        None => None,
    };
    let kept = Map {
        read_at: now,
        star: star.clone(),
    };
    ctx.data_mut(|data| data.insert_temp(map_key(), kept));
    star
}

fn legs_key() -> Id {
    Id::new("build buy list freight legs")
}

fn map_key() -> Id {
    Id::new("build buy list star map")
}

/// Why a service cannot carry the cargo, in words the panel shows.
pub(super) fn freight_reason(error: &FreightError, cx: &mut Shared) -> String {
    match error {
        FreightError::CannotCarry(type_id) => format!(
            "cannot carry {}",
            type_name(*type_id, cx.sde.as_ref(), &mut cx.notices)
        ),
        FreightError::NegativeRate | FreightError::Overflow => error.to_string(),
    }
}

/// The cheapest offer that has a quote; the earlier one wins a tie.
pub(super) fn cheapest(offers: &[Offer]) -> Option<usize> {
    let mut best: Option<(usize, Money)> = None;
    for (index, offer) in offers.iter().enumerate() {
        let Ok(quote) = &offer.outcome else { continue };
        if best.is_none_or(|(_, so_far)| quote.total < so_far) {
            best = Some((index, quote.total));
        }
    }
    best.map(|(index, _)| index)
}

/// The freight row: the service dropdown, the top-up tick, and the contracts of the chosen service.
pub(super) fn freight_row(
    ui: &mut Ui,
    record: &PlanRecord,
    offers: &[Offer],
    chosen: Option<usize>,
    cx: &Shared,
) -> Option<Edit> {
    ui.label("Freight");
    if record.route.is_none() {
        ui.label(RichText::new("No freight").color(cx.style.mute));
        return None;
    }
    let cheapest_name = cheapest(offers)
        .and_then(|index| offers.get(index))
        .map_or("none", |offer| offer.name.as_str());
    let mut options: Vec<(Option<i64>, String)> =
        vec![(None, format!("Cheapest ({cheapest_name})"))];
    options.extend(
        offers
            .iter()
            .map(|offer| (Some(offer.id), offer.name.clone())),
    );
    let mut edit = None;
    if let Some(choice) = choose(ui, "build_buy_service", record.service, &options) {
        edit = Some(Edit::Service(choice));
    }
    let mut top_up = record.packing == Packing::TopUp;
    if ui
        .add(Checkbox::new(&mut top_up, "Top up the last contract"))
        .changed()
    {
        edit = Some(Edit::TopUp(top_up));
    }
    match chosen
        .and_then(|index| offers.get(index))
        .map(|offer| &offer.outcome)
    {
        Some(Ok(quote)) => {
            let count = quote.contracts.len();
            let text = if count == 1 {
                "1 contract".to_owned()
            } else {
                format!("{count} contracts")
            };
            ui.label(text).on_hover_ui(|ui| {
                for (index, contract) in quote.contracts.iter().enumerate() {
                    ui.label(contract_line(index + 1, contract));
                }
            });
        }
        Some(Err(reason)) => {
            ui.label(RichText::new(reason.as_str()).color(cx.style.neg));
        }
        None => {
            ui.label(RichText::new("no freight service can carry this").color(cx.style.neg));
        }
    }
    edit
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

/// The rate card of the freight service the Buy list shows as chosen: none when the plan has no
/// route. The job cost code reads it, so a bought part's freight is the one the Buy list charges.
///
/// # Errors
/// The buy list cannot be worked out, so neither can the service it would choose.
pub(crate) fn chosen_card(
    ui: &Ui,
    record: &PlanRecord,
    plan: &Plan,
    cx: &mut Shared,
    held: &Held,
    now: i64,
) -> Result<Option<RateCard>, String> {
    if record.route.is_none() {
        return Ok(None);
    }
    let (inputs, _) = buy_inputs(plan, record, cx, held, now);
    let buy = buy_list(inputs).map_err(|BuyListError::Overflow| {
        "the buy list does not add up: a volume or value does not fit the supported range"
            .to_owned()
    })?;
    let cargo = buy.cargo();
    let offers = offers(ui, record, cx, &cargo);
    let Some(index) = chosen(record.service, &offers) else {
        return Ok(None);
    };
    Ok(offers.into_iter().nth(index).map(|offer| offer.card))
}
