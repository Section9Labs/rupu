use super::*;
use std::fs;
const SOURCE: &str = "---\nname: analyst\n---\nThe exact global instructions.\n";
fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let global = base.join("home");
    let project = base.join("parent/project");
    fs::create_dir_all(global.join("agents")).unwrap();
    fs::create_dir_all(global.join("workspaces")).unwrap();
    fs::create_dir_all(project.parent().unwrap().join(".rupu/agents")).unwrap();
    fs::create_dir_all(&project).unwrap();
    fs::write(global.join("agents/installed.md"), SOURCE).unwrap();
    fs::write(
        global.join("workspaces/ws_chosen.toml"),
        format!(
            "id = 'ws_chosen'\npath = '{}'\ncreated_at = '2026-11-09T00:00:00Z'\n",
            project.display()
        ),
    )
    .unwrap();
    (tmp, global, project)
}
#[test]
fn exact_file_survives_project_shadow_and_duplicate_names() {
    let (_tmp, global, project) = fixture();
    fs::create_dir_all(project.join(".rupu/agents")).unwrap();
    fs::write(
        project.join(".rupu/agents/analyst.md"),
        "---\nname: analyst\n---\nPROJECT SHADOW\n",
    )
    .unwrap();
    fs::write(
        global.join("agents/other.md"),
        "---\nname: analyst\n---\nGLOBAL DUPLICATE\n",
    )
    .unwrap();
    let reviewed = review(&global, "installed.md", "ws_chosen").unwrap();
    assert_eq!(reviewed.review.raw, SOURCE);
    assert_eq!(reviewed.review.binding.effective_name, "analyst");
    assert_eq!(
        load(&global, &reviewed.review.binding, &project)
            .unwrap()
            .spec
            .system_prompt,
        reviewed.spec.system_prompt
    );
}
#[test]
fn registered_root_without_layer_is_supported_and_layer_creation_invalidates() {
    let (_tmp, global, project) = fixture();
    assert!(project.parent().unwrap().join(".rupu").exists());
    let binding = review(&global, "installed.md", "ws_chosen")
        .unwrap()
        .review
        .binding;
    assert_eq!(binding.project_path, project.to_str().unwrap());
    assert!(load(&global, &binding, &project).is_ok());
    fs::create_dir(project.join(".rupu")).unwrap();
    assert!(load(&global, &binding, &project).is_err());
}
#[test]
fn child_revalidation_refuses_source_and_registry_drift() {
    let (_tmp, global, project) = fixture();
    let binding = review(&global, "installed.md", "ws_chosen")
        .unwrap()
        .review
        .binding;
    fs::write(
        global.join("agents/installed.md"),
        SOURCE.replace("exact", "other"),
    )
    .unwrap();
    assert!(load(&global, &binding, &project).is_err());
    let binding = review(&global, "installed.md", "ws_chosen")
        .unwrap()
        .review
        .binding;
    let registry = global.join("workspaces/ws_chosen.toml");
    fs::write(
        &registry,
        fs::read_to_string(&registry).unwrap() + "last_run_at = 'changed'\n",
    )
    .unwrap();
    assert!(load(&global, &binding, &project).is_err());
}
#[test]
fn wrong_workspace_cwd_and_path_escape_never_fall_back() {
    let (_tmp, global, project) = fixture();
    for key in [
        "../installed.md",
        "/installed.md",
        "x/y.md",
        "installed.md/..",
    ] {
        assert!(review(&global, key, "ws_chosen").is_err());
    }
    assert!(review(&global, "installed.md", "ws_missing").is_err());
    let binding = review(&global, "installed.md", "ws_chosen")
        .unwrap()
        .review
        .binding;
    assert!(load(&global, &binding, project.parent().unwrap()).is_err());
    fs::write(
        global.join("workspaces/ws_chosen.toml"),
        format!("id='ws_other'\npath='{}'\n", project.display()),
    )
    .unwrap();
    assert!(review(&global, "installed.md", "ws_chosen").is_err());
}
#[test]
fn source_symlink_hardlink_fifo_and_directory_are_refused() {
    use std::os::unix::fs::symlink;
    let (_tmp, global, _project) = fixture();
    let path = global.join("agents/installed.md");
    let other = global.join("saved.md");
    fs::rename(&path, &other).unwrap();
    symlink(&other, &path).unwrap();
    assert!(review(&global, "installed.md", "ws_chosen").is_err());
    fs::remove_file(&path).unwrap();
    fs::hard_link(&other, &path).unwrap();
    assert!(review(&global, "installed.md", "ws_chosen").is_err());
    fs::remove_file(&path).unwrap();
    assert!(std::process::Command::new("mkfifo")
        .args(["-m", "600"])
        .arg(&path)
        .status()
        .unwrap()
        .success());
    assert!(review(&global, "installed.md", "ws_chosen").is_err());
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(review(&global, "installed.md", "ws_chosen").is_err());
}
#[test]
fn project_alias_retarget_and_agent_directory_replacement_refuse_old_binding() {
    let (_tmp, global, project) = fixture();
    let binding = review(&global, "installed.md", "ws_chosen")
        .unwrap()
        .review
        .binding;
    fs::rename(&project, project.with_file_name("old_project")).unwrap();
    fs::create_dir(&project).unwrap();
    assert!(load(&global, &binding, &project).is_err());
    let binding = review(&global, "installed.md", "ws_chosen")
        .unwrap()
        .review
        .binding;
    fs::rename(global.join("agents"), global.join("old_agents")).unwrap();
    fs::create_dir(global.join("agents")).unwrap();
    fs::write(global.join("agents/installed.md"), SOURCE).unwrap();
    assert!(load(&global, &binding, &project).is_err());
}
#[test]
fn invalid_reserved_and_oversized_definitions_refuse_review() {
    let (_tmp, global, _) = fixture();
    for raw in [
        "---\nname: [broken\n---\nbody".to_owned(),
        "---\nname: rupu-session\n---\nbody".to_owned(),
        format!("{SOURCE}{}", "x".repeat(MAX_SOURCE)),
    ] {
        fs::write(global.join("agents/installed.md"), raw).unwrap();
        assert!(review(&global, "installed.md", "ws_chosen").is_err());
    }
}
