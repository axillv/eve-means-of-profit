//! The jEve data panel: which characters jEve's file holds and whether the app uses them.

use std::path::{Path, PathBuf};

use egui::{ComboBox, Context, Id, RichText, Ui};
use mop_jeve::{ReadOptions, list_profiles, read_profile};

use super::{Inner, Msg, queue, send};
use crate::Shared;
use crate::build::prices::age_text;
use crate::session::{JEVE_PROFILE, Source, lock_eve};
use crate::status::Notice;
use crate::table::Table;
use crate::table::model::{Cell, Column, ColumnKind, Row};

/// The setting that keeps the profile the player picked.
const SETTING: &str = "jeve.profile";

/// What the worker found in jEve's folder.
#[derive(Debug, Clone)]
pub(super) struct Listing {
    /// The profiles folder, when there is one to look in.
    folder: Option<PathBuf>,
    /// The profile names, the active one first.
    names: Vec<String>,
    /// The profile in use.
    chosen: Option<String>,
    /// Its file.
    file: Option<PathBuf>,
}

#[derive(Debug, Default)]
pub(super) struct State {
    asked: bool,
    listing: Option<Listing>,
}

impl State {
    pub(super) fn listed(&mut self, listing: Listing) {
        self.listing = Some(listing);
    }
}

/// When the job reads the profile it picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Read {
    /// Always: the player asked.
    Always,
    /// Only when the pick is not the profile the app read at start.
    UnlessDefault,
}

fn saved_choice(cx: &mut Shared) -> Option<String> {
    match cx.store.setting(SETTING) {
        Ok(name) => name,
        Err(error) => {
            cx.notices.push(Notice::error(format!(
                "Could not read the settings: {error}"
            )));
            None
        }
    }
}

/// Looks in jEve's folder on the worker, then reads the profile picked when `read` says so.
fn start(inner: &mut Inner, cx: &mut Shared, ctx: &Context, choice: Option<String>, read: Read) {
    let eve = cx.session.eve();
    let tx = inner.tx.clone();
    let ctx = ctx.clone();
    // Once the folder is known the job needs no login, so it never waits behind one.
    let known = inner
        .jeve
        .listing
        .as_ref()
        .map(|listing| listing.folder.clone());
    queue(inner, cx, move || {
        let folder = match known {
            Some(folder) => folder,
            None => lock_eve(&eve).jeve_folder(),
        };
        let (listing, result) = look(folder, choice.as_deref(), read);
        send(&tx, &ctx, Msg::JeveListed(listing));
        if let Some(result) = result {
            send(&tx, &ctx, Msg::JeveRead(result));
        }
    });
}

fn look(
    folder: Option<PathBuf>,
    choice: Option<&str>,
    read: Read,
) -> (Listing, Option<Result<Option<mop_jeve::JeveData>, String>>) {
    let Some(dir) = folder.clone() else {
        let listing = Listing {
            folder,
            names: Vec::new(),
            chosen: None,
            file: None,
        };
        return (listing, (read == Read::Always).then_some(Ok(None)));
    };
    let profiles = match list_profiles(&dir) {
        Ok(profiles) => profiles,
        Err(error) => {
            let listing = Listing {
                folder,
                names: Vec::new(),
                chosen: None,
                file: None,
            };
            return (listing, Some(Err(error.to_string())));
        }
    };
    // The active profile comes first, so a saved name no longer found falls back to it.
    let picked = choice
        .and_then(|name| profiles.iter().find(|profile| profile.name == name))
        .or_else(|| profiles.first());
    let listing = Listing {
        folder,
        names: profiles
            .iter()
            .map(|profile| profile.name.clone())
            .collect(),
        chosen: picked.map(|profile| profile.name.clone()),
        file: picked.map(|profile| profile.path.clone()),
    };
    let wanted = match read {
        Read::Always => true,
        Read::UnlessDefault => picked.is_some_and(|profile| {
            profile.path.file_name().and_then(|name| name.to_str()) != Some(JEVE_PROFILE)
        }),
    };
    if !wanted {
        return (listing, None);
    }
    let result = match picked {
        Some(profile) => read_file(&profile.path),
        None => Ok(None),
    };
    (listing, Some(result))
}

fn read_file(path: &Path) -> Result<Option<mop_jeve::JeveData>, String> {
    if !path.is_file() {
        return Ok(None);
    }
    read_profile(path, &ReadOptions::default())
        .map(Some)
        .map_err(|error| error.to_string())
}

pub(super) fn show(ui: &mut Ui, cx: &mut Shared, inner: &mut Inner, ctx: &Context) {
    if !inner.jeve.asked {
        inner.jeve.asked = true;
        let choice = saved_choice(cx);
        start(inner, cx, ctx, choice, Read::UnlessDefault);
    }
    let Some(listing) = inner.jeve.listing.clone() else {
        return;
    };
    let (picked, again) = controls(ui, cx, &listing);
    let choice = picked.clone().or_else(|| listing.chosen.clone());
    if let Some(name) = picked {
        if let Err(error) = cx.store.set_setting(SETTING, &name) {
            cx.notices.push(Notice::error(format!(
                "Could not save the settings: {error}"
            )));
        }
        start(inner, cx, ctx, choice, Read::Always);
    } else if again {
        start(inner, cx, ctx, choice, Read::Always);
    }
    if cx.session.jeve().is_none() {
        let text = match &listing.folder {
            Some(folder) => format!("No jEve data file found at {}.", folder.display()),
            None => "No jEve data file found.".to_owned(),
        };
        ui.label(RichText::new(text).color(cx.style.mute));
        return;
    }
    table(ui, cx);
}

/// The file, the profile dropdown and `Read again`. Returns the profile picked and whether
/// `Read again` was pressed.
fn controls(ui: &mut Ui, cx: &Shared, listing: &Listing) -> (Option<String>, bool) {
    let mut picked: Option<String> = None;
    let mut again = false;
    ui.horizontal(|ui| {
        if cx.session.jeve().is_some()
            && let Some(file) = &listing.file
        {
            ui.label(format!("File: {}", file.display()));
        }
        if listing.names.len() > 1 {
            let shown = listing.chosen.clone().unwrap_or_default();
            ComboBox::from_id_salt("jeve-profile")
                .selected_text(shown.as_str())
                .show_ui(ui, |ui| {
                    for name in &listing.names {
                        if ui.selectable_label(shown == *name, name.as_str()).clicked() {
                            picked = Some(name.clone());
                        }
                    }
                });
        }
        again = ui.button("Read again").clicked();
    });
    (picked, again)
}

fn table(ui: &mut Ui, cx: &Shared) {
    let Some(jeve) = cx.session.jeve() else {
        return;
    };
    let style = cx.style;
    let now = (cx.clock)();
    let columns = [
        column("character", "Character", 200.0),
        column("updated", "Assets updated", 140.0),
        column("used", "Used", 360.0),
    ];
    let rows: Vec<Row> = jeve
        .characters
        .iter()
        .enumerate()
        .map(|(position, held)| {
            // A floor at 0 is meant here: an assets fetch time after `now` (clock set back) shows as age zero.
            let updated = held
                .assets_updated
                .map_or_else(|| "never".to_owned(), |at| age_text(now.saturating_sub(at)));
            let by_login = u64::try_from(held.character_id)
                .ok()
                .and_then(|id| cx.session.source(id))
                == Some(Source::Login);
            let mut used = if by_login {
                "no: logged in to the app".to_owned()
            } else {
                "yes".to_owned()
            };
            if held.login_expired {
                used.push_str(" · login expired in jEve");
            }
            Row {
                key: u64::try_from(position).unwrap_or(u64::MAX),
                depth: 0,
                cells: vec![
                    Cell::text(held.name.as_str()),
                    Cell::text(updated),
                    Cell::text(used),
                ],
            }
        })
        .collect();
    Table::new(Id::new(super::CharactersPanel::JeveData.title()), &columns)
        .fit_rows(20)
        .show(ui, &style, cx.density, &rows, None);
    let left_out = jeve.skipped_corporations;
    if left_out > 0 {
        let word = if left_out == 1 {
            "corporation"
        } else {
            "corporations"
        };
        ui.label(RichText::new(format!("{left_out} {word} left out")).color(style.mute));
    }
}

fn column(id: &'static str, label: &str, width: f32) -> Column {
    Column {
        id,
        label: label.to_owned(),
        kind: ColumnKind::Text,
        width,
    }
}
