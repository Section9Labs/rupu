//! A factory-built Gemini SSO client settles its Code Assist project (the
//! Google Cloud project requests are billed against) before its first
//! request, end to end through the credential store.
//!
//! Before this, nothing ever wrote a project: every SSO request carried
//! `"project": ""`. A credential stored without one is now set up on the
//! first request and the project recorded in `auth.json`, so the next
//! process skips the setup; `GOOGLE_CLOUD_PROJECT` overrides it without
//! being stored. `#[serial]`: these set process-wide env vars.

use rupu_auth::backend::ProviderId;
use rupu_auth::resolver::KeychainResolver;
use rupu_auth::stored::StoredCredential;
use rupu_providers::types::{LlmRequest, Message};
use rupu_providers::AuthMode;
use serde_json::json;
use serial_test::serial;

/// Sets or clears env vars for the test's duration and restores what was
/// there on drop, even on panic — a developer's own `GOOGLE_CLOUD_PROJECT`
/// must neither leak in nor be lost.
struct EnvGuard(Vec<(&'static str, Option<String>)>);

impl EnvGuard {
    fn new(vars: &[(&'static str, Option<&str>)]) -> Self {
        let prior = vars
            .iter()
            .map(|(k, v)| {
                let prior = std::env::var(k).ok();
                match v {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
                (*k, prior)
            })
            .collect();
        Self(prior)
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (k, prior) in &self.0 {
            match prior {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
    }
}

fn gemini_sso(project: Option<&str>) -> StoredCredential {
    let expires = chrono::Utc::now() + chrono::Duration::hours(1);
    let mut extra = std::collections::HashMap::new();
    if let Some(p) = project {
        extra.insert("project_id".to_string(), json!(p));
    }
    StoredCredential {
        credentials: rupu_providers::auth::AuthCredentials::OAuth {
            access: "access-1".into(),
            refresh: "refresh-1".into(),
            expires: expires.timestamp_millis() as u64,
            extra,
        },
        refresh_token: Some("refresh-1".into()),
        expires_at: Some(expires),
    }
}

fn request() -> LlmRequest {
    LlmRequest {
        model: "gemini-2.5-pro".into(),
        system: None,
        messages: vec![Message::user("hi")],
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

/// A Code Assist answer, in its `{"response": …}` envelope.
fn answer() -> serde_json::Value {
    json!({
        "response": {
            "candidates": [{
                "content": { "role": "model", "parts": [{ "text": "hello" }] },
                "finishReason": "STOP",
            }],
            "usageMetadata": { "promptTokenCount": 2, "candidatesTokenCount": 1 },
        },
        "traceId": "t-1",
    })
}

async fn send_once(resolver: &KeychainResolver) -> rupu_providers::types::LlmResponse {
    let (_, mut provider) = rupu_runtime::provider_factory::build_for_provider_with_config(
        "gemini",
        "gemini-2.5-pro",
        Some(AuthMode::Sso),
        resolver,
        &rupu_runtime::provider_factory::ProviderConfig::default(),
        std::sync::Arc::new(rupu_netflow::NullSink),
    )
    .await
    .expect("client builds");
    provider
        .send(&request())
        .await
        .expect("the request succeeds")
}

fn stored_project(auth_path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(auth_path).unwrap();
    let map: std::collections::HashMap<String, String> = serde_json::from_str(&text).unwrap();
    let sc: StoredCredential = serde_json::from_str(&map["gemini/sso"]).unwrap();
    match sc.credentials {
        rupu_providers::auth::AuthCredentials::OAuth { extra, .. } => extra
            .get("project_id")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn a_credential_without_a_project_is_set_up_once_and_the_project_recorded() {
    use httpmock::prelude::*;
    let server = MockServer::start();
    let load = server.mock(|when, then| {
        when.method(POST)
            .path("/v1internal:loadCodeAssist")
            .header("authorization", "Bearer access-1");
        then.status(200).json_body(json!({
            "currentTier": { "id": "free-tier" },
            "cloudaicompanionProject": "managed-777",
        }));
    });
    let generate = server.mock(|when, then| {
        when.method(POST)
            .path("/v1internal:generateContent")
            .json_body_partial(r#"{ "project": "managed-777" }"#);
        then.status(200).json_body(answer());
    });
    let tmp = tempfile::tempdir().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let _env = EnvGuard::new(&[
        ("RUPU_AUTH_FILE", auth_path.to_str()),
        (
            "RUPU_CODE_ASSIST_ENDPOINT_OVERRIDE",
            Some(server.url("").as_str()),
        ),
        ("GOOGLE_CLOUD_PROJECT", None),
        ("GOOGLE_CLOUD_PROJECT_ID", None),
    ]);
    let resolver = KeychainResolver::new();
    resolver
        .store(ProviderId::Gemini, AuthMode::Sso, &gemini_sso(None))
        .await
        .unwrap();

    send_once(&resolver).await;
    assert_eq!(stored_project(&auth_path).as_deref(), Some("managed-777"));

    // The next client (the next process, in effect) reads it back: no setup.
    send_once(&resolver).await;
    load.assert_hits(1);
    generate.assert_hits(2);
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn google_cloud_project_overrides_the_stored_project_without_replacing_it() {
    use httpmock::prelude::*;
    let server = MockServer::start();
    let load = server.mock(|when, then| {
        when.method(POST)
            .path("/v1internal:loadCodeAssist")
            .json_body_partial(r#"{ "cloudaicompanionProject": "env-project-1" }"#);
        then.status(200).json_body(json!({
            "currentTier": { "id": "standard-tier" },
            "cloudaicompanionProject": "env-project-1",
        }));
    });
    let generate = server.mock(|when, then| {
        when.method(POST)
            .path("/v1internal:generateContent")
            .json_body_partial(r#"{ "project": "env-project-1" }"#);
        then.status(200).json_body(answer());
    });
    let tmp = tempfile::tempdir().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let _env = EnvGuard::new(&[
        ("RUPU_AUTH_FILE", auth_path.to_str()),
        (
            "RUPU_CODE_ASSIST_ENDPOINT_OVERRIDE",
            Some(server.url("").as_str()),
        ),
        ("GOOGLE_CLOUD_PROJECT", Some("env-project-1")),
        ("GOOGLE_CLOUD_PROJECT_ID", None),
    ]);
    let resolver = KeychainResolver::new();
    resolver
        .store(
            ProviderId::Gemini,
            AuthMode::Sso,
            &gemini_sso(Some("stored-project")),
        )
        .await
        .unwrap();

    send_once(&resolver).await;

    load.assert_hits(1);
    generate.assert_hits(1);
    assert_eq!(
        stored_project(&auth_path).as_deref(),
        Some("stored-project")
    );
}
