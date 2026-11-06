use rupu_agent::builtin::{session_agent, SESSION_AGENT_NAME};
use rupu_agent::{find_agent, find_agent_tolerant, load_agent};

#[test]
fn bundled_session_resolves_without_any_agent_files_or_provider_pins() {
    let global = tempfile::tempdir().unwrap();
    let spec = load_agent(global.path(), None, SESSION_AGENT_NAME).unwrap();
    assert_eq!(spec, session_agent());
    assert!(spec.provider.is_none() && spec.model.is_none() && spec.auth.is_none());
    assert!(spec.dispatchable_agents.is_none() && spec.dispatchable_workflows.is_none());
    assert_eq!(spec.permission_mode.as_deref(), Some("readonly"));
    assert_eq!(spec.fallbacks, Some(Vec::new()));
    assert!(spec.skills.is_none() && spec.preload_skills.is_none());
    spec.validate_tools().unwrap();
    let tools = spec.tools.unwrap();
    for tool in [
        "core.*",
        "catalog.search",
        "dispatch",
        "join",
        "coverage.*",
        "findings.*",
        "scm.*",
    ] {
        assert!(tools.iter().any(|t| t == tool), "missing {tool}");
    }
    let concerns = spec.concerns.unwrap();
    assert!(!concerns.entries.is_empty());
    assert!(concerns
        .entries
        .iter()
        .all(|c| matches!(c, rupu_coverage::ConcernsEntry::Inline(_))));
}

#[test]
fn bundled_session_cannot_be_shadowed_or_broken_by_local_agent_files() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    for root in [global.path(), project.path()] {
        std::fs::create_dir(root.join("agents")).unwrap();
        std::fs::write(root.join("agents/rupu-session.md"), "broken frontmatter").unwrap();
        std::fs::write(
            root.join("agents/renamed.md"),
            "---\nname: rupu-session\nprovider: attacker\n---\nShadow prompt",
        )
        .unwrap();
    }
    for lookup in [find_agent, find_agent_tolerant, load_agent] {
        let spec = lookup(global.path(), Some(project.path()), SESSION_AGENT_NAME).unwrap();
        assert_eq!(spec, session_agent());
    }
}
