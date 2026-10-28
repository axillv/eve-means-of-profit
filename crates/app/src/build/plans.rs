//! The Plans panel: the saved plans in the order the store lists them, the buttons that make, copy,
//! rename and delete them, and the dialogs those ask through.

use std::collections::BTreeMap;

use egui::{Align2, Button, Context, CursorIcon, Id, RichText, Sense, TextEdit, Ui, Window};
use mop_core::industry::TypeId;
use mop_core::paste::parse_stock;
use mop_core::reserve;
use mop_core::stock::resolve;
use mop_store::plans::PlanStage;
use mop_store::{PlanStock, PlanSummary};

use super::state::{BuildPage, Dialog, Next, refresh_claims, report};
use crate::Shared;

/// What the player picked in a dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    Save,
    Discard,
    Confirm,
    Cancel,
}

pub fn show(page: &mut BuildPage, ui: &mut Ui, cx: &mut Shared) {
    refresh_claims(cx);
    let mut plans = match cx.store.plans() {
        Ok(plans) => plans,
        Err(error) => {
            ui.label(
                RichText::new(format!("Could not read the plans: {error}")).color(cx.style.neg),
            );
            return;
        }
    };
    let reserving = match cx.store.reserving_plans() {
        Ok(ids) => ids,
        Err(error) => {
            report(cx, format!("Could not read the reserving plans: {error}"));
            Vec::new()
        }
    };
    advance_stages(&mut plans, page, cx);
    let selected = page.selected.is_some();
    let (mut new, mut duplicate, mut rename, mut delete) = (false, false, false, false);
    ui.horizontal(|ui| {
        new = ui.button("New").clicked();
        rename = ui.add_enabled(selected, Button::new("Rename")).clicked();
        duplicate = ui.add_enabled(selected, Button::new("Duplicate")).clicked();
        delete = ui.add_enabled(selected, Button::new("Delete")).clicked();
    });

    let picked = plan_rows(ui, &plans, &reserving, page.selected);
    if let Some((id, stage)) = picked.stage {
        set_stage(cx, id, stage);
    }
    if let Some(order) = picked.order {
        save_order(page, cx, &order);
    }
    if let Some(id) = picked.plan {
        page.request(Next::Select(id), cx);
    }
    if new {
        page.request(Next::New, cx);
    }
    if duplicate {
        page.request(Next::Duplicate, cx);
    }
    let name_of = |id: i64| {
        plans
            .iter()
            .find(|plan| plan.id == id)
            .map(|plan| plan.name.clone())
    };
    if let (true, Some(id)) = (rename, page.selected)
        && let Some(name) = name_of(id)
    {
        page.dialog = Some(Dialog::Rename { id, text: name });
    }
    if let (true, Some(id)) = (delete, page.selected)
        && let Some(name) = name_of(id)
    {
        page.dialog = Some(Dialog::Delete { id, name });
    }
    if let Some(dialog) = page.dialog.take() {
        page.dialog = match dialog {
            Dialog::Leave(next) => leave(page, ui.ctx(), cx, next),
            Dialog::Rename { id, text } => rename_dialog(page, ui.ctx(), cx, id, text),
            Dialog::Delete { id, name } => delete_dialog(page, ui.ctx(), cx, id, name),
        };
    }
}

/// What the plan rows picked this frame: a plan to show, or a new list order to save.
#[derive(Default)]
struct Picked {
    plan: Option<i64>,
    order: Option<Vec<i64>>,
    /// A stage picked from the row's menu: the plan and the stage to show, or `None` for automatic.
    stage: Option<(i64, Option<PlanStage>)>,
}

/// The signs the stage's automatic advancement reads.
#[expect(
    clippy::struct_excessive_bools,
    reason = "each game signal is a separate yes/no fact the stage formula reads"
)]
#[derive(Debug, Clone, Copy)]
struct StageFacts {
    owned: u64,
    quantity: u64,
    material_in: bool,
    assigned: bool,
    selling: bool,
    reached_done: bool,
}

/// The furthest stage the game data shows. A stage once reached is never lowered here.
fn automatic_stage(facts: &StageFacts) -> PlanStage {
    if facts.selling && facts.reached_done {
        PlanStage::Sold
    } else if facts.owned >= facts.quantity {
        PlanStage::Done
    } else if facts.assigned {
        PlanStage::Building
    } else if facts.material_in {
        PlanStage::Buying
    } else {
        PlanStage::Planned
    }
}

/// The stage tag on a row: ` · Buying`, or ` · Done (you)` when the user set it.
fn stage_label(stage: PlanStage, manual: bool) -> String {
    if manual {
        format!(" · {} (you)", stage.label())
    } else {
        format!(" · {}", stage.label())
    }
}

const STAGE_AUTO_HOVER: &str = "Stage follows the game; a job for this plan moves it to Building.";
const STAGE_MANUAL_HOVER: &str = "You set this stage; Automatic follows the game again.";

/// The row menu's stage choices, in order: `Automatic` clears the override, the rest set it.
const STAGE_MENU: [(&str, Option<PlanStage>); 6] = [
    ("Automatic", None),
    ("Planned", Some(PlanStage::Planned)),
    ("Buying", Some(PlanStage::Buying)),
    ("Building", Some(PlanStage::Building)),
    ("Done", Some(PlanStage::Done)),
    ("Sold", Some(PlanStage::Sold)),
];

/// Whether the row menu marks `choice`: `Automatic` when no override is set, else the override.
fn stage_marked(override_stage: Option<PlanStage>, choice: Option<PlanStage>) -> bool {
    choice.map_or(override_stage.is_none(), |stage| {
        override_stage == Some(stage)
    })
}

/// Sets the stage the row shows, or clears it so the automatic stage shows again.
fn set_stage(cx: &mut Shared, id: i64, stage: Option<PlanStage>) {
    if let Err(error) = cx.store.set_plan_stage_override(id, stage) {
        report(cx, format!("Could not set the plan stage: {error}"));
    }
}

/// Raises the automatic stage of every plan from the server's stock, jobs and orders.
fn advance_stages(plans: &mut [PlanSummary], page: &mut BuildPage, cx: &mut Shared) {
    if page.recipes.is_empty()
        && let Some(sde) = cx.sde.as_ref()
        && let Ok(recipes) = sde.recipe_book()
    {
        page.recipes = recipes;
    }
    let now = (cx.clock)();
    let assets = resolve(&cx.session.stock_characters()).assets;
    let login: Vec<u64> = cx
        .session
        .characters()
        .iter()
        .map(|character| character.id)
        .collect();
    for summary in plans.iter_mut() {
        let Ok(Some(record)) = cx.store.plan(summary.id) else {
            continue;
        };
        let Ok(stock) = cx.store.plan_stock(summary.id) else {
            continue;
        };
        let counted: BTreeMap<TypeId, u64> = match stock {
            PlanStock::Characters(scope) => {
                reserve::available(&scope, &assets, &cx.claims.claims, Some(summary.id))
                    .unwrap_or_default()
            }
            PlanStock::Pasted => match cx.sde.as_ref() {
                Some(sde) => {
                    parse_stock(&record.pasted_stock, &|name| {
                        sde.find_exact(name).ok().flatten()
                    })
                    .stock
                }
                None => BTreeMap::new(),
            },
        };
        let owned = counted.get(&record.product).copied().unwrap_or(0);
        let mut inputs = record.build.clone();
        if let Some(recipe) = page.recipes.get(record.product) {
            inputs.extend(recipe.materials.iter().map(|(type_id, _)| *type_id));
        }
        let material_in = inputs
            .iter()
            .any(|type_id| counted.get(type_id).is_some_and(|units| *units > 0));
        let assigned = cx
            .claims
            .running
            .iter()
            .any(|job| cx.claims.assignments.get(&job.job_id) == Some(&summary.id));
        let selling = login.iter().any(|&id| {
            cx.session
                .character_data(id)
                .and_then(|data| data.orders.as_ref())
                .is_some_and(|orders| {
                    orders.data.iter().any(|order| {
                        order.type_id == record.product
                            && !order.is_buy
                            && !order.is_corporation
                            && order.volume_remain > 0
                            && order.expires > now
                    })
                })
        });
        let auto = automatic_stage(&StageFacts {
            owned,
            quantity: record.quantity,
            material_in,
            assigned,
            selling,
            reached_done: summary.stage >= PlanStage::Done,
        });
        if auto > summary.stage {
            match cx.store.advance_plan_stage(summary.id, auto) {
                Ok(()) => summary.stage = auto,
                Err(error) => report(cx, format!("Could not save the plan stage: {error}")),
            }
        }
    }
}

/// Draws one selectable row per plan, each with the ` · reserves` label when it reserves. A click
/// picks the plan, a drag onto another row picks a new order, and the row's menu moves it one place.
fn plan_rows(
    ui: &mut Ui,
    plans: &[PlanSummary],
    reserving: &[i64],
    selected: Option<i64>,
) -> Picked {
    let mut picked = Picked::default();
    for (index, plan) in plans.iter().enumerate() {
        let row = ui.horizontal(|ui| {
            drop(ui.selectable_label(selected == Some(plan.id), plan.name.as_str()));
            let manual = plan.stage_override.is_some();
            let hover = if manual {
                STAGE_MANUAL_HOVER
            } else {
                STAGE_AUTO_HOVER
            };
            ui.label(RichText::new(stage_label(plan.effective_stage(), manual)).weak())
                .on_hover_text(hover);
            if reserving.contains(&plan.id) {
                ui.label(RichText::new(" · reserves").weak());
            }
        });
        // The whole row is the drag handle and catches the clicks the name label would, so a plan
        // can be dropped onto any part of another row.
        let response = ui
            .interact(
                row.response.rect,
                Id::new(("plan row", plan.id)),
                Sense::click_and_drag(),
            )
            .on_hover_cursor(CursorIcon::Grab);
        if response.clicked() {
            picked.plan = Some(plan.id);
        }
        response.dnd_set_drag_payload(plan.id);
        if let Some(dragged) = response.dnd_release_payload::<i64>()
            && *dragged != plan.id
        {
            picked.order = Some(moved(plans, *dragged, index));
        }
        response.context_menu(|menu| {
            let first = index == 0;
            let last = index + 1 == plans.len();
            menu.menu_button("Stage", |menu| {
                for (label, choice) in STAGE_MENU {
                    if menu
                        .radio(stage_marked(plan.stage_override, choice), label)
                        .clicked()
                    {
                        picked.stage = Some((plan.id, choice));
                        menu.close();
                    }
                }
            });
            if menu.add_enabled(!first, Button::new("Move up")).clicked() {
                picked.order = Some(swapped(plans, index, index - 1));
                menu.close();
            }
            if menu.add_enabled(!last, Button::new("Move down")).clicked() {
                picked.order = Some(swapped(plans, index, index + 1));
                menu.close();
            }
        });
    }
    picked
}

/// The order `plans` takes when the plan `from` is dragged onto the row at `to`: `from` leaves its
/// place and goes where `to` was before the move. `[A, B, C]` with A onto C gives `[B, C, A]`, and
/// C onto A gives `[C, A, B]`.
fn moved(plans: &[PlanSummary], from: i64, to: usize) -> Vec<i64> {
    let mut order: Vec<i64> = plans.iter().map(|plan| plan.id).collect();
    if let Some(index) = order.iter().position(|plan| *plan == from) {
        let id = order.remove(index);
        let at = to.min(order.len());
        order.insert(at, id);
    }
    order
}

/// The order `plans` takes when the plan at `from` swaps places with the one at `to`.
fn swapped(plans: &[PlanSummary], from: usize, to: usize) -> Vec<i64> {
    let mut order: Vec<i64> = plans.iter().map(|plan| plan.id).collect();
    order.swap(from, to);
    order
}

/// Saves a new list order and works the plans out again, so the next plan sees the moved claims.
fn save_order(page: &mut BuildPage, cx: &mut Shared, order: &[i64]) {
    match cx.store.set_plan_order(order) {
        Ok(()) => {
            cx.claims.mark_stale();
            page.recalc(cx);
        }
        Err(error) => report(cx, format!("Could not save the plan order: {error}")),
    }
}

/// Opens a centred window with `body`, and returns the answer picked in it, if any.
fn window(
    ctx: &Context,
    title: &str,
    body: impl FnOnce(&mut Ui) -> Option<Answer>,
) -> Option<Answer> {
    let mut answer = None;
    Window::new(title)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| answer = body(ui));
    answer
}

/// One button per choice, in a row. Returns the one clicked, if any.
fn buttons(ui: &mut Ui, choices: &[(&str, Answer)]) -> Option<Answer> {
    let mut answer = None;
    ui.horizontal(|ui| {
        for (label, choice) in choices {
            if ui.button(*label).clicked() {
                answer = Some(*choice);
            }
        }
    });
    answer
}

/// `Save changes to <name>?`: saves or drops the edits, then does what was asked. Returns the
/// dialog to keep open, if it is still open.
fn leave(page: &mut BuildPage, ctx: &Context, cx: &mut Shared, next: Next) -> Option<Dialog> {
    let name = page
        .record
        .as_ref()
        .map_or_else(String::new, |record| record.name.clone());
    let title = format!("Save changes to {name}?");
    let answer = window(ctx, &title, |ui| {
        buttons(
            ui,
            &[
                ("Save", Answer::Save),
                ("Discard", Answer::Discard),
                ("Cancel", Answer::Cancel),
            ],
        )
    });
    match answer {
        None => Some(Dialog::Leave(next)),
        Some(Answer::Cancel | Answer::Confirm) => None,
        Some(Answer::Save) => {
            if page.save(cx) {
                page.go(next, cx);
            }
            None
        }
        Some(Answer::Discard) => {
            page.discard(cx);
            page.go(next, cx);
            None
        }
    }
}

/// `Rename`: a name typed in, saved to the store. Returns the dialog to keep open, if any.
fn rename_dialog(
    page: &mut BuildPage,
    ctx: &Context,
    cx: &mut Shared,
    id: i64,
    mut text: String,
) -> Option<Dialog> {
    let answer = window(ctx, "Rename plan", |ui| {
        ui.add(TextEdit::singleline(&mut text).desired_width(240.0));
        buttons(
            ui,
            &[("Rename", Answer::Confirm), ("Cancel", Answer::Cancel)],
        )
    });
    match answer {
        None => Some(Dialog::Rename { id, text }),
        Some(Answer::Confirm) => {
            apply_rename(page, cx, id, &text);
            None
        }
        Some(_) => None,
    }
}

fn apply_rename(page: &mut BuildPage, cx: &mut Shared, id: i64, name: &str) {
    match cx.store.rename_plan(id, name) {
        Ok(()) => {
            if let Some(record) = page.record.as_mut().filter(|record| record.id == id) {
                record.name = name.trim().to_owned();
            }
        }
        Err(error) => report(cx, format!("Could not rename the plan: {error}")),
    }
}

/// `Delete plan <name>?`. Returns the dialog to keep open, if any.
fn delete_dialog(
    page: &mut BuildPage,
    ctx: &Context,
    cx: &mut Shared,
    id: i64,
    name: String,
) -> Option<Dialog> {
    let title = format!("Delete plan {name}?");
    let answer = window(ctx, &title, |ui| {
        buttons(
            ui,
            &[("Delete", Answer::Confirm), ("Cancel", Answer::Cancel)],
        )
    });
    match answer {
        None => Some(Dialog::Delete { id, name }),
        Some(Answer::Confirm) => {
            match cx.store.delete_plan(id) {
                Ok(()) => {
                    // A deleted plan no longer reserves, so the claims of the plans after it change.
                    cx.claims.mark_stale();
                    if page.selected == Some(id) {
                        page.clear();
                    } else if page.selected.is_some() {
                        page.recalc(cx);
                    }
                }
                Err(error) => report(cx, format!("Could not delete the plan: {error}")),
            }
            None
        }
        Some(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plan with nothing seen yet, so each test turns on only the signals it needs.
    fn facts(owned: u64, quantity: u64) -> StageFacts {
        StageFacts {
            owned,
            quantity,
            material_in: false,
            assigned: false,
            selling: false,
            reached_done: false,
        }
    }

    #[test]
    fn automatic_stage_follows_the_game_data() {
        assert_eq!(automatic_stage(&facts(0, 10)), PlanStage::Planned);
        assert_eq!(
            automatic_stage(&StageFacts {
                material_in: true,
                ..facts(0, 10)
            }),
            PlanStage::Buying
        );
        assert_eq!(
            automatic_stage(&StageFacts {
                material_in: true,
                assigned: true,
                ..facts(0, 10)
            }),
            PlanStage::Building
        );
        assert_eq!(
            automatic_stage(&StageFacts {
                material_in: true,
                assigned: true,
                ..facts(10, 10)
            }),
            PlanStage::Done
        );
        assert_eq!(
            automatic_stage(&StageFacts {
                selling: true,
                reached_done: true,
                ..facts(0, 10)
            }),
            PlanStage::Sold
        );
        assert_eq!(
            automatic_stage(&StageFacts {
                selling: true,
                ..facts(10, 10)
            }),
            PlanStage::Done
        );
    }

    #[test]
    fn every_stage_is_reached_in_order() {
        let stages = [
            PlanStage::Planned,
            PlanStage::Buying,
            PlanStage::Building,
            PlanStage::Done,
            PlanStage::Sold,
        ];
        assert!(stages.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn stage_label_names_the_stage_and_a_manual_one() {
        assert_eq!(stage_label(PlanStage::Buying, false), " · Buying");
        assert_eq!(stage_label(PlanStage::Done, true), " · Done (you)");
        assert_eq!(stage_label(PlanStage::Sold, true), " · Sold (you)");
    }

    #[test]
    fn the_menu_lists_every_stage() {
        let labels: Vec<&str> = STAGE_MENU.iter().map(|(label, _)| *label).collect();
        assert_eq!(
            labels,
            ["Automatic", "Planned", "Buying", "Building", "Done", "Sold"]
        );
    }

    #[test]
    fn the_menu_marks_automatic_or_the_override_only() {
        assert!(stage_marked(None, None));
        assert!(!stage_marked(None, Some(PlanStage::Planned)));
        assert!(stage_marked(Some(PlanStage::Done), Some(PlanStage::Done)));
        assert!(!stage_marked(Some(PlanStage::Done), None));
    }
}
