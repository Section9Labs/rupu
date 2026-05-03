//! What lives inside a single keychain entry.
//!
//! Slice B-1 spec §4b: provider adapters never see the refresh token.
//! `StoredCredential` is what the resolver writes to the keychain; the
//! resolver materializes a `Credentials` for adapters from this.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use rupu_providers::auth::AuthCredentials;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredCredential {
    /// The on-the-wire creds the provider adapter consumes.
    pub credentials: AuthCredentials,
    /// Refresh token, if SSO. None for API-key entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// When the access token expires (UTC). None means non-expiring (API key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

impl StoredCredential {
    pub fn api_key(key: impl Into<String>) -> Self {
        Self {
            credentials: AuthCredentials::ApiKey { key: key.into() },
            refresh_token: None,
            expires_at: None,
        }
    }

    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        match self.expires_at {
            Some(exp) => exp <= now,
            None => false,
        }
    }

    pub fn is_near_expiry(&self, now: DateTime<Utc>, buffer_secs: i64) -> bool {
        match self.expires_at {
            Some(exp) => (exp - chrono::Duration::seconds(buffer_secs)) <= now,
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_key_constructor_has_no_refresh_or_expiry() {
        let s = StoredCredential::api_key("sk-test");
        assert!(s.refresh_token.is_none());
        assert!(s.expires_at.is_none());
        assert!(matches!(s.credentials, AuthCredentials::ApiKey { .. }));
    }

    #[test]
    fn json_roundtrip() {
        let s = StoredCredential::api_key("sk-test");
        let json = serde_json::to_string(&s).unwrap();
        let back: StoredCredential = serde_json::from_str(&json).unwrap();
        match back.credentials {
            AuthCredentials::ApiKey { key } => assert_eq!(key, "sk-test"),
            _ => panic!(),
        }
    }

    #[test]
    fn near_expiry_window_correct() {
        let exp = Utc::now() + chrono::Duration::seconds(30);
        let s = StoredCredential {
            credentials: AuthCredentials::ApiKey { key: "x".into() },
            refresh_token: None,
            expires_at: Some(exp),
        };
        // 60-second buffer means 30s-from-now is "near".
        assert!(s.is_near_expiry(Utc::now(), 60));
        // 10-second buffer means 30s-from-now is NOT near.
        assert!(!s.is_near_expiry(Utc::now(), 10));
    }
}
