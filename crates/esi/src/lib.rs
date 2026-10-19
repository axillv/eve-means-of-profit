//! EVE single sign-on for the app: add characters through EVE's login page, keep them logged in, and hand out access tokens for ESI calls.

pub mod auth;
pub mod callback;
pub mod character_data;
pub mod characters;
pub mod client;
pub mod error;
pub mod http;
pub mod industry;
pub mod jwt;
pub mod pkce;
pub mod saved;
pub mod secrets;
pub mod token;
