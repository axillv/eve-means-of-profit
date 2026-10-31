use super::*;

pub(super) fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE)
}

pub(super) struct NoNetwork;

impl Fetch for NoNetwork {
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

pub(super) struct NoForge;

impl BytesHttp for NoForge {
    fn get_bytes(
        &self,
        _url: &str,
        _if_none_match: Option<&str>,
    ) -> Result<BytesResponse, MarketError> {
        Err(MarketError::Network("not in these tests".to_owned()))
    }
}

/// An image server that never answers, so a test sees only the pictures it keeps itself.
pub(super) struct NoImages;

impl images::Fetch for NoImages {
    fn get(&self, _url: &str) -> Result<Vec<u8>, String> {
        Err("offline".to_owned())
    }
}

pub(super) struct Env {
    _dir: TempDir,
    pub(super) data: PathBuf,
    pub(super) cache: PathBuf,
}

pub(super) fn env() -> Env {
    let dir = TempDir::new().expect("a temp folder");
    let data = dir.path().join("data");
    let cache = dir.path().join("cache");
    fs::create_dir_all(&data).expect("the data folder");
    import(&fixture(), &data.join("sde.sqlite")).expect("the fixture imports");
    history_off(&data.join("user.sqlite"));
    let mut store = Store::open(&data.join("user.sqlite"), NOW)
        .expect("the store opens")
        .store;
    let mut settings = store.settings().expect("settings");
    settings.last_sde_check = Some(NOW);
    store.save_settings(&settings).expect("settings saved");
    Env {
        _dir: dir,
        data,
        cache,
    }
}

pub(super) fn esi_dir(env: &Env) -> PathBuf {
    env.data.join("esi")
}

pub(super) type Results = Vec<(Kind, Result<Refreshed, EsiError>)>;
/// What the fake's `refresh_all` does to the data and answers.
pub(super) type Script = fn(&mut CharacterData) -> Results;

#[derive(Default)]
pub(super) struct Log {
    pub(super) refreshed: Vec<u64>,
    pub(super) named: Vec<(Vec<i64>, Vec<u64>)>,
    pub(super) removed: Vec<u64>,
    pub(super) opened: Vec<String>,
    pub(super) begun: usize,
    pub(super) keychain: usize,
}

#[derive(Clone)]
pub(super) struct FakeEve {
    pub(super) characters: Vec<Character>,
    pub(super) jeve: Option<PathBuf>,
    pub(super) esi: PathBuf,
    pub(super) script: Script,
    pub(super) begin: Result<String, String>,
    /// Set to let the fake's login finish with Gamma.
    pub(super) release: Arc<AtomicBool>,
    pub(super) warning: Option<String>,
    pub(super) failed_names: usize,
    /// While set, `refresh_all` waits.
    pub(super) hold: Arc<AtomicBool>,
    /// The fake's login ends with `Cancelled` at once, though the player did not cancel.
    pub(super) cancels_alone: bool,
    /// The fake's wait for the browser panics.
    pub(super) panics: bool,
    pub(super) log: Arc<Mutex<Log>>,
}

impl FakeEve {
    pub(super) fn new(env: &Env, script: Script) -> Self {
        Self {
            characters: vec![character(MAIN, "Main", Login::Ok), alt()],
            jeve: None,
            esi: esi_dir(env),
            script,
            begin: Ok("https://login.example/start".to_owned()),
            release: Arc::new(AtomicBool::new(false)),
            warning: None,
            failed_names: 0,
            hold: Arc::new(AtomicBool::new(false)),
            cancels_alone: false,
            panics: false,
            log: Arc::new(Mutex::new(Log::default())),
        }
    }

    pub(super) fn log<T>(&self, read: impl FnOnce(&Log) -> T) -> T {
        read(&self.log.lock().unwrap_or_else(PoisonError::into_inner))
    }

    pub(super) fn write(&self, change: impl FnOnce(&mut Log)) {
        change(&mut self.log.lock().unwrap_or_else(PoisonError::into_inner));
    }
}

impl EveAccess for FakeEve {
    fn characters(&self) -> Vec<Character> {
        self.characters.clone()
    }

    fn begin_login(&mut self) -> Result<PendingLogin, String> {
        self.write(|log| log.begun += 1);
        let url = self.begin.clone()?;
        let release = Arc::clone(&self.release);
        let cancels_alone = self.cancels_alone;
        let panics = self.panics;
        Ok(PendingLogin::new(url, move |cancel| {
            if panics {
                std::panic::resume_unwind(Box::new("the wait breaks"));
            }
            if cancels_alone {
                return Err(EsiError::Cancelled);
            }
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Err(EsiError::Cancelled);
                }
                if release.load(Ordering::Relaxed) {
                    return Ok("the code".to_owned());
                }
                thread::sleep(Duration::from_millis(5));
            }
        }))
    }

    fn finish_login(&mut self, code: &str) -> Result<Character, EsiError> {
        if code != "the code" {
            return Err(EsiError::StateMismatch);
        }
        Ok(character(GAMMA, "Gamma", Login::Ok))
    }

    fn refresh_all(&mut self, data: &mut CharacterData, _now: i64) -> Results {
        self.write(|log| log.refreshed.push(data.character_id));
        while self.hold.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(5));
        }
        (self.script)(data)
    }

    fn name_structures(
        &mut self,
        names: &mut StructureNames,
        ids: &[i64],
        _now: i64,
    ) -> Result<NamingReport, EsiError> {
        let logins: Vec<u64> = self.characters.iter().map(|held| held.id).collect();
        self.write(|log| log.named.push((ids.to_vec(), logins)));
        for id in ids {
            names.insert(
                *id,
                StructureEntry {
                    name: Some("Home Fort".to_owned()),
                    system_id: None,
                    refused: false,
                    checked_at: NOW,
                },
            );
        }
        Ok(NamingReport {
            asked: ids.len(),
            failed: (0..self.failed_names)
                .map(|_| (HOME_FORT, EsiError::Network("down".to_owned())))
                .collect(),
        })
    }

    fn save_character(&mut self, data: &CharacterData) -> Result<(), EsiError> {
        saved::save_character(&self.esi, data)
    }

    fn remove(&mut self, character_id: u64) -> Result<(), EsiError> {
        self.write(|log| log.removed.push(character_id));
        self.characters.retain(|held| held.id != character_id);
        Ok(())
    }

    fn keychain_warning(&self) -> Option<String> {
        self.write(|log| log.keychain += 1);
        self.warning.clone()
    }

    fn jeve_folder(&self) -> Option<PathBuf> {
        self.jeve.clone()
    }
}

pub(super) fn character(id: u64, name: &str, login: Login) -> Character {
    Character {
        id,
        name: name.to_owned(),
        scopes: BTreeSet::new(),
        added_at: NOW,
        login,
    }
}

pub(super) fn alt() -> Character {
    character(ALT, "Alt", Login::Expired)
}
