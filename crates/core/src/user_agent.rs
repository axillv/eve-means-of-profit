//! The identity every outgoing request carries, as CCP asks: app name and version, a way to reach
//! the maintainer, and where the source lives. This is the only place that text is written; every
//! HTTP client takes [`USER_AGENT`] from here. A new maintainer changes the contact lines below.

/// The app name sent as the product token.
pub const APP_NAME: &str = "eve-means-of-profit";

/// Email CCP can write to if the app misbehaves.
pub const CONTACT_EMAIL: &str = "tyronevedenker@gmail.com";

/// Discord name CCP can message.
pub const CONTACT_DISCORD: &str = "axillv";

/// Where the source lives.
pub const SOURCE_URL: &str = "https://github.com/axillv/eve-means-of-profit";

/// `name/version (email; Discord: name; +source)`, built at compile time.
pub const USER_AGENT: &str = concat!(
    "eve-means-of-profit/",
    env!("CARGO_PKG_VERSION"),
    " (tyronevedenker@gmail.com; Discord: axillv; +https://github.com/axillv/eve-means-of-profit)"
);

#[cfg(test)]
mod tests {
    use super::*;

    /// `concat!` cannot take constants, so the string repeats the fields; this keeps them equal.
    #[test]
    fn user_agent_is_built_from_the_fields() {
        let expected = format!(
            "{APP_NAME}/{} ({CONTACT_EMAIL}; Discord: {CONTACT_DISCORD}; +{SOURCE_URL})",
            env!("CARGO_PKG_VERSION")
        );
        assert_eq!(USER_AGENT, expected);
    }

    #[test]
    fn user_agent_is_a_valid_header_value() {
        assert!(USER_AGENT.is_ascii());
        assert!(USER_AGENT.bytes().all(|b| (0x20..0x7f).contains(&b)));
    }
}
