#![cfg(unix)]

use assert_fs::prelude::*;
use rupu_auth::{AuthBackend, JsonFileBackend, ProviderId};
use std::os::unix::fs::PermissionsExt;

#[test]
fn store_and_retrieve_round_trip() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let path = tmp.child("auth.json").to_path_buf();
    let b = JsonFileBackend { path: path.clone() };

    b.store(ProviderId::Anthropic, "sk-ant-XXX").unwrap();
    let got = b.retrieve(ProviderId::Anthropic).unwrap();
    assert_eq!(got, "sk-ant-XXX");
}

#[test]
fn store_creates_file_with_mode_0600() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let path = tmp.child("auth.json").to_path_buf();
    let b = JsonFileBackend { path: path.clone() };

    b.store(ProviderId::Anthropic, "k").unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "expected 0600, got {mode:o}");
}

#[test]
fn forget_removes_only_target_provider() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let path = tmp.child("auth.json").to_path_buf();
    let b = JsonFileBackend { path: path.clone() };

    b.store(ProviderId::Anthropic, "a").unwrap();
    b.store(ProviderId::Openai, "o").unwrap();
    b.forget(ProviderId::Anthropic).unwrap();
    assert!(b.retrieve(ProviderId::Anthropic).is_err());
    assert_eq!(b.retrieve(ProviderId::Openai).unwrap(), "o");
}

#[test]
fn retrieve_missing_provider_returns_not_configured_error() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let path = tmp.child("auth.json").to_path_buf();
    let b = JsonFileBackend { path };
    let err = b.retrieve(ProviderId::Anthropic).unwrap_err();
    assert!(matches!(err, rupu_auth::AuthError::NotConfigured(_)));
}

#[test]
fn wrong_mode_emits_warning_but_still_reads() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = assert_fs::TempDir::new().unwrap();
    let path = tmp.child("auth.json").to_path_buf();
    let b = JsonFileBackend { path: path.clone() };
    b.store(ProviderId::Anthropic, "k").unwrap();

    // Make it world-readable
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o644);
    std::fs::set_permissions(&path, perms).unwrap();

    // Should still retrieve successfully (warn, not fail)
    let got = b.retrieve(ProviderId::Anthropic).unwrap();
    assert_eq!(got, "k");
}
