use rupu_agent::{
    check_agent_files, check_agent_skills, load_agent, AgentLoadError, SkillCheckError,
};
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
    let err = load_agent(g.path(), None, "a").unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "agent `a`: preloadSkills: unknown skill \"code-reviw\" (did you mean code-review?) \
             (looked in {}; install it with `rupu skill install`)",
            g.path().join("skills").display()
        )
    );
}

fn load_err(global: &Path, project: Option<&Path>) -> String {
    match load_agent(global, project, "a") {
        Err(e @ AgentLoadError::Skill { .. }) => e.to_string(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_malformed_entry_names_its_key_once() {
    let g = tempfile::tempdir().unwrap();
    agent(g.path(), "skills: [\"Bad Name\"]\n");
    assert_eq!(
        load_err(g.path(), None),
        "agent `a`: skills: entry \"Bad Name\": skill name may only contain a-z, 0-9 and '-'; found 'B'"
    );
    agent(g.path(), "preloadSkills: [\"Bad Name\"]\n");
    assert_eq!(
        load_err(g.path(), None),
        "agent `a`: preloadSkills: entry \"Bad Name\": skill name may only contain a-z, 0-9 and '-'; found 'B'"
    );
}

#[test]
fn an_error_from_both_keys_is_reported_under_skills() {
    let g = tempfile::tempdir().unwrap();
    let p = tempfile::tempdir().unwrap();
    agent(g.path(), "skills: [nope]\npreloadSkills: [also-nope]\n");
    assert_eq!(
        load_err(g.path(), Some(p.path())),
        format!(
            "agent `a`: skills: unknown skill \"nope\"; unknown skill \"also-nope\" \
             (looked in {} and {}; install it with `rupu skill install`)",
            g.path().join("skills").display(),
            p.path().join("skills").display()
        )
    );
}

/// Loading an agent checks only that its skills are installed: whether
/// each is valid, and whether the set binds a name twice, is the run
/// assembler's to say, so a launch reads no skill twice.
/// `rupu agent validate` still resolves them in full.
#[test]
fn a_load_checks_names_and_validate_resolves() {
    let g = tempfile::tempdir().unwrap();
    let p = tempfile::tempdir().unwrap();
    skill(&g.path().join("skills"), "x");
    skill(&p.path().join("skills"), "x");
    let invalid = g.path().join("skills/invalid");
    fs::create_dir_all(&invalid).unwrap();
    fs::write(invalid.join("SKILL.md"), "no frontmatter\n").unwrap();
    agent(
        g.path(),
        "skills: [\"global:x\", invalid]\npreloadSkills: [\"project:x\"]\n",
    );
    assert!(load_agent(g.path(), Some(p.path()), "a").is_ok());
    let checks = check_agent_files(g.path(), Some(p.path()));
    let err = checks[0].error.as_ref().unwrap().to_string();
    assert!(
        err.starts_with("agent `a`: skills: skill \"invalid\" (global) is invalid"),
        "{err}"
    );
    assert!(
        err.ends_with(
            "skill \"x\" is granted from both the global and the project layer; \
             grant only one of global:x or project:x"
        ),
        "{err}"
    );
}

#[test]
fn a_skills_path_that_is_a_file_fails_only_agents_with_skill_keys() {
    let g = tempfile::tempdir().unwrap();
    fs::write(g.path().join("skills"), "not a directory").unwrap();
    agent(g.path(), "");
    assert!(load_agent(g.path(), None, "a").is_ok());
    agent(g.path(), "skills: [x]\n");
    let err = load_agent(g.path(), None, "a").unwrap_err();
    assert!(
        matches!(
            &err,
            AgentLoadError::Skill {
                key: "skills",
                source: SkillCheckError::Discover(rupu_skills::DiscoverError::Io { path, .. }),
                ..
            } if *path == g.path().join("skills")
        ),
        "{err:?}"
    );
}

/// A listing shares one discovery: a failed one is reported for every
/// agent that declares skills, without reading the directory again.
#[test]
fn a_shared_check_discovers_once_even_when_it_fails() {
    let g = tempfile::tempdir().unwrap();
    fs::write(g.path().join("skills"), "not a directory").unwrap();
    let dirs = rupu_skills::LayerDirs::for_layers(g.path(), None);
    let spec = rupu_agent::AgentSpec::parse("---\nname: a\nskills: [x]\n---\nhi\n").unwrap();
    let mut cache = None;
    let first = check_agent_skills(&spec, &dirs, &mut cache)
        .unwrap_err()
        .to_string();

    fs::remove_file(g.path().join("skills")).unwrap();
    skill(&g.path().join("skills"), "x");
    let again = check_agent_skills(&spec, &dirs, &mut cache)
        .unwrap_err()
        .to_string();
    assert_eq!(again, first);
    assert!(check_agent_skills(&spec, &dirs, &mut None).is_ok());
}

/// `check_agent_skills` loads each granted skill, as `rupu agent validate`
/// does: an installed skill that doesn't load fails it.
#[test]
fn check_agent_skills_loads_the_granted_skills() {
    let g = tempfile::tempdir().unwrap();
    let d = g.path().join("skills/big");
    fs::create_dir_all(&d).unwrap();
    let huge = "a".repeat(rupu_skills::MAX_SKILL_MD_BYTES as usize);
    fs::write(
        d.join("SKILL.md"),
        format!("---\nname: big\ndescription: d\n---\n{huge}\n"),
    )
    .unwrap();
    let dirs = rupu_skills::LayerDirs::for_layers(g.path(), None);
    let spec = rupu_agent::AgentSpec::parse("---\nname: a\nskills: [big]\n---\nhi\n").unwrap();
    let err = check_agent_skills(&spec, &dirs, &mut None)
        .unwrap_err()
        .to_string();
    assert!(
        err.starts_with("agent `a`: skills: skill \"big\" (global) is invalid: SKILL.md is "),
        "{err}"
    );
    assert!(err.ends_with("bytes; the limit is 1048576 bytes"), "{err}");
}

#[cfg(unix)]
#[test]
fn a_broken_skill_the_agent_does_not_name_never_fails_it() {
    let g = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    skill(&g.path().join("skills"), "healthy");
    skill(&g.path().join("skills"), "broken");
    std::os::unix::fs::symlink(outside.path(), g.path().join("skills/broken/escape")).unwrap();
    agent(
        g.path(),
        "skills: [healthy]\npreloadSkills: [\"global:healthy\"]\n",
    );
    assert!(load_agent(g.path(), None, "a").is_ok());
    agent(g.path(), "skills: [broken]\n");
    let checks = check_agent_files(g.path(), None);
    let err = checks[0].error.as_ref().unwrap().to_string();
    assert!(err.contains("points outside the skill"), "{err}");
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

#[test]
fn load_agent_in_accepts_a_skill_that_exists_only_in_the_overlay() {
    let g = tempfile::tempdir().unwrap();
    let shipped = tempfile::tempdir().unwrap();
    skill(shipped.path(), "pdf");
    agent(g.path(), "skills: [pdf]\n");
    let overlay = vec![rupu_skills::OverlaySkill {
        layer: rupu_skills::Layer::Global,
        name: "pdf".into(),
        dir: shipped.path().join("pdf"),
    }];
    assert!(matches!(
        load_agent(g.path(), None, "a"),
        Err(AgentLoadError::Skill { .. })
    ));
    assert_eq!(
        rupu_agent::load_agent_in(g.path(), None, "a", &overlay)
            .unwrap()
            .skills,
        Some(vec!["pdf".into()])
    );
}
