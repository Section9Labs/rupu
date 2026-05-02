//! These tests touch the real OS keychain; run with:
//!   cargo test -p rupu-auth -- --ignored

use rupu_auth::{AuthBackend, KeyringBackend, ProviderId};

#[test]
#[ignore]
fn real_keyring_round_trip() {
    if KeyringBackend::probe().is_err() {
        eprintln!("skipping: keyring not available");
        return;
    }
    let b = KeyringBackend::new();
    b.store(ProviderId::Anthropic, "test-secret-zzz").unwrap();
    let got = b.retrieve(ProviderId::Anthropic).unwrap();
    assert_eq!(got, "test-secret-zzz");
    b.forget(ProviderId::Anthropic).unwrap();
}
