use super::*;
use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use rupu_auth::StoredCredential;
use rupu_providers::AuthMode;
use rupu_scm::{AccountId, Platform};
use serde_json::{json, Value};
use tower::ServiceExt;

struct Fixture {
    home: tempfile::TempDir,
    resolver: KeychainResolver,
    source: Arc<CurrentScm>,
}
impl Fixture {
    fn new() -> Self {
        rupu_scm::install_default_crypto_provider();
        let home = tempfile::tempdir().unwrap();
        let resolver = KeychainResolver::at(home.path().join("auth.json")).without_environment();
        let source = Arc::new(CurrentScm::new(home.path().into(), resolver.clone()));
        Self {
            home,
            resolver,
            source,
        }
    }
    fn config(&self, text: &str) {
        std::fs::write(self.home.path().join("config.toml"), text).unwrap();
    }
    fn account(&self, origin: &str) {
        self.config(&format!(
            "[scm.work]\nkind = 'github'\nbase_url = '{origin}'\n"
        ));
    }
    async fn store(&self, name: &str, value: &str) {
        self.resolver
            .store_named(name, AuthMode::ApiKey, &StoredCredential::api_key(value))
            .await
            .unwrap();
    }
    fn router(&self) -> Router {
        let state = rupu_cp::state::AppState::new(self.home.path().into(), Default::default())
            .with_repos(Some(self.source.clone()));
        // Actual CP handlers, one unchanged AppState; no CP process/listener,
        // production credentials, provider endpoints, or background duties.
        Router::new()
            .merge(rupu_cp::api::repos::routes())
            .merge(rupu_cp::api::issues_read::routes())
            .merge(rupu_cp::api::forge_writes::routes())
            .with_state(state)
    }
}
async fn get(router: &Router, path: &str) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 512 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}
fn epoch(snapshot: &ScmSnapshot) -> String {
    snapshot
        .registry
        .github_extras_by_account(&AccountId::new("work"))
        .unwrap()
        .review_identity_epoch()
}

#[tokio::test]
async fn running_routes_see_login_and_logout_without_replacing_the_app_state() {
    let f = Fixture::new();
    let provider = httpmock::MockServer::start();
    f.account(&provider.base_url());
    let listed = provider.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/user/repos")
            .header("authorization", "Bearer enrollment-fixture-A");
        then.status(200).json_body(json!([]));
    });
    let app = f.router();
    assert_eq!(get(&app, "/api/repos").await, (StatusCode::OK, json!([])));
    assert_eq!(
        get(&app, "/api/issues/accounts").await.1["accounts"],
        json!([])
    );
    assert_eq!(listed.hits(), 0);

    f.store("work", "enrollment-fixture-A").await;
    assert_eq!(get(&app, "/api/repos").await, (StatusCode::OK, json!([])));
    assert_eq!(
        listed.hits(),
        1,
        "the next request reached the newly enrolled connector"
    );
    let (status, accounts) = get(&app, "/api/issues/accounts").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(accounts["accounts"][0]["account"], "work");
    #[cfg(unix)]
    assert_eq!(
        get(&app, "/api/forge/write-capabilities").await.1["capabilities"][0]["account"],
        "work"
    );

    f.resolver
        .forget_named("work", AuthMode::ApiKey)
        .await
        .unwrap();
    assert_eq!(get(&app, "/api/repos").await, (StatusCode::OK, json!([])));
    assert_eq!(
        listed.hits(),
        1,
        "logout does not reuse an old authenticated connector"
    );
    assert_eq!(
        get(&app, "/api/issues/accounts").await.1["accounts"],
        json!([])
    );
    #[cfg(unix)]
    assert_eq!(
        get(&app, "/api/forge/write-capabilities").await.1["capabilities"],
        json!([])
    );
}

#[tokio::test]
async fn unrelated_provider_changes_keep_receipts_but_scm_rotation_refuses_old_preflight() {
    use rupu_scm::forge_write::{ForgeAction, ForgeInput, ForgeRequest, ForgeTarget};
    let f = Fixture::new();
    let provider = httpmock::MockServer::start();
    f.account(&provider.base_url());
    f.store("work", "enrollment-fixture-A").await;
    let repo = provider.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/repos/org/repo")
            .header("authorization", "Bearer enrollment-fixture-A");
        then.status(200).json_body(
            json!({"id":42,"full_name":"org/repo","url":provider.url("/repos/org/repo")}),
        );
    });
    let writes = provider.mock(|when, then| {
        when.method(httpmock::Method::POST);
        then.status(500);
    });
    let first = f.source.current().await.unwrap();
    let request = ForgeRequest {
        action: ForgeAction::IssueCreate,
        target: ForgeTarget {
            provider: rupu_scm::IssueTracker::Github,
            account: "work".into(),
            api_origin: provider.base_url(),
            project_ref: "org/repo".into(),
            number: None,
            ws_id: None,
        },
        expected: Default::default(),
        input: ForgeInput {
            title: Some("Fixture-only preparation".into()),
            ..Default::default()
        },
    };
    let prepared = first
        .repos
        .forge_writer()
        .unwrap()
        .prepare(&request, None)
        .await
        .unwrap();
    f.store("openai", "unrelated-enrollment-fixture").await;
    let same = f.source.current().await.unwrap();
    assert!(Arc::ptr_eq(&first.repos, &same.repos));
    assert_eq!(epoch(&first), epoch(&same));
    let checked = same
        .repos
        .forge_writer()
        .unwrap()
        .preflight(&prepared, None)
        .await
        .unwrap();
    assert!(checked.is_current());
    drop(checked); // Preparation is read-only; this test never dispatches.
    assert_eq!(repo.hits(), 2);

    f.store("work", "enrollment-fixture-B").await;
    let rotated = f.source.current().await.unwrap();
    assert!(!Arc::ptr_eq(&first.repos, &rotated.repos));
    assert_ne!(epoch(&first), epoch(&rotated));
    let error = rotated
        .repos
        .forge_writer()
        .unwrap()
        .preflight(&prepared, None)
        .await
        .err()
        .unwrap();
    assert_eq!(error.code, "selection_changed");
    assert_eq!(
        repo.hits(),
        2,
        "old intent is refused before a provider re-read"
    );
    assert_eq!(writes.hits(), 0);
    assert_eq!(
        first.registry.accounts_for(Platform::Github),
        vec![AccountId::new("work")],
        "captured in-flight snapshot remains owned"
    );
}

#[tokio::test]
async fn config_enrollment_and_instance_scopes_refresh_independently() {
    let a = Fixture::new();
    let b = Fixture::new();
    a.store("work", "fixture-only-A").await;
    b.account("https://github.example.invalid");
    b.store("work", "fixture-only-B").await;
    let before_a = a.source.current().await.unwrap();
    let before_b = b.source.current().await.unwrap();
    assert!(before_a.registry.accounts_for(Platform::Github).is_empty());
    a.account("https://github.example.invalid");
    let after_a = a.source.current().await.unwrap();
    assert_eq!(
        after_a.registry.accounts_for(Platform::Github),
        vec![AccountId::new("work")]
    );
    assert!(Arc::ptr_eq(
        &before_b.repos,
        &b.source.current().await.unwrap().repos
    ));
    a.config("[scm.work]\nkind='github'\nbase_url='https://changed.example.invalid'\n");
    assert_ne!(epoch(&after_a), epoch(&a.source.current().await.unwrap()));
    a.config("");
    assert!(a
        .source
        .current()
        .await
        .unwrap()
        .registry
        .accounts_for(Platform::Github)
        .is_empty());
}

#[tokio::test]
async fn concurrent_refreshes_publish_one_stable_adapter() {
    let f = Fixture::new();
    f.account("https://github.example.invalid");
    let before = f.source.current().await.unwrap();
    f.store("work", "fixture-only").await;
    let snapshots = futures_util::future::join_all((0..12).map(|_| f.source.current())).await;
    let first = snapshots[0].as_ref().unwrap();
    for snapshot in snapshots.iter().map(|value| value.as_ref().unwrap()) {
        assert!(Arc::ptr_eq(&first.repos, &snapshot.repos));
    }
    assert!(!Arc::ptr_eq(&first.repos, &before.repos));
}

#[tokio::test]
async fn malformed_or_oversize_inputs_fail_closed_and_recover_without_stale_fallback() {
    let f = Fixture::new();
    f.account("https://github.example.invalid");
    f.store("work", "fixture-only-do-not-echo").await;
    let before = f.source.current().await.unwrap();
    let app = f.router();
    std::fs::write(
        f.home.path().join("auth.json"),
        "fixture-only-do-not-echo invalid json",
    )
    .unwrap();
    let (status, body) = get(&app, "/api/issues/accounts").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(!body.to_string().contains("do-not-echo"));
    let tools = f.source.for_tools().await.unwrap();
    assert!(!tools.available);
    assert!(
        tools.registry.accounts_for(Platform::Github).is_empty(),
        "non-SCM MCP context never falls back to the previous authenticated registry"
    );
    assert!(!Arc::ptr_eq(&before.repos, &tools.repos));
    std::fs::remove_file(f.home.path().join("auth.json")).unwrap();
    let after = f.source.current().await.unwrap();
    assert!(!Arc::ptr_eq(&before.repos, &after.repos));
    assert!(after.registry.accounts_for(Platform::Github).is_empty());
    f.config("[scm.work\nsecret = 'do-not-echo'");
    assert!(f.source.current().await.is_err());
    f.config(&"#".repeat(2 * 1024 * 1024 + 1));
    assert!(f.source.current().await.is_err());
    f.config("");
    assert!(f.source.current().await.is_ok());
}

#[tokio::test]
async fn invalid_scm_url_is_an_error_without_unwinding_or_using_previous_credentials() {
    let f = Fixture::new();
    f.account("https://github.example.invalid");
    f.store("work", "fixture-only").await;
    let before = f.source.current().await.unwrap();
    f.account("not a URL");
    assert!(f.source.current().await.is_err());
    f.account("https://credential:fixture-secret@example.invalid");
    let error = f.source.current().await.err().unwrap().to_string();
    assert!(!error.contains("fixture-secret"));
    f.account("https://github.example.invalid");
    assert!(Arc::ptr_eq(
        &before.repos,
        &f.source.current().await.unwrap().repos
    ));
}

#[tokio::test]
async fn project_only_account_is_refreshed_without_global_account_configuration() {
    let f = Fixture::new();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".rupu")).unwrap();
    std::fs::write(
        project.path().join(".rupu/config.toml"),
        "[scm.work]\nkind='github'\nbase_url='https://github.example.invalid'\n",
    )
    .unwrap();
    let source = f.source.project_source(project.path().into());
    let before = source.current().await.unwrap();
    assert!(before.registry.accounts_for(Platform::Github).is_empty());
    f.store("work", "project-fixture-only").await;
    let after = source.current().await.unwrap();
    assert_eq!(
        after.registry.accounts_for(Platform::Github),
        vec![AccountId::new("work")]
    );
    assert!(f
        .source
        .current()
        .await
        .unwrap()
        .registry
        .accounts_for(Platform::Github)
        .is_empty());
    assert!(!Arc::ptr_eq(&before.repos, &after.repos));
}

#[tokio::test]
async fn sso_precedence_and_gitlab_enrollment_use_the_same_revision_rules_as_resolution() {
    let f = Fixture::new();
    f.account("https://github.example.invalid");
    let sso = rupu_auth::StoredCredential {
        credentials: rupu_providers::auth::AuthCredentials::OAuth {
            access: "sso-fixture-only".into(),
            refresh: String::new(),
            expires: 0,
            extra: Default::default(),
        },
        refresh_token: None,
        expires_at: None,
    };
    f.resolver
        .store_named("work", AuthMode::Sso, &sso)
        .await
        .unwrap();
    let first = f.source.current().await.unwrap();
    f.store("work", "shadowed-api-key-fixture").await;
    assert!(Arc::ptr_eq(
        &first.repos,
        &f.source.current().await.unwrap().repos
    ));
    f.resolver
        .forget_named("work", AuthMode::Sso)
        .await
        .unwrap();
    let fallback = f.source.current().await.unwrap();
    assert_ne!(epoch(&first), epoch(&fallback));
    assert_eq!(
        fallback.registry.accounts_for(Platform::Github),
        vec![AccountId::new("work")]
    );
    f.store("gitlab", "gitlab-fixture-only").await;
    let gitlab = f.source.current().await.unwrap();
    assert_eq!(
        gitlab.registry.accounts_for(Platform::Gitlab),
        vec![AccountId::new("gitlab")]
    );
    f.resolver
        .forget_named("gitlab", AuthMode::ApiKey)
        .await
        .unwrap();
    assert!(f
        .source
        .current()
        .await
        .unwrap()
        .registry
        .accounts_for(Platform::Gitlab)
        .is_empty());
}

#[tokio::test]
async fn transient_enrolled_resolution_failure_is_not_a_cacheable_empty_registry() {
    use rupu_auth::CredentialResolver;
    struct Flaky {
        fail: std::sync::atomic::AtomicBool,
    }
    #[async_trait::async_trait]
    impl CredentialResolver for Flaky {
        async fn get(
            &self,
            name: &str,
            _: Option<AuthMode>,
        ) -> anyhow::Result<(AuthMode, rupu_providers::auth::AuthCredentials)> {
            if name != "github" || self.fail.load(std::sync::atomic::Ordering::Relaxed) {
                anyhow::bail!("fixture transient failure");
            }
            Ok((
                AuthMode::ApiKey,
                rupu_providers::auth::AuthCredentials::ApiKey {
                    key: "fixture-only".into(),
                },
            ))
        }
        async fn refresh(
            &self,
            _: &str,
            _: AuthMode,
        ) -> anyhow::Result<rupu_providers::auth::AuthCredentials> {
            anyhow::bail!("fixture has no refresh endpoint")
        }
    }
    rupu_scm::install_default_crypto_provider();
    let resolver = Flaky {
        fail: std::sync::atomic::AtomicBool::new(true),
    };
    let config = rupu_config::Config::default();
    let fixture = Fixture::new();
    fixture.store("github", "fixture-only").await;
    let expected = fixture
        .resolver
        .enrollment_revision(&[("github".into(), None)])
        .unwrap();
    assert!(discover_complete(&resolver, &config, &expected)
        .await
        .is_err());
    resolver
        .fail
        .store(false, std::sync::atomic::Ordering::Relaxed);
    let recovered = discover_complete(&resolver, &config, &expected)
        .await
        .unwrap();
    assert_eq!(
        recovered.accounts_for(Platform::Github),
        vec![AccountId::new("github")]
    );
    fixture.store("github", "different-captured-fixture").await;
    let changed = fixture
        .resolver
        .enrollment_revision(&[("github".into(), None)])
        .unwrap();
    assert!(
        discover_complete(&resolver, &config, &changed)
            .await
            .is_err(),
        "a resolver result from another publication cannot enter this snapshot"
    );
    fixture
        .resolver
        .forget_named("github", AuthMode::ApiKey)
        .await
        .unwrap();
    let absent = fixture
        .resolver
        .enrollment_revision(&[("github".into(), None)])
        .unwrap();
    assert!(
        discover_complete(&resolver, &config, &absent)
            .await
            .unwrap()
            .accounts_for(Platform::Github)
            .is_empty(),
        "an uncaptured transient account is never adopted"
    );
}

#[tokio::test]
async fn effective_account_vendor_changes_refresh_but_unrelated_config_does_not() {
    let f = Fixture::new();
    f.config("[providers.work]\nkind='github'\n[scm.work]\nkind='github'\n");
    f.store("work", "fixture-only").await;
    let first = f.source.current().await.unwrap();
    f.config("[providers.work]\nkind='github'\n[providers.other]\nkind='openai'\n[scm.work]\nkind='github'\n");
    assert!(Arc::ptr_eq(
        &first.repos,
        &f.source.current().await.unwrap().repos
    ));
    f.config("[providers.work]\nkind='gitlab'\n[scm.work]\nkind='github'\n");
    assert_ne!(epoch(&first), epoch(&f.source.current().await.unwrap()));
}

#[tokio::test]
async fn enrollment_equality_preserves_epochs_but_publication_guard_detects_aba() {
    let f = Fixture::new();
    let selected = vec![("github".to_string(), None)];
    f.store("github", "fixture-A").await;
    let before = f.resolver.enrollment_revision(&selected).unwrap();
    f.store("github", "fixture-B").await;
    f.store("github", "fixture-A").await;
    let after = f.resolver.enrollment_revision(&selected).unwrap();
    assert!(before == after, "effective enrollment is restored");
    assert!(
        !before.same_publication(&after),
        "a discovery crossing these publications must retry"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn fifo_credential_and_config_paths_are_refused_without_a_writer() {
    let f = Fixture::new();
    let auth = f.home.path().join("auth.json");
    assert!(std::process::Command::new("mkfifo")
        .args(["-m", "600"])
        .arg(&auth)
        .status()
        .expect("mkfifo must be available for FIFO regression coverage")
        .success());
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), f.source.current()).await;
    if result.is_err() {
        // Unblock a regressed reader before runtime shutdown.
        drop(rustix::fs::open(
            &auth,
            rustix::fs::OFlags::RDWR | rustix::fs::OFlags::NONBLOCK,
            rustix::fs::Mode::empty(),
        ));
    }
    assert!(result.is_ok_and(|value| value.is_err()));
    std::fs::remove_file(auth).unwrap();
    assert!(std::process::Command::new("mkfifo")
        .args(["-m", "600"])
        .arg(f.home.path().join("config.toml"))
        .status()
        .expect("mkfifo must be available for FIFO regression coverage")
        .success());
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), f.source.current()).await;
    if result.is_err() {
        drop(rustix::fs::open(
            f.home.path().join("config.toml"),
            rustix::fs::OFlags::RDWR | rustix::fs::OFlags::NONBLOCK,
            rustix::fs::Mode::empty(),
        ));
    }
    assert!(result.is_ok_and(|value| value.is_err()));
}
