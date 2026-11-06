use rupu_cli::run_target::{parse_run_target, RunTarget};
use rupu_scm::{IssueTracker, Platform};

#[test]
fn parses_repo_only_form() {
    let t = parse_run_target("github:section9labs/rupu").unwrap();
    assert_eq!(
        t,
        RunTarget::Repo {
            platform: Platform::Github,
            owner: "section9labs".into(),
            repo: "rupu".into(),
            ref_: None,
        }
    );
}

#[test]
fn parses_pr_form() {
    let t = parse_run_target("github:section9labs/rupu#42").unwrap();
    assert_eq!(
        t,
        RunTarget::Pr {
            platform: Platform::Github,
            owner: "section9labs".into(),
            repo: "rupu".into(),
            number: 42,
        }
    );
}

#[test]
fn parses_issue_form() {
    let t = parse_run_target("github:section9labs/rupu/issues/123").unwrap();
    assert_eq!(
        t,
        RunTarget::Issue {
            tracker: IssueTracker::Github,
            project: "section9labs/rupu".into(),
            number: 123,
        }
    );
}

#[test]
fn parses_gitlab_mr_with_bang() {
    let t = parse_run_target("gitlab:group/sub/project!7").unwrap();
    assert_eq!(
        t,
        RunTarget::Pr {
            platform: Platform::Gitlab,
            owner: "group/sub".into(),
            repo: "project".into(),
            number: 7,
        }
    );
}

#[test]
fn parses_gitlab_repo_nested_namespace() {
    let t = parse_run_target("gitlab:group/sub/project").unwrap();
    assert_eq!(
        t,
        RunTarget::Repo {
            platform: Platform::Gitlab,
            owner: "group/sub".into(),
            repo: "project".into(),
            ref_: None,
        }
    );
}

#[test]
fn parses_gitlab_issue() {
    let t = parse_run_target("gitlab:group/project/issues/9").unwrap();
    assert_eq!(
        t,
        RunTarget::Issue {
            tracker: IssueTracker::Gitlab,
            project: "group/project".into(),
            number: 9,
        }
    );
}

#[test]
fn parses_linear_issue() {
    let t = parse_run_target("linear:team-123/issues/42").unwrap();
    assert_eq!(
        t,
        RunTarget::Issue {
            tracker: IssueTracker::Linear,
            project: "team-123".into(),
            number: 42,
        }
    );
}

#[test]
fn parses_jira_issue() {
    let t = parse_run_target("jira:acme.atlassian.net/ENG/issues/42").unwrap();
    assert_eq!(
        t,
        RunTarget::Issue {
            tracker: IssueTracker::Jira,
            project: "acme.atlassian.net/ENG".into(),
            number: 42,
        }
    );
}

#[test]
fn rejects_unknown_platform() {
    assert!(parse_run_target("bitbucket:foo/bar").is_err());
}

#[test]
fn rejects_missing_separator() {
    assert!(parse_run_target("github-foo-bar").is_err());
}

#[test]
fn rejects_missing_owner() {
    assert!(parse_run_target("github:repo-only").is_err());
}

#[test]
fn rejects_bad_pr_number() {
    assert!(parse_run_target("github:foo/bar#abc").is_err());
}

#[test]
fn rejects_bad_issue_number() {
    assert!(parse_run_target("github:foo/bar/issues/notanumber").is_err());
}

/// A prompt that only starts like a target is not one: a target is a single
/// token with no empty path segment.
#[test]
fn rejects_whitespace_and_empty_segments() {
    for prompt in [
        "github: compare foo/bar",
        "github:compare foo/bar",
        "github:foo/bar and then fix it",
        "github:foo/bar ",
        " github:foo/bar",
        "github:foo/bar\n",
        "github:foo/bar#4 2",
        "github:foo/bar/issues/ 9",
        "gitlab:group/sub project/repo",
        "github:/bar",
        "github:foo/",
        "github:foo//bar",
        "gitlab:group//project!7",
        "github:/issues/3",
        "github:foo//bar/issues/3",
        "jira:acme.atlassian.net//issues/42",
    ] {
        assert!(parse_run_target(prompt).is_err(), "{prompt:?}");
    }
}
