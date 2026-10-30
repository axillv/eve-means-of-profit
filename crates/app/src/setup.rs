//! The Setup page: the facilities, freight services and default price rule the plans pick from. The
//! dock draws each panel's header, and [`show`] draws the body. The read of a character's skills and
//! standings that the panels share lives here.

mod facilities;
mod fees;
mod fields;
mod freight;
pub mod history;
mod invention;
mod price_rule;
mod structures;
pub mod value_prices;

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};

use egui::{Button, ComboBox, Context, Ui};

use mop_esi::error::EsiError;
use mop_esi::skills::{CharacterSkills, NpcStanding};
use mop_store::StoreError;

use self::fees::read_error;
use crate::Shared;
use crate::session::lock_eve;
use crate::settings_window;
use crate::status::Notice;

const NO_CHARACTER: &str = "No character logged in";
const READING: &str = "Reading…";

/// What one read answers: the character's skills and standings, or why they could not be read.
pub(super) type ReadResult = Result<(CharacterSkills, Vec<NpcStanding>), EsiError>;
/// The channel a read's answer comes back on.
pub(super) type ReadChannel = Arc<Mutex<Receiver<ReadResult>>>;

/// The read a panel waits for: the character's name and the channel the worker answers on.
#[derive(Debug, Clone)]
pub(super) struct Reading {
    name: String,
    result: ReadChannel,
}

/// A good read's answer: the character it read, and the skills and standings it found.
pub(super) struct ReadAnswer {
    pub(super) name: String,
    pub(super) skills: CharacterSkills,
    pub(super) standings: Vec<NpcStanding>,
}

#[derive(Debug, Default)]
pub struct SetupPage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupPanel {
    Facilities,
    FreightServices,
    PriceHistory,
    PriceRule,
    Structures,
    Fees,
    Invention,
    Display,
    ValuePrices,
}

impl SetupPanel {
    pub fn title(self) -> &'static str {
        match self {
            Self::Facilities => "Facilities",
            Self::FreightServices => "Freight services",
            Self::PriceHistory => "Price history",
            Self::PriceRule => "Price rule",
            Self::Structures => "Structures",
            Self::Fees => "Fees and skills",
            Self::Invention => "Invention skills",
            Self::Display => "Display",
            Self::ValuePrices => "Value prices",
        }
    }
}

/// The text a store error shows under a Setup panel, for the errors the panels share.
pub(super) fn store_text(error: &StoreError) -> String {
    if matches!(error, StoreError::RouteExists) {
        "This route already exists.".to_owned()
    } else if let StoreError::InUse { plans } = error {
        format!("In use by: {}", plans.join(", "))
    } else {
        error.to_string()
    }
}

/// The first row of a panel that reads skills: pick a logged-in character and read them.
pub(super) fn read_row(
    character: &mut Option<u64>,
    reading: &mut Option<Reading>,
    ui: &mut Ui,
    cx: &mut Shared,
    id_salt: &str,
) {
    let characters: Vec<(u64, String)> = cx
        .session
        .characters()
        .iter()
        .map(|character| (character.id, character.name.clone()))
        .collect();
    let known = character.is_some_and(|id| characters.iter().any(|(held, _)| *held == id));
    if !known {
        *character = characters.first().map(|(id, _)| *id);
    }
    ui.horizontal(|ui| {
        ui.label("Read from character");
        if characters.is_empty() {
            ui.add_enabled_ui(false, |ui| {
                ComboBox::from_id_salt(id_salt)
                    .selected_text(NO_CHARACTER)
                    .show_ui(ui, |_ui| {});
            });
            ui.add_enabled(false, Button::new("Read"));
        } else {
            let selected = character
                .and_then(|id| characters.iter().find(|(held, _)| *held == id))
                .map_or_else(|| NO_CHARACTER.to_owned(), |(_, name)| name.clone());
            ComboBox::from_id_salt(id_salt)
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    for (id, name) in &characters {
                        ui.selectable_value(character, Some(*id), name);
                    }
                });
            let busy = reading.is_some();
            let button = ui.add_enabled(!busy, Button::new(if busy { READING } else { "Read" }));
            if button.clicked() {
                start_read(*character, reading, cx, ui.ctx());
            }
        }
    });
}

/// Asks the worker for one character's skills and standings. The answer comes back on a channel
/// the panel keeps, and is taken in by [`take_read`].
fn start_read(
    character: Option<u64>,
    reading: &mut Option<Reading>,
    cx: &mut Shared,
    ctx: &Context,
) {
    let Some(id) = character else {
        return;
    };
    let Some(name) = cx
        .session
        .characters()
        .iter()
        .find(|character| character.id == id)
        .map(|character| character.name.clone())
    else {
        return;
    };
    let eve = cx.session.eve();
    let now = (cx.clock)();
    let (tx, rx) = mpsc::channel();
    *reading = Some(Reading {
        name,
        result: Arc::new(Mutex::new(rx)),
    });
    let ctx = ctx.clone();
    let job = move || {
        let result = lock_eve(&eve).read_skills_and_standings(id, now);
        drop(tx.send(result));
        ctx.request_repaint();
    };
    if !cx.worker.run(Box::new(job)) {
        *reading = None;
        cx.notices.push(Notice::error(
            "The background worker has stopped. Restart the app.",
        ));
    }
}

/// Takes in the answer of a read, when it has come back. A good answer is returned; a failure
/// shows its notice and leaves the panel's fields as they were.
pub(super) fn take_read(reading: &mut Option<Reading>, cx: &mut Shared) -> Option<ReadAnswer> {
    let held = reading.as_ref()?;
    let received = held
        .result
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .try_recv();
    match received {
        Ok(result) => {
            let name = held.name.clone();
            *reading = None;
            match result {
                Ok((skills, standings)) => Some(ReadAnswer {
                    name,
                    skills,
                    standings,
                }),
                Err(error) => {
                    cx.notices.push(Notice::error(read_error(&name, &error)));
                    None
                }
            }
        }
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => {
            let name = held.name.clone();
            *reading = None;
            cx.notices.push(Notice::error(format!(
                "Could not read {name}: the read stopped unexpectedly."
            )));
            None
        }
    }
}

/// Draws the body of one Setup panel.
pub fn show(_page: &mut SetupPage, panel: SetupPanel, ui: &mut Ui, cx: &mut Shared) {
    match panel {
        SetupPanel::Facilities => facilities::show(ui, cx),
        SetupPanel::FreightServices => freight::show(ui, cx),
        SetupPanel::PriceHistory => history::show(ui, cx),
        SetupPanel::PriceRule => price_rule::show(ui, cx),
        SetupPanel::Structures => structures::show(ui, cx),
        SetupPanel::Fees => fees::show(ui, cx),
        SetupPanel::Invention => invention::show(ui, cx),
        SetupPanel::ValuePrices => value_prices::show(ui, cx),
        SetupPanel::Display => {
            let data_dir = cx.data_dir.clone();
            egui::ScrollArea::vertical().show(ui, |ui| {
                settings_window::show(ui, cx, &data_dir);
            });
        }
    }
}
