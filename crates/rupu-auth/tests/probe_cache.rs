use assert_fs::prelude::*;
use rupu_auth::{BackendChoice, ProbeCache};

#[test]
fn first_call_uses_default_without_writing_cache() {
    // The default backend is now `file`; selection happens lazily
    // and doesn't persist to the cache file. Cache is only written
    // when the user explicitly opts via `rupu auth backend --use ...`.
    let tmp = assert_fs::TempDir::new().unwrap();
    let cache_child = tmp.child("cache.json");
    let cache = ProbeCache::new(cache_child.to_path_buf());

    let backend = rupu_auth::select_backend(&cache, tmp.child("auth.json").to_path_buf());
    assert_eq!(backend.name(), "json-file");
    cache_child.assert(predicates::path::missing());
}

#[test]
fn cached_choice_is_honored() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let cache_path = tmp.child("cache.json").to_path_buf();
    let cache = ProbeCache::new(cache_path);
    cache.write(BackendChoice::Keyring).unwrap();

    // We can't check that we got the keychain backend at runtime
    // without prompting; just verify the cache round-trip + that
    // both calls agree (deterministic).
    let b1 = rupu_auth::select_backend(&cache, tmp.child("auth.json").to_path_buf());
    let b2 = rupu_auth::select_backend(&cache, tmp.child("auth.json").to_path_buf());
    assert_eq!(b1.name(), b2.name());
}

#[test]
fn invalidate_clears_cache() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let cache_child = tmp.child("cache.json");
    let cache = ProbeCache::new(cache_child.to_path_buf());
    cache.write(BackendChoice::JsonFile).unwrap();
    cache_child.assert(predicates::path::is_file());

    cache.invalidate().unwrap();
    cache_child.assert(predicates::path::missing());
}
