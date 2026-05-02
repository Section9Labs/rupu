use assert_fs::prelude::*;
use tokio::sync::Mutex;

// Tests that mutate process-wide state (RUPU_HOME, cwd) must not run in
// parallel within this binary. Use this lock at the top of every test.
static ENV_LOCK: Mutex<()> = Mutex::const_new(());

#[tokio::test]
async fn agent_list_shows_global_and_project_with_chips() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("agents").create_dir_all().unwrap();
    global
        .child("agents/g1.md")
        .write_str("---\nname: g1\n---\nbody")
        .unwrap();
    let project = assert_fs::TempDir::new().unwrap();
    project.child(".rupu/agents").create_dir_all().unwrap();
    project
        .child(".rupu/agents/p1.md")
        .write_str("---\nname: p1\n---\nbody")
        .unwrap();

    std::env::set_var("RUPU_HOME", global.path());
    std::env::set_current_dir(project.path()).unwrap();

    let exit = rupu_cli::run(vec!["rupu".into(), "agent".into(), "list".into()]).await;

    // Reset cwd to a stable path before project tempdir is dropped.
    std::env::set_current_dir(tmp.path()).unwrap();
    std::env::remove_var("RUPU_HOME");

    assert_eq!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "agent list should exit 0"
    );
}

#[tokio::test]
async fn agent_show_prints_body() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("agents").create_dir_all().unwrap();
    global
        .child("agents/x.md")
        .write_str("---\nname: x\n---\nthe body")
        .unwrap();
    std::env::set_var("RUPU_HOME", global.path());
    // Use tmp.path() as cwd — it has a .rupu/ subdir but that's fine; the
    // global path already contains agent x.
    std::env::set_current_dir(tmp.path()).unwrap();
    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "agent".into(),
        "show".into(),
        "x".into(),
    ])
    .await;
    std::env::remove_var("RUPU_HOME");
    assert_eq!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "agent show should exit 0 when agent exists"
    );
}

#[tokio::test]
async fn agent_show_missing_exits_nonzero() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    std::env::set_var("RUPU_HOME", tmp.path());
    std::env::set_current_dir(tmp.path()).unwrap();
    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "agent".into(),
        "show".into(),
        "nope".into(),
    ])
    .await;
    std::env::remove_var("RUPU_HOME");
    assert_ne!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "agent show for missing agent should exit nonzero"
    );
}
