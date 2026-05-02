use chrono::{TimeZone, Utc};
use rupu_transcript::event::{Event, RunMode, RunStatus};
use rupu_transcript::{JsonlReader, JsonlWriter};
use std::io::Write;
use tempfile::NamedTempFile;

fn write_events(path: &std::path::Path, events: &[Event]) {
    let mut w = JsonlWriter::create(path).unwrap();
    for e in events {
        w.write(e).unwrap();
    }
    w.flush().unwrap();
}

#[test]
fn reads_complete_run_summary() {
    let f = NamedTempFile::new().unwrap();
    write_events(
        f.path(),
        &[
            Event::RunStart {
                run_id: "run_a".into(),
                workspace_id: "ws_a".into(),
                agent: "fix-bug".into(),
                provider: "anthropic".into(),
                model: "claude-sonnet-4-6".into(),
                started_at: Utc.with_ymd_and_hms(2026, 5, 1, 17, 0, 0).unwrap(),
                mode: RunMode::Ask,
            },
            Event::TurnStart { turn_idx: 0 },
            Event::TurnEnd {
                turn_idx: 0,
                tokens_in: Some(10),
                tokens_out: Some(20),
            },
            Event::RunComplete {
                run_id: "run_a".into(),
                status: RunStatus::Ok,
                total_tokens: 30,
                duration_ms: 1000,
                error: None,
            },
        ],
    );
    let summary = JsonlReader::summary(f.path()).unwrap();
    assert_eq!(summary.run_id, "run_a");
    assert_eq!(summary.status, RunStatus::Ok);
    assert_eq!(summary.total_tokens, 30);
}

#[test]
fn missing_run_complete_reports_aborted() {
    let f = NamedTempFile::new().unwrap();
    write_events(
        f.path(),
        &[
            Event::RunStart {
                run_id: "run_b".into(),
                workspace_id: "ws_a".into(),
                agent: "fix-bug".into(),
                provider: "anthropic".into(),
                model: "claude-sonnet-4-6".into(),
                started_at: Utc.with_ymd_and_hms(2026, 5, 1, 17, 0, 0).unwrap(),
                mode: RunMode::Ask,
            },
            Event::TurnStart { turn_idx: 0 },
            // no TurnEnd, no RunComplete
        ],
    );
    let summary = JsonlReader::summary(f.path()).unwrap();
    assert_eq!(summary.status, RunStatus::Aborted);
    assert_eq!(summary.run_id, "run_b");
}

#[test]
fn truncated_last_line_does_not_crash() {
    let f = NamedTempFile::new().unwrap();
    {
        let mut w = JsonlWriter::create(f.path()).unwrap();
        w.write(&Event::RunStart {
            run_id: "run_c".into(),
            workspace_id: "ws_a".into(),
            agent: "x".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            started_at: Utc.with_ymd_and_hms(2026, 5, 1, 17, 0, 0).unwrap(),
            mode: RunMode::Ask,
        })
        .unwrap();
    }
    // Append a partial JSON line (no trailing newline, malformed)
    let mut handle = std::fs::OpenOptions::new()
        .append(true)
        .open(f.path())
        .unwrap();
    handle.write_all(b"{\"type\":\"turn_start\"").unwrap();

    let summary = JsonlReader::summary(f.path()).unwrap();
    // Should still report as aborted (no run_complete present), not error
    assert_eq!(summary.status, RunStatus::Aborted);
    assert_eq!(summary.run_id, "run_c");
}

#[test]
fn iter_yields_all_events_in_order() {
    let f = NamedTempFile::new().unwrap();
    write_events(
        f.path(),
        &[
            Event::TurnStart { turn_idx: 0 },
            Event::TurnEnd {
                turn_idx: 0,
                tokens_in: Some(1),
                tokens_out: Some(2),
            },
            Event::TurnStart { turn_idx: 1 },
        ],
    );
    let events: Vec<_> = JsonlReader::iter(f.path())
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(events.len(), 3);
    matches!(events[0], Event::TurnStart { turn_idx: 0 });
    matches!(events[2], Event::TurnStart { turn_idx: 1 });
}
