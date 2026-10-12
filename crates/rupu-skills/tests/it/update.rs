use crate::support::write_skill;
use rupu_skills::install::{install_from, list_source, InstallSpec, Selection};
use rupu_skills::record::read_record;
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
