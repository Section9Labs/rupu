use assert_fs::prelude::*;
use rupu_workspace::discover;

#[test]
fn finds_rupu_dir_in_pwd() {
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child(".rupu").create_dir_all().unwrap();
    let d = discover(tmp.path()).unwrap();
    assert_eq!(
        d.project_root.as_deref().map(|p| p.canonicalize().unwrap()),
        Some(tmp.path().canonicalize().unwrap())
    );
    assert_eq!(d.canonical_pwd, tmp.path().canonicalize().unwrap());
}

#[test]
fn walks_up_to_find_rupu_dir() {
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child(".rupu").create_dir_all().unwrap();
    let nested = tmp.child("a/b/c");
    nested.create_dir_all().unwrap();

    let d = discover(nested.path()).unwrap();
    assert_eq!(
        d.project_root.as_deref().map(|p| p.canonicalize().unwrap()),
        Some(tmp.path().canonicalize().unwrap())
    );
}

#[test]
fn no_rupu_dir_means_no_project_root() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let nested = tmp.child("x/y");
    nested.create_dir_all().unwrap();
    let d = discover(nested.path()).unwrap();
    assert!(d.project_root.is_none());
    assert_eq!(d.canonical_pwd, nested.path().canonicalize().unwrap());
}

#[test]
fn nonexistent_pwd_returns_io_error() {
    use rupu_workspace::DiscoverError;
    let result = discover(std::path::Path::new(
        "/this/path/does/not/exist/hopefully-rupu",
    ));
    assert!(
        matches!(result, Err(DiscoverError::Io { .. })),
        "expected DiscoverError::Io for nonexistent pwd, got: {result:?}"
    );
}
