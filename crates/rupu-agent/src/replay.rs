//! Rebuild the exact provider conversation from a v2 transcript (spec §4).
//!
//! Inverse of the runner's emission contract: per turn, `Thinking` /
//! `AssistantMessage` / `ToolCall` events (in on-disk order) fold into one
//! assistant `Message`; the turn's `ToolResult`s fold into the following
//! user message using the SAME error-formatting + clamp the runner feeds
//! the model; `Seed` initializes state; `Compaction` replaces it. A turn
//! with no `TurnEnd` (paused / aborted mid-turn) is dropped, matching the
//! runner, which never committed it to `messages` either.
//!
//! Legacy (v1) transcripts reconstruct without reasoning blocks — the v1
//! `assistant_message.thinking` string has no `raw` payload to rebuild.

use rupu_providers::types::{ContentBlock, Message, Role};
use rupu_transcript::Event;
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ReplayError {
    #[error("malformed seed messages: {0}")]
    Seed(serde_json::Error),
    #[error("malformed compaction messages: {0}")]
    Compaction(serde_json::Error),
    #[error("seed references transcript {path}; use reconstruct_transcript to resolve chains")]
    SeedUnresolved { path: String },
    #[error("seed references transcript {path} but it could not be read: {source}")]
    SeedSource {
        path: String,
        source: rupu_transcript::ReadError,
    },
    #[error("seed hash mismatch for referenced transcript {path} (chain edited, pruned, or cross-version serialization drift)")]
    SeedHashMismatch { path: String },
    #[error("seed reference chain exceeds depth limit")]
    SeedChainTooDeep,
}

#[derive(Default)]
struct TurnAccum {
    assistant_blocks: Vec<ContentBlock>,
    call_order: Vec<String>,
    results: HashMap<String, (String, Option<String>)>,
}

impl TurnAccum {
    fn flush_into(&mut self, messages: &mut Vec<Message>) {
        if !self.assistant_blocks.is_empty() {
            messages.push(Message {
                role: Role::Assistant,
                content: std::mem::take(&mut self.assistant_blocks),
            });
        }
        let blocks: Vec<ContentBlock> = self
            .call_order
            .drain(..)
            .filter_map(|id| {
                self.results.remove(&id).map(|(output, error)| {
                    let is_error = error.is_some();
                    let content = match error {
                        Some(e) => {
                            crate::runner::clamp_tool_result_text(&format!("error: {e}\n{output}"))
                        }
                        None => output,
                    };
                    ContentBlock::ToolResult {
                        tool_use_id: id,
                        content,
                        is_error,
                    }
                })
            })
            .collect();
        if !blocks.is_empty() {
            messages.push(Message {
                role: Role::User,
                content: blocks,
            });
        }
        self.results.clear();
    }
}

/// Pure reconstruction: inline seeds only. A referenced seed errors with
/// `SeedUnresolved` — callers with filesystem access use
/// [`reconstruct_transcript`], which resolves chains.
pub fn reconstruct_messages(events: &[Event]) -> Result<Vec<Message>, ReplayError> {
    reconstruct_with(events, &mut |path| {
        Err(ReplayError::SeedUnresolved {
            path: path.to_string(),
        })
    })
}

/// Read `path` and reconstruct its conversation, recursively resolving
/// `Seed.source_transcript` chains (each hop re-enters this function) and
/// verifying every resolved seed against its recorded `sha256`. Depth-capped
/// so a cyclic/hostile chain terminates.
pub fn reconstruct_transcript(path: &std::path::Path) -> Result<Vec<Message>, ReplayError> {
    reconstruct_transcript_at_depth(path, 0)
}

const MAX_SEED_CHAIN_DEPTH: u32 = 1024;

fn reconstruct_transcript_at_depth(
    path: &std::path::Path,
    depth: u32,
) -> Result<Vec<Message>, ReplayError> {
    if depth > MAX_SEED_CHAIN_DEPTH {
        return Err(ReplayError::SeedChainTooDeep);
    }
    let read = |p: &std::path::Path| -> Result<Vec<Event>, rupu_transcript::ReadError> {
        Ok(rupu_transcript::JsonlReader::iter(p)?
            .filter_map(Result::ok)
            .collect())
    };
    let events = read(path).map_err(|e| ReplayError::SeedSource {
        path: path.display().to_string(),
        source: e,
    })?;
    reconstruct_with(&events, &mut |src| {
        reconstruct_transcript_at_depth(std::path::Path::new(src), depth + 1)
    })
}

fn reconstruct_with(
    events: &[Event],
    resolve: &mut dyn FnMut(&str) -> Result<Vec<Message>, ReplayError>,
) -> Result<Vec<Message>, ReplayError> {
    let mut messages: Vec<Message> = Vec::new();
    let mut accum = TurnAccum::default();

    for ev in events {
        match ev {
            Event::Seed {
                sha256,
                source_transcript,
                messages: inline,
                ..
            } => {
                let seed: Vec<Message> = match (inline, source_transcript) {
                    (Some(m), _) => serde_json::from_value(m.clone()).map_err(ReplayError::Seed)?,
                    (None, Some(src)) => {
                        let resolved = resolve(src)?;
                        // Verify the chain instead of trusting it.
                        if crate::runner::seed_sha256(&resolved) != *sha256 {
                            return Err(ReplayError::SeedHashMismatch { path: src.clone() });
                        }
                        resolved
                    }
                    (None, None) => Vec::new(), // malformed but tolerated: empty seed
                };
                messages = seed;
            }
            Event::UserMessage { content } => messages.push(Message::user(content)),
            Event::Compaction { messages: m, .. } => {
                messages = serde_json::from_value(m.clone()).map_err(ReplayError::Compaction)?;
            }
            Event::Thinking {
                text,
                provider,
                model,
                raw,
            } => {
                accum.assistant_blocks.push(ContentBlock::Reasoning {
                    text: text.clone(),
                    provider: provider.clone(),
                    model: model.clone(),
                    raw: raw.clone(),
                });
            }
            Event::AssistantMessage { content, .. } => {
                // v1 wrote a synthetic empty-content message to carry
                // thinking on tool-only turns; the model never saw an empty
                // text block, so skip those.
                if !content.is_empty() {
                    accum.assistant_blocks.push(ContentBlock::Text {
                        text: content.clone(),
                    });
                }
            }
            Event::ToolCall {
                call_id,
                tool,
                input,
            } => {
                accum.assistant_blocks.push(ContentBlock::ToolUse {
                    id: call_id.clone(),
                    name: tool.clone(),
                    input: input.clone(),
                });
                accum.call_order.push(call_id.clone());
            }
            Event::ToolResult {
                call_id,
                output,
                error,
                ..
            } => {
                accum
                    .results
                    .insert(call_id.clone(), (output.clone(), error.clone()));
            }
            Event::TurnEnd { .. } => accum.flush_into(&mut messages),
            // Non-conversation events.
            Event::RunStart { .. }
            | Event::TurnStart { .. }
            | Event::AssistantDelta { .. }
            | Event::ThinkingDelta { .. }
            | Event::FileEdit { .. }
            | Event::CommandRun { .. }
            | Event::ActionEmitted { .. }
            | Event::GateRequested { .. }
            | Event::Usage { .. }
            | Event::RunComplete { .. }
            | Event::ToolAudit { .. }
            | Event::NetFlow { .. }
            | Event::Notice { .. }
            | Event::Unknown => {}
        }
    }
    // No trailing flush: a turn without TurnEnd was never committed by the
    // runner either.
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{run_agent, tests::opts_for, MockProvider, ScriptedTurn};
    use rupu_providers::types::{ContentBlock, Message, StopReason};

    fn reasoning(text: &str) -> ContentBlock {
        ContentBlock::Reasoning {
            text: Some(text.into()),
            provider: "mock".into(),
            model: "mock-1".into(),
            raw: serde_json::json!({"type":"thinking","thinking": text, "signature":"sig-abc"}),
        }
    }

    fn read_events(path: &std::path::Path) -> Vec<rupu_transcript::Event> {
        rupu_transcript::JsonlReader::iter(path)
            .unwrap()
            .filter_map(Result::ok)
            .collect()
    }

    #[tokio::test]
    async fn round_trip_reasoning_and_tool_turns() {
        let tmp = tempfile::tempdir().unwrap();
        let transcript_path = tmp.path().join("run.jsonl");
        let provider = MockProvider::new(vec![
            ScriptedTurn::AssistantBlocks {
                content: vec![
                    reasoning("pick a tool"),
                    ContentBlock::ToolUse {
                        id: "c1".into(),
                        name: "no_such_tool".into(),
                        input: serde_json::json!({"q": 1}),
                    },
                ],
                stop: StopReason::ToolUse,
            },
            ScriptedTurn::AssistantBlocks {
                content: vec![
                    reasoning("summarize"),
                    ContentBlock::Text {
                        text: "done".into(),
                    },
                ],
                stop: StopReason::EndTurn,
            },
        ]);
        // Same opts constructor the runner tests use; it hardcodes the user
        // turn as "test prompt".
        let opts = opts_for(Box::new(provider), tmp.path(), transcript_path.clone());
        run_agent(opts).await.expect("run completes");

        let rebuilt = reconstruct_messages(&read_events(&transcript_path)).expect("reconstruct");
        let expected: Vec<Message> = vec![
            Message::user("test prompt"),
            Message {
                role: rupu_providers::types::Role::Assistant,
                content: vec![
                    reasoning("pick a tool"),
                    ContentBlock::ToolUse {
                        id: "c1".into(),
                        name: "no_such_tool".into(),
                        input: serde_json::json!({"q": 1}),
                    },
                ],
            },
            Message {
                role: rupu_providers::types::Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "c1".into(),
                    content: "error: unknown tool: no_such_tool\n".into(),
                    is_error: true,
                }],
            },
            Message {
                role: rupu_providers::types::Role::Assistant,
                content: vec![
                    reasoning("summarize"),
                    ContentBlock::Text {
                        text: "done".into(),
                    },
                ],
            },
        ];
        assert_eq!(
            serde_json::to_value(&rebuilt).unwrap(),
            serde_json::to_value(&expected).unwrap(),
            "transcript must rebuild the exact conversation, raw signatures included"
        );
    }

    #[tokio::test]
    async fn round_trip_preserves_seed_and_redacted_thinking() {
        let tmp = tempfile::tempdir().unwrap();
        let transcript_path = tmp.path().join("run.jsonl");
        let seed = vec![
            Message::user("earlier"),
            Message {
                role: rupu_providers::types::Role::Assistant,
                content: vec![
                    ContentBlock::Reasoning {
                        text: None, // redacted
                        provider: "mock".into(),
                        model: "mock-1".into(),
                        raw: serde_json::json!({"type":"redacted_thinking","data":"opaque"}),
                    },
                    ContentBlock::Text {
                        text: "noted".into(),
                    },
                ],
            },
        ];
        let provider = MockProvider::new(vec![ScriptedTurn::AssistantBlocks {
            content: vec![ContentBlock::Text { text: "ok".into() }],
            stop: StopReason::EndTurn,
        }]);
        let mut opts = opts_for(Box::new(provider), tmp.path(), transcript_path.clone());
        opts.initial_messages = seed.clone();
        run_agent(opts).await.expect("run completes");

        let rebuilt = reconstruct_messages(&read_events(&transcript_path)).unwrap();
        assert_eq!(
            serde_json::to_value(&rebuilt[..2]).unwrap(),
            serde_json::to_value(&seed).unwrap(),
            "inline seed must survive the transcript byte-exact, redacted raw included"
        );
    }

    #[tokio::test]
    async fn referenced_seed_chain_resolves_and_verifies_hash() {
        let tmp = tempfile::tempdir().unwrap();

        // Turn 1: a fresh run, its own transcript.
        let t1 = tmp.path().join("turn1.jsonl");
        let p1 = MockProvider::new(vec![ScriptedTurn::AssistantBlocks {
            content: vec![
                reasoning("first"),
                ContentBlock::Text {
                    text: "hello".into(),
                },
            ],
            stop: StopReason::EndTurn,
        }]);
        run_agent(opts_for(Box::new(p1), tmp.path(), t1.clone()))
            .await
            .expect("turn 1 completes");
        let turn1_convo = reconstruct_transcript(&t1).expect("turn 1 reconstructs");

        // Turn 2: seeded BY REFERENCE to turn 1's transcript — no re-embed.
        let t2 = tmp.path().join("turn2.jsonl");
        let p2 = MockProvider::new(vec![ScriptedTurn::AssistantBlocks {
            content: vec![ContentBlock::Text {
                text: "again".into(),
            }],
            stop: StopReason::EndTurn,
        }]);
        let mut opts = opts_for(Box::new(p2), tmp.path(), t2.clone());
        opts.initial_messages = turn1_convo.clone();
        opts.seed_source = Some(t1.clone());
        run_agent(opts).await.expect("turn 2 completes");

        // The seed is a reference, not a copy.
        let seed_ev = read_events(&t2).into_iter().find_map(|e| match e {
            rupu_transcript::Event::Seed {
                source_transcript,
                messages,
                ..
            } => Some((source_transcript, messages)),
            _ => None,
        });
        let (src, inline) = seed_ev.expect("turn 2 has a seed event");
        assert_eq!(src.as_deref(), Some(t1.display().to_string().as_str()));
        assert!(
            inline.is_none(),
            "referenced seed must not re-embed the messages"
        );

        // Chain-resolved reconstruction yields the full conversation.
        let full = reconstruct_transcript(&t2).expect("chain resolves");
        assert_eq!(
            serde_json::to_value(&full[..turn1_convo.len()]).unwrap(),
            serde_json::to_value(&turn1_convo).unwrap()
        );
        assert!(
            full.len() > turn1_convo.len(),
            "turn 2's own messages follow the seed"
        );

        // The pure entrypoint refuses to guess.
        assert!(matches!(
            reconstruct_messages(&read_events(&t2)),
            Err(ReplayError::SeedUnresolved { .. })
        ));

        // Tampering with the source is caught, never silently absorbed.
        std::fs::write(&t1, "").unwrap();
        assert!(matches!(
            reconstruct_transcript(&t2),
            Err(ReplayError::SeedHashMismatch { .. }) | Err(ReplayError::SeedSource { .. })
        ));
    }
}
