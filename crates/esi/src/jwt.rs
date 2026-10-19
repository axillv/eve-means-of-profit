//! Checks EVE's access tokens: which key signed them, who they are for, and whether they are still valid.

use std::collections::BTreeSet;

use base64::Engine as _;
use base64::prelude::BASE64_URL_SAFE_NO_PAD;
use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::Deserialize;

use crate::error::EsiError;

const ISSUERS: [&str; 2] = ["https://login.eveonline.com", "login.eveonline.com"];
const AUDIENCE: &str = "EVE Online";
const SUBJECT_PREFIX: &str = "CHARACTER:EVE:";
const LEEWAY: i64 = 60;

/// EVE's public signing keys.
pub struct Jwks(JwkSet);

impl Jwks {
    pub fn parse(bytes: &[u8]) -> Result<Self, EsiError> {
        serde_json::from_slice(bytes)
            .map(Self)
            .ok()
            .ok_or(EsiError::BadToken("key set"))
    }
}

/// What an access token says about the character it was issued for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claims {
    pub character_id: u64,
    pub name: String,
    pub scopes: BTreeSet<String>,
    pub expires: i64,
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    kid: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    fn contains(&self, wanted: &str) -> bool {
        match self {
            Self::One(value) => value == wanted,
            Self::Many(values) => values.iter().any(|value| value == wanted),
        }
    }

    fn into_set(self) -> BTreeSet<String> {
        match self {
            Self::One(value) => BTreeSet::from([value]),
            Self::Many(values) => values.into_iter().collect(),
        }
    }
}

#[derive(Deserialize)]
struct Raw {
    sub: String,
    name: String,
    exp: i64,
    iss: String,
    aud: OneOrMany,
    scp: Option<OneOrMany>,
}

/// Accepts the token only if it is signed by a known RS256 or ES256 key, is for this app and EVE, and has not expired (with a minute of leeway).
pub fn verify(token: &str, keys: &Jwks, client_id: &str, now: i64) -> Result<Claims, EsiError> {
    let header = header(token)?;
    let algorithm = match header.alg.as_str() {
        "RS256" => Algorithm::RS256,
        "ES256" => Algorithm::ES256,
        _ => return Err(EsiError::BadToken("algorithm")),
    };
    let kid = header.kid.ok_or(EsiError::BadToken("unknown key"))?;
    let jwk = keys.0.find(&kid).ok_or(EsiError::BadToken("unknown key"))?;
    let key = DecodingKey::from_jwk(jwk)
        .ok()
        .ok_or(EsiError::BadToken("key"))?;
    // Time, audience and issuer are checked below with the caller's clock, so the library's own checks are off.
    let mut validation = Validation::new(algorithm);
    validation.validate_exp = false;
    validation.validate_aud = false;
    validation.required_spec_claims.clear();
    let decoded = jsonwebtoken::decode::<Raw>(token, &key, &validation).map_err(|error| {
        if matches!(error.kind(), ErrorKind::Json(_)) {
            EsiError::BadToken("claims")
        } else {
            EsiError::BadToken("signature")
        }
    })?;
    let raw = decoded.claims;

    if !ISSUERS.contains(&raw.iss.as_str()) {
        return Err(EsiError::BadToken("issuer"));
    }
    if !raw.aud.contains(client_id) || !raw.aud.contains(AUDIENCE) {
        return Err(EsiError::BadToken("audience"));
    }
    if !raw.exp.checked_add(LEEWAY).is_some_and(|limit| limit > now) {
        return Err(EsiError::BadToken("expiry"));
    }
    let character_id = raw
        .sub
        .strip_prefix(SUBJECT_PREFIX)
        .filter(|id| id.bytes().all(|byte| byte.is_ascii_digit()) && !id.starts_with('0'))
        .and_then(|id| id.parse::<u64>().ok())
        .ok_or(EsiError::BadToken("subject"))?;
    Ok(Claims {
        character_id,
        name: raw.name,
        scopes: raw.scp.map(OneOrMany::into_set).unwrap_or_default(),
        expires: raw.exp,
    })
}

fn header(token: &str) -> Result<Header, EsiError> {
    let first = token.split('.').next().unwrap_or_default();
    let bytes = BASE64_URL_SAFE_NO_PAD
        .decode(first)
        .ok()
        .ok_or(EsiError::BadToken("header"))?;
    serde_json::from_slice(&bytes)
        .ok()
        .ok_or(EsiError::BadToken("header"))
}
