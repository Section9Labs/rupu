//! Collector injections are part of the transcript (spec F1a §6): every
//! `once` injection is an `injected` event replay re-inserts, so a continued
//! run rebuilds exactly the messages the provider was sent; a delivered
//! message is a `message { received }` event and gets a receipt once a model
//! call has seen it.

use rupu_agent::runner::{CapturingMockProvider, ScriptedTurn};
use rupu_agent::{
    run_agent, AgentRunOpts, Cadence, InjectedMessage, Injection, InjectionKind, TurnCollector,
    TurnContext,
};
use rupu_providers::model_limits::ModelLimits;
use rupu_providers::types::{ContentBlock, StopReason};
use rupu_tools::ToolContext;
use rupu_transcript::{Event, JsonlReader, MessageDirection};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// A message on turns 0 and 1, and the same standing status every turn.
#[derive(Default)]
struct Scripted {
    seen: Mutex<Vec<(Vec<String>, u32)>>,
}

impl TurnCollector for Scripted {
    fn name(&self) -> &str {
        "scripted"
    }

    fn collect(&self, ctx: &TurnContext) -> Vec<Injection> {
        let mut out = vec![Injection {
            source: "status".into(),
            kind: InjectionKind::Status,
            cadence: Cadence::EveryTurn,
            priority: 10,
            content: "standing status".into(),
            messages: Vec::new(),
        }];
        if ctx.turn_index < 2 {
            let id = format!("msg_{}", ctx.turn_index);
            out.push(Injection {
                source: "mailbox:reader".into(),
                kind: InjectionKind::Message,
                cadence: Cadence::Once,
                priority: 200,
                content: format!("from lynx#1 at t: note {} ==> fenced", ctx.turn_index),
                messages: vec![InjectedMessage {
                    id,
                    from: "lynx#1".into(),
                    from_codename: None,
                    to: Some("reader".into()),
                    kind: "direct".into(),
                    body: format!("note {}", ctx.turn_index),
                    urgent: false,
                }],
            });
        }
        out
    }

    fn seen(&self, ids: &[String], turn: u32) {
        self.seen.lock().unwrap().push((ids.to_vec(), turn));
    }
}

fn read_turn(id: &str) -> ScriptedTurn {
    ScriptedTurn::AssistantToolUse {
        text: None,
        tool_id: id.into(),
        tool_name: "read_file".into(),
        tool_input: serde_json::json!({ "path": "notes.txt" }),
        stop: StopReason::ToolUse,
    }
}

fn opts(
    provider: CapturingMockProvider,
    ws: &Path,
    collector: Arc<Scripted>,
    transcript: std::path::PathBuf,
) -> AgentRunOpts {
    std::fs::write(ws.join("notes.txt"), "invented notes\n").unwrap();
    rupu_agent::grant::with_grant(
        AgentRunOpts {
            system_prompt: "You read notes.".into(),
            prompt: rupu_agent::UserTurn::new("go"),
            provider: Box::new(provider),
            limits: ModelLimits::unknown(),
            recovery: Default::default(),
            permission: rupu_tools::PermissionPolicy::bypass(),
            grant: Default::default(),
            alias_scope: Default::default(),
            tool_context: ToolContext::in_workspace(ws),
            pins: Default::default(),
            concerns: None,
            max_turns: 10,
            stream: rupu_agent::StreamOpts {
                no_stream: true,
                suppress_stdout: true,
                on_stream_event: None,
            },
            hooks: Default::default(),
            pause: None,
            collectors: vec![collector],
            transcript_path: transcript,
        },
        Some(&["read_file".to_string()]),
        &[],
    )
    .expect("grant")
}

fn events(path: &Path) -> Vec<Event> {
    JsonlReader::iter(path).unwrap().flatten().collect()
}

fn json<T: serde::Serialize + ?Sized>(v: &T) -> serde_json::Value {
    serde_json::to_value(v).unwrap()
}

#[tokio::test]
async fn injected_replay_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("t.jsonl");
    let provider = CapturingMockProvider::new(vec![
        read_turn("c0"),
        read_turn("c1"),
        ScriptedTurn::AssistantText {
            text: "done".into(),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        },
    ]);
    let captured = provider.captured.clone();
    let collector = Arc::new(Scripted::default());
    let result = run_agent(opts(
        provider,
        tmp.path(),
        collector.clone(),
        transcript.clone(),
    ))
    .await
    .unwrap();
    let requests = captured.lock().unwrap().clone();
    assert_eq!(requests.len(), 3);

    // The whole run replays to the conversation it ended with.
    let all = events(&transcript);
    assert_eq!(
        json(&rupu_agent::replay::reconstruct_transcript(&transcript).unwrap()),
        json(&result.final_messages)
    );

    // Each message is received once, ahead of the injection carrying it;
    // the standing status is recorded once, not per turn.
    let received: Vec<&str> = all
        .iter()
        .filter_map(|e| match e {
            Event::Message {
                id,
                direction: MessageDirection::Received,
                ..
            } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(received, vec!["msg_0", "msg_1"]);
    let injected: Vec<(&str, &str)> = all
        .iter()
        .filter_map(|e| match e {
            Event::Injected {
                source, cadence, ..
            } => Some((source.as_str(), cadence.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(
        injected,
        vec![
            ("mailbox:reader", "once"),
            ("status", "every_turn"),
            ("mailbox:reader", "once"),
        ]
    );
    // Receipts after the call that saw each message.
    assert_eq!(
        *collector.seen.lock().unwrap(),
        vec![
            (vec!["msg_0".to_string()], 0),
            (vec!["msg_1".to_string()], 1)
        ]
    );

    // Cut the transcript after turn 1, as a crash would: continuing it
    // rebuilds exactly what turn 2's request sent, less that turn's
    // transient status.
    let cut = all
        .iter()
        .position(|e| matches!(e, Event::TurnEnd { turn_idx: 1, .. }))
        .unwrap();
    let interrupted = tmp.path().join("interrupted.jsonl");
    let mut w = rupu_transcript::JsonlWriter::create(&interrupted).unwrap();
    for e in &all[..=cut] {
        w.write(e).unwrap();
    }
    w.flush().unwrap();
    let rebuilt = match rupu_agent::continuation::prepare_continuation(&interrupted).unwrap() {
        rupu_agent::continuation::Continuation::Resume { messages, .. } => messages,
        other => panic!("an interrupted run resumes: {other:?}"),
    };
    let sent = &requests[2].messages;
    assert_eq!(json(&rebuilt), json(&sent[..sent.len() - 1]));
    match &sent[sent.len() - 1].content[0] {
        ContentBlock::Text { text } => assert!(text.contains("standing status"), "{text}"),
        other => panic!("{other:?}"),
    }
}
