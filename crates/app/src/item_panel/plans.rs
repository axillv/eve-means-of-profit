//! The saved plans that use the selected item: the plans whose product is the item, or that build
//! or buy it. Worked out when the selection or the saved plans change, not every frame.

use egui::{Id, RichText, Ui};
use mop_core::industry::TypeId;
use mop_core::plan::Plan;
use mop_store::{PlanRecord, PlanSummary};

use super::grouped;
use crate::Shared;
use crate::build::state::plan_for;

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
        match plan_for(&record, &stock, cx, &facilities) {
            Ok(plan) => {
                if let Some(text) = usage(&record, &plan, type_id) {
                    lines.push(Line { text, muted: false });
                }
            }
            Err(_) => lines.push(not_worked_out(&record.name)),
        }
    }
    lines
}

fn not_worked_out(name: &str) -> Line {
    Line {
        text: format!("{name} · could not work out the plan"),
        muted: true,
    }
}

/// How a plan uses the item: as its product, or the units it needs as a built step or a bought
/// type. `None` when the plan has no use of it.
fn usage(record: &PlanRecord, plan: &Plan, type_id: TypeId) -> Option<String> {
    if record.product == type_id {
        return Some(format!(
            "{} · product × {}",
            record.name,
            grouped(record.quantity)
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
