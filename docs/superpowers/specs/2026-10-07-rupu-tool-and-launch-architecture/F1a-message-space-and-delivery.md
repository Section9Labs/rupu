# F1a: Message space, log, addressing, delivery, transcript events

- **Card:** F1a · **Depends on:** W1–W7 merged · **Blocks:** F1b, F1c
- **Overview + decisions:** [F1-messaging-everywhere.md](F1-messaging-everywhere.md) (FD1–FD7)
- **Closes:** M1 (store side), M2, M3, M4, M5

## 1. Goal

Every root run can carry one **message space**. Everything said in it is written once, through one writer API, into an append-only **log** together with the working state that tools and collectors read. Messages are addressed to **codename-based participants**, delivered **before each model call**, and given a recorded **delivery status**. Both the sender's and the receiver's transcripts record them as typed events.

## 2. Message space layout

| Root kind | Space directory | Notes |
|---|---|---|
| agentiflow | `<global>/agentiflows/af_<id>/` (the run dir itself) | **No migration.** `board/` and `mailboxes/` stay where they are; `log.jsonl` and `participants.jsonl` are added beside them. Flows recorded before F1 have no log, and F1c falls back to `board/*.jsonl` for them |
| workflow run | `<global>/runs/<run_id>/messages/` | created at run start when `messaging:` is set or any step agent is granted a messaging tool; otherwise lazily (FD1) |
| session | `<sessions>/<session_id>/messages/` | lazy (first child or operator message) |
| standalone `rupu run` | `<global>/runs/<run_id>/messages/` | lazy |

```
<space>/
  log.jsonl            # canonical, append-only (FD3 size cap)
  log.jsonl.lock
  participants.jsonl   # append-only roster events
  board/               # posts.jsonl, directives.jsonl, claims/ (unchanged formats)
  mailboxes/           # <address>/inbox.jsonl, broadcast/log.jsonl, cursors (unchanged mechanics)
```

The root is resolved by `RunAssembler` per `Origin` (one new row in `defaults_for`, W3 §3.3). `rupu_fleet::Bus` (W5) takes the space directory.

## 3. Participants and addresses (closes M3, M4)

### 3.1 Participant registry

`participants.jsonl` is append-only, one event per transition. It is written by `RunAssembler` when a run that has the space starts, and by the agent loop's teardown when it ends:

```jsonc
{"t":"joined","address":"lynx#1","codename":"cobalt-harbor/heron#412>lynx#1","agent":"recon","run_id":"run_…","step_id":"scan","unit":"3","kind":"workflow_step","at":"…"}
{"t":"left","address":"lynx#1","run_id":"run_…","status":"done","at":"…"}
```

- **The address is the codename's role tail** (`lynx#1`), which is unique within a root because one namer mints every codename in a run (W7).
- **Two fixed participants:**
  - `operator` is always present.
  - In an agentiflow, `lead` is an alias for the lead's address.
- **Legacy flows:** today's `<agent>#n` participant names are accepted as aliases of the matching codename address. W7's minting keeps both.
- **Mailbox directory names** become the address with `#` → `_` (`lynx_1`). Addresses are generated from a fixed alphabet, so the old sanitize collision (`recon#1` vs `recon-1`) can't happen. `Mailbox::sanitize` remains only for reading legacy dirs.

### 3.2 Address grammar (`to:` in `msg.send`, `addressed_to` in `board.post` / `board.directive`)

| Address | Resolves to | When nobody matches |
|---|---|---|
| `lynx#1` or a full codename | that participant | `undelivered: unknown_participant` |
| `role:recon` (or bare `recon` when it isn't an address) | every **currently joined** participant whose agent has role `recon`, resolved **at send time** | `undelivered: no_live_participant` |
| `step:scan` | every currently joined participant running step `scan` (workflows) | `undelivered: no_live_participant` |
| `parent` | the sender's parent run's participant (from `RunIdentity.parent`, W3) | `undelivered: no_parent` |
| `lead` | the flow lead (flows only) | `undelivered: no_lead` (workflows) |
| `operator` | the operator's inbox (shown in the UI; no agent drains it) | — |
| `broadcast` | every participant, through the broadcast log + per-reader cursors (unchanged mechanics) | — |

A message to a participant who has **left** is still appended to the log, with `undelivered: participant_left`. It is never silently dropped.

## 4. The log (closes M1, M5)

### 4.1 Line schema

```jsonc
{"t":"msg",
 "id":"msg_01J…",                 // ULID, also the anchor id used by the UI and transcripts
 "ts":"2026-10-08T…Z",
 "kind":"post|direct|broadcast|directive|retract|operator",
 "from":"lynx#1",                  // set by Bus from the caller's identity, never from tool input (FD5)
 "from_codename":"cobalt-harbor/heron#412>lynx#1",
 "to":"role:recon" | "lynx#2" | "broadcast" | null,   // null only for an unaddressed board post
 "post_kind":"observation|question|answer|vote|note",  // posts only
 "body":"…",
 "urgent":false,                   // operator messages (F1b)
 "refs":{"retracts":"dir_…","reply_to":"msg_…"},
 "delivery":{"resolved":["lynx#2","otter#1"],"undelivered":[{"to":"parent","reason":"no_parent"}]}}
{"t":"seen","id":"msg_01J…","by":"lynx#2","turn":7,"at":"…"}   // written when a collector delivers it
```

- **One meaning of broadcast (M5).** `kind: "broadcast"` is the only broadcast. A board post with `to: null` is a channel post, and F1c renders it as such, not as a broadcast.
- **Directives and retractions** are log lines too (`kind: directive`, `kind: retract` with `refs.retracts`), so the UI needs no second fold source.

### 4.2 One writer API (`rupu-fleet/src/bus.rs`, extends W5's `Bus`)

```rust
impl Bus {
    pub fn post(&self, kind: PostKind, to: Option<&Address>, body: &str) -> Result<MessageId, BusError>;
    pub fn send(&self, to: &Address, body: &str) -> Result<SendReceipt, BusError>;   // direct | broadcast | role | step
    pub fn directive(&self, to: Option<&Address>, body: &str) -> Result<MessageId, BusError>;
    pub fn retract(&self, directive: &DirectiveId) -> Result<MessageId, BusError>;
    pub fn mark_seen(&self, ids: &[MessageId], by: &str, turn: u32) -> Result<(), BusError>;
    pub fn read_log(&self, after: Option<&MessageId>, limit: usize) -> Result<LogPage, BusError>;
    // F1b adds operator_send.
}
```

Each write takes `log.jsonl.lock`, checks the FD3 size cap, resolves addresses against `participants.jsonl`, appends the log line, then updates the working state (board files / inboxes). That sequence keeps the log the superset. A crash between the two steps leaves a log line whose working-state write is missing. That direction is safe: the UI still shows the message, and the reader sees "not delivered" because there is no `seen` line. The working state can never hold a message the log lacks.

`board.*` and `msg.send` (W5's catalog tools) call these methods and return the `id` + `delivery` in their tool output, so the sender model learns immediately that a message was undeliverable.

## 5. Delivery (collectors)

- **One module, many origins.** `MessageCollector` (the drained inbox + the broadcast cursor) and `DirectiveCollector` (standing directives, now matching `role:`/`step:` addresses because the collector knows its participant's role and step) move out of `rupu-agentiflow` into `rupu-runtime/src/services/messaging.rs`. `RunAssembler` attaches them for **every** origin whose run has a message space, when the agent is granted any `board.*`/`msg.*` tool **or** the space is active (so operator messages reach agents without messaging tools, receive-only; see F1b).
- **Ordering.** Urgent operator messages, then directives, then direct messages, then broadcasts. These are injection priorities 250 / 230 / 200 / 190; the pipeline sorts descending.
- **Receipts.** After a model call that included delivered messages, the loop calls `Bus::mark_seen`.

## 6. Transcript events (closes M2)

```rust
// rupu-transcript/src/event.rs
Event::Message {
    id: String, direction: Direction /* Sent | Received */, from: String, from_codename: Option<String>,
    to: Option<String>, kind: String, body: String, urgent: bool,
    via: Via /* Tool | Injected */, delivery: Option<Value>,
}
Event::Injected { source: String, kind: String, cadence: String, content: String, message_ids: Vec<String> }
```

- **Sent** is written by the agent loop right after a messaging tool's `ToolResult`, carrying the returned id and delivery.
- **Received** is written when a collector delivers it, one event per message, before the model call that sees it.
- **`Injected`** records **every `Once` injection** from any collector (not only messaging). It fixes `collector.rs:13`'s false claim, and lets replay and continuation reproduce what the model saw: `continuation::replay` re-inserts `Injected` content at the recorded position. `EveryTurn` injections are recorded only when their content changes from the previous turn (a digest comparison), so a standing directive doesn't write one line per turn.
- **Old readers** read both as `Event::Unknown` and keep them. Update `docs/transcript-schema.md`.

## 7. Workflow opt-in (FD2) and process children

```yaml
# workflow YAML
messaging: true                 # grants board.claim/release/post/read, msg.send to every agent step + unit
# or
messaging: { tools: [board.post, board.read, msg.send] }
```

- The orchestrator parses it into `Workflow.messaging`. `RunAssembler` adds it as `AmbientGrant { reason: "ambient:workflow_messaging" }` for `Origin::WorkflowStep`.
- **Process children** (`dispatch kind: agent`, W7) get `--message-space <dir> --participant <address>` on their `RunArgv` (W6). The old `--fleet-run-dir/--fleet-participant` stay as aliases. Children therefore share their parent's root (FD6).
- `workflow` children are their own root: the child workflow gets a fresh space.

**Example (matt's "a lead in a workflow"):**

```yaml
name: parallel-review
messaging: true
steps:
  - id: coordinate
    agent: review-lead        # tools: [dispatch, join, board.directive, board.read, msg.send]
    prompt: "Split crates/ among reviewers with dispatch, steer them with directives, merge their findings."
```

`review-lead` dispatches `subagent`/`agent` children. They join the same space, receive its directives, post on the board, and message `parent`.

## 8. Files

| File | Change |
|---|---|
| `rupu-fleet/src/{bus,log,participants,address}.rs` | writer API, log schema, registry, address grammar + resolution |
| `rupu-fleet/src/mailbox.rs` | address-derived dir names; `sanitize` kept for legacy reads |
| `rupu-runtime/src/services/messaging.rs` | collectors (moved from `rupu-agentiflow/src/collectors.rs`); space resolution per `Origin` |
| `rupu-runtime/src/assembly/defaults.rs` | message-space row; participant join/leave; ambient grant for `messaging:` |
| `rupu-tools/src/{board,msg}/` | return `id` + `delivery`; address validation |
| `rupu-agent/src/runner.rs` | `Event::Message` (sent/received), `Event::Injected`, `mark_seen` |
| `rupu-agent/src/continuation.rs`, `replay.rs` | replay `Injected` |
| `rupu-transcript/src/event.rs`, `docs/transcript-schema.md` | new events |
| `rupu-orchestrator/src/workflow.rs` | `messaging:` key (parse + validate) |
| `rupu-runtime/src/argv.rs`, `rupu-cli/src/cmd/run.rs` | `--message-space` / `--participant` (+ aliases) |
| `docs/` | new `docs/messaging.md` operator doc (addresses, kinds, delivery, retention) |

## 9. Tests

1. **`log_is_superset`** (`rupu-fleet`): every `Bus` method appends exactly one log line before any working-state write. A crash-injected failure between the two leaves the log line and no working state.
2. **`address_resolution`**: table tests covering participant, codename, `role:`, bare role, `step:`, `parent`, `lead` in a workflow (undelivered), a left participant (undelivered + logged), and `broadcast`.
3. **`workflow_parallel_branches_talk`** (`rupu-orchestrator` it, mock provider): with `messaging: true`, branch A `msg.send to: role:<B's role>` → B's next model call sees the injection. Both transcripts carry `Event::Message`; the log has a `seen` line.
4. **`dispatch_child_shares_space`** (`rupu-launch` it): an `agent` child posts to the board, and the parent's next turn sees it via `board.read`.
5. **`injected_replay_roundtrip`** (`rupu-agent`): a run with `Once` injections, continued via `rupu run --continue`, rebuilds byte-identical provider messages.
6. **`from_cannot_be_spoofed`**: a `msg.send` input containing `"from": "operator"` is rejected as an unknown field (schema), and the logged `from` is the caller's address.
7. **`log_size_cap`**: past `log_max_mb`, `msg.send` returns an explicit error and a `messaging_full` notice is written.
8. **`legacy_flow_reads`**: a pre-F1 agentiflow run dir (no log) still works for `board.read`, and its participants resolve by `<agent>#n` aliases.

## 10. Acceptance

- A workflow with `messaging: true` and two parallel agent steps can coordinate. Every message appears in the log, in both transcripts, and with a `seen` receipt.
- No message, of any kind or to any address, is ever dropped without a log line saying so.
- CLAUDE.md is updated (`rupu-fleet`, `rupu-runtime`, `rupu-transcript` entries), along with `docs/messaging.md`.
