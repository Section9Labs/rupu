use rupu_transcript::{Event, JsonlWriter, RunStatus};
use tempfile::NamedTempFile;

#[test]
fn writes_events_one_per_line() {
    let f = NamedTempFile::new().unwrap();
    let mut w = JsonlWriter::create(f.path()).unwrap();
    w.write(&Event::TurnStart { turn_idx: 0 }).unwrap();
    w.write(&Event::TurnEnd {
        turn_idx: 0,
        tokens_in: Some(10),
        tokens_out: Some(20),
    })
    .unwrap();
    w.flush().unwrap();

    let content = std::fs::read_to_string(f.path()).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 2, "expected 2 lines, got: {content}");
    assert!(lines[0].contains("\"turn_start\""));
    assert!(lines[1].contains("\"turn_end\""));
}

#[test]
fn append_extends_existing_file() {
    let f = NamedTempFile::new().unwrap();
    {
        let mut w = JsonlWriter::create(f.path()).unwrap();
        w.write(&Event::TurnStart { turn_idx: 0 }).unwrap();
    }
    {
        let mut w = JsonlWriter::append(f.path()).unwrap();
        w.write(&Event::TurnEnd {
            turn_idx: 0,
            tokens_in: Some(1),
            tokens_out: Some(1),
        })
        .unwrap();
    }
    let content = std::fs::read_to_string(f.path()).unwrap();
    assert_eq!(content.lines().count(), 2);
}

#[test]
fn each_line_is_valid_json() {
    let f = NamedTempFile::new().unwrap();
    let mut w = JsonlWriter::create(f.path()).unwrap();
    w.write(&Event::RunComplete {
        run_id: "run_x".into(),
        status: RunStatus::Ok,
        total_tokens: 0,
        duration_ms: 0,
        error: None,
    })
    .unwrap();
    let content = std::fs::read_to_string(f.path()).unwrap();
    for (i, line) in content.lines().enumerate() {
        let _: serde_json::Value =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("line {i} not JSON: {e}"));
    }
}
