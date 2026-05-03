use rupu_auth::backend::ProviderId;
use rupu_auth::in_memory::InMemoryResolver;
use rupu_auth::resolver::CredentialResolver;
use rupu_auth::stored::StoredCredential;
use rupu_providers::AuthMode;

#[tokio::test]
async fn sso_wins_when_both_present_and_no_hint() {
    let r = InMemoryResolver::new();
    r.put(
        ProviderId::Anthropic,
        AuthMode::ApiKey,
        StoredCredential::api_key("sk-test"),
    )
    .await;
    r.put(
        ProviderId::Anthropic,
        AuthMode::Sso,
        StoredCredential {
            credentials: rupu_providers::auth::AuthCredentials::OAuth {
                access: "tok".into(),
                refresh: "rt".into(),
                expires: 0,
                extra: Default::default(),
            },
            refresh_token: Some("rt".into()),
            expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
        },
    )
    .await;
    let (mode, _) = r.get("anthropic", None).await.unwrap();
    assert_eq!(mode, AuthMode::Sso);
}

#[tokio::test]
async fn api_key_used_when_only_api_key_present() {
    let r = InMemoryResolver::new();
    r.put(
        ProviderId::Openai,
        AuthMode::ApiKey,
        StoredCredential::api_key("sk-test"),
    )
    .await;
    let (mode, _) = r.get("openai", None).await.unwrap();
    assert_eq!(mode, AuthMode::ApiKey);
}

#[tokio::test]
async fn explicit_hint_overrides_precedence() {
    let r = InMemoryResolver::new();
    r.put(
        ProviderId::Openai,
        AuthMode::ApiKey,
        StoredCredential::api_key("sk-test"),
    )
    .await;
    r.put(
        ProviderId::Openai,
        AuthMode::Sso,
        StoredCredential {
            credentials: rupu_providers::auth::AuthCredentials::OAuth {
                access: "tok".into(),
                refresh: "rt".into(),
                expires: 0,
                extra: Default::default(),
            },
            refresh_token: Some("rt".into()),
            expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
        },
    )
    .await;
    let (mode, _) = r.get("openai", Some(AuthMode::ApiKey)).await.unwrap();
    assert_eq!(mode, AuthMode::ApiKey);
}

#[tokio::test]
async fn missing_credential_errors() {
    let r = InMemoryResolver::new();
    let result = r.get("gemini", None).await;
    assert!(result.is_err());
}
