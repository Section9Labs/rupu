use assert_fs::prelude::*;
use rupu_auth::ProbeCache;

#[test]
fn writes_cache_file_on_first_call() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let cache_child = tmp.child("cache.json");
    let cache = ProbeCache {
        path: cache_child.to_path_buf(),
    };

    let _backend = rupu_auth::select_backend(&cache, tmp.child("auth.json").to_path_buf());

    cache_child.assert(predicates::path::is_file());
}

#[test]
fn second_call_uses_cached_choice() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let cache_path = tmp.child("cache.json").to_path_buf();
    let cache = ProbeCache {
        path: cache_path.clone(),
    };

    let b1 = rupu_auth::select_backend(&cache, tmp.child("auth.json").to_path_buf());
    let b2 = rupu_auth::select_backend(&cache, tmp.child("auth.json").to_path_buf());
    assert_eq!(b1.name(), b2.name());
}

#[test]
fn invalidate_clears_cache() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let cache_child = tmp.child("cache.json");
    let cache = ProbeCache {
        path: cache_child.to_path_buf(),
    };

    let _ = rupu_auth::select_backend(&cache, tmp.child("auth.json").to_path_buf());
    cache.invalidate().unwrap();
    cache_child.assert(predicates::path::missing());
}
