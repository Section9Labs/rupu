use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest_middleware::ClientWithMiddleware;
use tracing::{debug, error, info, warn};

use crate::auth::{is_token_expired, save_provider_auth, AuthCredentials};
use crate::error::ProviderError;
use crate::sse::SseParser;
use crate::types::*;

pub mod code_assist;

// ── Endpoint URLs ────────────────────────────────────────────────────

const GEMINI_CLI_ENDPOINT: &str = "https://cloudcode-pa.googleapis.com";
const ANTIGRAVITY_ENDPOINT: &str = "https://daily-cloudcode-pa.sandbox.googleapis.com";
/// AI Studio (api-key) endpoint. Distinct from the Cloud Code Assist
/// endpoint above — different URL pattern, different request shape,
/// different auth header.
const AI_STUDIO_ENDPOINT: &str = "https://generativelanguage.googleapis.com";

const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// Netflow-instrumented client bound to `sink`. No run context is
/// available at construction — stamped
/// `FlowCtx::system(Origin::Provider("gemini"))`. `with_tuning` (the
/// path every factory-built client goes through) rebuilds this with the
/// configured timeout via `client_with`, reusing the same sink.
///
/// Fallible rather than panicking: `client_with`'s only failure mode is
/// `builder.build()` (TLS backend / resolver init), an environment
/// condition a long-lived daemon (`rupu cp serve`) can genuinely hit per
/// run. `new` already returns `Result`, so this propagates via `?`
/// instead of aborting the process.
fn gemini_http_client(
    sink: Arc<dyn rupu_netflow::FlowSink>,
) -> Result<ClientWithMiddleware, ProviderError> {
    let ctx = rupu_netflow::FlowCtx::system(rupu_netflow::Origin::Provider("gemini".into()));
    Ok(rupu_netflow::http::shared_client(
        ctx,
        rupu_netflow::http::Transport::default(),
        sink,
    )?)
}

/// Canonical provider tag stamped on Reasoning blocks; gates the replay.
pub(crate) const PROVIDER_TAG: &str = "google_gemini";

// Google's public CLI OAuth client IDs and secrets (same as Pi, and as
// google-gemini/gemini-cli's `packages/core/src/code_assist/oauth2.ts`).
// These are embedded in all CLI tools that use Google OAuth (safe to embed:
// an installed application's "secret" is not treated as one). Public so
// `rupu-auth`'s login and refresh send the same pair this client does.
pub const GEMINI_CLI_CLIENT_ID: &str =
    "681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com";
pub const GEMINI_CLI_CLIENT_SECRET: &str = "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";

pub const ANTIGRAVITY_CLIENT_ID: &str =
    "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
pub const ANTIGRAVITY_CLIENT_SECRET: &str = "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";

/// Which Google Gemini variant to use. The first two are Cloud Code
/// Assist (OAuth, paid Gemini-CLI / Antigravity quotas); `AiStudio`
/// is the public api-key endpoint at generativelanguage.googleapis.com
/// for users with an AI Studio API key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeminiVariant {
    /// Production Cloud Code Assist: cloudcode-pa.googleapis.com (OAuth).
    GeminiCli,
    /// Sandbox Cloud Code Assist: daily-cloudcode-pa.sandbox.googleapis.com (OAuth).
    Antigravity,
    /// AI Studio public endpoint: generativelanguage.googleapis.com (api-key).
    AiStudio,
}

impl GeminiVariant {
    /// The OAuth variant a stored credential's `extra.variant` hint names:
    /// `"antigravity"` → [`GeminiVariant::Antigravity`], anything else (or
    /// no hint) → the production [`GeminiVariant::GeminiCli`]. The one
    /// mapping the factory and the credential store's refresh share, so a
    /// refresh always carries the client id/secret pair the credential was
    /// issued to.
    pub fn from_credential_hint(hint: Option<&str>) -> Self {
        match hint {
            Some("antigravity") => GeminiVariant::Antigravity,
            _ => GeminiVariant::GeminiCli,
        }
    }

    /// The `extra.variant` hint a credential issued to this variant's OAuth
    /// client carries — the inverse of [`Self::from_credential_hint`].
    /// `None` for AI Studio, which has no OAuth client.
    pub fn credential_hint(&self) -> Option<&'static str> {
        match self {
            GeminiVariant::GeminiCli => Some("gemini-cli"),
            GeminiVariant::Antigravity => Some("antigravity"),
            GeminiVariant::AiStudio => None,
        }
    }

    /// `true` when this variant uses an AI Studio api-key (no OAuth
    /// refresh, different URL pattern, different request body shape).
    fn is_api_key(&self) -> bool {
        matches!(self, GeminiVariant::AiStudio)
    }

    fn endpoint(&self) -> &'static str {
        match self {
            GeminiVariant::GeminiCli => GEMINI_CLI_ENDPOINT,
            GeminiVariant::Antigravity => ANTIGRAVITY_ENDPOINT,
            GeminiVariant::AiStudio => AI_STUDIO_ENDPOINT,
        }
    }

    /// The OAuth client id this variant's tokens were issued to.
    pub fn client_id(&self) -> &'static str {
        match self {
            GeminiVariant::GeminiCli => GEMINI_CLI_CLIENT_ID,
            GeminiVariant::Antigravity => ANTIGRAVITY_CLIENT_ID,
            // AI Studio doesn't use OAuth client credentials. Returning
            // an empty &'static str keeps the impl total without
            // forcing every caller to handle Option.
            GeminiVariant::AiStudio => "",
        }
    }

    /// The matching client secret (empty for AI Studio, which has none).
    pub fn client_secret(&self) -> &'static str {
        match self {
            GeminiVariant::GeminiCli => GEMINI_CLI_CLIENT_SECRET,
            GeminiVariant::Antigravity => ANTIGRAVITY_CLIENT_SECRET,
            GeminiVariant::AiStudio => "",
        }
    }

    fn provider_id(&self) -> crate::provider_id::ProviderId {
        match self {
            GeminiVariant::GeminiCli => crate::provider_id::ProviderId::GoogleGeminiCli,
            GeminiVariant::Antigravity => crate::provider_id::ProviderId::GoogleAntigravity,
            // AI Studio shares the GeminiCli provider id for now —
            // they target the same Gemini models, just via different
            // billing surfaces. Distinct ids would let us track
            // usage separately; revisit when usage analytics demand it.
            GeminiVariant::AiStudio => crate::provider_id::ProviderId::GoogleGeminiCli,
        }
    }

    fn user_agent(&self) -> &'static str {
        match self {
            GeminiVariant::GeminiCli => "google-cloud-sdk vscode_cloudshelleditor/0.1",
            GeminiVariant::Antigravity => "antigravity/1.18.4 darwin/arm64",
            GeminiVariant::AiStudio => "rupu/0.3 (https://github.com/Section9Labs/rupu)",
        }
    }
}

/// Google Gemini client for Cloud Code Assist API.
/// Shared implementation for both Gemini CLI and Antigravity variants.
pub struct GoogleGeminiClient {
    client: ClientWithMiddleware,
    /// The exact sink `client` was bound to, so `with_tuning`'s rebuild
    /// keeps using the same run's sink. See `AnthropicClient`'s `sink`.
    sink: Arc<dyn rupu_netflow::FlowSink>,
    variant: GeminiVariant,
    access_token: String,
    refresh_token: String,
    expires_ms: u64,
    /// The Code Assist project (a Google Cloud project ID) the credential
    /// stores: `extra.project_id`. Empty until the account is set up — at
    /// login, or by this client's first request ([`Self::ensure_project`]).
    project_id: String,
    /// The `GOOGLE_CLOUD_PROJECT` override's project, once
    /// [`Self::ensure_project`] set the account up with it: requests carry it
    /// instead of `project_id`. Never stored.
    override_project: Option<String>,
    /// [`Self::ensure_project`] has settled the project requests carry.
    project_settled: bool,
    /// Where Code Assist calls go (setup and generateContent):
    /// [`code_assist::endpoint`] — the variant's endpoint, or the
    /// `RUPU_CODE_ASSIST_ENDPOINT_OVERRIDE` test seam.
    code_assist_base: String,
    /// How the client reads `GOOGLE_CLOUD_PROJECT` / `GOOGLE_CLOUD_PROJECT_ID`
    /// (`std::env::var`; tests swap it out).
    env: fn(&str) -> Option<String>,
    /// The onboarding operation's poll ([`code_assist::OnboardPoll`]).
    onboard_poll: code_assist::OnboardPoll,
    auth_json_path: Option<PathBuf>,
    /// Test seam: replaces variant.endpoint() for model listing only.
    pub(crate) api_base_override: Option<String>,
    /// The OAuth token endpoint ([`GOOGLE_TOKEN_URL`]; tests point it at a
    /// mock).
    token_url: String,
    /// A token refresh this client started and is (or was) waiting for. Kept
    /// across a dropped call so the next call adopts it instead of starting
    /// a second refresh.
    pending_refresh: Option<PendingGoogleRefresh>,
    /// The credential store's refresher (rupu-auth's `KeychainResolver`):
    /// when set, token refreshes go through it — under the store's
    /// cross-process lock, persisted — instead of this client's own token
    /// request, whose result would otherwise live only in memory.
    oauth_refresher: Option<Arc<dyn crate::credential_writes::OAuthRefresher>>,
}

/// A spawned Google token refresh: `(access_token, refresh_token, expires_ms)`.
type PendingGoogleRefresh = tokio::task::JoinHandle<Result<(String, String, u64), ProviderError>>;

impl GoogleGeminiClient {
    /// Apply `[providers.<name>]` tuning — currently the `timeout_ms`
    /// inactivity deadline (ISSUES.md I-9) on this client's HTTP stack.
    /// Applied as connect + read timeouts so a long streaming generation is
    /// never cut off mid-flight. A builder failure leaves the existing client
    /// in place rather than panicking on user-supplied config.
    pub fn with_tuning(mut self, tuning: &crate::tuning::ProviderTuning) -> Self {
        let ctx = rupu_netflow::FlowCtx::system(rupu_netflow::Origin::Provider("gemini".into()));
        if let Ok(client) =
            rupu_netflow::http::shared_client(ctx, tuning.transport(), self.sink.clone())
        {
            self.client = client;
        }
        self
    }

    /// Create from resolved AuthCredentials + variant.
    ///
    /// Auth-mode rules:
    /// - `GeminiCli` / `Antigravity` (Cloud Code Assist) require
    ///   OAuth credentials. Their Code Assist project is the credential's
    ///   `extra.project_id`, or is set up before the first request
    ///   ([`Self::ensure_project`]). Bare api-key creds are rejected.
    /// - `AiStudio` requires `ApiKey` credentials. The api-key is
    ///   stored in `access_token` and `refresh_token` is empty so
    ///   `ensure_valid_token` never tries to refresh.
    ///
    /// `sink` is the run's netflow sink; there is no process-global
    /// fallback.
    pub fn new(
        creds: AuthCredentials,
        variant: GeminiVariant,
        auth_json_path: Option<PathBuf>,
        sink: Arc<dyn rupu_netflow::FlowSink>,
    ) -> Result<Self, ProviderError> {
        match (creds, variant) {
            (
                AuthCredentials::OAuth {
                    access,
                    refresh,
                    expires,
                    extra,
                },
                GeminiVariant::GeminiCli | GeminiVariant::Antigravity,
            ) => {
                let project_id = extra
                    .get(code_assist::EXTRA_PROJECT_ID)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                Ok(Self {
                    client: gemini_http_client(sink.clone())?,
                    sink,
                    variant,
                    access_token: access,
                    refresh_token: refresh,
                    expires_ms: expires,
                    project_id,
                    override_project: None,
                    project_settled: false,
                    code_assist_base: code_assist::endpoint(variant),
                    env: read_env,
                    onboard_poll: code_assist::OnboardPoll::default(),
                    auth_json_path,
                    api_base_override: None,
                    token_url: GOOGLE_TOKEN_URL.to_string(),
                    pending_refresh: None,
                    oauth_refresher: None,
                })
            }
            (AuthCredentials::ApiKey { key, .. }, GeminiVariant::AiStudio) => Ok(Self {
                client: gemini_http_client(sink.clone())?,
                sink,
                variant,
                access_token: key,
                refresh_token: String::new(),
                expires_ms: 0,
                project_id: String::new(),
                override_project: None,
                project_settled: false,
                code_assist_base: code_assist::endpoint(variant),
                env: read_env,
                onboard_poll: code_assist::OnboardPoll::default(),
                auth_json_path: None,
                api_base_override: None,
                token_url: GOOGLE_TOKEN_URL.to_string(),
                pending_refresh: None,
                oauth_refresher: None,
            }),
            (AuthCredentials::ApiKey { .. }, _) => Err(ProviderError::AuthConfig(
                "Google Cloud Code Assist (gemini-cli / antigravity) requires OAuth, \
                 not API key. Use the AI Studio variant for api-key auth."
                    .into(),
            )),
            (AuthCredentials::OAuth { .. }, GeminiVariant::AiStudio) => {
                Err(ProviderError::AuthConfig(
                    "AI Studio requires API-key auth, not OAuth. Run `rupu auth login \
                 --provider gemini --mode api-key`."
                        .into(),
                ))
            }
        }
    }

    /// Non-streaming send.
    pub async fn send(&mut self, request: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        self.ensure_valid_token().await?;
        self.ensure_project().await?;
        let body = self.build_request_body(request);
        let url = self.build_url(&request.model);

        let response = self
            .client
            .post(&url)
            .headers(self.build_headers()?)
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let headers = response.headers().clone();
            let text = response.text().await.unwrap_or_default();
            return Err(crate::error::api_error_from_response(
                status,
                &headers,
                extract_google_error(&text),
            ));
        }

        let resp_json: serde_json::Value = response.json().await?;
        parse_generate_content_response(&resp_json, &request.model)
    }

    /// Streaming send with SSE.
    pub async fn stream(
        &mut self,
        request: &LlmRequest,
        on_event: &mut (impl FnMut(StreamEvent) + Send + ?Sized),
    ) -> Result<LlmResponse, ProviderError> {
        self.ensure_valid_token().await?;
        self.ensure_project().await?;
        let body = self.build_request_body(request);
        let url = self.build_stream_url(&request.model);

        let response = self
            .client
            .post(&url)
            .headers(self.build_headers()?)
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let headers = response.headers().clone();
            let text = response.text().await.unwrap_or_default();
            return Err(crate::error::api_error_from_response(
                status,
                &headers,
                extract_google_error(&text),
            ));
        }

        let mut parser = SseParser::new();
        let mut acc = GeminiAccumulator::new(&request.model);
        let mut bytes_stream = response.bytes_stream();

        use futures_util::StreamExt;
        while let Some(chunk) = bytes_stream.next().await {
            let chunk = chunk.map_err(|e| ProviderError::Http(e.to_string()))?;
            let events = parser.feed(&chunk)?;
            for event in events {
                process_gemini_sse(&event, &mut acc, on_event)?;
            }
        }

        acc.into_response()
            .ok_or(ProviderError::UnexpectedEndOfStream)
    }

    fn build_url(&self, model: &str) -> String {
        match self.variant {
            GeminiVariant::AiStudio => format!(
                "{}/v1beta/models/{}:generateContent",
                self.variant.endpoint(),
                model
            ),
            _ => format!("{}/v1internal:generateContent", self.code_assist_base),
        }
    }

    fn build_stream_url(&self, model: &str) -> String {
        match self.variant {
            GeminiVariant::AiStudio => format!(
                "{}/v1beta/models/{}:streamGenerateContent?alt=sse",
                self.variant.endpoint(),
                model
            ),
            _ => format!(
                "{}/v1internal:streamGenerateContent?alt=sse",
                self.code_assist_base
            ),
        }
    }

    fn build_headers(&self) -> Result<reqwest::header::HeaderMap, ProviderError> {
        let mut headers = reqwest::header::HeaderMap::new();
        match self.variant {
            GeminiVariant::AiStudio => {
                let key_val = self.access_token.parse().map_err(|_| {
                    ProviderError::AuthConfig("api key contains invalid header characters".into())
                })?;
                headers.insert("x-goog-api-key", key_val);
            }
            _ => {
                let auth_val = format!("Bearer {}", self.access_token)
                    .parse()
                    .map_err(|_| {
                        ProviderError::AuthConfig(
                            "access token contains invalid header characters".into(),
                        )
                    })?;
                headers.insert(reqwest::header::AUTHORIZATION, auth_val);
            }
        }
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );
        headers.insert(
            reqwest::header::ACCEPT,
            "text/event-stream".parse().unwrap(),
        );
        headers.insert(
            reqwest::header::USER_AGENT,
            self.variant.user_agent().parse().unwrap(),
        );
        Ok(headers)
    }

    fn build_request_body(&self, request: &LlmRequest) -> serde_json::Value {
        // Convert messages to Gemini contents format
        let contents = convert_messages(&request.messages);

        let mut inner_request = serde_json::json!({
            "contents": contents,
        });

        // System instruction
        if let Some(system) = &request.system {
            inner_request["systemInstruction"] = serde_json::json!({
                "parts": [{"text": system}]
            });
        }

        // Generation config
        let mut gen_config = serde_json::json!({});
        if let Some(n) = request.max_tokens {
            gen_config["maxOutputTokens"] = serde_json::json!(n);
        }

        // Thinking config. `Auto` uses Gemini's dynamic-budget sentinel
        // (`thinkingBudget: -1`); the level field is omitted in that
        // case because the server picks.
        //
        // For non-`Auto` levels, `thinkingLevel` and `thinkingBudget` are
        // two different generations' controls, not two names for the same
        // thing: `thinkingLevel` is a Gemini-3-only enum key, while
        // `thinkingBudget` is the numeric knob Gemini 2.5 understands.
        // Per Google's documented contract, exactly one is sent, gated on
        // the model string (not the provider/variant — both model
        // families share this provider). The level string is sent
        // lowercase, matching the documented casing (tolerance for
        // uppercase is unverified against the live API either way).
        if let Some(level) = &request.thinking {
            use crate::model_tier::ThinkingLevel;
            let is_gemini_3 = request.model.starts_with("gemini-3");
            gen_config["thinkingConfig"] = match level {
                ThinkingLevel::Auto => serde_json::json!({
                    "includeThoughts": true,
                    "thinkingBudget": -1,
                }),
                _ => {
                    let (level_str, budget) = match level {
                        ThinkingLevel::Minimal => ("minimal", 128),
                        ThinkingLevel::Low => ("low", 2048),
                        ThinkingLevel::Medium => ("medium", 8192),
                        ThinkingLevel::High => ("high", 32768),
                        // Gemini's API caps thinkingBudget at 32768; Max
                        // is clamped to High for now.
                        ThinkingLevel::Max => ("high", 32768),
                        ThinkingLevel::Auto => unreachable!(),
                    };
                    if is_gemini_3 {
                        serde_json::json!({
                            "includeThoughts": true,
                            "thinkingLevel": level_str,
                        })
                    } else {
                        serde_json::json!({
                            "includeThoughts": true,
                            "thinkingBudget": budget,
                        })
                    }
                }
            };
        }

        inner_request["generationConfig"] = gen_config;

        // Tools
        if !request.tools.is_empty() {
            let declarations: Vec<serde_json::Value> = request
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema,
                    })
                })
                .collect();
            inner_request["tools"] = serde_json::json!([{"functionDeclarations": declarations}]);
        }

        // AI Studio's API takes the inner request directly. Cloud
        // Code Assist wraps it in an envelope with project + model
        // + userAgent + requestId routing fields.
        if self.variant == GeminiVariant::AiStudio {
            return inner_request;
        }
        let user_agent = if self.variant == GeminiVariant::Antigravity {
            "antigravity"
        } else {
            "phi-coding-agent"
        };
        serde_json::json!({
            "project": self.override_project.as_deref().unwrap_or(&self.project_id),
            "model": request.model,
            "userAgent": user_agent,
            "requestId": format!("phi-{}-{}", now_ms(), request_counter()),
            "request": inner_request,
        })
    }

    /// Settle the Google Cloud project this client's Code Assist requests
    /// carry, before the first one — gemini-cli runs its `setupUser` before
    /// its first request too ([`code_assist`]):
    ///
    /// - `GOOGLE_CLOUD_PROJECT` (else `GOOGLE_CLOUD_PROJECT_ID`) overrides:
    ///   the account is set up with it — unless it already is the stored
    ///   project — and its project is used by this client only, never stored;
    /// - else the credential's stored `project_id`;
    /// - else the account is set up now (a login from before rupu stored
    ///   projects, or one whose login-time setup failed) and the project is
    ///   recorded in the credential store, so the next process skips this.
    ///
    /// Fails with the setup's actionable error rather than let a request go
    /// out with an empty project; a failed setup is retried by the next call.
    async fn ensure_project(&mut self) -> Result<(), ProviderError> {
        if self.variant.is_api_key() || self.project_settled {
            return Ok(());
        }
        match code_assist::project_override(self.env)? {
            Some(o) if o.project == self.project_id => {}
            Some(o) => {
                let (project, _) = self.set_up_user(Some(&o.project)).await?;
                info!(var = o.var, requested = %o.project, project = %project, "Code Assist project set up from the environment");
                self.override_project = Some(project);
            }
            None if !self.project_id.is_empty() => {}
            None => {
                let (project, ran_it) = self.set_up_user(None).await?;
                // Settled before the record, so a caller dropped while the
                // store waits for its lock leaves this client settled.
                self.project_id = project.clone();
                self.project_settled = true;
                // The client that ran the setup records it; the ones that
                // shared its result (see `shared_setup`) leave it to that one.
                if ran_it {
                    self.record_project(&project).await;
                }
                return Ok(());
            }
        }
        self.project_settled = true;
        Ok(())
    }

    /// The setup for this client's account at its endpoint, shared with
    /// every other client in the process asking for the same
    /// ([`code_assist::shared_setup`]); `true` when this call ran it. The
    /// grant is keyed by a hash of its refresh token (the access token when
    /// there is none), so no token is kept beyond the clients holding it.
    async fn set_up_user(&self, project: Option<&str>) -> Result<(String, bool), ProviderError> {
        use std::hash::{Hash, Hasher};
        let grant = if self.refresh_token.is_empty() {
            &self.access_token
        } else {
            &self.refresh_token
        };
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        grant.hash(&mut hasher);
        let key = format!(
            "{}\n{:?}\n{:016x}\n{}",
            self.code_assist_base,
            self.variant,
            hasher.finish(),
            project.unwrap_or_default()
        );
        code_assist::shared_setup(
            key,
            code_assist::setup_user(
                &self.client,
                &self.code_assist_base,
                self.variant,
                &self.access_token,
                project,
                &self.onboard_poll,
            ),
        )
        .await
    }

    /// Record a freshly set-up project (and the variant it was set up for)
    /// in the stored credential. Best-effort: this client has the project
    /// either way, and a process that finds none stored sets it up again.
    async fn record_project(&self, project: &str) {
        match (&self.oauth_refresher, &self.auth_json_path) {
            (Some(store), _) => {
                let mut fields = HashMap::new();
                fields.insert(
                    code_assist::EXTRA_PROJECT_ID.to_string(),
                    serde_json::Value::String(project.to_string()),
                );
                if let Some(hint) = self.variant.credential_hint() {
                    fields.insert(
                        code_assist::EXTRA_VARIANT.to_string(),
                        serde_json::Value::String(hint.to_string()),
                    );
                }
                let holder = AuthCredentials::OAuth {
                    access: self.access_token.clone(),
                    refresh: self.refresh_token.clone(),
                    expires: self.expires_ms,
                    extra: HashMap::new(),
                };
                match store.record_extra(holder, fields).await {
                    Ok(true) => info!(project, "recorded the Code Assist project in the stored credential"),
                    Ok(false) => info!(project, "the stored credential is no longer the one this client holds (a re-login or logout since, or another holder's token rotation); the Code Assist project was not recorded in it"),
                    Err(e) => warn!(project, error = %e, "the Code Assist project could not be recorded in the stored credential; the next process sets it up again"),
                }
            }
            // The legacy auth.json (`ProviderRegistry`): keyed per variant,
            // written whole, as its refresh writes it.
            (None, Some(path)) => {
                let mut extra = HashMap::new();
                extra.insert(
                    code_assist::EXTRA_PROJECT_ID.to_string(),
                    serde_json::Value::String(project.to_string()),
                );
                let creds = AuthCredentials::OAuth {
                    access: self.access_token.clone(),
                    refresh: self.refresh_token.clone(),
                    expires: self.expires_ms,
                    extra,
                };
                let (path, provider) = (path.clone(), self.variant.provider_id());
                let written = tokio::task::spawn_blocking({
                    let path = path.clone();
                    move || save_provider_auth(&path, provider, &creds)
                })
                .await
                .map_err(|e| ProviderError::AuthConfig(format!("write task failed: {e}")))
                .and_then(|r| r);
                if let Err(e) = written {
                    warn!(path = %path.display(), project, error = %e, "the Code Assist project could not be written; the next process sets it up again");
                }
            }
            (None, None) => warn!(project, "no credential store to record the Code Assist project in; the next process sets it up again"),
        }
    }

    /// Hand token refreshes to the credential store's refresher (see the
    /// `oauth_refresher` field). `None` keeps the client's own refresh.
    pub fn with_oauth_refresher(
        mut self,
        refresher: Option<Arc<dyn crate::credential_writes::OAuthRefresher>>,
    ) -> Self {
        self.oauth_refresher = refresher;
        self
    }

    /// Cancel-safe: the refresh and its persistence run as their own task
    /// (tracked by [`crate::credential_writes`], which the binary drains
    /// before exit — once per refresh: through a refresher, the refresher's
    /// own task is the tracked one), so a caller dropped mid-flight (a
    /// pause, a timeout) only stops waiting. The task's handle stays on the client, so the next
    /// call adopts the refresh in flight (or its finished result) instead of
    /// starting a second one.
    async fn ensure_valid_token(&mut self) -> Result<(), ProviderError> {
        // Two rounds at most: an adopted refresh is checked like any other
        // token — one that finished long ago on an idle client can itself
        // be expired already, and is then refreshed once more.
        for _ in 0..2 {
            if self.pending_refresh.is_none() {
                // AI Studio uses a stable api-key — no refresh path.
                if self.variant.is_api_key() {
                    return Ok(());
                }
                if self.refresh_token.is_empty() || !is_token_expired(self.expires_ms) {
                    return Ok(());
                }
                self.pending_refresh = Some(match self.oauth_refresher.clone() {
                    Some(refresher) => {
                        let stale = AuthCredentials::OAuth {
                            access: self.access_token.clone(),
                            refresh: self.refresh_token.clone(),
                            expires: self.expires_ms,
                            extra: HashMap::new(),
                        };
                        // Not tracked here: the refresher's own
                        // refresh-and-persist task is the tracked write
                        // (one refresh, one count); this task only waits
                        // on it and reshapes the result.
                        tokio::spawn(async move {
                            match refresher.refresh(stale).await? {
                                AuthCredentials::OAuth {
                                    access,
                                    refresh,
                                    expires,
                                    ..
                                } => Ok((access, refresh, expires)),
                                AuthCredentials::ApiKey { .. } => {
                                    Err(ProviderError::TokenRefreshFailed(
                                        "the credential store returned an API key for an OAuth refresh"
                                            .into(),
                                    ))
                                }
                            }
                        })
                    }
                    None => crate::credential_writes::spawn(refresh_and_persist_google_token(
                        self.client.clone(),
                        self.token_url.clone(),
                        self.variant,
                        self.refresh_token.clone(),
                        self.project_id.clone(),
                        self.auth_json_path.clone(),
                    )),
                });
            }
            let Some(job) = self.pending_refresh.as_mut() else {
                return Ok(());
            };
            // A cancellation point: dropped here, the handle stays on `self`.
            let finished = job.await;
            self.pending_refresh = None;
            let (access_token, refresh_token, expires_ms) = finished.map_err(|e| {
                ProviderError::TokenRefreshFailed(format!("token refresh task failed: {e}"))
            })??;
            self.access_token = access_token;
            self.refresh_token = refresh_token;
            self.expires_ms = expires_ms;
        }
        Ok(())
    }
}

/// The process environment, as [`GoogleGeminiClient`] reads it.
fn read_env(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

/// Refresh a Google OAuth token and persist the refreshed credentials to
/// auth.json. Owns everything it needs so it can run as its own task (see
/// `GoogleGeminiClient::ensure_valid_token`). Returns
/// `(access_token, refresh_token, expires_ms)`.
async fn refresh_and_persist_google_token(
    client: ClientWithMiddleware,
    token_url: String,
    variant: GeminiVariant,
    refresh_token: String,
    project_id: String,
    auth_json_path: Option<PathBuf>,
) -> Result<(String, String, u64), ProviderError> {
    info!(variant = ?variant, "refreshing Google OAuth token");

    let response = client
        .post(&token_url)
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", variant.client_id()),
            ("client_secret", variant.client_secret()),
            ("refresh_token", &refresh_token),
        ])
        .send()
        .await
        .map_err(|e| ProviderError::TokenRefreshFailed(e.to_string()))?;

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        return Err(ProviderError::TokenRefreshFailed(format!(
            "HTTP {status}: {}",
            truncate(&body, 500)
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
    let refresh_token = body["refresh_token"]
        .as_str()
        .map(str::to_string)
        .unwrap_or(refresh_token);

    let expires_in_secs = body["expires_in"].as_u64().unwrap_or(3600);
    let expires_ms = now_ms() + (expires_in_secs * 1000);

    info!("Google token refreshed, expires in {expires_in_secs}s");

    // Persist refreshed credentials
    if let Some(ref path) = auth_json_path {
        let mut extra = HashMap::new();
        if !project_id.is_empty() {
            extra.insert(
                "project_id".to_string(),
                serde_json::Value::String(project_id.clone()),
            );
        }
        let creds = AuthCredentials::OAuth {
            access: access_token.clone(),
            refresh: refresh_token.clone(),
            expires: expires_ms,
            extra,
        };
        if let Err(e) = save_provider_auth(path, variant.provider_id(), &creds) {
            error!(path = %path.display(), error = %e, "the refreshed Google token could not be written; this process keeps using it, the next one will need to log in again");
        }
    }

    Ok((access_token, refresh_token, expires_ms))
}

#[async_trait::async_trait]
impl crate::provider::LlmProvider for GoogleGeminiClient {
    async fn send(&mut self, request: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        GoogleGeminiClient::send(self, request).await
    }

    async fn stream(
        &mut self,
        request: &LlmRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        GoogleGeminiClient::stream(self, request, on_event).await
    }

    fn default_model(&self) -> &str {
        "gemini-2.5-pro"
    }

    fn provider_id(&self) -> crate::provider_id::ProviderId {
        self.variant.provider_id()
    }

    async fn fetch_models(&mut self) -> Result<Vec<crate::model_pool::ModelInfo>, ProviderError> {
        // Code Assist (`v1internal`) has no listing method (spec §3).
        if self.variant != GeminiVariant::AiStudio {
            return Err(ProviderError::NotImplemented {
                provider: self.provider_id().to_string(),
            });
        }
        let base = self
            .api_base_override
            .clone()
            .unwrap_or_else(|| self.variant.endpoint().to_string());
        let provider = self.provider_id().to_string();
        let mut out = Vec::new();
        let mut page_token: Option<String> = None;
        // Every token already requested: a server that cycles (A -> B -> A)
        // would otherwise re-collect the same models until the page cap.
        let mut seen_tokens: std::collections::HashSet<String> = std::collections::HashSet::new();
        // Every model id already collected: a server that ignores `pageToken`
        // and re-sends a page must not leave its models in the result twice
        // (the first occurrence wins).
        let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        for i in 0..50 {
            let mut query: Vec<(&str, String)> = vec![("pageSize", "1000".to_string())];
            if let Some(t) = &page_token {
                query.push(("pageToken", t.clone()));
            }
            let resp = self
                .client
                .get(format!("{base}/v1beta/models"))
                .query(&query)
                .header("x-goog-api-key", &self.access_token)
                .header(reqwest::header::ACCEPT, "application/json")
                .send()
                .await
                .map_err(|e| ProviderError::Http(e.to_string()))?;
            let status = resp.status();
            if !status.is_success() {
                let message: String = resp
                    .text()
                    .await
                    .unwrap_or_default()
                    .chars()
                    .take(500)
                    .collect();
                return Err(ProviderError::Api {
                    status: status.as_u16(),
                    message,
                });
            }
            let body = resp
                .text()
                .await
                .map_err(|e| ProviderError::Http(e.to_string()))?;
            let v: serde_json::Value = crate::error::parse_listing_json(&provider, &body)?;
            let (models, next) = gemini_models_from_listing(&v, self.variant.provider_id());
            out.extend(models.into_iter().filter(|m| seen_ids.insert(m.id.clone())));
            match next {
                Some(t) => {
                    // The token just requested, or any earlier one (a cycle).
                    if !seen_tokens.insert(t.clone()) {
                        warn!(
                            provider = %provider,
                            page_token = %t,
                            collected = out.len(),
                            "model listing stopped: page token already seen; \
                             returning the models collected so far"
                        );
                        break;
                    }
                    // If this is the last iteration and we still have a next token, we hit the cap
                    if i == 49 {
                        warn!(
                            provider = %provider,
                            collected = out.len(),
                            "model listing stopped: page limit (50) reached while a next \
                             token is present; returning the models collected so far"
                        );
                    }
                    page_token = Some(t);
                }
                None => break,
            }
        }
        Ok(out)
    }

    fn output_shares_context(&self) -> bool {
        false
    }
}

// ── Model Listing ───────────────────────────────────────────────────

/// AI Studio `GET /v1beta/models` page → (`ModelInfo`s, next page token)
/// (spec 2026-09-30 §3). Keeps models that support `generateContent`.
pub(crate) fn gemini_models_from_listing(
    v: &serde_json::Value,
    provider: crate::provider_id::ProviderId,
) -> (Vec<crate::model_pool::ModelInfo>, Option<String>) {
    let models = v
        .get("models")
        .and_then(|m| m.as_array())
        .into_iter()
        .flatten()
        .filter(|e| {
            e.get("supportedGenerationMethods")
                .and_then(|m| m.as_array())
                .is_none_or(|ms| ms.iter().any(|x| x.as_str() == Some("generateContent")))
        })
        .filter_map(|e| {
            let name = e.get("name")?.as_str()?;
            let n = |k: &str| {
                e.get(k)
                    .and_then(|x| x.as_u64())
                    .map(|x| x.min(u32::MAX as u64) as u32)
                    .unwrap_or(0)
            };
            Some(crate::model_pool::ModelInfo {
                id: name.strip_prefix("models/").unwrap_or(name).to_string(),
                provider,
                context_window: n("inputTokenLimit"),
                max_output_tokens: n("outputTokenLimit"),
                capabilities: Vec::new(),
                cost: crate::model_pool::ModelCost::default(),
                status: crate::model_pool::ModelStatus::default(),
            })
        })
        .collect();
    let next = v
        .get("nextPageToken")
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty())
        .map(str::to_string);
    (models, next)
}

// ── Message Conversion ───────────────────────────────────────────────

/// Convert LlmRequest messages to Gemini contents format.
/// Builds a tool_use_id → name lookup from the conversation history so that
/// functionResponse parts include the correct function name (required by Gemini).
fn convert_messages(messages: &[Message]) -> Vec<serde_json::Value> {
    // Build tool_use_id → name lookup from all ToolUse blocks in the history
    let mut tool_name_map: HashMap<String, String> = HashMap::new();
    for msg in messages {
        for block in &msg.content {
            if let ContentBlock::ToolUse { id, name, .. } = block {
                tool_name_map.insert(id.clone(), name.clone());
            }
        }
    }

    let mut contents = Vec::new();

    for msg in messages {
        let role = match msg.role {
            Role::User => "user",
            Role::Assistant => "model",
        };

        // If this turn carries a verbatim parts replay from Gemini, send those
        // parts back exactly as they arrived. This is what returns
        // thoughtSignature on the exact part it was received on — Google
        // requires it, and omitting it on the first functionCall part is a hard
        // 400. Google's own SDKs replay the model's content the same way.
        //
        // The gate is the provider tag only: a foreign provider's block is an
        // alien wire format and is ignored (falling back to the rebuild).
        // Find a google_gemini-tagged Reasoning block first (without extracting
        // `parts` yet) so the empty-array and unusable-raw cases below can be
        // told apart for diagnostics — `find_map` collapsing straight to
        // `Option<Vec<Value>>` made them indistinguishable.
        let gemini_reasoning_raw = msg.content.iter().find_map(|b| match b {
            ContentBlock::Reasoning { provider, raw, .. } if provider == PROVIDER_TAG => Some(raw),
            _ => None,
        });
        if let Some(raw) = gemini_reasoning_raw {
            match raw.get("parts").and_then(|p| p.as_array()) {
                Some(parts) if !parts.is_empty() => {
                    contents.push(serde_json::json!({"role": role, "parts": parts}));
                    continue;
                }
                Some(_) => {
                    debug!("gemini replay block had empty parts; rebuilding from blocks");
                }
                None => {
                    debug!(
                        "gemini replay block tagged {PROVIDER_TAG:?} had unusable raw \
                         (missing or non-array \"parts\"); rebuilding from blocks — \
                         a first functionCall part will be sent without its \
                         thoughtSignature and Google will hard-400"
                    );
                }
            }
        }

        let mut parts = Vec::new();

        for block in &msg.content {
            match block {
                ContentBlock::Text { text } => {
                    if !text.is_empty() {
                        parts.push(serde_json::json!({"text": text}));
                    }
                }
                ContentBlock::ToolUse { name, input, .. } => {
                    parts.push(serde_json::json!({
                        "functionCall": {
                            "name": name,
                            "args": input,
                        }
                    }));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                    ..
                } => {
                    // Look up the function name from the preceding ToolUse
                    let name = tool_name_map.get(tool_use_id).cloned().unwrap_or_default();
                    let response_value = if *is_error {
                        serde_json::json!({"error": content})
                    } else {
                        serde_json::json!({"output": content})
                    };
                    parts.push(serde_json::json!({
                        "functionResponse": {
                            "name": name,
                            "response": response_value,
                        }
                    }));
                }
                // Either already consumed by the verbatim replay above, or a
                // foreign provider's block, which is deliberately ignored.
                ContentBlock::Reasoning { .. } => {}
                ContentBlock::Unknown => {}
            }
        }

        if !parts.is_empty() {
            contents.push(serde_json::json!({"role": role, "parts": parts}));
        }
    }

    contents
}

// ── Response Parsing ─────────────────────────────────────────────────

/// Parse a complete GenerateContentResponse into LlmResponse.
/// The GenerateContentResponse inside `json`. Code Assist wraps every answer
/// — a whole one, or each stream chunk — as `{"response": …, "traceId": …}`
/// (gemini-cli's `CaGenerateContentResponse`, read by its
/// `fromGenerateContentResponse`); AI Studio sends it bare.
fn generate_content_payload(json: &serde_json::Value) -> &serde_json::Value {
    json.get("response")
        .filter(|inner| inner.is_object())
        .unwrap_or(json)
}

fn parse_generate_content_response(
    json: &serde_json::Value,
    model: &str,
) -> Result<LlmResponse, ProviderError> {
    let json = generate_content_payload(json);
    let mut content = Vec::new();
    let mut stop_reason = Some(StopReason::EndTurn);
    let mut tool_call_counter: u32 = 0;
    // The turn's parts, stored verbatim so `convert_messages` can replay them.
    let mut raw_parts: Vec<serde_json::Value> = Vec::new();
    let mut thought_text = String::new();

    if let Some(candidates) = json.get("candidates").and_then(|c| c.as_array()) {
        if let Some(candidate) = candidates.first() {
            if let Some(parts) = candidate
                .get("content")
                .and_then(|c| c.get("parts"))
                .and_then(|p| p.as_array())
            {
                for part in parts {
                    // Every part is stored verbatim — it may carry a
                    // thoughtSignature that has to return on this exact part.
                    raw_parts.push(part.clone());

                    let is_thought = part.get("thought").and_then(|t| t.as_bool()) == Some(true);
                    if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                        if is_thought {
                            // A thought part's text is reasoning, not answer text:
                            // collect it for the transcript, emit no Text block.
                            if !thought_text.is_empty() {
                                thought_text.push_str("\n\n");
                            }
                            thought_text.push_str(text);
                        } else {
                            content.push(ContentBlock::Text {
                                text: text.to_string(),
                            });
                        }
                    }
                    // Checked even on a thought part: a part can carry both, and
                    // the old `continue` here silently dropped the tool call.
                    if let Some(fc) = part.get("functionCall") {
                        let name = fc["name"].as_str().unwrap_or("").to_string();
                        let args = fc.get("args").cloned().unwrap_or(serde_json::json!({}));
                        tool_call_counter += 1;
                        content.push(ContentBlock::ToolUse {
                            id: format!("gemini_tc_{tool_call_counter}"),
                            name,
                            input: args,
                        });
                        stop_reason = Some(StopReason::ToolUse);
                    }
                }
            }

            // Map finish reason
            if let Some(reason) = candidate.get("finishReason").and_then(|r| r.as_str()) {
                stop_reason = Some(map_finish_reason(reason));
            }
        }
    }

    // Store the turn's parts verbatim so convert_messages can replay them. This is
    // what carries thoughtSignature back on the exact part it arrived on — omitting
    // it on the first functionCall part is a hard 400.
    if !raw_parts.is_empty() {
        content.insert(
            0,
            ContentBlock::Reasoning {
                text: if thought_text.is_empty() {
                    None
                } else {
                    Some(thought_text)
                },
                provider: PROVIDER_TAG.to_string(),
                model: model.to_string(),
                raw: serde_json::json!({ "parts": raw_parts }),
            },
        );
    }

    let usage = if let Some(meta) = json.get("usageMetadata") {
        Usage {
            input_tokens: meta["promptTokenCount"].as_u64().unwrap_or(0) as u32,
            output_tokens: meta["candidatesTokenCount"].as_u64().unwrap_or(0) as u32,
            // Reported outside candidatesTokenCount by Google (I-48) — a
            // model that thinks (e.g. gemini-2.5-pro, which thinks by
            // default) would otherwise silently drop these tokens from both
            // the transcript and the bill.
            reasoning_tokens: meta["thoughtsTokenCount"].as_u64().unwrap_or(0) as u32,
            ..Default::default()
        }
    } else {
        Usage::default()
    };

    Ok(LlmResponse {
        id: String::new(), // Gemini doesn't return a response ID in the same way
        model: model.to_string(),
        content,
        stop: legacy_stop(stop_reason),
        usage,
    })
}

/// Wrap an accumulated stop reason into a [`Stop`] (a `None` is `Unreported`).
fn legacy_stop(reason: Option<StopReason>) -> Stop {
    match reason {
        Some(r) => Stop::synthetic(r, "google-gemini-cli"),
        None => Stop::from_wire(StopReason::Unreported, "google-gemini-cli", None),
    }
}

/// Map Google finish reason string to StopReason.
fn map_finish_reason(reason: &str) -> StopReason {
    match reason {
        "STOP" => StopReason::EndTurn,
        "MAX_TOKENS" => StopReason::MaxTokens,
        "STOP_SEQUENCE" => StopReason::StopSequence,
        "FUNCTION_CALLING" => StopReason::ToolUse,
        _ => StopReason::EndTurn, // SAFETY, OTHER, etc. → graceful
    }
}

// ── SSE Processing ───────────────────────────────────────────────────

/// Accumulator for Gemini streaming responses.
struct GeminiAccumulator {
    text: String,
    content_blocks: Vec<ContentBlock>,
    stop_reason: Option<StopReason>,
    input_tokens: u32,
    output_tokens: u32,
    reasoning_tokens: u32,
    tool_call_counter: u32,
    /// Every streamed part, verbatim and in arrival order, for replay.
    raw_parts: Vec<serde_json::Value>,
    thought_text: String,
    model: String,
}

impl GeminiAccumulator {
    fn new(model: &str) -> Self {
        Self {
            text: String::new(),
            content_blocks: Vec::new(),
            stop_reason: None,
            input_tokens: 0,
            output_tokens: 0,
            reasoning_tokens: 0,
            tool_call_counter: 0,
            raw_parts: Vec::new(),
            thought_text: String::new(),
            model: model.to_string(),
        }
    }

    fn into_response(self) -> Option<LlmResponse> {
        if self.text.is_empty() && self.content_blocks.is_empty() && self.raw_parts.is_empty() {
            return None;
        }
        let mut content = Vec::new();
        if !self.text.is_empty() {
            content.push(ContentBlock::Text { text: self.text });
        }
        content.extend(self.content_blocks);

        // Same verbatim-parts replay block as the non-streaming parse.
        if !self.raw_parts.is_empty() {
            content.insert(
                0,
                ContentBlock::Reasoning {
                    text: if self.thought_text.is_empty() {
                        None
                    } else {
                        Some(self.thought_text)
                    },
                    provider: PROVIDER_TAG.to_string(),
                    model: self.model.clone(),
                    raw: serde_json::json!({ "parts": self.raw_parts }),
                },
            );
        }

        Some(LlmResponse {
            id: String::new(),
            model: self.model,
            content,
            stop: legacy_stop(self.stop_reason),
            usage: Usage {
                input_tokens: self.input_tokens,
                output_tokens: self.output_tokens,
                reasoning_tokens: self.reasoning_tokens,
                ..Default::default()
            },
        })
    }
}

/// Process a Gemini SSE event.
fn process_gemini_sse(
    event: &crate::sse::SseEvent,
    acc: &mut GeminiAccumulator,
    on_event: &mut (impl FnMut(StreamEvent) + ?Sized),
) -> Result<(), ProviderError> {
    if event.data == "[DONE]" {
        return Ok(());
    }

    let data: serde_json::Value = serde_json::from_str(&event.data)?;
    let data = generate_content_payload(&data);

    // Process candidates
    if let Some(candidates) = data.get("candidates").and_then(|c| c.as_array()) {
        if let Some(candidate) = candidates.first() {
            if let Some(parts) = candidate
                .get("content")
                .and_then(|c| c.get("parts"))
                .and_then(|p| p.as_array())
            {
                for part in parts {
                    // Every part is stored verbatim — it may carry a
                    // thoughtSignature that has to return on this exact part.
                    acc.raw_parts.push(part.clone());

                    let is_thought = part.get("thought").and_then(|t| t.as_bool()) == Some(true);
                    if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                        if is_thought {
                            // Deliberately no "\n\n" separator here, unlike the
                            // non-streaming join in parse_generate_content_response:
                            // SSE commonly delivers a single thought summary
                            // fragmented across chunks, and inserting a separator
                            // per chunk would corrupt it into disjoint paragraphs.
                            // Display-only (reasoning_text()) — raw["parts"] and
                            // the replay path are unaffected either way. Do not
                            // "fix" this to match the non-streaming path.
                            acc.thought_text.push_str(text);
                            on_event(StreamEvent::ReasoningDelta(text.to_string()));
                        } else {
                            acc.text.push_str(text);
                            on_event(StreamEvent::TextDelta(text.to_string()));
                        }
                    }

                    // Checked even on a thought part: a part can carry both, and
                    // the old `continue` here silently dropped the tool call.
                    if let Some(fc) = part.get("functionCall") {
                        let name = fc["name"].as_str().unwrap_or("").to_string();
                        let args = fc.get("args").cloned().unwrap_or(serde_json::json!({}));
                        acc.tool_call_counter += 1;
                        let id = format!("gemini_tc_{}", acc.tool_call_counter);
                        on_event(StreamEvent::ToolUseStart {
                            id: id.clone(),
                            name: name.clone(),
                        });
                        let args_str = args.to_string();
                        on_event(StreamEvent::InputJsonDelta(args_str));
                        acc.content_blocks.push(ContentBlock::ToolUse {
                            id,
                            name,
                            input: args,
                        });
                    }
                }
            }

            if let Some(reason) = candidate.get("finishReason").and_then(|r| r.as_str()) {
                acc.stop_reason = Some(map_finish_reason(reason));
            }
        }
    }

    // Process usage metadata
    if let Some(meta) = data.get("usageMetadata") {
        if let Some(input) = meta.get("promptTokenCount").and_then(|v| v.as_u64()) {
            acc.input_tokens = input as u32;
        }
        if let Some(output) = meta.get("candidatesTokenCount").and_then(|v| v.as_u64()) {
            acc.output_tokens = output as u32;
        }
        // See the non-streaming parse site and the contrast note on
        // `Usage::reasoning_tokens` (I-48): reported outside
        // candidatesTokenCount by Google.
        if let Some(reasoning) = meta.get("thoughtsTokenCount").and_then(|v| v.as_u64()) {
            acc.reasoning_tokens = reasoning as u32;
        }
        on_event(StreamEvent::UsageSnapshot(Usage {
            input_tokens: acc.input_tokens,
            output_tokens: acc.output_tokens,
            cached_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: acc.reasoning_tokens,
        }));
    }

    Ok(())
}

// ── Helpers ──────────────────────────────────────────────────────────

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn request_counter() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn truncate(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        text.to_string()
    } else {
        let end = (0..=max_len)
            .rev()
            .find(|&i| text.is_char_boundary(i))
            .unwrap_or(0);
        format!("{}...", &text[..end])
    }
}

/// Extract a clean error message from a Google API JSON error response.
fn extract_google_error(text: &str) -> String {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(text) {
        if let Some(msg) = json
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
        {
            return msg.to_string();
        }
    }
    truncate(text, 500)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An adopted refresh is checked like any other token: one that
    /// finished long ago on an idle client (or was issued short-lived) may
    /// already be expired, and must be refreshed again rather than sent.
    #[tokio::test]
    async fn an_adopted_refresh_that_is_already_expired_is_refreshed_again() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let first = server.mock(|when, then| {
            when.method(POST).path("/token").body_contains("refresh-1");
            then.status(200)
                .delay(std::time::Duration::from_millis(300))
                // Already inside the 5-minute expiry buffer when it lands.
                .json_body(serde_json::json!({
                    "access_token": "access-2",
                    "refresh_token": "refresh-2",
                    "expires_in": 1
                }));
        });
        let second = server.mock(|when, then| {
            when.method(POST).path("/token").body_contains("refresh-2");
            then.status(200).json_body(serde_json::json!({
                "access_token": "access-3",
                "refresh_token": "refresh-3",
                "expires_in": 3600
            }));
        });
        let mut creds = test_creds("proj");
        if let AuthCredentials::OAuth {
            refresh, expires, ..
        } = &mut creds
        {
            *refresh = "refresh-1".into();
            *expires = 1;
        }
        let mut client = GoogleGeminiClient::new(
            creds,
            GeminiVariant::GeminiCli,
            None,
            Arc::new(rupu_netflow::NullSink),
        )
        .unwrap();
        client.token_url = server.url("/token");
        let dropped = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            client.ensure_valid_token(),
        )
        .await;
        assert!(dropped.is_err(), "dropped mid-refresh");
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        client.ensure_valid_token().await.unwrap();
        first.assert_hits(1);
        second.assert_hits(1);
        assert_eq!(client.access_token, "access-3");
    }

    /// An OAuthRefresher that records the stale refresh token it was handed
    /// and returns a fixed fresh credential.
    struct FakeRefresher {
        seen: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl crate::credential_writes::OAuthRefresher for FakeRefresher {
        async fn refresh(&self, stale: AuthCredentials) -> Result<AuthCredentials, ProviderError> {
            if let AuthCredentials::OAuth { refresh, .. } = &stale {
                self.seen.lock().unwrap().push(refresh.clone());
            }
            Ok(AuthCredentials::OAuth {
                access: "access-9".into(),
                refresh: "refresh-9".into(),
                expires: u64::MAX / 2,
                extra: Default::default(),
            })
        }
    }

    /// With a refresher (the store that owns the credential), the client
    /// hands its refresh over — so the rotation is persisted under the
    /// store's lock — and never calls the token endpoint itself.
    #[tokio::test]
    async fn a_client_with_a_refresher_hands_it_the_refresh() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let own = server.mock(|when, then| {
            when.method(POST);
            then.status(200)
                .json_body(serde_json::json!({ "access_token": "own" }));
        });
        let refresher = std::sync::Arc::new(FakeRefresher {
            seen: std::sync::Mutex::new(Vec::new()),
        });
        let mut creds = test_creds("proj");
        if let AuthCredentials::OAuth {
            refresh, expires, ..
        } = &mut creds
        {
            *refresh = "refresh-1".into();
            *expires = 1;
        }
        let mut client = GoogleGeminiClient::new(
            creds,
            GeminiVariant::GeminiCli,
            None,
            Arc::new(rupu_netflow::NullSink),
        )
        .unwrap()
        .with_oauth_refresher(Some(refresher.clone()));
        client.token_url = server.url("/token");
        client.ensure_valid_token().await.unwrap();
        own.assert_hits(0);
        assert_eq!(
            *refresher.seen.lock().unwrap(),
            vec!["refresh-1".to_string()]
        );
        assert_eq!(client.access_token, "access-9");
        assert_eq!(client.refresh_token, "refresh-9");
    }

    /// A client left behind by a dropped call adopts the refresh still in
    /// flight on its next call instead of starting a second one with a
    /// possibly rotated-out refresh token.
    #[tokio::test]
    async fn a_reused_client_adopts_the_refresh_in_flight() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let token = server.mock(|when, then| {
            when.method(POST).path("/token");
            then.status(200)
                .delay(std::time::Duration::from_millis(300))
                .json_body(serde_json::json!({
                    "access_token": "access-2",
                    "refresh_token": "refresh-2",
                    "expires_in": 3600
                }));
        });
        let mut creds = test_creds("proj");
        if let AuthCredentials::OAuth {
            refresh, expires, ..
        } = &mut creds
        {
            *refresh = "refresh-1".into();
            *expires = 1; // long expired
        }
        let mut client = GoogleGeminiClient::new(
            creds,
            GeminiVariant::GeminiCli,
            None,
            Arc::new(rupu_netflow::NullSink),
        )
        .unwrap();
        client.token_url = server.url("/token");
        let request = LlmRequest {
            model: "gemini-2.5-pro".into(),
            system: None,
            messages: vec![Message::user("Hello")],
            max_tokens: Some(16),
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
        };
        let dropped =
            tokio::time::timeout(std::time::Duration::from_millis(50), client.send(&request)).await;
        assert!(dropped.is_err(), "dropped mid-refresh");
        // The next call's token step (what `send` runs first) — calling it
        // directly keeps the test off the real Code Assist endpoint.
        client.ensure_valid_token().await.unwrap();
        token.assert_hits(1);
        assert_eq!(client.access_token, "access-2");
        assert_eq!(client.refresh_token, "refresh-2");
    }

    /// A token refresh started by a call that is then dropped (a pause, a
    /// timeout) must still persist the refreshed credentials. The refresh
    /// runs as its own task, so it finishes and persists after the caller is
    /// gone.
    #[tokio::test]
    async fn an_abandoned_token_refresh_still_persists_the_rotated_token() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let token = server.mock(|when, then| {
            when.method(POST).path("/token");
            then.status(200)
                .delay(std::time::Duration::from_millis(300))
                .json_body(serde_json::json!({
                    "access_token": "access-2",
                    "refresh_token": "refresh-2",
                    "expires_in": 3600
                }));
        });
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("auth.json");
        let mut creds = test_creds("proj");
        if let AuthCredentials::OAuth {
            refresh, expires, ..
        } = &mut creds
        {
            *refresh = "refresh-1".into();
            *expires = 1; // long expired (0 means "no expiry info")
        }
        let mut client = GoogleGeminiClient::new(
            creds,
            GeminiVariant::GeminiCli,
            Some(path.clone()),
            Arc::new(rupu_netflow::NullSink),
        )
        .unwrap();
        client.token_url = server.url("/token");
        let request = LlmRequest {
            model: "gemini-2.5-pro".into(),
            system: None,
            messages: vec![Message::user("Hello")],
            max_tokens: Some(16),
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
        };
        let dropped =
            tokio::time::timeout(std::time::Duration::from_millis(50), client.send(&request)).await;
        assert!(
            dropped.is_err(),
            "the caller gave up mid-refresh: {dropped:?}"
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !std::fs::read_to_string(&path).is_ok_and(|s| s.contains("refresh-2"))
            && std::time::Instant::now() < deadline
        {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        token.assert_hits(1);
        let saved = std::fs::read_to_string(&path).expect("the refresh persisted");
        assert!(
            saved.contains("refresh-2") && saved.contains("access-2"),
            "{saved}"
        );
    }

    fn test_creds(project_id: &str) -> AuthCredentials {
        let mut extra = HashMap::new();
        extra.insert(
            "project_id".to_string(),
            serde_json::Value::String(project_id.to_string()),
        );
        AuthCredentials::OAuth {
            access: "test-token".into(),
            refresh: "test-refresh".into(),
            expires: 9999999999999,
            extra,
        }
    }

    #[test]
    fn test_new_gemini_cli() {
        let client = GoogleGeminiClient::new(
            test_creds("my-project"),
            GeminiVariant::GeminiCli,
            None,
            Arc::new(rupu_netflow::NullSink),
        )
        .unwrap();
        assert_eq!(client.project_id, "my-project");
        assert_eq!(client.variant, GeminiVariant::GeminiCli);
    }

    #[test]
    fn test_new_antigravity() {
        let client = GoogleGeminiClient::new(
            test_creds("ag-project"),
            GeminiVariant::Antigravity,
            None,
            Arc::new(rupu_netflow::NullSink),
        )
        .unwrap();
        assert_eq!(client.project_id, "ag-project");
        assert_eq!(client.variant, GeminiVariant::Antigravity);
    }

    #[test]
    fn test_api_key_rejected() {
        let creds = AuthCredentials::ApiKey {
            key: "sk-test".into(),
        };
        let result = GoogleGeminiClient::new(
            creds,
            GeminiVariant::GeminiCli,
            None,
            Arc::new(rupu_netflow::NullSink),
        );
        match result {
            Err(e) => assert!(
                e.to_string().contains("OAuth"),
                "expected OAuth error, got: {e}"
            ),
            Ok(_) => panic!("expected error for API key auth"),
        }
    }

    #[test]
    fn test_build_request_body_basic() {
        let client = GoogleGeminiClient::new(
            test_creds("proj"),
            GeminiVariant::GeminiCli,
            None,
            Arc::new(rupu_netflow::NullSink),
        )
        .unwrap();

        let request = LlmRequest {
            model: "gemini-2.5-pro".into(),
            system: Some("Be helpful.".into()),
            messages: vec![Message::user("Hello")],
            max_tokens: Some(4096),
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
        };

        let body = client.build_request_body(&request);
        assert_eq!(body["project"], "proj");
        assert_eq!(body["model"], "gemini-2.5-pro");
        assert_eq!(body["userAgent"], "phi-coding-agent");

        let inner = &body["request"];
        assert!(inner["contents"].as_array().unwrap().len() == 1);
        assert_eq!(
            inner["systemInstruction"]["parts"][0]["text"],
            "Be helpful."
        );
        assert_eq!(inner["generationConfig"]["maxOutputTokens"], 4096);
    }

    #[test]
    fn unset_max_tokens_omits_max_output_tokens() {
        let client = GoogleGeminiClient::new(
            test_creds("proj"),
            GeminiVariant::GeminiCli,
            None,
            Arc::new(rupu_netflow::NullSink),
        )
        .unwrap();

        let mut request = LlmRequest {
            model: "gemini-2.5-pro".into(),
            messages: vec![Message::user("Hello")],
            max_tokens: None,
            ..Default::default()
        };
        let body = client.build_request_body(&request);
        // `Value::get` on a missing/null `generationConfig` would also be
        // `None`; prove the object is really there and only the cap is absent.
        assert!(
            body["request"]["generationConfig"].is_object(),
            "generationConfig must still be sent: {}",
            body["request"]
        );
        assert!(body["request"]["generationConfig"]
            .get("maxOutputTokens")
            .is_none());

        request.max_tokens = Some(2048);
        let body = client.build_request_body(&request);
        assert_eq!(body["request"]["generationConfig"]["maxOutputTokens"], 2048);
    }

    #[test]
    fn test_build_request_body_antigravity_user_agent() {
        let client = GoogleGeminiClient::new(
            test_creds("proj"),
            GeminiVariant::Antigravity,
            None,
            Arc::new(rupu_netflow::NullSink),
        )
        .unwrap();

        let request = LlmRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("Hi")],
            max_tokens: Some(1024),
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
        };

        let body = client.build_request_body(&request);
        assert_eq!(body["userAgent"], "antigravity");
    }

    #[test]
    fn test_build_request_body_with_tools() {
        let client = GoogleGeminiClient::new(
            test_creds("proj"),
            GeminiVariant::GeminiCli,
            None,
            Arc::new(rupu_netflow::NullSink),
        )
        .unwrap();

        let request = LlmRequest {
            model: "gemini-2.5-pro".into(),
            system: None,
            messages: vec![Message::user("read file")],
            max_tokens: Some(4096),
            tools: vec![ToolDefinition {
                name: "read_file".into(),
                description: "Read a file".into(),
                input_schema: serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            }],
            cell_id: None,
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
        };

        let body = client.build_request_body(&request);
        let tools = body["request"]["tools"][0]["functionDeclarations"]
            .as_array()
            .unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "read_file");
    }

    #[test]
    fn test_build_request_body_with_thinking() {
        let client = GoogleGeminiClient::new(
            test_creds("proj"),
            GeminiVariant::GeminiCli,
            None,
            Arc::new(rupu_netflow::NullSink),
        )
        .unwrap();

        let request = LlmRequest {
            model: "gemini-2.5-pro".into(),
            system: None,
            messages: vec![Message::user("think hard")],
            max_tokens: Some(16000),
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: Some(crate::model_tier::ThinkingLevel::Medium),
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
        };

        let body = client.build_request_body(&request);
        let config = &body["request"]["generationConfig"]["thinkingConfig"];
        assert_eq!(config["includeThoughts"], true);
        assert_eq!(config["thinkingBudget"], 8192);
        // gemini-2.5-pro is not a Gemini-3 model: `thinkingLevel` is a
        // Gemini-3-only key and must not be sent alongside `thinkingBudget`.
        assert!(
            config.get("thinkingLevel").is_none(),
            "gemini-2.5-pro must not receive the Gemini-3-only thinkingLevel key"
        );
    }

    #[test]
    fn test_build_request_body_thinking_max_clamped() {
        let client = GoogleGeminiClient::new(
            test_creds("proj"),
            GeminiVariant::GeminiCli,
            None,
            Arc::new(rupu_netflow::NullSink),
        )
        .unwrap();

        let request = LlmRequest {
            model: "gemini-2.5-pro".into(),
            system: None,
            messages: vec![Message::user("max")],
            max_tokens: Some(32000),
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: Some(crate::model_tier::ThinkingLevel::Max),
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
        };

        let body = client.build_request_body(&request);
        let config = &body["request"]["generationConfig"]["thinkingConfig"];
        assert_eq!(config["thinkingBudget"], 32768); // clamped to High
        assert!(
            config.get("thinkingLevel").is_none(),
            "gemini-2.5-pro must not receive the Gemini-3-only thinkingLevel key"
        );
    }

    #[test]
    fn test_build_request_body_thinking_gemini_3_uses_level_not_budget() {
        let client = GoogleGeminiClient::new(
            test_creds("proj"),
            GeminiVariant::GeminiCli,
            None,
            Arc::new(rupu_netflow::NullSink),
        )
        .unwrap();

        let request = LlmRequest {
            model: "gemini-3-pro-preview".into(),
            system: None,
            messages: vec![Message::user("think hard")],
            max_tokens: Some(16000),
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: Some(crate::model_tier::ThinkingLevel::Medium),
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
        };

        let body = client.build_request_body(&request);
        let config = &body["request"]["generationConfig"]["thinkingConfig"];
        assert_eq!(config["includeThoughts"], true);
        // Lowercase per Google's documented contract (I-46).
        assert_eq!(config["thinkingLevel"], "medium");
        assert!(
            config.get("thinkingBudget").is_none(),
            "gemini-3-pro-preview must not receive the Gemini-2.5-only thinkingBudget key"
        );
    }

    #[test]
    fn test_thinking_config_never_sends_both_keys() {
        // Mutual exclusion is the invariant worth pinning: whichever model
        // family, thinkingLevel and thinkingBudget must never co-occur.
        let client = GoogleGeminiClient::new(
            test_creds("proj"),
            GeminiVariant::GeminiCli,
            None,
            Arc::new(rupu_netflow::NullSink),
        )
        .unwrap();

        for model in ["gemini-2.5-pro", "gemini-3-pro-preview"] {
            for level in [
                crate::model_tier::ThinkingLevel::Minimal,
                crate::model_tier::ThinkingLevel::Low,
                crate::model_tier::ThinkingLevel::Medium,
                crate::model_tier::ThinkingLevel::High,
                crate::model_tier::ThinkingLevel::Max,
            ] {
                let request = LlmRequest {
                    model: model.into(),
                    system: None,
                    messages: vec![Message::user("test")],
                    max_tokens: Some(4096),
                    tools: vec![],
                    cell_id: None,
                    trace_id: None,
                    thinking: Some(level),
                    context_window: None,
                    task_type: None,
                    output_format: None,
                    output_schema: None,
                    anthropic_task_budget: None,
                    anthropic_context_management: None,
                    anthropic_speed: None,
                    disable_prompt_cache: false,
                };
                let body = client.build_request_body(&request);
                let config = &body["request"]["generationConfig"]["thinkingConfig"];
                let has_level = config.get("thinkingLevel").is_some();
                let has_budget = config.get("thinkingBudget").is_some();
                assert!(
                    has_level ^ has_budget,
                    "model={model} level={level:?}: exactly one of thinkingLevel/thinkingBudget must be present (level={has_level}, budget={has_budget})"
                );
            }
        }
    }

    #[test]
    fn test_convert_messages_user_assistant() {
        let messages = vec![
            Message::user("Hello"),
            Message::assistant("Hi there!"),
            Message::user("How are you?"),
        ];
        let contents = convert_messages(&messages);
        assert_eq!(contents.len(), 3);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[0]["parts"][0]["text"], "Hello");
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(contents[2]["role"], "user");
    }

    #[test]
    fn test_convert_messages_tool_result_with_name_lookup() {
        // Simulate a multi-turn: assistant calls a tool, user provides result
        let messages = vec![
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "tc_1".into(),
                    name: "read_file".into(),
                    input: serde_json::json!({"path": "/tmp"}),
                }],
            },
            Message::tool_result("tc_1", "file contents", false),
        ];
        let contents = convert_messages(&messages);
        assert_eq!(contents.len(), 2);
        // Tool result should have the function name resolved from the ToolUse
        let func_resp = &contents[1]["parts"][0]["functionResponse"];
        assert_eq!(func_resp["name"], "read_file");
        assert_eq!(func_resp["response"]["output"], "file contents");
    }

    /// An assistant turn as Gemini's parse produces it: a replay block carrying
    /// the verbatim parts, plus the ToolUse block rupu uses for dispatch.
    fn replay_turn(parts: serde_json::Value) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Reasoning {
                    text: None,
                    provider: PROVIDER_TAG.to_string(),
                    model: "gemini-3-pro-preview".to_string(),
                    raw: serde_json::json!({ "parts": parts }),
                },
                ContentBlock::ToolUse {
                    id: "gemini_tc_1".into(),
                    name: "f".into(),
                    input: serde_json::json!({"x": 1}),
                },
            ],
        }
    }

    #[test]
    fn convert_messages_replays_stored_parts_verbatim() {
        let stored = serde_json::json!([
            {"functionCall": {"name": "f", "args": {"x": 1}}, "thoughtSignature": "sig_abc"}
        ]);
        let contents = convert_messages(&[replay_turn(stored.clone())]);

        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["role"], "model");
        // The stored parts go back on the wire byte-for-byte.
        assert_eq!(contents[0]["parts"], stored);
        // The functionCall appears exactly once — the ToolUse block must not be
        // rebuilt into a second part on a turn that replays.
        let parts = contents[0]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(
            parts
                .iter()
                .filter(|p| p.get("functionCall").is_some())
                .count(),
            1
        );
    }

    #[test]
    fn convert_messages_replay_preserves_thought_signature_on_its_part() {
        // Two parts, only the second signed: the signature must stay attached to
        // the part it arrived on, not hoisted, moved, or reordered.
        let stored = serde_json::json!([
            {"thought": true, "text": "Let me think..."},
            {"functionCall": {"name": "f", "args": {"x": 1}}, "thoughtSignature": "sig_abc"}
        ]);
        let contents = convert_messages(&[replay_turn(stored)]);

        let parts = contents[0]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["thought"], true);
        assert!(parts[0].get("thoughtSignature").is_none());
        assert_eq!(parts[1]["thoughtSignature"], "sig_abc");
        assert_eq!(parts[1]["functionCall"]["name"], "f");
    }

    #[test]
    fn convert_messages_falls_back_to_rebuild_without_reasoning_block() {
        // Backward compat: no Reasoning block -> converts exactly as before.
        let messages = vec![Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: "calling".into(),
                },
                ContentBlock::ToolUse {
                    id: "gemini_tc_1".into(),
                    name: "f".into(),
                    input: serde_json::json!({"x": 1}),
                },
            ],
        }];
        let contents = convert_messages(&messages);

        assert_eq!(contents.len(), 1);
        let parts = contents[0]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["text"], "calling");
        assert_eq!(parts[1]["functionCall"]["name"], "f");
        assert_eq!(parts[1]["functionCall"]["args"]["x"], 1);
    }

    #[test]
    fn convert_messages_drops_foreign_provider_reasoning_block() {
        // An Anthropic thinking block is an alien wire format: never replay it,
        // and never let its signature reach Gemini.
        let messages = vec![Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Reasoning {
                    text: Some("thinking...".into()),
                    provider: "anthropic".to_string(),
                    model: "claude-opus-4-6".to_string(),
                    raw: serde_json::json!({
                        "type": "thinking",
                        "thinking": "thinking...",
                        "signature": "anthropic_sig",
                    }),
                },
                ContentBlock::ToolUse {
                    id: "toolu_1".into(),
                    name: "f".into(),
                    input: serde_json::json!({"x": 1}),
                },
            ],
        }];
        let contents = convert_messages(&messages);

        assert_eq!(contents.len(), 1);
        let parts = contents[0]["parts"].as_array().unwrap();
        // Rebuild path: only the ToolUse becomes a part.
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["functionCall"]["name"], "f");
        let wire = serde_json::to_string(&contents).unwrap();
        assert!(!wire.contains("anthropic_sig"));
        assert!(!wire.contains("thinking"));
    }

    #[test]
    fn convert_messages_ignores_replay_block_with_malformed_raw() {
        // raw missing "parts", "parts" not an array, and an empty parts array all
        // fall back to the rebuild rather than sending garbage.
        for raw in [
            serde_json::json!({}),
            serde_json::json!({"parts": "not-an-array"}),
            serde_json::json!({"parts": []}),
        ] {
            let messages = vec![Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::Reasoning {
                        text: None,
                        provider: PROVIDER_TAG.to_string(),
                        model: "gemini-3-pro-preview".to_string(),
                        raw: raw.clone(),
                    },
                    ContentBlock::ToolUse {
                        id: "gemini_tc_1".into(),
                        name: "f".into(),
                        input: serde_json::json!({"x": 1}),
                    },
                ],
            }];
            let contents = convert_messages(&messages);

            assert_eq!(contents.len(), 1, "raw: {raw}");
            let parts = contents[0]["parts"].as_array().unwrap();
            assert_eq!(parts.len(), 1, "raw: {raw}");
            assert_eq!(parts[0]["functionCall"]["name"], "f", "raw: {raw}");
        }
    }

    #[test]
    fn convert_messages_still_maps_tool_result_names_with_replay_present() {
        // The tool_name_map pre-scan reads ToolUse blocks; a replaying assistant
        // turn must not break the following user turn's functionResponse naming.
        let stored = serde_json::json!([
            {"functionCall": {"name": "f", "args": {"x": 1}}, "thoughtSignature": "sig_abc"}
        ]);
        let messages = vec![
            replay_turn(stored),
            Message::tool_result("gemini_tc_1", "file contents", false),
        ];
        let contents = convert_messages(&messages);

        assert_eq!(contents.len(), 2);
        assert_eq!(contents[0]["parts"][0]["thoughtSignature"], "sig_abc");
        let func_resp = &contents[1]["parts"][0]["functionResponse"];
        assert_eq!(func_resp["name"], "f");
        assert_eq!(func_resp["response"]["output"], "file contents");
    }

    /// Guards the seam between `parse_generate_content_response` (which writes
    /// `raw["parts"]`) and `convert_messages` (which reads `raw["parts"]`).
    /// Every other test in this file drives only one half — Task 1's tests
    /// assert `raw["parts"]` is written; Task 2's tests (e.g. `replay_turn`
    /// above) hand-build the assistant `Message` and assert `raw["parts"]` is
    /// read. Neither proves the two halves actually compose. If the internal
    /// `"parts"` key inside `raw` were renamed in
    /// `parse_generate_content_response` and only Task 1's tests were updated
    /// to match, every other gemini test would stay green while every real
    /// Gemini turn silently reverted to the rebuild path — reintroducing the
    /// hard 400 in production, undetected. This test drives the real
    /// functions back-to-back with no hand-built intermediate, so that rename
    /// fails here.
    #[test]
    fn parse_then_convert_round_trips_thought_signature() {
        let parts = serde_json::json!([
            {"thought": true, "text": "I should call f."},
            {"functionCall": {"name": "f", "args": {"x": 1}}, "thoughtSignature": "sig_abc"}
        ]);
        let json = serde_json::json!({
            "candidates": [{
                "content": {"role": "model", "parts": parts},
                "finishReason": "FUNCTION_CALLING"
            }]
        });

        let resp = parse_generate_content_response(&json, "gemini-3-pro-preview").unwrap();

        // Feed resp.content straight into a fresh Message — do NOT hand-build
        // the assistant turn the way `replay_turn` does elsewhere in this file.
        let message = Message {
            role: Role::Assistant,
            content: resp.content,
        };

        let contents = convert_messages(&[message]);

        assert_eq!(contents.len(), 1);
        let sent_parts = contents[0]["parts"].as_array().unwrap();
        // The thoughtSignature reached the replayed functionCall part.
        assert_eq!(sent_parts[1]["thoughtSignature"], "sig_abc");
        assert_eq!(sent_parts[1]["functionCall"]["name"], "f");
        // Exactly one functionCall — the rebuild path must not have also
        // turned the parsed ToolUse block into a second, duplicate part.
        assert_eq!(
            sent_parts
                .iter()
                .filter(|p| p.get("functionCall").is_some())
                .count(),
            1
        );
    }

    #[test]
    fn test_parse_response_text() {
        let json = serde_json::json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{"text": "The answer is 42."}]
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 15,
                "candidatesTokenCount": 8,
                "totalTokenCount": 23
            }
        });

        let response = parse_generate_content_response(&json, "gemini-2.5-pro").unwrap();
        assert_eq!(response.text(), Some("The answer is 42."));
        assert_eq!(response.stop.reason, StopReason::EndTurn);
        assert_eq!(response.usage.input_tokens, 15);
        assert_eq!(response.usage.output_tokens, 8);
    }

    /// I-48: gemini-2.5-pro thinks by default, and Google reports those
    /// tokens *outside* candidatesTokenCount. A realistic usageMetadata
    /// including thoughtsTokenCount must be reflected in the parsed
    /// `Usage`, without inflating output_tokens (candidatesTokenCount
    /// stays exactly what Google's console would show for "output").
    #[test]
    fn test_parse_response_counts_thinking_tokens() {
        let json = serde_json::json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{"text": "The answer is 42."}]
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 15,
                "candidatesTokenCount": 8,
                "thoughtsTokenCount": 200,
                "totalTokenCount": 223
            }
        });

        let response = parse_generate_content_response(&json, "gemini-2.5-pro").unwrap();
        assert_eq!(response.usage.input_tokens, 15);
        assert_eq!(response.usage.output_tokens, 8);
        assert_eq!(response.usage.reasoning_tokens, 200);
    }

    #[test]
    fn test_parse_response_function_call() {
        let json = serde_json::json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{
                        "functionCall": {
                            "name": "read_file",
                            "args": {"path": "/tmp/test.txt"}
                        }
                    }]
                },
                "finishReason": "FUNCTION_CALLING"
            }],
            "usageMetadata": {"promptTokenCount": 10, "candidatesTokenCount": 5}
        });

        let response = parse_generate_content_response(&json, "gemini-2.5-pro").unwrap();
        assert_eq!(response.stop.reason, StopReason::ToolUse);
        let tools = response.tool_calls();
        assert_eq!(tools.len(), 1);
        match &tools[0] {
            ContentBlock::ToolUse { name, input, .. } => {
                assert_eq!(name, "read_file");
                assert_eq!(input["path"], "/tmp/test.txt");
            }
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn test_parse_response_max_tokens() {
        let json = serde_json::json!({
            "candidates": [{
                "content": {"role": "model", "parts": [{"text": "partial"}]},
                "finishReason": "MAX_TOKENS"
            }]
        });

        let response = parse_generate_content_response(&json, "gemini-2.5-pro").unwrap();
        assert_eq!(response.stop.reason, StopReason::MaxTokens);
    }

    /// Pull the single `Reasoning` block out of a response, or panic.
    fn only_reasoning(response: &LlmResponse) -> (&Option<String>, &str, &serde_json::Value) {
        let mut found = response.content.iter().filter_map(|b| match b {
            ContentBlock::Reasoning {
                text,
                provider,
                raw,
                ..
            } => Some((text, provider.as_str(), raw)),
            _ => None,
        });
        let block = found.next().expect("expected a Reasoning block");
        assert!(
            found.next().is_none(),
            "expected exactly one Reasoning block"
        );
        block
    }

    // Replaces the former `test_parse_response_skips_thinking`, which asserted the
    // old drop behavior (`content.len() == 1`). Capturing thoughts is a deliberate
    // behavior change.
    #[test]
    fn parse_response_captures_thinking_as_reasoning_block() {
        let parts = serde_json::json!([
            {"thought": true, "text": "Let me think..."},
            {"text": "The answer is 42."}
        ]);
        let json = serde_json::json!({
            "candidates": [{
                "content": {"role": "model", "parts": parts},
                "finishReason": "STOP"
            }]
        });

        let response = parse_generate_content_response(&json, "gemini-3-pro").unwrap();

        // Text blocks are unaffected: the thought part never becomes answer text.
        assert_eq!(response.text(), Some("The answer is 42."));

        let (text, provider, raw) = only_reasoning(&response);
        assert_eq!(text.as_deref(), Some("Let me think..."));
        assert_eq!(provider, PROVIDER_TAG);
        // The whole parts array is stored verbatim — including the thought part.
        assert_eq!(raw["parts"], parts);
    }

    #[test]
    fn parse_response_preserves_thought_signature_in_raw() {
        let parts = serde_json::json!([
            {"functionCall": {"name": "f", "args": {}}, "thoughtSignature": "sig_abc"}
        ]);
        let json = serde_json::json!({
            "candidates": [{
                "content": {"role": "model", "parts": parts},
                "finishReason": "FUNCTION_CALLING"
            }]
        });

        let response = parse_generate_content_response(&json, "gemini-3-pro").unwrap();

        let (_, _, raw) = only_reasoning(&response);
        assert_eq!(raw["parts"][0]["thoughtSignature"], "sig_abc");
        assert_eq!(raw["parts"], parts);

        // The ToolUse block is still emitted exactly as today.
        let tools = response.tool_calls();
        assert_eq!(tools.len(), 1);
        match tools[0] {
            ContentBlock::ToolUse { name, .. } => assert_eq!(name, "f"),
            _ => panic!("expected ToolUse"),
        }
        assert_eq!(response.stop.reason, StopReason::ToolUse);
    }

    #[test]
    fn parse_response_thought_part_with_function_call_still_yields_tool_use() {
        // Non-streaming mirror of `sse_thought_part_with_function_call_still_yields_tool_use`.
        // `parse_response_preserves_thought_signature_in_raw` only covers a
        // functionCall part WITHOUT thought:true, so a refactor reintroducing a
        // `continue` on thought:true in the non-streaming parse (but not the SSE
        // one) would go green without this test.
        let json = serde_json::json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{
                        "thought": true,
                        "functionCall": {"name": "f", "args": {"x": 1}},
                        "thoughtSignature": "sig_abc"
                    }]
                },
                "finishReason": "FUNCTION_CALLING"
            }]
        });

        let response = parse_generate_content_response(&json, "gemini-3-pro").unwrap();
        let tools = response.tool_calls();
        assert_eq!(tools.len(), 1);
        match tools[0] {
            ContentBlock::ToolUse { name, input, .. } => {
                assert_eq!(name, "f");
                assert_eq!(input["x"], 1);
            }
            _ => panic!("expected ToolUse"),
        }
        assert_eq!(response.stop.reason, StopReason::ToolUse);
    }

    #[test]
    fn parse_response_without_thoughts_still_stores_parts_for_replay() {
        // Gemini 3 signs the first functionCall part even with no thought parts.
        let parts = serde_json::json!([{"text": "The answer is 42."}]);
        let json = serde_json::json!({
            "candidates": [{
                "content": {"role": "model", "parts": parts},
                "finishReason": "STOP"
            }]
        });

        let response = parse_generate_content_response(&json, "gemini-3-pro").unwrap();

        let (text, _, raw) = only_reasoning(&response);
        assert_eq!(*text, None);
        assert_eq!(raw["parts"], parts);
    }

    #[test]
    fn parse_response_with_no_parts_emits_no_reasoning_block() {
        let json = serde_json::json!({
            "candidates": [{
                "content": {"role": "model", "parts": []},
                "finishReason": "STOP"
            }]
        });

        let response = parse_generate_content_response(&json, "gemini-3-pro").unwrap();
        assert!(response
            .content
            .iter()
            .all(|b| !matches!(b, ContentBlock::Reasoning { .. })));
    }

    #[test]
    fn sse_captures_thought_parts_and_emits_reasoning_delta() {
        let mut acc = GeminiAccumulator::new("gemini-3-pro");
        let mut events = Vec::new();

        let event = crate::sse::SseEvent {
            event_type: "message".into(),
            data:
                r#"{"candidates":[{"content":{"parts":[{"thought":true,"text":"thinking..."}]}}]}"#
                    .into(),
        };
        process_gemini_sse(&event, &mut acc, &mut |e| events.push(e)).unwrap();

        assert_eq!(events.len(), 1);
        match &events[0] {
            StreamEvent::ReasoningDelta(text) => assert_eq!(text, "thinking..."),
            other => panic!("expected ReasoningDelta, got {other:?}"),
        }
        assert!(!events
            .iter()
            .any(|e| matches!(e, StreamEvent::TextDelta(_))));
    }

    #[test]
    fn sse_accumulates_all_parts_verbatim_across_chunks() {
        let mut acc = GeminiAccumulator::new("gemini-3-pro");

        let event1 = crate::sse::SseEvent {
            event_type: "message".into(),
            data: r#"{"candidates":[{"content":{"parts":[{"thought":true,"text":"hmm","thoughtSignature":"sig_1"}]}}]}"#.into(),
        };
        process_gemini_sse(&event1, &mut acc, &mut |_| {}).unwrap();

        let event2 = crate::sse::SseEvent {
            event_type: "message".into(),
            data: r#"{"candidates":[{"content":{"parts":[{"text":"answer","thoughtSignature":"sig_2"}]},"finishReason":"STOP"}]}"#.into(),
        };
        process_gemini_sse(&event2, &mut acc, &mut |_| {}).unwrap();

        let response = acc.into_response().unwrap();
        let (text, provider, raw) = only_reasoning(&response);
        assert_eq!(text.as_deref(), Some("hmm"));
        assert_eq!(provider, PROVIDER_TAG);
        assert_eq!(
            raw["parts"],
            serde_json::json!([
                {"thought": true, "text": "hmm", "thoughtSignature": "sig_1"},
                {"text": "answer", "thoughtSignature": "sig_2"}
            ])
        );
        assert_eq!(response.text(), Some("answer"));
    }

    #[test]
    fn sse_thought_part_with_function_call_still_yields_tool_use() {
        // The old code `continue`d on thought:true before checking for a
        // functionCall on the same part, silently losing the tool call.
        let mut acc = GeminiAccumulator::new("gemini-3-pro");

        let event = crate::sse::SseEvent {
            event_type: "message".into(),
            data: r#"{"candidates":[{"content":{"parts":[{"thought":true,"functionCall":{"name":"f","args":{"x":1}},"thoughtSignature":"sig_abc"}]},"finishReason":"FUNCTION_CALLING"}]}"#.into(),
        };
        process_gemini_sse(&event, &mut acc, &mut |_| {}).unwrap();

        let response = acc.into_response().unwrap();
        let tools = response.tool_calls();
        assert_eq!(tools.len(), 1);
        match tools[0] {
            ContentBlock::ToolUse { name, input, .. } => {
                assert_eq!(name, "f");
                assert_eq!(input["x"], 1);
            }
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn test_sse_text_streaming() {
        let mut acc = GeminiAccumulator::new("gemini-2.5-pro");
        let mut events = Vec::new();

        let event1 = crate::sse::SseEvent {
            event_type: "message".into(),
            data: r#"{"candidates":[{"content":{"parts":[{"text":"Hello "}]}}]}"#.into(),
        };
        process_gemini_sse(&event1, &mut acc, &mut |e| events.push(format!("{e:?}"))).unwrap();

        let event2 = crate::sse::SseEvent {
            event_type: "message".into(),
            data: r#"{"candidates":[{"content":{"parts":[{"text":"world!"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5}}"#.into(),
        };
        process_gemini_sse(&event2, &mut acc, &mut |e| events.push(format!("{e:?}"))).unwrap();

        let response = acc.into_response().unwrap();
        assert_eq!(response.text(), Some("Hello world!"));
        assert_eq!(response.stop.reason, StopReason::EndTurn);
        assert_eq!(response.usage.input_tokens, 10);
        assert_eq!(events.len(), 3);
        assert!(events.iter().any(
            |event| event.contains("UsageSnapshot(Usage { input_tokens: 10, output_tokens: 5")
        ));
    }

    /// I-48, streaming path: `thoughtsTokenCount` must reach `Usage` via
    /// `StreamEvent::UsageSnapshot` too, not just the non-streaming parse.
    #[test]
    fn test_sse_usage_snapshot_counts_thinking_tokens() {
        let mut acc = GeminiAccumulator::new("gemini-2.5-pro");
        let mut events = Vec::new();

        let event = crate::sse::SseEvent {
            event_type: "message".into(),
            data: r#"{"candidates":[{"content":{"parts":[{"text":"world!"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5,"thoughtsTokenCount":120}}"#.into(),
        };
        process_gemini_sse(&event, &mut acc, &mut |e| events.push(format!("{e:?}"))).unwrap();

        let response = acc.into_response().unwrap();
        assert_eq!(response.usage.input_tokens, 10);
        assert_eq!(response.usage.output_tokens, 5);
        assert_eq!(response.usage.reasoning_tokens, 120);
        assert!(events.iter().any(|event| event.contains(
            "UsageSnapshot(Usage { input_tokens: 10, output_tokens: 5, cached_tokens: 0, cache_write_tokens: 0, reasoning_tokens: 120"
        )));
    }

    #[test]
    fn test_sse_function_call_streaming() {
        let mut acc = GeminiAccumulator::new("gemini-2.5-pro");
        let mut events = Vec::new();

        let event = crate::sse::SseEvent {
            event_type: "message".into(),
            data: r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"shell_exec","args":{"command":"ls"}}}]},"finishReason":"FUNCTION_CALLING"}]}"#.into(),
        };
        process_gemini_sse(&event, &mut acc, &mut |e| events.push(format!("{e:?}"))).unwrap();

        let response = acc.into_response().unwrap();
        assert_eq!(response.stop.reason, StopReason::ToolUse);
        assert_eq!(response.tool_calls().len(), 1);
        assert!(!events.is_empty()); // ToolUseStart + InputJsonDelta
    }

    #[test]
    fn test_variant_endpoints() {
        assert!(GeminiVariant::GeminiCli
            .endpoint()
            .contains("cloudcode-pa.googleapis.com"));
        assert!(GeminiVariant::Antigravity
            .endpoint()
            .contains("sandbox.googleapis.com"));
    }

    #[test]
    fn test_variant_client_ids_differ() {
        assert_ne!(
            GeminiVariant::GeminiCli.client_id(),
            GeminiVariant::Antigravity.client_id()
        );
    }

    #[test]
    fn test_variant_user_agents() {
        assert!(GeminiVariant::GeminiCli
            .user_agent()
            .contains("google-cloud-sdk"));
        assert!(GeminiVariant::Antigravity
            .user_agent()
            .contains("antigravity"));
    }

    #[test]
    fn test_map_finish_reason() {
        assert_eq!(map_finish_reason("STOP"), StopReason::EndTurn);
        assert_eq!(map_finish_reason("MAX_TOKENS"), StopReason::MaxTokens);
        assert_eq!(map_finish_reason("FUNCTION_CALLING"), StopReason::ToolUse);
        assert_eq!(map_finish_reason("SAFETY"), StopReason::EndTurn); // graceful
        assert_eq!(map_finish_reason("UNKNOWN"), StopReason::EndTurn);
    }

    #[test]
    fn test_extract_google_error_json() {
        let text = r#"{"error":{"code":429,"message":"Rate limit exceeded","status":"RESOURCE_EXHAUSTED"}}"#;
        assert_eq!(extract_google_error(text), "Rate limit exceeded");
    }

    #[test]
    fn test_extract_google_error_plain() {
        let text = "Internal server error";
        assert_eq!(extract_google_error(text), "Internal server error");
    }

    #[test]
    fn test_stream_url() {
        let client = GoogleGeminiClient::new(
            test_creds("proj"),
            GeminiVariant::GeminiCli,
            None,
            Arc::new(rupu_netflow::NullSink),
        )
        .unwrap();
        assert!(client
            .build_stream_url("gemini-2.5-pro")
            .contains("streamGenerateContent"));
        assert!(client
            .build_stream_url("gemini-2.5-pro")
            .contains("alt=sse"));
    }

    #[test]
    fn ai_studio_url_uses_v1beta_with_model_in_path() {
        let key_creds = AuthCredentials::ApiKey {
            key: "AIzaSy-test".into(),
        };
        let client = GoogleGeminiClient::new(
            key_creds,
            GeminiVariant::AiStudio,
            None,
            Arc::new(rupu_netflow::NullSink),
        )
        .unwrap();
        let url = client.build_url("gemini-2.5-pro");
        assert!(
            url.contains("generativelanguage.googleapis.com")
                && url.contains("v1beta/models/gemini-2.5-pro:generateContent"),
            "got: {url}"
        );
        let stream_url = client.build_stream_url("gemini-2.5-pro");
        assert!(stream_url.contains("streamGenerateContent") && stream_url.contains("alt=sse"));
    }

    #[test]
    fn ai_studio_headers_use_x_goog_api_key_not_authorization() {
        let key_creds = AuthCredentials::ApiKey {
            key: "AIzaSy-key".into(),
        };
        let client = GoogleGeminiClient::new(
            key_creds,
            GeminiVariant::AiStudio,
            None,
            Arc::new(rupu_netflow::NullSink),
        )
        .unwrap();
        let headers = client.build_headers().unwrap();
        assert_eq!(
            headers.get("x-goog-api-key").unwrap().to_str().unwrap(),
            "AIzaSy-key"
        );
        assert!(headers.get(reqwest::header::AUTHORIZATION).is_none());
    }

    #[test]
    fn ai_studio_rejects_oauth_credentials() {
        let oauth_creds = AuthCredentials::OAuth {
            access: "tok".into(),
            refresh: "rtok".into(),
            expires: 0,
            extra: HashMap::new(),
        };
        let result = GoogleGeminiClient::new(
            oauth_creds,
            GeminiVariant::AiStudio,
            None,
            Arc::new(rupu_netflow::NullSink),
        );
        let err = result.err().expect("AI Studio + OAuth should fail");
        match err {
            ProviderError::AuthConfig(msg) => {
                assert!(
                    msg.contains("AI Studio") && msg.contains("API-key"),
                    "unexpected error message: {msg}"
                );
            }
            _ => panic!("expected AuthConfig error"),
        }
    }

    #[test]
    fn cloud_code_assist_still_rejects_api_key() {
        let key_creds = AuthCredentials::ApiKey { key: "k".into() };
        let result = GoogleGeminiClient::new(
            key_creds,
            GeminiVariant::GeminiCli,
            None,
            Arc::new(rupu_netflow::NullSink),
        );
        assert!(matches!(result, Err(ProviderError::AuthConfig(_))));
    }

    #[test]
    fn test_thinking_levels() {
        let client = GoogleGeminiClient::new(
            test_creds("proj"),
            GeminiVariant::GeminiCli,
            None,
            Arc::new(rupu_netflow::NullSink),
        )
        .unwrap();

        for (level, expected_budget) in [
            (crate::model_tier::ThinkingLevel::Minimal, 128),
            (crate::model_tier::ThinkingLevel::Low, 2048),
            (crate::model_tier::ThinkingLevel::Medium, 8192),
            (crate::model_tier::ThinkingLevel::High, 32768),
        ] {
            let request = LlmRequest {
                model: "gemini-2.5-pro".into(),
                system: None,
                messages: vec![Message::user("test")],
                max_tokens: Some(64000),
                tools: vec![],
                cell_id: None,
                trace_id: None,
                thinking: Some(level),
                context_window: None,
                task_type: None,
                output_format: None,
                output_schema: None,
                anthropic_task_budget: None,
                anthropic_context_management: None,
                anthropic_speed: None,
                disable_prompt_cache: false,
            };
            let body = client.build_request_body(&request);
            let config = &body["request"]["generationConfig"]["thinkingConfig"];
            let budget = config["thinkingBudget"].as_u64().unwrap();
            assert_eq!(
                budget, expected_budget,
                "ThinkingLevel::{level:?} budget mismatch"
            );
            // gemini-2.5-pro is not Gemini-3: thinkingLevel (a Gemini-3-only
            // key) must never accompany thinkingBudget.
            assert!(
                config.get("thinkingLevel").is_none(),
                "ThinkingLevel::{level:?}: thinkingLevel must not be sent for gemini-2.5-pro"
            );
        }
    }

    #[test]
    fn test_process_gemini_sse_malformed_json_returns_error() {
        let mut acc = GeminiAccumulator::new("gemini-2.5-pro");
        let bad_event = crate::sse::SseEvent {
            event_type: "message".into(),
            data: "{ not valid json".into(),
        };
        let result = process_gemini_sse(&bad_event, &mut acc, &mut |_| {});
        assert!(result.is_err());
    }

    #[test]
    fn test_gemini_accumulator_empty_returns_none() {
        let acc = GeminiAccumulator::new("gemini-2.5-pro");
        assert!(acc.into_response().is_none());
    }
}

#[cfg(test)]
mod llm_provider_impl_tests {
    use super::*;
    use crate::provider::LlmProvider;
    use crate::provider_id::ProviderId;

    fn oauth_creds() -> AuthCredentials {
        let mut extra = std::collections::HashMap::new();
        extra.insert(
            "project_id".to_string(),
            serde_json::Value::String("test-project".to_string()),
        );
        AuthCredentials::OAuth {
            access: "test-token".into(),
            refresh: "test-refresh".into(),
            expires: 9_999_999_999_999,
            extra,
        }
    }

    #[test]
    fn implements_llm_provider_trait() {
        let client = GoogleGeminiClient::new(
            oauth_creds(),
            GeminiVariant::GeminiCli,
            None,
            Arc::new(rupu_netflow::NullSink),
        )
        .expect("new");
        let boxed: Box<dyn LlmProvider> = Box::new(client);
        assert_eq!(boxed.provider_id(), ProviderId::GoogleGeminiCli);
        assert!(!boxed.default_model().is_empty());
    }

    #[tokio::test]
    async fn list_models_is_empty_for_code_assist() {
        // `list_models` is not overridden, so it keeps the trait default
        // (empty); live limits come from `fetch_models`.
        let client = GoogleGeminiClient::new(
            oauth_creds(),
            GeminiVariant::GeminiCli,
            None,
            Arc::new(rupu_netflow::NullSink),
        )
        .unwrap();
        let models = <GoogleGeminiClient as LlmProvider>::list_models(&client).await;
        assert!(
            models.is_empty(),
            "Gemini list_models keeps the empty trait default; got {} entries",
            models.len()
        );
    }

    fn no_page_token(req: &httpmock::prelude::HttpMockRequest) -> bool {
        !req.query_params
            .as_ref()
            .is_some_and(|q| q.iter().any(|(k, _)| k == "pageToken"))
    }

    #[test]
    fn gemini_listing_strips_prefix_and_reads_limits() {
        let v = serde_json::json!({ "models": [
            { "name": "models/gemini-test-pro", "inputTokenLimit": 1048576, "outputTokenLimit": 65536,
              "supportedGenerationMethods": ["generateContent", "countTokens"] },
            { "name": "models/text-embed-1", "inputTokenLimit": 2048, "outputTokenLimit": 1,
              "supportedGenerationMethods": ["embedContent"] }
        ], "nextPageToken": "p2" });
        let (ms, next) =
            gemini_models_from_listing(&v, crate::provider_id::ProviderId::GoogleGeminiCli);
        assert_eq!(next.as_deref(), Some("p2"));
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].id, "gemini-test-pro");
        assert_eq!(
            (ms[0].context_window, ms[0].max_output_tokens),
            (1_048_576, 65_536)
        );
    }

    #[tokio::test]
    async fn fetch_models_ai_studio_follows_pages() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let p1 = server.mock(|when, then| {
            when.method(GET)
                .path("/v1beta/models")
                .header("x-goog-api-key", "g-key")
                .matches(no_page_token);
            then.status(200).json_body(serde_json::json!({ "models": [
                { "name": "models/g-one", "inputTokenLimit": 10, "outputTokenLimit": 5, "supportedGenerationMethods": ["generateContent"] }
            ], "nextPageToken": "tok2" }));
        });
        let p2 = server.mock(|when, then| {
            when.method(GET)
                .path("/v1beta/models")
                .query_param("pageToken", "tok2");
            then.status(200).json_body(serde_json::json!({ "models": [
                { "name": "models/g-two", "inputTokenLimit": 20, "outputTokenLimit": 6, "supportedGenerationMethods": ["generateContent"] }
            ]}));
        });
        let mut client = GoogleGeminiClient::new(
            AuthCredentials::ApiKey {
                key: "g-key".into(),
            },
            GeminiVariant::AiStudio,
            None,
            Arc::new(rupu_netflow::NullSink),
        )
        .unwrap();
        client.api_base_override = Some(server.url(""));
        let ms = <GoogleGeminiClient as LlmProvider>::fetch_models(&mut client)
            .await
            .unwrap();
        p1.assert();
        p2.assert();
        assert_eq!(
            ms.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["g-one", "g-two"]
        );
    }

    #[tokio::test]
    async fn fetch_models_code_assist_has_no_listing() {
        let mut client = GoogleGeminiClient::new(
            oauth_creds(),
            GeminiVariant::GeminiCli,
            None,
            Arc::new(rupu_netflow::NullSink),
        )
        .unwrap();
        let err = <GoogleGeminiClient as LlmProvider>::fetch_models(&mut client)
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::NotImplemented { .. }));
        assert!(!<GoogleGeminiClient as LlmProvider>::output_shares_context(
            &client
        ));
    }

    #[tokio::test]
    async fn fetch_models_ai_studio_surfaces_non_2xx_as_error() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let _mock = server.mock(|when, then| {
            when.method(GET).path("/v1beta/models");
            then.status(403);
        });
        let mut client = GoogleGeminiClient::new(
            AuthCredentials::ApiKey {
                key: "g-key".into(),
            },
            GeminiVariant::AiStudio,
            None,
            Arc::new(rupu_netflow::NullSink),
        )
        .unwrap();
        client.api_base_override = Some(server.url(""));
        let err = <GoogleGeminiClient as LlmProvider>::fetch_models(&mut client)
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Api { status: 403, .. }));
    }

    #[tokio::test]
    async fn fetch_models_duplicate_token_stops_early() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let p1 = server.mock(|when, then| {
            when.method(GET)
                .path("/v1beta/models")
                .matches(no_page_token);
            then.status(200).json_body(serde_json::json!({ "models": [
                { "name": "models/g-one", "inputTokenLimit": 10, "outputTokenLimit": 5, "supportedGenerationMethods": ["generateContent"] }
            ], "nextPageToken": "t" }));
        });
        let p2 = server.mock(|when, then| {
            when.method(GET)
                .path("/v1beta/models")
                .query_param("pageToken", "t");
            then.status(200).json_body(serde_json::json!({ "models": [
                { "name": "models/g-two", "inputTokenLimit": 20, "outputTokenLimit": 6, "supportedGenerationMethods": ["generateContent"] }
            ], "nextPageToken": "t" }));
        });
        let mut client = GoogleGeminiClient::new(
            AuthCredentials::ApiKey {
                key: "g-key".into(),
            },
            GeminiVariant::AiStudio,
            None,
            Arc::new(rupu_netflow::NullSink),
        )
        .unwrap();
        client.api_base_override = Some(server.url(""));
        let ms = <GoogleGeminiClient as LlmProvider>::fetch_models(&mut client)
            .await
            .unwrap();
        // Each id should appear exactly once (no duplicates on duplicate token loop)
        assert_eq!(
            ms.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["g-one", "g-two"]
        );
        // First page (no pageToken) should be hit once
        p1.assert_hits(1);
        // Second page with pageToken=t should be hit exactly once (duplicate token stops loop)
        p2.assert_hits(1);
    }

    fn ai_studio_client(base: String) -> GoogleGeminiClient {
        let mut client = GoogleGeminiClient::new(
            AuthCredentials::ApiKey {
                key: "g-key".into(),
            },
            GeminiVariant::AiStudio,
            None,
            Arc::new(rupu_netflow::NullSink),
        )
        .unwrap();
        client.api_base_override = Some(base);
        client
    }

    /// An A -> B -> A page-token cycle (not just an immediate repeat) must
    /// stop before re-requesting a page, so no model is collected twice.
    #[tokio::test]
    async fn fetch_models_stops_on_an_a_b_a_token_cycle() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let entry = |id: &str| {
            serde_json::json!({ "name": format!("models/{id}"), "inputTokenLimit": 10,
                "outputTokenLimit": 5, "supportedGenerationMethods": ["generateContent"] })
        };
        let p1 = server.mock(|when, then| {
            when.method(GET)
                .path("/v1beta/models")
                .matches(no_page_token);
            then.status(200).json_body(
                serde_json::json!({ "models": [entry("g-one")], "nextPageToken": "tok-a" }),
            );
        });
        let p2 = server.mock(|when, then| {
            when.method(GET)
                .path("/v1beta/models")
                .query_param("pageToken", "tok-a");
            then.status(200).json_body(
                serde_json::json!({ "models": [entry("g-two")], "nextPageToken": "tok-b" }),
            );
        });
        let p3 = server.mock(|when, then| {
            when.method(GET)
                .path("/v1beta/models")
                .query_param("pageToken", "tok-b");
            then.status(200).json_body(
                serde_json::json!({ "models": [entry("g-three")], "nextPageToken": "tok-a" }),
            );
        });
        let mut client = ai_studio_client(server.url(""));
        let ms = <GoogleGeminiClient as LlmProvider>::fetch_models(&mut client)
            .await
            .unwrap();
        p1.assert_hits(1);
        p2.assert_hits(1);
        p3.assert_hits(1);
        assert_eq!(
            ms.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["g-one", "g-two", "g-three"],
            "no duplicates"
        );
    }

    /// A server that ignores `pageToken` and answers every request with the
    /// same page: the repeated token stops the loop, but the repeated page
    /// must not leave its models in the result twice.
    #[tokio::test]
    async fn fetch_models_dedupes_ids_when_the_server_ignores_the_token() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let p1 = server.mock(|when, then| {
            when.method(GET)
                .path("/v1beta/models")
                .matches(no_page_token);
            then.status(200).json_body(serde_json::json!({ "models": [
                { "name": "models/g-one", "inputTokenLimit": 10, "outputTokenLimit": 5, "supportedGenerationMethods": ["generateContent"] }
            ], "nextPageToken": "tok-a" }));
        });
        // Same page again, same token again.
        let p2 = server.mock(|when, then| {
            when.method(GET)
                .path("/v1beta/models")
                .query_param("pageToken", "tok-a");
            then.status(200).json_body(serde_json::json!({ "models": [
                { "name": "models/g-one", "inputTokenLimit": 999, "outputTokenLimit": 99, "supportedGenerationMethods": ["generateContent"] }
            ], "nextPageToken": "tok-a" }));
        });
        let mut client = ai_studio_client(server.url(""));
        let ms = <GoogleGeminiClient as LlmProvider>::fetch_models(&mut client)
            .await
            .unwrap();
        p1.assert_hits(1);
        p2.assert_hits(1);
        assert_eq!(
            ms.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["g-one"],
            "no duplicate ids"
        );
        // The first occurrence wins.
        assert_eq!((ms[0].context_window, ms[0].max_output_tokens), (10, 5));
    }

    /// A 200 whose body is not JSON is a decode failure, not a transport
    /// failure.
    #[tokio::test]
    async fn fetch_models_non_json_body_is_a_json_error() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/v1beta/models");
            then.status(200).body("not json");
        });
        let mut client = ai_studio_client(server.url(""));
        let err = <GoogleGeminiClient as LlmProvider>::fetch_models(&mut client)
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Json(_)), "{err:?}");
    }

    /// Every page — not just the first — asks for `pageSize=1000` and
    /// authenticates with `x-goog-api-key`.
    #[tokio::test]
    async fn fetch_models_page_two_keeps_page_size_and_api_key_header() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let p1 = server.mock(|when, then| {
            when.method(GET)
                .path("/v1beta/models")
                .matches(no_page_token);
            then.status(200).json_body(serde_json::json!({ "models": [
                { "name": "models/g-one", "inputTokenLimit": 10, "outputTokenLimit": 5, "supportedGenerationMethods": ["generateContent"] }
            ], "nextPageToken": "tok2" }));
        });
        let p2 = server.mock(|when, then| {
            when.method(GET)
                .path("/v1beta/models")
                .query_param("pageToken", "tok2")
                .query_param("pageSize", "1000")
                .header("x-goog-api-key", "g-key");
            then.status(200).json_body(serde_json::json!({ "models": [
                { "name": "models/g-two", "inputTokenLimit": 20, "outputTokenLimit": 6, "supportedGenerationMethods": ["generateContent"] }
            ]}));
        });
        let mut client = ai_studio_client(server.url(""));
        let ms = <GoogleGeminiClient as LlmProvider>::fetch_models(&mut client)
            .await
            .unwrap();
        p1.assert_hits(1);
        p2.assert_hits(1);
        assert_eq!(ms.len(), 2);
    }

    #[tokio::test]
    async fn fetch_models_respects_50_page_limit() {
        use httpmock::prelude::*;
        let server = MockServer::start();

        // Page 1 mock (no pageToken)
        let page1 = server.mock(|when, then| {
            when.method(GET)
                .path("/v1beta/models")
                .matches(no_page_token);
            then.status(200).json_body(serde_json::json!({
                "models": [{"name": "models/g-1", "inputTokenLimit": 1000, "outputTokenLimit": 100, "supportedGenerationMethods": ["generateContent"]}],
                "nextPageToken": "t1"
            }));
        });

        // Pages 2-50 mocks (pageToken t1..t49, each with nextPageToken t2..t50)
        let mut page_mocks = Vec::new();
        for i in 1..=49 {
            let token = format!("t{}", i);
            let next_token = format!("t{}", i + 1);
            let model_name = format!("g-{}", i + 1);
            let mock = server.mock(|when, then| {
                when.method(GET)
                    .path("/v1beta/models")
                    .query_param("pageToken", token.as_str());
                then.status(200).json_body(serde_json::json!({
                    "models": [{"name": format!("models/{}", model_name), "inputTokenLimit": 1000 + i, "outputTokenLimit": 100, "supportedGenerationMethods": ["generateContent"]}],
                    "nextPageToken": next_token
                }));
            });
            page_mocks.push(mock);
        }

        // Page 51 mock (pageToken t50) — should never be called because cap stops at 50 iterations
        let page51 = server.mock(|when, then| {
            when.method(GET)
                .path("/v1beta/models")
                .query_param("pageToken", "t50");
            then.status(200).json_body(serde_json::json!({
                "models": [{"name": "models/g-51", "inputTokenLimit": 1000, "outputTokenLimit": 100, "supportedGenerationMethods": ["generateContent"]}],
                "nextPageToken": "t51"
            }));
        });

        let mut client = GoogleGeminiClient::new(
            AuthCredentials::ApiKey {
                key: "g-key".into(),
            },
            GeminiVariant::AiStudio,
            None,
            Arc::new(rupu_netflow::NullSink),
        )
        .unwrap();
        client.api_base_override = Some(server.url(""));
        let ms = <GoogleGeminiClient as LlmProvider>::fetch_models(&mut client)
            .await
            .unwrap();

        // Verify exactly 50 pages fetched
        assert_eq!(ms.len(), 50, "should fetch exactly 50 models at the cap");

        // Verify no duplicates
        let ids: std::collections::HashSet<_> = ms.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids.len(), 50, "all 50 model ids should be unique");

        // Verify page 1 (no pageToken) was hit exactly once
        page1.assert_hits(1);

        // Verify pages 2-50 (pageToken t1..t49) were each hit exactly once
        for mock in &page_mocks {
            mock.assert_hits(1);
        }

        // Verify page 51 (pageToken t50) was never called — the cap stopped at 50
        page51.assert_hits(0);
    }
}

/// The Code Assist project a request carries: settled before the first
/// request (stored, `GOOGLE_CLOUD_PROJECT`, or set up and recorded), never
/// sent empty.
#[cfg(test)]
mod code_assist_project_tests {
    use super::*;
    use crate::credential_writes::OAuthRefresher;
    use httpmock::prelude::*;
    use serde_json::json;

    /// A credential store that records what the client asks it to record.
    #[derive(Default)]
    struct RecordingStore {
        recorded: std::sync::Mutex<Vec<(String, HashMap<String, serde_json::Value>)>>,
    }

    #[async_trait::async_trait]
    impl OAuthRefresher for RecordingStore {
        async fn refresh(&self, _stale: AuthCredentials) -> Result<AuthCredentials, ProviderError> {
            Err(ProviderError::TokenRefreshFailed(
                "no refresh expected".into(),
            ))
        }

        async fn record_extra(
            &self,
            holder: AuthCredentials,
            fields: HashMap<String, serde_json::Value>,
        ) -> Result<bool, ProviderError> {
            if let AuthCredentials::OAuth { refresh, .. } = holder {
                self.recorded.lock().unwrap().push((refresh, fields));
            }
            Ok(true)
        }
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn env_project(k: &str) -> Option<String> {
        (k == "GOOGLE_CLOUD_PROJECT").then(|| "env-project".to_string())
    }

    fn creds(project: Option<&str>, variant: Option<&str>) -> AuthCredentials {
        let mut extra = HashMap::new();
        if let Some(p) = project {
            extra.insert("project_id".to_string(), json!(p));
        }
        if let Some(v) = variant {
            extra.insert("variant".to_string(), json!(v));
        }
        AuthCredentials::OAuth {
            access: "access-1".into(),
            refresh: "refresh-1".into(),
            expires: 9_999_999_999_999,
            extra,
        }
    }

    fn client_for(
        server: &MockServer,
        creds: AuthCredentials,
        variant: GeminiVariant,
        store: Option<Arc<RecordingStore>>,
    ) -> GoogleGeminiClient {
        let mut client =
            GoogleGeminiClient::new(creds, variant, None, Arc::new(rupu_netflow::NullSink))
                .unwrap()
                .with_oauth_refresher(store.map(|s| s as Arc<dyn OAuthRefresher>));
        client.code_assist_base = server.url("");
        client.env = no_env;
        client
    }

    fn request() -> LlmRequest {
        LlmRequest {
            model: "gemini-2.5-pro".into(),
            system: None,
            messages: vec![Message::user("Hello")],
            max_tokens: Some(16),
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
        }
    }

    /// A Code Assist `generateContent` answer: the response wrapped in
    /// gemini-cli's `CaGenerateContentResponse` envelope.
    fn wrapped_answer() -> serde_json::Value {
        json!({
            "response": {
                "candidates": [{
                    "content": { "role": "model", "parts": [{ "text": "hello back" }] },
                    "finishReason": "STOP",
                }],
                "usageMetadata": { "promptTokenCount": 3, "candidatesTokenCount": 2 },
            },
            "traceId": "trace-1",
        })
    }

    fn text_of(response: &LlmResponse) -> String {
        response
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    /// A login from before rupu stored projects: the first request sets the
    /// account up, carries the project, and has the store record it (with
    /// the variant) against the grant the client holds. The second request
    /// reuses it.
    #[tokio::test]
    async fn a_credential_without_a_project_is_set_up_on_the_first_request_and_recorded() {
        let server = MockServer::start();
        let load = server.mock(|when, then| {
            when.method(POST)
                .path("/v1internal:loadCodeAssist")
                .header("authorization", "Bearer access-1");
            then.status(200).json_body(json!({
                "currentTier": { "id": "free-tier" },
                "cloudaicompanionProject": "managed-1",
            }));
        });
        let generate = server.mock(|when, then| {
            when.method(POST)
                .path("/v1internal:generateContent")
                .json_body_partial(r#"{ "project": "managed-1" }"#);
            then.status(200).json_body(wrapped_answer());
        });
        let store = Arc::new(RecordingStore::default());
        let mut client = client_for(
            &server,
            creds(None, None),
            GeminiVariant::GeminiCli,
            Some(store.clone()),
        );

        let first = client.send(&request()).await.unwrap();
        client.send(&request()).await.unwrap();

        assert_eq!(text_of(&first), "hello back");
        load.assert_hits(1);
        generate.assert_hits(2);
        let recorded = store.recorded.lock().unwrap();
        assert_eq!(recorded.len(), 1, "{recorded:?}");
        assert_eq!(recorded[0].0, "refresh-1");
        assert_eq!(recorded[0].1["project_id"], json!("managed-1"));
        assert_eq!(recorded[0].1["variant"], json!("gemini-cli"));
    }

    /// An Antigravity credential is set up against its own endpoint, and
    /// its variant is recorded with the project.
    #[tokio::test]
    async fn an_antigravity_credential_records_its_variant_with_the_project() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v1internal:loadCodeAssist");
            then.status(200).json_body(json!({
                "currentTier": { "id": "standard-tier" },
                "cloudaicompanionProject": "ag-1",
            }));
        });
        server.mock(|when, then| {
            when.method(POST)
                .path("/v1internal:generateContent")
                .json_body_partial(r#"{ "project": "ag-1" }"#);
            then.status(200).json_body(wrapped_answer());
        });
        let store = Arc::new(RecordingStore::default());
        let mut client = client_for(
            &server,
            creds(None, Some("antigravity")),
            GeminiVariant::Antigravity,
            Some(store.clone()),
        );

        client.send(&request()).await.unwrap();

        let recorded = store.recorded.lock().unwrap();
        assert_eq!(recorded[0].1["variant"], json!("antigravity"));
        assert_eq!(recorded[0].1["project_id"], json!("ag-1"));
    }

    /// A stored project is used as is: no setup call, nothing recorded.
    #[tokio::test]
    async fn a_stored_project_is_used_without_setup() {
        let server = MockServer::start();
        let load = server.mock(|when, then| {
            when.method(POST).path("/v1internal:loadCodeAssist");
            then.status(500);
        });
        let generate = server.mock(|when, then| {
            when.method(POST)
                .path("/v1internal:streamGenerateContent")
                .query_param("alt", "sse")
                .json_body_partial(r#"{ "project": "stored-1" }"#);
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(format!("data: {}\n\n", wrapped_answer()));
        });
        let store = Arc::new(RecordingStore::default());
        let mut client = client_for(
            &server,
            creds(Some("stored-1"), None),
            GeminiVariant::GeminiCli,
            Some(store.clone()),
        );

        let response = client.stream(&request(), &mut |_| {}).await.unwrap();

        assert_eq!(text_of(&response), "hello back");
        load.assert_hits(0);
        generate.assert_hits(1);
        assert!(store.recorded.lock().unwrap().is_empty());
    }

    /// `GOOGLE_CLOUD_PROJECT` overrides the stored project: the account is
    /// set up with it and requests carry it, but it is never recorded — the
    /// stored project is still there once the variable goes.
    #[tokio::test]
    async fn the_env_override_is_set_up_and_used_but_never_recorded() {
        let server = MockServer::start();
        let load = server.mock(|when, then| {
            when.method(POST)
                .path("/v1internal:loadCodeAssist")
                .json_body_partial(r#"{ "cloudaicompanionProject": "env-project" }"#);
            then.status(200).json_body(json!({
                "currentTier": { "id": "standard-tier" },
                "cloudaicompanionProject": "env-project",
            }));
        });
        let generate = server.mock(|when, then| {
            when.method(POST)
                .path("/v1internal:generateContent")
                .json_body_partial(r#"{ "project": "env-project" }"#);
            then.status(200).json_body(wrapped_answer());
        });
        let store = Arc::new(RecordingStore::default());
        let mut client = client_for(
            &server,
            creds(Some("stored-1"), None),
            GeminiVariant::GeminiCli,
            Some(store.clone()),
        );
        client.env = env_project;

        client.send(&request()).await.unwrap();
        client.send(&request()).await.unwrap();

        load.assert_hits(1);
        generate.assert_hits(2);
        assert!(store.recorded.lock().unwrap().is_empty());
        assert_eq!(client.project_id, "stored-1");
    }

    /// An override equal to the stored project needs no setup.
    #[tokio::test]
    async fn an_override_equal_to_the_stored_project_needs_no_setup() {
        let server = MockServer::start();
        let load = server.mock(|when, then| {
            when.method(POST).path("/v1internal:loadCodeAssist");
            then.status(500);
        });
        let generate = server.mock(|when, then| {
            when.method(POST)
                .path("/v1internal:generateContent")
                .json_body_partial(r#"{ "project": "env-project" }"#);
            then.status(200).json_body(wrapped_answer());
        });
        let mut client = client_for(
            &server,
            creds(Some("env-project"), None),
            GeminiVariant::GeminiCli,
            None,
        );
        client.env = env_project;

        client.send(&request()).await.unwrap();

        load.assert_hits(0);
        generate.assert_hits(1);
    }

    /// No project to be had: the request fails with the actionable setup
    /// error and nothing goes out with an empty project — on either path.
    #[tokio::test]
    async fn no_project_fails_before_any_request_goes_out() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v1internal:loadCodeAssist");
            then.status(200)
                .json_body(json!({ "currentTier": { "id": "standard-tier" } }));
        });
        let generate = server.mock(|when, then| {
            when.method(POST).path_contains("GenerateContent");
            then.status(200).json_body(wrapped_answer());
        });
        let generate_lower = server.mock(|when, then| {
            when.method(POST).path_contains("generateContent");
            then.status(200).json_body(wrapped_answer());
        });
        let store = Arc::new(RecordingStore::default());
        let mut client = client_for(
            &server,
            creds(None, None),
            GeminiVariant::GeminiCli,
            Some(store.clone()),
        );

        let sent = client.send(&request()).await.unwrap_err().to_string();
        let streamed = client
            .stream(&request(), &mut |_| {})
            .await
            .unwrap_err()
            .to_string();

        for msg in [sent, streamed] {
            assert!(msg.contains("GOOGLE_CLOUD_PROJECT"), "{msg}");
        }
        generate.assert_hits(0);
        generate_lower.assert_hits(0);
        assert!(store.recorded.lock().unwrap().is_empty());
    }

    /// The first requests of a fan-out on an account with no stored project
    /// (several clients in one process at once) set the account up once —
    /// one `loadCodeAssist`, one record — and all carry the project.
    #[tokio::test]
    async fn concurrent_first_requests_set_the_account_up_once() {
        let server = MockServer::start();
        let load = server.mock(|when, then| {
            when.method(POST).path("/v1internal:loadCodeAssist");
            then.status(200)
                .delay(std::time::Duration::from_millis(200))
                .json_body(json!({
                    "currentTier": { "id": "free-tier" },
                    "cloudaicompanionProject": "managed-fan",
                }));
        });
        let generate = server.mock(|when, then| {
            when.method(POST)
                .path("/v1internal:generateContent")
                .json_body_partial(r#"{ "project": "managed-fan" }"#);
            then.status(200).json_body(wrapped_answer());
        });
        let store = Arc::new(RecordingStore::default());
        let mut clients: Vec<GoogleGeminiClient> = (0..3)
            .map(|_| {
                client_for(
                    &server,
                    creds(None, None),
                    GeminiVariant::GeminiCli,
                    Some(store.clone()),
                )
            })
            .collect();
        let req = request();

        let results =
            futures_util::future::join_all(clients.iter_mut().map(|c| c.send(&req))).await;

        for r in results {
            r.unwrap();
        }
        load.assert_hits(1);
        generate.assert_hits(3);
        assert_eq!(store.recorded.lock().unwrap().len(), 1);
    }

    /// The setup is shared only while it runs: a client that asks after it
    /// finished sets the account up afresh (a long-lived process never pins
    /// a result, and a record that failed is retried by the next client).
    #[tokio::test]
    async fn a_setup_is_shared_only_while_it_runs() {
        let server = MockServer::start();
        let load = server.mock(|when, then| {
            when.method(POST).path("/v1internal:loadCodeAssist");
            then.status(200).json_body(json!({
                "currentTier": { "id": "free-tier" },
                "cloudaicompanionProject": "managed-seq",
            }));
        });
        server.mock(|when, then| {
            when.method(POST).path("/v1internal:generateContent");
            then.status(200).json_body(wrapped_answer());
        });
        let store = Arc::new(RecordingStore::default());

        for _ in 0..2 {
            let mut client = client_for(
                &server,
                creds(None, None),
                GeminiVariant::GeminiCli,
                Some(store.clone()),
            );
            client.send(&request()).await.unwrap();
        }

        load.assert_hits(2);
        assert_eq!(store.recorded.lock().unwrap().len(), 2);
    }

    /// Clients waiting on a setup that fails get its failure, rather than
    /// each running the setup again one after another.
    #[tokio::test]
    async fn clients_waiting_on_a_failed_setup_share_its_failure() {
        let server = MockServer::start();
        let load = server.mock(|when, then| {
            when.method(POST).path("/v1internal:loadCodeAssist");
            then.status(200)
                .delay(std::time::Duration::from_millis(200))
                .json_body(json!({ "currentTier": { "id": "standard-tier" } }));
        });
        let mut clients: Vec<GoogleGeminiClient> = (0..3)
            .map(|_| client_for(&server, creds(None, None), GeminiVariant::GeminiCli, None))
            .collect();
        let req = request();

        let results =
            futures_util::future::join_all(clients.iter_mut().map(|c| c.send(&req))).await;

        for r in results {
            let msg = r.unwrap_err().to_string();
            assert!(msg.contains("GOOGLE_CLOUD_PROJECT"), "{msg}");
        }
        load.assert_hits(1);
    }

    /// A client of the legacy auth file (`ProviderRegistry`, no credential
    /// store) writes the project there, as its token refresh does.
    #[tokio::test]
    async fn the_legacy_auth_file_records_the_project() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v1internal:loadCodeAssist");
            then.status(200).json_body(json!({
                "currentTier": { "id": "free-tier" },
                "cloudaicompanionProject": "managed-legacy",
            }));
        });
        server.mock(|when, then| {
            when.method(POST).path("/v1internal:generateContent");
            then.status(200).json_body(wrapped_answer());
        });
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("auth.json");
        let mut client = GoogleGeminiClient::new(
            creds(None, None),
            GeminiVariant::GeminiCli,
            Some(path.clone()),
            Arc::new(rupu_netflow::NullSink),
        )
        .unwrap();
        client.code_assist_base = server.url("");
        client.env = no_env;

        client.send(&request()).await.unwrap();

        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            saved["google-gemini-cli"]["project_id"],
            json!("managed-legacy"),
            "{saved}"
        );
        assert_eq!(saved["google-gemini-cli"]["refresh"], json!("refresh-1"));
    }

    #[test]
    fn a_variant_s_credential_hint_maps_back_to_it() {
        for variant in [GeminiVariant::GeminiCli, GeminiVariant::Antigravity] {
            let hint = variant.credential_hint();
            assert!(hint.is_some(), "{variant:?}");
            assert_eq!(GeminiVariant::from_credential_hint(hint), variant);
        }
        assert_eq!(GeminiVariant::AiStudio.credential_hint(), None);
    }

    /// Code Assist wraps every answer — a whole one, or each stream chunk —
    /// as `{"response": …, "traceId": …}` (gemini-cli's
    /// `CaGenerateContentResponse`); both parsers read through it.
    #[test]
    fn the_code_assist_envelope_is_read_through() {
        let whole = parse_generate_content_response(&wrapped_answer(), "gemini-2.5-pro").unwrap();
        assert_eq!(text_of(&whole), "hello back");
        assert_eq!(whole.usage.input_tokens, 3);

        let mut acc = GeminiAccumulator::new("gemini-2.5-pro");
        let event = crate::sse::SseEvent {
            event_type: "message".into(),
            data: wrapped_answer().to_string(),
        };
        process_gemini_sse(&event, &mut acc, &mut |_| {}).unwrap();
        let streamed = acc.into_response().unwrap();
        assert_eq!(text_of(&streamed), "hello back");
        assert_eq!(streamed.usage.input_tokens, 3);
    }
}
