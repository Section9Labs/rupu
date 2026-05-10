//! Webhook signature validation for GitHub + GitLab + Linear.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SignatureError {
    #[error("missing signature header")]
    Missing,
    #[error("malformed signature header: {0}")]
    Malformed(String),
    #[error("signature mismatch")]
    Mismatch,
    #[error("stale timestamp")]
    StaleTimestamp,
}

type HmacSha256 = Hmac<Sha256>;

/// Verify a GitHub webhook signature. The `signature_header` is the
/// raw value of `x-hub-signature-256`, expected to start with
/// `sha256=` followed by a 64-char lowercase hex digest. Compares in
/// constant time to defeat timing oracles.
pub fn verify_github_signature(
    secret: &[u8],
    body: &[u8],
    signature_header: Option<&str>,
) -> Result<(), SignatureError> {
    let header = signature_header.ok_or(SignatureError::Missing)?;
    let hex_digest = header
        .strip_prefix("sha256=")
        .ok_or_else(|| SignatureError::Malformed("expected `sha256=...` prefix".into()))?;
    let provided = hex::decode(hex_digest)
        .map_err(|e| SignatureError::Malformed(format!("not valid hex: {e}")))?;

    let mut mac = <HmacSha256 as Mac>::new_from_slice(secret)
        .map_err(|e| SignatureError::Malformed(format!("hmac init: {e}")))?;
    mac.update(body);
    let expected = mac.finalize().into_bytes();

    if expected.as_slice().ct_eq(&provided).into() {
        Ok(())
    } else {
        Err(SignatureError::Mismatch)
    }
}

/// Verify a GitLab webhook token. GitLab's model is a shared-secret
/// comparison rather than HMAC: the value sent in `x-gitlab-token`
/// must match the secret configured for the project's webhook.
/// Constant-time comparison still — even though there's no HMAC,
/// the token itself is the secret being compared.
pub fn verify_gitlab_token(
    expected_token: &[u8],
    token_header: Option<&str>,
) -> Result<(), SignatureError> {
    let provided = token_header.ok_or(SignatureError::Missing)?;
    if expected_token.ct_eq(provided.as_bytes()).into() {
        Ok(())
    } else {
        Err(SignatureError::Mismatch)
    }
}

/// Verify a Linear webhook signature and freshness window. Linear
/// signs the exact raw request body with HMAC-SHA256 and delivers the
/// hex digest in the `Linear-Signature` header. The payload also
/// carries `webhookTimestamp` in milliseconds; we reject webhooks more
/// than 60 seconds away from local wall clock to reduce replay risk.
pub fn verify_linear_signature(
    secret: &[u8],
    body: &[u8],
    signature_header: Option<&str>,
    webhook_timestamp_ms: Option<i64>,
) -> Result<(), SignatureError> {
    let header = signature_header.ok_or(SignatureError::Missing)?;
    let provided = hex::decode(header)
        .map_err(|e| SignatureError::Malformed(format!("not valid hex: {e}")))?;

    let mut mac = <HmacSha256 as Mac>::new_from_slice(secret)
        .map_err(|e| SignatureError::Malformed(format!("hmac init: {e}")))?;
    mac.update(body);
    let expected = mac.finalize().into_bytes();
    if !bool::from(expected.as_slice().ct_eq(&provided)) {
        return Err(SignatureError::Mismatch);
    }

    let ts = webhook_timestamp_ms
        .ok_or_else(|| SignatureError::Malformed("missing webhookTimestamp in body".into()))?;
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default();
    let skew_ms = (now_ms - ts).abs();
    if skew_ms > 60_000 {
        return Err(SignatureError::StaleTimestamp);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_round_trip_passes() {
        let secret = b"It's a Secret to Everybody";
        let body = b"Hello, World!";
        // Pre-computed sha256 HMAC of the body with the secret —
        // value lifted from GitHub's docs example for parity.
        let sig = "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17";
        verify_github_signature(secret, body, Some(sig)).expect("expected match");
    }

    #[test]
    fn github_rejects_wrong_secret() {
        let body = b"Hello, World!";
        let sig = "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17";
        let err = verify_github_signature(b"different secret", body, Some(sig)).unwrap_err();
        assert!(matches!(err, SignatureError::Mismatch));
    }

    #[test]
    fn github_rejects_missing_header() {
        let err = verify_github_signature(b"k", b"body", None).unwrap_err();
        assert!(matches!(err, SignatureError::Missing));
    }

    #[test]
    fn github_rejects_malformed_header() {
        for bad in ["", "abcdef", "sha1=deadbeef", "sha256=zzz"] {
            let err = verify_github_signature(b"k", b"body", Some(bad)).unwrap_err();
            assert!(matches!(err, SignatureError::Malformed(_)), "for {bad}");
        }
    }

    #[test]
    fn gitlab_token_match() {
        verify_gitlab_token(b"shared-secret", Some("shared-secret")).expect("match");
    }

    #[test]
    fn gitlab_rejects_wrong_token() {
        let err = verify_gitlab_token(b"shared-secret", Some("nope")).unwrap_err();
        assert!(matches!(err, SignatureError::Mismatch));
    }

    #[test]
    fn linear_round_trip_passes() {
        let secret = b"linear-secret";
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let body = format!(r#"{{"type":"Issue","action":"update","webhookTimestamp":{ts}}}"#);
        let mut mac = <HmacSha256 as Mac>::new_from_slice(secret).unwrap();
        mac.update(body.as_bytes());
        let sig = hex::encode(mac.finalize().into_bytes());
        verify_linear_signature(secret, body.as_bytes(), Some(&sig), Some(ts)).expect("match");
    }

    #[test]
    fn linear_rejects_stale_timestamp() {
        let secret = b"linear-secret";
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
            - 120_000;
        let body = format!(r#"{{"type":"Issue","action":"update","webhookTimestamp":{ts}}}"#);
        let mut mac = <HmacSha256 as Mac>::new_from_slice(secret).unwrap();
        mac.update(body.as_bytes());
        let sig = hex::encode(mac.finalize().into_bytes());
        let err =
            verify_linear_signature(secret, body.as_bytes(), Some(&sig), Some(ts)).unwrap_err();
        assert!(matches!(err, SignatureError::StaleTimestamp));
    }
}
