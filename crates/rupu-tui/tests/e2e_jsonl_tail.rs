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
