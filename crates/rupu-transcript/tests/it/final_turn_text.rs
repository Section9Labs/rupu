use rupu_transcript::{final_turn_text, Event};

fn turn(idx: u32) -> Event {
    Event::TurnStart { turn_idx: idx }
}

fn say(text: &str) -> Event {
    Event::AssistantMessage {
        content: text.into(),
        thinking: None,
    }
}

fn think(text: &str) -> Event {
    Event::Thinking {
        text: Some(text.into()),
        provider: "anthropic".into(),
        model: "m".into(),
        raw: serde_json::json!({}),
    }
}

/// A final turn text → thinking → text yields both fragments, joined by a
/// blank line; earlier turns' text is not included.
#[test]
fn final_turn_fragments_are_joined() {
    let events = vec![
        turn(0),
        say("earlier turn"),
        turn(1),
        say("first part"),
        think("pondering"),
        say("  "),
        say("second part"),
    ];
    assert_eq!(
        final_turn_text(events).as_deref(),
        Some("first part\n\nsecond part")
    );
}

/// A transcript without `TurnStart` keeps the old rule: the last non-empty
/// assistant message.
#[test]
fn without_turn_start_the_last_message_wins() {
    let events = vec![say("one"), say("two"), say("")];
    assert_eq!(final_turn_text(events).as_deref(), Some("two"));
}

/// A final turn with no text (a tool-only turn) falls back to the last
/// non-empty assistant message.
#[test]
fn a_final_turn_without_text_falls_back_to_the_last_message() {
    let events = vec![turn(0), say("answer"), turn(1)];
    assert_eq!(final_turn_text(events).as_deref(), Some("answer"));
}

#[test]
fn no_text_at_all_is_none() {
    assert_eq!(final_turn_text(vec![turn(0)]), None);
    assert_eq!(final_turn_text(Vec::new()), None);
}
