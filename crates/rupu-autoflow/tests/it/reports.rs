//! What a directory sees: `show`, `explain` and the run precheck over a
//! `RUPU_HOME` holding one global autoflow bound to `[autoflow].repo`.

use rupu_autoflow::{explain_report, refusal_of, run_precheck, show_report, Refusal};
use std::path::{Path, PathBuf};

const FIX: &str = r#"name: fix
autoflow:
  enabled: true
  priority: 100
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: hi
"#;

/// `(home, global, pwd)`: a global layer with the `fix` autoflow, bound to
/// `github:acme/widgets`, seen from a directory with no project.
fn home() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("home");
    std::fs::create_dir_all(global.join("workflows")).unwrap();
    std::fs::write(global.join("workflows/fix.yaml"), FIX).unwrap();
    std::fs::write(
        global.join("config.toml"),
        "[autoflow]\nrepo = \"github:acme/widgets\"\n",
    )
    .unwrap();
    let pwd = tmp.path().join("elsewhere");
    std::fs::create_dir_all(&pwd).unwrap();
    (tmp, global, pwd)
}

fn refusal(e: anyhow::Error) -> Refusal {
    refusal_of(&e)
        .cloned()
        .unwrap_or_else(|| panic!("not a refusal: {e:#}"))
}

#[test]
fn show_reports_the_visible_autoflow_with_its_body() {
    let (_tmp, global, pwd) = home();
    let report = serde_json::to_value(show_report(&global, &pwd, "fix", None).unwrap()).unwrap();
    assert_eq!(report["kind"], "autoflow_show");
    let item = &report["item"];
    assert_eq!(item["name"], "fix");
    assert_eq!(item["scope"], "global");
    assert_eq!(item["repo"], "github:acme/widgets");
    assert_eq!(item["source"], "github:acme/widgets");
    assert_eq!(item["priority"], 100);
    assert_eq!(item["workspace"], "worktree");
    assert_eq!(item["body"], FIX);
}

#[test]
fn show_of_an_unknown_name_or_repo_is_not_found() {
    let (_tmp, global, pwd) = home();
    assert_eq!(
        refusal(show_report(&global, &pwd, "nope", None).unwrap_err()),
        Refusal::NotFound("no autoflow named `nope` is visible".into())
    );
    assert_eq!(
        refusal(show_report(&global, &pwd, "fix", Some("github:acme/other")).unwrap_err()),
        Refusal::NotFound("no autoflow named `fix` is visible for repo `github:acme/other`".into())
    );
}

#[test]
fn explain_of_an_unclaimed_issue_names_its_candidates() {
    let (_tmp, global, pwd) = home();
    let report = explain_report(
        &global,
        &pwd,
        "github:acme/widgets/issues/7",
        None,
        &rupu_config::PricingConfig::default(),
    )
    .unwrap();
    let report = serde_json::to_value(report).unwrap();
    assert_eq!(report["kind"], "autoflow_explain");
    let item = &report["item"];
    assert_eq!(item["issue"], "github:acme/widgets/issues/7");
    assert_eq!(item["repo"], "github:acme/widgets");
    assert_eq!(item["status"], "unclaimed");
    assert_eq!(item["candidate_workflows"], serde_json::json!(["fix"]));
    assert!(item["claim"].is_null());
}

#[test]
fn explain_takes_the_full_form_only() {
    let (_tmp, global, pwd) = home();
    for bare in ["7", "#7", "acme/widgets#7"] {
        let err = explain_report(
            &global,
            &pwd,
            bare,
            None,
            &rupu_config::PricingConfig::default(),
        )
        .unwrap_err();
        assert!(
            matches!(refusal(err), Refusal::Invalid(_)),
            "{bare} is refused"
        );
    }
}

#[test]
fn the_run_precheck_refuses_a_non_issue_and_an_unknown_autoflow() {
    let (_tmp, global, pwd) = home();
    let check = |name: &str, target: &str| run_precheck(&global, &pwd, name, target, None, None);
    assert!(matches!(
        refusal(check("fix", "github:acme/widgets").unwrap_err()),
        Refusal::Invalid(_)
    ));
    assert_eq!(
        refusal(check("nope", "github:acme/widgets/issues/7").unwrap_err()),
        Refusal::NotFound(
            "no autoflow named `nope` is visible for issue `github:acme/widgets/issues/7`".into()
        )
    );
}

/// The project layer is the `pwd`'s: a directory inside a project sees the
/// project's autoflows, bound to the project's configured repo.
#[test]
fn a_project_directory_sees_its_project_autoflows() {
    let (tmp, global, _pwd) = home();
    let project = tmp.path().join("proj");
    let workflows = project.join(".rupu/workflows");
    std::fs::create_dir_all(&workflows).unwrap();
    std::fs::write(
        workflows.join("local.yaml"),
        FIX.replace("name: fix", "name: local"),
    )
    .unwrap();
    std::fs::write(
        project.join(".rupu/config.toml"),
        "[autoflow]\nrepo = \"github:acme/proj\"\n",
    )
    .unwrap();
    let inside = project.join("src");
    std::fs::create_dir_all(&inside).unwrap();
    let names = |pwd: &Path| {
        rupu_autoflow::visible::visible_autoflows(&global, pwd)
            .unwrap()
            .into_iter()
            .map(|e| (e.name, e.scope, e.repo_ref))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        names(&inside),
        vec![
            (
                "fix".into(),
                "global".into(),
                Some("github:acme/proj".into())
            ),
            (
                "local".into(),
                "project".into(),
                Some("github:acme/proj".into())
            ),
        ]
    );
}
