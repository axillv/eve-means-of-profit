//! The PKCE pair (RFC 7636) and the state value that ties the browser answer to this login.

use base64::Engine as _;
use base64::prelude::BASE64_URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

use crate::error::EsiError;

#[derive(Clone)]
pub struct Pkce {
    verifier: String,
    state: String,
}

impl Pkce {
    /// A fresh pair from the system's random source.
    pub fn new() -> Result<Self, EsiError> {
        let mut verifier = [0; 32];
        let mut state = [0; 16];
        getrandom::fill(&mut verifier).map_err(|error| EsiError::Random(error.to_string()))?;
        getrandom::fill(&mut state).map_err(|error| EsiError::Random(error.to_string()))?;
        Ok(Self::from_bytes(verifier, state))
    }

    pub fn from_bytes(verifier: [u8; 32], state: [u8; 16]) -> Self {
        Self {
            verifier: BASE64_URL_SAFE_NO_PAD.encode(verifier),
            state: BASE64_URL_SAFE_NO_PAD.encode(state),
        }
    }

    pub fn verifier(&self) -> &str {
        &self.verifier
    }

    pub fn state(&self) -> &str {
        &self.state
    }

    /// The S256 challenge: base64url of the SHA-256 of the verifier's text.
    pub fn challenge(&self) -> String {
        BASE64_URL_SAFE_NO_PAD.encode(Sha256::digest(self.verifier.as_bytes()))
    }
}
