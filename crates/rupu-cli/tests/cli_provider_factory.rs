use rupu_auth::backend::ProviderId;
use rupu_auth::in_memory::InMemoryResolver;
use rupu_auth::stored::StoredCredential;
use rupu_providers::AuthMode;
use rupu_runtime::provider_factory::build_for_provider;

/// Test-only seam consumed by `build_anthropic` to redirect the Anthropic
/// Messages endpoint at an httpmock server. Mirrors the
/// `RUPU_OAUTH_TOKEN_URL_OVERRIDE` seam used by the resolver.
const ANTHROPIC_BASE_URL_OVERRIDE: &str = "RUPU_ANTHROPIC_BASE_URL_OVERRIDE";

#[tokio::test]
async fn anthropic_factory_requires_credential() {
    // No stored secret. Should fail with a clear missing-credential message.
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    let resolver = InMemoryResolver::new();
    let res = build_for_provider("anthropic", "claude-sonnet-4-6", None, &resolver).await;
    let err = format!("{}", res.err().expect("expected Err"));
    assert!(
        err.contains("anthropic") || err.contains("credential"),
        "expected clear missing-credential error, got: {err}"
    );
}

#[tokio::test]
async fn unknown_provider_errors_clearly() {
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    let resolver = InMemoryResolver::new();
    let res = build_for_provider("teleport", "model-x", None, &resolver).await;
    let err = format!("{}", res.err().expect("expected Err"));
    assert!(
        err.contains("teleport"),
        "expected provider name in error: {err}"
    );
}

#[tokio::test]
async fn deferred_provider_returns_blocked_error() {
    // openai and copilot are wired (fail with MissingCredential when no key
    // configured); gemini and local remain NotWiredInV0 in this slice.
    // Both shapes produce a clear, named error rather than a silent miss.
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    let resolver = InMemoryResolver::new();
    // No credentials inserted — resolver returns "no credentials configured".

    // openai + copilot: wired, should fail with MissingCredential when no key.
    for p in ["openai", "copilot"] {
        let res = build_for_provider(p, "x", None, &resolver).await;
        let err = format!("{}", res.err().expect("expected Err"));
        assert!(
            err.contains(p) && err.contains("missing credential"),
            "{p}: expected missing-credential error: {err}"
        );
    }

    // Insert a dummy credential for `local` so we test its NotWiredInV0 path
    // rather than the resolver-side missing-credential path. Gemini was wired
    // (TODO.md: "Gemini API-key support via AI Studio ✅ shipped") so it now
    // takes the credential happily — assertion-set narrowed to `local` only.
    resolver
        .put(
            ProviderId::Local,
            AuthMode::ApiKey,
            StoredCredential::api_key("dummy"),
        )
        .await;

    for p in ["local"] {
        let res = build_for_provider(p, "x", None, &resolver).await;
        let err = format!("{}", res.err().expect("expected Err"));
        assert!(
            err.contains(p) && (err.contains("not wired") || err.contains("v0")),
            "{p}: expected v0-deferral error: {err}"
        );
    }
}

/// Regression: the suggestion command in `MissingCredential`'s error message
/// used to embed the resolver error inside the `{provider}` field, producing
/// `configure with \`rupu auth login --provider anthropic: expected value at
/// line 1 column 1\`` — copy-paste-broken advice. The resolver error now
/// lives in a separate `source` field and the suggestion stays clean.
#[tokio::test]
async fn missing_credential_error_keeps_suggestion_command_clean() {
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    let resolver = InMemoryResolver::new();
    let res = build_for_provider("anthropic", "claude-sonnet-4-6", None, &resolver).await;
    let err = format!("{}", res.err().expect("expected Err"));
    assert!(
        err.contains("`rupu auth login --provider anthropic`"),
        "suggestion command should be clean: {err}"
    );
}

/// Regression test for the SSO bug where an OAuth credential resolved
/// for Anthropic was being shipped via the `x-api-key` header instead
/// of `Authorization: Bearer …`. The httpmock matcher fires only when
/// the bearer header is present, so a regression results in 404 →
/// `send()` returns `Err`, and `assert_hits(1)` fails.
#[tokio::test]
async fn anthropic_factory_oauth_credential_uses_bearer_not_x_api_key() {
    use httpmock::prelude::*;
    use rupu_providers::types::{LlmRequest, Message};

    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");

    let server = MockServer::start();
    // Match the wire-level shape we want OAuth requests to take:
    // Bearer auth + a `User-Agent` rupu identifies itself with + a body
    // carrying the OAuth `betas` array. If any of these regress the
    // matcher won't fire and `assert_hits(1)` below will fail.
    let mock = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .header("Authorization", "Bearer test-access-token")
            .matches(|req| {
                let ua_ok = req
                    .headers
                    .as_ref()
                    .map(|hs| {
                        hs.iter().any(|(k, v)| {
                            k.eq_ignore_ascii_case("User-Agent") && v.starts_with("claude-cli/")
                        })
                    })
                    .unwrap_or(false);
                let body_ok = req
                    .body
                    .as_ref()
                    .map(|b| {
                        let s = String::from_utf8_lossy(b);
                        // `betas` must NOT appear in the body — it is a
                        // header-only field; including it 400s.
                        s.contains("\"metadata\":") && !s.contains("\"betas\":")
                    })
                    .unwrap_or(false);
                ua_ok && body_ok
            });
        then.status(200)
            .header("content-type", "application/json")
            .json_body(serde_json::json!({
                "id": "msg_test",
                "type": "message",
                "role": "assistant",
                "model": "claude-sonnet-4-6",
                "content": [{"type": "text", "text": "hi"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 1, "output_tokens": 1}
            }));
    });

    std::env::set_var(
        ANTHROPIC_BASE_URL_OVERRIDE,
        format!("{}/v1/messages", server.url("")),
    );

    let resolver = InMemoryResolver::new();
    let stored = StoredCredential {
        credentials: rupu_providers::auth::AuthCredentials::OAuth {
            access: "test-access-token".into(),
            refresh: "test-refresh-token".into(),
            // 0 == non-expiring per `is_token_expired`; prevents the
            // client from kicking off a real refresh round-trip.
            expires: 0,
            extra: Default::default(),
        },
        refresh_token: Some("test-refresh-token".into()),
        expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
    };
    resolver
        .put(ProviderId::Anthropic, AuthMode::Sso, stored)
        .await;

    let (mode, mut provider) = build_for_provider(
        "anthropic",
        "claude-sonnet-4-6",
        Some(AuthMode::Sso),
        &resolver,
    )
    .await
    .expect("build_for_provider should succeed with OAuth credential");
    assert_eq!(mode, AuthMode::Sso);

    let request = LlmRequest {
        model: "claude-sonnet-4-6".into(),
        system: None,
        messages: vec![Message::user("hi")],
        max_tokens: 16,
        tools: vec![],
        cell_id: None,
        trace_id: None,
        thinking: None,
        context_window: None,
        task_type: None,
        output_format: None,
        anthropic_task_budget: None,
        anthropic_context_management: None,
        anthropic_speed: None,
    };

    let result = provider.send(&request).await;

    std::env::remove_var(ANTHROPIC_BASE_URL_OVERRIDE);

    mock.assert_hits(1);
    assert!(
        result.is_ok(),
        "send() should succeed when factory emits Bearer auth: {:?}",
        result.err()
    );
}
