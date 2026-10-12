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
