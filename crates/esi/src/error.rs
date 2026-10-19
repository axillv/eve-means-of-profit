use std::fmt;
use std::path::{Path, PathBuf};

/// Why a login step failed. Tokens and answer bodies never appear here.
#[derive(Debug)]
pub enum EsiError {
    /// The request never got a response (DNS, connection, timeout).
    Network(String),
    /// A file could not be read or written.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The app has no EVE client id yet, so no login can start.
    NoClientId,
    /// The callback port is taken, so the browser cannot reach the app.
    PortInUse(u16),
    /// The player or the app stopped the wait before the answer came.
    Cancelled,
    /// No answer came within the wait.
    TimedOut,
    /// The answer belongs to another login than the one the app started.
    StateMismatch,
    /// EVE's page was closed with an error; the text is the description EVE gave.
    Denied(String),
    /// The character's login is no longer valid; the player must add it again.
    LoginExpired,
    /// EVE's login server refused a request.
    Sso { status: u16, body: String },
    /// EVE's login server failed.
    Server(u16),
    /// A token or key from EVE failed a check.
    BadToken(&'static str),
    /// A saved file is unreadable or has a version this app does not know.
    BadFile { path: PathBuf, reason: String },
    /// The system keychain failed.
    Keychain(String),
    /// No character with this id is added.
    UnknownCharacter(u64),
    /// The system could not give random bytes for a login.
    Random(String),
    /// EVE refused the request: the character has not given the app this permission, or the login was revoked.
    Forbidden { route: String },
    /// EVE has nothing at this address.
    NotFound { route: String },
    /// Too many failed requests from the app; EVE asks it to send nothing until the reset.
    ErrorLimited { reset_seconds: u64 },
    /// EVE is limiting this character's requests of this kind; retry after the given seconds.
    RateLimited { retry_after_seconds: u64 },
    /// EVE answered with a status this app does not expect.
    Unexpected { status: u16, route: String },
    /// The pages of one list changed while they were being read.
    Inconsistent { route: String },
    /// EVE sent a value that cannot be read; `text` is the value as sent.
    BadData {
        route: String,
        field: &'static str,
        text: String,
    },
}

impl EsiError {
    pub(crate) fn bad_file(path: &Path, reason: impl Into<String>) -> Self {
        Self::BadFile {
            path: path.to_owned(),
            reason: reason.into(),
        }
    }
}

impl fmt::Display for EsiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network(reason) => write!(f, "could not reach EVE's servers: {reason}"),
            Self::Io { path, source } => {
                write!(f, "could not read or write {}: {source}", path.display())
            }
            Self::NoClientId => write!(f, "the app has no EVE client id yet"),
            Self::PortInUse(port) => write!(
                f,
                "port {port} is in use; close the program using it and try again"
            ),
            Self::Cancelled => write!(f, "the login was cancelled"),
            Self::TimedOut => write!(f, "the login timed out; try again"),
            Self::StateMismatch => write!(
                f,
                "the answer did not belong to the login the app started; try again"
            ),
            Self::Denied(description) => write!(f, "EVE did not allow the login: {description}"),
            Self::LoginExpired => write!(f, "the login has expired; add the character again"),
            Self::Sso { status, body } => {
                write!(f, "EVE's login server answered {status}: {body}")
            }
            Self::Server(status) => write!(f, "EVE's login server failed with status {status}"),
            Self::BadToken(check) => {
                write!(f, "EVE sent a login token that failed the {check} check")
            }
            Self::BadFile { path, reason } => {
                write!(f, "{} cannot be read: {reason}", path.display())
            }
            Self::Keychain(reason) => write!(f, "the system keychain failed: {reason}"),
            Self::UnknownCharacter(id) => write!(f, "no character with id {id} is added"),
            Self::Random(reason) => write!(f, "could not get random bytes: {reason}"),
            Self::Forbidden { route } => write!(
                f,
                "EVE refused {route}: the character has not given the app this permission, or the login was revoked"
            ),
            Self::NotFound { route } => write!(f, "EVE has nothing at {route}"),
            Self::ErrorLimited { reset_seconds } => write!(
                f,
                "EVE asked the app to stop for {reset_seconds} s after too many failed requests"
            ),
            Self::RateLimited {
                retry_after_seconds,
            } => write!(
                f,
                "EVE is limiting this character's requests; try again in {retry_after_seconds} s"
            ),
            Self::Unexpected { status, route } => write!(f, "EVE answered {status} to {route}"),
            Self::Inconsistent { route } => write!(
                f,
                "EVE's pages for {route} changed while they were read; try again"
            ),
            Self::BadData { route, field, text } => {
                write!(f, "EVE sent an unreadable {field} for {route}: {text}")
            }
        }
    }
}

impl std::error::Error for EsiError {}
