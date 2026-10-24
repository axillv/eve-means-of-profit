//! The login: adds characters through EVE's browser login, keeps them logged in, and hands out access tokens.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use crate::characters::{Character, CharacterList, Login};
use crate::error::EsiError;
use crate::http::{Http, HttpRequest, encode};
use crate::jwt::{self, Claims, Jwks};
use crate::pkce::Pkce;
use crate::secrets::Secrets;
use crate::token::{self, AccessToken};

pub const CALLBACK_PORT: u16 = 9376;
pub const REDIRECT_URI: &str = "http://localhost:9376/eve-means-of-profit";
/// The app's public client id from the EVE developer portal. Empty until it is registered; logins cannot start while it is empty.
pub const CLIENT_ID: &str = "";
pub const SCOPES: [&str; 17] = [
    "esi-assets.read_assets.v1",
    "esi-universe.read_structures.v1",
    "esi-search.search_structures.v1",
    "esi-markets.read_character_orders.v1",
    "esi-markets.structure_markets.v1",
    "esi-industry.read_character_jobs.v1",
    "esi-industry.read_character_mining.v1",
    "esi-wallet.read_character_wallet.v1",
    "esi-skills.read_skills.v1",
    "esi-characters.read_blueprints.v1",
    "esi-characters.read_standings.v1",
    "esi-characters.read_loyalty.v1",
    "esi-contracts.read_character_contracts.v1",
    "esi-planets.manage_planets.v1",
    "esi-location.read_location.v1",
    "esi-ui.open_window.v1",
    "esi-ui.write_waypoint.v1",
];
const AUTHORIZE_URL: &str = "https://login.eveonline.com/v2/oauth/authorize";
pub const TOKEN_URL: &str = "https://login.eveonline.com/v2/oauth/token";
pub const REVOKE_URL: &str = "https://login.eveonline.com/v2/oauth/revoke";
const JWKS_URL: &str = "https://login.eveonline.com/oauth/jwks";
/// An access token is renewed when it has less than this many seconds left.
const RENEW_BEFORE: i64 = 60;

/// One login in progress: the page to open, and the PKCE pair the answer is checked against.
pub struct LoginStart {
    pub url: String,
    pub pkce: Pkce,
}

/// The address the browser opens: EVE's authorize page with this app's id, scopes and PKCE challenge.
pub fn authorize_url(client_id: &str, scopes: &[&str], pkce: &Pkce) -> String {
    format!(
        "{AUTHORIZE_URL}?response_type=code&redirect_uri={}&client_id={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}",
        encode(REDIRECT_URI),
        encode(client_id),
        encode(&scopes.join(" ")),
        pkce.challenge(),
        pkce.state(),
    )
}

struct Held {
    token: AccessToken,
    expires: i64,
}

pub struct Auth {
    http: Box<dyn Http>,
    secrets: Secrets,
    characters: CharacterList,
    client_id: String,
    keys: Option<Jwks>,
    held: BTreeMap<u64, Held>,
}

impl fmt::Debug for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Auth")
            .field("characters", &self.characters.all().len())
            .finish_non_exhaustive()
    }
}

impl Auth {
    /// Opens the character list kept in `dir`. The steps that need the time take `now` from the caller.
    pub fn open(
        dir: &Path,
        http: Box<dyn Http>,
        secrets: Secrets,
        client_id: &str,
    ) -> Result<Self, EsiError> {
        Ok(Self {
            http,
            secrets,
            characters: CharacterList::open(dir)?,
            client_id: client_id.to_owned(),
            keys: None,
            held: BTreeMap::new(),
        })
    }

    pub fn begin_login(&self) -> Result<LoginStart, EsiError> {
        if self.client_id.is_empty() {
            return Err(EsiError::NoClientId);
        }
        let pkce = Pkce::new()?;
        let url = authorize_url(&self.client_id, &SCOPES, &pkce);
        Ok(LoginStart { url, pkce })
    }

    /// Exchanges the browser answer for tokens, stores the refresh token and then adds the character to the list.
    pub fn finish_login(
        &mut self,
        start: &LoginStart,
        code: &str,
        now: i64,
    ) -> Result<Character, EsiError> {
        let tokens = token::exchange_code(
            &*self.http,
            &self.client_id,
            code,
            start.pkce.verifier(),
            now,
        )?;
        let claims = self.verify(tokens.access.as_str(), now)?;
        self.secrets
            .store
            .set(claims.character_id, &tokens.refresh)?;
        let character = self.characters.upsert(Character {
            id: claims.character_id,
            name: claims.name,
            scopes: claims.scopes,
            added_at: now,
            login: Login::Ok,
        })?;
        self.held.insert(
            character.id,
            Held {
                token: tokens.access,
                expires: tokens.expires,
            },
        );
        Ok(character)
    }

    /// The character's access token, renewed first when it has less than a minute left.
    pub fn access_token(&mut self, character_id: u64, now: i64) -> Result<AccessToken, EsiError> {
        if self.characters.get(character_id).is_none() {
            return Err(EsiError::UnknownCharacter(character_id));
        }
        if let Some(held) = self.held.get(&character_id).filter(|held| {
            held.expires
                .checked_sub(now)
                .is_some_and(|left| left > RENEW_BEFORE)
        }) {
            return Ok(held.token.clone());
        }
        self.renew(character_id, now)
    }

    /// Ends the login with EVE, then forgets the character. A failed revoke does not stop the removal.
    pub fn remove(&mut self, character_id: u64) -> Result<(), EsiError> {
        if self.characters.get(character_id).is_none() {
            return Err(EsiError::UnknownCharacter(character_id));
        }
        // The revoke is best effort: the local copy is removed whatever EVE answers, and a token that cannot be read cannot be revoked.
        if let Ok(Some(stored)) = self.secrets.store.get(character_id) {
            drop(token::revoke(&*self.http, &self.client_id, &stored));
        }
        self.secrets.store.delete(character_id)?;
        self.held.remove(&character_id);
        self.characters.remove(character_id)
    }

    pub fn characters(&self) -> &[Character] {
        self.characters.all()
    }

    /// Verifies a token against EVE's keys. The key set is fetched on first use and again once when the token names an unknown key.
    fn verify(&mut self, token: &str, now: i64) -> Result<Claims, EsiError> {
        let mut keys = match self.keys.take() {
            Some(keys) => keys,
            None => self.fetch_keys()?,
        };
        let result = match jwt::verify(token, &keys, &self.client_id, now) {
            Err(EsiError::BadToken("unknown key")) => {
                keys = self.fetch_keys()?;
                jwt::verify(token, &keys, &self.client_id, now)
            }
            other => other,
        };
        self.keys = Some(keys);
        result
    }

    fn fetch_keys(&self) -> Result<Jwks, EsiError> {
        let response = self.http.send(&HttpRequest::get(JWKS_URL))?;
        if response.status != 200 {
            return Err(EsiError::Server(response.status));
        }
        Jwks::parse(&response.body)
    }

    fn renew(&mut self, character_id: u64, now: i64) -> Result<AccessToken, EsiError> {
        let Some(stored) = self.secrets.store.get(character_id)? else {
            return self.expire(character_id);
        };
        match token::refresh(&*self.http, &self.client_id, &stored, now) {
            Ok(tokens) => {
                // The new refresh token is stored before the new access token is handed out.
                self.secrets.store.set(character_id, &tokens.refresh)?;
                self.held.insert(
                    character_id,
                    Held {
                        token: tokens.access.clone(),
                        expires: tokens.expires,
                    },
                );
                Ok(tokens.access)
            }
            Err(EsiError::LoginExpired) => self.expire(character_id),
            Err(error) => Err(error),
        }
    }

    /// EVE refused the refresh token: the character stays listed, marked expired, with no stored login.
    fn expire(&mut self, character_id: u64) -> Result<AccessToken, EsiError> {
        self.held.remove(&character_id);
        self.characters.set_login(character_id, Login::Expired)?;
        // EVE has already refused the token, so a copy left behind by a failed delete is refused again on the next renewal; the refusal is what the caller must see.
        drop(self.secrets.store.delete(character_id));
        Err(EsiError::LoginExpired)
    }
}
