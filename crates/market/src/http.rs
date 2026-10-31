//! The real HTTP client: ESI over HTTPS, through `ureq`.

use std::str::FromStr;
use std::time::Duration;

use crate::cache::{Http, HttpResponse};
use crate::error::MarketError;
use crate::fuzzwork::{BytesHttp, BytesResponse};

const TIMEOUT: Duration = Duration::from_secs(20);
/// The whole-region file is several megabytes, so its download gets more time.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);
/// The largest body read for a download.
const MAX_DOWNLOAD_BYTES: u64 = 64 * 1024 * 1024;
const COMPATIBILITY_DATE: &str = "2025-12-16";

/// ESI over HTTPS. Statuses other than success come back as responses, not errors, so the cache can decide.
pub struct UreqHttp {
    agent: ureq::Agent,
    download_agent: ureq::Agent,
}

impl UreqHttp {
    pub fn new() -> Self {
        Self {
            agent: agent(TIMEOUT),
            download_agent: agent(DOWNLOAD_TIMEOUT),
        }
    }
}

fn agent(timeout: Duration) -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .user_agent(mop_core::user_agent::USER_AGENT)
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build();
    ureq::Agent::new_with_config(config)
}

impl Default for UreqHttp {
    fn default() -> Self {
        Self::new()
    }
}

impl Http for UreqHttp {
    fn get(&self, url: &str, if_none_match: Option<&str>) -> Result<HttpResponse, MarketError> {
        let mut request = self
            .agent
            .get(url)
            .header("X-Compatibility-Date", COMPATIBILITY_DATE);
        if let Some(etag) = if_none_match {
            request = request.header("If-None-Match", etag);
        }
        let mut response = request
            .call()
            .map_err(|error| MarketError::Network(error.to_string()))?;
        let status = response.status().as_u16();
        let etag = header(response.headers(), "etag");
        let last_modified = header(response.headers(), "last-modified");
        let expires = header(response.headers(), "expires");
        let pages = pages_for(status, header(response.headers(), "x-pages"))?;
        // An unreadable `retry-after` counts as absent, so a rate-limit answer still starts the default wait.
        let retry_after =
            header(response.headers(), "retry-after").and_then(|text| text.parse().ok());
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|error| MarketError::Network(error.to_string()))?;
        Ok(HttpResponse {
            status,
            etag,
            last_modified,
            expires,
            pages,
            retry_after,
            body,
        })
    }
}

impl BytesHttp for UreqHttp {
    fn get_bytes(
        &self,
        url: &str,
        if_none_match: Option<&str>,
    ) -> Result<BytesResponse, MarketError> {
        let mut request = self
            .download_agent
            .get(url)
            .header("X-Compatibility-Date", COMPATIBILITY_DATE);
        if let Some(etag) = if_none_match {
            request = request.header("If-None-Match", etag);
        }
        let mut response = request
            .call()
            .map_err(|error| MarketError::Network(error.to_string()))?;
        let status = response.status().as_u16();
        let etag = header(response.headers(), "etag");
        let last_modified = header(response.headers(), "last-modified");
        let retry_after =
            header(response.headers(), "retry-after").and_then(|text| text.parse().ok());
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_DOWNLOAD_BYTES)
            .read_to_vec()
            .map_err(|error| download_error(&error))?;
        Ok(BytesResponse {
            status,
            etag,
            last_modified,
            retry_after,
            body,
        })
    }
}

/// A response header as text, when present and valid UTF-8.
fn header(headers: &ureq::http::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

/// A body over the download limit is [`MarketError::TooLarge`]; any other read failure is a network error.
fn download_error(error: &ureq::Error) -> MarketError {
    if let ureq::Error::BodyExceedsLimit(_) = error {
        MarketError::TooLarge
    } else {
        MarketError::Network(error.to_string())
    }
}

/// The page count, read only from a 200. Other statuses never use it, so a bad value must not hide the status.
fn pages_for(status: u16, value: Option<String>) -> Result<Option<u32>, MarketError> {
    if status == 200 {
        optional_number("x-pages", value)
    } else {
        Ok(None)
    }
}

/// A header that holds a number, or `None` when absent. A present header that is not a number is an error.
fn optional_number<T: FromStr>(
    name: &'static str,
    value: Option<String>,
) -> Result<Option<T>, MarketError> {
    match value {
        None => Ok(None),
        Some(text) => match text.parse::<T>() {
            Ok(number) => Ok(Some(number)),
            Err(_) => Err(MarketError::BadHeader { name, value: text }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_DOWNLOAD_BYTES, header, optional_number, pages_for};
    use crate::error::MarketError;

    #[test]
    fn the_download_limit_is_64_megabytes() {
        assert_eq!(MAX_DOWNLOAD_BYTES, 64 * 1024 * 1024);
    }

    #[test]
    fn a_header_is_read_as_text_when_present() {
        let mut headers = ureq::http::HeaderMap::new();
        headers.insert("etag", ureq::http::HeaderValue::from_static("\"a1\""));
        assert_eq!(header(&headers, "etag"), Some("\"a1\"".to_owned()));
        assert_eq!(header(&headers, "last-modified"), None);
    }

    #[test]
    fn a_numeric_header_is_read() {
        assert_eq!(
            optional_number::<u32>("x-pages", Some("3".to_owned())),
            Ok(Some(3))
        );
        assert_eq!(
            optional_number::<u64>("retry-after", Some("30".to_owned())),
            Ok(Some(30))
        );
    }

    #[test]
    fn an_absent_header_is_none() {
        assert_eq!(optional_number::<u32>("x-pages", None), Ok(None));
    }

    #[test]
    fn a_header_that_is_not_a_number_is_an_error() {
        assert_eq!(
            optional_number::<u32>("x-pages", Some("many".to_owned())),
            Err(MarketError::BadHeader {
                name: "x-pages",
                value: "many".to_owned(),
            })
        );
    }

    #[test]
    fn a_bad_page_count_is_an_error_on_a_200() {
        assert_eq!(pages_for(200, Some("5".to_owned())), Ok(Some(5)));
        assert_eq!(
            pages_for(200, Some("many".to_owned())),
            Err(MarketError::BadHeader {
                name: "x-pages",
                value: "many".to_owned(),
            })
        );
    }

    #[test]
    fn a_bad_page_count_is_ignored_on_other_statuses() {
        for status in [304, 420, 429, 503] {
            assert_eq!(pages_for(status, Some("many".to_owned())), Ok(None));
            assert_eq!(pages_for(status, Some("5".to_owned())), Ok(None));
        }
    }
}
