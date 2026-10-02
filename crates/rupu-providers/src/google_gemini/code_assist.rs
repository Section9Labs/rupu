//! Gemini Code Assist user setup: which Google Cloud project a Code Assist
//! request is billed and metered against.
//!
//! Every Cloud Code Assist request (`v1internal:generateContent`) names a
//! `project` — a Google Cloud project ID, not a rupu project. Google assigns
//! one per account: a free-tier account gets a Google-managed project when it
//! is onboarded, and a paid (standard-tier) account brings its own through
//! `GOOGLE_CLOUD_PROJECT`.
//!
//! A port of gemini-cli's `setupUser` (`packages/core/src/code_assist/setup.ts`)
//! and the `loadCodeAssist` / `onboardUser` / `getOperation` calls of its
//! `CodeAssistServer` (`packages/core/src/code_assist/server.ts`), as of
//! google-gemini/gemini-cli@c9096a847193c16e282d7bd20a70fddc57646bbe — same
//! endpoints, same request bodies, same choice of project, same onboarding
//! poll. Not ported: gemini-cli's interactive account-validation handler (the
//! validation link is reported in the error instead) and its in-memory 30s
//! cache of the result (rupu stores the project in the credential instead).

use std::time::Duration;

use reqwest_middleware::ClientWithMiddleware;
use serde::Deserialize;
use serde_json::json;
use tracing::{info, warn};

use super::GeminiVariant;
use crate::error::ProviderError;

/// The credential `extra` key holding the Code Assist project.
pub const EXTRA_PROJECT_ID: &str = "project_id";

/// The credential `extra` key naming the OAuth client the token was issued
/// to ([`GeminiVariant::credential_hint`]).
pub const EXTRA_VARIANT: &str = "variant";

/// Test seam: when set, every Code Assist call (setup and generateContent)
/// goes to this base URL instead of the variant's endpoint — the
/// counterpart of `RUPU_OAUTH_TOKEN_URL_OVERRIDE`.
pub const ENDPOINT_OVERRIDE_ENV: &str = "RUPU_CODE_ASSIST_ENDPOINT_OVERRIDE";

/// The two variables gemini-cli reads for a user-chosen project, in its
/// order (`setup.ts`: `GOOGLE_CLOUD_PROJECT || GOOGLE_CLOUD_PROJECT_ID`).
pub const PROJECT_ENV_VARS: [&str; 2] = ["GOOGLE_CLOUD_PROJECT", "GOOGLE_CLOUD_PROJECT_ID"];

/// What every "no project" error tells the user to do.
const PROJECT_HINT: &str = "Set GOOGLE_CLOUD_PROJECT (or GOOGLE_CLOUD_PROJECT_ID) to the ID of a \
     Google Cloud project this account can use for Gemini Code Assist (e.g. `export \
     GOOGLE_CLOUD_PROJECT=my-project-123`), or use an AI Studio API key instead: `rupu auth login \
     --provider gemini --mode api-key`.";

/// The Code Assist base URL for `variant` (no API version): the variant's
/// endpoint, or [`ENDPOINT_OVERRIDE_ENV`] when set.
pub fn endpoint(variant: GeminiVariant) -> String {
    std::env::var(ENDPOINT_OVERRIDE_ENV).unwrap_or_else(|_| variant.endpoint().to_string())
}

/// gemini-cli's `CODE_ASSIST_API_VERSION`.
const API_VERSION: &str = "v1internal";

/// gemini-cli's `UserTierId.FREE`: onboarded without a project (Google
/// provisions a managed one).
const FREE_TIER: &str = "free-tier";

/// gemini-cli's `UserTierId.STANDARD`.
const STANDARD_TIER: &str = "standard-tier";

/// gemini-cli's `UserTierId.LEGACY`: the tier onboarded when the server
/// marks none as default (`getOnboardTier`).
const LEGACY_TIER: &str = "legacy-tier";

/// How long to wait for an onboarding operation. gemini-cli polls every 5s
/// with no bound; rupu bounds the wait so a run can't hang on an operation
/// Google never finishes.
#[derive(Debug, Clone)]
pub struct OnboardPoll {
    pub interval: Duration,
    pub timeout: Duration,
}

impl Default for OnboardPoll {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(5),
            timeout: Duration::from_secs(300),
        }
    }
}

/// A project the user chose through the environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectOverride {
    /// The variable it came from (one of [`PROJECT_ENV_VARS`]).
    pub var: &'static str,
    pub project: String,
}

/// The user's project override: `GOOGLE_CLOUD_PROJECT`, else
/// `GOOGLE_CLOUD_PROJECT_ID`, an empty value counting as unset — read through
/// `env` (`std::env::var` in production). A numeric value is a project
/// *number*, which Code Assist rejects: refused here with the fix, as
/// gemini-cli's `InvalidNumericProjectIdError` does.
pub fn project_override(
    env: impl Fn(&str) -> Option<String>,
) -> Result<Option<ProjectOverride>, ProviderError> {
    let Some((var, project)) = PROJECT_ENV_VARS
        .iter()
        .find_map(|var| env(var).filter(|v| !v.is_empty()).map(|v| (*var, v)))
    else {
        return Ok(None);
    };
    if project.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ProviderError::AuthConfig(format!(
            "{var} is \"{project}\", a numeric Google Cloud project number. Gemini Code Assist \
             needs the project ID instead (e.g. \"my-project-123\"): set {var} to it."
        )));
    }
    Ok(Some(ProjectOverride { var, project }))
}

/// gemini-cli's `setupUser`: ask Code Assist which project `access_token`'s
/// account uses (`loadCodeAssist`), onboarding the account first when it has
/// no tier yet (`onboardUser`, polling the long-running operation), and
/// return the project ID. `project` is the user's override, sent as the
/// requested project. Fails with an actionable error when no project can be
/// obtained — never an empty one.
pub async fn setup_user(
    client: &ClientWithMiddleware,
    base: &str,
    variant: GeminiVariant,
    access_token: &str,
    project: Option<&str>,
    poll: &OnboardPoll,
) -> Result<String, ProviderError> {
    let api = Api {
        client,
        base: format!("{}/{API_VERSION}", base.trim_end_matches('/')),
        variant,
        access_token,
        project,
    };

    let mut load_body = json!({ "metadata": client_metadata(project) });
    if let Some(p) = project {
        load_body["cloudaicompanionProject"] = json!(p);
    }
    let load: LoadCodeAssistResponse = match api.post("loadCodeAssist", &load_body).await {
        Ok(load) => load,
        // gemini-cli's `loadCodeAssist`: an account behind VPC Service
        // Controls is refused, and treated as a standard-tier one.
        Err(Failure::Status { body, .. }) if is_vpc_sc_refusal(&body) => {
            warn!("Code Assist loadCodeAssist was refused by VPC Service Controls; treating the account as standard tier");
            LoadCodeAssistResponse {
                current_tier: Some(UserTier {
                    id: Some(STANDARD_TIER.into()),
                    ..Default::default()
                }),
                ..Default::default()
            }
        }
        Err(e) => return Err(api.error("loadCodeAssist", e)),
    };
    validate(&load)?;

    if let Some(tier) = &load.current_tier {
        let chosen = non_empty(load.cloudaicompanion_project.clone())
            .or_else(|| project.map(str::to_string))
            .ok_or_else(|| no_project(&load))?;
        info!(tier = ?tier.id, project = %chosen, "Code Assist account already set up");
        return Ok(chosen);
    }

    let tier = onboard_tier(&load);
    let onboard_body = if tier.id.as_deref() == Some(FREE_TIER) {
        // The free tier uses a managed project: naming one fails the
        // onboarding with `Precondition Failed` (gemini-cli).
        json!({ "tierId": FREE_TIER, "metadata": client_metadata(None) })
    } else {
        let mut body = json!({ "metadata": client_metadata(project) });
        if let Some(id) = &tier.id {
            body["tierId"] = json!(id);
        }
        if let Some(p) = project {
            body["cloudaicompanionProject"] = json!(p);
        }
        body
    };
    info!(tier = ?tier.id, "onboarding the account to Gemini Code Assist");
    let mut op: Operation = api
        .post("onboardUser", &onboard_body)
        .await
        .map_err(|e| api.error("onboardUser", e))?;
    if !op.done {
        if let Some(name) = op.name.clone() {
            let deadline = tokio::time::Instant::now() + poll.timeout;
            while !op.done {
                if tokio::time::Instant::now() >= deadline {
                    return Err(ProviderError::Other(anyhow::anyhow!(
                        "Google had not finished setting up Gemini Code Assist for this account \
                         after {}s (onboarding operation {name}). Retry in a minute; the next \
                         attempt checks again.",
                        poll.timeout.as_secs()
                    )));
                }
                tokio::time::sleep(poll.interval).await;
                op = api
                    .get(&name)
                    .await
                    .map_err(|e| api.error("getOperation", e))?;
            }
        }
    }

    let chosen = op
        .response
        .and_then(|r| r.cloudaicompanion_project)
        .and_then(|p| non_empty(p.id))
        .or_else(|| project.map(str::to_string))
        .ok_or_else(|| no_project(&load))?;
    info!(tier = ?tier.id, project = %chosen, "Code Assist onboarding finished");
    Ok(chosen)
}

/// The result of a setup some caller in this process already ran for `key`
/// (an account at an endpoint, with the requested project), or `setup`'s —
/// run by one caller at a time per key, the others waiting for it. `true`
/// with the project when this call ran `setup` itself.
///
/// The first requests of a fan-out on an account with no stored project
/// would otherwise each set it up, and onboard it, at once; gemini-cli
/// caches `setupUser` per auth client for the same reason (`userDataCache`
/// in `setup.ts`). Only a success is kept: after a failure the next caller
/// runs the setup again.
pub async fn shared_setup<F>(key: String, setup: F) -> Result<(String, bool), ProviderError>
where
    F: std::future::Future<Output = Result<String, ProviderError>>,
{
    type Slot = std::sync::Arc<tokio::sync::Mutex<Option<String>>>;
    static SETUPS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Slot>>> =
        std::sync::OnceLock::new();
    let slot = SETUPS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .entry(key)
        .or_default()
        .clone();
    let mut done = slot.lock().await;
    if let Some(project) = done.as_ref() {
        return Ok((project.clone(), false));
    }
    let project = setup.await?;
    *done = Some(project.clone());
    Ok((project, true))
}

/// gemini-cli's `coreClientMetadata`, with `duetProject` when a project is
/// requested.
fn client_metadata(project: Option<&str>) -> serde_json::Value {
    let mut metadata = json!({
        "ideType": "IDE_UNSPECIFIED",
        "platform": "PLATFORM_UNSPECIFIED",
        "pluginType": "GEMINI",
    });
    if let Some(p) = project {
        metadata["duetProject"] = json!(p);
    }
    metadata
}

/// gemini-cli's `validateLoadCodeAssistResponse`: an account with no tier
/// that Google wants validated first.
fn validate(load: &LoadCodeAssistResponse) -> Result<(), ProviderError> {
    if load.current_tier.is_some() {
        return Ok(());
    }
    let validation = load.ineligible_tiers.iter().flatten().find(|t| {
        t.reason_code.as_deref() == Some("VALIDATION_REQUIRED") && non_empty_ref(&t.validation_url)
    });
    match validation {
        Some(t) => Err(ProviderError::AuthConfig(format!(
            "Google requires this account to be validated before it can use Gemini Code \
             Assist: {} Complete the validation at {}, then retry.",
            t.reason_message
                .as_deref()
                .unwrap_or("account validation required."),
            t.validation_url.as_deref().unwrap_or_default(),
        ))),
        None => Ok(()),
    }
}

/// gemini-cli's `getOnboardTier`: the server's default tier, else legacy.
fn onboard_tier(load: &LoadCodeAssistResponse) -> UserTier {
    load.allowed_tiers
        .iter()
        .flatten()
        .find(|t| t.is_default)
        .cloned()
        .unwrap_or_else(|| UserTier {
            id: Some(LEGACY_TIER.into()),
            ..Default::default()
        })
}

/// gemini-cli's `throwIneligibleOrProjectIdError`: the server's reasons when
/// it gave any, else `ProjectIdRequiredError` — each with what to do.
fn no_project(load: &LoadCodeAssistResponse) -> ProviderError {
    let reasons: Vec<&str> = load
        .ineligible_tiers
        .iter()
        .flatten()
        .filter_map(|t| t.reason_message.as_deref())
        .filter(|m| !m.is_empty())
        .collect();
    if reasons.is_empty() {
        ProviderError::AuthConfig(format!(
            "Gemini Code Assist needs a Google Cloud project for this Google account, and Google \
             did not assign one. {PROJECT_HINT}"
        ))
    } else {
        ProviderError::AuthConfig(format!(
            "Google did not assign a Gemini Code Assist project to this Google account: {}. \
             {PROJECT_HINT}",
            reasons.join(", ")
        ))
    }
}

/// gemini-cli's `isVpcScAffectedUser`: an error whose details carry the
/// `SECURITY_POLICY_VIOLATED` reason.
fn is_vpc_sc_refusal(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.pointer("/error/details").cloned())
        .and_then(|d| d.as_array().cloned())
        .is_some_and(|details| {
            details.iter().any(|d| {
                d.get("reason").and_then(|r| r.as_str()) == Some("SECURITY_POLICY_VIOLATED")
            })
        })
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.is_empty())
}

fn non_empty_ref(s: &Option<String>) -> bool {
    s.as_deref().is_some_and(|s| !s.is_empty())
}

/// One setup's connection to Code Assist.
struct Api<'a> {
    client: &'a ClientWithMiddleware,
    /// `<endpoint>/v1internal`.
    base: String,
    variant: GeminiVariant,
    access_token: &'a str,
    /// The user's override, for the hint on a refusal.
    project: Option<&'a str>,
}

/// A setup call that did not return the expected JSON.
enum Failure {
    /// The server answered with a non-2xx status.
    Status {
        status: u16,
        headers: reqwest::header::HeaderMap,
        body: String,
    },
    /// No answer, or an answer that is not the expected JSON.
    Other(ProviderError),
}

impl Api<'_> {
    /// gemini-cli's `requestPost`: `POST <base>:<method>`.
    async fn post<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        body: &serde_json::Value,
    ) -> Result<T, Failure> {
        let url = format!("{}:{method}", self.base);
        self.send(self.client.post(url).json(body)).await
    }

    /// gemini-cli's `requestGetOperation`: `GET <base>/<operation name>`.
    async fn get<T: serde::de::DeserializeOwned>(&self, name: &str) -> Result<T, Failure> {
        let url = format!("{}/{name}", self.base);
        self.send(self.client.get(url)).await
    }

    async fn send<T: serde::de::DeserializeOwned>(
        &self,
        request: reqwest_middleware::RequestBuilder,
    ) -> Result<T, Failure> {
        let response = request
            .bearer_auth(self.access_token)
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::USER_AGENT, self.variant.user_agent())
            .send()
            .await
            .map_err(|e| Failure::Other(e.into()))?;
        let status = response.status();
        if !status.is_success() {
            let headers = response.headers().clone();
            let body = response.text().await.unwrap_or_default();
            return Err(Failure::Status {
                status: status.as_u16(),
                headers,
                body,
            });
        }
        let body = response
            .text()
            .await
            .map_err(|e| Failure::Other(e.into()))?;
        serde_json::from_str(&body).map_err(|e| Failure::Other(e.into()))
    }

    /// The error for `method`'s failure: the HTTP status kept (so the retry
    /// layers classify it) with Google's message and the call named.
    fn error(&self, method: &str, failure: Failure) -> ProviderError {
        match failure {
            Failure::Status {
                status,
                headers,
                body,
            } => {
                let mut message = format!(
                    "Gemini Code Assist setup ({method}) failed: {}",
                    super::extract_google_error(&body)
                );
                if let (Some(p), 403) = (self.project, status) {
                    message.push_str(&format!(
                        ". Check that the Google Cloud project \"{p}\" (GOOGLE_CLOUD_PROJECT) \
                         exists and this account can use Gemini Code Assist in it"
                    ));
                }
                crate::error::api_error_from_response(status, &headers, message)
            }
            Failure::Other(ProviderError::Json(e)) => ProviderError::Json(format!(
                "Gemini Code Assist setup ({method}) returned an unexpected body: {e}"
            )),
            Failure::Other(e) => e,
        }
    }
}

/// gemini-cli's `LoadCodeAssistResponse` (the fields setup reads).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoadCodeAssistResponse {
    #[serde(default)]
    current_tier: Option<UserTier>,
    #[serde(default)]
    allowed_tiers: Option<Vec<UserTier>>,
    #[serde(default)]
    ineligible_tiers: Option<Vec<IneligibleTier>>,
    #[serde(default)]
    cloudaicompanion_project: Option<String>,
}

/// gemini-cli's `GeminiUserTier` (the fields setup reads).
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserTier {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    is_default: bool,
}

/// gemini-cli's `IneligibleTier` (the fields setup reads).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IneligibleTier {
    #[serde(default)]
    reason_code: Option<String>,
    #[serde(default)]
    reason_message: Option<String>,
    #[serde(default)]
    validation_url: Option<String>,
}

/// gemini-cli's `LongRunningOperationResponse`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Operation {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    response: Option<OnboardUserResponse>,
}

/// gemini-cli's `OnboardUserResponse`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OnboardUserResponse {
    #[serde(default)]
    cloudaicompanion_project: Option<ProjectRef>,
}

#[derive(Debug, Default, Deserialize)]
struct ProjectRef {
    #[serde(default)]
    id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use serde_json::json;
    use std::sync::Arc;

    fn client() -> ClientWithMiddleware {
        super::super::gemini_http_client(Arc::new(rupu_netflow::NullSink)).unwrap()
    }

    fn fast() -> OnboardPoll {
        OnboardPoll {
            interval: Duration::from_millis(10),
            timeout: Duration::from_secs(5),
        }
    }

    async fn setup(server: &MockServer, project: Option<&str>) -> Result<String, ProviderError> {
        setup_user(
            &client(),
            &server.url(""),
            GeminiVariant::GeminiCli,
            "access-1",
            project,
            &fast(),
        )
        .await
    }

    /// The `metadata` gemini-cli sends (`coreClientMetadata`), with
    /// `duetProject` only when a project was requested.
    fn metadata(project: Option<&str>) -> serde_json::Value {
        let mut m = json!({
            "ideType": "IDE_UNSPECIFIED",
            "platform": "PLATFORM_UNSPECIFIED",
            "pluginType": "GEMINI",
        });
        if let Some(p) = project {
            m["duetProject"] = json!(p);
        }
        m
    }

    /// An account that already has a tier gets its project straight from
    /// `loadCodeAssist`; nothing is onboarded.
    #[tokio::test]
    async fn an_onboarded_account_s_project_comes_from_load_code_assist() {
        let server = MockServer::start();
        let load = server.mock(|when, then| {
            when.method(POST)
                .path("/v1internal:loadCodeAssist")
                .header("authorization", "Bearer access-1")
                .json_body(json!({ "metadata": metadata(None) }));
            then.status(200).json_body(json!({
                "currentTier": { "id": "free-tier", "name": "Gemini Code Assist for individuals" },
                "allowedTiers": [{ "id": "free-tier", "isDefault": true }],
                "cloudaicompanionProject": "managed-project-123",
            }));
        });
        let onboard = server.mock(|when, then| {
            when.method(POST).path("/v1internal:onboardUser");
            then.status(500);
        });

        assert_eq!(setup(&server, None).await.unwrap(), "managed-project-123");
        load.assert_hits(1);
        onboard.assert_hits(0);
    }

    /// A new free-tier account is onboarded with the default tier and no
    /// project (gemini-cli: setting one makes the free tier fail with
    /// `Precondition Failed`), and the managed project comes out of the
    /// long-running operation once it reports done.
    #[tokio::test]
    async fn a_new_free_tier_account_is_onboarded_and_the_operation_polled() {
        let server = MockServer::start();
        let load = server.mock(|when, then| {
            when.method(POST).path("/v1internal:loadCodeAssist");
            then.status(200).json_body(json!({
                "allowedTiers": [
                    { "id": "standard-tier", "userDefinedCloudaicompanionProject": true },
                    { "id": "free-tier", "name": "Free", "isDefault": true },
                ],
            }));
        });
        let onboard = server.mock(|when, then| {
            when.method(POST)
                .path("/v1internal:onboardUser")
                .header("authorization", "Bearer access-1")
                .json_body(json!({ "tierId": "free-tier", "metadata": metadata(None) }));
            then.status(200)
                .json_body(json!({ "name": "operations/onboard-1", "done": false }));
        });
        let pending = server.mock(|when, then| {
            when.method(GET)
                .path("/v1internal/operations/onboard-1")
                .header("authorization", "Bearer access-1");
            then.status(200)
                .json_body(json!({ "name": "operations/onboard-1", "done": false }));
        });

        // Two polls see "not done", then the operation finishes. httpmock
        // answers with the first-created matching mock, so the "done" mock is
        // added before the "pending" one goes: no poll can fall in a gap.
        let flip = async {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            while pending.hits() < 2 {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "the operation was never polled twice"
                );
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            let done = server.mock(|when, then| {
                when.method(GET).path("/v1internal/operations/onboard-1");
                then.status(200).json_body(json!({
                    "name": "operations/onboard-1",
                    "done": true,
                    "response": {
                        "cloudaicompanionProject": { "id": "managed-456", "name": "Managed" }
                    },
                }));
            });
            let mut pending = pending;
            pending.delete();
            done
        };
        let (project, done) = tokio::join!(setup(&server, None), flip);

        assert_eq!(project.unwrap(), "managed-456");
        load.assert_hits(1);
        onboard.assert_hits(1);
        done.assert_hits(1);
    }

    /// The user's project is the requested project of both calls
    /// (`cloudaicompanionProject` and `metadata.duetProject`); a standard-tier
    /// account onboarded onto it uses it when the operation names none.
    #[tokio::test]
    async fn the_override_is_requested_and_used_for_a_standard_tier_account() {
        let server = MockServer::start();
        let load = server.mock(|when, then| {
            when.method(POST)
                .path("/v1internal:loadCodeAssist")
                .json_body(json!({
                    "cloudaicompanionProject": "my-project-123",
                    "metadata": metadata(Some("my-project-123")),
                }));
            then.status(200).json_body(json!({
                "allowedTiers": [{ "id": "standard-tier", "isDefault": true }],
            }));
        });
        let onboard = server.mock(|when, then| {
            when.method(POST)
                .path("/v1internal:onboardUser")
                .json_body(json!({
                    "tierId": "standard-tier",
                    "cloudaicompanionProject": "my-project-123",
                    "metadata": metadata(Some("my-project-123")),
                }));
            then.status(200)
                .json_body(json!({ "done": true, "response": {} }));
        });

        assert_eq!(
            setup(&server, Some("my-project-123")).await.unwrap(),
            "my-project-123"
        );
        load.assert_hits(1);
        onboard.assert_hits(1);
    }

    /// An account with a tier but no project, and no override: gemini-cli's
    /// `ProjectIdRequiredError`, saying what to set.
    #[tokio::test]
    async fn no_project_and_no_override_is_an_actionable_error() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v1internal:loadCodeAssist");
            then.status(200)
                .json_body(json!({ "currentTier": { "id": "standard-tier" } }));
        });

        let err = setup(&server, None).await.unwrap_err();
        assert!(matches!(err, ProviderError::AuthConfig(_)), "{err:?}");
        let msg = err.to_string();
        assert!(msg.contains("GOOGLE_CLOUD_PROJECT"), "{msg}");
        assert!(msg.contains("--mode api-key"), "{msg}");
    }

    /// Ineligibility reasons from the server are surfaced, with the same fix.
    #[tokio::test]
    async fn ineligible_tiers_are_reported_with_their_reasons() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v1internal:loadCodeAssist");
            then.status(200).json_body(json!({
                "allowedTiers": [{ "id": "free-tier", "isDefault": true }],
                "ineligibleTiers": [{
                    "reasonCode": "INELIGIBLE_ACCOUNT",
                    "reasonMessage": "Your account is not eligible for the free tier",
                }],
            }));
        });
        server.mock(|when, then| {
            when.method(POST).path("/v1internal:onboardUser");
            then.status(200)
                .json_body(json!({ "done": true, "response": {} }));
        });

        let msg = setup(&server, None).await.unwrap_err().to_string();
        assert!(
            msg.contains("Your account is not eligible for the free tier"),
            "{msg}"
        );
        assert!(msg.contains("GOOGLE_CLOUD_PROJECT"), "{msg}");
    }

    /// An account Google wants validated first: the validation link is in
    /// the error (rupu has no interactive handler to open it).
    #[tokio::test]
    async fn a_validation_requirement_reports_the_link() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v1internal:loadCodeAssist");
            then.status(200).json_body(json!({
                "ineligibleTiers": [{
                    "reasonCode": "VALIDATION_REQUIRED",
                    "reasonMessage": "Verify your account to continue.",
                    "validationUrl": "https://accounts.google.com/verify?x=1",
                }],
            }));
        });
        let onboard = server.mock(|when, then| {
            when.method(POST).path("/v1internal:onboardUser");
            then.status(500);
        });

        let msg = setup(&server, None).await.unwrap_err().to_string();
        assert!(msg.contains("Verify your account to continue."), "{msg}");
        assert!(
            msg.contains("https://accounts.google.com/verify?x=1"),
            "{msg}"
        );
        onboard.assert_hits(0);
    }

    /// gemini-cli treats a VPC Service Controls refusal of `loadCodeAssist`
    /// as a standard-tier account: the override is used.
    #[tokio::test]
    async fn a_vpc_sc_refusal_is_a_standard_tier_account() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v1internal:loadCodeAssist");
            then.status(403).json_body(json!({
                "error": {
                    "code": 403,
                    "message": "Request is prohibited by organization's policy.",
                    "details": [{ "reason": "SECURITY_POLICY_VIOLATED" }],
                },
            }));
        });

        assert_eq!(setup(&server, Some("corp-1")).await.unwrap(), "corp-1");
    }

    /// Any other HTTP failure keeps its status (so the retry layers classify
    /// it) and Google's message, naming the setup call.
    #[tokio::test]
    async fn an_http_failure_keeps_its_status_and_google_s_message() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v1internal:loadCodeAssist");
            then.status(503)
                .json_body(json!({ "error": { "message": "backend unavailable" } }));
        });

        match setup(&server, None).await.unwrap_err() {
            ProviderError::Api { status, message } => {
                assert_eq!(status, 503);
                assert!(message.contains("loadCodeAssist"), "{message}");
                assert!(message.contains("backend unavailable"), "{message}");
            }
            other => panic!("expected Api, got {other:?}"),
        }
    }

    /// An onboarding operation that never reports done fails at the bound
    /// instead of hanging the run.
    #[tokio::test]
    async fn an_onboarding_that_never_finishes_fails_at_the_bound() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v1internal:loadCodeAssist");
            then.status(200)
                .json_body(json!({ "allowedTiers": [{ "id": "free-tier", "isDefault": true }] }));
        });
        server.mock(|when, then| {
            when.method(POST).path("/v1internal:onboardUser");
            then.status(200)
                .json_body(json!({ "name": "operations/slow", "done": false }));
        });
        server.mock(|when, then| {
            when.method(GET).path("/v1internal/operations/slow");
            then.status(200)
                .json_body(json!({ "name": "operations/slow", "done": false }));
        });

        let err = setup_user(
            &client(),
            &server.url(""),
            GeminiVariant::GeminiCli,
            "access-1",
            None,
            &OnboardPoll {
                interval: Duration::from_millis(10),
                timeout: Duration::from_millis(100),
            },
        )
        .await
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("operations/slow"), "{msg}");
        assert!(!crate::tuned::is_retryable(&err), "{err:?}");
    }

    fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| {
            pairs
                .iter()
                .find(|(name, _)| *name == k)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn google_cloud_project_wins_over_its_id_variant() {
        let got = project_override(env_of(&[
            ("GOOGLE_CLOUD_PROJECT", "first"),
            ("GOOGLE_CLOUD_PROJECT_ID", "second"),
        ]))
        .unwrap();
        assert_eq!(
            got,
            Some(ProjectOverride {
                var: "GOOGLE_CLOUD_PROJECT",
                project: "first".into()
            })
        );
    }

    #[test]
    fn an_empty_variable_counts_as_unset() {
        let got = project_override(env_of(&[
            ("GOOGLE_CLOUD_PROJECT", ""),
            ("GOOGLE_CLOUD_PROJECT_ID", "second"),
        ]))
        .unwrap();
        assert_eq!(
            got,
            Some(ProjectOverride {
                var: "GOOGLE_CLOUD_PROJECT_ID",
                project: "second".into()
            })
        );
        assert_eq!(project_override(env_of(&[])).unwrap(), None);
    }

    #[test]
    fn a_numeric_project_number_is_refused_with_the_fix() {
        let msg = project_override(env_of(&[("GOOGLE_CLOUD_PROJECT", "123456789012")]))
            .unwrap_err()
            .to_string();
        assert!(msg.contains("123456789012"), "{msg}");
        assert!(msg.contains("project ID"), "{msg}");
    }

    #[test]
    fn the_endpoint_follows_the_variant() {
        // Env-free: the override seam is exercised by the integration tests.
        if std::env::var_os(ENDPOINT_OVERRIDE_ENV).is_some() {
            return;
        }
        assert_eq!(
            endpoint(GeminiVariant::GeminiCli),
            "https://cloudcode-pa.googleapis.com"
        );
        assert_eq!(
            endpoint(GeminiVariant::Antigravity),
            "https://daily-cloudcode-pa.sandbox.googleapis.com"
        );
    }
}
