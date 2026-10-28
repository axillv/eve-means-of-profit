//! The game's static data: item names, volumes and blueprint recipes, kept in a local database.
//!
//! [`import`] reads a download and writes a fresh database. [`Sde`] answers queries from it.

pub mod decryptor;
pub mod error;
pub mod facility;
pub mod fetch;
pub mod import;
pub mod invention;
pub mod map;
pub mod query;
pub mod schema;
pub mod stations;

pub use error::SdeError;
pub use facility::{
    Facility, FacilityError, FacilityNeed, Rig, Security, Structure, StructureKind,
};
pub use fetch::{
    Fetch, FetchError, HttpFetch, LATEST_URL, download_and_import, latest_build, needs_update,
    update_due, zip_url,
};
pub use import::{ImportReport, import};
pub use invention::{InventionActivity, InventionProduct, SkillRequirement};
pub use map::{MapError, RouteKind, SolarSystem, StarMap};
pub use query::{Item, Sde, SdeBuild};
pub use stations::Station;
