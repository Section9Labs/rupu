use rupu_skills::tree::{content_hash, stamp, walk_tree, EntryKind, TreeError};
use std::fs;
use std::path::Path;

fn write(p: &Path, s: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, s).unwrap();
}

fn skill(dir: &Path) {
    write(&dir.join("SKILL.md"), "---\nname: s\ndescription: d\n---\n");
    write(&dir.join("scripts/run.sh"), "echo hi\n");
    write(&dir.join("references/REF.md"), "ref\n");
}

#[test]
fn walk_is_sorted_relative_and_skips_noise() {
    let t = tempfile::tempdir().unwrap();
    skill(t.path());
    write(&t.path().join(".git/HEAD"), "ref: refs/heads/main\n");
    write(&t.path().join(".DS_Store"), "x");
    write(&t.path().join("references/.DS_Store"), "x");
    fs::create_dir_all(t.path().join("empty")).unwrap();
    let rels: Vec<String> = walk_tree(t.path())
        .unwrap()
        .into_iter()
        .map(|e| e.rel)
        .collect();
    assert_eq!(
        rels,
        vec!["SKILL.md", "references/REF.md", "scripts/run.sh"]
    );
}

#[test]
fn stamp_ignores_noise_and_tracks_edits() {
    let t = tempfile::tempdir().unwrap();
    skill(t.path());
    let a = stamp(&walk_tree(t.path()).unwrap());
    write(&t.path().join(".git/FETCH_HEAD"), "abc\n");
    write(&t.path().join(".DS_Store"), "y");
    assert_eq!(
        a,
        stamp(&walk_tree(t.path()).unwrap()),
        "noise moved the stamp"
    );

    write(&t.path().join("references/NEW.md"), "new\n");
    let b = stamp(&walk_tree(t.path()).unwrap());
    assert_ne!(a, b, "an added file must move the stamp");
}

#[cfg(unix)]
#[test]
fn chmod_moves_both_fingerprints() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    skill(t.path());
    let e1 = walk_tree(t.path()).unwrap();
    let (s1, h1) = (stamp(&e1), content_hash(t.path(), &e1).unwrap());
    fs::set_permissions(
        t.path().join("scripts/run.sh"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let e2 = walk_tree(t.path()).unwrap();
    assert_ne!(s1, stamp(&e2));
    assert_ne!(h1, content_hash(t.path(), &e2).unwrap());
}

#[test]
fn content_hash_is_stable_and_content_sensitive() {
    let t = tempfile::tempdir().unwrap();
    skill(t.path());
    let e = walk_tree(t.path()).unwrap();
    let h = content_hash(t.path(), &e).unwrap();
    assert_eq!(h, content_hash(t.path(), &e).unwrap());
    assert_eq!(h.hex().len(), 64);
    assert_eq!(h.short(), h.hex()[..12]);

    let u = tempfile::tempdir().unwrap();
    skill(u.path());
    write(&u.path().join("scripts/run.sh"), "echo bye\n");
    let eu = walk_tree(u.path()).unwrap();
    assert_ne!(h, content_hash(u.path(), &eu).unwrap());
}

#[cfg(unix)]
#[test]
fn symlinks_inside_are_kept_outside_are_refused() {
    let t = tempfile::tempdir().unwrap();
    skill(t.path());
    std::os::unix::fs::symlink("references/REF.md", t.path().join("REF-LINK.md")).unwrap();
    let e = walk_tree(t.path()).unwrap();
    let link = e.iter().find(|x| x.rel == "REF-LINK.md").unwrap();
    assert!(matches!(&link.kind, EntryKind::Symlink { target } if target == "references/REF.md"));

    let outside = tempfile::tempdir().unwrap();
    write(&outside.path().join("secret"), "s");
    std::os::unix::fs::symlink(outside.path().join("secret"), t.path().join("leak")).unwrap();
    assert!(matches!(walk_tree(t.path()), Err(TreeError::BadSymlink { rel, .. }) if rel == "leak"));
}

#[cfg(unix)]
#[test]
fn dangling_symlink_is_refused() {
    let t = tempfile::tempdir().unwrap();
    skill(t.path());
    std::os::unix::fs::symlink("nowhere", t.path().join("dangle")).unwrap();
    assert!(matches!(
        walk_tree(t.path()),
        Err(TreeError::BadSymlink { .. })
    ));
}

fn fingerprints(dir: &Path) -> (rupu_skills::Stamp, rupu_skills::ContentHash) {
    let e = walk_tree(dir).unwrap();
    (stamp(&e), content_hash(dir, &e).unwrap())
}

#[cfg(unix)]
#[test]
fn fifo_is_refused_without_opening_it() {
    let t = tempfile::tempdir().unwrap();
    skill(t.path());
    let status = std::process::Command::new("mkfifo")
        .arg(t.path().join("references/pipe"))
        .status()
        .unwrap();
    assert!(status.success());
    assert!(matches!(
        walk_tree(t.path()),
        Err(TreeError::NotRegular { rel }) if rel == "references/pipe"
    ));
}

#[cfg(unix)]
#[test]
fn backslash_in_a_file_name_is_part_of_the_name() {
    let t = tempfile::tempdir().unwrap();
    skill(t.path());
    write(&t.path().join("a\\b"), "x");
    write(&t.path().join("a/b"), "x");
    let e = walk_tree(t.path()).unwrap();
    let rels: Vec<&str> = e.iter().map(|x| x.rel.as_str()).collect();
    assert!(rels.contains(&"a\\b") && rels.contains(&"a/b"), "{rels:?}");
    content_hash(t.path(), &e).unwrap();

    let only_slash = tempfile::tempdir().unwrap();
    skill(only_slash.path());
    write(&only_slash.path().join("a/b"), "x");
    let only_backslash = tempfile::tempdir().unwrap();
    skill(only_backslash.path());
    write(&only_backslash.path().join("a\\b"), "x");
    assert_ne!(
        fingerprints(only_slash.path()),
        fingerprints(only_backslash.path())
    );
}

#[test]
fn entries_are_in_path_string_order() {
    let t = tempfile::tempdir().unwrap();
    write(&t.path().join("a/x"), "1");
    write(&t.path().join("a-b/x"), "2");
    let rels: Vec<String> = walk_tree(t.path())
        .unwrap()
        .into_iter()
        .map(|e| e.rel)
        .collect();
    assert_eq!(rels, vec!["a-b/x", "a/x"]);
}

#[cfg(unix)]
#[test]
fn only_the_exec_bit_counts_in_the_mode() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    skill(t.path());
    let script = t.path().join("scripts/run.sh");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o644)).unwrap();
    let base = fingerprints(t.path());
    fs::set_permissions(&script, fs::Permissions::from_mode(0o664)).unwrap();
    assert_eq!(base, fingerprints(t.path()), "g+w moved a fingerprint");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let exec = fingerprints(t.path());
    assert_ne!(base.0, exec.0, "exec bit must move the stamp");
    assert_ne!(base.1, exec.1, "exec bit must move the content hash");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(exec, fingerprints(t.path()), "any exec bit hashes as 755");
}

#[cfg(unix)]
#[test]
fn symlink_into_an_ignored_dir_is_refused() {
    let t = tempfile::tempdir().unwrap();
    skill(t.path());
    write(&t.path().join(".git/config"), "[core]\n");
    std::os::unix::fs::symlink(".git/config", t.path().join("cfg")).unwrap();
    match walk_tree(t.path()) {
        Err(TreeError::BadSymlink { rel, reason }) => {
            assert_eq!(rel, "cfg");
            assert!(reason.contains(".git"), "{reason}");
        }
        other => panic!("expected BadSymlink, got {other:?}"),
    }

    let u = tempfile::tempdir().unwrap();
    skill(u.path());
    write(&u.path().join(".DS_Store"), "x");
    std::os::unix::fs::symlink(".DS_Store", u.path().join("ds")).unwrap();
    assert!(matches!(
        walk_tree(u.path()),
        Err(TreeError::BadSymlink { rel, reason }) if rel == "ds" && reason.contains(".DS_Store")
    ));
}

#[test]
fn same_length_edit_moves_the_content_hash() {
    let t = tempfile::tempdir().unwrap();
    skill(t.path());
    let p = t.path().join("references/REF.md");
    write(&p, "aaaa");
    let (_, before) = fingerprints(t.path());
    write(&p, "bbbb");
    let (_, after) = fingerprints(t.path());
    assert_ne!(before, after);
}

#[test]
fn rewrite_with_a_new_mtime_moves_the_stamp() {
    use std::time::{Duration, SystemTime};
    let t = tempfile::tempdir().unwrap();
    skill(t.path());
    let p = t.path().join("references/REF.md");
    let set = |when: SystemTime| {
        fs::File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_modified(when)
            .unwrap()
    };
    let epoch = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    write(&p, "aaaa");
    set(epoch);
    let (before, _) = fingerprints(t.path());
    write(&p, "aaaa");
    set(epoch + Duration::from_secs(1));
    let (after, _) = fingerprints(t.path());
    assert_ne!(before, after);
}

#[test]
fn rename_moves_both_fingerprints() {
    let t = tempfile::tempdir().unwrap();
    skill(t.path());
    let (s1, h1) = fingerprints(t.path());
    fs::rename(
        t.path().join("references/REF.md"),
        t.path().join("references/RENAMED.md"),
    )
    .unwrap();
    let (s2, h2) = fingerprints(t.path());
    assert_ne!(s1, s2);
    assert_ne!(h1, h2);
}
