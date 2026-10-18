//! The real HTTP client: ESI over HTTPS, through `ureq`.

use std::str::FromStr;
use std::time::Duration;

use crate::cache::{Http, HttpResponse};
use crate::error::MarketError;

const TIMEOUT: Duration = Duration::from_secs(20);
const COMPATIBILITY_DATE: &str = "2025-12-16";

/// ESI over HTTPS. Statuses other than success come back as responses, not errors, so the cache can decide.
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
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        };
        let etag = header("etag");
        let last_modified = header("last-modified");
        let expires = header("expires");
        let pages = pages_for(status, header("x-pages"))?;
        // An unreadable `retry-after` counts as absent, so a rate-limit answer still starts the default wait.
        let retry_after = header("retry-after").and_then(|text| text.parse().ok());
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
    use super::{optional_number, pages_for};
    use crate::error::MarketError;

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
