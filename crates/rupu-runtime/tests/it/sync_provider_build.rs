//! `build_provider_from_credential`: the checks that touch process env, so they
//! are `#[serial]` like every other provider-factory test in this binary (the
//! factory reads `RUPU_MOCK_PROVIDER_SCRIPT`; the bootstrap check points
//! `RUPU_ANTHROPIC_BASE_URL_OVERRIDE` at an httpmock server). The env-free
//! checks (sync vs async equivalence, the `RequiresAsyncBootstrap` refusal)
//! are unit tests in `provider_factory.rs`.

use std::sync::Arc;

use async_trait::async_trait;
use httpmock::prelude::*;
use rupu_providers::auth::AuthCredentials;
use rupu_providers::types::{LlmRequest, Message};
use rupu_providers::AuthMode;
use rupu_runtime::provider_factory::{
    build_for_provider_with_config, build_provider_from_credential, FactoryError, ProviderConfig,
};
use serial_test::serial;

/// Clears the env vars a test sets, even on panic.
struct EnvGuard(Vec<&'static str>);
impl Drop for EnvGuard {
    fn drop(&mut self) {
        for k in &self.0 {
            std::env::remove_var(k);
        }
    }
}

fn sink() -> Arc<dyn rupu_netflow::FlowSink> {
    Arc::new(rupu_netflow::NullSink)
}

fn oauth() -> AuthCredentials {
    AuthCredentials::OAuth {
        access: "sk-ant-oat01-test".to_string(),
        refresh: "refresh".to_string(),
        expires: 0,
        extra: Default::default(),
    }
}

struct OAuthResolver;

#[async_trait]
impl rupu_auth::CredentialResolver for OAuthResolver {
    async fn get(
        &self,
        _provider: &str,
        _hint: Option<AuthMode>,
    ) -> anyhow::Result<(AuthMode, AuthCredentials)> {
        Ok((AuthMode::Sso, oauth()))
    }

    async fn refresh(&self, _provider: &str, _mode: AuthMode) -> anyhow::Result<AuthCredentials> {
        unreachable!("tests never refresh")
    }
}

/// The seam an e2e drives through the sync builder: with the script set it
/// returns the mock (mode `ApiKey`, whatever the caller passed), and does so
/// BEFORE the Anthropic-OAuth refusal, exactly where the async builder does.
#[tokio::test]
#[serial]
async fn the_mock_script_is_honored_before_anything_else() {
    let _guard = EnvGuard(vec!["RUPU_MOCK_PROVIDER_SCRIPT"]);
    std::env::set_var(
        "RUPU_MOCK_PROVIDER_SCRIPT",
        r#"[{"AssistantText":{"text":"ok","stop":"end_turn"}}]"#,
    );
    // Anthropic OAuth would be refused with a real script-less env; under the
    // mock seam it builds the mock instead.
    let (mode, mut p) = build_provider_from_credential(
        "anthropic",
        "mock-model",
        AuthMode::Sso,
        &oauth(),
        None,
        &ProviderConfig::default(),
        sink(),
    )
    .unwrap_or_else(|e| panic!("the mock seam must build: {e}"));
    assert_eq!(mode, AuthMode::ApiKey);
    // It is the mock: it replays the script (no network, no credential).
    assert_eq!(p.default_model(), "mock-1");
    let request = LlmRequest {
        model: "mock-model".into(),
        messages: vec![Message::user("hi")],
        max_tokens: Some(16),
        ..Default::default()
    };
    let reply = p
        .send(&request)
        .await
        .unwrap_or_else(|e| panic!("mock send failed: {e}"));
    assert_eq!(reply.text(), Some("ok"));

    // And a malformed script is the same `Other` error the async builder gives.
    std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", "not json");
    let Err(err) = build_provider_from_credential(
        "anthropic",
        "mock-model",
        AuthMode::ApiKey,
        &AuthCredentials::ApiKey { key: "k".into() },
        None,
        &ProviderConfig::default(),
        sink(),
    ) else {
        panic!("a malformed mock script must error");
    };
    assert!(matches!(err, FactoryError::Other(ref m) if m.contains("mock script")));
}

/// Pins both halves of the bootstrap contract against the same credential:
/// the async builder still performs the `/api/claude_cli/bootstrap` request
/// for Anthropic OAuth (its behavior is unchanged by the sync split), and the
/// sync builder refuses that credential WITHOUT issuing it.
#[tokio::test]
#[serial]
async fn the_async_builder_still_bootstraps_and_the_sync_one_does_not() {
    let _guard = EnvGuard(vec![
        "RUPU_MOCK_PROVIDER_SCRIPT",
        "RUPU_ANTHROPIC_BASE_URL_OVERRIDE",
    ]);
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    let server = MockServer::start();
    let bootstrap = server.mock(|when, then| {
        when.method(GET).path("/api/claude_cli/bootstrap");
        then.status(200);
    });
    std::env::set_var(
        "RUPU_ANTHROPIC_BASE_URL_OVERRIDE",
        format!("{}/v1/messages", server.url("")),
    );

    // Sync: refused, and the endpoint was never touched.
    let Err(err) = build_provider_from_credential(
        "anthropic",
        "claude-sonnet-4-6",
        AuthMode::Sso,
        &oauth(),
        None,
        &ProviderConfig::default(),
        sink(),
    ) else {
        panic!("sync build of Anthropic OAuth must be refused");
    };
    assert!(matches!(err, FactoryError::RequiresAsyncBootstrap { .. }));
    assert_eq!(bootstrap.hits(), 0, "the sync builder must not bootstrap");

    // Async: builds, after one bootstrap request.
    let (mode, p) = build_for_provider_with_config(
        "anthropic",
        "claude-sonnet-4-6",
        None,
        &OAuthResolver,
        &ProviderConfig::default(),
        sink(),
    )
    .await
    .unwrap_or_else(|e| panic!("async build of Anthropic OAuth must succeed: {e}"));
    assert_eq!(mode, AuthMode::Sso);
    assert_eq!(p.provider_id(), rupu_providers::ProviderId::Anthropic);
    assert_eq!(bootstrap.hits(), 1, "the async builder must bootstrap once");
}
