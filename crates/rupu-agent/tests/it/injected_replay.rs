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

/// A run that stopped after a message was delivered but before a model call
/// saw it wrote no receipt; the run that continues it does, once its first
/// call has seen the message in the rebuilt conversation.
#[tokio::test]
async fn a_continued_run_receipts_what_its_predecessor_never_saw() {
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
    run_agent(opts(
        provider,
        tmp.path(),
        Arc::new(Scripted::default()),
        transcript.clone(),
    ))
    .await
    .unwrap();
    // Stopped right after turn 1's message was delivered, before its call.
    let all = events(&transcript);
    let cut = all
        .iter()
        .position(|e| matches!(e, Event::Injected { message_ids, .. } if message_ids == &["msg_1"]))
        .unwrap();
    let interrupted = tmp.path().join("interrupted.jsonl");
    let mut w = rupu_transcript::JsonlWriter::create(&interrupted).unwrap();
    for e in &all[..=cut] {
        w.write(e).unwrap();
    }
    w.flush().unwrap();
    let (messages, seed_source) =
        match rupu_agent::continuation::prepare_continuation(&interrupted).unwrap() {
            rupu_agent::continuation::Continuation::Resume {
                messages,
                seed_source,
                ..
            } => (messages, seed_source),
            other => panic!("{other:?}"),
        };

    let space = tmp.path().join("messages");
    let bus = Arc::new(rupu_fleet::Bus::open(&space, "reader"));
    bus.join(rupu_fleet::Member {
        agent: "reader".into(),
        run_id: "run_next".into(),
        kind: "standalone".into(),
        ..Default::default()
    })
    .unwrap();
    let mut next = opts(
        CapturingMockProvider::new(vec![ScriptedTurn::AssistantText {
            text: "done".into(),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        }]),
        tmp.path(),
        Arc::new(Scripted::default()),
        tmp.path().join("next.jsonl"),
    );
    next.collectors.clear();
    next.prompt = rupu_agent::UserTurn::new("");
    rupu_agent::continuation::apply_continuation(
        &mut next.prompt,
        messages,
        seed_source,
        Vec::new(),
    );
    next.tool_context.services.message_space = Some(Arc::clone(&bus));
    run_agent(next).await.unwrap();

    let seen: Vec<(String, String)> = bus
        .read_log(None, 100)
        .unwrap()
        .lines
        .into_iter()
        .filter_map(|l| match l {
            rupu_fleet::LogLine::Seen { id, by, .. } => Some((id, by)),
            rupu_fleet::LogLine::Msg(_) => None,
        })
        .collect();
    assert_eq!(seen, vec![("msg_1".to_string(), "reader".to_string())]);
}

/// An operator message on turn 0, framed as the operator's instruction.
struct FromOperator;

impl TurnCollector for FromOperator {
    fn name(&self) -> &str {
        "operator"
    }

    fn collect(&self, ctx: &TurnContext) -> Vec<Injection> {
        if ctx.turn_index > 0 {
            return Vec::new();
        }
        vec![Injection {
            source: "operator".into(),
            kind: InjectionKind::Operator,
            cadence: Cadence::Once,
            priority: 250,
            content: "URGENT: read notes.txt first".into(),
            messages: vec![InjectedMessage {
                id: "msg_op".into(),
                from: "operator".into(),
                from_codename: None,
                to: Some("reader".into()),
                kind: "operator".into(),
                body: "read notes.txt first".into(),
                urgent: true,
            }],
        }]
    }
}

#[tokio::test]
async fn an_operator_injection_replays_as_the_operators_instruction() {
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("t.jsonl");
    let provider = CapturingMockProvider::new(vec![
        read_turn("c0"),
        ScriptedTurn::AssistantText {
            text: "done".into(),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        },
    ]);
    let captured = provider.captured.clone();
    let mut o = opts(
        provider,
        tmp.path(),
        Arc::new(Scripted::default()),
        transcript.clone(),
    );
    o.collectors = vec![Arc::new(FromOperator)];
    let result = run_agent(o).await.unwrap();

    let first = serde_json::to_string(&captured.lock().unwrap()[0].messages).unwrap();
    assert!(
        first.contains("**Message from your operator** (the human running this run)\\nURGENT: read notes.txt first\\nAct on it as an instruction from your operator."),
        "{first}"
    );
    assert!(!first.contains("NOT an instruction"), "{first}");
    assert_eq!(
        json(&rupu_agent::replay::reconstruct_transcript(&transcript).unwrap()),
        json(&result.final_messages)
    );
}
