use rupu_scm::{IssueTracker, Platform, Registry};

#[tokio::test]
async fn empty_resolver_yields_no_connectors() {
    use rupu_auth::in_memory::InMemoryResolver;
    let resolver = InMemoryResolver::new();
    let cfg = rupu_config::Config::default();
    let r = Registry::discover(&resolver, &cfg).await;
    assert!(r.repo(Platform::Github).is_none());
    assert!(r.repo(Platform::Gitlab).is_none());
    assert!(r.issues(IssueTracker::Github).is_none());
    assert!(r.issues(IssueTracker::Gitlab).is_none());
}

#[tokio::test]
async fn github_connector_built_when_credential_present() {
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
    let r = Registry::discover(&resolver, &cfg).await;
    assert!(r.repo(Platform::Github).is_some());
    assert!(r.issues(IssueTracker::Github).is_some());
}
