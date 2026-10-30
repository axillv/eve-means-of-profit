//! The status line at the bottom of the window: the activity or idle text, then the notices.
//!
//! Activity (a background update) shows first. Otherwise the first notice shows, with
//! ` (1 of <n>)` when there are several. With no notice, the idle text shows, followed by each
//! login character's login state or data age.

use egui::{Align, Layout, Ui};
use mop_esi::character_data::CharacterData;
use mop_esi::characters::Login;

use crate::build::prices::age_text;
use crate::session::Session;
use crate::theme::Style;

pub const IDLE_TEXT: &str = "No market data yet";
/// Longer notices are cut to this many characters; the full text shows on hover.
const CUT_AT: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Notice,
    Error,
}

/// A button on the status line. A notice's own button also removes the notice; `Refresh all` does
/// not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Downloads and imports build `build` in the background.
    Update { build: u64 },
    /// Refreshes every login character and The Forge prices.
    RefreshAll,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub level: Level,
    pub text: String,
    pub action: Option<Action>,
}

impl Notice {
    pub fn info(text: impl Into<String>) -> Self {
        Self {
            level: Level::Notice,
            text: text.into(),
            action: None,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            level: Level::Error,
            text: text.into(),
            action: None,
        }
    }

    pub fn with_action(mut self, action: Action) -> Self {
        self.action = Some(action);
        self
    }
}

/// Draws the status line. Returns the action whose button was pressed, if any.
pub fn show(
    ui: &mut Ui,
    style: &Style,
    notices: &mut Vec<Notice>,
    activity: Option<&str>,
    idle: &str,
    characters: &str,
) -> Option<Action> {
    let mut action = None;
    let mut notice_action = false;
    let mut dismiss = false;
    egui::Panel::bottom("status").show(ui, |ui| {
        ui.horizontal(|ui| {
            if let Some(text) = activity {
                ui.label(text);
            } else if let Some(first) = notices.first() {
                let (label, color) = match first.level {
                    Level::Notice => ("NOTICE", style.mute),
                    Level::Error => ("ERROR", style.neg),
                };
                ui.colored_label(color, label);
                let count = if notices.len() > 1 {
                    format!(" (1 of {})", notices.len())
                } else {
                    String::new()
                };
                ui.label(format!("{}{count}", cut(&first.text)))
                    .on_hover_text(first.text.as_str());
                if let Some(pending) = first.action
                    && ui.button(button_text(pending)).clicked()
                {
                    action = Some(pending);
                    notice_action = true;
                }
                if ui.button("Dismiss").clicked() {
                    dismiss = true;
                }
            } else {
                ui.label(idle);
                if !characters.is_empty() {
                    ui.label("·");
                    ui.label(characters);
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Refresh all").clicked() {
                    action = Some(Action::RefreshAll);
                }
            });
        });
    });
    if (dismiss || notice_action) && !notices.is_empty() {
        notices.remove(0);
    }
    action
}

/// The login characters' line: `<name>: login expired` for an expired login, otherwise
/// `<name>: <age>` when its data holds any kind. Empty when there is nothing to say.
pub(crate) fn characters_text(session: &Session, now: i64) -> String {
    session
        .characters()
        .iter()
        .filter_map(|character| match character.login {
            Login::Expired => Some(format!("{}: login expired", character.name)),
            Login::Ok => session
                .character_data(character.id)
                .and_then(oldest_of)
                .map(|fetched_at| {
                    format!(
                        "{}: {}",
                        character.name,
                        age_text(now.saturating_sub(fetched_at))
                    )
                }),
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The oldest fetch time among one character's kinds of data.
fn oldest_of(held: &CharacterData) -> Option<i64> {
    let assets = held.assets.as_ref().map(|fetched| fetched.fetched_at);
    let orders = held.orders.as_ref().map(|fetched| fetched.fetched_at);
    let jobs = held.jobs.as_ref().map(|fetched| fetched.fetched_at);
    let wallet = held.wallet.as_ref().map(|fetched| fetched.fetched_at);
    [assets, orders, jobs, wallet].into_iter().flatten().min()
}

fn button_text(action: Action) -> &'static str {
    match action {
        Action::Update { .. } => "Update",
        Action::RefreshAll => "Refresh all",
    }
}

/// The text cut to [`CUT_AT`] characters, with `…` when it was cut.
fn cut(text: &str) -> String {
    if text.chars().count() <= CUT_AT {
        return text.to_owned();
    }
    let head: String = text.chars().take(CUT_AT).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use mop_core::money::Money;
    use mop_esi::character_data::{CharacterData, Fetched};
    use mop_esi::characters::{Character, Login};
    use mop_esi::places::StructureNames;

    use super::characters_text;
    use crate::session::eve::NoEve;
    use crate::session::{Saved, Session};

    const NOW: i64 = 1_000_000;

    fn character(id: u64, name: &str, login: Login) -> Character {
        Character {
            id,
            name: name.to_owned(),
            scopes: BTreeSet::new(),
            added_at: 0,
            login,
        }
    }

    fn fetched<T>(data: T, fetched_at: i64) -> Fetched<T> {
        Fetched {
            data,
            fetched_at,
            last_modified: None,
            expires: None,
        }
    }

    fn wallet_data(character_id: u64, fetched_at: i64) -> CharacterData {
        let mut data = CharacterData::new(character_id);
        data.wallet = Some(fetched(Money::ZERO, fetched_at));
        data
    }

    fn session(characters: Vec<Character>, data: Vec<CharacterData>) -> Session {
        let mut session = Session::new(Box::new(NoEve));
        session.set_saved(Saved {
            characters,
            data,
            structures: StructureNames::new(),
        });
        session
    }

    #[test]
    fn an_ok_login_shows_its_age_and_an_expired_one_says_so() {
        let session = session(
            vec![
                character(1, "Main", Login::Ok),
                character(2, "Alt", Login::Expired),
            ],
            vec![wallet_data(1, NOW - 7_200)],
        );
        assert_eq!(
            characters_text(&session, NOW),
            "Main: 2h · Alt: login expired"
        );
    }

    #[test]
    fn the_oldest_kind_decides_the_age() {
        let mut data = wallet_data(1, NOW - 10_800);
        data.orders = Some(fetched(Vec::new(), NOW - 3_600));
        let session = session(vec![character(1, "Main", Login::Ok)], vec![data]);
        assert_eq!(characters_text(&session, NOW), "Main: 3h");
    }

    #[test]
    fn an_expired_login_without_data_says_so_and_an_ok_one_without_data_is_left_out() {
        let session = session(
            vec![
                character(1, "Alt", Login::Expired),
                character(2, "Third", Login::Ok),
            ],
            Vec::new(),
        );
        assert_eq!(characters_text(&session, NOW), "Alt: login expired");
    }

    #[test]
    fn no_characters_says_nothing_and_a_future_fetch_is_zero() {
        assert_eq!(characters_text(&session(Vec::new(), Vec::new()), NOW), "");
        let session = session(
            vec![character(1, "Main", Login::Ok)],
            vec![wallet_data(1, NOW + 60)],
        );
        assert_eq!(characters_text(&session, NOW), "Main: 0s");
    }
}
