use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, info, warn};

use crate::auth::credential_store::resolve_provider_auth;
use crate::auth::{is_token_expired, AuthCredentials, AuthFile, AuthMethod, OAUTH_BETA_HEADER};
use crate::error::ProviderError;
use crate::provider_id::ProviderId;
use crate::sse::SseParser;
use crate::types::*;

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
/// Anthropic API version. Update when new SSE event types or features are needed.
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Maximum retries for 429 rate-limit responses.
/// Per-request 429 retries. Set to 1 (one retry) so the ProviderRouter can
/// handle cross-provider fallback quickly. When used without a router, the
/// single retry handles brief transient rate limits.
const MAX_RATE_LIMIT_RETRIES: u32 = 1;
/// Initial backoff for 429 retries (doubles each attempt).
const INITIAL_BACKOFF_MS: u64 = 2000;

const ANTHROPIC_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const ANTHROPIC_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

/// Resolve authentication for Anthropic.
/// Delegates to resolve_provider_auth, with legacy Pi format fallback.
/// Search order: auth_json_path -> cortex/auth.json -> ~/.pi/agent/auth.json -> ANTHROPIC_API_KEY
pub fn resolve_anthropic_auth(
    auth_json_path: Option<&Path>,
    cortex_dir: Option<&Path>,
) -> Result<AuthMethod, ProviderError> {
    // Primary path: use the generalized provider auth resolver
    if let Ok(creds) = resolve_provider_auth(ProviderId::Anthropic, auth_json_path, cortex_dir) {
        return Ok(creds.into_anthropic_auth_method());
    }

    // Fallback: legacy Pi format support (load_auth_json handles non-tagged JSON)
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

    for path in &paths_to_try {
        if path.exists() {
            if let Ok(Some(method)) = load_auth_json(path) {
                info!(path = %path.display(), "loaded auth from auth.json (legacy format)");
                return Ok(method);
            }
        }
    }

    // Fallback: read from Claude Code's macOS Keychain entry
    #[cfg(target_os = "macos")]
    if let Some(method) = load_claude_code_keychain() {
        info!("loaded auth from Claude Code keychain");
        return Ok(method);
    }

    // Final fallback: env var with AuthMethod::detect (handles OAuth prefix)
    match std::env::var("ANTHROPIC_API_KEY") {
        Ok(key) if !key.is_empty() => {
            info!("using ANTHROPIC_API_KEY from environment");
            Ok(AuthMethod::detect(&key))
        }
        _ => Err(ProviderError::MissingAuth {
            provider: "anthropic".into(),
            env_hint: "ANTHROPIC_API_KEY".into(),
        }),
    }
}

/// Load Anthropic credentials from an auth.json file.
/// Supports both tagged enum format ({"type":"oauth",...}) and legacy Pi format.
pub(crate) fn load_auth_json(path: &Path) -> Result<Option<AuthMethod>, ProviderError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| ProviderError::AuthConfig(format!("cannot read {}: {e}", path.display())))?;

    // Try new tagged enum format first
    if let Ok(auth) = serde_json::from_str::<AuthFile>(&content) {
        return match auth.get("anthropic") {
            Some(AuthCredentials::OAuth {
                access,
                refresh,
                expires,
                ..
            }) => Ok(Some(AuthMethod::OAuth {
                access_token: access.clone(),
                refresh_token: refresh.clone(),
                expires_ms: *expires,
            })),
            Some(AuthCredentials::ApiKey { key }) => Ok(Some(AuthMethod::ApiKey(key.clone()))),
            None => Ok(None),
        };
    }

    // Fallback: try legacy Pi format (has "type" field as plain string, not serde tag)
    #[derive(Deserialize)]
    struct LegacyCredentials {
        #[serde(rename = "type", default)]
        auth_type: String,
        #[serde(default)]
        access: String,
        #[serde(default)]
        refresh: String,
        #[serde(default)]
        expires: u64,
    }
    type LegacyAuthFile = HashMap<String, LegacyCredentials>;

    let legacy: LegacyAuthFile = serde_json::from_str(&content)
        .map_err(|e| ProviderError::AuthConfig(format!("invalid auth.json: {e}")))?;

    if let Some(creds) = legacy.get("anthropic") {
        if creds.auth_type == "oauth" || !creds.access.is_empty() {
            return Ok(Some(AuthMethod::OAuth {
                access_token: creds.access.clone(),
                refresh_token: creds.refresh.clone(),
                expires_ms: creds.expires,
            }));
        }
    }

    Ok(None)
}

/// Load Anthropic OAuth tokens from Claude Code's macOS Keychain.
///
/// Claude Code stores credentials in the macOS Keychain under the service
/// name "Claude Code-credentials" as hex-encoded JSON. The JSON contains
/// a `claudeAiOauth` key with `accessToken`, `refreshToken`, and `expiresAt`.
#[cfg(target_os = "macos")]
pub(crate) fn load_claude_code_keychain() -> Option<AuthMethod> {
    let output = std::process::Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            "Claude Code-credentials",
            "-w",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let raw = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if raw.is_empty() {
        return None;
    }

    // Claude Code stores as either raw JSON or hex-encoded JSON
    let parsed: serde_json::Value = if raw.starts_with('{') {
        serde_json::from_str(&raw).ok()?
    } else {
        let json_bytes = (0..raw.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(raw.get(i..i + 2)?, 16).ok())
            .collect::<Option<Vec<u8>>>()?;
        serde_json::from_slice(&json_bytes).ok()?
    };
    let oauth = parsed.get("claudeAiOauth")?;

    let access_token = oauth.get("accessToken")?.as_str()?.to_string();
    let refresh_token = oauth
        .get("refreshToken")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let expires_at = oauth.get("expiresAt")?.as_u64()?;

    if access_token.is_empty() {
        return None;
    }

    Some(AuthMethod::OAuth {
        access_token,
        refresh_token,
        expires_ms: expires_at,
    })
}

/// Refresh an Anthropic OAuth token. Returns updated AuthMethod.
/// Uses application/x-www-form-urlencoded as required by the token endpoint.
pub async fn refresh_anthropic_token(
    client: &Client,
    refresh_token: &str,
) -> Result<AuthMethod, ProviderError> {
    info!("refreshing Anthropic OAuth token");

    let response = client
        .post(ANTHROPIC_TOKEN_URL)
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", ANTHROPIC_CLIENT_ID),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await
        .map_err(|e| ProviderError::TokenRefreshFailed(e.to_string()))?;

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        return Err(ProviderError::TokenRefreshFailed(format!(
            "HTTP {status}: {body}"
        )));
    }

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| ProviderError::TokenRefreshFailed(e.to_string()))?;

    let access_token = body["access_token"]
        .as_str()
        .ok_or_else(|| ProviderError::TokenRefreshFailed("missing access_token".into()))?
        .to_string();

    let new_refresh = body["refresh_token"]
        .as_str()
        .unwrap_or(refresh_token)
        .to_string();

    let expires_in_secs = body["expires_in"].as_u64().unwrap_or(3600);
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let expires_ms = now_ms + (expires_in_secs * 1000);

    info!("token refreshed, expires in {expires_in_secs}s");

    Ok(AuthMethod::OAuth {
        access_token,
        refresh_token: new_refresh,
        expires_ms,
    })
}

/// Write updated credentials back to auth.json (preserving other providers).
/// Uses atomic write (temp + rename) with 0o600 permissions set on temp BEFORE rename.
pub fn save_auth_json(path: &Path, auth_method: &AuthMethod) -> Result<(), ProviderError> {
    use fs2::FileExt;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ProviderError::AuthConfig(format!("cannot create dir: {e}")))?;
    }

    // File-locked read-modify-write to prevent race with CredentialStore
    let lock_path = path.with_extension("lock");
    let lock_file = std::fs::File::create(&lock_path)
        .map_err(|e| ProviderError::AuthConfig(format!("cannot create lock: {e}")))?;
    lock_file
        .lock_exclusive()
        .map_err(|e| ProviderError::AuthConfig(format!("cannot acquire lock: {e}")))?;

    let content = std::fs::read_to_string(path).unwrap_or_else(|_| "{}".into());
    let mut auth: serde_json::Value =
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}));

    match auth_method {
        AuthMethod::OAuth {
            access_token,
            refresh_token,
            expires_ms,
        } => {
            auth["anthropic"] = serde_json::json!({
                "type": "oauth",
                "access": access_token,
                "refresh": refresh_token,
                "expires": expires_ms,
            });
        }
        AuthMethod::ApiKey(key) => {
            auth["anthropic"] = serde_json::json!({
                "type": "api_key",
                "key": key,
            });
        }
    }

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

    info!(path = %path.display(), "auth.json updated");
    Ok(())
}

/// Anthropic Messages API client with SSE streaming.
/// Supports both API key (`x-api-key`) and OAuth (`Authorization: Bearer`) auth.
pub struct AnthropicClient {
    client: Client,
    auth: AuthMethod,
    api_url: String,
    /// Path to auth.json for writing refreshed tokens back (legacy).
    auth_json_path: Option<std::path::PathBuf>,
    /// Credential store for persisting refreshed tokens (preferred over auth_json_path).
    credential_store: Option<std::sync::Arc<dyn crate::credential_source::CredentialSource>>,
}

impl AnthropicClient {
    /// Create a new client from a resolved AuthMethod.
    pub fn from_auth(auth: AuthMethod) -> Self {
        Self {
            client: Client::new(),
            auth,
            api_url: ANTHROPIC_API_URL.to_string(),
            auth_json_path: None,
            credential_store: None,
        }
    }

    /// Create a client with an auth.json path for persisting refreshed tokens.
    pub fn from_auth_with_path(auth: AuthMethod, auth_json_path: std::path::PathBuf) -> Self {
        Self {
            client: Client::new(),
            auth,
            api_url: ANTHROPIC_API_URL.to_string(),
            auth_json_path: Some(auth_json_path),
            credential_store: None,
        }
    }

    /// Create a client backed by a CredentialStore for token persistence.
    pub fn from_auth_with_store(
        auth: AuthMethod,
        store: std::sync::Arc<dyn crate::credential_source::CredentialSource>,
    ) -> Self {
        Self {
            client: Client::new(),
            auth,
            api_url: ANTHROPIC_API_URL.to_string(),
            auth_json_path: None,
            credential_store: Some(store),
        }
    }

    /// Create a new client. Reads `ANTHROPIC_API_KEY` from environment.
    pub fn from_env() -> Result<Self, ProviderError> {
        let auth = resolve_anthropic_auth(None, None)?;
        Ok(Self::from_auth(auth))
    }

    /// Create a client with an explicit API key (for testing).
    pub fn new(api_key: String) -> Self {
        Self::from_auth(AuthMethod::ApiKey(api_key))
    }

    /// Create a client pointing at a custom URL (for testing with mock servers).
    pub fn with_url(api_key: String, api_url: String) -> Self {
        Self {
            client: Client::new(),
            auth: AuthMethod::ApiKey(api_key),
            api_url,
            auth_json_path: None,
            credential_store: None,
        }
    }

    /// Ensure the OAuth token is still valid, refreshing if expired.
    /// Persists refreshed tokens via CredentialStore (preferred) or save_auth_json (legacy).
    async fn ensure_valid_token(&mut self) -> Result<(), ProviderError> {
        if let AuthMethod::OAuth {
            refresh_token,
            expires_ms,
            ..
        } = &self.auth
        {
            if !refresh_token.is_empty() && is_token_expired(*expires_ms) {
                info!("OAuth token expired, refreshing");
                let new_auth = refresh_anthropic_token(&self.client, refresh_token).await?;

                // Persist via CredentialStore (file-locked, preserves other providers)
                if let Some(store) = &self.credential_store {
                    let creds = match &new_auth {
                        AuthMethod::OAuth {
                            access_token,
                            refresh_token,
                            expires_ms,
                        } => AuthCredentials::OAuth {
                            access: access_token.clone(),
                            refresh: refresh_token.clone(),
                            expires: *expires_ms,
                            extra: std::collections::HashMap::new(),
                        },
                        AuthMethod::ApiKey(key) => AuthCredentials::ApiKey { key: key.clone() },
                    };
                    if let Err(e) = store.update(crate::provider_id::ProviderId::Anthropic, creds) {
                        warn!(error = %e, "failed to persist refreshed token via credential store");
                    }
                } else if let Some(path) = &self.auth_json_path {
                    // Legacy fallback
                    if let Err(e) = save_auth_json(path, &new_auth) {
                        warn!(error = %e, "failed to save refreshed token to auth.json");
                    }
                }

                self.auth = new_auth;
            }
        }
        Ok(())
    }

    /// Apply auth headers to a request builder based on auth method.
    fn apply_auth_headers(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.auth {
            AuthMethod::ApiKey(key) => builder
                .header("x-api-key", key)
                .header("anthropic-version", ANTHROPIC_VERSION),
            AuthMethod::OAuth { access_token, .. } => builder
                .header("Authorization", format!("Bearer {access_token}"))
                .header("anthropic-beta", OAUTH_BETA_HEADER)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .header("x-app", "cli"),
        }
    }

    /// Send a message and get the complete response (non-streaming).
    /// Retries with exponential backoff on 429 rate-limit responses.
    pub async fn send(&mut self, request: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        self.ensure_valid_token().await?;
        let body = self.build_request_body(request, false);

        let mut last_err = None;
        for attempt in 0..=MAX_RATE_LIMIT_RETRIES {
            if attempt > 0 {
                let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                warn!(
                    attempt,
                    backoff_ms = backoff,
                    "rate-limited (429), retrying"
                );
                tokio::time::sleep(std::time::Duration::from_millis(backoff)).await;
            }

            let builder = self
                .client
                .post(&self.api_url)
                .header("Content-Type", "application/json")
                .json(&body);
            let response = self.apply_auth_headers(builder).send().await?;

            let status = response.status();
            if status.as_u16() == 429 {
                let text = response.text().await.unwrap_or_default();
                warn!(
                    attempt,
                    body = text.as_str(),
                    "429 response from Anthropic API"
                );
                last_err = Some(ProviderError::Api {
                    status: 429,
                    message: text,
                });
                continue;
            }
            if !status.is_success() {
                let text = response.text().await.unwrap_or_default();
                let truncated = if text.len() > 4096 {
                    format!("{}... (truncated)", &text[..4096])
                } else {
                    text
                };
                return Err(ProviderError::Api {
                    status: status.as_u16(),
                    message: truncated,
                });
            }

            let api_response: AnthropicResponse = response.json().await?;
            return Ok(api_response.into_llm_response());
        }

        Err(last_err.unwrap_or_else(|| ProviderError::Api {
            status: 429,
            message: "rate-limited after max retries".into(),
        }))
    }

    /// Send a message with SSE streaming. Calls `on_event` for each stream event.
    /// Returns the complete response after the stream ends.
    ///
    /// The callback is `Send` to support async consumers that may hold the callback
    /// across `.await` boundaries.
    ///
    /// **Note on content block ordering**: Text deltas are accumulated into a single
    /// text block placed first in the response, followed by tool_use blocks. If the
    /// model interleaves text and tool_use, the original ordering is not preserved.
    /// Use `response.text()` and `response.tool_calls()` for access — they don't
    /// depend on ordering.
    pub async fn stream(
        &mut self,
        request: &LlmRequest,
        mut on_event: impl FnMut(StreamEvent) + Send,
    ) -> Result<LlmResponse, ProviderError> {
        self.ensure_valid_token().await?;
        let body = self.build_request_body(request, true);

        // Retry loop for 429 rate-limits
        let response = {
            let mut last_err = None;
            let mut got_response = None;
            for attempt in 0..=MAX_RATE_LIMIT_RETRIES {
                if attempt > 0 {
                    let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
                    warn!(
                        attempt,
                        backoff_ms = backoff,
                        "rate-limited (429), retrying"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(backoff)).await;
                }

                let builder = self
                    .client
                    .post(&self.api_url)
                    .header("Content-Type", "application/json")
                    .json(&body);
                let resp = self.apply_auth_headers(builder).send().await?;

                let status = resp.status();
                if status.as_u16() == 429 {
                    let text = resp.text().await.unwrap_or_default();
                    last_err = Some(ProviderError::Api {
                        status: 429,
                        message: text,
                    });
                    continue;
                }
                if !status.is_success() {
                    let text = resp.text().await.unwrap_or_default();
                    let truncated = if text.len() > 4096 {
                        format!("{}... (truncated)", &text[..4096])
                    } else {
                        text
                    };
                    return Err(ProviderError::Api {
                        status: status.as_u16(),
                        message: truncated,
                    });
                }
                got_response = Some(resp);
                break;
            }
            match got_response {
                Some(r) => r,
                None => {
                    return Err(last_err.unwrap_or_else(|| ProviderError::Api {
                        status: 429,
                        message: "rate-limited after max retries".into(),
                    }))
                }
            }
        };

        let mut parser = SseParser::new();
        let mut accumulator = StreamAccumulator::new();
        let mut response = response;

        while let Some(chunk) = response.chunk().await? {
            let events = parser.feed(&chunk)?;
            for event in events {
                self.process_sse_event(&event, &mut accumulator, &mut on_event)?;
            }
        }

        accumulator
            .into_response()
            .ok_or(ProviderError::UnexpectedEndOfStream)
    }

    fn build_request_body(&self, request: &LlmRequest, stream: bool) -> serde_json::Value {
        let mut body = serde_json::json!({
            "model": request.model,
            "max_tokens": request.max_tokens,
            "messages": request.messages,
            "stream": stream,
        });

        if let Some(system) = &request.system {
            body["system"] = serde_json::json!(system);
        }

        if !request.tools.is_empty() {
            body["tools"] = serde_json::json!(request.tools);
        }

        // Thinking/extended thinking — Anthropic uses budget_tokens.
        // Budget is clamped to max_tokens and must be >= 1024 (API minimum).
        if let Some(level) = &request.thinking {
            use crate::model_tier::ThinkingLevel;
            let raw_budget = match level {
                ThinkingLevel::Minimal => 0,
                ThinkingLevel::Low => 2000,
                ThinkingLevel::Medium => 5000,
                ThinkingLevel::High => 10000,
                ThinkingLevel::Max => request.max_tokens.saturating_sub(2000),
            };
            if raw_budget > 0 {
                let clamped = raw_budget.min(request.max_tokens);
                if clamped >= 1024 {
                    body["thinking"] = serde_json::json!({
                        "type": "enabled",
                        "budget_tokens": clamped,
                    });
                }
                // If clamped < 1024, skip thinking silently (too small for API minimum)
            }
        }

        body
    }

    fn process_sse_event(
        &self,
        event: &crate::sse::SseEvent,
        acc: &mut StreamAccumulator,
        on_event: &mut impl FnMut(StreamEvent),
    ) -> Result<(), ProviderError> {
        match event.event_type.as_str() {
            "message_start" => {
                let data: serde_json::Value = serde_json::from_str(&event.data)?;
                if let Some(msg) = data.get("message") {
                    acc.id = msg
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    acc.model = msg
                        .get("model")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    if let Some(usage) = msg.get("usage") {
                        acc.input_tokens = usage
                            .get("input_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as u32;
                    }
                }
            }
            "content_block_start" => {
                let data: serde_json::Value = serde_json::from_str(&event.data)?;
                if let Some(block) = data.get("content_block") {
                    let block_type = block
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default();
                    if block_type == "tool_use" {
                        let id = block
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        let name = block
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        acc.current_tool_id = Some(id.clone());
                        acc.current_tool_name = Some(name.clone());
                        acc.current_tool_input.clear();
                        on_event(StreamEvent::ToolUseStart { id, name });
                    }
                }
            }
            "content_block_delta" => {
                let data: serde_json::Value = serde_json::from_str(&event.data)?;
                if let Some(delta) = data.get("delta") {
                    let delta_type = delta
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default();
                    match delta_type {
                        "text_delta" => {
                            if let Some(text) = delta.get("text").and_then(|v| v.as_str()) {
                                acc.text.push_str(text);
                                on_event(StreamEvent::TextDelta(text.to_string()));
                            }
                        }
                        "input_json_delta" => {
                            if let Some(json) = delta.get("partial_json").and_then(|v| v.as_str()) {
                                acc.current_tool_input.push_str(json);
                                on_event(StreamEvent::InputJsonDelta(json.to_string()));
                            }
                        }
                        _ => {
                            debug!(delta_type, "unknown delta type");
                        }
                    }
                }
            }
            "content_block_stop" => {
                // If we were accumulating a tool use, finalize it
                if let (Some(id), Some(name)) =
                    (acc.current_tool_id.take(), acc.current_tool_name.take())
                {
                    // Architect fix: propagate JSON error instead of silent default
                    let input: serde_json::Value = if acc.current_tool_input.is_empty() {
                        serde_json::Value::Object(serde_json::Map::new())
                    } else {
                        serde_json::from_str(&acc.current_tool_input).map_err(|e| {
                            ProviderError::Json(format!(
                                "malformed tool input JSON for tool '{}': {}",
                                name, e
                            ))
                        })?
                    };
                    acc.content_blocks
                        .push(ContentBlock::ToolUse { id, name, input });
                    acc.current_tool_input.clear();
                }
            }
            "message_delta" => {
                let data: serde_json::Value = serde_json::from_str(&event.data)?;
                if let Some(delta) = data.get("delta") {
                    if let Some(reason) = delta.get("stop_reason").and_then(|v| v.as_str()) {
                        acc.stop_reason =
                            serde_json::from_value(serde_json::Value::String(reason.to_string()))
                                .ok();
                    }
                }
                if let Some(usage) = data.get("usage") {
                    acc.output_tokens = usage
                        .get("output_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32;
                }
            }
            "message_stop" | "ping" => {}
            other => {
                debug!(event_type = other, "unhandled SSE event type");
            }
        }
        Ok(())
    }
}

/// Accumulates SSE events into a complete LlmResponse.
///
/// **Ordering limitation**: Text deltas are accumulated into a single text block
/// placed at position 0. Tool use blocks follow. If the model interleaves text
/// and tool_use blocks, the original ordering is not preserved. This is acceptable
/// for Phase 1 where the agent loop uses `response.text()` and `response.tool_calls()`
/// which don't depend on ordering.
#[derive(Debug, Default)]
struct StreamAccumulator {
    id: String,
    model: String,
    text: String,
    content_blocks: Vec<ContentBlock>,
    stop_reason: Option<StopReason>,
    input_tokens: u32,
    output_tokens: u32,
    current_tool_id: Option<String>,
    current_tool_name: Option<String>,
    current_tool_input: String,
}

impl StreamAccumulator {
    fn new() -> Self {
        Self::default()
    }

    fn into_response(mut self) -> Option<LlmResponse> {
        if self.id.is_empty() {
            return None;
        }

        if !self.text.is_empty() {
            self.content_blocks
                .insert(0, ContentBlock::Text { text: self.text });
        }

        Some(LlmResponse {
            id: self.id,
            model: self.model,
            content: self.content_blocks,
            stop_reason: self.stop_reason,
            usage: Usage {
                input_tokens: self.input_tokens,
                output_tokens: self.output_tokens,
            },
        })
    }
}

/// Anthropic API response (non-streaming).
#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    id: String,
    model: String,
    content: Vec<ContentBlock>,
    stop_reason: Option<StopReason>,
    usage: Usage,
}

impl AnthropicResponse {
    fn into_llm_response(self) -> LlmResponse {
        LlmResponse {
            id: self.id,
            model: self.model,
            content: self.content,
            stop_reason: self.stop_reason,
            usage: self.usage,
        }
    }
}

#[async_trait::async_trait]
impl crate::provider::LlmProvider for AnthropicClient {
    async fn send(&mut self, request: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        AnthropicClient::send(self, request).await
    }

    async fn stream(
        &mut self,
        request: &LlmRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        // &mut dyn FnMut implements FnMut, so this delegates to the generic inherent method.
        AnthropicClient::stream(self, request, on_event).await
    }

    fn default_model(&self) -> &str {
        "claude-sonnet-4-6"
    }

    fn provider_id(&self) -> ProviderId {
        ProviderId::Anthropic
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_with_url_constructor() {
        let client = AnthropicClient::with_url(
            "test-key".into(),
            "http://localhost:8080/v1/messages".into(),
        );
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 100,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            task_type: None,
        };
        let body = client.build_request_body(&request, false);
        assert_eq!(body["model"], "claude-sonnet-4-6");
    }

    #[test]
    fn test_build_request_body_minimal() {
        let client = AnthropicClient::new("test-key".into());
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("hello")],
            max_tokens: 1024,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            task_type: None,
        };
        let body = client.build_request_body(&request, false);
        assert_eq!(body["model"], "claude-sonnet-4-6");
        assert_eq!(body["max_tokens"], 1024);
        assert_eq!(body["stream"], false);
        assert!(body.get("system").is_none());
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn test_build_request_body_with_system_and_tools() {
        let client = AnthropicClient::new("test-key".into());
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: Some("You are helpful.".into()),
            messages: vec![Message::user("hello")],
            max_tokens: 4096,
            tools: vec![ToolDefinition {
                name: "test".into(),
                description: "A test tool".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }],
            cell_id: Some("test-cell".into()),
            trace_id: Some("trace-123".into()),
            thinking: None,
            task_type: None,
        };
        let body = client.build_request_body(&request, true);
        assert_eq!(body["system"], "You are helpful.");
        assert_eq!(body["stream"], true);
        assert!(body["tools"].is_array());
    }

    #[test]
    fn test_build_request_body_thinking_low() {
        let client = AnthropicClient::new("test-key".into());
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("low effort")],
            max_tokens: 8000,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: Some(crate::model_tier::ThinkingLevel::Low),
            task_type: None,
        };
        let body = client.build_request_body(&request, false);
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 2000);
    }

    #[test]
    fn test_build_request_body_thinking_medium() {
        let client = AnthropicClient::new("test-key".into());
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("medium effort")],
            max_tokens: 8000,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: Some(crate::model_tier::ThinkingLevel::Medium),
            task_type: None,
        };
        let body = client.build_request_body(&request, false);
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 5000);
    }

    #[test]
    fn test_build_request_body_with_thinking_high() {
        let client = AnthropicClient::new("test-key".into());
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("think hard")],
            max_tokens: 16000,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: Some(crate::model_tier::ThinkingLevel::High),
            task_type: None,
        };
        let body = client.build_request_body(&request, false);
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 10000);
    }

    #[test]
    fn test_build_request_body_thinking_none() {
        let client = AnthropicClient::new("test-key".into());
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("quick")],
            max_tokens: 100,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            task_type: None,
        };
        let body = client.build_request_body(&request, false);
        assert!(body.get("thinking").is_none());
    }

    #[test]
    fn test_build_request_body_thinking_minimal_skipped() {
        let client = AnthropicClient::new("test-key".into());
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("classify")],
            max_tokens: 100,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: Some(crate::model_tier::ThinkingLevel::Minimal),
            task_type: None,
        };
        let body = client.build_request_body(&request, false);
        assert!(body.get("thinking").is_none()); // Minimal = skip
    }

    #[test]
    fn test_build_request_body_thinking_max() {
        let client = AnthropicClient::new("test-key".into());
        let request = LlmRequest {
            model: "claude-opus-4-6".into(),
            system: None,
            messages: vec![Message::user("deep analysis")],
            max_tokens: 32000,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: Some(crate::model_tier::ThinkingLevel::Max),
            task_type: None,
        };
        let body = client.build_request_body(&request, false);
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 30000); // 32000 - 2000
    }

    #[test]
    fn test_build_request_body_thinking_high_clamped_to_max_tokens() {
        let client = AnthropicClient::new("test-key".into());
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("think")],
            max_tokens: 4096,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: Some(crate::model_tier::ThinkingLevel::High),
            task_type: None,
        };
        let body = client.build_request_body(&request, false);
        let budget = body["thinking"]["budget_tokens"].as_u64().unwrap();
        assert!(
            budget <= 4096,
            "budget {budget} should be <= max_tokens 4096"
        );
        assert!(budget >= 1024, "budget {budget} should be >= minimum 1024");
    }

    #[test]
    fn test_build_request_body_thinking_skipped_when_max_tokens_too_small() {
        let client = AnthropicClient::new("test-key".into());
        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("tiny")],
            max_tokens: 500,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: Some(crate::model_tier::ThinkingLevel::Low),
            task_type: None,
        };
        let body = client.build_request_body(&request, false);
        assert!(
            body.get("thinking").is_none(),
            "budget too small for 1024 minimum"
        );
    }

    #[test]
    fn test_stream_accumulator_text_only() {
        let mut acc = StreamAccumulator::new();
        acc.id = "msg_123".into();
        acc.model = "claude-sonnet-4-6".into();
        acc.text = "Hello world".into();
        acc.stop_reason = Some(StopReason::EndTurn);
        acc.input_tokens = 10;
        acc.output_tokens = 5;

        let response = acc.into_response().unwrap();
        assert_eq!(response.id, "msg_123");
        assert_eq!(response.text(), Some("Hello world"));
        assert_eq!(response.stop_reason, Some(StopReason::EndTurn));
        assert_eq!(response.usage.input_tokens, 10);
    }

    #[test]
    fn test_stream_accumulator_with_tool_use() {
        let mut acc = StreamAccumulator::new();
        acc.id = "msg_456".into();
        acc.model = "claude-sonnet-4-6".into();
        acc.text = "Let me check.".into();
        acc.content_blocks.push(ContentBlock::ToolUse {
            id: "toolu_1".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": "/tmp/test"}),
        });
        acc.stop_reason = Some(StopReason::ToolUse);

        let response = acc.into_response().unwrap();
        assert_eq!(response.content.len(), 2);
        assert_eq!(response.tool_calls().len(), 1);
    }

    #[test]
    fn test_stream_accumulator_empty_returns_none() {
        let acc = StreamAccumulator::new();
        assert!(acc.into_response().is_none());
    }

    #[test]
    fn test_new_creates_api_key_auth() {
        let client = AnthropicClient::new("sk-ant-api-test".into());
        assert!(!client.auth.is_oauth());
    }

    #[test]
    fn test_from_auth_oauth() {
        let auth = crate::auth::AuthMethod::OAuth {
            access_token: "sk-ant-oat01-test".into(),
            refresh_token: "refresh".into(),
            expires_ms: 9999999999999,
        };
        let client = AnthropicClient::from_auth(auth);
        assert!(client.auth.is_oauth());
    }

    #[test]
    fn test_anthropic_response_deserialization() {
        let json = r#"{
            "id": "msg_test",
            "model": "claude-sonnet-4-6",
            "content": [{"type": "text", "text": "Hello!"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 3}
        }"#;
        let response: AnthropicResponse = serde_json::from_str(json).unwrap();
        let llm = response.into_llm_response();
        assert_eq!(llm.text(), Some("Hello!"));
        assert_eq!(llm.stop_reason, Some(StopReason::EndTurn));
    }

    #[test]
    fn test_process_sse_events_full_text_stream() {
        // Architect requested: test process_sse_event through a full event sequence
        let client = AnthropicClient::new("test-key".into());
        let mut acc = StreamAccumulator::new();
        let mut events_received: Vec<String> = Vec::new();

        let sse_events = vec![
            crate::sse::SseEvent {
                event_type: "message_start".into(),
                data: r#"{"type":"message_start","message":{"id":"msg_1","model":"claude-sonnet-4-6","usage":{"input_tokens":25}}}"#.into(),
            },
            crate::sse::SseEvent {
                event_type: "content_block_start".into(),
                data: r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#.into(),
            },
            crate::sse::SseEvent {
                event_type: "content_block_delta".into(),
                data: r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#.into(),
            },
            crate::sse::SseEvent {
                event_type: "content_block_delta".into(),
                data: r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" world"}}"#.into(),
            },
            crate::sse::SseEvent {
                event_type: "content_block_stop".into(),
                data: r#"{"type":"content_block_stop","index":0}"#.into(),
            },
            crate::sse::SseEvent {
                event_type: "message_delta".into(),
                data: r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}"#.into(),
            },
            crate::sse::SseEvent {
                event_type: "message_stop".into(),
                data: r#"{"type":"message_stop"}"#.into(),
            },
        ];

        for event in &sse_events {
            client
                .process_sse_event(event, &mut acc, &mut |se| {
                    events_received.push(format!("{:?}", se));
                })
                .unwrap();
        }

        let response = acc.into_response().unwrap();
        assert_eq!(response.id, "msg_1");
        assert_eq!(response.model, "claude-sonnet-4-6");
        assert_eq!(response.text(), Some("Hello world"));
        assert_eq!(response.stop_reason, Some(StopReason::EndTurn));
        assert_eq!(response.usage.input_tokens, 25);
        assert_eq!(response.usage.output_tokens, 3);
        // Verify callback was called with text deltas
        assert!(events_received.iter().any(|e| e.contains("Hello")));
        assert!(events_received.iter().any(|e| e.contains(" world")));
    }

    #[test]
    fn test_process_sse_events_tool_use_stream() {
        let client = AnthropicClient::new("test-key".into());
        let mut acc = StreamAccumulator::new();
        acc.id = "msg_2".into();
        acc.model = "claude-sonnet-4-6".into();
        let mut callback_events = Vec::new();

        let sse_events = vec![
            crate::sse::SseEvent {
                event_type: "content_block_start".into(),
                data: r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_abc","name":"read_file"}}"#.into(),
            },
            crate::sse::SseEvent {
                event_type: "content_block_delta".into(),
                data: r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#.into(),
            },
            crate::sse::SseEvent {
                event_type: "content_block_delta".into(),
                data: r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"/tmp/test\"}"}}"#.into(),
            },
            crate::sse::SseEvent {
                event_type: "content_block_stop".into(),
                data: r#"{"type":"content_block_stop","index":1}"#.into(),
            },
        ];

        for event in &sse_events {
            client
                .process_sse_event(event, &mut acc, &mut |se| {
                    callback_events.push(format!("{:?}", se));
                })
                .unwrap();
        }

        assert_eq!(acc.content_blocks.len(), 1);
        match &acc.content_blocks[0] {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "toolu_abc");
                assert_eq!(name, "read_file");
                assert_eq!(input["path"], "/tmp/test");
            }
            _ => panic!("expected ToolUse block"),
        }
        assert!(callback_events.iter().any(|e| e.contains("ToolUseStart")));
    }

    #[test]
    fn test_process_sse_event_malformed_json_returns_error() {
        let client = AnthropicClient::new("test-key".into());
        let mut acc = StreamAccumulator::new();
        let bad_event = crate::sse::SseEvent {
            event_type: "message_start".into(),
            data: "{ this is not json".into(),
        };
        let result = client.process_sse_event(&bad_event, &mut acc, &mut |_| {});
        assert!(result.is_err());
    }

    #[test]
    fn test_process_sse_event_malformed_tool_input_json() {
        let client = AnthropicClient::new("test-key".into());
        let mut acc = StreamAccumulator::new();
        acc.id = "msg_1".into();
        acc.current_tool_id = Some("toolu_bad".into());
        acc.current_tool_name = Some("some_tool".into());
        acc.current_tool_input = "{ broken json".into();
        let stop_event = crate::sse::SseEvent {
            event_type: "content_block_stop".into(),
            data: r#"{"type":"content_block_stop","index":0}"#.into(),
        };
        let result = client.process_sse_event(&stop_event, &mut acc, &mut |_| {});
        assert!(result.is_err());
    }

    // ── Anthropic-specific auth tests (moved from auth/mod.rs) ───────

    #[test]
    fn test_load_auth_json_valid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(
            &path,
            r#"{
                "anthropic": {
                    "type": "oauth",
                    "access": "sk-ant-oat01-test",
                    "refresh": "sk-ant-ort01-test",
                    "expires": 9999999999999
                }
            }"#,
        )
        .unwrap();
        let result = load_auth_json(&path).unwrap();
        assert!(result.is_some());
        match result.unwrap() {
            AuthMethod::OAuth { access_token, .. } => {
                assert!(access_token.contains("oat01"));
            }
            _ => panic!("expected OAuth"),
        }
    }

    #[test]
    fn test_load_auth_json_no_anthropic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(
            &path,
            r#"{"openai": {"type": "oauth", "access": "x", "refresh": "y", "expires": 0}}"#,
        )
        .unwrap();
        let result = load_auth_json(&path).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_load_auth_json_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(&path, "not json").unwrap();
        assert!(load_auth_json(&path).is_err());
    }

    #[test]
    fn test_save_auth_json_preserves_other_providers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(
            &path,
            r#"{"openai": {"type": "oauth", "access": "x", "refresh": "y", "expires": 0}}"#,
        )
        .unwrap();

        let auth = AuthMethod::OAuth {
            access_token: "new-access".into(),
            refresh_token: "new-refresh".into(),
            expires_ms: 12345,
        };
        save_auth_json(&path, &auth).unwrap();

        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(content.get("openai").is_some());
        assert_eq!(content["anthropic"]["access"], "new-access");
    }

    #[test]
    fn test_load_auth_json_api_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(&path, r#"{"anthropic":{"type":"api_key","key":"sk-test"}}"#).unwrap();
        let method = load_auth_json(&path).unwrap().unwrap();
        assert!(matches!(method, AuthMethod::ApiKey(k) if k == "sk-test"));
    }

    #[test]
    fn test_save_auth_json_api_key_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        save_auth_json(&path, &AuthMethod::ApiKey("sk-roundtrip".into())).unwrap();
        let method = load_auth_json(&path).unwrap().unwrap();
        assert!(matches!(method, AuthMethod::ApiKey(k) if k == "sk-roundtrip"));
    }

    #[test]
    fn test_resolve_with_cortex_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("auth.json"),
            r#"{"anthropic":{"type":"api_key","key":"sk-cortex"}}"#,
        )
        .unwrap();
        let method = resolve_anthropic_auth(None, Some(dir.path())).unwrap();
        assert!(matches!(method, AuthMethod::ApiKey(k) if k == "sk-cortex"));
    }

    #[cfg(unix)]
    #[test]
    fn test_save_auth_json_sets_0o600_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        save_auth_json(&path, &AuthMethod::ApiKey("sk-test".into())).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_load_claude_code_keychain_returns_some_or_none() {
        // This test verifies the function doesn't panic regardless of keychain state.
        // On CI or machines without Claude Code it returns None; on dev machines with
        // Claude Code installed it returns Some with valid OAuth tokens.
        let result = load_claude_code_keychain();
        if let Some(AuthMethod::OAuth { access_token, .. }) = &result {
            assert!(
                access_token.starts_with("sk-ant-"),
                "keychain token should have sk-ant- prefix"
            );
        }
        // None is also acceptable — no Claude Code installed
    }
}
