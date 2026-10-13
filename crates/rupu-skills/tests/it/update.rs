use crate::support::write_skill;
use rupu_skills::install::{install_from, list_source, InstallError, InstallSpec, Selection};
use rupu_skills::record::{list_records, read_record, write_record, SourceRecord};
use rupu_skills::update::{update_skills, UpdateOutcome};
use rupu_skills::{Fetch, FetchedTree, GitSource, Layer, Source, SourceError};
use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;

/// Serves one "repository" from a local directory, with a settable commit.
struct FakeFetch {
    repo: PathBuf,
    commit: RefCell<String>,
    calls: RefCell<usize>,
}

impl Fetch for FakeFetch {
    fn fetch(&self, src: &GitSource) -> Result<FetchedTree, SourceError> {
        *self.calls.borrow_mut() += 1;
        let root = match &src.path {
            Some(p) => self.repo.join(p),
            None => self.repo.clone(),
        };
        Ok(FetchedTree::local_with_commit(
            root,
            Some(self.commit.borrow().clone()),
        ))
    }
}

fn gh(path: Option<&str>) -> Source {
    Source::Git(GitSource {
        url: "https://github.com/o/skills".into(),
        git_ref: None,
        path: path.map(Into::into),
    })
}

fn setup() -> (tempfile::TempDir, tempfile::TempDir, FakeFetch) {
    let repo = tempfile::tempdir().unwrap();
    write_skill(&repo.path().join("skills"), "pdf", "pdf", "v1");
    write_skill(&repo.path().join("skills"), "docx", "docx", "v1");
    let dst = tempfile::tempdir().unwrap();
    let f = FakeFetch {
        repo: repo.path().to_owned(),
        commit: RefCell::new("c1".into()),
        calls: RefCell::new(0),
    };
    (repo, dst, f)
}

#[test]
fn install_from_git_writes_a_record() {
    let (_repo, dst, f) = setup();
    let source = gh(None);
    let report = install_from(
        &InstallSpec {
            source: &source,
            input: "o/skills",
            selection: Selection::Named(vec!["pdf".into()]),
            skills_dir: dst.path(),
            layer: Layer::Global,
            force: false,
        },
        &f,
    )
    .unwrap();
    assert_eq!(report.installed.len(), 1);
    let rec = read_record(dst.path(), "pdf").unwrap().unwrap();
    assert_eq!(rec.source, "o/skills");
    assert_eq!(rec.url.as_deref(), Some("https://github.com/o/skills"));
    assert_eq!(rec.path.as_deref(), Some("skills/pdf"));
    assert_eq!(rec.commit.as_deref(), Some("c1"));
    assert_eq!(rec.content_hash, report.installed[0].content_hash.hex());
    assert!(dst.path().join(".sources/pdf.json").is_file());
}

#[test]
fn list_source_shows_every_skill() {
    let (_repo, _dst, f) = setup();
    let names: Vec<String> = list_source(&gh(None), &f)
        .unwrap()
        .into_iter()
        .map(|x| x.name)
        .collect();
    assert_eq!(names, vec!["docx", "pdf"]);
}

#[test]
fn update_moves_up_to_date_and_respects_local_edits() {
    let (repo, dst, f) = setup();
    let source = gh(None);
    install_from(
        &InstallSpec {
            source: &source,
            input: "o/skills",
            selection: Selection::All,
            skills_dir: dst.path(),
            layer: Layer::Global,
            force: false,
        },
        &f,
    )
    .unwrap();
    *f.calls.borrow_mut() = 0;

    assert_eq!(
        update_skills(dst.path(), Layer::Global, &[], false, &f),
        vec![
            ("docx".into(), UpdateOutcome::UpToDate),
            ("pdf".into(), UpdateOutcome::UpToDate)
        ]
    );
    assert_eq!(*f.calls.borrow(), 1, "one fetch per (url, ref)");

    write_skill(&repo.path().join("skills"), "pdf", "pdf", "v2");
    *f.commit.borrow_mut() = "c2".into();
    let got = update_skills(dst.path(), Layer::Global, &["pdf".into()], false, &f);
    assert_eq!(
        got,
        vec![(
            "pdf".into(),
            UpdateOutcome::Updated {
                from: "c1".into(),
                to: "c2".into()
            }
        )]
    );
    let installed = fs::read_to_string(dst.path().join("pdf/SKILL.md")).unwrap();
    assert!(installed.contains("v2"));
    assert_eq!(
        read_record(dst.path(), "pdf")
            .unwrap()
            .unwrap()
            .commit
            .as_deref(),
        Some("c2")
    );

    fs::write(dst.path().join("docx/scripts/run.sh"), "echo edited\n").unwrap();
    write_skill(&repo.path().join("skills"), "docx", "docx", "v2");
    assert_eq!(
        update_skills(dst.path(), Layer::Global, &["docx".into()], false, &f),
        vec![("docx".into(), UpdateOutcome::LocallyModified)]
    );
    assert!(matches!(
        update_skills(dst.path(), Layer::Global, &["docx".into()], true, &f)[0].1,
        UpdateOutcome::Updated { .. }
    ));
}

#[test]
fn update_reports_unmanaged_and_missing_skills() {
    let (_repo, dst, f) = setup();
    write_skill(dst.path(), "hand-made", "hand-made", "placed by hand");
    assert_eq!(
        update_skills(
            dst.path(),
            Layer::Global,
            &["hand-made".into(), "ghost".into()],
            false,
            &f
        ),
        vec![
            ("hand-made".into(), UpdateOutcome::NoRecord),
            ("ghost".into(), UpdateOutcome::NotInstalled)
        ]
    );
    assert!(
        update_skills(dst.path(), Layer::Global, &[], false, &f).is_empty(),
        "--all covers recorded skills only"
    );
}

#[test]
fn local_sources_update_from_their_directory() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let dir = write_skill(src.path(), "notes", "notes", "v1");
    let source = Source::Local(dir.canonicalize().unwrap());
    let none = FakeFetch {
        repo: PathBuf::new(),
        commit: RefCell::new(String::new()),
        calls: RefCell::new(0),
    };
    install_from(
        &InstallSpec {
            source: &source,
            input: dir.to_str().unwrap(),
            selection: Selection::Default,
            skills_dir: dst.path(),
            layer: Layer::Global,
            force: false,
        },
        &none,
    )
    .unwrap();
    let rec = read_record(dst.path(), "notes").unwrap().unwrap();
    assert_eq!(rec.local, Some(dir.canonicalize().unwrap()));
    assert_eq!(rec.url, None);

    write_skill(src.path(), "notes", "notes", "v2");
    assert!(matches!(
        update_skills(dst.path(), Layer::Global, &[], false, &none)[0].1,
        UpdateOutcome::Updated { .. }
    ));
    assert_eq!(*none.calls.borrow(), 0, "a local source never fetches");
}

#[test]
fn a_forced_update_that_matches_the_edit_adopts_it() {
    let (repo, dst, f) = setup();
    let source = gh(None);
    install_from(
        &InstallSpec {
            source: &source,
            input: "o/skills",
            selection: Selection::Named(vec!["pdf".into()]),
            skills_dir: dst.path(),
            layer: Layer::Global,
            force: false,
        },
        &f,
    )
    .unwrap();
    fs::write(dst.path().join("pdf/scripts/run.sh"), "echo patched\n").unwrap();
    fs::write(
        repo.path().join("skills/pdf/scripts/run.sh"),
        "echo patched\n",
    )
    .unwrap();
    assert_eq!(
        update_skills(dst.path(), Layer::Global, &["pdf".into()], true, &f),
        vec![("pdf".into(), UpdateOutcome::UpToDate)]
    );
    assert_eq!(
        update_skills(dst.path(), Layer::Global, &["pdf".into()], false, &f),
        vec![("pdf".into(), UpdateOutcome::UpToDate)],
        "the record now holds the adopted content"
    );
}

#[test]
fn a_renamed_upstream_skill_is_not_installed_under_its_new_name() {
    let (repo, dst, f) = setup();
    let source = gh(None);
    install_from(
        &InstallSpec {
            source: &source,
            input: "o/skills",
            selection: Selection::Named(vec!["pdf".into()]),
            skills_dir: dst.path(),
            layer: Layer::Global,
            force: false,
        },
        &f,
    )
    .unwrap();
    write_skill(&repo.path().join("skills"), "pdf", "pdf-tools", "renamed");
    let got = update_skills(dst.path(), Layer::Global, &["pdf".into()], false, &f);
    assert!(
        matches!(&got[0].1, UpdateOutcome::Failed(e) if e.contains("pdf-tools")),
        "{got:?}"
    );
    assert!(!dst.path().join("pdf-tools").exists());
    assert!(fs::read_to_string(dst.path().join("pdf/SKILL.md"))
        .unwrap()
        .contains("v1"));
}

fn install_pdf(dst: &std::path::Path, f: &FakeFetch) {
    let source = gh(None);
    install_from(
        &InstallSpec {
            source: &source,
            input: "o/skills",
            selection: Selection::Named(vec!["pdf".into()]),
            skills_dir: dst,
            layer: Layer::Global,
            force: false,
        },
        f,
    )
    .unwrap();
}

fn tamper(dst: &std::path::Path, name: &str, edit: impl FnOnce(&mut SourceRecord)) {
    let mut rec = read_record(dst, name).unwrap().unwrap();
    edit(&mut rec);
    write_record(dst, name, &rec).unwrap();
}

fn installed_description(dst: &std::path::Path) -> String {
    fs::read_to_string(dst.join("pdf/SKILL.md")).unwrap()
}

#[test]
fn a_recorded_path_that_leaves_the_clone_fails_the_update() {
    let (repo, dst, f) = setup();
    install_pdf(dst.path(), &f);
    let elsewhere = tempfile::tempdir().unwrap();
    let evil = write_skill(elsewhere.path(), "pdf", "pdf", "planted");
    let dotdot = format!(
        "../{}/pdf",
        elsewhere.path().file_name().unwrap().to_str().unwrap()
    );
    assert!(repo.path().join(&dotdot).join("SKILL.md").is_file());

    for path in [evil.to_str().unwrap().to_owned(), dotdot] {
        tamper(dst.path(), "pdf", |r| r.path = Some(path.clone()));
        let got = update_skills(dst.path(), Layer::Global, &["pdf".into()], true, &f);
        assert!(
            matches!(&got[0].1, UpdateOutcome::Failed(e) if e.contains("inside the repository")),
            "{path}: {got:?}"
        );
        assert!(installed_description(dst.path()).contains("v1"), "{path}");
    }
}

#[cfg(unix)]
#[test]
fn a_recorded_path_through_a_symlink_fails_the_update() {
    let (repo, dst, f) = setup();
    install_pdf(dst.path(), &f);
    let elsewhere = tempfile::tempdir().unwrap();
    write_skill(elsewhere.path(), "pdf", "pdf", "planted");
    std::os::unix::fs::symlink(elsewhere.path(), repo.path().join("linked")).unwrap();
    tamper(dst.path(), "pdf", |r| r.path = Some("linked/pdf".into()));
    let got = update_skills(dst.path(), Layer::Global, &["pdf".into()], true, &f);
    assert!(
        matches!(&got[0].1, UpdateOutcome::Failed(e) if e.contains("inside the repository")),
        "{got:?}"
    );
    assert!(installed_description(dst.path()).contains("v1"));
}

#[test]
fn a_project_record_may_not_point_outside_the_project() {
    let project = tempfile::tempdir().unwrap();
    let skills_dir = project.path().join(".rupu/skills");
    let inside = write_skill(&project.path().join("vendor"), "notes", "notes", "v1");
    let source = Source::Local(inside.canonicalize().unwrap());
    let none = FakeFetch {
        repo: PathBuf::new(),
        commit: RefCell::new(String::new()),
        calls: RefCell::new(0),
    };
    install_from(
        &InstallSpec {
            source: &source,
            input: "vendor/notes",
            selection: Selection::Default,
            skills_dir: &skills_dir,
            layer: Layer::Project,
            force: false,
        },
        &none,
    )
    .unwrap();
    write_skill(&project.path().join("vendor"), "notes", "notes", "v2");
    assert!(matches!(
        update_skills(&skills_dir, Layer::Project, &[], false, &none)[0].1,
        UpdateOutcome::Updated { .. }
    ));

    let elsewhere = tempfile::tempdir().unwrap();
    let outside = write_skill(elsewhere.path(), "notes", "notes", "planted");
    tamper(&skills_dir, "notes", |r| r.local = Some(outside.clone()));
    let got = update_skills(&skills_dir, Layer::Project, &[], true, &none);
    assert!(
        matches!(&got[0].1, UpdateOutcome::Failed(e) if e.contains("outside the project")),
        "{got:?}"
    );
    assert!(fs::read_to_string(skills_dir.join("notes/SKILL.md"))
        .unwrap()
        .contains("v2"));
}

#[test]
fn a_moved_skill_is_found_by_name_and_its_record_follows() {
    let (repo, dst, f) = setup();
    install_pdf(dst.path(), &f);
    fs::create_dir_all(repo.path().join("tools")).unwrap();
    fs::rename(
        repo.path().join("skills/pdf"),
        repo.path().join("tools/pdf"),
    )
    .unwrap();
    assert_eq!(
        update_skills(dst.path(), Layer::Global, &["pdf".into()], false, &f),
        vec![("pdf".into(), UpdateOutcome::UpToDate)]
    );
    assert_eq!(
        read_record(dst.path(), "pdf")
            .unwrap()
            .unwrap()
            .path
            .as_deref(),
        Some("tools/pdf")
    );
}

#[test]
fn naming_a_skill_twice_installs_and_records_it_once() {
    let (_repo, dst, f) = setup();
    let source = gh(None);
    let report = install_from(
        &InstallSpec {
            source: &source,
            input: "o/skills",
            selection: Selection::Named(vec!["pdf".into(), "pdf".into()]),
            skills_dir: dst.path(),
            layer: Layer::Global,
            force: false,
        },
        &f,
    )
    .unwrap();
    assert_eq!(report.installed.len(), 1);
    assert!(read_record(dst.path(), "pdf").unwrap().is_some());
}

#[cfg(unix)]
#[test]
fn a_failure_mid_batch_keeps_the_earlier_skills_recorded() {
    let (repo, dst, f) = setup();
    let root = write_skill(&repo.path().join("skills"), "b-dir", "b-two", "d");
    std::os::unix::fs::symlink("../b-dir/SKILL.md", root.join("link")).unwrap();
    let source = gh(None);
    let got = install_from(
        &InstallSpec {
            source: &source,
            input: "o/skills",
            selection: Selection::Named(vec!["pdf".into(), "b-two".into()]),
            skills_dir: dst.path(),
            layer: Layer::Global,
            force: false,
        },
        &f,
    );
    let Err(InstallError::Partial { report, error }) = got else {
        panic!("expected a partial install: {got:?}");
    };
    let landed: Vec<&str> = report.installed.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(landed, vec!["pdf"]);
    assert!(
        matches!(*error, InstallError::Invalid { ref name, .. } if name == "b-two"),
        "{error:?}"
    );
    assert!(dst.path().join("pdf/SKILL.md").is_file());
    assert!(read_record(dst.path(), "pdf").unwrap().is_some());
    assert!(!dst.path().join("b-two").exists());

    let first = install_from(
        &InstallSpec {
            source: &source,
            input: "o/skills",
            selection: Selection::Named(vec!["b-two".into(), "docx".into()]),
            skills_dir: dst.path(),
            layer: Layer::Global,
            force: false,
        },
        &f,
    );
    assert!(
        matches!(first, Err(InstallError::Invalid { ref name, .. }) if name == "b-two"),
        "nothing landed, so the error is not partial: {first:?}"
    );
    assert!(!dst.path().join("docx").exists());
}

#[test]
fn a_project_install_from_outside_the_project_is_not_tracked() {
    let project = tempfile::tempdir().unwrap();
    let skills_dir = project.path().join(".rupu/skills");
    let elsewhere = tempfile::tempdir().unwrap();
    let outside = write_skill(elsewhere.path(), "notes", "notes", "v1");
    let source = Source::Local(outside.canonicalize().unwrap());
    let none = FakeFetch {
        repo: PathBuf::new(),
        commit: RefCell::new(String::new()),
        calls: RefCell::new(0),
    };
    let report = install_from(
        &InstallSpec {
            source: &source,
            input: outside.to_str().unwrap(),
            selection: Selection::Default,
            skills_dir: &skills_dir,
            layer: Layer::Project,
            force: false,
        },
        &none,
    )
    .unwrap();
    assert_eq!(report.installed.len(), 1);
    let untracked: Vec<&str> = report.untracked.iter().map(|n| n.as_str()).collect();
    assert_eq!(untracked, vec!["notes"]);
    assert!(skills_dir.join("notes/SKILL.md").is_file());
    assert!(read_record(&skills_dir, "notes").unwrap().is_none());
    assert_eq!(
        update_skills(&skills_dir, Layer::Project, &["notes".into()], false, &none),
        vec![("notes".into(), UpdateOutcome::NoRecord)]
    );

    let global = tempfile::tempdir().unwrap();
    let report = install_from(
        &InstallSpec {
            source: &source,
            input: outside.to_str().unwrap(),
            selection: Selection::Default,
            skills_dir: global.path(),
            layer: Layer::Global,
            force: false,
        },
        &none,
    )
    .unwrap();
    assert!(report.untracked.is_empty());
    assert!(read_record(global.path(), "notes").unwrap().is_some());
}

#[test]
fn update_puts_back_a_skill_an_interrupted_replace_left_aside() {
    let (_repo, dst, f) = setup();
    install_pdf(dst.path(), &f);
    fs::rename(
        dst.path().join("pdf"),
        dst.path().join(".replaced-pdf-4242"),
    )
    .unwrap();
    assert_eq!(
        update_skills(dst.path(), Layer::Global, &["pdf".into()], false, &f),
        vec![("pdf".into(), UpdateOutcome::UpToDate)]
    );
    assert!(dst.path().join("pdf/SKILL.md").is_file());
    assert!(!dst.path().join(".replaced-pdf-4242").exists());
}

#[test]
fn an_invalid_name_fails_without_touching_the_filesystem() {
    let (_repo, root, f) = setup();
    let skills = root.path().join("a/b/skills");
    fs::create_dir_all(&skills).unwrap();
    let outside = write_skill(&root.path().join("a"), "x", "x", "outside");
    let before = fs::read_to_string(outside.join("SKILL.md")).unwrap();

    let got = update_skills(&skills, Layer::Global, &["../../x".into()], true, &f);

    assert_eq!(
        got,
        vec![(
            "../../x".to_owned(),
            UpdateOutcome::Failed(
                "not a valid skill name: skill name may only contain a-z, 0-9 and '-'; \
                 found '.'"
                    .into()
            )
        )]
    );
    assert_eq!(*f.calls.borrow(), 0);
    assert_eq!(
        fs::read_to_string(outside.join("SKILL.md")).unwrap(),
        before
    );
    assert_eq!(
        fs::read_dir(root.path().join("a")).unwrap().count(),
        2,
        "a/ holds only b/ and x/"
    );
}

#[test]
fn list_records_skips_a_record_whose_name_is_invalid() {
    let t = tempfile::tempdir().unwrap();
    let rec = SourceRecord {
        version: 1,
        source: "/somewhere".into(),
        url: None,
        git_ref: None,
        path: None,
        commit: None,
        local: Some("/somewhere".into()),
        content_hash: "h".into(),
        installed_at: 0,
    };
    write_skill(t.path(), "good", "good", "d");
    write_record(t.path(), "good", &rec).unwrap();
    write_skill(t.path(), "Bad_Name", "Bad_Name", "d");
    write_record(t.path(), "Bad_Name", &rec).unwrap();

    assert_eq!(list_records(t.path()), vec!["good".to_owned()]);
}

#[cfg(unix)]
#[test]
fn a_symlinked_record_is_refused_unread() {
    let elsewhere = tempfile::tempdir().unwrap();
    let rec = SourceRecord {
        version: 1,
        source: "o/skills".into(),
        url: None,
        git_ref: None,
        path: None,
        commit: None,
        local: None,
        content_hash: "abc".into(),
        installed_at: 0,
    };
    write_record(elsewhere.path(), "pdf", &rec).unwrap();
    let skills = tempfile::tempdir().unwrap();
    fs::create_dir_all(skills.path().join(".sources")).unwrap();
    std::os::unix::fs::symlink(
        elsewhere.path().join(".sources/pdf.json"),
        skills.path().join(".sources/pdf.json"),
    )
    .unwrap();
    assert!(matches!(
        read_record(skills.path(), "pdf"),
        Err(rupu_skills::record::RecordError::NotAFile { .. })
    ));
}
