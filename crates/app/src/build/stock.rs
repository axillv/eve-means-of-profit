//! The Pasted stock panel: the text of what the player owns, with the lines that could not be read
//! and the stock the plan did not use.

use egui::{RichText, TextEdit, Ui};

use super::state::{BuildPage, type_name};
use crate::Shared;

pub fn show(page: &mut BuildPage, ui: &mut Ui, cx: &mut Shared) {
    if page.record.is_none() {
        ui.label(RichText::new("No plan selected.").color(cx.style.mute));
        return;
    }
    let mut changed = false;
    if let Some(record) = page.record.as_mut() {
        let text = TextEdit::multiline(&mut record.pasted_stock)
            .desired_width(f32::INFINITY)
            .desired_rows(12)
            .hint_text("Paste what you own");
        changed = ui.add(text).changed();
    }
    if changed {
        page.edited(cx);
    }
    problems(page, ui, cx);
    unused(page, ui, cx);
}

/// `<n> line(s) not read: <line numbers>`, with each line and its reason on hover.
fn problems(page: &BuildPage, ui: &mut Ui, cx: &Shared) {
    let problems = &page.pasted.problems;
    if problems.is_empty() {
        return;
    }
    let numbers: Vec<String> = problems
        .iter()
        .map(|problem| problem.line.to_string())
        .collect();
    let noun = if problems.len() == 1 { "line" } else { "lines" };
    let text = format!("{} {noun} not read: {}", problems.len(), numbers.join(", "));
    let hover: Vec<String> = problems
        .iter()
        .map(|problem| format!("Line {}: {} ({})", problem.line, problem.text, problem.kind))
        .collect();
    ui.label(RichText::new(text).color(cx.style.neg))
        .on_hover_text(hover.join("\n"));
}

/// `<n> not used`, with each unused type and its units on hover. Only once a plan can be worked
/// out: there is nothing to compare the stock with otherwise.
fn unused(page: &BuildPage, ui: &mut Ui, cx: &mut Shared) {
    let Ok(plan) = page.result.as_ref() else {
        return;
    };
    if plan.unused_stock.is_empty() {
        return;
    }
    let hover: Vec<String> = plan
        .unused_stock
        .iter()
        .map(|(id, units)| {
            format!(
                "{} {units}",
                type_name(*id, cx.sde.as_ref(), &mut cx.notices)
            )
        })
        .collect();
    let mute = cx.style.mute;
    ui.label(RichText::new(format!("{} not used", plan.unused_stock.len())).color(mute))
        .on_hover_text(hover.join("\n"));
}
