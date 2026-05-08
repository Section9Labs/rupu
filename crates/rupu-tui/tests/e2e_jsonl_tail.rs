use std::io::Write;
use std::time::Duration;

use chrono::Utc;
use rupu_transcript::{Event, RunMode};
use rupu_tui::source::{EventSource, JsonlTailSource, SourceEvent};
use tempfile::tempdir;

#[test]
fn tail_emits_events_with_filename_stem_step_id() {
    let dir = tempdir().unwrap();
    let transcripts = dir.path().join("transcripts");
    std::fs::create_dir_all(&transcripts).unwrap();

    let mut src = JsonlTailSource::new(dir.path().to_path_buf()).unwrap();

    let path = transcripts.join("run_01KX.jsonl");
    let mut f = std::fs::File::create(&path).unwrap();
    let ev = Event::RunStart {
        run_id: "run_01KX".into(),
        workspace_id: "ws".into(),
        agent: "a".into(),
        provider: "anthropic".into(),
        model: "m".into(),
        started_at: Utc::now(),
        mode: RunMode::Ask,
    };
    writeln!(f, "{}", serde_json::to_string(&ev).unwrap()).unwrap();
    f.sync_all().unwrap();

    let drained = wait_for_events(&mut src, Duration::from_secs(2));
    assert!(
        drained.iter().any(|e| matches!(
            e,
            SourceEvent::StepEvent { step_id, .. } if step_id == "run_01KX"
        )),
        "expected StepEvent with step_id derived from filename stem; got {drained:?}"
    );
}

use rupu_orchestrator::{RunRecord, RunStatus};

#[test]
fn tail_emits_run_update_when_run_json_changes() {
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("transcripts")).unwrap();

    let rec = RunRecord {
        id: "run_001".into(),
        workflow_name: "wf".into(),
        status: RunStatus::Running,
        inputs: Default::default(),
        event: None,
        workspace_id: "ws".into(),
        workspace_path: dir.path().to_path_buf(),
        transcript_dir: dir.path().join("transcripts"),
        started_at: Utc::now(),
        finished_at: None,
        error_message: None,
        awaiting_step_id: None,
        approval_prompt: None,
        awaiting_since: None,
        expires_at: None,
        issue_ref: None,
        issue: None,
        parent_run_id: None,
    };
    std::fs::write(
        dir.path().join("run.json"),
        serde_json::to_vec(&rec).unwrap(),
    )
    .unwrap();

    let mut src = JsonlTailSource::new(dir.path().to_path_buf()).unwrap();

    // Touch run.json with a status change.
    let mut updated = rec;
    updated.status = RunStatus::AwaitingApproval;
    std::fs::write(
        dir.path().join("run.json"),
        serde_json::to_vec(&updated).unwrap(),
    )
    .unwrap();

    let drained = wait_for_events(&mut src, Duration::from_secs(2));
    assert!(
        drained.iter().any(
            |e| matches!(e, SourceEvent::RunUpdate(r) if r.status == RunStatus::AwaitingApproval)
        ),
        "expected RunUpdate(AwaitingApproval), got {drained:?}"
    );
}

fn wait_for_events(src: &mut JsonlTailSource, total: Duration) -> Vec<SourceEvent> {
    let deadline = std::time::Instant::now() + total;
    let mut out = Vec::new();
    while std::time::Instant::now() < deadline {
        out.extend(src.poll());
        if !out.is_empty() {
            return out;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    out
}
