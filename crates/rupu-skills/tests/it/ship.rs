use rupu_skills::ship::{pack_skill, unpack_skill, ShipError, MAX_SHIPPED_SKILL_BYTES};
use rupu_skills::tree::{content_hash, walk_tree, ContentHasher};
use rupu_skills::{load_skill, Layer};

#[test]
fn hasher_matches_content_hash() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = crate::support::write_skill(tmp.path(), "pdf", "pdf", "Read PDFs.");
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    std::fs::write(dir.join("scripts/run.sh"), b"#!/bin/sh\necho hi\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("scripts/run.sh", dir.join("LINK.sh")).unwrap();
    let entries = walk_tree(&dir).unwrap();
    let mut h = ContentHasher::new();
    for e in &entries {
        match &e.kind {
            rupu_skills::tree::EntryKind::File { mode, .. } => {
                h.file(&e.rel, *mode, &std::fs::read(dir.join(&e.rel)).unwrap())
            }
            rupu_skills::tree::EntryKind::Symlink { target } => h.symlink(&e.rel, target),
        }
    }
    assert_eq!(h.finish(), content_hash(&dir, &entries).unwrap());
}

#[test]
fn pack_then_unpack_reproduces_the_content_hash() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = crate::support::write_skill(tmp.path(), "pdf", "pdf", "Read PDFs.");
    std::fs::create_dir_all(dir.join("references")).unwrap();
    std::fs::write(dir.join("references/REF.md"), b"ref").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("references/REF.md", dir.join("LINK.md")).unwrap();
    let skill = load_skill(&dir, Layer::Global).unwrap();
    let packed = pack_skill(&skill).unwrap();
    assert_eq!(packed.hash, skill.content_hash().unwrap());
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("pdf");
    unpack_skill(&packed.bytes, &dest, &packed.hash).unwrap();
    let entries = walk_tree(&dest).unwrap();
    assert_eq!(content_hash(&dest, &entries).unwrap(), packed.hash);
    assert_eq!(load_skill(&dest, Layer::Global).unwrap().body, skill.body);
}

#[cfg(unix)]
#[test]
fn an_absolute_symlink_is_refused_even_inside_the_root() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = crate::support::write_skill(tmp.path(), "pdf", "pdf", "Read PDFs.");
    std::fs::write(dir.join("a.md"), b"a").unwrap();
    let abs = dir.canonicalize().unwrap().join("a.md");
    std::os::unix::fs::symlink(&abs, dir.join("b.md")).unwrap();
    let skill = load_skill(&dir, Layer::Global).unwrap();
    let err = pack_skill(&skill).unwrap_err();
    assert!(
        matches!(&err, ShipError::AbsoluteSymlink { rel, .. } if rel == "b.md"),
        "{err}"
    );
}

#[test]
fn a_tampered_archive_fails_the_hash_check_and_leaves_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = crate::support::write_skill(tmp.path(), "pdf", "pdf", "Read PDFs.");
    let skill = load_skill(&dir, Layer::Global).unwrap();
    let packed = pack_skill(&skill).unwrap();
    let wrong = rupu_skills::ContentHash([7u8; 32]);
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("pdf");
    let err = unpack_skill(&packed.bytes, &dest, &wrong).unwrap_err();
    assert!(matches!(err, ShipError::HashMismatch { .. }), "{err}");
    assert!(!dest.exists());
}

/// Hand-built archives the coordinator would never produce.
fn tar_with(path: &str, kind: tar::EntryType, link: Option<&str>, data: &[u8]) -> Vec<u8> {
    let mut b = tar::Builder::new(Vec::new());
    let mut h = tar::Header::new_ustar();
    h.set_entry_type(kind);
    h.set_mode(0o644);
    h.set_size(data.len() as u64);
    if let Some(l) = link {
        h.set_link_name(l).unwrap();
    }
    // set_path refuses "..", so write the raw name bytes.
    h.as_old_mut().name[..path.len()].copy_from_slice(path.as_bytes());
    h.set_cksum();
    b.append(&h, data).unwrap();
    b.into_inner().unwrap()
}

#[test]
fn unpack_refuses_escaping_paths_links_and_devices() {
    let any = rupu_skills::ContentHash([0u8; 32]);
    for (bytes, what) in [
        (
            tar_with("../evil", tar::EntryType::Regular, None, b"x"),
            "dot-dot",
        ),
        (
            tar_with("/etc/evil", tar::EntryType::Regular, None, b"x"),
            "absolute",
        ),
        (
            tar_with("l", tar::EntryType::Symlink, Some("../../x"), b""),
            "escaping link",
        ),
        (
            tar_with("l", tar::EntryType::Symlink, Some("/etc/passwd"), b""),
            "absolute link",
        ),
        (tar_with("d", tar::EntryType::Char, None, b""), "device"),
    ] {
        let out = tempfile::tempdir().unwrap();
        let dest = out.path().join("pdf");
        let err = unpack_skill(&bytes, &dest, &any).unwrap_err();
        assert!(matches!(err, ShipError::Malformed(_)), "{what}: {err}");
        assert!(!dest.exists(), "{what}");
        assert!(
            !out.path().join("evil").exists()
                && !out.path().parent().unwrap().join("evil").exists()
        );
    }
}

fn tar_multi(entries: &[(&str, tar::EntryType, Option<&str>, &[u8])]) -> Vec<u8> {
    let mut b = tar::Builder::new(Vec::new());
    for (path, kind, link, data) in entries {
        let mut h = tar::Header::new_ustar();
        h.set_entry_type(*kind);
        h.set_mode(0o644);
        h.set_size(data.len() as u64);
        if let Some(l) = link {
            h.set_link_name(l).unwrap();
        }
        h.as_old_mut().name[..path.len()].copy_from_slice(path.as_bytes());
        h.set_cksum();
        b.append(&h, *data).unwrap();
    }
    b.into_inner().unwrap()
}

#[cfg(unix)]
#[test]
fn chained_symlinks_and_duplicates_are_refused_before_anything_is_written() {
    use tar::EntryType::{Regular, Symlink};
    let any = rupu_skills::ContentHash([0u8; 32]);
    let cases = [
        (
            "chain",
            tar_multi(&[
                ("a", Symlink, Some("."), b""),
                ("a/l", Symlink, Some(".."), b""),
                ("a/l/evil", Regular, None, b"x"),
            ]),
        ),
        (
            "deep chain",
            tar_multi(&[
                ("a", Symlink, Some("."), b""),
                ("a/l", Symlink, Some(".."), b""),
                ("a/l/x", Symlink, Some(".."), b""),
                ("a/l/x/evil", Regular, None, b"x"),
            ]),
        ),
        (
            "case-fold chain",
            tar_multi(&[
                ("A", Symlink, Some("."), b""),
                ("a/l", Symlink, Some(".."), b""),
                ("a/L/x/y", Symlink, Some("z"), b""),
            ]),
        ),
        (
            "case-fold duplicate",
            tar_multi(&[("f", Regular, None, b"1"), ("F", Regular, None, b"2")]),
        ),
        (
            "ignored name",
            tar_multi(&[
                ("SKILL.md", Regular, None, b"x"),
                (".git/a", Symlink, Some(".."), b""),
                (".git/b", Symlink, Some("a/.."), b""),
            ]),
        ),
        (
            "ignored name, folded",
            tar_multi(&[(".GIT/a", Regular, None, b"x")]),
        ),
        (
            "duplicate",
            tar_multi(&[("f", Regular, None, b"1"), ("f", Regular, None, b"2")]),
        ),
        (
            "below a file",
            tar_multi(&[("f", Regular, None, b"1"), ("f/g", Regular, None, b"2")]),
        ),
    ];
    for (what, bytes) in cases {
        let out = tempfile::tempdir().unwrap();
        let dest = out.path().join("pdf");
        let err = unpack_skill(&bytes, &dest, &any).unwrap_err();
        assert!(matches!(err, ShipError::Malformed(_)), "{what}: {err}");
        assert!(!dest.exists(), "{what}");
        assert!(!out.path().join("evil").exists(), "{what}");
        assert!(!out.path().join("x").exists(), "{what}");
    }
}

#[test]
fn a_truncated_archive_is_malformed() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = crate::support::write_skill(tmp.path(), "pdf", "pdf", "Read PDFs.");
    let packed = pack_skill(&load_skill(&dir, Layer::Global).unwrap()).unwrap();
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("pdf");
    let cut = &packed.bytes[..packed.bytes.len() / 2 + 3];
    let err = unpack_skill(cut, &dest, &packed.hash).unwrap_err();
    assert!(matches!(err, ShipError::Malformed(_)), "{err}");
    assert!(!dest.exists());
}

#[test]
fn unpacking_into_an_existing_dest_fails_and_leaves_it_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = crate::support::write_skill(tmp.path(), "pdf", "pdf", "Read PDFs.");
    let packed = pack_skill(&load_skill(&dir, Layer::Global).unwrap()).unwrap();
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("pdf");
    std::fs::create_dir(&dest).unwrap();
    std::fs::write(dest.join("keep"), b"k").unwrap();
    let err = unpack_skill(&packed.bytes, &dest, &packed.hash).unwrap_err();
    assert!(matches!(err, ShipError::Malformed(_)), "{err}");
    assert_eq!(std::fs::read(dest.join("keep")).unwrap(), b"k");
}

#[test]
fn an_over_cap_skill_is_refused_by_pack() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = crate::support::write_skill(tmp.path(), "big", "big", "Big.");
    let f = std::fs::File::create(dir.join("blob.bin")).unwrap();
    f.set_len(MAX_SHIPPED_SKILL_BYTES + 1).unwrap(); // sparse; cheap
    let skill = load_skill(&dir, Layer::Global).unwrap();
    assert!(matches!(
        pack_skill(&skill).unwrap_err(),
        ShipError::TooLarge { .. }
    ));
}

#[test]
fn names_differing_only_by_case_are_a_collision() {
    use rupu_skills::ship::case_collision;
    assert_eq!(
        case_collision(&["README.md", "readme.md"]),
        Some(("README.md".into(), "readme.md".into()))
    );
    assert!(case_collision(&["a", "A/x"]).is_some());
    assert!(case_collision(&["A/x", "a/y"]).is_none());
    assert!(case_collision(&["SKILL.md", "scripts/run.sh"]).is_none());
}

#[cfg(target_os = "linux")]
#[test]
fn pack_refuses_a_case_collision() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = crate::support::write_skill(tmp.path(), "pdf", "pdf", "Read PDFs.");
    std::fs::write(dir.join("README.md"), b"1").unwrap();
    std::fs::write(dir.join("readme.md"), b"2").unwrap();
    let skill = load_skill(&dir, Layer::Global).unwrap();
    assert!(matches!(
        pack_skill(&skill).unwrap_err(),
        ShipError::CaseCollision { .. }
    ));
}

mod cache {
    use rupu_skills::ship::*;

    fn packed(tmp: &std::path::Path, name: &str) -> PackedSkill {
        let dir = crate::support::write_skill(tmp, name, name, "D.");
        pack_skill(&rupu_skills::load_skill(&dir, rupu_skills::Layer::Global).unwrap()).unwrap()
    }

    #[test]
    fn store_then_missing_and_overlay() {
        let src = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let cache = cache_dir(home.path());
        let p = packed(src.path(), "pdf");
        let r = p.reference();
        assert_eq!(missing(&cache, std::slice::from_ref(&r)), vec![r.clone()]);
        let dir = store(&cache, &r, &p.bytes).unwrap();
        assert_eq!(dir, cached_path(&cache, &r));
        assert!(missing(&cache, std::slice::from_ref(&r)).is_empty());
        let m = SkillBundleManifest {
            version: 1,
            skills: vec![r.clone()],
        };
        let o = overlay_for(&cache, &m).unwrap();
        assert_eq!(o[0].dir, dir);
        assert_eq!(store(&cache, &r, &p.bytes).unwrap(), dir);
    }

    #[test]
    fn concurrent_stores_of_one_hash_both_succeed() {
        let src = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let cache = cache_dir(home.path());
        let p = std::sync::Arc::new(packed(src.path(), "pdf"));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let (p, cache) = (p.clone(), cache.clone());
                std::thread::spawn(move || store(&cache, &p.reference(), &p.bytes))
            })
            .collect();
        for h in handles {
            h.join().unwrap().unwrap();
        }
        let hash_dirs: Vec<_> = std::fs::read_dir(&cache)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .collect();
        assert_eq!(hash_dirs.len(), 1);
    }

    #[test]
    fn a_leftover_temp_dir_is_not_cached_and_a_later_store_works() {
        let src = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let cache = cache_dir(home.path());
        let p = packed(src.path(), "pdf");
        std::fs::create_dir_all(cache.join(".tmp-crashed").join("pdf")).unwrap();
        assert_eq!(missing(&cache, &[p.reference()]).len(), 1);
        store(&cache, &p.reference(), &p.bytes).unwrap();
        assert!(missing(&cache, &[p.reference()]).is_empty());
    }

    #[test]
    fn overlay_names_a_gap() {
        let home = tempfile::tempdir().unwrap();
        let m = SkillBundleManifest {
            version: 1,
            skills: vec![ShippedSkillRef {
                layer: rupu_skills::Layer::Global,
                name: "pdf".into(),
                content_hash: "a".repeat(64),
            }],
        };
        let err = overlay_for(&cache_dir(home.path()), &m).unwrap_err();
        assert!(err.to_string().contains("global:pdf"), "{err}");
    }

    #[test]
    fn manifest_validation() {
        let ok = ShippedSkillRef {
            layer: rupu_skills::Layer::Global,
            name: "pdf".into(),
            content_hash: "a".repeat(64),
        };
        assert!(SkillBundleManifest {
            version: 1,
            skills: vec![ok.clone()]
        }
        .validate()
        .is_ok());
        for bad in [
            SkillBundleManifest {
                version: 2,
                skills: vec![ok.clone()],
            },
            SkillBundleManifest {
                version: 1,
                skills: vec![ShippedSkillRef {
                    name: "../x".into(),
                    ..ok.clone()
                }],
            },
            SkillBundleManifest {
                version: 1,
                skills: vec![ShippedSkillRef {
                    content_hash: "zz".into(),
                    ..ok.clone()
                }],
            },
            SkillBundleManifest {
                version: 1,
                skills: vec![ShippedSkillRef {
                    content_hash: "A".repeat(64),
                    ..ok.clone()
                }],
            },
            SkillBundleManifest {
                version: 1,
                skills: vec![ok.clone(), ok.clone()],
            },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn stream_round_trip_and_cap() {
        let src = tempfile::tempdir().unwrap();
        let (a, b) = (packed(src.path(), "pdf"), packed(src.path(), "notes"));
        let (ra, rb) = (a.reference(), b.reference());
        let bytes = encode_stream(&[(&ra, &a.bytes[..]), (&rb, &b.bytes[..])]);
        let got = decode_stream(&mut &bytes[..], MAX_BUNDLE_BYTES).unwrap();
        assert_eq!(
            got,
            vec![(ra.clone(), a.bytes.clone()), (rb, b.bytes.clone())]
        );
        assert!(decode_stream(&mut &bytes[..], 10).is_err());
        assert!(decode_stream(&mut &b"nonsense\n"[..], MAX_BUNDLE_BYTES).is_err());
        let short = &bytes[..bytes.len() - 5];
        assert!(decode_stream(&mut &short[..], MAX_BUNDLE_BYTES).is_err());
    }
}
