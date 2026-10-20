//! The Plans panel: the saved plans in the order the store lists them, the buttons that make, copy,
//! rename and delete them, and the dialogs those ask through.

use egui::{Align2, Button, Context, RichText, TextEdit, Ui, Window};

use super::state::{BuildPage, Dialog, Next, report};
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
    let plans = match cx.store.plans() {
        Ok(plans) => plans,
        Err(error) => {
            ui.label(
                RichText::new(format!("Could not read the plans: {error}")).color(cx.style.neg),
            );
            return;
        }
    };
    let selected = page.selected.is_some();
    let (mut new, mut duplicate, mut rename, mut delete) = (false, false, false, false);
    ui.horizontal(|ui| {
        new = ui.button("New").clicked();
        rename = ui.add_enabled(selected, Button::new("Rename")).clicked();
        duplicate = ui.add_enabled(selected, Button::new("Duplicate")).clicked();
        delete = ui.add_enabled(selected, Button::new("Delete")).clicked();
    });

    let mut clicked = None;
    for plan in &plans {
        if ui
            .selectable_label(page.selected == Some(plan.id), plan.name.as_str())
            .clicked()
        {
            clicked = Some(plan.id);
        }
    }

    if let Some(id) = clicked {
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
                Ok(()) if page.selected == Some(id) => page.clear(),
                Ok(()) => {}
                Err(error) => report(cx, format!("Could not delete the plan: {error}")),
            }
            None
        }
        Some(_) => None,
    }
}
