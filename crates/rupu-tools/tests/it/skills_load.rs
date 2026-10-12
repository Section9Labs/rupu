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
