//! The Characters panel: the login characters, the account each one belongs to, how old each kind
//! of their data is, and the buttons that fetch, log in again and remove.

use egui::{Button, Color32, ComboBox, Id, Label, RichText, TextEdit, Ui};
use mop_esi::character_data::{CharacterData, Fetched, Kind};
use mop_esi::characters::Login;
use mop_store::StoreError;
use mop_store::net_worth::Account;

use super::Inner;
use crate::Shared;
use crate::build::prices::age_text;
use crate::session::eve::NO_CLIENT_ID;
use crate::status::Notice;
use crate::table::Table;
use crate::table::model::{Cell, Column, ColumnKind, Row};
use crate::theme::Style;

/// One kind's column: the age text, its colour, and the hover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KindView {
    pub text: String,
    /// Amber once EVE allows a refresh, the error colour when the next refresh cannot be worked
    /// out, the text colour otherwise.
    pub color: Color32,
    pub hover: String,
}

/// The column for data fetched at the time given (`None` when never) with the time EVE lets the
/// player refresh it again, or why that time is not known.
pub fn kind_view(held: Option<(i64, Result<i64, String>)>, now: i64, style: &Style) -> KindView {
    let Some((fetched_at, next_refresh)) = held else {
        return KindView {
            text: "never".to_owned(),
            color: style.text,
            hover: "Can be refreshed now".to_owned(),
        };
    };
    // A floor at 0 is meant here: a fetch time after `now` (clock skew) shows as age zero, not a negative age.
    let text = age_text(now.saturating_sub(fetched_at));
    match next_refresh {
        Err(error) => KindView {
            text,
            color: style.neg,
            hover: format!("Could not work out the next refresh: {error}"),
        },
        Ok(next) if now < next => KindView {
            text,
            color: style.text,
            hover: format!(
                "Next refresh allowed in {}",
                // Cannot saturate: the guard above makes now < next, and both are Unix seconds, so the gap is far below i64::MAX.
                age_text(next.saturating_sub(now))
            ),
        },
        Ok(_) => KindView {
            text,
            color: style.stale,
            hover: "Can be refreshed now".to_owned(),
        },
    }
}

/// When EVE gives no expiry time, a refresh is allowed this long after the fetch. These match the
/// defaults in the esi crate (`Kind::default_age` in `character_data.rs`, private there).
fn default_age(kind: Kind) -> i64 {
    match kind {
        Kind::Assets => 3_600,
        Kind::Orders => 1_200,
        Kind::Jobs => 300,
        Kind::Wallet => 120,
    }
}

fn view_of<T>(held: Option<&Fetched<T>>, kind: Kind, now: i64, style: &Style) -> KindView {
    kind_view(
        held.map(|fetched| {
            let next = fetched
                .next_refresh(default_age(kind))
                .map_err(|error| error.to_string());
            (fetched.fetched_at, next)
        }),
        now,
        style,
    )
}

/// The four kind columns of one character's data, in column order.
fn kinds(data: Option<&CharacterData>, now: i64, style: &Style) -> [KindView; 4] {
    [
        view_of(
            data.and_then(|held| held.assets.as_ref()),
            Kind::Assets,
            now,
            style,
        ),
        view_of(
            data.and_then(|held| held.orders.as_ref()),
            Kind::Orders,
            now,
            style,
        ),
        view_of(
            data.and_then(|held| held.jobs.as_ref()),
            Kind::Jobs,
            now,
            style,
        ),
        view_of(
            data.and_then(|held| held.wallet.as_ref()),
            Kind::Wallet,
            now,
            style,
        ),
    ]
}

/// The login column: its text and colour.
pub fn login_view(expired: bool, style: &Style) -> (&'static str, Color32) {
    if expired {
        ("login expired: log in again", style.neg)
    } else {
        ("ok", style.text)
    }
}

/// What the page knows that the panel draws.
pub(super) struct View<'a> {
    busy: Option<&'a str>,
    waiting: bool,
    confirm: Option<&'a str>,
    keychain: Option<&'a str>,
    login_possible: bool,
}

impl<'a> View<'a> {
    pub(super) fn new(inner: &'a Inner, login_possible: bool) -> Self {
        Self {
            busy: inner.busy.as_deref(),
            waiting: inner.waiting.is_some(),
            confirm: inner.confirm.as_ref().map(|(_, name)| name.as_str()),
            keychain: inner.keychain.as_deref(),
            login_possible,
        }
    }
}

/// What the player pressed.
#[derive(Debug, Clone, Copy)]
pub(super) enum Click {
    Add,
    RefreshAll,
    Refresh(u64),
    Remove(u64),
    ConfirmRemove,
    Keep,
    Cancel,
}

/// A row of the table, worked out before the table draws it.
struct Line {
    id: u64,
    name: String,
    expired: bool,
    kinds: [KindView; 4],
}

fn columns() -> Vec<Column> {
    let column = |id: &'static str, label: &str, width: f32| Column {
        id,
        label: label.to_owned(),
        kind: ColumnKind::Text,
        width,
    };
    vec![
        column("character", "Character", 180.0),
        column("login", "Login", 210.0),
        column("account", "Account", 150.0),
        column("assets", "Assets", 80.0),
        column("orders", "Orders", 80.0),
        column("jobs", "Jobs", 80.0),
        column("wallet", "Wallet", 80.0),
        column("actions", "", 300.0),
    ]
}

/// A small button that can be disabled with a reason shown on hover. Returns whether it was clicked.
fn gated(ui: &mut Ui, label: &str, reason: Option<&str>) -> bool {
    let response = ui.add_enabled(reason.is_none(), Button::new(label).small());
    match reason {
        Some(text) => response.on_disabled_hover_text(text).clicked(),
        None => response.clicked(),
    }
}

/// Why a button is off, shown on hover: what runs, or that this build has no login.
struct Gates {
    busy: Option<String>,
    login: Option<String>,
}

pub(super) fn show(ui: &mut Ui, cx: &mut Shared, view: &View) -> Option<Click> {
    let style = cx.style;
    let busy = view.busy.map(|what| format!("Busy: {what}"));
    let login = busy
        .clone()
        .or_else(|| (!view.login_possible).then(|| NO_CLIENT_ID.to_owned()));
    let gates = Gates { busy, login };
    let mut click = top(ui, view, &gates);
    let accounts = match cx.store.accounts() {
        Ok(held) => Some(held),
        Err(error) => {
            ui.colored_label(style.neg, format!("Could not read the accounts: {error}"));
            None
        }
    };
    if accounts.is_some() {
        new_account(ui, cx);
    }
    if let Some(text) = view.keychain {
        ui.label(RichText::new(text).color(style.mute));
    }
    if let Some(name) = view.confirm {
        click = confirm(ui, name).or(click);
    }
    let now = (cx.clock)();
    let mut lines: Vec<Line> = cx
        .session
        .characters()
        .iter()
        .map(|character| Line {
            id: character.id,
            name: character.name.clone(),
            expired: character.login == Login::Expired,
            kinds: kinds(cx.session.character_data(character.id), now, &style),
        })
        .collect();
    if lines.is_empty() {
        ui.label(
            RichText::new("No characters yet. Add character opens the EVE login in your browser.")
                .color(style.mute),
        );
        return click;
    }
    lines.sort_by_key(|line| (line.name.to_lowercase(), line.name.clone()));
    table(ui, cx, &lines, accounts.as_deref(), &gates).or(click)
}

/// The account holding the character, when the accounts are read and one of them holds it.
fn held_account(accounts: &[Account], character: u64) -> Option<&Account> {
    accounts
        .iter()
        .find(|account| account.characters.contains(&character))
}

/// The `New account` field and the `Add account` button, under the buttons. The name being typed
/// lives in the frame memory, so it survives the frames; a name the store stores empties the
/// field, and one it refuses becomes an ERROR notice.
fn new_account(ui: &mut Ui, cx: &mut Shared) {
    let mut name = ui
        .data(|data| data.get_temp::<String>(new_account_key()))
        .unwrap_or_default();
    let mut add = false;
    ui.horizontal(|ui| {
        let caption = ui.label("New account");
        ui.add(TextEdit::singleline(&mut name).desired_width(220.0))
            .labelled_by(caption.id);
        add = ui.button("Add account").clicked();
    });
    if add {
        match cx.store.add_account(&name) {
            Ok(_) => name.clear(),
            Err(error) => cx.notices.push(Notice::error(add_error(&error))),
        }
    }
    ui.data_mut(|data| data.insert_temp(new_account_key(), name));
}

fn new_account_key() -> Id {
    Id::new("characters new account")
}

/// The buttons above the table, and the wait for the browser.
fn top(ui: &mut Ui, view: &View, gates: &Gates) -> Option<Click> {
    let mut click = None;
    ui.horizontal(|ui| {
        if gated(ui, "Add character", gates.login.as_deref()) {
            click = Some(Click::Add);
        }
        if gated(ui, "Refresh all", gates.busy.as_deref()) {
            click = Some(Click::RefreshAll);
        }
    });
    if view.waiting {
        ui.horizontal(|ui| {
            ui.label("Waiting for the EVE login in your browser…");
            if ui.button("Cancel").clicked() {
                click = Some(Click::Cancel);
            }
        });
    }
    click
}

fn confirm(ui: &mut Ui, name: &str) -> Option<Click> {
    let mut click = None;
    ui.horizontal(|ui| {
        ui.label(format!(
            "Remove {name}? Its saved data is deleted. Net worth history is kept."
        ));
        if ui.button("Remove").clicked() {
            click = Some(Click::ConfirmRemove);
        }
        if ui.button("Keep").clicked() {
            click = Some(Click::Keep);
        }
    });
    click
}

fn table(
    ui: &mut Ui,
    cx: &mut Shared,
    lines: &[Line],
    accounts: Option<&[Account]>,
    gates: &Gates,
) -> Option<Click> {
    let style = cx.style;
    let density = cx.density;
    let mut click = None;
    // What the player picked in an Account dropdown, applied once the table is drawn.
    let mut picks: Vec<(u64, Option<i64>)> = Vec::new();
    let rows: Vec<Row> = lines
        .iter()
        .map(|line| {
            let mut cells = vec![
                Cell::text(line.name.as_str()),
                Cell::text(login_view(line.expired, &style).0),
                Cell::text(account_text(accounts, line.id)),
            ];
            cells.extend(line.kinds.iter().map(|kind| Cell::text(kind.text.as_str())));
            cells.push(Cell::text(""));
            Row {
                key: line.id,
                depth: 0,
                cells,
            }
        })
        .collect();
    let columns = columns();
    let mut draw = |ui: &mut Ui, key: u64, column: &'static str| -> bool {
        let Some(line) = lines.iter().find(|line| line.id == key) else {
            return false;
        };
        match column {
            "login" => {
                let (text, color) = login_view(line.expired, &style);
                let text = RichText::new(text).color(color);
                ui.add(Label::new(text).selectable(false));
                true
            }
            "account" => {
                let Some(accounts) = accounts else {
                    return false;
                };
                account_cell(ui, accounts, key, &mut picks);
                true
            }
            "assets" | "orders" | "jobs" | "wallet" => {
                let position = KIND_COLUMNS.iter().position(|id| *id == column);
                let Some(kind) = position.and_then(|at| line.kinds.get(at)) else {
                    return false;
                };
                let text = RichText::new(kind.text.as_str()).color(kind.color);
                ui.add(Label::new(text).selectable(false))
                    .on_hover_text(kind.hover.as_str());
                true
            }
            "actions" => {
                if line.expired && gated(ui, "Log in again", gates.login.as_deref()) {
                    click = Some(Click::Add);
                }
                if gated(ui, "Refresh", gates.busy.as_deref()) {
                    click = Some(Click::Refresh(line.id));
                }
                if gated(ui, "Remove…", gates.busy.as_deref()) {
                    click = Some(Click::Remove(line.id));
                }
                true
            }
            _ => false,
        }
    };
    Table::new(
        Id::new(super::CharactersPanel::Characters.title()),
        &columns,
    )
    .show_with(ui, &style, density, &rows, None, &mut draw);
    for (character, account) in picks {
        if let Err(error) = cx.store.set_account(character, account) {
            cx.notices.push(Notice::error(pick_error(&error)));
        }
    }
    click
}

/// Draws the Account cell of one character: a dropdown of `(none)` and every account, with the
/// accounts that hold three other characters off. What the player picks is pushed into `picks`,
/// to be stored once the table is drawn.
fn account_cell(ui: &mut Ui, accounts: &[Account], key: u64, picks: &mut Vec<(u64, Option<i64>)>) {
    let held = held_account(accounts, key);
    let mut picked = None;
    ComboBox::from_id_salt(("characters account", key))
        .selected_text(held.map_or(NO_ACCOUNT, |account| account.name.as_str()))
        .show_ui(ui, |ui| {
            if ui.selectable_label(held.is_none(), NO_ACCOUNT).clicked() {
                picked = Some(None);
            }
            for account in accounts {
                let others = account
                    .characters
                    .iter()
                    .filter(|&&character| character != key)
                    .count();
                let response = ui.add_enabled(
                    others < ACCOUNT_SIZE,
                    Button::selectable(
                        held.is_some_and(|held| held.id == account.id),
                        account.name.as_str(),
                    ),
                );
                if others >= ACCOUNT_SIZE {
                    response.on_disabled_hover_text(ACCOUNT_FULL);
                } else if response.clicked() {
                    picked = Some(Some(account.id));
                }
            }
        });
    if let Some(account) = picked {
        picks.push((key, account));
    }
}

/// What the Account cell shows and copies: the account's name, `(none)` when the character is in
/// no account, and nothing when the accounts could not be read.
fn account_text(accounts: Option<&[Account]>, character: u64) -> String {
    let Some(accounts) = accounts else {
        return String::new();
    };
    held_account(accounts, character)
        .map_or_else(|| NO_ACCOUNT.to_owned(), |account| account.name.clone())
}

/// What an error from adding an account says to the player.
fn add_error(error: &StoreError) -> String {
    if let StoreError::EmptyName = error {
        return "Type a name for the account.".to_owned();
    }
    if let StoreError::NameTaken(name) = error {
        return format!("An account called \"{name}\" already exists.");
    }
    format!("Could not add the account: {error}")
}

/// What an error from putting a character in an account says to the player.
fn pick_error(error: &StoreError) -> String {
    if let StoreError::TooLarge = error {
        return ACCOUNT_FULL.to_owned();
    }
    format!("Could not save the account: {error}")
}

const KIND_COLUMNS: [&str; 4] = ["assets", "orders", "jobs", "wallet"];

/// What an Account cell says when the character belongs to no account, and the dropdown's entry
/// for it.
const NO_ACCOUNT: &str = "(none)";

/// Why an account's dropdown entry is off: it holds three other characters already.
const ACCOUNT_FULL: &str = "This account already has 3 characters.";

/// The most characters one EVE account holds.
const ACCOUNT_SIZE: usize = 3;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_of_an_account_error_says_what_happened() {
        assert_eq!(
            pick_error(&StoreError::TooLarge),
            "This account already has 3 characters."
        );
        assert_eq!(
            pick_error(&StoreError::NotFound),
            "Could not save the account: not found"
        );
        assert_eq!(
            add_error(&StoreError::EmptyName),
            "Type a name for the account."
        );
        assert_eq!(
            add_error(&StoreError::NameTaken("Main acct".to_owned())),
            "An account called \"Main acct\" already exists."
        );
    }
}
