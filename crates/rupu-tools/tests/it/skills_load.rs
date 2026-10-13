use crate::support::resolved;
use rupu_tools::{Tool, ToolContext};
use serde_json::json;
use std::fs;

fn install(global: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    let d = global.join("skills").join(name);
    fs::create_dir_all(d.join("references")).unwrap();
    fs::write(
        d.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: d\nallowed-tools: Bash(jq:*) Read\n---\n{body}\n"),
    )
    .unwrap();
    fs::write(d.join("references/REF.md"), "ref\n").unwrap();
    d
}

fn ctx(global: &std::path::Path, grants: &[&str]) -> ToolContext {
    let mut c = ToolContext::in_workspace(global);
    c.services.skills = Some(resolved(global, grants));
    c
}

#[tokio::test]
async fn loads_a_granted_skill() {
    let g = tempfile::tempdir().unwrap();
    let dir = install(g.path(), "pdf", "Step one.");
    let out = rupu_tools::skills::SkillsLoadTool
        .invoke(json!({"name": "pdf"}), &ctx(g.path(), &["pdf"]))
        .await
        .unwrap();
    assert_eq!(out.error, None);
    assert!(
        out.stdout.starts_with("Skill: pdf (version "),
        "{}",
        out.stdout
    );
    assert!(out.stdout.contains(&format!(
        "Base directory: {}",
        dir.canonicalize().unwrap().display()
    )));
    assert!(out.stdout.contains("Files: references/REF.md"));
    assert!(out.stdout.ends_with("Step one."));
    let s = out.structured.unwrap();
    assert_eq!(s["skill"], "pdf");
    assert_eq!(s["layer"], "global");
    assert_eq!(s["changed"], false);
    assert_eq!(s["allowed_tools"], json!(["bash", "read_file"]));
    assert_eq!(s["content_hash"].as_str().unwrap().len(), 64);
}

#[tokio::test]
async fn an_ungranted_skill_is_refused_with_the_granted_names() {
    let g = tempfile::tempdir().unwrap();
    install(g.path(), "pdf", "x");
    install(g.path(), "docx", "x");
    let out = rupu_tools::skills::SkillsLoadTool
        .invoke(json!({"name": "docx"}), &ctx(g.path(), &["pdf"]))
        .await
        .unwrap();
    let err = out.error.unwrap();
    assert!(err.contains("docx") && err.contains("pdf"), "{err}");
}

#[tokio::test]
async fn an_edited_skill_comes_back_current_with_a_note() {
    let g = tempfile::tempdir().unwrap();
    let dir = install(g.path(), "pdf", "Old.");
    let c = ctx(g.path(), &["pdf"]);
    fs::write(
        dir.join("SKILL.md"),
        "---\nname: pdf\ndescription: d\n---\nNew text that is longer.\n",
    )
    .unwrap();
    let out = rupu_tools::skills::SkillsLoadTool
        .invoke(json!({"name": "pdf"}), &c)
        .await
        .unwrap();
    assert!(
        out.stdout.starts_with(rupu_skills::render::CHANGED_NOTE),
        "{}",
        out.stdout
    );
    assert!(out.stdout.ends_with("New text that is longer."));
    assert_eq!(out.structured.unwrap()["changed"], true);
}

/// Spec §3.2: granted with `disable-model-invocation: true` and not
/// preloaded, a skill is reachable only by an operator, never by the model.
#[tokio::test]
async fn a_granted_skill_hidden_from_the_model_is_refused() {
    let g = tempfile::tempdir().unwrap();
    install(g.path(), "pdf", "x");
    let dir = install(g.path(), "deploy", "x");
    fs::write(
        dir.join("SKILL.md"),
        "---\nname: deploy\ndescription: d\ndisable-model-invocation: true\n---\nShip.\n",
    )
    .unwrap();
    let c = ctx(g.path(), &["pdf", "deploy"]);
    for name in ["deploy", "docx"] {
        let out = rupu_tools::skills::SkillsLoadTool
            .invoke(json!({ "name": name }), &c)
            .await
            .unwrap();
        assert_eq!(
            out.error.as_deref(),
            Some(
                format!("no skill named {name:?} is available to this run; available: pdf")
                    .as_str()
            ),
        );
    }
}

/// The run's read roots are fixed at start, so a global skill whose symlink
/// now points elsewhere would hand the model a base directory it can't read.
#[cfg(unix)]
#[tokio::test]
async fn a_skill_moved_on_disk_since_the_run_started_fails_with_the_reason() {
    let g = tempfile::tempdir().unwrap();
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (a, b) = (
        install(a.path(), "pdf", "A."),
        install(b.path(), "pdf", "B."),
    );
    fs::create_dir_all(g.path().join("skills")).unwrap();
    let link = g.path().join("skills/pdf");
    std::os::unix::fs::symlink(&a, &link).unwrap();
    let c = ctx(g.path(), &["pdf"]);
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&b, &link).unwrap();
    let out = rupu_tools::skills::SkillsLoadTool
        .invoke(json!({"name": "pdf"}), &c)
        .await
        .unwrap();
    let now = b.canonicalize().unwrap();
    assert_eq!(
        out.error.as_deref(),
        Some(
            format!(
                "skill \"pdf\" moved on disk since this run started (now at {}); its files can't be read in this run",
                now.display()
            )
            .as_str()
        ),
    );
}

#[tokio::test]
async fn a_skill_deleted_mid_run_fails() {
    let g = tempfile::tempdir().unwrap();
    let dir = install(g.path(), "pdf", "x");
    let c = ctx(g.path(), &["pdf"]);
    fs::remove_dir_all(&dir).unwrap();
    let out = rupu_tools::skills::SkillsLoadTool
        .invoke(json!({"name": "pdf"}), &c)
        .await
        .unwrap();
    let err = out.error.unwrap();
    assert!(
        err.starts_with("skill \"pdf\" can no longer be loaded: "),
        "{err}"
    );
    assert_eq!(out.stdout, "");
}

/// A session's turn rebuilds its skills from the frozen set: the granted
/// skill is the live one, so `changed` must compare against the baseline
/// the session's index recorded, not the stamp this turn read.
#[tokio::test]
async fn changed_is_judged_against_the_frozen_baseline() {
    let g = tempfile::tempdir().unwrap();
    let dir = install(g.path(), "pdf", "Old.");
    let frozen = resolved(g.path(), &["pdf"]).freeze();
    fs::write(
        dir.join("SKILL.md"),
        "---\nname: pdf\ndescription: d\n---\nNew text that is longer.\n",
    )
    .unwrap();
    let cat =
        rupu_skills::SkillCatalog::discover(&rupu_skills::LayerDirs::for_layers(g.path(), None))
            .unwrap();
    let (skills, missing) = rupu_skills::run::resolve_frozen(&cat, &frozen);
    assert!(missing.is_empty());
    let mut c = ToolContext::in_workspace(g.path());
    c.services.skills = Some(std::sync::Arc::new(skills));
    let out = rupu_tools::skills::SkillsLoadTool
        .invoke(json!({"name": "pdf"}), &c)
        .await
        .unwrap();
    assert!(
        out.stdout.starts_with(rupu_skills::render::CHANGED_NOTE),
        "{}",
        out.stdout
    );
    assert_eq!(out.structured.unwrap()["changed"], true);
}
