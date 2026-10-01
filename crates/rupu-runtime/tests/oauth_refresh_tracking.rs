//! One client refresh is one tracked credential write.
//!
//! A factory-built client hands its refresh to the store's refresher, whose
//! own refresh-and-persist task is what `rupu_providers::credential_writes`
//! tracks (and the binary drains before exit). The client's wrapper around
//! that call must not be tracked a second time — "waiting for 2 credential
//! write(s)" for one refresh is wrong, and a double count would also hold
//! the SIGTERM drain for a task that itself only waits. Its own test binary:
//! the count is process-wide and the env vars are too.

use rupu_auth::backend::ProviderId;
use rupu_auth::resolver::KeychainResolver;
use rupu_auth::stored::StoredCredential;
use rupu_providers::credential_writes;
use rupu_providers::types::{LlmRequest, Message};
use rupu_providers::AuthMode;

#[tokio::test(flavor = "multi_thread")]
async fn a_factory_client_refresh_counts_as_one_tracked_write() {
    use httpmock::prelude::*;
    let server = MockServer::start();
    let token = server.mock(|when, then| {
        when.method(POST).path("/token").body_contains("refresh-1");
        then.status(200)
            .delay(std::time::Duration::from_millis(400))
            .json_body(serde_json::json!({
                "access_token": "access-2",
                "refresh_token": "refresh-2",
                "expires_in": 3600
            }));
    });
    let tmp = tempfile::tempdir().unwrap();
    let auth_path = tmp.path().join("auth.json");
    std::env::set_var("RUPU_AUTH_FILE", &auth_path);
    std::env::set_var("RUPU_OAUTH_TOKEN_URL_OVERRIDE", server.url("/token"));
    std::env::set_var(
        "RUPU_ANTHROPIC_BASE_URL_OVERRIDE",
        format!("{}/v1/messages", server.url("")),
    );

    let resolver = KeychainResolver::new();
    // Inside the client's 5-minute buffer, outside the resolver's 60s one:
    // the client, not the resolver's `get`, starts the refresh.
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
    assert_eq!(credential_writes::pending(), 0, "nothing in flight yet");

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
    let send = tokio::spawn(async move { provider.send(&request).await });
    // Mid-refresh (the token endpoint answers after 400ms).
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(
        credential_writes::pending(),
        1,
        "one refresh is one tracked write, not a task wrapping a task"
    );
    let _ = send.await.unwrap();
    token.assert_hits(1);
    assert!(
        credential_writes::wait(std::time::Duration::from_secs(5)).await,
        "and it finishes"
    );
    assert_eq!(credential_writes::pending(), 0);
}
