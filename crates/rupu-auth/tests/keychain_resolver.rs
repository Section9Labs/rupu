//! Real-keyring round-trip. Verifies the spec §13 risk: "no mock
//! features" — the test shells out to `security` on macOS to confirm
//! data actually persisted.

use rupu_auth::backend::ProviderId;
use rupu_auth::resolver::{CredentialResolver, KeychainResolver};
use rupu_auth::stored::StoredCredential;
use rupu_providers::AuthMode;

#[tokio::test]
async fn keychain_resolver_roundtrip_api_key() {
    // Use a unique service name so parallel test runs don't collide.
    let unique = format!("rupu-test-{}", uuid_like());
    let r = KeychainResolver::with_service(&unique);
    r.store(
        ProviderId::Anthropic,
        AuthMode::ApiKey,
        &StoredCredential::api_key("sk-roundtrip"),
    )
    .await
    .expect("store");
    let (mode, creds) = r
        .get("anthropic", Some(AuthMode::ApiKey))
        .await
        .expect("get");
    assert_eq!(mode, AuthMode::ApiKey);
    match creds {
        rupu_providers::auth::AuthCredentials::ApiKey { key } => assert_eq!(key, "sk-roundtrip"),
        _ => panic!(),
    }

    // On macOS, confirm with `security find-generic-password`.
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("security")
            .args([
                "find-generic-password",
                "-s",
                &unique,
                "-a",
                "anthropic/api-key",
            ])
            .output()
            .expect("run security");
        assert!(out.status.success(), "security exited non-zero");
    }

    r.forget(ProviderId::Anthropic, AuthMode::ApiKey)
        .await
        .expect("forget");
    assert!(r.get("anthropic", Some(AuthMode::ApiKey)).await.is_err());
}

fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
        .to_string()
}
