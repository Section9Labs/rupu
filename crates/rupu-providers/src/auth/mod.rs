pub mod credential_store;
pub mod discovery;

pub use credential_store::{resolve_provider_auth, save_provider_auth};

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

// Re-export Anthropic-specific auth for backward compatibility.
// Consumers use rupu_providers::auth::resolve_anthropic_auth etc.
pub use crate::anthropic::{refresh_anthropic_token, resolve_anthropic_auth, save_auth_json};

/// The OAuth beta header required for Anthropic OAuth requests.
pub const OAUTH_BETA_HEADER: &str = "oauth-2025-04-20";

/// 5-minute buffer before actual expiry — treat as expired early.
const EXPIRY_BUFFER_MS: u64 = 5 * 60 * 1000;

/// How authentication is performed for API requests.
/// Custom Debug redacts secrets to prevent accidental log exposure.
#[derive(Clone)]
pub enum AuthMethod {
    /// Standard API key via x-api-key header.
    ApiKey(String),
    /// OAuth token via Authorization: Bearer header.
    /// Requires anthropic-beta: oauth-2025-04-20 header.
    OAuth {
        access_token: String,
        refresh_token: String,
        expires_ms: u64,
    },
}

impl std::fmt::Debug for AuthMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApiKey(key) => {
                let redacted = if key.len() > 10 {
                    format!("{}****", &key[..10])
                } else {
                    "****".into()
                };
                f.debug_tuple("ApiKey").field(&redacted).finish()
            }
            Self::OAuth { expires_ms, .. } => f
                .debug_struct("OAuth")
                .field("access_token", &"****")
                .field("refresh_token", &"****")
                .field("expires_ms", expires_ms)
                .finish(),
        }
    }
}

impl AuthMethod {
    /// Detect auth method from a raw token string.
    /// OAuth tokens have the "sk-ant-oat" prefix.
    pub fn detect(token: &str) -> Self {
        if token.starts_with("sk-ant-oat") {
            Self::OAuth {
                access_token: token.to_string(),
                refresh_token: String::new(),
                expires_ms: 0,
            }
        } else {
            Self::ApiKey(token.to_string())
        }
    }

    pub fn is_oauth(&self) -> bool {
        matches!(self, Self::OAuth { .. })
    }
}

/// Credentials for a provider — either API key or OAuth.
/// Uses serde tagged enum for clean JSON: `{"type":"oauth",...}` or `{"type":"api_key",...}`.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AuthCredentials {
    #[serde(rename = "oauth")]
    OAuth {
        access: String,
        refresh: String,
        expires: u64,
        /// Provider-specific extra fields (e.g., Google project_id, Copilot enterprise_url).
        #[serde(flatten)]
        extra: HashMap<String, serde_json::Value>,
    },
    #[serde(rename = "api_key")]
    ApiKey { key: String },
}

/// Custom Debug redacts secrets to prevent accidental log exposure.
impl std::fmt::Debug for AuthCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OAuth { expires, extra, .. } => f
                .debug_struct("OAuth")
                .field("access", &"****")
                .field("refresh", &"****")
                .field("expires", expires)
                .field("extra_keys", &extra.keys().collect::<Vec<_>>())
                .finish(),
            Self::ApiKey { .. } => f.debug_tuple("ApiKey").field(&"****").finish(),
        }
    }
}

/// Reserved field names that must not appear in the OAuth `extra` HashMap.
/// These are used by the tagged enum discriminator and named fields.
const RESERVED_EXTRA_KEYS: &[&str] = &["type", "access", "refresh", "expires"];

impl AuthCredentials {
    /// Strip reserved field names from the OAuth extra HashMap to prevent
    /// serde(flatten) field collision on serialization roundtrip.
    pub fn sanitize_extra(&mut self) {
        if let AuthCredentials::OAuth { extra, .. } = self {
            for key in RESERVED_EXTRA_KEYS {
                extra.remove(*key);
            }
        }
    }

    /// Convert to Anthropic-specific AuthMethod.
    pub fn into_anthropic_auth_method(self) -> AuthMethod {
        match self {
            AuthCredentials::OAuth {
                access,
                refresh,
                expires,
                ..
            } => AuthMethod::OAuth {
                access_token: access,
                refresh_token: refresh,
                expires_ms: expires,
            },
            AuthCredentials::ApiKey { key } => AuthMethod::detect(&key),
        }
    }
}

/// The full auth.json structure — map of provider name to credentials.
pub type AuthFile = HashMap<String, AuthCredentials>;

/// Check if an OAuth token is expired (with 5-minute buffer).
pub fn is_token_expired(expires_ms: u64) -> bool {
    if expires_ms == 0 {
        return false; // No expiry info — assume valid
    }
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    now_ms >= expires_ms.saturating_sub(EXPIRY_BUFFER_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_oauth_token() {
        let auth = AuthMethod::detect("sk-ant-oat01-abc123");
        assert!(auth.is_oauth());
    }

    #[test]
    fn test_detect_api_key() {
        let auth = AuthMethod::detect("sk-ant-api03-abc123");
        assert!(!auth.is_oauth());
    }

    #[test]
    fn test_is_token_expired_zero_means_valid() {
        assert!(!is_token_expired(0));
    }

    #[test]
    fn test_is_token_expired_future() {
        let future_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            + 3_600_000;
        assert!(!is_token_expired(future_ms));
    }

    #[test]
    fn test_is_token_expired_past() {
        assert!(is_token_expired(1000));
    }

    #[test]
    fn test_is_token_expired_within_buffer() {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        // Expires in 2 minutes — within the 5-minute buffer
        let expires = now_ms + 120_000;
        assert!(is_token_expired(expires));
    }

    #[test]
    fn test_oauth_beta_header_value() {
        assert_eq!(OAUTH_BETA_HEADER, "oauth-2025-04-20");
    }

    // Phase 4B-2 spec-required tests

    #[test]
    fn test_auth_credentials_api_key_roundtrip() {
        let creds = AuthCredentials::ApiKey {
            key: "sk-ant-test".into(),
        };
        let json = serde_json::to_string(&creds).unwrap();
        assert!(json.contains("\"type\":\"api_key\""));
        let parsed: AuthCredentials = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, AuthCredentials::ApiKey { key } if key == "sk-ant-test"));
    }

    #[test]
    fn test_auth_credentials_oauth_roundtrip() {
        let creds = AuthCredentials::OAuth {
            access: "acc".into(),
            refresh: "ref".into(),
            expires: 999,
            extra: HashMap::new(),
        };
        let json = serde_json::to_string(&creds).unwrap();
        assert!(json.contains("\"type\":\"oauth\""));
        let parsed: AuthCredentials = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, AuthCredentials::OAuth { expires, .. } if expires == 999));
    }

    // ── Phase 13A: Multi-provider auth tests ──────────────────────────

    #[test]
    fn test_auth_credentials_oauth_with_extra_fields() {
        let json = r#"{
            "type": "oauth",
            "access": "token",
            "refresh": "ref",
            "expires": 9999,
            "project_id": "my-project",
            "client_secret": "secret123"
        }"#;
        let creds: AuthCredentials = serde_json::from_str(json).unwrap();
        match &creds {
            AuthCredentials::OAuth { access, extra, .. } => {
                assert_eq!(access, "token");
                assert_eq!(extra.get("project_id").unwrap(), "my-project");
                assert_eq!(extra.get("client_secret").unwrap(), "secret123");
            }
            _ => panic!("expected OAuth"),
        }
        // Roundtrip preserves extra fields
        let json_out = serde_json::to_string(&creds).unwrap();
        assert!(json_out.contains("project_id"));
    }

    #[test]
    fn test_auth_credentials_oauth_without_extra_backward_compat() {
        let json = r#"{"type":"oauth","access":"a","refresh":"r","expires":100}"#;
        let creds: AuthCredentials = serde_json::from_str(json).unwrap();
        match &creds {
            AuthCredentials::OAuth { extra, .. } => {
                assert!(extra.is_empty());
            }
            _ => panic!("expected OAuth"),
        }
    }

    #[test]
    fn test_resolve_provider_auth_anthropic() {
        use crate::provider_id::ProviderId;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(&path, r#"{"anthropic":{"type":"api_key","key":"sk-test"}}"#).unwrap();
        let creds = resolve_provider_auth(ProviderId::Anthropic, Some(&path), None).unwrap();
        assert!(matches!(creds, AuthCredentials::ApiKey { key } if key == "sk-test"));
    }

    #[test]
    fn test_resolve_provider_auth_openai() {
        use crate::provider_id::ProviderId;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(
            &path,
            r#"{"openai-codex":{"type":"oauth","access":"oai-token","refresh":"oai-ref","expires":9999}}"#,
        )
        .unwrap();
        let creds = resolve_provider_auth(ProviderId::OpenaiCodex, Some(&path), None).unwrap();
        match creds {
            AuthCredentials::OAuth { access, .. } => assert_eq!(access, "oai-token"),
            _ => panic!("expected OAuth"),
        }
    }

    #[test]
    fn test_resolve_provider_auth_missing() {
        use crate::provider_id::ProviderId;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(&path, r#"{"anthropic":{"type":"api_key","key":"sk-test"}}"#).unwrap();
        // Temporarily clear env var and HOME to block all fallback paths
        // (including ~/.codex/auth.json Codex CLI discovery)
        let prev_key = std::env::var("OPENAI_API_KEY").ok();
        let prev_home = std::env::var("HOME").ok();
        std::env::remove_var("OPENAI_API_KEY");
        std::env::set_var("HOME", dir.path());
        let result = resolve_provider_auth(ProviderId::OpenaiCodex, Some(&path), None);
        // Restore
        if let Some(val) = prev_key {
            std::env::set_var("OPENAI_API_KEY", val);
        }
        if let Some(val) = prev_home {
            std::env::set_var("HOME", val);
        }
        assert!(result.is_err());
    }

    #[test]
    fn test_save_provider_auth_preserves_others() {
        use crate::provider_id::ProviderId;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(&path, r#"{"anthropic":{"type":"api_key","key":"keep-me"}}"#).unwrap();

        let creds = AuthCredentials::OAuth {
            access: "oai-new".into(),
            refresh: "oai-ref".into(),
            expires: 12345,
            extra: HashMap::new(),
        };
        save_provider_auth(&path, ProviderId::OpenaiCodex, &creds).unwrap();

        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(content["anthropic"]["key"], "keep-me");
        assert_eq!(content["openai-codex"]["access"], "oai-new");
    }

    #[test]
    fn test_into_anthropic_auth_method_oauth() {
        let creds = AuthCredentials::OAuth {
            access: "tok".into(),
            refresh: "ref".into(),
            expires: 999,
            extra: HashMap::new(),
        };
        let method = creds.into_anthropic_auth_method();
        assert!(method.is_oauth());
    }

    #[test]
    fn test_into_anthropic_auth_method_api_key() {
        let creds = AuthCredentials::ApiKey {
            key: "sk-ant-test".into(),
        };
        let method = creds.into_anthropic_auth_method();
        assert!(!method.is_oauth());
    }

    #[test]
    fn test_resolve_provider_auth_cortex_dir() {
        use crate::provider_id::ProviderId;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("auth.json"),
            r#"{"google-gemini-cli":{"type":"oauth","access":"gem","refresh":"ref","expires":100}}"#,
        )
        .unwrap();
        let creds =
            resolve_provider_auth(ProviderId::GoogleGeminiCli, None, Some(dir.path())).unwrap();
        match creds {
            AuthCredentials::OAuth { access, .. } => assert_eq!(access, "gem"),
            _ => panic!("expected OAuth"),
        }
    }

    // ── Security review fixes ─────────────────────────────────────────

    #[test]
    fn test_auth_credentials_debug_redacts_secrets() {
        let creds = AuthCredentials::OAuth {
            access: "secret-access-token".into(),
            refresh: "secret-refresh-token".into(),
            expires: 12345,
            extra: HashMap::new(),
        };
        let debug = format!("{:?}", creds);
        assert!(
            !debug.contains("secret-access"),
            "debug must not contain access token"
        );
        assert!(
            !debug.contains("secret-refresh"),
            "debug must not contain refresh token"
        );
        assert!(
            debug.contains("****"),
            "debug should contain redaction marker"
        );
        assert!(
            debug.contains("12345"),
            "expires is not secret and should be visible"
        );
    }

    #[test]
    fn test_auth_credentials_debug_redacts_api_key() {
        let creds = AuthCredentials::ApiKey {
            key: "sk-secret-key".into(),
        };
        let debug = format!("{:?}", creds);
        assert!(!debug.contains("sk-secret"), "debug must not contain key");
        assert!(debug.contains("****"));
    }

    #[test]
    fn test_sanitize_extra_strips_reserved_keys() {
        let mut creds = AuthCredentials::OAuth {
            access: "tok".into(),
            refresh: "ref".into(),
            expires: 100,
            extra: {
                let mut m = HashMap::new();
                m.insert("type".into(), serde_json::json!("injected"));
                m.insert("access".into(), serde_json::json!("injected"));
                m.insert("project_id".into(), serde_json::json!("legit"));
                m
            },
        };
        creds.sanitize_extra();
        match &creds {
            AuthCredentials::OAuth { extra, .. } => {
                assert!(
                    !extra.contains_key("type"),
                    "reserved key 'type' must be stripped"
                );
                assert!(
                    !extra.contains_key("access"),
                    "reserved key 'access' must be stripped"
                );
                assert!(
                    extra.contains_key("project_id"),
                    "non-reserved key must be kept"
                );
            }
            _ => panic!("expected OAuth"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_save_provider_auth_sets_0o600_permissions() {
        use crate::provider_id::ProviderId;
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let creds = AuthCredentials::ApiKey {
            key: "sk-test".into(),
        };
        save_provider_auth(&path, ProviderId::Anthropic, &creds).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "auth.json must be owner-read-write only"
        );
    }

    #[test]
    fn test_auth_method_debug_redacts_api_key() {
        let auth = AuthMethod::ApiKey("sk-ant-api03-secret-key".into());
        let debug = format!("{:?}", auth);
        assert!(
            !debug.contains("secret-key"),
            "debug must not contain raw key"
        );
        assert!(debug.contains("****"));
    }

    #[test]
    fn test_auth_method_debug_redacts_oauth_tokens() {
        let auth = AuthMethod::OAuth {
            access_token: "secret-access".into(),
            refresh_token: "secret-refresh".into(),
            expires_ms: 12345,
        };
        let debug = format!("{:?}", auth);
        assert!(!debug.contains("secret-access"));
        assert!(!debug.contains("secret-refresh"));
        assert!(debug.contains("12345"));
    }
}
