//! `KeychainResolver::refresh_inner` is the one migrated site with no
//! existing runtime coverage: `resolver_refresh.rs` only drives
//! `InMemoryResolver`, which never reaches it. This is the
//! credential-carrying request — a live OAuth refresh-token grant — so
//! it is exactly the site an operator most wants proven to go through
//! the instrumented client, not just proven to type-check.
//!
//! Single test, own binary: cargo runs each `tests/*.rs` file as a
//! separate process, so the env vars set here (`RUPU_AUTH_FILE`,
//! `RUPU_OAUTH_TOKEN_URL_OVERRIDE`) cannot race another test's.

use rupu_auth::resolver::{CredentialResolver, KeychainResolver};
use rupu_auth::stored::StoredCredential;
use rupu_auth::ProviderId;
use rupu_netflow::{MemorySink, Origin};
use rupu_providers::auth::AuthCredentials;
use rupu_providers::AuthMode;
use std::sync::Arc;

#[tokio::test]
async fn keychain_refresh_records_flow_under_system_origin() {
    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/oauth/token");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"access_token":"new-access","refresh_token":"new-refresh","expires_in":3600}"#);
        })
        .await;

    let sink = Arc::new(MemorySink::default());
    rupu_netflow::http::init(sink.clone());

    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("RUPU_AUTH_FILE", dir.path().join("auth.json"));
    std::env::set_var("RUPU_OAUTH_TOKEN_URL_OVERRIDE", server.url("/oauth/token"));

    let resolver = KeychainResolver::with_service("rupu");
    resolver
        .store(
            ProviderId::Anthropic,
            AuthMode::Sso,
            &StoredCredential {
                credentials: AuthCredentials::OAuth {
                    access: "old-access".into(),
                    refresh: "old-refresh".into(),
                    expires: 0,
                    extra: Default::default(),
                },
                refresh_token: Some("old-refresh".into()),
                expires_at: Some(chrono::Utc::now() - chrono::Duration::seconds(1)),
            },
        )
        .await
        .unwrap();

    let refreshed = resolver.refresh("anthropic", AuthMode::Sso).await.unwrap();
    match refreshed {
        AuthCredentials::OAuth { access, .. } => assert_eq!(access, "new-access"),
        _ => panic!("expected OAuth credentials"),
    }

    std::env::remove_var("RUPU_AUTH_FILE");
    std::env::remove_var("RUPU_OAUTH_TOKEN_URL_OVERRIDE");

    let records = sink.records();
    assert_eq!(
        records.len(),
        1,
        "refresh_inner should have gone through the instrumented client exactly once"
    );
    assert_eq!(records[0].path, "/oauth/token");
    assert_eq!(records[0].ctx.origin, Origin::System);
}
