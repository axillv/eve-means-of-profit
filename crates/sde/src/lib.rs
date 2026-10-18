//! The game's static data: item names, volumes and blueprint recipes, kept in a local database.
//!
//! [`import`] reads a download and writes a fresh database. [`Sde`] answers queries from it.

pub mod error;
pub mod fetch;
pub mod import;
pub mod query;
pub mod schema;

pub use error::SdeError;
pub use fetch::{
    Fetch, FetchError, HttpFetch, LATEST_URL, download_and_import, latest_build, needs_update,
    update_due, zip_url,
};
pub use import::{ImportReport, import};
pub use query::{Item, Sde, SdeBuild};
