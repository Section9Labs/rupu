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

#[test]
fn a_continuation_chain_joins_across_turns() {
    use rupu_transcript::{final_turn_text, Event, RecoveryAction};
    let ev = vec![
        Event::TurnStart { turn_idx: 0 },
        Event::AssistantMessage {
            content: "part one".into(),
            thinking: None,
        },
        Event::Recovery {
            outcome_id: "oc".into(),
            rung: 0,
            action: RecoveryAction::Continued,
            attempt: Some(1),
            budget: Some(3),
            provider: None,
            model: None,
            reason: None,
            merge_into_previous: false,
            continues_output: true,
        },
        Event::UserMessage {
            content: "continue".into(),
        },
        Event::TurnStart { turn_idx: 1 },
        Event::AssistantMessage {
            content: "part two".into(),
            thinking: None,
        },
    ];
    assert_eq!(final_turn_text(ev).as_deref(), Some("part one\n\npart two"));
}

#[test]
fn a_discarded_turn_does_not_contribute() {
    use rupu_transcript::{final_turn_text, Event};
    let ev = vec![
        Event::TurnStart { turn_idx: 0 },
        Event::AssistantMessage {
            content: "kept".into(),
            thinking: None,
        },
        Event::TurnEnd {
            turn_idx: 0,
            tokens_in: None,
            tokens_out: None,
            stop_reason: None,
            response_id: None,
            stop: None,
            discarded: false,
        },
        Event::TurnStart { turn_idx: 1 },
        Event::AssistantMessage {
            content: "refused partial".into(),
            thinking: None,
        },
        Event::TurnEnd {
            turn_idx: 1,
            tokens_in: None,
            tokens_out: None,
            stop_reason: None,
            response_id: None,
            stop: None,
            discarded: true,
        },
    ];
    assert_eq!(final_turn_text(ev).as_deref(), Some("kept"));
}
