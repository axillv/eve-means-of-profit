//! The Characters panel: the login characters, the account each one belongs to, how old each kind
//! of their data is, and the buttons that fetch, log in again and remove.

use egui::{Button, Color32, ComboBox, Id, Label, RichText, TextEdit, Ui};
use mop_esi::character_data::{CharacterData, Fetched, Kind};
use mop_esi::characters::Login;
use mop_store::net_worth::Account;
use mop_store::{Density, StoreError};

use super::Inner;
use crate::Shared;
use crate::build::prices::age_text;
use crate::images::{self, Picture};
use crate::session::eve::NO_CLIENT_ID;
use crate::status::Notice;
use crate::table::Table;
use crate::table::model::{Cell, Column, ColumnKind, Row, row_height};
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
    } else {
        lines.sort_by_key(|line| (line.name.to_lowercase(), line.name.clone()));
        click = table(ui, cx, &lines, accounts.as_deref(), &gates).or(click);
    }
    if let Some(accounts) = accounts.as_deref() {
        account_blocks(ui, cx, accounts, now);
    }
    click
}

/// The accounts under the table: their characters, the rename field and the PLEX count for a date.
fn account_blocks(ui: &mut Ui, cx: &mut Shared, accounts: &[Account], now: i64) {
    let style = cx.style;
    let names: Vec<(u64, String)> = cx
        .session
        .characters()
        .iter()
        .map(|character| (character.id, character.name.clone()))
        .collect();
    for account in accounts {
        ui.separator();
        let members = member_text(account, &names);
        ui.horizontal(|ui| {
            ui.label(RichText::new(account.name.as_str()).strong());
            ui.label(RichText::new(members).color(style.mute));
        });
        rename_line(ui, cx, account);
        plex_line(ui, cx, account, now);
    }
}

/// The account's characters by name, or `no characters` when it holds none. A character the
/// session does not know is shown by its id.
fn member_text(account: &Account, names: &[(u64, String)]) -> String {
    if account.characters.is_empty() {
        return "no characters".to_owned();
    }
    let mut members: Vec<String> = account
        .characters
        .iter()
        .map(|id| {
            names
                .iter()
                .find(|(known, _)| known == id)
                .map_or_else(|| format!("Character {id}"), |(_, name)| name.clone())
        })
        .collect();
    members.sort_by_key(|name| (name.to_lowercase(), name.clone()));
    members.join(", ")
}

/// The `Rename <name>` field and button of one account. The name being typed lives in the frame
/// memory under the account's id, so it survives the frames; a name the store keeps drops the
/// buffer, and one it refuses becomes an ERROR notice.
fn rename_line(ui: &mut Ui, cx: &mut Shared, account: &Account) {
    let key = rename_key(account.id);
    let mut name = ui
        .data(|data| data.get_temp::<String>(key))
        .unwrap_or_else(|| account.name.clone());
    let mut rename = false;
    ui.horizontal(|ui| {
        let caption = ui.label(format!("Rename {}", account.name));
        ui.add(TextEdit::singleline(&mut name).desired_width(220.0))
            .labelled_by(caption.id);
        rename = ui.button("Rename").clicked();
    });
    if rename {
        match cx.store.rename_account(account.id, &name) {
            Ok(()) => ui.data_mut(|data| data.remove::<String>(key)),
            Err(error) => {
                cx.notices.push(Notice::error(rename_error(&error)));
                ui.data_mut(|data| data.insert_temp(key, name));
            }
        }
    } else {
        ui.data_mut(|data| data.insert_temp(key, name));
    }
}

/// The `PLEX in <name>` and `Date (UTC)` fields with the `Set PLEX` button of one account, and the
/// newest count's age. A good count empties the count field and puts the date back to today.
fn plex_line(ui: &mut Ui, cx: &mut Shared, account: &Account, now: i64) {
    let style = cx.style;
    let today = now.div_euclid(SECONDS_PER_DAY);
    let count_key = plex_count_key(account.id);
    let date_key = plex_date_key(account.id);
    let mut count = ui
        .data(|data| data.get_temp::<String>(count_key))
        .unwrap_or_default();
    let mut date = ui
        .data(|data| data.get_temp::<String>(date_key))
        .unwrap_or_else(|| date_text(today));
    ui.horizontal(|ui| {
        let caption = ui.label(format!("PLEX in {}", account.name));
        ui.add(TextEdit::singleline(&mut count).desired_width(90.0))
            .labelled_by(caption.id);
        let caption = ui.label("Date (UTC)");
        ui.add(TextEdit::singleline(&mut date).desired_width(110.0))
            .labelled_by(caption.id);
        if ui.button("Set PLEX").clicked() {
            match save_plex(cx, account.id, &count, &date, today, now) {
                Ok(()) => {
                    count.clear();
                    date = date_text(today);
                }
                Err(error) => cx.notices.push(Notice::error(error)),
            }
        }
        let latest = latest_plex(cx, account.id, now);
        ui.label(RichText::new(latest).color(style.mute));
    });
    ui.data_mut(|data| {
        data.insert_temp(count_key, count);
        data.insert_temp(date_key, date);
    });
}

/// Reads the count and date the player typed and stores the count for that day. The error text is
/// what the player sees.
fn save_plex(
    cx: &mut Shared,
    account: i64,
    count: &str,
    date: &str,
    today: i64,
    now: i64,
) -> Result<(), String> {
    let count = parse_plex(count)?;
    let date = date.trim();
    let day = if date.is_empty() {
        today
    } else {
        parse_date(date).ok_or_else(|| DATE_ERROR.to_owned())?
    };
    if day > today {
        return Err("The date cannot be in the future.".to_owned());
    }
    let at = if day == today {
        now
    } else {
        day.checked_mul(SECONDS_PER_DAY)
            .ok_or_else(|| DATE_ERROR.to_owned())?
    };
    cx.store
        .set_plex(account, count, at)
        .map_err(|error| format!("Could not save the PLEX count: {error}"))
}

/// The number of PLEX the player typed: commas are allowed and the rest must be a whole number.
fn parse_plex(text: &str) -> Result<u64, String> {
    let cleaned = text.replace(',', "");
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return Err("Type the number of PLEX.".to_owned());
    }
    match trimmed.parse::<u64>() {
        Ok(count) => Ok(count),
        Err(_) => Err("Type a whole number of PLEX.".to_owned()),
    }
}

/// The newest stored count of the account and how long ago it was set.
fn latest_plex(cx: &Shared, account: i64, now: i64) -> String {
    match cx.store.plex_counts(account) {
        Ok(counts) => match counts.last() {
            Some((at, count)) => format!(
                "{} PLEX, set {} ago",
                plex_text(*count),
                age_text(now.saturating_sub(*at))
            ),
            None => "no PLEX count yet".to_owned(),
        },
        Err(error) => format!("Could not read the PLEX counts: {error}"),
    }
}

fn rename_key(account: i64) -> Id {
    Id::new(("characters rename account", account))
}

fn plex_count_key(account: i64) -> Id {
    Id::new(("characters plex count", account))
}

fn plex_date_key(account: i64) -> Id {
    Id::new(("characters plex date", account))
}

/// The count with its thousands grouped by commas.
pub fn plex_text(count: u64) -> String {
    let digits = count.to_string();
    let mut text = String::with_capacity(digits.len() + digits.len() / 3);
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at).is_multiple_of(3) {
            text.push(',');
        }
        text.push(digit);
    }
    text
}

/// The days since 1970-01-01 that `YYYY-MM-DD` names, or `None` when it is not a real date.
pub fn parse_date(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() != 10 {
        return None;
    }
    if bytes.get(4) != Some(&b'-') || bytes.get(7) != Some(&b'-') {
        return None;
    }
    let year = digits(bytes.get(0..4)?)?;
    let month = digits(bytes.get(5..7)?)?;
    let day = digits(bytes.get(8..10)?)?;
    if !(1..=12).contains(&month) || !(1..=days_in_month(year, month)).contains(&day) {
        return None;
    }
    days_from_civil(year, month, day)
}

fn digits(bytes: &[u8]) -> Option<i64> {
    let mut value = 0_i64;
    for byte in bytes {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add(i64::from(byte - b'0'))?;
    }
    Some(value)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 1970-01-01 to the given civil date (proleptic Gregorian calendar).
fn days_from_civil(year: i64, month: i64, day: i64) -> Option<i64> {
    let year = if month <= 2 {
        year.checked_sub(1)?
    } else {
        year
    };
    let era = year.div_euclid(400);
    let year_of_era = year.checked_sub(era.checked_mul(400)?)?;
    let shifted_month = (month + 9) % 12;
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era
        .checked_mul(365)?
        .checked_add(year_of_era / 4)?
        .checked_sub(year_of_era / 100)?
        .checked_add(day_of_year)?;
    era.checked_mul(146_097)?
        .checked_add(day_of_era)?
        .checked_sub(719_468)
}

/// The day number as `YYYY-MM-DD`, counted from 1970-01-01.
pub fn date_text(day: i64) -> String {
    // Civil date from a day count, after Howard Hinnant's algorithm.
    let z = day.saturating_add(719_468);
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
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

/// The side of a character portrait, in points: the row height less the gap around it, never so
/// small that the blank square is hard to see.
pub fn portrait_size(density: Density) -> f32 {
    (row_height(density) - 4.0).max(8.0)
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
    let portrait = portrait_size(density);
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
            "character" => {
                images::show(ui, Picture::Character(key), portrait);
                ui.add_space(4.0);
                ui.add(Label::new(line.name.as_str()).selectable(false));
                true
            }
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

/// What an error from renaming an account says to the player.
fn rename_error(error: &StoreError) -> String {
    if let StoreError::EmptyName = error {
        return "Type a name for the account.".to_owned();
    }
    if let StoreError::NameTaken(name) = error {
        return format!("An account called \"{name}\" already exists.");
    }
    format!("Could not rename the account: {error}")
}

/// What an error from putting a character in an account says to the player.
fn pick_error(error: &StoreError) -> String {
    if let StoreError::TooLarge = error {
        return ACCOUNT_FULL.to_owned();
    }
    format!("Could not save the account: {error}")
}

const KIND_COLUMNS: [&str; 4] = ["assets", "orders", "jobs", "wallet"];

/// The seconds in a UTC day.
const SECONDS_PER_DAY: i64 = 86_400;

/// What a date the player typed must look like.
const DATE_ERROR: &str = "Type the date as year-month-day, like 2026-10-18.";

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
