//! Generic credential I/O for all providers.
//!
//! Loads and saves provider credentials to/from auth.json files.
//! Provider-specific auth resolution lives in each provider module.

use std::path::{Path, PathBuf};

use tracing::{info, warn};

use crate::error::ProviderError;
use crate::provider_id::ProviderId;

use super::{AuthCredentials, AuthFile};

/// Resolve authentication credentials for any provider.
/// Search order: auth_json_path → cortex/auth.json → ~/.pi/agent/auth.json → env var.
pub fn resolve_provider_auth(
    provider: ProviderId,
    auth_json_path: Option<&Path>,
    cortex_dir: Option<&Path>,
) -> Result<AuthCredentials, ProviderError> {
    let mut paths_to_try: Vec<PathBuf> = Vec::new();

    if let Some(p) = auth_json_path {
        paths_to_try.push(p.to_path_buf());
    } else {
        if let Some(cortex) = cortex_dir {
            paths_to_try.push(cortex.join("auth.json"));
        }
        if let Ok(home) = std::env::var("HOME") {
            paths_to_try.push(PathBuf::from(home).join(".pi/agent/auth.json"));
        }
    }

    let auth_key = provider.auth_key();

    for path in &paths_to_try {
        if path.exists() {
            match load_provider_credentials(path, auth_key) {
                Ok(Some(creds)) => {
                    info!(path = %path.display(), provider = auth_key, "loaded auth from auth.json");
                    return Ok(creds);
                }
                Ok(None) => {
                    info!(path = %path.display(), provider = auth_key, "auth.json exists but no entry for provider");
                }
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "failed to read auth.json");
                }
            }
        }
    }

    // Provider-specific CLI tool fallback
    if provider == ProviderId::OpenaiCodex {
        if let Some(creds) = load_codex_cli_auth() {
            info!("loaded auth from ~/.codex/auth.json (Codex CLI)");
            return Ok(creds);
        }
    }

    // Env var fallback
    let env_var = provider.env_var_name();
    match std::env::var(env_var) {
        Ok(key) if !key.is_empty() => {
            info!(provider = auth_key, env_var, "using env var for auth");
            Ok(AuthCredentials::ApiKey { key })
        }
        _ => Err(ProviderError::MissingAuth {
            provider: auth_key.to_string(),
            env_hint: env_var.to_string(),
        }),
    }
}

/// Load OpenAI credentials from the Codex CLI's auth.json (~/.codex/auth.json).
/// Supports the ChatGPT OAuth format: { auth_mode: "chatgpt", tokens: { access_token, refresh_token, account_id } }
fn load_codex_cli_auth() -> Option<AuthCredentials> {
    let home = std::env::var("HOME").ok()?;
    let path = std::path::PathBuf::from(home).join(".codex/auth.json");
    if !path.exists() {
        return None;
    }

    let content = std::fs::read_to_string(&path).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&content).ok()?;

    // Codex CLI format: { auth_mode, tokens: { access_token, refresh_token, account_id } }
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

    let mut extra = std::collections::HashMap::new();
    if !account_id.is_empty() {
        extra.insert(
            "account_id".to_string(),
            serde_json::Value::String(account_id),
        );
    }

    // Set expires to 1 (epoch) to force refresh on first use — the Codex CLI
    // doesn't store expiry in our format and the JWT is likely expired
    Some(AuthCredentials::OAuth {
        access,
        refresh,
        expires: 1,
        extra,
    })
}

/// Load credentials for a specific provider key from auth.json.
pub(crate) fn load_provider_credentials(
    path: &Path,
    provider_key: &str,
) -> Result<Option<AuthCredentials>, ProviderError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| ProviderError::AuthConfig(format!("cannot read {}: {e}", path.display())))?;

    let auth: AuthFile = serde_json::from_str(&content)
        .map_err(|e| ProviderError::AuthConfig(format!("invalid auth.json: {e}")))?;

    Ok(auth.get(provider_key).cloned())
}

/// Write updated credentials for any provider back to auth.json.
/// Preserves other providers' entries. File-locked atomic write with 0o600 permissions.
pub fn save_provider_auth(
    path: &Path,
    provider: ProviderId,
    creds: &AuthCredentials,
) -> Result<(), ProviderError> {
    use fs2::FileExt;

    let mut creds = creds.clone();
    creds.sanitize_extra();

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ProviderError::AuthConfig(format!("cannot create dir: {e}")))?;
    }

    // File-locked read-modify-write
    let lock_path = path.with_extension("lock");
    let lock_file = std::fs::File::create(&lock_path)
        .map_err(|e| ProviderError::AuthConfig(format!("cannot create lock: {e}")))?;
    lock_file
        .lock_exclusive()
        .map_err(|e| ProviderError::AuthConfig(format!("cannot acquire lock: {e}")))?;

    let content = std::fs::read_to_string(path).unwrap_or_else(|_| "{}".into());
    let mut auth: serde_json::Value =
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}));

    let creds_value =
        serde_json::to_value(&creds).map_err(|e| ProviderError::AuthConfig(e.to_string()))?;
    auth[provider.auth_key()] = creds_value;

    let updated = serde_json::to_string_pretty(&auth)
        .map_err(|e| ProviderError::AuthConfig(e.to_string()))?;

    let temp = path.with_extension(format!("tmp.{:?}", std::thread::current().id()));
    std::fs::write(&temp, updated.as_bytes())
        .map_err(|e| ProviderError::AuthConfig(format!("cannot write {}: {e}", temp.display())))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| ProviderError::AuthConfig(format!("cannot set permissions: {e}")))?;
    }

    std::fs::rename(&temp, path)
        .map_err(|e| ProviderError::AuthConfig(format!("cannot rename: {e}")))?;

    drop(lock_file);
    let _ = std::fs::remove_file(&lock_path);

    info!(path = %path.display(), provider = %provider, "auth.json updated");
    Ok(())
}
