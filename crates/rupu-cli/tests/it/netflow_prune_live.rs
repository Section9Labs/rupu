//! `rupu netflow prune` never deletes a live run's ledger, however old its
//! mtime: an idle-but-live workflow run (non-terminal `run.json` with a
//! live runner pid) and a standalone `rupu run` (metadata with a live pid)
//! keep theirs, reported `skipped_live`; a finished run's and an unknown
//! id's old ledgers are deleted as before.

use assert_cmd::Command;
use std::path::Path;
use std::time::{Duration, SystemTime};

fn old_ledger(dir: &Path, id: &str) {
    let path = dir.join(format!("{id}.jsonl"));
    std::fs::write(&path, "{}\n").unwrap();
    let f = std::fs::File::options().write(true).open(&path).unwrap();
    f.set_modified(SystemTime::now() - Duration::from_secs(90 * 86_400))
        .unwrap();
}

fn run_json(home: &Path, id: &str, status: &str, pid: Option<u32>) {
    let dir = home.join("runs").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let rec = serde_json::json!({
        "id": id,
        "workflow_name": "nightly",
        "status": status,
        "inputs": {},
        "workspace_id": "ws_1",
        "workspace_path": "/tmp/proj",
        "transcript_dir": "/tmp/proj/.rupu/transcripts",
        "started_at": "2026-01-01T00:00:00Z",
        "runner_pid": pid,
    });
    std::fs::write(dir.join("run.json"), rec.to_string()).unwrap();
    std::fs::write(dir.join("workflow.yaml"), "name: nightly\nsteps: []\n").unwrap();
}

#[test]
fn prune_skips_ledgers_of_live_runs() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let netflow = home.path().join("netflow");
    std::fs::create_dir_all(&netflow).unwrap();
    let me = std::process::id();

    // Idle-but-live workflow run: Running, runner pid alive.
    run_json(home.path(), "run_wf_live", "running", Some(me));
    old_ledger(&netflow, "run_wf_live");
    // Paused workflow run: a resume appends to the same ledger.
    run_json(home.path(), "run_wf_paused", "paused", None);
    old_ledger(&netflow, "run_wf_paused");
    // Finished workflow run.
    run_json(home.path(), "run_wf_done", "completed", None);
    old_ledger(&netflow, "run_wf_done");
    // Standalone `rupu run` in flight: no run.json yet, live metadata pid.
    let transcripts = home.path().join("transcripts");
    std::fs::create_dir_all(&transcripts).unwrap();
    std::fs::write(
        transcripts.join("run_sa_live.meta.json"),
        serde_json::json!({
            "version": 1,
            "run_id": "run_sa_live",
            "workspace_path": "/tmp/proj",
            "backend_id": "local_checkout",
            "trigger_source": "run_cli",
            "pid": me,
        })
        .to_string(),
    )
    .unwrap();
    old_ledger(&netflow, "run_sa_live");
    // No owner anywhere.
    old_ledger(&netflow, "run_unknown");

    let out = Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(cwd.path())
        .env("RUPU_HOME", home.path())
        .env("RUPU_NO_UPDATE_CHECK", "1")
        .args(["--format", "json", "netflow", "prune"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let status_of = |id: &str| {
        report["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["run_id"] == id)
            .map(|r| r["status"].as_str().unwrap().to_string())
    };
    for id in ["run_wf_live", "run_wf_paused", "run_sa_live"] {
        assert_eq!(status_of(id).as_deref(), Some("skipped_live"), "{id}");
        assert!(netflow.join(format!("{id}.jsonl")).exists(), "{id} deleted");
    }
    for id in ["run_wf_done", "run_unknown"] {
        assert_eq!(status_of(id).as_deref(), Some("deleted"), "{id}");
        assert!(!netflow.join(format!("{id}.jsonl")).exists(), "{id} kept");
    }
}
