use rupu_skills::pinned::{install_pins, KeptReason, Pin, PinOutcome};
use rupu_skills::record::{read_record, write_record, SourceRecord};
use rupu_skills::{Fetch, FetchedTree, GitSource, Layer, SourceError};
use std::cell::RefCell;
use std::path::{Path, PathBuf};

use crate::support::write_skill;

const URL: &str = "https://github.com/Section9Labs/rupu-skills";
const C1: &str = "1111111111111111111111111111111111111111";
const C2: &str = "2222222222222222222222222222222222222222";

/// Serves a generated repository (`skills/<name>/SKILL.md` per name) and
/// reports whatever commit it is told to; records every fetch.
struct FixtureFetch {
    root: PathBuf,
    commit: String,
    fail_url: Option<String>,
    fetches: RefCell<Vec<(String, Option<String>)>>,
}

impl FixtureFetch {
    fn new(root: &Path, names: &[&str], body: &str, commit: &str) -> Self {
        for n in names {
            write_skill(&root.join("skills"), n, n, &format!("{n} {body}"));
        }
        Self {
            root: root.to_owned(),
            commit: commit.to_owned(),
            fail_url: None,
            fetches: RefCell::new(Vec::new()),
        }
    }
}

impl Fetch for FixtureFetch {
    fn fetch(&self, src: &GitSource) -> Result<FetchedTree, SourceError> {
        self.fetches
            .borrow_mut()
            .push((src.url.clone(), src.git_ref.clone()));
        if self.fail_url.as_deref() == Some(src.url.as_str()) {
            return Err(SourceError::Clone("network is down".into()));
        }
        Ok(FetchedTree::local_with_commit(
            self.root.clone(),
            Some(self.commit.clone()),
        ))
    }
}

fn pin(name: &str, commit: &str) -> Pin {
    Pin {
        name: name.into(),
        source: "Section9Labs/rupu-skills".into(),
        tag: "v1.0.0".into(),
        commit: commit.into(),
    }
}

fn outcomes(r: &rupu_skills::pinned::PinReport) -> Vec<(String, PinOutcome)> {
    r.results
        .iter()
        .map(|(p, o)| (p.name.clone(), o.clone()))
        .collect()
}

#[test]
fn a_first_install_creates_every_skill_with_a_pinned_record() {
    let src = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let f = FixtureFetch::new(src.path(), &["network-recon", "work-units"], "v1", C1);
    let r = install_pins(
        &[pin("network-recon", C1), pin("work-units", C1)],
        dest.path(),
        Layer::Global,
        false,
        &f,
    );
    assert_eq!(
        outcomes(&r),
        vec![
            ("network-recon".into(), PinOutcome::Created),
            ("work-units".into(), PinOutcome::Created)
        ]
    );
    assert_eq!(f.fetches.borrow().len(), 1, "one fetch per (url, commit)");
    assert_eq!(f.fetches.borrow()[0], (URL.to_owned(), Some(C1.to_owned())));
    let rec = read_record(dest.path(), "network-recon").unwrap().unwrap();
    assert_eq!(rec.source, "Section9Labs/rupu-skills");
    assert_eq!(rec.url.as_deref(), Some(URL));
    assert_eq!(rec.git_ref.as_deref(), Some(C1));
    assert_eq!(rec.commit.as_deref(), Some(C1));
    assert_eq!(rec.path.as_deref(), Some("skills/network-recon"));
    assert!(r.missing().is_empty());
}

#[test]
fn a_second_install_at_the_same_pin_skips_without_fetching() {
    let src = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let f = FixtureFetch::new(src.path(), &["work-units"], "v1", C1);
    install_pins(
        &[pin("work-units", C1)],
        dest.path(),
        Layer::Global,
        false,
        &f,
    );
    let r = install_pins(
        &[pin("work-units", C1)],
        dest.path(),
        Layer::Global,
        false,
        &f,
    );
    assert_eq!(
        outcomes(&r),
        vec![("work-units".into(), PinOutcome::Skipped)]
    );
    assert_eq!(f.fetches.borrow().len(), 1);
}

#[test]
fn a_new_pin_updates_an_unmodified_skill() {
    let src1 = tempfile::tempdir().unwrap();
    let src2 = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    install_pins(
        &[pin("work-units", C1)],
        dest.path(),
        Layer::Global,
        false,
        &FixtureFetch::new(src1.path(), &["work-units"], "v1", C1),
    );
    let r = install_pins(
        &[pin("work-units", C2)],
        dest.path(),
        Layer::Global,
        false,
        &FixtureFetch::new(src2.path(), &["work-units"], "v2", C2),
    );
    assert_eq!(
        outcomes(&r),
        vec![(
            "work-units".into(),
            PinOutcome::Updated {
                from: Some(C1.into())
            }
        )]
    );
    let md = std::fs::read_to_string(dest.path().join("work-units/SKILL.md")).unwrap();
    assert!(md.contains("work-units v2"));
    assert_eq!(
        read_record(dest.path(), "work-units")
            .unwrap()
            .unwrap()
            .commit
            .as_deref(),
        Some(C2)
    );
}

#[test]
fn a_locally_modified_skill_is_kept_and_force_replaces_it() {
    let src1 = tempfile::tempdir().unwrap();
    let src2 = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    install_pins(
        &[pin("work-units", C1)],
        dest.path(),
        Layer::Global,
        false,
        &FixtureFetch::new(src1.path(), &["work-units"], "v1", C1),
    );
    std::fs::write(
        dest.path().join("work-units/scripts/run.sh"),
        "#!/bin/sh\necho mine\n",
    )
    .unwrap();
    let f2 = FixtureFetch::new(src2.path(), &["work-units"], "v2", C2);
    let kept = install_pins(
        &[pin("work-units", C2)],
        dest.path(),
        Layer::Global,
        false,
        &f2,
    );
    assert_eq!(
        outcomes(&kept),
        vec![(
            "work-units".into(),
            PinOutcome::Kept(KeptReason::LocallyModified)
        )]
    );
    assert_eq!(
        std::fs::read_to_string(dest.path().join("work-units/scripts/run.sh")).unwrap(),
        "#!/bin/sh\necho mine\n"
    );
    let forced = install_pins(
        &[pin("work-units", C2)],
        dest.path(),
        Layer::Global,
        true,
        &f2,
    );
    assert_eq!(
        outcomes(&forced),
        vec![("work-units".into(), PinOutcome::Replaced)]
    );
}

#[test]
fn a_hand_installed_skill_of_the_same_name_is_kept() {
    let src = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    write_skill(dest.path(), "network-recon", "network-recon", "my own");
    let f = FixtureFetch::new(src.path(), &["network-recon"], "v1", C1);
    let r = install_pins(
        &[pin("network-recon", C1)],
        dest.path(),
        Layer::Global,
        false,
        &f,
    );
    assert_eq!(
        outcomes(&r),
        vec![(
            "network-recon".into(),
            PinOutcome::Kept(KeptReason::OtherSource)
        )]
    );
    assert!(
        f.fetches.borrow().is_empty(),
        "nothing to install, nothing fetched"
    );
    let md = std::fs::read_to_string(dest.path().join("network-recon/SKILL.md")).unwrap();
    assert!(md.contains("my own"));
}

#[test]
fn a_fetched_commit_other_than_the_pin_installs_nothing() {
    let src = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let f = FixtureFetch::new(src.path(), &["work-units"], "v1", C2);
    let r = install_pins(
        &[pin("work-units", C1)],
        dest.path(),
        Layer::Global,
        false,
        &f,
    );
    match &r.results[0].1 {
        PinOutcome::Failed(msg) => assert!(msg.contains(C1) && msg.contains(C2), "{msg}"),
        other => panic!("expected Failed, got {other:?}"),
    }
    assert!(!dest.path().join("work-units").exists());
    assert_eq!(r.missing().len(), 1);
}

#[test]
fn one_failing_source_does_not_stop_another() {
    let src = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let mut f = FixtureFetch::new(src.path(), &["work-units", "other"], "v1", C1);
    f.fail_url = Some("https://github.com/someone/elsewhere".into());
    let mut other = pin("other", C1);
    other.source = "someone/elsewhere".into();
    let r = install_pins(
        &[pin("work-units", C1), other],
        dest.path(),
        Layer::Global,
        false,
        &f,
    );
    assert_eq!(r.results[0].1, PinOutcome::Created);
    assert!(matches!(r.results[1].1, PinOutcome::Failed(ref m) if m.contains("network is down")));
    assert_eq!(
        r.missing()
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["other"]
    );
}

#[test]
fn a_pin_missing_from_the_fetched_tree_fails_alone() {
    let src = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let f = FixtureFetch::new(src.path(), &["work-units"], "v1", C1);
    let r = install_pins(
        &[pin("work-units", C1), pin("no-such-skill", C1)],
        dest.path(),
        Layer::Global,
        false,
        &f,
    );
    assert_eq!(r.results[0].1, PinOutcome::Created);
    assert!(matches!(r.results[1].1, PinOutcome::Failed(_)));
}

#[test]
fn a_non_skill_directory_of_the_same_name_is_kept() {
    let src = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dest.path().join("network-recon")).unwrap();
    std::fs::write(dest.path().join("network-recon/notes.txt"), "mine").unwrap();
    let f = FixtureFetch::new(src.path(), &["network-recon"], "v1", C1);
    let kept = install_pins(
        &[pin("network-recon", C1)],
        dest.path(),
        Layer::Global,
        false,
        &f,
    );
    assert_eq!(
        outcomes(&kept),
        vec![(
            "network-recon".into(),
            PinOutcome::Kept(KeptReason::NotASkill)
        )]
    );
    assert!(
        f.fetches.borrow().is_empty(),
        "nothing to install, nothing fetched"
    );
    assert_eq!(kept.missing().len(), 1, "a non-skill in the way is missing");
    assert_eq!(
        std::fs::read_to_string(dest.path().join("network-recon/notes.txt")).unwrap(),
        "mine"
    );
    let forced = install_pins(
        &[pin("network-recon", C1)],
        dest.path(),
        Layer::Global,
        true,
        &f,
    );
    assert_eq!(
        outcomes(&forced),
        vec![("network-recon".into(), PinOutcome::Replaced)]
    );
    assert!(dest.path().join("network-recon/SKILL.md").is_file());
}

#[test]
fn a_plain_file_at_the_name_is_not_a_skill_and_is_missing() {
    let src = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    std::fs::write(dest.path().join("network-recon"), "a file in the way").unwrap();
    let f = FixtureFetch::new(src.path(), &["network-recon"], "v1", C1);
    let r = install_pins(
        &[pin("network-recon", C1)],
        dest.path(),
        Layer::Global,
        false,
        &f,
    );
    assert_eq!(
        outcomes(&r),
        vec![(
            "network-recon".into(),
            PinOutcome::Kept(KeptReason::NotASkill)
        )]
    );
    assert_eq!(r.missing().len(), 1);
    assert!(f.fetches.borrow().is_empty());
    assert_eq!(
        std::fs::read_to_string(dest.path().join("network-recon")).unwrap(),
        "a file in the way"
    );
}

#[test]
fn a_kept_skill_is_not_missing() {
    let src = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    write_skill(dest.path(), "network-recon", "network-recon", "my own");
    let f = FixtureFetch::new(src.path(), &["network-recon"], "v1", C1);
    let r = install_pins(
        &[pin("network-recon", C1)],
        dest.path(),
        Layer::Global,
        false,
        &f,
    );
    assert!(matches!(
        r.results[0].1,
        PinOutcome::Kept(KeptReason::OtherSource)
    ));
    assert!(r.missing().is_empty());
}

#[test]
fn a_hand_install_at_the_pinned_commit_is_re_pinned_not_left_on_its_branch() {
    let src = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let f = FixtureFetch::new(src.path(), &["work-units"], "v1", C1);
    // The skill as a hand install of the same repository left it: the same
    // bytes, the pinned commit, but no ref (`rupu skill update` would
    // follow the default branch off the pin).
    let skill_dir = write_skill(dest.path(), "work-units", "work-units", "work-units v1");
    let hash = rupu_skills::load_skill(&skill_dir, Layer::Global)
        .unwrap()
        .content_hash()
        .unwrap()
        .hex();
    for hand_ref in [None, Some("v1.0.0".to_owned())] {
        let rec = SourceRecord {
            version: 1,
            source: "https://github.com/Section9Labs/rupu-skills".into(),
            url: Some(URL.into()),
            git_ref: hand_ref.clone(),
            path: Some("skills/work-units".into()),
            commit: Some(C1.into()),
            local: None,
            content_hash: hash.clone(),
            installed_at: 7,
        };
        write_record(dest.path(), "work-units", &rec).unwrap();
        let r = install_pins(
            &[pin("work-units", C1)],
            dest.path(),
            Layer::Global,
            false,
            &f,
        );
        assert_eq!(
            outcomes(&r),
            vec![("work-units".into(), PinOutcome::Skipped)],
            "{hand_ref:?}"
        );
        let got = read_record(dest.path(), "work-units").unwrap().unwrap();
        assert_eq!(got.git_ref.as_deref(), Some(C1), "{hand_ref:?}");
        assert_eq!(got.source, "Section9Labs/rupu-skills");
        assert_eq!(got.commit.as_deref(), Some(C1));
        assert_eq!(got.content_hash, hash);
        assert_eq!(got.path.as_deref(), Some("skills/work-units"));
    }
    assert!(f.fetches.borrow().is_empty(), "no fetch, no copy");
}

#[cfg(unix)]
#[test]
fn a_dangling_symlink_at_the_name_is_kept() {
    let src = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink("nowhere", dest.path().join("network-recon")).unwrap();
    let f = FixtureFetch::new(src.path(), &["network-recon"], "v1", C1);
    let r = install_pins(
        &[pin("network-recon", C1)],
        dest.path(),
        Layer::Global,
        false,
        &f,
    );
    assert_eq!(
        outcomes(&r),
        vec![(
            "network-recon".into(),
            PinOutcome::Kept(KeptReason::NotASkill)
        )]
    );
    assert!(f.fetches.borrow().is_empty());
    assert_eq!(r.missing().len(), 1, "a non-skill in the way is missing");
    assert!(std::fs::symlink_metadata(dest.path().join("network-recon"))
        .unwrap()
        .is_symlink());
}

#[test]
fn a_skill_recorded_from_a_local_directory_is_kept() {
    let src = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    write_skill(dest.path(), "network-recon", "network-recon", "mine");
    let rec = SourceRecord {
        version: 1,
        source: "./my-network-recon".into(),
        url: None,
        git_ref: None,
        path: None,
        commit: None,
        local: Some(PathBuf::from("/somewhere/my-network-recon")),
        content_hash: "0".repeat(64),
        installed_at: 0,
    };
    write_record(dest.path(), "network-recon", &rec).unwrap();
    let f = FixtureFetch::new(src.path(), &["network-recon"], "v1", C1);
    let r = install_pins(
        &[pin("network-recon", C1)],
        dest.path(),
        Layer::Global,
        false,
        &f,
    );
    assert_eq!(
        outcomes(&r),
        vec![(
            "network-recon".into(),
            PinOutcome::Kept(KeptReason::OtherSource)
        )]
    );
    assert!(f.fetches.borrow().is_empty());
}

#[test]
fn an_unreadable_record_fails_the_pin_and_force_replaces_it() {
    let src = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    write_skill(dest.path(), "network-recon", "network-recon", "mine");
    std::fs::create_dir_all(dest.path().join(".sources")).unwrap();
    std::fs::write(dest.path().join(".sources/network-recon.json"), "not json").unwrap();
    let f = FixtureFetch::new(src.path(), &["network-recon"], "v1", C1);
    let failed = install_pins(
        &[pin("network-recon", C1)],
        dest.path(),
        Layer::Global,
        false,
        &f,
    );
    assert!(
        matches!(failed.results[0].1, PinOutcome::Failed(_)),
        "{:?}",
        failed.results[0].1
    );
    assert_eq!(failed.missing().len(), 1);
    assert!(f.fetches.borrow().is_empty());
    let forced = install_pins(
        &[pin("network-recon", C1)],
        dest.path(),
        Layer::Global,
        true,
        &f,
    );
    assert_eq!(
        outcomes(&forced),
        vec![("network-recon".into(), PinOutcome::Replaced)]
    );
    assert_eq!(
        read_record(dest.path(), "network-recon")
            .unwrap()
            .unwrap()
            .commit
            .as_deref(),
        Some(C1)
    );
}

#[test]
fn replacing_a_plain_file_leaves_no_backup_behind() {
    let src = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    std::fs::write(dest.path().join("network-recon"), "a file in the way").unwrap();
    let f = FixtureFetch::new(src.path(), &["network-recon"], "v1", C1);
    let r = install_pins(
        &[pin("network-recon", C1)],
        dest.path(),
        Layer::Global,
        true,
        &f,
    );
    assert_eq!(
        outcomes(&r),
        vec![("network-recon".into(), PinOutcome::Replaced)]
    );
    assert!(dest.path().join("network-recon/SKILL.md").is_file());
    let debris: Vec<_> = std::fs::read_dir(dest.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".replaced-") || n.starts_with(".staging-"))
        .collect();
    assert!(debris.is_empty(), "{debris:?}");
}
