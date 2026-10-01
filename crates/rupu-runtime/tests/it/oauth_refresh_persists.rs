//! A provider client built by the factory refreshes its OAuth token through
//! the credential store, so the rotated refresh token is persisted.
//!
//! The resolver refreshes a stored SSO credential within 60s of expiry; the
//! provider clients refresh within 5 minutes. A token handed out with 1–5
//! minutes left used to be refreshed by the client alone, in memory: the
//! server rotated the refresh token, the store kept the dead one, and the
//! next process got `invalid_grant`. `#[serial]`: it sets process-wide env
//! vars.

use rupu_auth::backend::ProviderId;
use rupu_auth::resolver::KeychainResolver;
use rupu_auth::stored::StoredCredential;
use rupu_providers::types::{LlmRequest, Message};
use rupu_providers::AuthMode;
use serial_test::serial;

/// Clears the env vars this test sets, even on panic, so they can't leak
/// into a sibling test in this binary.
struct EnvGuard(&'static [&'static str]);
impl Drop for EnvGuard {
    fn drop(&mut self) {
        for k in self.0 {
            std::env::remove_var(k);
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn a_factory_built_client_persists_its_token_refresh() {
    use httpmock::prelude::*;
    let server = MockServer::start();
    let token = server.mock(|when, then| {
        when.method(POST).path("/token").body_contains("refresh-1");
        then.status(200).json_body(serde_json::json!({
            "access_token": "access-2",
            "refresh_token": "refresh-2",
            "expires_in": 3600
        }));
    });
    let tmp = tempfile::tempdir().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let _env = EnvGuard(&[
        "RUPU_AUTH_FILE",
        "RUPU_OAUTH_TOKEN_URL_OVERRIDE",
        "RUPU_ANTHROPIC_BASE_URL_OVERRIDE",
    ]);
    std::env::set_var("RUPU_AUTH_FILE", &auth_path);
    std::env::set_var("RUPU_OAUTH_TOKEN_URL_OVERRIDE", server.url("/token"));
    // Messages and the OAuth bootstrap go to the mock (unmatched: 404).
    std::env::set_var(
        "RUPU_ANTHROPIC_BASE_URL_OVERRIDE",
        format!("{}/v1/messages", server.url("")),
    );

    let resolver = KeychainResolver::new();
    let three_minutes = chrono::Utc::now() + chrono::Duration::minutes(3);
    resolver
        .store(
            ProviderId::Anthropic,
            AuthMode::Sso,
            &StoredCredential {
                credentials: rupu_providers::auth::AuthCredentials::OAuth {
                    access: "access-1".into(),
                    refresh: "refresh-1".into(),
                    expires: three_minutes.timestamp_millis() as u64,
                    extra: Default::default(),
                },
                refresh_token: Some("refresh-1".into()),
                expires_at: Some(three_minutes),
            },
        )
        .await
        .unwrap();

    let (_, mut provider) = rupu_runtime::provider_factory::build_for_provider_with_config(
        "anthropic",
        "claude-sonnet-4-6",
        Some(AuthMode::Sso),
        &resolver,
        &rupu_runtime::provider_factory::ProviderConfig::default(),
        std::sync::Arc::new(rupu_netflow::NullSink),
    )
    .await
    .expect("client builds");
    let request = LlmRequest {
        model: "claude-sonnet-4-6".into(),
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
    };
    // The message itself 404s; the refresh before it is what matters.
    let _ = provider.send(&request).await;

    token.assert_hits(1);
    let saved = std::fs::read_to_string(&auth_path).unwrap();
    assert!(
        saved.contains("refresh-2") && saved.contains("access-2"),
        "the rotation is persisted: {saved}"
    );
}
