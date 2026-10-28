//! The market client for tests that never read prices: every request fails, as on a machine with
//! no network.

use mop_market::error::MarketError;
use mop_market::fuzzwork::{BytesHttp, BytesResponse};

/// Prices that cannot be reached: every request fails, as on a machine with no network.
pub(crate) struct Offline;

impl BytesHttp for Offline {
    fn get_bytes(
        &self,
        _url: &str,
        _if_none_match: Option<&str>,
    ) -> Result<BytesResponse, MarketError> {
        Err(MarketError::Network("offline in tests".to_string()))
    }
}
