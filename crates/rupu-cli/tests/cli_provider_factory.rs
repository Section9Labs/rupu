use rupu_cli::provider_factory::build_for_provider;

#[tokio::test]
async fn anthropic_factory_requires_credential() {
    // No auth.json; no env var. Should fail with a clear message.
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let res = build_for_provider("anthropic", "claude-sonnet-4-6", &auth_path).await;
    let err = format!("{}", res.err().expect("expected Err"));
    assert!(
        err.contains("anthropic") || err.contains("credential"),
        "expected clear missing-credential error, got: {err}"
    );
}

#[tokio::test]
async fn unknown_provider_errors_clearly() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let res = build_for_provider("teleport", "model-x", &auth_path).await;
    let err = format!("{}", res.err().expect("expected Err"));
    assert!(
        err.contains("teleport"),
        "expected provider name in error: {err}"
    );
}

#[tokio::test]
async fn deferred_provider_returns_blocked_error() {
    // openai/copilot/gemini/local are defined types but v0 wires only
    // anthropic. Expect a clear "not wired in v0" error.
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    for p in ["openai", "copilot", "gemini", "local"] {
        let res = build_for_provider(p, "x", &auth_path).await;
        let err = format!("{}", res.err().expect("expected Err"));
        assert!(
            err.contains(p) && (err.contains("not wired") || err.contains("v0")),
            "{p}: expected v0-deferral error: {err}"
        );
    }
}
