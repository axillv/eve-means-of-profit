//! The Characters page: the login characters and jEve's data. Nothing is fetched without a click:
//! `Refresh`, `Refresh all` and a finished login are the only things that fetch.
//!
//! The page keeps its state in the egui context, under one id, because the app builds it as a
//! unit value. Jobs on the worker send [`Msg`]s over a channel the page owns; every panel's
//! [`show`] takes them in at the start of the frame.

mod jeve;
pub mod list;
mod refresh;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use egui::{Context, Id, Ui};
use mop_esi::auth::CLIENT_ID;
use mop_esi::character_data::CharacterData;
use mop_esi::characters::{Character, Login};
use mop_esi::error::EsiError;
use mop_esi::places::StructureNames;
use mop_jeve::JeveData;

use crate::Shared;
use crate::dock;
use crate::session::{EveAccess, lock_eve};
use crate::status::Notice;

#[derive(Debug, Default)]
pub struct CharactersPage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharactersPanel {
    Characters,
    JeveData,
}

impl CharactersPanel {
    pub fn title(self) -> &'static str {
        match self {
            Self::Characters => "Characters",
            Self::JeveData => "jEve data",
        }
    }
}

/// Opens an address in the browser.
pub type Opener = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

#[derive(Clone)]
struct OpenerSlot(Opener);

#[derive(Clone, Copy)]
struct ClientIdSlot(bool);

/// Test hook: replaces the browser opener for the app that owns `ctx`.
pub fn set_opener(ctx: &Context, opener: Opener) {
    ctx.data_mut(|data| data.insert_temp(Id::new("characters-opener"), OpenerSlot(opener)));
}

/// Test hook: tells the page whether this build has an EVE client id, for the app that owns `ctx`.
pub fn set_client_id_present(ctx: &Context, present: bool) {
    ctx.data_mut(|data| data.insert_temp(Id::new("characters-client-id"), ClientIdSlot(present)));
}

fn opener(ctx: &Context) -> Opener {
    ctx.data(|data| data.get_temp::<OpenerSlot>(Id::new("characters-opener")))
        .map_or_else(|| Arc::new(open_in_browser) as Opener, |slot| slot.0)
}

fn login_possible(ctx: &Context) -> bool {
    ctx.data(|data| data.get_temp::<ClientIdSlot>(Id::new("characters-client-id")))
        .map_or(!CLIENT_ID.is_empty(), |slot| slot.0)
}

/// The platform's opener for an address.
fn open_in_browser(url: &str) -> Result<(), String> {
    let mut command = if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(target_os = "windows") {
        let mut command = Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler");
        command
    } else {
        Command::new("xdg-open")
    };
    let status = command
        .arg(url)
        .status()
        .map_err(|error| error.to_string())?;
    check_exit(status)
}

/// `Ok` when the opener finished well.
fn check_exit(status: ExitStatus) -> Result<(), String> {
    if status.success() {
        Ok(())
    } else {
        Err(format!("the opener stopped with {status}"))
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::check_exit;

    #[cfg(unix)]
    #[test]
    fn an_opener_that_exits_with_an_error_is_reported() {
        let failed = Command::new("sh")
            .args(["-c", "exit 3"])
            .status()
            .expect("sh runs");
        let error = check_exit(failed).expect_err("a non-zero exit is an error");
        assert!(error.starts_with("the opener stopped with"), "{error}");
        let fine = Command::new("sh")
            .args(["-c", "exit 0"])
            .status()
            .expect("sh runs");
        assert_eq!(check_exit(fine), Ok(()));
    }
}

/// What a job on the worker tells the page.
enum Msg {
    Keychain(Option<String>),
    Notice(Notice),
    /// The login ended without a character; the notice says why, when it needs saying.
    LoginEnded(Notice),
    LoggedIn(Character),
    Refreshed(Box<refresh::Outcome>),
    RefreshDone(Option<Notice>),
    Removed {
        id: u64,
        name: String,
        result: Result<(), EsiError>,
    },
    JeveListed(jeve::Listing),
    JeveRead(Result<Option<JeveData>, String>),
}

struct Inner {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    /// What the one running job is doing, such as `refreshing Main`.
    busy: Option<String>,
    /// Set while the app waits for the browser's answer; the flag stops the wait.
    waiting: Option<Arc<AtomicBool>>,
    /// The character whose Remove… asks for a confirmation.
    confirm: Option<(u64, String)>,
    keychain: Option<String>,
    keychain_asked: bool,
    jeve: jeve::State,
}

#[derive(Clone)]
struct Handle(Arc<Mutex<Inner>>);

fn handle(ctx: &Context) -> Handle {
    let id = Id::new("characters-page");
    if let Some(held) = ctx.data(|data| data.get_temp::<Handle>(id)) {
        return held;
    }
    let (tx, rx) = mpsc::channel();
    let made = Handle(Arc::new(Mutex::new(Inner {
        tx,
        rx,
        busy: None,
        waiting: None,
        confirm: None,
        keychain: None,
        keychain_asked: false,
        jeve: jeve::State::default(),
    })));
    ctx.data_mut(|data| data.insert_temp(id, made.clone()));
    made
}

fn lock(inner: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    inner.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Sends a message to the page and asks for a repaint.
fn send(tx: &Sender<Msg>, ctx: &Context, msg: Msg) {
    // The receiver is gone only when the app is closing, and then nobody needs the message.
    drop(tx.send(msg));
    ctx.request_repaint();
}

/// Draws the body of one panel of this page.
pub fn show(_page: &mut CharactersPage, panel: CharactersPanel, ui: &mut Ui, cx: &mut Shared) {
    if !cx.session.loaded() {
        dock::placeholder(ui, cx);
        return;
    }
    let ctx = ui.ctx().clone();
    let held = handle(&ctx);
    let mut inner = lock(&held.0);
    drain(&mut inner, cx, &ctx);
    match panel {
        CharactersPanel::Characters => {
            ask_keychain(&mut inner, cx, &ctx);
            let click = list::show(ui, cx, &list::View::new(&inner, login_possible(&ctx)));
            if let Some(click) = click {
                act(&mut inner, cx, &ctx, click);
            }
        }
        CharactersPanel::JeveData => jeve::show(ui, cx, &mut inner, &ctx),
    }
}

/// Asks the worker for the keychain warning, once per session.
fn ask_keychain(inner: &mut Inner, cx: &mut Shared, ctx: &Context) {
    if inner.keychain_asked {
        return;
    }
    inner.keychain_asked = true;
    let eve = cx.session.eve();
    let tx = inner.tx.clone();
    let ctx = ctx.clone();
    queue(inner, cx, move || {
        let warning = lock_eve(&eve).keychain_warning();
        send(&tx, &ctx, Msg::Keychain(warning));
    });
}

/// Queues a job. When the worker has stopped, the player is told and `busy` is cleared.
fn queue(inner: &mut Inner, cx: &mut Shared, job: impl FnOnce() + Send + 'static) {
    if !cx.worker.run(Box::new(job)) {
        inner.busy = None;
        inner.waiting = None;
        cx.notices.push(Notice::error(
            "The background worker has stopped. Restart the app.",
        ));
    }
}

fn act(inner: &mut Inner, cx: &mut Shared, ctx: &Context, click: list::Click) {
    match click {
        list::Click::Add => start_login(inner, cx, ctx),
        list::Click::RefreshAll => {
            let ids: Vec<u64> = cx
                .session
                .characters()
                .iter()
                .map(|character| character.id)
                .collect();
            start_refresh(inner, cx, ctx, &ids);
        }
        list::Click::Refresh(id) => start_refresh(inner, cx, ctx, &[id]),
        list::Click::Remove(id) => {
            let name = name_of(cx, id);
            inner.confirm = Some((id, name));
        }
        list::Click::Keep => inner.confirm = None,
        list::Click::ConfirmRemove => start_remove(inner, cx, ctx),
        list::Click::Cancel => {
            if let Some(flag) = &inner.waiting {
                flag.store(true, Ordering::Relaxed);
            }
        }
    }
}

fn name_of(cx: &Shared, id: u64) -> String {
    cx.session
        .characters()
        .iter()
        .find(|character| character.id == id)
        .map_or_else(|| id.to_string(), |character| character.name.clone())
}

fn start_login(inner: &mut Inner, cx: &mut Shared, ctx: &Context) {
    let cancel = Arc::new(AtomicBool::new(false));
    inner.waiting = Some(Arc::clone(&cancel));
    inner.busy = Some("waiting for the EVE login".to_owned());
    let eve = cx.session.eve();
    let tx = inner.tx.clone();
    let ctx = ctx.clone();
    let open = opener(&ctx);
    // The wait for the browser can take minutes. It has a thread of its own and holds no lock on
    // the login: the lock is taken only for the short calls before and after it.
    let spawned = std::thread::Builder::new()
        .name("eve-login".to_owned())
        .spawn(move || {
            let attempt = catch_unwind(AssertUnwindSafe(|| {
                run_login(&eve, &open, &cancel, &|msg| send(&tx, &ctx, msg))
            }));
            let ended = attempt.unwrap_or_else(|_| {
                Msg::LoginEnded(Notice::error(
                    "Could not log in: the login stopped unexpectedly.",
                ))
            });
            send(&tx, &ctx, ended);
        });
    if let Err(error) = spawned {
        inner.busy = None;
        inner.waiting = None;
        cx.notices
            .push(Notice::error(format!("Could not start the login: {error}")));
    }
}

/// Begins the login, opens the browser, waits for the answer and completes the login. Returns
/// what the page is told at the end; the notice about a browser that would not open is sent on
/// the way. The lock on the login is held only for `begin_login` and `finish_login`.
fn run_login(
    eve: &Mutex<Box<dyn EveAccess>>,
    open: &Opener,
    cancel: &AtomicBool,
    send: &dyn Fn(Msg),
) -> Msg {
    let begun = lock_eve(eve).begin_login();
    let pending = match begun {
        Ok(pending) => pending,
        Err(text) => return Msg::LoginEnded(Notice::error(format!("Could not log in: {text}"))),
    };
    if let Err(reason) = open(&pending.url) {
        send(Msg::Notice(Notice::error(format!(
            "Could not open the browser ({reason}). Open this address yourself: {}",
            pending.url
        ))));
    }
    let code = match pending.wait(cancel) {
        Ok(code) => code,
        Err(EsiError::Cancelled) if cancel.load(Ordering::Relaxed) => {
            return Msg::LoginEnded(Notice::info("Login cancelled."));
        }
        Err(error) => {
            return Msg::LoginEnded(Notice::error(format!("Could not log in: {error}")));
        }
    };
    let finished = lock_eve(eve).finish_login(&code);
    match finished {
        Ok(character) => Msg::LoggedIn(character),
        Err(error) => Msg::LoginEnded(Notice::error(format!("Could not log in: {error}"))),
    }
}

fn start_refresh(inner: &mut Inner, cx: &mut Shared, ctx: &Context, ids: &[u64]) {
    let mut targets: Vec<refresh::Target> = cx
        .session
        .characters()
        .iter()
        .filter(|character| ids.contains(&character.id))
        .map(|character| refresh::Target {
            id: character.id,
            name: character.name.clone(),
            data: cx
                .session
                .character_data(character.id)
                .cloned()
                .unwrap_or_else(|| CharacterData::new(character.id)),
        })
        .collect();
    if targets.is_empty() {
        return;
    }
    targets.sort_by_key(|target| (target.name.to_lowercase(), target.name.clone()));
    inner.busy = Some(match targets.as_slice() {
        [only] => format!("refreshing {}", only.name),
        _ => "refreshing the characters".to_owned(),
    });
    let eve = cx.session.eve();
    let names: StructureNames = cx.session.structures().clone();
    let data_dir = cx.data_dir.clone();
    let now = (cx.clock)();
    let tx = inner.tx.clone();
    let ctx = ctx.clone();
    queue(inner, cx, move || {
        refresh::run(&eve, targets, names, &data_dir, now, &|msg| {
            send(&tx, &ctx, msg);
        });
    });
}

fn start_remove(inner: &mut Inner, cx: &mut Shared, ctx: &Context) {
    let Some((id, name)) = inner.confirm.take() else {
        return;
    };
    inner.busy = Some(format!("removing {name}"));
    let eve = cx.session.eve();
    let tx = inner.tx.clone();
    let ctx = ctx.clone();
    queue(inner, cx, move || {
        let result = lock_eve(&eve).remove(id);
        send(&tx, &ctx, Msg::Removed { id, name, result });
    });
}

/// Takes in what the jobs have sent.
fn drain(inner: &mut Inner, cx: &mut Shared, ctx: &Context) {
    let msgs: Vec<Msg> = inner.rx.try_iter().collect();
    for msg in msgs {
        apply(inner, cx, ctx, msg);
    }
}

fn apply(inner: &mut Inner, cx: &mut Shared, ctx: &Context, msg: Msg) {
    match msg {
        Msg::Keychain(warning) => {
            if let Some(text) = &warning {
                cx.notices.push(Notice::info(text.as_str()));
            }
            inner.keychain = warning;
        }
        Msg::Notice(notice) => cx.notices.push(notice),
        Msg::LoginEnded(notice) => {
            inner.busy = None;
            inner.waiting = None;
            cx.notices.push(notice);
        }
        Msg::LoggedIn(character) => {
            inner.busy = None;
            inner.waiting = None;
            let id = character.id;
            cx.session.put_character(character);
            start_refresh(inner, cx, ctx, &[id]);
        }
        Msg::Refreshed(outcome) => apply_outcome(cx, *outcome),
        Msg::RefreshDone(notice) => {
            inner.busy = None;
            cx.notices.extend(notice);
        }
        Msg::Removed { id, name, result } => {
            inner.busy = None;
            match result {
                Ok(()) => cx.session.remove_character(id),
                Err(error) => cx
                    .notices
                    .push(Notice::error(format!("Could not remove {name}: {error}"))),
            }
        }
        Msg::JeveListed(listing) => inner.jeve.listed(listing),
        Msg::JeveRead(result) => match result {
            Ok(data) => cx.session.set_jeve(data),
            Err(error) => cx.notices.push(Notice::error(format!(
                "Could not read jEve's data file: {error}"
            ))),
        },
    }
}

fn apply_outcome(cx: &mut Shared, outcome: refresh::Outcome) {
    if let Some(data) = outcome.data {
        cx.session.replace(data);
    }
    if let Some(names) = outcome.structures {
        cx.session.set_structures(names);
    }
    if outcome.expired
        && let Some(held) = cx
            .session
            .characters()
            .iter()
            .find(|character| character.id == outcome.id)
    {
        let mut expired = held.clone();
        expired.login = Login::Expired;
        cx.session.put_character(expired);
    }
    cx.notices.extend(outcome.notices);
}

/// Test hook: makes the page forget its state, as if no panel had been shown yet. Jobs already
/// running send to the old state and are ignored.
pub fn forget_state(ctx: &Context) {
    ctx.data_mut(|data| data.remove::<Handle>(Id::new("characters-page")));
}
