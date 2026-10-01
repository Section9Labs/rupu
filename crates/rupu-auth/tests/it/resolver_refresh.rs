use rupu_auth::backend::ProviderId;
use rupu_auth::in_memory::InMemoryResolver;
use rupu_auth::resolver::CredentialResolver;
use rupu_auth::stored::StoredCredential;
use rupu_providers::AuthMode;

#[tokio::test]
async fn near_expiry_triggers_refresh_callback() {
    let r = InMemoryResolver::new();
    let near = chrono::Utc::now() + chrono::Duration::seconds(10);
    r.put(
        ProviderId::Anthropic,
        AuthMode::Sso,
        StoredCredential {
            credentials: rupu_providers::auth::AuthCredentials::OAuth {
                access: "old".into(),
                refresh: "rt".into(),
                expires: 0,
                extra: Default::default(),
            },
            refresh_token: Some("rt".into()),
            expires_at: Some(near),
        },
    )
    .await;
    r.set_refresh_callback(|_p, mode, sc| {
        assert_eq!(mode, AuthMode::Sso);
        assert!(sc.refresh_token.is_some());
        Ok(StoredCredential {
            credentials: rupu_providers::auth::AuthCredentials::OAuth {
                access: "new".into(),
                refresh: "rt".into(),
                expires: 0,
                extra: Default::default(),
            },
            refresh_token: Some("rt".into()),
            expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
        })
    })
    .await;
    let (mode, creds) = r.get("anthropic", None).await.unwrap();
    assert_eq!(mode, AuthMode::Sso);
    match creds {
        rupu_providers::auth::AuthCredentials::OAuth { access, .. } => {
            assert_eq!(
                access, "new",
                "refresh should have replaced the access token"
            );
        }
        _ => panic!(),
    }
}
