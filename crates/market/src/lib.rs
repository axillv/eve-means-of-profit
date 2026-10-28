//! Jita prices from ESI: order books, price quotes and a cache that follows ESI's cache and rate-limit headers.
//! Forge summary prices from Fuzzwork's whole-region file, with the same kind of refresh rules.
//! No UI and no clock: callers pass the current time in.

pub mod book;
pub mod cache;
pub mod error;
pub mod fuzzwork;
pub mod history;
pub mod http;
pub mod plex;
