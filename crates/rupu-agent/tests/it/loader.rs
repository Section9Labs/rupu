use assert_fs::prelude::*;
use rupu_agent::loader::load_agents;

fn write_agent(dir: &assert_fs::fixture::ChildPath, name: &str, body: &str) {
    dir.create_dir_all().unwrap();
    dir.child(format!("{name}.md")).write_str(body).unwrap();
}

const HELLO: &str = "---\nname: hello\n---\nyou are hello\n";
const HELLO2: &str = "---\nname: hello\n---\nyou are HELLO TWO\n";
const ONLY_GLOBAL: &str = "---\nname: only-global\n---\ng\n";
const ONLY_PROJECT: &str = "---\nname: only-project\n---\np\n";

#[test]
fn project_shadows_global_by_name() {
    let global = assert_fs::TempDir::new().unwrap();
    let project = assert_fs::TempDir::new().unwrap();
    write_agent(&global.child("agents"), "hello", HELLO);
    write_agent(&project.child("agents"), "hello", HELLO2);

    let agents = load_agents(global.path(), Some(project.path())).unwrap();
    let hello = agents.iter().find(|a| a.name == "hello").unwrap();
    assert!(hello.system_prompt.contains("HELLO TWO"));
}

#[test]
fn unique_in_each_layer_both_present() {
    let global = assert_fs::TempDir::new().unwrap();
    let project = assert_fs::TempDir::new().unwrap();
    write_agent(&global.child("agents"), "only-global", ONLY_GLOBAL);
    write_agent(&project.child("agents"), "only-project", ONLY_PROJECT);

    let agents = load_agents(global.path(), Some(project.path())).unwrap();
    let names: Vec<_> = agents.iter().map(|a| a.name.as_str()).collect();
    assert!(names.contains(&"only-global"));
    assert!(names.contains(&"only-project"));
}

#[test]
fn missing_global_dir_is_ok() {
    let global = assert_fs::TempDir::new().unwrap(); // exists but no agents/ subdir
    let project = assert_fs::TempDir::new().unwrap();
    write_agent(&project.child("agents"), "p", "---\nname: p\n---\nx\n");
    let agents = load_agents(global.path(), Some(project.path())).unwrap();
    assert_eq!(agents.len(), 1);
}

#[test]
fn parse_error_includes_path() {
    let global = assert_fs::TempDir::new().unwrap();
    global.child("agents").create_dir_all().unwrap();
    global
        .child("agents/bad.md")
        .write_str("no frontmatter at all")
        .unwrap();
    let res = load_agents(global.path(), None);
    let err = res.unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("bad.md"), "error should reference path: {msg}");
}
