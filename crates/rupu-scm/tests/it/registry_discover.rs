use rupu_scm::{IssueTracker, Platform, Registry, RepoRef};

#[tokio::test]
async fn empty_resolver_yields_no_connectors() {
    rupu_scm::install_default_crypto_provider();
    use rupu_auth::in_memory::InMemoryResolver;
    let resolver = InMemoryResolver::new();
    let cfg = rupu_config::Config::default();
    let r = Registry::discover(&resolver, &cfg, std::sync::Arc::new(rupu_netflow::NullSink)).await;
    // `repo()`/`issues()` (the account-arbitrary shims) were deleted in
    // Arc 2 Task 6 review — asserting through the real call paths
    // (`repo_for`/`issues_for`) instead of the internals they used to
    // wrap.
    assert!(r.repo_for(&github_repo_ref(), None, None).is_err());
    assert!(r.repo_for(&gitlab_repo_ref(), None, None).is_err());
    assert!(r
        .issues_for(IssueTracker::Github, None, None, None)
        .is_err());
    assert!(r
        .issues_for(IssueTracker::Gitlab, None, None, None)
        .is_err());
}

/// Placeholder `RepoRef`s for the existence-only assertions below —
/// no `[[scm.rules]]` are configured in these tests, so the sole-account
/// tier (or `NoAccounts`) resolves regardless of `owner`/`repo`.
fn github_repo_ref() -> RepoRef {
    RepoRef {
        platform: Platform::Github,
        owner: "any".into(),
        repo: "any".into(),
    }
}

fn gitlab_repo_ref() -> RepoRef {
    RepoRef {
        platform: Platform::Gitlab,
        owner: "any".into(),
        repo: "any".into(),
    }
}

#[tokio::test]
async fn github_connector_built_when_credential_present() {
    rupu_scm::install_default_crypto_provider();
    use rupu_auth::backend::ProviderId;
    use rupu_auth::in_memory::InMemoryResolver;
    use rupu_auth::stored::StoredCredential;
    use rupu_providers::AuthMode;

    let resolver = InMemoryResolver::new();
    resolver
        .put(
            ProviderId::Github,
            AuthMode::ApiKey,
            StoredCredential::api_key("ghp_test"),
        )
        .await;
    let cfg = rupu_config::Config::default();
    let r = Registry::discover(&resolver, &cfg, std::sync::Arc::new(rupu_netflow::NullSink)).await;
    assert!(r.repo_for(&github_repo_ref(), None, None).is_ok());
    assert!(r.issues_for(IssueTracker::Github, None, None, None).is_ok());
    assert!(r.github_extras().is_some());
    // Without GitLab credential, gitlab extras should be None.
    assert!(r.gitlab_extras().is_none());
}

#[tokio::test]
async fn gitlab_connector_built_when_credential_present() {
    rupu_scm::install_default_crypto_provider();
    use rupu_auth::backend::ProviderId;
    use rupu_auth::in_memory::InMemoryResolver;
    use rupu_auth::stored::StoredCredential;
    use rupu_providers::AuthMode;

    let resolver = InMemoryResolver::new();
    resolver
        .put(
            ProviderId::Gitlab,
            AuthMode::ApiKey,
            StoredCredential::api_key("glpat_test"),
        )
        .await;
    let cfg = rupu_config::Config::default();
    let r = Registry::discover(&resolver, &cfg, std::sync::Arc::new(rupu_netflow::NullSink)).await;
    assert!(r.repo_for(&gitlab_repo_ref(), None, None).is_ok());
    assert!(r.issues_for(IssueTracker::Gitlab, None, None, None).is_ok());
    assert!(r.gitlab_extras().is_some());
    // Without GitHub credential, github extras should be None.
    assert!(r.github_extras().is_none());
}

#[tokio::test]
async fn linear_event_connector_built_when_credential_present() {
    rupu_scm::install_default_crypto_provider();
    use rupu_auth::backend::ProviderId;
    use rupu_auth::in_memory::InMemoryResolver;
    use rupu_auth::stored::StoredCredential;
    use rupu_providers::AuthMode;
    use rupu_scm::EventSourceRef;

    let resolver = InMemoryResolver::new();
    resolver
        .put(
            ProviderId::Linear,
            AuthMode::ApiKey,
            StoredCredential::api_key("lin_api_test"),
        )
        .await;
    let cfg = rupu_config::Config::default();
    let r = Registry::discover(&resolver, &cfg, std::sync::Arc::new(rupu_netflow::NullSink)).await;
    assert!(r.issues_for(IssueTracker::Linear, None, None, None).is_ok());
    assert!(r
        .events_for_source(
            &EventSourceRef::TrackerProject {
                tracker: IssueTracker::Linear,
                project: "team-123".into(),
                account: None,
            },
            None,
            None,
        )
        .is_ok());
}

#[tokio::test]
async fn jira_event_connector_built_when_credential_present() {
    rupu_scm::install_default_crypto_provider();
    use rupu_auth::backend::ProviderId;
    use rupu_auth::in_memory::InMemoryResolver;
    use rupu_auth::stored::StoredCredential;
    use rupu_providers::AuthMode;
    use rupu_scm::EventSourceRef;

    let resolver = InMemoryResolver::new();
    resolver
        .put(
            ProviderId::Jira,
            AuthMode::ApiKey,
            StoredCredential::api_key("matt@example.com:api-token"),
        )
        .await;
    let mut cfg = rupu_config::Config::default();
    cfg.scm.platforms.insert(
        "jira".into(),
        rupu_config::ScmPlatformConfig {
            base_url: Some("https://acme.atlassian.net".into()),
            ..Default::default()
        },
    );
    let r = Registry::discover(&resolver, &cfg, std::sync::Arc::new(rupu_netflow::NullSink)).await;
    assert!(r.issues_for(IssueTracker::Jira, None, None, None).is_ok());
    assert!(r
        .events_for_source(
            &EventSourceRef::TrackerProject {
                tracker: IssueTracker::Jira,
                project: "ENG".into(),
                account: None,
            },
            None,
            None,
        )
        .is_ok());
}
