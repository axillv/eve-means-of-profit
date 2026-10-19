//! The requests the character data makes to ESI: the token, the compatibility date, EVE's answers
//! mapped to errors, and the pauses EVE asks for. Tests replay canned answers through `Http`.

use std::collections::BTreeMap;
use std::time::UNIX_EPOCH;

use serde_json::Value;

use crate::error::EsiError;
use crate::http::{Http, HttpRequest, HttpResponse, Method};
use crate::token::AccessToken;

pub const BASE_URL: &str = "https://esi.evetech.net";
const COMPATIBILITY_DATE: &str = "2025-12-16";
/// When EVE's error budget has fewer failed requests left than this, nothing more is sent until the reset.
const ERROR_MARGIN: u64 = 10;
/// The wait used when EVE's reply gives no reset or retry time, or one that cannot be read.
const DEFAULT_WAIT_SECONDS: u64 = 60;
const BODY_IN_ERROR: usize = 80;

/// A parsed 200 answer and its headers. Header names are lower case.
pub struct Answer {
    pub body: Value,
    headers: Vec<(String, String)>,
}

impl Answer {
    pub fn header(&self, name: &str) -> Option<&str> {
        header_of(&self.headers, name)
    }

    /// An HTTP date header as Unix seconds; `None` when the header is absent.
    pub fn http_date(&self, name: &'static str, route: &str) -> Result<Option<i64>, EsiError> {
        let Some(text) = self.header(name) else {
            return Ok(None);
        };
        httpdate::parse_http_date(text)
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .and_then(|span| i64::try_from(span.as_secs()).ok())
            .map(Some)
            .ok_or_else(|| bad_data(route, name, text))
    }
}

/// Asks ESI through `Http`. Keeps the pauses EVE asked for, so every caller that shares one client obeys them.
pub struct Client<'a> {
    http: &'a dyn Http,
    /// Nothing is sent to any character until this Unix time, after EVE's error budget ran low.
    stopped_until: Option<i64>,
    /// One character's kind of data waits until this Unix time, after EVE's per-group limit.
    group_until: BTreeMap<String, i64>,
}

impl<'a> Client<'a> {
    pub fn new(http: &'a dyn Http) -> Self {
        Self {
            http,
            stopped_until: None,
            group_until: BTreeMap::new(),
        }
    }

    /// GET `path`, a path with an optional query and no host. `group` names one character and kind of data.
    pub fn get_json(
        &mut self,
        now: i64,
        token: &AccessToken,
        path: &str,
        group: &str,
    ) -> Result<Answer, EsiError> {
        let request = request(Method::Get, path, Some(token), None);
        self.send(now, &request, path, group)
    }

    /// GET `path` with no login, for public data. Same rules as [`Client::get_json`], without the `Authorization` header.
    pub fn get_public_json(
        &mut self,
        now: i64,
        path: &str,
        group: &str,
    ) -> Result<Answer, EsiError> {
        let request = request(Method::Get, path, None, None);
        self.send(now, &request, path, group)
    }

    /// POST `body` to `path`, with the same rules as [`Client::get_json`].
    pub fn post_json(
        &mut self,
        now: i64,
        token: &AccessToken,
        path: &str,
        group: &str,
        body: &Value,
    ) -> Result<Answer, EsiError> {
        let bytes = serde_json::to_vec(body)
            .map_err(|error| bad_data(route_of(path), "request body", &error.to_string()))?;
        let request = request(Method::Post, path, Some(token), Some(bytes));
        self.send(now, &request, path, group)
    }

    fn send(
        &mut self,
        now: i64,
        request: &HttpRequest,
        path: &str,
        group: &str,
    ) -> Result<Answer, EsiError> {
        let route = route_of(path);
        if let Some(until) = self.stopped_until.filter(|until| now < *until) {
            return Err(EsiError::ErrorLimited {
                reset_seconds: wait(until, now),
            });
        }
        if let Some(until) = self
            .group_until
            .get(group)
            .copied()
            .filter(|until| now < *until)
        {
            return Err(EsiError::RateLimited {
                retry_after_seconds: wait(until, now),
            });
        }
        let HttpResponse {
            status,
            headers,
            body,
        } = self.http.send(request)?;
        let reset =
            number_header(&headers, "x-esi-error-limit-reset").unwrap_or(DEFAULT_WAIT_SECONDS);
        let remain = number_header(&headers, "x-esi-error-limit-remain");
        if status == 420 || remain.is_some_and(|left| left < ERROR_MARGIN) {
            self.stopped_until = Some(pause_until(now, reset, route, "x-esi-error-limit-reset")?);
        }
        match status {
            200 => {
                let body: Value = serde_json::from_slice(&body)
                    .ok()
                    .ok_or_else(|| bad_data(route, "body", &preview(&body)))?;
                Ok(Answer { body, headers })
            }
            420 => Err(EsiError::ErrorLimited {
                reset_seconds: reset,
            }),
            429 => {
                let wait = number_header(&headers, "retry-after").unwrap_or(DEFAULT_WAIT_SECONDS);
                self.group_until.insert(
                    group.to_owned(),
                    pause_until(now, wait, route, "retry-after")?,
                );
                Err(EsiError::RateLimited {
                    retry_after_seconds: wait,
                })
            }
            403 => Err(EsiError::Forbidden {
                route: route.to_owned(),
            }),
            404 => Err(EsiError::NotFound {
                route: route.to_owned(),
            }),
            500..=599 => Err(EsiError::Server(status)),
            _ => Err(EsiError::Unexpected {
                status,
                route: route.to_owned(),
            }),
        }
    }
}

/// A value read from a field EVE sent, with the route it came from, for errors.
pub(crate) fn bad_data(route: &str, field: &'static str, text: &str) -> EsiError {
    EsiError::BadData {
        route: route.to_owned(),
        field,
        text: text.to_owned(),
    }
}

fn request(
    method: Method,
    path: &str,
    token: Option<&AccessToken>,
    body: Option<Vec<u8>>,
) -> HttpRequest {
    let mut headers = vec![("Accept".to_owned(), "application/json".to_owned())];
    if let Some(token) = token {
        headers.push((
            "Authorization".to_owned(),
            format!("Bearer {}", token.as_str()),
        ));
    }
    headers.push((
        "X-Compatibility-Date".to_owned(),
        COMPATIBILITY_DATE.to_owned(),
    ));
    if body.is_some() {
        headers.push(("Content-Type".to_owned(), "application/json".to_owned()));
    }
    HttpRequest {
        method,
        url: format!("{BASE_URL}{path}"),
        headers,
        body,
    }
}

/// The path without its query: the name used in errors.
fn route_of(path: &str) -> &str {
    path.split('?').next().unwrap_or(path)
}

fn header_of<'h>(headers: &'h [(String, String)], name: &str) -> Option<&'h str> {
    headers
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// A header read as a whole number; a missing or unreadable one is `None`.
fn number_header(headers: &[(String, String)], name: &str) -> Option<u64> {
    header_of(headers, name).and_then(|text| text.trim().parse().ok())
}

/// The Unix time `seconds` after `now`, or an error when that does not fit a time.
fn pause_until(now: i64, seconds: u64, route: &str, field: &'static str) -> Result<i64, EsiError> {
    i64::try_from(seconds)
        .ok()
        .and_then(|seconds| now.checked_add(seconds))
        .ok_or_else(|| bad_data(route, field, &seconds.to_string()))
}

/// Seconds from `now` until `until`; only called when `now` is before `until`.
fn wait(until: i64, now: i64) -> u64 {
    until
        .checked_sub(now)
        .and_then(|seconds| u64::try_from(seconds).ok())
        .unwrap_or(0)
}

/// The start of a body for an error message, so a long answer is not copied whole.
fn preview(body: &[u8]) -> String {
    String::from_utf8_lossy(body)
        .chars()
        .take(BODY_IN_ERROR)
        .collect()
}
