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

use crate::Shared;
use crate::session::lock_eve;
use crate::settings_window;
use crate::status::Notice;

const NO_CHARACTER: &str = "No character logged in";
const READING: &str = "Reading…";
const REFUSED: &str = "EVE refused. Log in again and allow skills and standings.";

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

/// The notice for a read that failed. EVE's refusal tells the player to log in again and allow
/// skills and standings; anything else shows the error itself.
fn read_error(name: &str, error: &EsiError) -> String {
    if matches!(error, EsiError::Server(403) | EsiError::Denied(_)) {
        format!("Could not read {name}: {REFUSED}")
    } else {
        format!("Could not read {name}: {error}")
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::Write;
    use std::ops::ControlFlow;
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};

    use mop_esi::error::EsiError;
    use mop_esi::industry::IndustryData;
    use mop_esi::skills::{CharacterSkills, NpcStanding, StandingFrom};
    use mop_market::error::MarketError;
    use mop_market::fuzzwork::{BytesHttp, BytesResponse};
    use mop_sde::{Fetch, FetchError};
    use mop_store::{Density, Store};
    use tempfile::TempDir;

    use super::{ReadResult, Reading, read_error, take_read};
    use crate::Shared;
    use crate::build::prices::Prices;
    use crate::claims::ClaimsState;
    use crate::selection::Selection;
    use crate::session::{NoEve, Session};
    use crate::status::Level;
    use crate::theme::Style;
    use crate::worker::Worker;

    const NOW: i64 = 1_000_000;

    /// The network, never reached: these tests never start a read.
    struct NoFetch;

    impl Fetch for NoFetch {
        fn get_text(&self, _url: &str) -> Result<String, FetchError> {
            Err(FetchError::Http { status: 503 })
        }

        fn download(
            &self,
            _url: &str,
            _dest: &mut dyn Write,
            _progress: &mut dyn FnMut(u64, Option<u64>) -> ControlFlow<()>,
        ) -> Result<u64, FetchError> {
            Err(FetchError::Http { status: 503 })
        }
    }

    /// A market that never answers: no price is read in these tests.
    struct NoHttp;

    impl BytesHttp for NoHttp {
        fn get_bytes(
            &self,
            _url: &str,
            _if_none_match: Option<&str>,
        ) -> Result<BytesResponse, MarketError> {
            Err(MarketError::Network("not in these tests".to_owned()))
        }
    }

    /// An empty user store with everything a panel reads around it. The clock is fixed at [`NOW`].
    fn shared(dir: &TempDir) -> Shared {
        Shared {
            sde: None,
            store: Store::open(&dir.path().join("user.sqlite"), NOW)
                .expect("the store opens")
                .store,
            worker: Worker::spawn(egui::Context::default(), Box::new(NoFetch)),
            market: Arc::new(Prices::new(Box::new(NoHttp))),
            plex_http: Arc::new(mop_market::http::UreqHttp::new()),
            copy_notices: Arc::new(Mutex::new(Vec::new())),
            style: Style::eve_dense(),
            density: Density::Normal,
            rail_icons_only: false,
            refresh_while_open: false,
            data_dir: dir.path().to_path_buf(),
            notices: Vec::new(),
            clock: Box::new(|| NOW),
            session: Session::new(Box::new(NoEve)),
            selection: Selection::default(),
            industry: IndustryData::default(),
            claims: ClaimsState::default(),
        }
    }

    /// `rx` as the read Ana Tester is waiting on.
    fn waiting_on(rx: mpsc::Receiver<ReadResult>) -> Reading {
        Reading {
            name: "Ana Tester".to_owned(),
            result: Arc::new(Mutex::new(rx)),
        }
    }

    #[test]
    fn a_read_waiting_for_its_answer_stays_waiting() {
        let dir = TempDir::new().expect("a temporary directory");
        let mut cx = shared(&dir);
        let (_tx, rx) = mpsc::channel();
        let mut reading = Some(waiting_on(rx));

        assert!(take_read(&mut reading, &mut cx).is_none());
        assert!(reading.is_some());
        assert!(cx.notices.is_empty());
    }

    #[test]
    fn a_good_answer_ends_the_read_and_is_returned() {
        let dir = TempDir::new().expect("a temporary directory");
        let mut cx = shared(&dir);
        let skills = CharacterSkills {
            levels: BTreeMap::new(),
            total_sp: 4_000_000,
        };
        let standings = vec![NpcStanding {
            from_id: 500_001,
            from: StandingFrom::Faction,
            hundredths: -250,
        }];
        let (tx, rx) = mpsc::channel();
        let mut reading = Some(waiting_on(rx));
        tx.send(Ok((skills.clone(), standings.clone())))
            .expect("the answer is sent");

        let answer = take_read(&mut reading, &mut cx).expect("the answer is returned");
        assert_eq!(answer.name, "Ana Tester");
        assert_eq!(answer.skills, skills);
        assert_eq!(answer.standings, standings);
        assert!(reading.is_none());
        assert!(cx.notices.is_empty());
    }

    #[test]
    fn a_refused_answer_clears_the_read_and_notifies() {
        let dir = TempDir::new().expect("a temporary directory");
        let mut cx = shared(&dir);
        let (tx, rx) = mpsc::channel();
        let mut reading = Some(waiting_on(rx));
        tx.send(Err(EsiError::Server(403)))
            .expect("the refusal is sent");

        assert!(take_read(&mut reading, &mut cx).is_none());
        assert!(reading.is_none());
        assert_eq!(cx.notices.len(), 1);
        assert_eq!(cx.notices[0].level, Level::Error);
        assert_eq!(
            cx.notices[0].text,
            "Could not read Ana Tester: EVE refused. Log in again and allow skills and standings."
        );
    }

    #[test]
    fn a_dropped_channel_ends_the_read_with_a_notice() {
        let dir = TempDir::new().expect("a temporary directory");
        let mut cx = shared(&dir);
        let (tx, rx) = mpsc::channel::<ReadResult>();
        drop(tx);
        let mut reading = Some(waiting_on(rx));

        assert!(take_read(&mut reading, &mut cx).is_none());
        assert!(reading.is_none());
        assert_eq!(cx.notices.len(), 1);
        assert_eq!(cx.notices[0].level, Level::Error);
        assert_eq!(
            cx.notices[0].text,
            "Could not read Ana Tester: the read stopped unexpectedly."
        );
    }

    #[test]
    fn a_refused_read_says_to_log_in_again() {
        assert_eq!(
            read_error("Ana Tester", &EsiError::Server(403)),
            "Could not read Ana Tester: EVE refused. Log in again and allow skills and standings."
        );
        assert_eq!(
            read_error("Ana Tester", &EsiError::Denied("no scope".to_owned())),
            "Could not read Ana Tester: EVE refused. Log in again and allow skills and standings."
        );
    }

    #[test]
    fn another_read_error_shows_the_error_itself() {
        let error = EsiError::Network("timed out".to_owned());
        assert_eq!(
            read_error("Ana Tester", &error),
            format!("Could not read Ana Tester: {error}")
        );
    }
}
