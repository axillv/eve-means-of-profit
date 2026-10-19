//! The HTTP calls the login makes, behind a trait so tests can replay canned answers.

use std::time::Duration;

use ureq::RequestBuilder;

use crate::error::EsiError;

const TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

impl HttpRequest {
    pub fn get(url: &str) -> Self {
        Self {
            method: Method::Get,
            url: url.to_owned(),
            headers: Vec::new(),
            body: None,
        }
    }

    /// A POST whose body is a form: the fields are percent-encoded and joined with `&`.
    pub fn post_form(url: &str, fields: &[(&str, &str)]) -> Self {
        let body = fields
            .iter()
            .map(|(key, value)| format!("{key}={}", encode(value)))
            .collect::<Vec<_>>()
            .join("&");
        Self {
            method: Method::Post,
            url: url.to_owned(),
            headers: vec![(
                "Content-Type".to_owned(),
                "application/x-www-form-urlencoded".to_owned(),
            )],
            body: Some(body.into_bytes()),
        }
    }
}

/// Response header names are lower case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

pub trait Http: Send {
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, EsiError>;
}

/// The real client. Statuses other than success come back as responses, not errors.
pub struct UreqHttp {
    agent: ureq::Agent,
}

impl UreqHttp {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .user_agent(concat!("eve-means-of-profit/", env!("CARGO_PKG_VERSION")))
            .timeout_global(Some(TIMEOUT))
            .http_status_as_error(false)
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
        }
    }
}

impl Default for UreqHttp {
    fn default() -> Self {
        Self::new()
    }
}

impl Http for UreqHttp {
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, EsiError> {
        let result = match request.method {
            Method::Get => with_headers(self.agent.get(&request.url), &request.headers).call(),
            Method::Post => {
                let body = request.body.as_deref().unwrap_or_default();
                with_headers(self.agent.post(&request.url), &request.headers).send(body)
            }
        };
        let mut response = result.map_err(|error| EsiError::Network(error.to_string()))?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|text| (name.as_str().to_owned(), text.to_owned()))
            })
            .collect();
        let body = response
            .body_mut()
            .read_to_vec()
            .map_err(|error| EsiError::Network(error.to_string()))?;
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

fn with_headers<B>(
    mut builder: RequestBuilder<B>,
    headers: &[(String, String)],
) -> RequestBuilder<B> {
    for (name, value) in headers {
        builder = builder.header(name.as_str(), value.as_str());
    }
    builder
}

/// Percent-encodes for a URL or a form: letters, digits and `-._~` stay, every other byte becomes `%XX`.
pub fn encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Reverses [`encode`]. `None` when an escape is incomplete or the result is not UTF-8.
pub fn decode(text: &str) -> Option<String> {
    let mut out = Vec::with_capacity(text.len());
    let mut bytes = text.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let high = bytes.next().and_then(hex_digit)?;
            let low = bytes.next().and_then(hex_digit)?;
            out.push(high * 16 + low);
        } else {
            out.push(byte);
        }
    }
    String::from_utf8(out).ok()
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
