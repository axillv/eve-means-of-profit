//! The saved plans that use the selected item: the plans one of whose products is the item, or
//! that build or buy it. Worked out when the selection or the saved plans change, not every frame.

use std::collections::BTreeMap;

use egui::{Id, RichText, Ui};
use mop_core::industry::TypeId;
use mop_core::plan::{Plan, PlanProduct};
use mop_store::{PlanRecord, PlanSummary};

use super::grouped;
use crate::Shared;
use crate::build::state::{plan_for, plan_with_products, report};

/// The text of one plan line, and how it is drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Line {
    text: String,
    muted: bool,
}

/// What the lines were worked out for. When any part changes the lines are worked out again.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Key {
    item: TypeId,
    game_data: bool,
    plans: Vec<(i64, i64)>,
}

/// The last lines worked out, kept in the frame memory under the key they were worked out for.
#[derive(Debug, Clone)]
struct Cached {
    key: Key,
    lines: Vec<Line>,
}

pub(super) fn show(ui: &mut Ui, cx: &mut Shared, type_id: TypeId) {
    ui.label(RichText::new("Plans that use it").color(cx.style.mute));
    let summaries = match cx.store.plans() {
        Ok(summaries) => summaries,
        Err(error) => {
            ui.label(
                RichText::new(format!("Could not read the plans: {error}")).color(cx.style.neg),
            );
            return;
        }
    };
    let key = Key {
        item: type_id,
        game_data: cx.sde.is_some(),
        plans: summaries
            .iter()
            .map(|plan| (plan.id, plan.updated_at))
            .collect(),
    };
    let cache = Id::new("item panel plans");
    let kept = ui.data(|data| data.get_temp::<Cached>(cache));
    let lines = match kept {
        Some(kept) if kept.key == key => kept.lines,
        _ => {
            let lines = lines_for(cx, type_id, &summaries);
            ui.data_mut(|data| {
                data.insert_temp(
                    cache,
                    Cached {
                        key,
                        lines: lines.clone(),
                    },
                );
            });
            lines
        }
    };
    if lines.is_empty() {
        ui.label(RichText::new("No saved plan uses it.").color(cx.style.mute));
    }
    for line in lines {
        let colour = if line.muted {
            cx.style.mute
        } else {
            cx.style.text
        };
        ui.label(RichText::new(line.text).color(colour));
    }
}

/// One line per saved plan that uses the item. A plan that does not work out is listed too, in mute.
fn lines_for(cx: &mut Shared, type_id: TypeId, summaries: &[PlanSummary]) -> Vec<Line> {
    let facilities = match cx.store.facilities() {
        Ok(facilities) => facilities,
        Err(error) => {
            return vec![Line {
                text: format!("Could not read the facilities: {error}"),
                muted: false,
            }];
        }
    };
    let mut lines = Vec::new();
    for summary in summaries {
        let record = match cx.store.plan(summary.id) {
            Ok(Some(record)) => record,
            Ok(None) => {
                lines.push(not_worked_out(&summary.name));
                continue;
            }
            Err(error) => {
                lines.push(Line {
                    text: format!("{} · could not read the plan: {error}", summary.name),
                    muted: false,
                });
                continue;
            }
        };
        let Ok(stock) = cx.store.plan_stock(summary.id) else {
            lines.push(not_worked_out(&summary.name));
            continue;
        };
        let (products, worked) = match cx.store.plan_products(summary.id) {
            Ok(products) => {
                let products = filled_products(products);
                let worked = plan_with_products(
                    &record,
                    &products,
                    &stock,
                    cx,
                    &facilities,
                    &BTreeMap::new(),
                );
                (products, worked)
            }
            Err(error) => {
                report(cx, format!("Could not read the plan's products: {error}"));
                // The record's own product still works out as the plan's only one.
                (
                    vec![PlanProduct {
                        type_id: record.product,
                        quantity: record.quantity,
                    }],
                    plan_for(&record, &stock, cx, &facilities),
                )
            }
        };
        match worked {
            Ok(plan) => {
                if let Some(text) = usage(&record, &products, &plan, type_id) {
                    lines.push(Line { text, muted: false });
                }
            }
            Err(_) => lines.push(not_worked_out(&record.name)),
        }
    }
    lines
}

/// The products a plan is worked out from and named by. An unfilled product has type 0 and is not
/// one of them, so it neither breaks the plan nor becomes a line of its own.
fn filled_products(products: Vec<PlanProduct>) -> Vec<PlanProduct> {
    products
        .into_iter()
        .filter(|product| product.type_id != 0)
        .collect()
}

fn not_worked_out(name: &str) -> Line {
    Line {
        text: format!("{name} · could not work out the plan"),
        muted: true,
    }
}

/// How a plan uses the item: as any of its products, or the units it needs as a built step or a
/// bought type. `None` when the plan has no use of it. A product a plan never filled in has type 0
/// and is not one of its products.
fn usage(
    record: &PlanRecord,
    products: &[PlanProduct],
    plan: &Plan,
    type_id: TypeId,
) -> Option<String> {
    if let Some(product) = products
        .iter()
        .find(|product| product.type_id == type_id && product.type_id != 0)
    {
        return Some(format!(
            "{} · product × {}",
            record.name,
            grouped(product.quantity)
        ));
    }
    let need = plan
        .steps
        .iter()
        .find(|step| step.type_id == type_id)
        .map(|step| step.needed)
        .or_else(|| {
            plan.buy
                .iter()
                .find(|line| line.type_id == type_id)
                .map(|line| line.needed)
        })?;
    Some(format!("{} · need {}", record.name, grouped(need)))
}

#[cfg(test)]
mod tests {
    use mop_core::plan::BuyLine;

    use super::*;

    const HOBGOBLIN_II: TypeId = 2456;
    const RIFTER: TypeId = 587;
    const TRITANIUM: TypeId = 34;
    const ZYDRINE: TypeId = 39;

    fn product(type_id: TypeId, quantity: u64) -> PlanProduct {
        PlanProduct { type_id, quantity }
    }

    fn record() -> PlanRecord {
        PlanRecord {
            id: 1,
            name: "Kit".to_owned(),
            product: HOBGOBLIN_II,
            quantity: 10,
            build: Default::default(),
            me: Default::default(),
            manufacturing: None,
            reactions: None,
            price_rule: Default::default(),
            route: None,
            service: None,
            packing: Default::default(),
            pasted_stock: String::new(),
            created_at: 0,
            updated_at: 0,
        }
    }

    fn plan() -> Plan {
        Plan {
            steps: Vec::new(),
            buy: vec![BuyLine {
                type_id: TRITANIUM,
                depth: 0,
                needed: 64_000,
                from_stock: 0,
                to_buy: 64_000,
            }],
            unused_stock: Vec::new(),
        }
    }

    #[test]
    fn every_product_is_named_with_its_own_quantity() {
        let products = [product(HOBGOBLIN_II, 10), product(RIFTER, 2)];

        assert_eq!(
            usage(&record(), &products, &plan(), HOBGOBLIN_II).as_deref(),
            Some("Kit · product × 10")
        );
        assert_eq!(
            usage(&record(), &products, &plan(), RIFTER).as_deref(),
            Some("Kit · product × 2")
        );
    }

    #[test]
    fn an_unfilled_product_is_left_out_and_the_others_still_count() {
        let products = filled_products(vec![product(RIFTER, 2), product(0, 1)]);

        assert_eq!(products, vec![product(RIFTER, 2)]);
        assert_eq!(
            usage(&record(), &products, &plan(), RIFTER).as_deref(),
            Some("Kit · product × 2")
        );
        assert_eq!(
            usage(&record(), &products, &plan(), TRITANIUM).as_deref(),
            Some("Kit · need 64,000")
        );
        assert!(filled_products(vec![product(0, 1)]).is_empty());
    }

    #[test]
    fn one_product_is_named_as_before() {
        let products = [product(HOBGOBLIN_II, 10)];

        assert_eq!(
            usage(&record(), &products, &plan(), HOBGOBLIN_II).as_deref(),
            Some("Kit · product × 10")
        );
        assert_eq!(
            usage(&record(), &products, &plan(), TRITANIUM).as_deref(),
            Some("Kit · need 64,000")
        );
    }

    #[test]
    fn a_material_keeps_the_units_needed() {
        let products = [product(HOBGOBLIN_II, 10), product(RIFTER, 2)];

        assert_eq!(
            usage(&record(), &products, &plan(), TRITANIUM).as_deref(),
            Some("Kit · need 64,000")
        );
        assert_eq!(usage(&record(), &products, &plan(), ZYDRINE), None);
    }
}
