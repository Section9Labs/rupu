//! The customer layer on the real `rupu run` path. Like `policy_lock.rs`,
//! these observe the decision the layer drives — whether a `write_file`
//! call is permitted — rather than a config value, so they fail if any
//! launch-path loader skipped the customer layer.

use crate::ENV_LOCK;
use assert_fs::prelude::*;
use rupu_workspace::{CustomerStore, NewCustomer, ProjectRef};

const WRITE_SCRIPT: &str = r#"
[
  { "AssistantToolUse": { "text": null, "tool_id": "call_1", "tool_name": "write_file", "tool_input": {"path": "out.txt", "content": "written"}, "stop": "tool_use" } },
  { "AssistantText": { "text": "done", "stop": "end_turn" } }
]
"#;

const WRITER_AGENT: &str =
    "---\nname: writer\nprovider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 2\ntools: [write_file]\n---\nyou write files.";

/// `<tmp>/.rupu` (home, with the writer agent and `global_cfg`) and
/// `<tmp>/proj` (with `.rupu/config.toml` = `project_cfg`), the project
/// assigned to customer `acme` whose layer is `customer_cfg`.
fn fixture(
    global_cfg: &str,
    customer_cfg: &str,
    project_cfg: &str,
) -> (assert_fs::TempDir, std::path::PathBuf) {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child(".rupu");
    home.child("agents").create_dir_all().unwrap();
    home.child("agents/writer.md")
        .write_str(WRITER_AGENT)
        .unwrap();
    home.child("config.toml").write_str(global_cfg).unwrap();

    let project = tmp.child("proj");
    project.child(".rupu").create_dir_all().unwrap();
    project
        .child(".rupu/config.toml")
        .write_str(project_cfg)
        .unwrap();

    let store = CustomerStore::new(home.path());
    store
        .create(
            "acme",
            &NewCustomer {
                name: "Acme".into(),
                ..NewCustomer::default()
            },
        )
        .unwrap();
    std::fs::write(store.config_path("acme"), customer_cfg).unwrap();
    store
        .assign("acme", ProjectRef::Path(project.path()))
        .unwrap();
    (tmp, project.path().to_path_buf())
}

/// `ExitCode` has no `PartialEq`; compare the way the other serial tests do.
fn ok(code: std::process::ExitCode) -> bool {
    format!("{code:?}") == format!("{:?}", std::process::ExitCode::from(0))
}

async fn run_writer(home: &std::path::Path, cwd: &std::path::Path) -> std::process::ExitCode {
    std::env::set_var("RUPU_HOME", home);
    std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", WRITE_SCRIPT);
    std::env::set_current_dir(cwd).unwrap();
    Box::pin(rupu_cli::run(vec![
        "rupu".into(),
        "run".into(),
        "writer".into(),
        "go".into(),
    ]))
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_customer_layer_applies_when_the_project_is_silent() {
    let _guard = ENV_LOCK.lock().await;
    let (tmp, project) = fixture(
        "permission_mode = \"bypass\"\n",
        "permission_mode = \"readonly\"\n",
        "",
    );
    let code = run_writer(tmp.child(".rupu").path(), &project).await;
    assert!(
        ok(code),
        "the run must complete; the write must be denied, not skipped"
    );
    assert!(
        !project.join("out.txt").exists(),
        "the customer's readonly mode should have denied write_file"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unlocked_project_key_beats_the_customer() {
    let _guard = ENV_LOCK.lock().await;
    let (tmp, project) = fixture(
        "",
        "permission_mode = \"readonly\"\n",
        "permission_mode = \"bypass\"\n",
    );
    run_writer(tmp.child(".rupu").path(), &project).await;
    assert!(
        project.join("out.txt").exists(),
        "project bypass should win"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_customer_lock_beats_the_project() {
    let _guard = ENV_LOCK.lock().await;
    let (tmp, project) = fixture(
        "",
        "permission_mode = \"readonly\"\n[policy]\nlock = [\"permission_mode\"]\n",
        "permission_mode = \"bypass\"\n",
    );
    let code = run_writer(tmp.child(".rupu").path(), &project).await;
    assert!(
        ok(code),
        "the run must complete; the write must be denied, not skipped"
    );
    assert!(
        !project.join("out.txt").exists(),
        "the repo config overrode a customer-LOCKED permission_mode"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_subdirectory_run_inherits_the_customer() {
    let _guard = ENV_LOCK.lock().await;
    let (tmp, project) = fixture(
        "permission_mode = \"bypass\"\n",
        "permission_mode = \"readonly\"\n",
        "",
    );
    let sub = project.join("src");
    std::fs::create_dir_all(&sub).unwrap();
    let code = run_writer(tmp.child(".rupu").path(), &sub).await;
    assert!(
        ok(code),
        "the run must complete; the write must be denied, not skipped"
    );
    assert!(!sub.join("out.txt").exists() && !project.join("out.txt").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dangling_assignment_fails_the_run() {
    let _guard = ENV_LOCK.lock().await;
    let (tmp, project) = fixture("permission_mode = \"bypass\"\n", "", "");
    std::fs::remove_dir_all(tmp.child(".rupu/customers/acme").path()).unwrap();
    let code = run_writer(tmp.child(".rupu").path(), &project).await;
    assert!(!ok(code), "must not fall back to global config");
    assert!(
        !project.join("out.txt").exists(),
        "no model call may happen"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_malformed_customer_layer_fails_the_run() {
    let _guard = ENV_LOCK.lock().await;
    let (tmp, project) = fixture("permission_mode = \"bypass\"\n", "permission_mode = \n", "");
    let code = run_writer(tmp.child(".rupu").path(), &project).await;
    assert!(!ok(code));
    assert!(!project.join("out.txt").exists());
}

/// The `$HOME` situation: the global home `<tmp>/.rupu` makes
/// `project_root_for(<tmp>/repo)` resolve to `<tmp>`, but the repo (which has
/// no `.rupu/` of its own) is what the customer is assigned to.
#[tokio::test(flavor = "multi_thread")]
async fn a_repo_without_its_own_rupu_dir_uses_its_customer() {
    let _guard = ENV_LOCK.lock().await;
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child(".rupu");
    home.child("agents").create_dir_all().unwrap();
    home.child("agents/writer.md")
        .write_str(WRITER_AGENT)
        .unwrap();
    home.child("config.toml")
        .write_str("permission_mode = \"bypass\"\n")
        .unwrap();
    let repo = tmp.child("repo");
    repo.create_dir_all().unwrap();
    assert!(!repo.child(".rupu").path().exists());

    let store = CustomerStore::new(home.path());
    store
        .create(
            "acme",
            &NewCustomer {
                name: "Acme".into(),
                ..NewCustomer::default()
            },
        )
        .unwrap();
    std::fs::write(
        store.config_path("acme"),
        "permission_mode = \"readonly\"\n",
    )
    .unwrap();
    store.assign("acme", ProjectRef::Path(repo.path())).unwrap();

    let code = run_writer(home.path(), repo.path()).await;
    assert!(
        ok(code),
        "the run must complete; the write must be denied, not skipped"
    );
    assert!(
        !repo.path().join("out.txt").exists(),
        "the repo's customer (readonly) should have denied write_file"
    );
}

const ECHO_SCRIPT: &str = r#"
[
  { "AssistantText": { "text": "step output", "stop": "end_turn" } }
]
"#;

const HELLO_WF: &str =
    "name: hello-wf\nsteps:\n  - id: a\n    agent: echo\n    actions: []\n    prompt: hi\n";

/// `echo` names no provider, so the run takes `default_provider` from config.
const ECHO_AGENT: &str = "---\nname: echo\nmodel: claude-sonnet-4-6\n---\nyou echo.";

async fn run_hello_wf(home: &std::path::Path, cwd: &std::path::Path) -> std::process::ExitCode {
    std::env::set_var("RUPU_HOME", home);
    std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", ECHO_SCRIPT);
    std::env::set_current_dir(cwd).unwrap();
    Box::pin(rupu_cli::run(vec![
        "rupu".into(),
        "workflow".into(),
        "run".into(),
        "hello-wf".into(),
        "--mode".into(),
        "bypass".into(),
    ]))
    .await
}

fn workflow_fixture(customer_cfg: &str) -> (assert_fs::TempDir, std::path::PathBuf) {
    let (tmp, project) = fixture("default_provider = \"anthropic\"\n", customer_cfg, "");
    let home = tmp.child(".rupu");
    home.child("agents/echo.md").write_str(ECHO_AGENT).unwrap();
    home.child("workflows").create_dir_all().unwrap();
    home.child("workflows/hello-wf.yaml")
        .write_str(HELLO_WF)
        .unwrap();
    (tmp, project)
}

/// The first line of the only run's first step transcript.
fn first_step_run_start(home: &std::path::Path) -> String {
    let store = rupu_orchestrator::RunStore::new(home.join("runs"));
    let runs = store.list().unwrap();
    assert_eq!(runs.len(), 1, "expected exactly one workflow run");
    let steps = store.read_step_results(&runs[0].id).unwrap();
    let transcript = std::fs::read_to_string(&steps[0].transcript_path).unwrap();
    transcript.lines().next().unwrap().to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn workflow_run_takes_the_customer_default_provider() {
    let _guard = ENV_LOCK.lock().await;
    let (tmp, project) = workflow_fixture(
        "default_provider = \"anthropic-acme\"\n[providers.anthropic-acme]\nkind = \"anthropic\"\n",
    );
    let code = run_hello_wf(tmp.child(".rupu").path(), &project).await;
    assert!(ok(code), "workflow run failed");
    let run_start = first_step_run_start(tmp.child(".rupu").path());
    assert!(
        run_start.contains("\"provider\":\"anthropic-acme\""),
        "the step should run on the customer's account: {run_start}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn workflow_run_fails_on_a_dangling_assignment() {
    let _guard = ENV_LOCK.lock().await;
    let (tmp, project) = workflow_fixture("");
    std::fs::remove_dir_all(tmp.child(".rupu/customers/acme").path()).unwrap();
    let code = run_hello_wf(tmp.child(".rupu").path(), &project).await;
    assert!(!ok(code), "must not fall back to global config");
}
