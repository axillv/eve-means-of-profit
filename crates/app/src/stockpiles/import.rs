//! jEve's stockpiles imported in one go: the file picker that starts in jEve's folder, the store
//! writes, and the window that lists what came in and what was left out.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use egui::{Button, Ui};
use mop_core::stockpile::{Include, Stockpile};
use mop_jeve::{ImportedStockpile, LeftOut, StockpileImport, profiles_dir, read_stockpiles_file};
use mop_sde::Sde;
use mop_store::Store;

use crate::Shared;
use crate::build::state::push_error;
use crate::status::Notice;

/// The import's state: the open file picker, the worker's answer, and the result window.
#[derive(Default)]
pub(crate) struct State {
    picker: Option<Picker>,
    pending: Option<Receiver<Result<Outcome, String>>>,
    result: Option<Outcome>,
}

impl fmt::Debug for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("State")
            .field("picker", &self.picker)
            .field("pending", &self.pending.is_some())
            .field("result", &self.result)
            .finish()
    }
}

#[derive(Debug, Clone)]
struct Picker {
    dir: PathBuf,
    selected: Option<PathBuf>,
}

/// What the result window shows.
#[derive(Debug, Clone)]
struct Outcome {
    imported: usize,
    notes: Vec<(String, Vec<String>)>,
    stopped: Option<String>,
}

impl State {
    pub(crate) fn start(&mut self) {
        self.picker = Some(Picker {
            dir: start_dir(),
            selected: None,
        });
    }

    pub(crate) fn show(&mut self, ui: &mut Ui, cx: &mut Shared) {
        self.drain(cx);
        self.picker_window(ui, cx);
        self.result_window(ui);
    }

    /// Takes the worker's answer when it has arrived.
    fn drain(&mut self, cx: &mut Shared) {
        let received = match self.pending.as_ref() {
            Some(receiver) => receiver.try_recv(),
            None => return,
        };
        match received {
            Ok(Ok(outcome)) => {
                self.pending = None;
                if let Some(text) = &outcome.stopped {
                    push_error(&mut cx.notices, text.clone());
                }
                self.result = Some(outcome);
            }
            Ok(Err(text)) => {
                self.pending = None;
                cx.notices.push(Notice::error(text));
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.pending = None;
                push_error(
                    &mut cx.notices,
                    "The jEve import stopped unexpectedly.".to_owned(),
                );
            }
        }
    }

    fn picker_window(&mut self, ui: &mut Ui, cx: &mut Shared) {
        let Some(picker) = self.picker.as_mut() else {
            return;
        };
        let mut open = true;
        let mut chosen: Option<PathBuf> = None;
        let mut cancelled = false;
        egui::Window::new("Import stockpiles from jEve")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .show(ui.ctx(), |ui| {
                ui.label(picker.dir.display().to_string());
                ui.horizontal(|ui| {
                    if ui.button("Up").clicked()
                        && let Some(parent) = picker.dir.parent()
                    {
                        picker.dir = parent.to_path_buf();
                        picker.selected = None;
                    }
                });
                egui::ScrollArea::vertical()
                    .max_height(240.0)
                    .show(ui, |ui| {
                        for (dir, file) in entries(&picker.dir) {
                            match file {
                                Some(path) => {
                                    if ui
                                        .selectable_label(
                                            picker.selected.as_ref() == Some(&path),
                                            path.file_name()
                                                .and_then(|name| name.to_str())
                                                .unwrap_or_default(),
                                        )
                                        .clicked()
                                    {
                                        picker.selected = Some(path);
                                    }
                                }
                                None => {
                                    let name = dir
                                        .file_name()
                                        .and_then(|name| name.to_str())
                                        .unwrap_or_default();
                                    if ui.button(format!("{name}/")).clicked() {
                                        picker.dir = dir;
                                        picker.selected = None;
                                    }
                                }
                            }
                        }
                    });
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(picker.selected.is_some(), Button::new("Import"))
                        .clicked()
                    {
                        chosen = picker.selected.clone();
                    }
                    if ui.button("Cancel").clicked() {
                        cancelled = true;
                    }
                });
            });
        if let Some(path) = chosen {
            self.picker = None;
            self.begin_import(path, cx);
        } else if cancelled || !open {
            self.picker = None;
        }
    }

    /// Reads the file on the worker, then writes what it holds to the store there too.
    fn begin_import(&mut self, path: PathBuf, cx: &mut Shared) {
        let existing = match existing_names(cx) {
            Ok(names) => names,
            Err(text) => {
                cx.notices.push(Notice::error(text));
                return;
            }
        };
        let sde_path = cx.data_dir.join("sde.sqlite");
        let user_db = cx.data_dir.join("user.sqlite");
        let now = (cx.clock)();
        let (tx, receiver) = mpsc::channel();
        let job = move || {
            let result = import_on_worker(&path, &existing, &sde_path, &user_db, now);
            drop(tx.send(result));
        };
        if cx.worker.run(Box::new(job)) {
            self.pending = Some(receiver);
        } else {
            push_error(
                &mut cx.notices,
                "The jEve import could not start.".to_owned(),
            );
        }
    }

    fn result_window(&mut self, ui: &mut Ui) {
        let Some(result) = self.result.as_ref() else {
            return;
        };
        let mut open = true;
        let mut close = false;
        egui::Window::new("Import from jEve")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .show(ui.ctx(), |ui| {
                let text = if result.imported == 1 {
                    "Imported 1 stockpile".to_owned()
                } else {
                    format!("Imported {} stockpiles", result.imported)
                };
                ui.strong(text);
                for (name, notes) in &result.notes {
                    ui.strong(name.as_str());
                    for note in notes {
                        ui.label(note.as_str());
                    }
                }
                if let Some(stopped) = &result.stopped {
                    ui.colored_label(crate::theme::Style::eve_dense().neg, stopped.as_str());
                }
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        if !open || close {
            self.result = None;
        }
    }
}

/// The directory the picker starts in: jEve's assets folder, the parent of its profiles.
fn start_dir() -> PathBuf {
    profiles_dir()
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .or_else(profiles_dir)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default()
}

/// The subfolders and files of `dir`, folders first and each sorted ignoring case.
fn entries(dir: &Path) -> Vec<(PathBuf, Option<PathBuf>)> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            dirs.push((path, None));
        } else {
            files.push((path.clone(), Some(path)));
        }
    }
    dirs.sort_by_cached_key(|(path, _)| path_key(path));
    files.sort_by_cached_key(|(path, _)| path_key(path));
    dirs.extend(files);
    dirs
}

fn path_key(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_lowercase()
}

/// Imports the stockpiles in `path`. A read that fails leaves the store alone and reports why.
pub fn import_file(path: &Path, cx: &mut Shared) -> bool {
    let existing = match existing_names(cx) {
        Ok(names) => names,
        Err(text) => {
            cx.notices.push(Notice::error(text));
            return false;
        }
    };
    let sde_path = cx.data_dir.join("sde.sqlite");
    let user_db = cx.data_dir.join("user.sqlite");
    let now = (cx.clock)();
    match import_on_worker(path, &existing, &sde_path, &user_db, now) {
        Ok(outcome) => {
            if let Some(text) = &outcome.stopped {
                push_error(&mut cx.notices, text.clone());
            }
            true
        }
        Err(text) => {
            cx.notices.push(Notice::error(text));
            false
        }
    }
}

/// The names the store already has, for a new name that is taken.
fn existing_names(cx: &mut Shared) -> Result<Vec<String>, String> {
    match cx.store.stockpiles() {
        Ok(list) => Ok(list
            .into_iter()
            .map(|stockpile| stockpile.name)
            .collect::<Vec<String>>()),
        Err(error) => Err(format!("Could not read the stockpiles: {error}")),
    }
}

/// Reads the file and writes it to the store. Runs on the worker: the game data and the user
/// database are opened there, so a large file never holds up a frame.
fn import_on_worker(
    path: &Path,
    existing: &[String],
    sde_path: &Path,
    user_db: &Path,
    now: i64,
) -> Result<Outcome, String> {
    let sde =
        Sde::open(sde_path).map_err(|error| format!("Could not read the game data: {error}"))?;
    let type_name = |id| match sde.item(id) {
        Ok(item) => item.map(|item| item.name),
        Err(_) => None,
    };
    let import =
        read_stockpiles_file(path, existing, &type_name).map_err(|error| error.to_string())?;
    let mut store = Store::open(user_db, now)
        .map_err(|error| format!("Could not open the store: {error}"))?
        .store;
    Ok(write_import(&import, &mut store))
}

/// Creates every stockpile, then saves each one with its parts and mapped includes. A store error
/// stops the import; the stockpiles already saved stay and are counted.
fn write_import(import: &StockpileImport, store: &mut Store) -> Outcome {
    let mut ids: BTreeMap<String, i64> = BTreeMap::new();
    let mut stopped = None;
    for stockpile in &import.stockpiles {
        match store.create_stockpile(&stockpile.name, stockpile.group.as_deref()) {
            Ok(id) => {
                ids.insert(stockpile.name.clone(), id);
            }
            Err(error) => {
                stopped = Some(format!("Import stopped at {}: {error}", stockpile.name));
                break;
            }
        }
    }
    let mut saved = Vec::new();
    if stopped.is_none() {
        for stockpile in &import.stockpiles {
            let Some(&id) = ids.get(&stockpile.name) else {
                break;
            };
            let record = saved_stockpile(stockpile, id, &ids);
            match store.save_stockpile(&record) {
                Ok(()) => saved.push(stockpile.name.clone()),
                Err(error) => {
                    stopped = Some(format!("Import stopped at {}: {error}", stockpile.name));
                    break;
                }
            }
        }
    }
    let notes = saved
        .iter()
        .filter_map(|name| {
            let lines = notes_for(&import.left_out, name);
            (!lines.is_empty()).then(|| (name.clone(), lines))
        })
        .collect();
    Outcome {
        imported: saved.len(),
        notes,
        stopped,
    }
}

fn saved_stockpile(
    imported: &ImportedStockpile,
    id: i64,
    ids: &BTreeMap<String, i64>,
) -> Stockpile {
    let includes = imported
        .includes
        .iter()
        .filter_map(|(name, count)| {
            ids.get(name).map(|&stockpile_id| Include {
                stockpile_id,
                count: *count,
            })
        })
        .collect();
    Stockpile {
        id,
        name: imported.name.clone(),
        group: imported.group.clone(),
        multiplier: imported.multiplier,
        targets: imported.targets.clone(),
        includes,
        filters: imported.filters.clone(),
    }
}

fn notes_for(left_out: &[LeftOut], name: &str) -> Vec<String> {
    left_out
        .iter()
        .filter(|left| left.stockpile == name)
        .map(|left| left.what.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn imported(name: &str) -> ImportedStockpile {
        ImportedStockpile {
            name: name.to_owned(),
            group: None,
            multiplier: 1,
            targets: Vec::new(),
            includes: Vec::new(),
            filters: Vec::new(),
        }
    }

    #[test]
    fn includes_are_mapped_by_name_to_the_new_ids() {
        let ids = BTreeMap::from([("Kit".to_owned(), 4_i64)]);
        let mut stockpile = imported("Weekly");
        stockpile.includes = vec![("Kit".to_owned(), 3), ("Missing".to_owned(), 1)];
        let saved = saved_stockpile(&stockpile, 9, &ids);
        assert_eq!(
            saved.includes,
            vec![Include {
                stockpile_id: 4,
                count: 3
            }]
        );
    }

    #[test]
    fn a_save_that_stops_counts_only_the_stockpiles_already_saved() {
        let dir = tempfile::TempDir::new().expect("a temp folder");
        let mut store = Store::open(&dir.path().join("user.sqlite"), 0)
            .expect("the store opens")
            .store;
        let mut a = imported("A");
        a.includes = vec![("B".to_owned(), 1)];
        let mut b = imported("B");
        b.includes = vec![("A".to_owned(), 1)];
        let import = StockpileImport {
            stockpiles: vec![a, b],
            left_out: Vec::new(),
        };

        let outcome = write_import(&import, &mut store);

        assert_eq!(outcome.imported, 1, "B was created but never saved");
        assert!(
            outcome
                .stopped
                .as_deref()
                .is_some_and(|text| text.starts_with("Import stopped at B:")),
            "{:?}",
            outcome.stopped
        );
    }

    #[test]
    fn notes_are_kept_for_the_stockpile_they_belong_to() {
        let left_out = vec![
            LeftOut {
                stockpile: "Kit".to_owned(),
                what: "one".to_owned(),
            },
            LeftOut {
                stockpile: "Weekly".to_owned(),
                what: "two".to_owned(),
            },
        ];
        assert_eq!(notes_for(&left_out, "Kit"), ["one"]);
        assert!(notes_for(&left_out, "Half").is_empty());
    }
}
