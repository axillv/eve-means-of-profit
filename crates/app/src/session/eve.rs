//! The app's link to EVE: the login characters, their saved data, and the calls that fetch and name
//! things. [`LiveEve`] is the real one. [`NoEve`] has no characters and no login; the tests use it
//! where they do not need one.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mop_core::stock::CharacterId;
use mop_esi::auth::{Auth, CLIENT_ID, LoginStart};
use mop_esi::callback;
use mop_esi::character_data::{self, CharacterData, Kind, Refreshed};
use mop_esi::characters::Character;
use mop_esi::client::Client;
use mop_esi::error::EsiError;
use mop_esi::http::UreqHttp;
use mop_esi::industry::{self, IndustryData, IndustryKind};
use mop_esi::places::{self, NamingReport, StructureNames};
use mop_esi::saved;
use mop_esi::secrets::{fallback_warning, open_secrets};
use mop_jeve::profiles_dir;

/// How long the browser has to answer a login.
const LOGIN_WAIT: Duration = Duration::from_secs(300);
/// The text the login button shows when this build has no EVE client id.
pub(crate) const NO_CLIENT_ID: &str = "This build has no EVE login yet.";

/// The wait for the browser's answer: blocks until the code comes or the cancel flag is set.
type Waiter = Box<dyn FnOnce(&AtomicBool) -> Result<String, EsiError> + Send>;

/// A login the browser has been sent to. The wait for the browser's answer owns no shared state,
/// so it runs without any lock on the login.
pub struct PendingLogin {
    /// The address the browser opens.
    pub url: String,
    wait: Waiter,
}

impl PendingLogin {
    /// A login at `url` whose answer `wait` fetches: it blocks until the answer comes, `cancel` is
    /// set, or the time is up, and gives the code EVE sent.
    pub fn new(
        url: String,
        wait: impl FnOnce(&AtomicBool) -> Result<String, EsiError> + Send + 'static,
    ) -> Self {
        Self {
            url,
            wait: Box::new(wait),
        }
    }

    /// Waits for the browser's answer, for up to 300 seconds, or until `cancel` is set. Gives the
    /// code to pass to [`EveAccess::finish_login`].
    pub fn wait(self, cancel: &AtomicBool) -> Result<String, EsiError> {
        (self.wait)(cancel)
    }
}

/// What the app needs from EVE and from the files next to it. The worker runs the calls that touch
/// the network or the disk; the pages never call it.
pub trait EveAccess: Send {
    /// The characters added to the login, in the order they were added.
    fn characters(&self) -> Vec<Character>;
    /// Starts a login. The pending login holds the address the browser opens and the wait for the
    /// answer. The text is what the player reads when the login cannot start.
    fn begin_login(&mut self) -> Result<PendingLogin, String>;
    /// Completes the login begun last with the code the wait gave: trades it for tokens and adds
    /// the character. Quick; the wait for the browser is [`PendingLogin::wait`].
    fn finish_login(&mut self, code: &str) -> Result<Character, EsiError>;
    /// Fetches each kind of one character's data that is due, and gives one result per kind.
    fn refresh_all(
        &mut self,
        data: &mut CharacterData,
        now: i64,
    ) -> Vec<(Kind, Result<Refreshed, EsiError>)>;
    /// Fetches one kind of the shared industry data (cost indices or adjusted prices) when it is
    /// due, and saves it. The default does nothing, so fakes need not implement it.
    fn refresh_industry(
        &mut self,
        _data: &mut IndustryData,
        _kind: IndustryKind,
        now: i64,
    ) -> Result<Refreshed, EsiError> {
        Ok(Refreshed::NotYet { until: now })
    }
    /// Names the structures in `ids` that are due for a check, with the login characters.
    fn name_structures(
        &mut self,
        names: &mut StructureNames,
        ids: &[i64],
        now: i64,
    ) -> Result<NamingReport, EsiError>;
    /// Writes one character's data to its saved file.
    fn save_character(&mut self, data: &CharacterData) -> Result<(), EsiError>;
    /// Removes a character from the login and forgets its saved data.
    fn remove(&mut self, character_id: u64) -> Result<(), EsiError>;
    /// The warning to show once when the login keeps its tokens in a plain file.
    fn keychain_warning(&self) -> Option<String>;
    /// The folder where jEveAssets keeps its profiles, or `None` when there is none to read.
    fn jeve_folder(&self) -> Option<PathBuf>;
}

/// The real login: the characters and tokens under `<data_dir>/esi`, EVE's servers, and jEve's
/// folder in the user's home.
pub struct LiveEve {
    auth: Auth,
    http: UreqHttp,
    esi_dir: PathBuf,
    jeve_dir: Option<PathBuf>,
    warning: Option<String>,
    pending: Option<LoginStart>,
}

impl LiveEve {
    /// Opens the logins kept under `data_dir/esi`.
    pub fn open(data_dir: &Path) -> Result<Self, EsiError> {
        let esi_dir = data_dir.join("esi");
        fs::create_dir_all(&esi_dir).map_err(|source| EsiError::Io {
            path: esi_dir.clone(),
            source,
        })?;
        let secrets = open_secrets(&esi_dir);
        let warning = secrets
            .fallback
            .as_ref()
            .map(|reason| fallback_warning(&esi_dir, reason));
        let auth = Auth::open(&esi_dir, Box::new(UreqHttp::new()), secrets, CLIENT_ID)?;
        Ok(Self {
            auth,
            http: UreqHttp::new(),
            esi_dir,
            jeve_dir: profiles_dir(),
            warning,
            pending: None,
        })
    }
}

impl EveAccess for LiveEve {
    fn characters(&self) -> Vec<Character> {
        self.auth.characters().to_vec()
    }

    fn begin_login(&mut self) -> Result<PendingLogin, String> {
        match self.auth.begin_login() {
            Ok(start) => {
                let url = start.url.clone();
                let state = start.pkce.state().to_owned();
                self.pending = Some(start);
                Ok(PendingLogin::new(url, move |cancel| {
                    let deadline = Instant::now() + LOGIN_WAIT;
                    callback::wait_for_code(&state, deadline, cancel)
                }))
            }
            Err(EsiError::NoClientId) => Err(NO_CLIENT_ID.to_owned()),
            Err(error) => Err(error.to_string()),
        }
    }

    fn finish_login(&mut self, code: &str) -> Result<Character, EsiError> {
        // Without a begun login no answer can belong to it.
        let start = self.pending.take().ok_or(EsiError::StateMismatch)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| {
                i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
            });
        self.auth.finish_login(&start, code, now)
    }

    fn refresh_all(
        &mut self,
        data: &mut CharacterData,
        now: i64,
    ) -> Vec<(Kind, Result<Refreshed, EsiError>)> {
        let mut client = Client::new(&self.http);
        character_data::refresh_all(data, &mut self.auth, &mut client, now)
    }

    fn refresh_industry(
        &mut self,
        data: &mut IndustryData,
        kind: IndustryKind,
        now: i64,
    ) -> Result<Refreshed, EsiError> {
        let done = industry::refresh_industry(data, kind, &self.http, now)?;
        save_fetched(&self.esi_dir, data, kind)?;
        Ok(done)
    }

    fn name_structures(
        &mut self,
        names: &mut StructureNames,
        ids: &[i64],
        now: i64,
    ) -> Result<NamingReport, EsiError> {
        let characters: Vec<CharacterId> = self
            .auth
            .characters()
            .iter()
            .filter_map(|character| i64::try_from(character.id).ok())
            .collect();
        places::name_structures(
            &self.esi_dir,
            names,
            ids,
            &characters,
            &mut self.auth,
            &self.http,
            now,
        )
    }

    fn save_character(&mut self, data: &CharacterData) -> Result<(), EsiError> {
        saved::save_character(&self.esi_dir, data)
    }

    fn remove(&mut self, character_id: u64) -> Result<(), EsiError> {
        self.auth.remove(character_id)?;
        saved::forget(&self.esi_dir, character_id)
    }

    fn keychain_warning(&self) -> Option<String> {
        self.warning.clone()
    }

    fn jeve_folder(&self) -> Option<PathBuf> {
        self.jeve_dir.clone()
    }
}

/// Saves the `kind` just fetched into `data` to the industry file in `dir`. Only that kind is
/// merged into what is on disk, so a job that started earlier with an older copy of the other
/// kind cannot write that copy back over a newer file. A saved file that cannot be read is
/// replaced by `data`, so the fetch is not lost and later refreshes can save again.
///
/// # Errors
/// The file cannot be written.
pub fn save_fetched(dir: &Path, data: &IndustryData, kind: IndustryKind) -> Result<(), EsiError> {
    let mut saved = industry::load_industry(dir).unwrap_or_else(|_unreadable| data.clone());
    match kind {
        IndustryKind::Systems => saved.systems.clone_from(&data.systems),
        IndustryKind::Prices => saved.prices.clone_from(&data.prices),
    }
    industry::save_industry(dir, &saved)
}

/// No characters and no login. Holds nothing and reads nothing.
#[derive(Debug, Default)]
pub struct NoEve;

impl EveAccess for NoEve {
    fn characters(&self) -> Vec<Character> {
        Vec::new()
    }

    fn begin_login(&mut self) -> Result<PendingLogin, String> {
        Err(NO_CLIENT_ID.to_owned())
    }

    fn finish_login(&mut self, _code: &str) -> Result<Character, EsiError> {
        Err(EsiError::StateMismatch)
    }

    fn refresh_all(
        &mut self,
        _data: &mut CharacterData,
        _now: i64,
    ) -> Vec<(Kind, Result<Refreshed, EsiError>)> {
        Vec::new()
    }

    fn name_structures(
        &mut self,
        _names: &mut StructureNames,
        _ids: &[i64],
        _now: i64,
    ) -> Result<NamingReport, EsiError> {
        Ok(NamingReport {
            asked: 0,
            failed: Vec::new(),
        })
    }

    fn save_character(&mut self, _data: &CharacterData) -> Result<(), EsiError> {
        Ok(())
    }

    fn remove(&mut self, character_id: u64) -> Result<(), EsiError> {
        Err(EsiError::UnknownCharacter(character_id))
    }

    fn keychain_warning(&self) -> Option<String> {
        None
    }

    fn jeve_folder(&self) -> Option<PathBuf> {
        None
    }
}
