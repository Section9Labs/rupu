//! Per-provider credential discovery from known sources.
//!
//! Each function checks local credential stores (Keychain, CLI config files,
//! env vars) and returns discovered credentials without modifying the sources.
//! Used by `phi-node --setup-auth` for one-time import into cortex/auth.json.

use std::collections::HashMap;
use std::path::Path;

use tracing::info;

use crate::auth::{AuthCredentials, AuthFile};
use crate::provider_id::ProviderId;

/// Result of a discovery attempt for one provider.
#[derive(Debug)]
pub struct DiscoveredCredential {
    pub provider: ProviderId,
    pub credentials: AuthCredentials,
    pub source: String,
    pub expires_ms: Option<u64>,
}

/// Discover credentials for a specific provider from all known sources.
pub fn discover(provider: ProviderId) -> Option<DiscoveredCredential> {
    match provider {
        ProviderId::Anthropic => discover_anthropic(),
        ProviderId::OpenaiCodex => discover_openai_codex(),
        ProviderId::GoogleGeminiCli => {
            discover_google("google-gemini-cli", ProviderId::GoogleGeminiCli)
        }
        ProviderId::GoogleAntigravity => {
            discover_google("google-antigravity", ProviderId::GoogleAntigravity)
        }
        ProviderId::GithubCopilot => discover_github_copilot(),
    }
}

/// Discover all providers that have credentials available.
pub fn discover_all() -> Vec<DiscoveredCredential> {
    ProviderId::ALL
        .iter()
        .filter_map(|id| discover(*id))
        .collect()
}

/// Check if credentials already exist in an auth.json file for a provider.
pub fn check_existing(auth_path: &Path, provider: ProviderId) -> Option<DiscoveredCredential> {
    let content = std::fs::read_to_string(auth_path).ok()?;
    let auth: AuthFile = serde_json::from_str(&content).ok()?;
    let creds = auth.get(provider.auth_key())?.clone();

    let expires_ms = match &creds {
        AuthCredentials::OAuth { expires, .. } if *expires > 0 => Some(*expires),
        _ => None,
    };

    Some(DiscoveredCredential {
        provider,
        credentials: creds,
        source: "existing auth.json".into(),
        expires_ms,
    })
}

fn discover_anthropic() -> Option<DiscoveredCredential> {
    // 1. Claude Code macOS Keychain
    #[cfg(target_os = "macos")]
    if let Some(creds) = discover_from_keychain() {
        return Some(creds);
    }

    // 2. Claude Code credentials file (older versions)
    if let Some(creds) = discover_from_claude_code_file() {
        return Some(creds);
    }

    // 3. Environment variable
    discover_from_env("ANTHROPIC_API_KEY", ProviderId::Anthropic)
}

fn discover_openai_codex() -> Option<DiscoveredCredential> {
    if let Some(creds) = discover_from_codex_cli() {
        return Some(creds);
    }
    discover_from_env("OPENAI_API_KEY", ProviderId::OpenaiCodex)
}

fn discover_google(_label: &str, provider: ProviderId) -> Option<DiscoveredCredential> {
    if let Some(mut creds) = discover_from_gcloud() {
        creds.provider = provider;
        return Some(creds);
    }
    discover_from_env("GOOGLE_API_KEY", provider)
}

fn discover_github_copilot() -> Option<DiscoveredCredential> {
    if let Some(creds) = discover_from_gh_cli() {
        return Some(creds);
    }
    discover_from_env("GITHUB_TOKEN", ProviderId::GithubCopilot)
}

// --- Source-specific discovery ---

#[cfg(target_os = "macos")]
fn discover_from_keychain() -> Option<DiscoveredCredential> {
    use crate::auth::AuthMethod;

    let auth = crate::anthropic::load_claude_code_keychain()?;

    let (creds, expires_ms) = match auth {
        AuthMethod::OAuth {
            access_token,
            refresh_token,
            expires_ms,
        } => (
            AuthCredentials::OAuth {
                access: access_token,
                refresh: refresh_token,
                expires: expires_ms,
                extra: HashMap::new(),
            },
            if expires_ms > 0 {
                Some(expires_ms)
            } else {
                None
            },
        ),
        AuthMethod::ApiKey(key) => (AuthCredentials::ApiKey { key }, None),
    };

    info!("discovered Anthropic credentials from Claude Code Keychain");
    Some(DiscoveredCredential {
        provider: ProviderId::Anthropic,
        credentials: creds,
        source: "Claude Code Keychain".into(),
        expires_ms,
    })
}

fn discover_from_claude_code_file() -> Option<DiscoveredCredential> {
    let home = std::env::var("HOME").ok()?;
    let path = std::path::PathBuf::from(&home).join(".claude/.credentials.json");
    if !path.exists() {
        return None;
    }

    let content = std::fs::read_to_string(&path).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&content).ok()?;
    let oauth = parsed.get("claudeAiOauth")?;

    let access = oauth.get("accessToken")?.as_str()?.to_string();
    let refresh = oauth
        .get("refreshToken")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let expires = oauth.get("expiresAt")?.as_u64()?;

    if access.is_empty() {
        return None;
    }

    info!("discovered Anthropic credentials from Claude Code credentials file");
    Some(DiscoveredCredential {
        provider: ProviderId::Anthropic,
        credentials: AuthCredentials::OAuth {
            access,
            refresh,
            expires,
            extra: HashMap::new(),
        },
        source: "Claude Code credentials file".into(),
        expires_ms: Some(expires),
    })
}

fn discover_from_codex_cli() -> Option<DiscoveredCredential> {
    let home = std::env::var("HOME").ok()?;
    let path = std::path::PathBuf::from(&home).join(".codex/auth.json");
    if !path.exists() {
        return None;
    }

    let content = std::fs::read_to_string(&path).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&content).ok()?;

    let tokens = parsed.get("tokens")?;
    let access = tokens.get("access_token")?.as_str()?.to_string();
    let refresh = tokens
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let account_id = tokens
        .get("account_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if access.is_empty() {
        return None;
    }

    let mut extra = HashMap::new();
    if !account_id.is_empty() {
        extra.insert("account_id".into(), serde_json::Value::String(account_id));
    }

    // Extract expiry from the JWT access token's `exp` claim
    let expires_ms = parse_jwt_exp_ms(&access).unwrap_or(1);

    info!("discovered OpenAI credentials from Codex CLI");
    Some(DiscoveredCredential {
        provider: ProviderId::OpenaiCodex,
        credentials: AuthCredentials::OAuth {
            access,
            refresh,
            expires: expires_ms,
            extra,
        },
        source: "Codex CLI (~/.codex/auth.json)".into(),
        expires_ms: if expires_ms > 1 {
            Some(expires_ms)
        } else {
            None
        },
    })
}

fn discover_from_gcloud() -> Option<DiscoveredCredential> {
    let output = std::process::Command::new("gcloud")
        .args(["auth", "application-default", "print-access-token"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let token = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if token.is_empty() {
        return None;
    }

    info!("discovered Google credentials from gcloud CLI");
    Some(DiscoveredCredential {
        provider: ProviderId::GoogleGeminiCli,
        credentials: AuthCredentials::OAuth {
            access: token,
            refresh: String::new(),
            expires: 1, // Force refresh on first use — gcloud tokens expire in ~1 hour
            extra: HashMap::new(),
        },
        source: "gcloud CLI".into(),
        expires_ms: None,
    })
}

fn discover_from_gh_cli() -> Option<DiscoveredCredential> {
    let output = std::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let token = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if token.is_empty() {
        return None;
    }

    info!("discovered GitHub credentials from gh CLI");
    Some(DiscoveredCredential {
        provider: ProviderId::GithubCopilot,
        credentials: AuthCredentials::OAuth {
            access: token,
            refresh: String::new(),
            expires: 0,
            extra: HashMap::new(),
        },
        source: "gh CLI".into(),
        expires_ms: None,
    })
}

/// Parse the `exp` claim from a JWT access token and convert to milliseconds.
/// Returns None if the token is not a valid JWT or has no exp claim.
fn parse_jwt_exp_ms(jwt: &str) -> Option<u64> {
    let parts: Vec<&str> = jwt.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    // Decode the payload (second segment, base64url)
    let payload = base64_url_decode(parts[1])?;
    let claims: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    let exp_secs = claims.get("exp")?.as_u64()?;
    Some(exp_secs * 1000)
}

/// Decode base64url (no padding) to bytes.
fn base64_url_decode(input: &str) -> Option<Vec<u8>> {
    // Add padding if needed
    let padded = match input.len() % 4 {
        2 => format!("{input}=="),
        3 => format!("{input}="),
        _ => input.to_string(),
    };
    // Replace URL-safe chars with standard base64
    let standard = padded.replace('-', "+").replace('_', "/");
    base64_decode(&standard)
}

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    // Simple base64 decoder — avoids adding a dependency for this one use
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = Vec::new();
    let bytes: Vec<u8> = input
        .bytes()
        .filter(|&b| b != b'\n' && b != b'\r')
        .collect();
    for chunk in bytes.chunks(4) {
        if chunk.len() < 2 {
            break;
        }
        let vals: Vec<Option<u8>> = chunk
            .iter()
            .map(|&b| {
                if b == b'=' {
                    Some(0)
                } else {
                    TABLE.iter().position(|&t| t == b).map(|p| p as u8)
                }
            })
            .collect();
        if vals.iter().any(|v| v.is_none()) {
            return None;
        }
        let v: Vec<u8> = vals.into_iter().map(|v| v.expect("checked")).collect();
        output.push((v[0] << 2) | (v[1] >> 4));
        if chunk.len() > 2 && chunk[2] != b'=' {
            output.push((v[1] << 4) | (v[2] >> 2));
        }
        if chunk.len() > 3 && chunk[3] != b'=' {
            output.push((v[2] << 6) | v[3]);
        }
    }
    Some(output)
}

fn discover_from_env(var_name: &str, provider: ProviderId) -> Option<DiscoveredCredential> {
    let key = std::env::var(var_name).ok()?;
    if key.is_empty() {
        return None;
    }

    info!(var_name, provider = %provider, "discovered credentials from env var");
    Some(DiscoveredCredential {
        provider,
        credentials: AuthCredentials::ApiKey { key },
        source: format!("${var_name} environment variable"),
        expires_ms: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_discover_from_env() {
        std::env::set_var("_PHI_TEST_SETUP_KEY", "sk-test-key");
        let result = discover_from_env("_PHI_TEST_SETUP_KEY", ProviderId::Anthropic);
        std::env::remove_var("_PHI_TEST_SETUP_KEY");

        let d = result.expect("should discover from env");
        assert_eq!(d.provider, ProviderId::Anthropic);
        assert!(d.source.contains("_PHI_TEST_SETUP_KEY"));
        match d.credentials {
            AuthCredentials::ApiKey { key } => assert_eq!(key, "sk-test-key"),
            _ => panic!("expected ApiKey"),
        }
    }

    #[test]
    fn test_discover_from_env_empty() {
        std::env::set_var("_PHI_TEST_EMPTY", "");
        let result = discover_from_env("_PHI_TEST_EMPTY", ProviderId::Anthropic);
        std::env::remove_var("_PHI_TEST_EMPTY");
        assert!(result.is_none());
    }

    #[test]
    fn test_discover_from_env_missing() {
        std::env::remove_var("_PHI_TEST_MISSING");
        assert!(discover_from_env("_PHI_TEST_MISSING", ProviderId::Anthropic).is_none());
    }

    #[test]
    fn test_check_existing_finds_provider() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        fs::write(
            &path,
            r#"{"anthropic":{"type":"api_key","key":"sk-exist"}}"#,
        )
        .unwrap();
        let result = check_existing(&path, ProviderId::Anthropic);
        assert!(result.is_some());
        assert_eq!(result.unwrap().source, "existing auth.json");
    }

    #[test]
    fn test_check_existing_returns_none_for_missing_provider() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        fs::write(&path, r#"{}"#).unwrap();
        assert!(check_existing(&path, ProviderId::Anthropic).is_none());
    }

    #[test]
    fn test_check_existing_returns_none_for_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(check_existing(&dir.path().join("nope.json"), ProviderId::Anthropic).is_none());
    }

    #[test]
    fn test_check_existing_oauth_expires() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        fs::write(
            &path,
            r#"{"anthropic":{"type":"oauth","access":"tok","refresh":"ref","expires":9999999999999}}"#,
        )
        .unwrap();
        let result = check_existing(&path, ProviderId::Anthropic).unwrap();
        assert_eq!(result.expires_ms, Some(9999999999999));
    }

    #[test]
    fn test_parse_jwt_exp_ms() {
        // Create a minimal JWT with exp claim
        // Header: {"alg":"none"} = eyJhbGciOiJub25lIn0
        // Payload: {"exp":1776972843} = eyJleHAiOjE3NzY5NzI4NDN9
        // Signature: (empty)
        let jwt = "eyJhbGciOiJub25lIn0.eyJleHAiOjE3NzY5NzI4NDN9.";
        let result = parse_jwt_exp_ms(jwt);
        assert_eq!(result, Some(1776972843000));
    }

    #[test]
    fn test_parse_jwt_exp_ms_invalid() {
        assert_eq!(parse_jwt_exp_ms("not-a-jwt"), None);
        assert_eq!(parse_jwt_exp_ms(""), None);
    }

    #[test]
    fn test_base64_url_decode() {
        // "hello" base64url-encoded = "aGVsbG8"
        let decoded = base64_url_decode("aGVsbG8");
        assert_eq!(decoded, Some(b"hello".to_vec()));
    }
}
