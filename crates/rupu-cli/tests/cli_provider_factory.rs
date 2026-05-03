use rupu_auth::backend::ProviderId;
use rupu_auth::in_memory::InMemoryResolver;
use rupu_auth::stored::StoredCredential;
use rupu_cli::provider_factory::build_for_provider;
use rupu_providers::AuthMode;

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

    // Insert dummy credentials for gemini and local so we test their
    // NotWiredInV0 path rather than the resolver-side missing-credential path.
    resolver
        .put(
            ProviderId::Gemini,
            AuthMode::ApiKey,
            StoredCredential::api_key("dummy"),
        )
        .await;
    resolver
        .put(
            ProviderId::Local,
            AuthMode::ApiKey,
            StoredCredential::api_key("dummy"),
        )
        .await;

    for p in ["gemini", "local"] {
        let res = build_for_provider(p, "x", None, &resolver).await;
        let err = format!("{}", res.err().expect("expected Err"));
        assert!(
            err.contains(p) && (err.contains("not wired") || err.contains("v0")),
            "{p}: expected v0-deferral error: {err}"
        );
    }
}
