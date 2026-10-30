//! Short names for the login characters: what the player calls each character in every table,
//! with the full EVE name kept on hover. A short name is saved per character under
//! `character.short.<id>`; an empty one clears it.

use std::collections::{BTreeMap, BTreeSet};

use egui::{Context, Id, Label, Ui};

use crate::Shared;
use crate::status::Notice;

/// The saved short names, in the egui context under one id: what each character is called, which
/// characters have been read from the store, and whether a read ever failed (so the failure is
/// said once, not every frame).
#[derive(Debug, Default, Clone)]
struct Cache {
    names: BTreeMap<u64, String>,
    read: BTreeSet<u64>,
    read_failed: bool,
}

fn cache_id() -> Id {
    Id::new("character short names")
}

fn with_cache<T>(ctx: &Context, use_cache: impl FnOnce(&mut Cache) -> T) -> T {
    ctx.data_mut(|data| use_cache(data.get_temp_mut_or_default::<Cache>(cache_id())))
}

/// The settings key the short name of a character is saved under.
fn key(id: u64) -> String {
    format!("character.short.{id}")
}

/// The saved short name of a character, or `full` when the player gave none. The store is read
/// once per character and the answer kept in the context; a read that fails draws `full` and is
/// one notice: `Could not read the short names: {error}`.
pub fn current(ctx: &Context, cx: &mut Shared, id: u64, full: &str) -> String {
    with_cache(ctx, |cache| {
        if !cache.read.contains(&id) {
            match cx.store.setting(&key(id)) {
                Ok(Some(name)) => {
                    cache.names.insert(id, name);
                    cache.read.insert(id);
                }
                Ok(None) => {
                    cache.read.insert(id);
                }
                Err(error) => {
                    // The id stays unread, so a store that recovers is read again; the flag
                    // keeps the failure to one notice for the whole run.
                    if !cache.read_failed {
                        cache.read_failed = true;
                        cx.notices.push(Notice::error(format!(
                            "Could not read the short names: {error}"
                        )));
                    }
                }
            }
        }
        match cache.names.get(&id) {
            Some(name) => name.clone(),
            None => full.to_owned(),
        }
    })
}

/// Saves `text` as the character's short name: trimmed, an empty one clears the key, and one
/// longer than 24 characters is refused with a notice and writes nothing. Returns whether the
/// store now holds what was asked for; a save that fails says so once per attempt.
pub fn set(ctx: &Context, cx: &mut Shared, id: u64, text: &str) -> bool {
    let name = text.trim();
    if name.chars().count() > MAX_LEN {
        cx.notices.push(Notice::error(format!(
            "A short name can have at most {MAX_LEN} characters."
        )));
        return false;
    }
    let saved = if name.is_empty() {
        cx.store.clear_setting(&key(id))
    } else {
        cx.store.set_setting(&key(id), name)
    };
    match saved {
        Ok(()) => {
            with_cache(ctx, |cache| {
                cache.read.insert(id);
                if name.is_empty() {
                    cache.names.remove(&id);
                } else {
                    cache.names.insert(id, name.to_owned());
                }
            });
            true
        }
        Err(error) => {
            cx.notices.push(Notice::error(format!(
                "Could not save the short name: {error}"
            )));
            false
        }
    }
}

/// Draws `full`, or the short name saved for the login character of that exact name, with `full`
/// on hover only when the two differ. A text that names no login character draws `full`. Returns
/// that it drew the cell.
pub fn label(ui: &mut Ui, cx: &mut Shared, full: &str) -> bool {
    let id = cx
        .session
        .characters()
        .iter()
        .find(|character| character.name == full)
        .map(|character| character.id);
    let shown = match id {
        Some(id) => current(ui.ctx(), cx, id, full),
        None => full.to_owned(),
    };
    let response = ui.add(Label::new(shown.as_str()).selectable(false));
    if shown != full {
        response.on_hover_text(full);
    }
    true
}

/// The most characters a short name may have.
const MAX_LEN: usize = 24;
