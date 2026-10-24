//! The `Name this structure` window: names a structure that EVE gives no name for, or clears the name
//! the player typed so EVE's name shows again.

use egui::{Context, Key, Modifiers, TextEdit, Window};
use mop_esi::places;

use crate::{Notice, Shared};

/// The window's state: the structure, the text being typed, and the error of the last save.
#[derive(Debug, Clone)]
pub(super) struct Dialog {
    id: i64,
    text: String,
    error: Option<String>,
}

impl Dialog {
    pub(super) fn new(id: i64, text: String) -> Self {
        Self {
            id,
            text,
            error: None,
        }
    }
}

/// Draws the window while one is open. Save stores the name and reloads the names the session shows;
/// a save that fails keeps the window open with the error under the field. Cancel and Esc close it.
pub(super) fn show(dialog: &mut Option<Dialog>, ctx: &Context, cx: &mut Shared) {
    let Some(state) = dialog.as_mut() else {
        return;
    };
    let eve_name = places::place_label(state.id, None, None, cx.session.structures());
    let mut save = false;
    let mut cancel = ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
    Window::new("Name this structure")
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.add(TextEdit::singleline(&mut state.text).desired_width(280.0));
            ui.colored_label(cx.style.mute, format!("EVE's name: {eve_name}"));
            if let Some(error) = &state.error {
                ui.colored_label(cx.style.neg, error.as_str());
            }
            ui.horizontal(|ui| {
                save = ui.button("Save").clicked();
                cancel |= ui.button("Cancel").clicked();
            });
        });
    if cancel {
        *dialog = None;
    } else if save {
        save_name(dialog, cx);
    }
}

/// Saves the typed name, then reloads the names. The window closes on success.
fn save_name(dialog: &mut Option<Dialog>, cx: &mut Shared) {
    let Some(state) = dialog.as_mut() else {
        return;
    };
    let now = (cx.clock)();
    if let Err(error) = cx.store.set_structure_name(state.id, &state.text, now) {
        state.error = Some(error.to_string());
        return;
    }
    match cx.store.structure_names() {
        Ok(typed) => cx.session.set_typed_names(typed),
        Err(error) => cx.notices.push(Notice::error(format!(
            "Could not reload the structure names: {error}"
        ))),
    }
    *dialog = None;
}
