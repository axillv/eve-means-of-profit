//! Token requests to EVE's login server: code exchange, refresh and revoke.

use std::fmt;

use serde::Deserialize;

use crate::auth::{REVOKE_URL, TOKEN_URL};
use crate::error::EsiError;
use crate::http::{Http, HttpRequest, HttpResponse};

const MAX_BODY_IN_ERROR: usize = 300;

/// The long-lived login key. Its `Debug` output hides the text; there is no `Display`.
#[derive(Clone, PartialEq, Eq)]
pub struct RefreshToken(String);

impl RefreshToken {
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RefreshToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RefreshToken(…)")
    }
}

/// The short-lived key ESI calls use. Its `Debug` output hides the text; there is no `Display`.
#[derive(Clone, PartialEq, Eq)]
pub struct AccessToken(String);

impl AccessToken {
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AccessToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AccessToken(…)")
    }
}

/// The answer to a code exchange or a refresh; `expires` is the absolute time the access token ends.
pub struct Tokens {
    pub access: AccessToken,
    pub expires: i64,
    pub refresh: RefreshToken,
}

#[derive(Deserialize)]
struct Reply {
    access_token: String,
    expires_in: i64,
    refresh_token: String,
}

#[derive(Deserialize)]
struct Failure {
    error: Option<String>,
}

pub fn exchange_code(
    http: &dyn Http,
    client_id: &str,
    code: &str,
    verifier: &str,
    now: i64,
) -> Result<Tokens, EsiError> {
    let request = HttpRequest::post_form(
        TOKEN_URL,
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", client_id),
            ("code_verifier", verifier),
        ],
    );
    tokens_from(&http.send(&request)?, now)
}

pub fn refresh(
    http: &dyn Http,
    client_id: &str,
    refresh: &RefreshToken,
    now: i64,
) -> Result<Tokens, EsiError> {
    let request = HttpRequest::post_form(
        TOKEN_URL,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh.as_str()),
            ("client_id", client_id),
        ],
    );
    tokens_from(&http.send(&request)?, now)
}

/// Asks EVE to end the login. The caller decides what a failure means.
pub fn revoke(http: &dyn Http, client_id: &str, refresh: &RefreshToken) -> Result<(), EsiError> {
    let request = HttpRequest::post_form(
        REVOKE_URL,
        &[
            ("token_type_hint", "refresh_token"),
            ("token", refresh.as_str()),
            ("client_id", client_id),
        ],
    );
    http.send(&request).map(|_| ())
}

fn tokens_from(response: &HttpResponse, now: i64) -> Result<Tokens, EsiError> {
    let status = response.status;
    match status {
        200..=299 => {
            let reply: Reply = serde_json::from_slice(&response.body)
                .ok()
                .ok_or(EsiError::BadToken("token reply"))?;
            let expires = now
                .checked_add(reply.expires_in)
                .ok_or(EsiError::BadToken("token reply"))?;
            Ok(Tokens {
                access: AccessToken::new(reply.access_token),
                expires,
                refresh: RefreshToken::new(reply.refresh_token),
            })
        }
        400 | 401 if is_invalid_grant(&response.body) => Err(EsiError::LoginExpired),
        500..=599 => Err(EsiError::Server(status)),
        _ => Err(EsiError::Sso {
            status,
            body: String::from_utf8_lossy(&response.body)
                .chars()
                .take(MAX_BODY_IN_ERROR)
                .collect(),
        }),
    }
}

fn is_invalid_grant(body: &[u8]) -> bool {
    serde_json::from_slice::<Failure>(body)
        .is_ok_and(|failure| failure.error.as_deref() == Some("invalid_grant"))
}
