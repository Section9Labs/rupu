//! `rupu __skills missing|receive` against an isolated RUPU_HOME.

use assert_cmd::Command;
use rupu_skills::ship::{encode_stream, pack_skill};
use rupu_skills::{load_skill, Layer};
use std::path::Path;

fn rupu(home: &Path, args: &[&str], stdin: Vec<u8>) -> std::process::Output {
    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", home)
        .env_remove("RUPU_LOG")
        .args(args)
        .write_stdin(stdin)
        .output()
        .unwrap()
}

fn packed(dir: &Path) -> rupu_skills::ship::PackedSkill {
    let root = dir.join("pdf");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("SKILL.md"),
        "---\nname: pdf\ndescription: PDFs.\n---\n\nBody.\n",
    )
    .unwrap();
    pack_skill(&load_skill(&root, Layer::Global).unwrap()).unwrap()
}

#[test]
fn missing_receive_missing() {
    let home = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let p = packed(src.path());
    let r = p.reference();
    let arg = format!("{}:global:pdf", r.content_hash);

    let out = rupu(home.path(), &["__skills", "missing", &arg], vec![]);
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), arg);

    let stream = encode_stream(&[(&r, &p.bytes)]);
    let out = rupu(home.path(), &["__skills", "receive"], stream);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("ok {}", r.content_hash)
    );

    let out = rupu(home.path(), &["__skills", "missing", &arg], vec![]);
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
}

#[test]
fn receive_with_wrong_hash_fails_and_caches_nothing() {
    let home = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let p = packed(src.path());
    let mut r = p.reference();
    r.content_hash = "0".repeat(64);

    let stream = encode_stream(&[(&r, &p.bytes)]);
    let out = rupu(home.path(), &["__skills", "receive"], stream);
    assert_eq!(out.status.code(), Some(1));
    let cache = home.path().join("cache/skills");
    let leftovers = std::fs::read_dir(&cache)
        .map(|d| d.filter_map(|e| e.ok()).count())
        .unwrap_or(0);
    assert_eq!(leftovers, 0, "nothing cached");
}

#[test]
fn missing_rejects_a_malformed_ref() {
    let home = tempfile::tempdir().unwrap();
    let out = rupu(home.path(), &["__skills", "missing", "nonsense"], vec![]);
    assert_eq!(out.status.code(), Some(1));
}
