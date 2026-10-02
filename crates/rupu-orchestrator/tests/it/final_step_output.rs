//! A step's output is its agent's final turn text: every text fragment of
//! that turn, not only the last one (a turn can be text → thinking → text).

use rupu_orchestrator::read_final_assistant_text;
use rupu_transcript::{Event, JsonlWriter};

#[test]
fn a_two_fragment_final_turn_yields_both_fragments() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("step.jsonl");
    let mut w = JsonlWriter::create(&path).unwrap();
    for e in [
        Event::TurnStart { turn_idx: 0 },
        Event::AssistantMessage {
            content: "looking around".into(),
            thinking: None,
        },
        Event::TurnStart { turn_idx: 1 },
        Event::AssistantMessage {
            content: "Findings:".into(),
            thinking: None,
        },
        Event::Thinking {
            text: Some("double-check".into()),
            provider: "anthropic".into(),
            model: "m".into(),
            raw: serde_json::json!({}),
        },
        Event::AssistantMessage {
            content: "- none".into(),
            thinking: None,
        },
    ] {
        w.write(&e).unwrap();
    }
    w.flush().unwrap();
    assert_eq!(
        read_final_assistant_text(&path, true, "run_1", "step_1"),
        "Findings:\n\n- none"
    );
}
