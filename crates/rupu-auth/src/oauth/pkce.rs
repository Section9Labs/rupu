//! PKCE (RFC 7636) helpers — base64url-no-pad encoded random verifier
//! plus its S256 challenge.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub struct PkcePair {
    pub verifier: String,
    pub challenge: String,
    pub method: &'static str,
}

impl PkcePair {
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        let verifier = URL_SAFE_NO_PAD.encode(bytes);
        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());
        Self {
            verifier,
            challenge,
            method: "S256",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_produces_valid_lengths() {
        let p = PkcePair::generate();
        assert!(
            p.verifier.len() >= 43,
            "verifier too short: {}",
            p.verifier.len()
        );
        assert!(p.verifier.len() <= 128, "verifier too long");
        assert_eq!(p.challenge.len(), 43); // 32 bytes -> 43 char base64url
        assert_eq!(p.method, "S256");
    }

    #[test]
    fn challenge_matches_verifier_hash() {
        let p = PkcePair::generate();
        let mut h = Sha256::new();
        h.update(p.verifier.as_bytes());
        let expected = URL_SAFE_NO_PAD.encode(h.finalize());
        assert_eq!(p.challenge, expected);
    }

    #[test]
    fn each_call_is_unique() {
        let a = PkcePair::generate();
        let b = PkcePair::generate();
        assert_ne!(a.verifier, b.verifier);
    }
}
