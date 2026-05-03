use rupu_auth::JsonFileBackend;
use rupu_cli::provider_factory::build_for_provider;

fn fresh_backend() -> JsonFileBackend {
    let tmp = assert_fs::TempDir::new().unwrap();
    JsonFileBackend {
        path: tmp.path().join("auth.json"),
    }
}

#[tokio::test]
async fn anthropic_factory_requires_credential() {
    // No stored secret; no env var. Should fail with a clear message.
    // Defensively unset the env var since tests share process state.
    std::env::remove_var("ANTHROPIC_API_KEY");
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    let backend = fresh_backend();
    let res = build_for_provider("anthropic", "claude-sonnet-4-6", &backend).await;
    let err = format!("{}", res.err().expect("expected Err"));
    assert!(
        err.contains("anthropic") || err.contains("credential"),
        "expected clear missing-credential error, got: {err}"
    );
}

#[tokio::test]
async fn unknown_provider_errors_clearly() {
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    let backend = fresh_backend();
    let res = build_for_provider("teleport", "model-x", &backend).await;
    let err = format!("{}", res.err().expect("expected Err"));
    assert!(
        err.contains("teleport"),
        "expected provider name in error: {err}"
    );
}

#[tokio::test]
async fn deferred_provider_returns_blocked_error() {
    // After Slice B-1 Plan 1 Tasks 12-14: openai and copilot are wired
    // (fall through to MissingCredential when no key configured), while
    // gemini and local remain NotWiredInV0 in this slice. Both shapes
    // produce a clear, named error rather than a silent miss.
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    // Clear env-var fallbacks so we exercise the missing-credential path
    // for the wired providers instead of accidentally succeeding.
    let prev_openai = std::env::var("OPENAI_API_KEY").ok();
    let prev_github = std::env::var("GITHUB_TOKEN").ok();
    std::env::remove_var("OPENAI_API_KEY");
    std::env::remove_var("GITHUB_TOKEN");

    let backend = fresh_backend();

    // openai + copilot: wired, should fail with MissingCredential when no key.
    for p in ["openai", "copilot"] {
        let res = build_for_provider(p, "x", &backend).await;
        let err = format!("{}", res.err().expect("expected Err"));
        assert!(
            err.contains(p) && err.contains("missing credential"),
            "{p}: expected missing-credential error: {err}"
        );
    }

    // gemini + local: still NotWiredInV0 in Plan 1.
    for p in ["gemini", "local"] {
        let res = build_for_provider(p, "x", &backend).await;
        let err = format!("{}", res.err().expect("expected Err"));
        assert!(
            err.contains(p) && (err.contains("not wired") || err.contains("v0")),
            "{p}: expected v0-deferral error: {err}"
        );
    }

    if let Some(v) = prev_openai {
        std::env::set_var("OPENAI_API_KEY", v);
    }
    if let Some(v) = prev_github {
        std::env::set_var("GITHUB_TOKEN", v);
    }
}
