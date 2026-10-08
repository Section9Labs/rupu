# F1 (feature): messaging for every run kind

- **Card:** F1 · **Depends on:** W1–W7 (it builds on `MessageBus` from W5, `Launcher`/codenames from W7, and `RunAssembler` from W3)
- **Status:** the design direction was agreed in conversation on 2026-10-07. This file is the *feature brief* the F1 session starts from. That session runs its own brainstorm on the open questions in §7 before writing a plan.

## 1. What matt asked for

- Agents in a **workflow** (parallel branches, fan-out units, long-running steps, children) can talk to each other in real time through both the **board** and **mailboxes**, just as agentiflow agents do.
- Messages belong in the **transcripts**: they are part of an agent's living history, and the UI should show them there as identifiable chat bubbles.
- The Messages tab shows **everything**, not just board posts. The **operator is a participant** who can message the lead, any agent, a role, or everyone.

## 2. What the deep dive found (the channel inventory)

There are ten operator↔agent and agent↔agent channels, each with its own store, identity, timing, framing and visibility. The full table is in 00-index §3.4. The parts F1 must resolve:

- **Missing from the CP.** Direct `msg.send`, broadcasts and the operator's own steering never appear. Inboxes are cleared on read (`Mailbox::drain` renames and deletes), so delivered messages cannot be shown afterwards.
- **Collector injections are not recorded** in transcripts, so replay diverges from what the model saw.
- **Dead letters:**
  - `msg.send` to `"parent"` or to a role is never drained;
  - role-addressed directives never match, because `role` is always `None`;
  - workflow units' `--fleet-participant` is discarded.
- **Inconsistent identities:**
  - steering has no author;
  - directives always say `"lead"`;
  - participants are `<agent>#n`, not codenames;
  - mailbox sanitizing collides `recon#1` with `recon-1`.
- **Two meanings of "broadcast":** a board post with no `addressed_to` (the UI's convention) versus the real broadcast log.

## 3. Direction

### 3.1 One message space per run root

Every **root run** gets one message space at `<root run dir>/messages/`. The root runs are a workflow run, an agentiflow, a session, and a standalone `rupu run` that has children. The space holds `board/`, `mailboxes/` and the canonical **`log.jsonl`**: an append-only log of every message of every kind (post, direct, broadcast, directive, retract, operator), each with a ULID `id`. The log is never trimmed or drained, which makes it the source the UI reads.

### 3.2 Addresses

- Participant addresses come from codenames (W7): the role tail `lynx#1`, unique within a root, or the full codename.
- Reserved addresses: `operator`, `broadcast`, `parent`, `lead` (the flow lead; in a workflow, undefined → recorded undeliverable).
- A role name (`recon`) is resolved **at send time** to that role's current participants.
- Delivery status is recorded in the log line (`delivered_to: [...]` or `undelivered: <reason>`), never dropped silently.

### 3.3 Delivery and framing

- **The collectors generalize.** The mailbox, broadcast and directive collectors attach to every agent run under a root with a message space whenever the agent is granted any `board.*` / `msg.*` tool or has mail waiting. The operator can therefore message an agent that has no messaging tools of its own: receive-only.
- **Agent → agent messages are framed as untrusted data,** as today.
- **Operator → agent messages are framed as operator instructions** (the trusted channel, like today's steering). `now: true` interrupts the agent's current turn at a safe boundary and delivers the message, generalizing agentiflow `send --now` to any running agent.
- The agentiflow `OperatorQueue` folds into this. The lead's round-prompt steering block becomes "operator messages addressed to the lead".

### 3.4 Transcripts

- `Event::Message { id, direction: sent | received, from, to, kind, body, via: tool | injected }` is written when an agent sends a message (its tool call) and when a collector delivers one.
- A general `Event::Injected { source, kind, content }` records every `Once` injection, which fixes replay fidelity for all collectors, not only messaging.
- Old readers see both events as `Unknown` and keep them.

### 3.5 UI

- **Run detail gains a Messages tab** for workflow runs and sessions, reusing `MessageFeed` and reading the log. The agentiflow tab switches to the log, with a legacy fallback to `board/*.jsonl` for flows recorded before F1.
- **The composer is the same everywhere:** to `{participant | role | broadcast}`, with a `now` toggle.
- **The transcript view renders `Message` events inline as chat bubbles** (sender codename tint, sent vs received, an @mention pill). Clicking one jumps to it in the channel, and back.
- **API:** `GET /api/runs/:id/messages` and `POST /api/runs/:id/messages` (operator send). The agentiflow `/messages` + `/steer` endpoints become aliases of these.

## 4. Fits the architecture

- **No new tool homes.** `board.*` and `msg.send` are already catalog tools after W5. F1 makes `MessageBus` present for more origins (`WorkflowStep`, `SessionTurn`, `SubAgent`, process children through `--fleet-run-dir` → generalized `--message-space`). That is one row in `RunAssembler::defaults_for`.
- **Workflows get no automatic grants.** An agent opts in through its `tools:` (`board.*`, `msg.send`), or a workflow step adds them with a new step key `grants:` (open question §7.2).

## 5. Out of scope for F1

- A messaging transport for remote placed units (`host:`/`distribute:`). They get W2's `tool_unavailable` notice until a transport exists.
- Cross-root messaging (two separate workflow runs talking).

## 6. Rough card split

- **F1a:** message space + log + addresses + delivery status + collectors for all origins, and the transcript events.
- **F1b:** the operator as a participant (send/now for any agent; steering folded in) + CP API.
- **F1c:** the web Messages tab for workflows/sessions, inline transcript bubbles, the agentiflow tab on the log. GUI work, so it stops for matt's visual check.

## 7. Open questions for the F1 brainstorm

1. Should a standalone `rupu run` with no children get a message space at all? It only matters for operator messages to a long single agent.
2. A workflow-level or step-level `grants:` key to add messaging tools without editing agent files: yes or no?
3. Retention: is the log capped per run (as broadcasts are today)? If so, what happens when it hits the cap?
4. Does `now` interrupt a turn that is mid-tool-call, or wait for the tool to finish?
