use std::fmt;

/// Why a market request or a price read failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarketError {
    /// The request never got a response (DNS, connection, timeout).
    Network(String),
    /// ESI answered with a status that is neither success nor "not modified".
    Http { status: u16 },
    /// ESI asked us to slow down; nothing is requested again before `retry_after_secs` have passed.
    RateLimited { retry_after_secs: u64 },
    /// The body is not the JSON we expect.
    Json(String),
    /// An order price is not a decimal with at most four places.
    BadPrice { order_id: u64, text: String },
    /// A response header is missing or unreadable.
    BadHeader { name: &'static str, value: String },
    /// A price times a quantity does not fit the supported range.
    Overflow,
}

impl fmt::Display for MarketError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network(reason) => write!(f, "network request failed: {reason}"),
            Self::Http { status } => write!(f, "ESI answered with status {status}"),
            Self::RateLimited { retry_after_secs } => {
                write!(
                    f,
                    "ESI rate limit reached; retry in {retry_after_secs} seconds"
                )
            }
            Self::Json(reason) => write!(f, "ESI sent unreadable JSON: {reason}"),
            Self::BadPrice { order_id, text } => {
                write!(f, "order {order_id} has an unreadable price {text:?}")
            }
            Self::BadHeader { name, value } => {
                write!(f, "ESI sent an unreadable {name} header {value:?}")
            }
            Self::Overflow => write!(
                f,
                "the price times the quantity does not fit the supported range"
            ),
        }
    }
}

impl std::error::Error for MarketError {}
