//! Shared process-local authentication for recorded-source cursors. The empty
//! domain preserves the existing transcript pager's byte-for-byte wire format.
use crate::error::{ApiError, ApiResult};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use serde::{de::DeserializeOwned, Serialize};
use sha2::Sha256;
use std::sync::OnceLock;

fn mac(domain: &str) -> Hmac<Sha256> {
    static KEY: OnceLock<[u8; 32]> = OnceLock::new();
    let secret = KEY.get_or_init(|| {
        let mut secret = [0; 32];
        for chunk in secret.chunks_mut(8) {
            chunk.copy_from_slice(&ulid::Ulid::new().to_bytes()[8..]);
        }
        secret
    });
    let mut signer = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts a 32-byte key");
    signer.update(domain.as_bytes());
    signer
}
pub(crate) fn signature(domain: &str, bytes: &[u8]) -> String {
    let mut signer = mac(domain);
    signer.update(bytes);
    URL_SAFE_NO_PAD.encode(signer.finalize().into_bytes())
}
pub(crate) fn encode<T: Serialize>(domain: &str, cursor: &T) -> ApiResult<String> {
    let bytes = serde_json::to_vec(cursor).map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(&bytes),
        signature(domain, &bytes)
    ))
}
pub(crate) fn decode<T: DeserializeOwned>(domain: &str, token: &str) -> ApiResult<T> {
    let invalid = || ApiError::bad_request("invalid recorded source cursor");
    if token.len() > 16 * 1024 {
        return Err(invalid());
    }
    let (payload, signature) = token.split_once('.').ok_or_else(invalid)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).map_err(|_| invalid())?;
    let signature = URL_SAFE_NO_PAD.decode(signature).map_err(|_| invalid())?;
    let mut verifier = mac(domain);
    verifier.update(&bytes);
    verifier.verify_slice(&signature).map_err(|_| invalid())?;
    serde_json::from_slice(&bytes).map_err(|_| invalid())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_format_and_domain_isolation() {
        let value = serde_json::json!({"offset":12,"scope":"owner"});
        let bytes = serde_json::to_vec(&value).unwrap();
        let mut legacy = mac("");
        legacy.update(&bytes);
        let expected = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(&bytes),
            URL_SAFE_NO_PAD.encode(legacy.finalize().into_bytes())
        );
        assert_eq!(encode("", &value).unwrap(), expected);
        assert_eq!(decode::<serde_json::Value>("", &expected).unwrap(), value);
        assert!(decode::<serde_json::Value>("coordination.history.v1\0", &expected).is_err());
        let current = encode("coordination.history.v1\0", &value).unwrap();
        assert!(decode::<serde_json::Value>("", &current).is_err());
        assert!(
            decode::<serde_json::Value>("coordination.history.v1\0", &(current + "x")).is_err()
        );
    }
}
