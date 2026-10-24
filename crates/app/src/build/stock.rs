//! The Pasted stock panel: the text of what the player owns, with the lines that could not be read
//! and the stock the plan did not use.

use egui::{RichText, TextEdit, Ui};

use mop_store::reservations::PlanStock;

use super::state::{BuildPage, type_name};
use crate::Shared;

pub fn show(page: &mut BuildPage, ui: &mut Ui, cx: &mut Shared) {
    if page.record.is_none() {
        ui.label(RichText::new("No plan selected.").color(cx.style.mute));
        return;
    }
    let from_characters = matches!(page.stock, PlanStock::Characters(_));
    if from_characters {
        let line = "This plan uses stock from characters. The pasted text is kept but not used.";
        ui.label(RichText::new(line).color(cx.style.mute));
    }
    let mut changed = false;
    if let Some(record) = page.record.as_mut() {
        let text = TextEdit::multiline(&mut record.pasted_stock)
            .desired_width(f32::INFINITY)
            .desired_rows(12)
            .hint_text("Paste what you own")
            .interactive(!from_characters);
        changed = ui.add(text).changed();
    }
    if changed {
        page.edited(cx);
    }
    if from_characters {
        return;
    }
    problems(page, ui, cx);
    unused(page, ui, cx);
}

/// The most lines either list shows before it says how many more there are.
const SHOWN: usize = 6;

/// Each line that could not be read, with the line and its reason: `Line 2: "No Such Thing 4" (unknown item)`.
fn problems(page: &BuildPage, ui: &mut Ui, cx: &Shared) {
    let problems = &page.pasted.problems;
    if problems.is_empty() {
        return;
    }
    let neg = cx.style.neg;
    ui.label(RichText::new("Left out, could not be read:").color(neg));
    for problem in problems.iter().take(SHOWN) {
        ui.label(
            RichText::new(format!(
                "Line {}: \"{}\" ({})",
                problem.line, problem.text, problem.kind
            ))
            .color(neg),
        );
    }
    more(ui, problems.len(), cx.style.mute);
}

/// The stock the plan does not use, one item and its units per line. Only once a plan can be
/// worked out: there is nothing to compare the stock with otherwise.
fn unused(page: &BuildPage, ui: &mut Ui, cx: &mut Shared) {
    let Ok(plan) = page.result.as_ref() else {
        return;
    };
    if plan.unused_stock.is_empty() {
        return;
    }
    let mute = cx.style.mute;
    ui.label(RichText::new("Pasted, but this plan does not use:").color(mute));
    for (id, units) in plan.unused_stock.iter().take(SHOWN) {
        let name = type_name(*id, cx.sde.as_ref(), &mut cx.notices);
        ui.label(RichText::new(format!("{name} {units}")).color(mute));
    }
    more(ui, plan.unused_stock.len(), mute);
}

/// `and <n> more`, when a list was cut at [`SHOWN`].
fn more(ui: &mut Ui, total: usize, color: egui::Color32) {
    if let Some(rest) = total.checked_sub(SHOWN).filter(|rest| *rest > 0) {
        ui.label(RichText::new(format!("and {rest} more")).color(color));
    }
}
