use chrono::{TimeZone, Utc};
use rupu_transcript::event::{Event, RunMode, RunStatus};
use rupu_transcript::{JsonlReader, JsonlWriter, ReadError};
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
                schema: None,
                system_prompt: None,
            },
            Event::TurnStart { turn_idx: 0 },
            Event::TurnEnd {
                turn_idx: 0,
                tokens_in: Some(10),
                tokens_out: Some(20),
                stop_reason: None,
                response_id: None,
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
                schema: None,
                system_prompt: None,
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
            schema: None,
            system_prompt: None,
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
                stop_reason: None,
                response_id: None,
            },
            Event::TurnStart { turn_idx: 1 },
        ],
    );
    let events: Vec<_> = JsonlReader::iter(f.path())
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(events.len(), 3);
    assert!(matches!(events[0], Event::TurnStart { turn_idx: 0 }));
    assert!(matches!(events[2], Event::TurnStart { turn_idx: 1 }));
}

/// `<global>/transcripts/` is a shared namespace — a `run:` workflow step
/// writes a single `{"type":"RunStep",…}` record there under the same
/// `run_<ulid>.jsonl` naming an agent transcript uses. Summarizing one must
/// report "not an agent transcript" (with the producer's tag, so a scanner
/// can skip it quietly) rather than a generic read failure: on a real
/// workspace this shape outnumbered corrupt files 2951-to-0, and warning
/// per file buried the `rupu cp serve` log.
#[test]
fn summary_of_a_foreign_step_transcript_is_classified_not_reported_as_broken() {
    let f = NamedTempFile::new().unwrap();
    std::fs::write(
        f.path(),
        b"{\"type\":\"RunStep\",\"cmd\":\"nmap\",\"exit_code\":0}\n",
    )
    .unwrap();

    match JsonlReader::summary(f.path()) {
        Err(ReadError::NotAnAgentTranscript { first_tag }) => {
            assert_eq!(first_tag.as_deref(), Some("RunStep"));
        }
        other => panic!("expected NotAnAgentTranscript, got {other:?}"),
    }
}

/// An empty file is classified the same way, with no tag to report — it is
/// still simply "not an agent transcript".
#[test]
fn summary_of_an_empty_file_is_classified_with_no_tag() {
    let f = NamedTempFile::new().unwrap();
    match JsonlReader::summary(f.path()) {
        Err(ReadError::NotAnAgentTranscript { first_tag }) => assert_eq!(first_tag, None),
        other => panic!("expected NotAnAgentTranscript, got {other:?}"),
    }
}
