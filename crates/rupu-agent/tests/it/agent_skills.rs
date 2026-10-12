use rupu_agent::{check_agent_files, load_agent, AgentLoadError};
use std::fs;
use std::path::Path;

fn skill(skills_dir: &Path, name: &str) {
    let d = skills_dir.join(name);
    fs::create_dir_all(&d).unwrap();
    fs::write(
        d.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: d\n---\nbody\n"),
    )
    .unwrap();
}

fn agent(global: &Path, body: &str) {
    fs::create_dir_all(global.join("agents")).unwrap();
    fs::write(
        global.join("agents/a.md"),
        format!("---\nname: a\n{body}---\nprompt\n"),
    )
    .unwrap();
}

#[test]
fn load_agent_accepts_installed_skills() {
    let g = tempfile::tempdir().unwrap();
    skill(&g.path().join("skills"), "code-review");
    agent(g.path(), "skills: [code-review]\n");
    assert_eq!(
        load_agent(g.path(), None, "a").unwrap().skills,
        Some(vec!["code-review".into()])
    );
}

#[test]
fn load_agent_refuses_an_unknown_skill_with_a_suggestion() {
    let g = tempfile::tempdir().unwrap();
    skill(&g.path().join("skills"), "code-review");
    agent(g.path(), "preloadSkills: [code-reviw]\n");
    match load_agent(g.path(), None, "a") {
        Err(AgentLoadError::Skill { agent, message }) => {
            assert_eq!(agent, "a");
            assert!(message.contains("did you mean code-review"), "{message}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn project_skills_need_an_exact_grant() {
    let g = tempfile::tempdir().unwrap();
    let p = tempfile::tempdir().unwrap();
    skill(&p.path().join("skills"), "local-only");
    agent(g.path(), "skills: [\"*\", local-only]\n");
    assert!(load_agent(g.path(), Some(p.path()), "a").is_ok());
}

#[test]
fn check_agent_files_reports_skill_errors() {
    let g = tempfile::tempdir().unwrap();
    agent(g.path(), "skills: [missing]\n");
    let checks = check_agent_files(g.path(), None);
    assert!(matches!(
        &checks[0].error,
        Some(AgentLoadError::Skill { .. })
    ));
}
