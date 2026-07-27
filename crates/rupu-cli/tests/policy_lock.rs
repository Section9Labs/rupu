//! `[policy].lock` enforcement on the CLI's own config-loading paths.
//!
//! Regression tests for ISSUES.md **I-7**, where lock enforcement lived only
//! inside `rupu_config::resolve()` (6 call sites, all in rupu-cp) while every
//! CLI path loaded through `rupu_config::layer_files`, which performs plain
//! project-over-global layering and never consults `[policy].lock`.
//!
//! The first two tests deliberately drive a **real CLI command** (`rupu run`)
//! and observe the security decision the lock is supposed to protect — whether
//! a write tool is permitted — rather than asserting on the config crate. A
//! library-only assertion would pass even with every CLI call site still on
//! `layer_files`.

use assert_fs::prelude::*;
use tokio::sync::Mutex;

/// These tests mutate process-global state (`RUPU_HOME`, cwd), so they may
/// not overlap.
static ENV_LOCK: Mutex<()> = Mutex::const_new(());

/// Turn 0 asks to write a file; turn 1 ends the run. Under `bypass` the file
/// lands on disk; under `readonly` the `write_file` call is denied
/// (`ReadonlyDecider`) and the run still completes normally.
const WRITE_SCRIPT: &str = r#"
[
  { "AssistantToolUse": { "text": null, "tool_id": "call_1", "tool_name": "write_file", "tool_input": {"path": "locked.txt", "content": "written"}, "stop": "tool_use" } },
  { "AssistantText": { "text": "done", "stop": "end_turn" } }
]
"#;

const WRITER_AGENT: &str =
    "---\nname: writer\nprovider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 2\ntools: [write_file]\n---\nyou write files.";

/// Set up `<tmp>/.rupu` (global) + `<tmp>/proj/.rupu` (project) with the given
/// config bodies and a `writer` agent, and return `(tmp, project_dir)`.
fn fixture(global_cfg: &str, project_cfg: &str) -> (assert_fs::TempDir, std::path::PathBuf) {
    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.create_dir_all().unwrap();
    global.child("agents").create_dir_all().unwrap();
    global.child("agents/writer.md").write_str(WRITER_AGENT).unwrap();
    global.child("config.toml").write_str(global_cfg).unwrap();

    let project = tmp.child("proj");
    project.create_dir_all().unwrap();
    project.child(".rupu").create_dir_all().unwrap();
    project
        .child(".rupu/config.toml")
        .write_str(project_cfg)
        .unwrap();

    let project_path = project.path().to_path_buf();
    (tmp, project_path)
}

/// Run `rupu run writer "go"` with no `--mode` flag (so the effective mode
/// comes from config alone) inside `project`, with `global` as `RUPU_HOME`.
async fn run_writer(global_home: &std::path::Path, project: &std::path::Path) {
    std::env::set_var("RUPU_HOME", global_home);
    std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", WRITE_SCRIPT);
    let restore_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(project).unwrap();

    let _exit = rupu_cli::run(vec![
        "rupu".into(),
        "run".into(),
        "writer".into(),
        "go".into(),
    ])
    .await;

    std::env::set_current_dir(restore_cwd).unwrap();
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    std::env::remove_var("RUPU_HOME");
}

/// A global `permission_mode` named in `[policy].lock` must survive a
/// conflicting project config on the `rupu run` path: the agent's `write_file`
/// call is denied because the effective mode stays `readonly`.
#[tokio::test(flavor = "multi_thread")]
async fn cli_run_honors_a_locked_global_permission_mode() {
    let _guard = ENV_LOCK.lock().await;

    let (tmp, project) = fixture(
        "permission_mode = \"readonly\"\n[policy]\nlock = [\"permission_mode\"]\n",
        "permission_mode = \"bypass\"\n",
    );
    run_writer(&tmp.child(".rupu").path().to_path_buf(), &project).await;

    assert!(
        !project.join("locked.txt").exists(),
        "project config overrode a LOCKED global permission_mode: the run \
         executed write_file under bypass instead of denying it under readonly"
    );
}

/// Control for the test above: with no `[policy].lock`, ordinary
/// project-over-global layering still applies — the project's `bypass` wins
/// and the write goes through. Without this, the locked-case assertion could
/// pass for the wrong reason (e.g. the tool never running at all).
#[tokio::test(flavor = "multi_thread")]
async fn cli_run_lets_an_unlocked_project_permission_mode_win() {
    let _guard = ENV_LOCK.lock().await;

    let (tmp, project) = fixture(
        "permission_mode = \"readonly\"\n",
        "permission_mode = \"bypass\"\n",
    );
    run_writer(&tmp.child(".rupu").path().to_path_buf(), &project).await;

    assert!(
        project.join("locked.txt").exists(),
        "unlocked project permission_mode should still override the global one"
    );
}

/// The library-level contract the CLI paths now depend on (Task 2's
/// `layer_files_locked`). Kept here so the CLI-side regression tests document
/// what they are asserting through.
#[test]
fn layer_files_locked_keeps_the_locked_global_value() {
    let dir = assert_fs::TempDir::new().unwrap();
    let global = dir.path().join("config.toml");
    let project_dir = dir.path().join("proj/.rupu");
    std::fs::create_dir_all(&project_dir).unwrap();
    let project = project_dir.join("config.toml");
    std::fs::write(
        &global,
        "permission_mode = \"readonly\"\n[policy]\nlock = [\"permission_mode\"]\n",
    )
    .unwrap();
    std::fs::write(&project, "permission_mode = \"bypass\"\n").unwrap();

    let cfg = rupu_config::layer_files_locked(Some(&global), Some(&project)).unwrap();
    assert_eq!(cfg.permission_mode.as_deref(), Some("readonly"));
}

/// Regression for the I-7 follow-up: `crates/rupu-cli/src/cmd/editor.rs` was
/// wrongly annotated `// UI prefs only — lock does not apply (I-7)` even
/// though `[ui].editor` is the program name `open_for_edit` spawns as a
/// subprocess on `rupu agent edit` / `rupu workflow create` — exactly the
/// kind of governance-relevant value the lock exists to protect. `editor.rs`'s
/// `resolve_editor` is not reachable from an integration test (it is a
/// private fn resolving env vars + cwd as a side effect), so this pins the
/// same fixture shape through `layer_files_locked` directly: a global
/// `ui.editor` locked via `[policy] lock = ["ui.editor"]` must survive a
/// conflicting project `ui.editor`. The call-site swap itself (`editor.rs`
/// now calling `layer_files_locked` instead of `layer_files`) is verified by
/// the I-7 completeness grep, not by this test.
#[test]
fn layer_files_locked_keeps_a_locked_global_ui_editor() {
    let dir = assert_fs::TempDir::new().unwrap();
    let global = dir.path().join("config.toml");
    let project_dir = dir.path().join("proj/.rupu");
    std::fs::create_dir_all(&project_dir).unwrap();
    let project = project_dir.join("config.toml");
    std::fs::write(
        &global,
        "[ui]\neditor = \"locked-editor\"\n[policy]\nlock = [\"ui.editor\"]\n",
    )
    .unwrap();
    std::fs::write(&project, "[ui]\neditor = \"project-editor\"\n").unwrap();

    let cfg = rupu_config::layer_files_locked(Some(&global), Some(&project)).unwrap();
    assert_eq!(
        cfg.ui.editor.as_deref(),
        Some("locked-editor"),
        "a LOCKED global ui.editor must survive a conflicting project override — \
         ui.editor is the program name spawned as a subprocess, not a display preference"
    );
}
