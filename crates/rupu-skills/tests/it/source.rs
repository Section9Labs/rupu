use rupu_skills::source::{parse_source, GitSource, Source, SourceError};

fn git(url: &str, git_ref: Option<&str>, path: Option<&str>) -> Source {
    Source::Git(GitSource {
        url: url.into(),
        git_ref: git_ref.map(Into::into),
        path: path.map(Into::into),
    })
}

#[test]
fn local_paths() {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("skills/x")).unwrap();
    let canon = t.path().join("skills/x").canonicalize().unwrap();
    let p = t.path().join("skills/x");
    assert_eq!(
        parse_source(p.to_str().unwrap(), None, None).unwrap(),
        Source::Local(canon.clone())
    );
    assert_eq!(
        parse_source("~/skills/x", None, Some(t.path())).unwrap(),
        Source::Local(canon)
    );
    assert!(matches!(
        parse_source("./definitely-not-here", None, None),
        Err(SourceError::NotFound(_))
    ));
}

#[test]
fn github_shorthand_and_urls() {
    assert_eq!(
        parse_source("anthropics/skills", None, None).unwrap(),
        git("https://github.com/anthropics/skills", None, None)
    );
    assert_eq!(
        parse_source("anthropics/skills#skills/pdf", None, None).unwrap(),
        git(
            "https://github.com/anthropics/skills",
            None,
            Some("skills/pdf")
        )
    );
    assert_eq!(
        parse_source("https://github.com/anthropics/skills", Some("v2"), None).unwrap(),
        git("https://github.com/anthropics/skills", Some("v2"), None)
    );
    assert_eq!(
        parse_source(
            "https://github.com/anthropics/skills/tree/main/skills/pdf",
            None,
            None
        )
        .unwrap(),
        git(
            "https://github.com/anthropics/skills",
            Some("main"),
            Some("skills/pdf")
        )
    );
    assert_eq!(
        parse_source(
            "https://github.com/o/r/blob/v1/skills/pdf/SKILL.md",
            None,
            None
        )
        .unwrap(),
        git("https://github.com/o/r", Some("v1"), Some("skills/pdf"))
    );
    assert_eq!(
        parse_source(
            "https://gitlab.example.com/team/skills.git#tools/lint",
            None,
            None
        )
        .unwrap(),
        git(
            "https://gitlab.example.com/team/skills.git",
            None,
            Some("tools/lint")
        )
    );
}

#[test]
fn refusals() {
    assert!(matches!(
        parse_source("git@github.com:o/r.git", None, None),
        Err(SourceError::UnsupportedScheme(_))
    ));
    assert!(matches!(
        parse_source("ssh://github.com/o/r", None, None),
        Err(SourceError::UnsupportedScheme(_))
    ));
    assert!(matches!(
        parse_source("file:///tmp/r", None, None),
        Err(SourceError::UnsupportedScheme(_))
    ));
    assert!(matches!(
        parse_source("http://github.com/o/r", None, None),
        Err(SourceError::UnsupportedScheme(_))
    ));
    assert!(matches!(
        parse_source("o/r#../escape", None, None),
        Err(SourceError::BadPath(_))
    ));
    assert!(matches!(
        parse_source("o/r#/abs", None, None),
        Err(SourceError::BadPath(_))
    ));
    assert!(matches!(
        parse_source("https://github.com/o/r/tree/main/x", Some("dev"), None),
        Err(SourceError::RefConflict { .. })
    ));
    assert_eq!(
        parse_source("https://github.com/o/r/tree/main/x", Some("main"), None).unwrap(),
        git("https://github.com/o/r", Some("main"), Some("x"))
    );
    assert!(matches!(
        parse_source("just-a-word", None, None),
        Err(SourceError::Unrecognized(_))
    ));
}
