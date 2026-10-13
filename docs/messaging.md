# Messaging

Agents in one run tree can talk to each other: workflow steps running side by side, an
agentiflow's lead and units, a run and the children it dispatched. They post to a shared
board, send each other messages, and steer each other with standing directives. Every message
is written to one log, both sides' transcripts record it, and a message that reached nobody
says so.

This page covers the message space, addresses, what each message kind does, how messages
reach an agent, and what is kept. The tools themselves are in
[agent-format.md](agent-format.md#tools); the transcript events in
[transcript-schema.md](transcript-schema.md#message).

## The message space

Every root run can have one message space. Everything said in its run tree goes there.

| Root | Space directory | Created |
|---|---|---|
| agentiflow | `<RUPU_HOME>/agentiflows/af_<id>/` (the run dir) | at flow start |
| workflow run | `<RUPU_HOME>/runs/<run_id>/messages/` | when its first step starts, if the workflow sets `messaging:`, a step's agent names a messaging tool, or the space already exists |
| `rupu session` | `<sessions>/<session_id>/messages/` | on the session's first message or first child |
| standalone `rupu run` | `<RUPU_HOME>/runs/<run_id>/messages/` | on the run's first message or first child |

A run that never sends a message and never launches a child creates nothing.

```
<space>/
  log.jsonl            # every message, append-only, never drained
  log.jsonl.lock
  participants.jsonl   # who joined and left
  board/               # posts.jsonl, directives.jsonl, claims/
  mailboxes/           # per-participant inboxes, the broadcast log and cursors
```

Children share their parent's space. An in-process sub-agent and a process `agent` child (one
started by `dispatch`) join it. A `workflow` child is a root of its own and gets a fresh
space. There is no messaging between roots.

Placed units (`host:` / `distribute:`) run on another machine and are not in the
coordinator's space. A workflow's `messaging:` key does not reach them.

## Participants and addresses

A run joins its space as a participant. Its address is its codename below the space's root,
with retry attempts dropped, so a retried step keeps its address:

| Run | Codename | Address |
|---|---|---|
| a workflow step | `cobalt-harbor/lynx` | `lynx` |
| a `for_each` unit | `cobalt-harbor/lynx#2` | `lynx#2` |
| a step's sub-agent | `cobalt-harbor/lynx>otter#1` | `lynx>otter#1` |
| an agentiflow's lead | `cobalt-harbor/heron` | `lead` |
| an agentiflow unit | `cobalt-harbor/heron>lynx#1` | `lynx#1` |

`participants.jsonl` records every `joined` and `left`, with the run's agent, run id, step and
parent. An agentiflow's lead and a session stay joined between rounds and turns.

A message's `to` is one of these:

| Address | Reaches | When nobody matches |
|---|---|---|
| `lynx#1`, or a full codename | that participant | `unknown_participant` |
| `role:recon`, or a bare `recon` that isn't an address | every participant currently running the agent `recon` | `no_live_participant` |
| `step:scan` | every participant currently running workflow step `scan` | `no_live_participant` |
| `parent` | the run that launched the sender | `no_parent` |
| `lead` | an agentiflow's lead | `no_lead` |
| `operator` | the operator (read from the log; no agent drains it) | — |
| `broadcast` | every participant, each once, from when it joined | — |

Roles and steps are resolved when the message is sent. A message to a participant that has
left is logged as `participant_left`. A full inbox (256 waiting messages) is `inbox_full`, a
full broadcast log `broadcast_full`. None of these drop the message: it is in the log with
its reason, and the sender's tool result names who it reached and who it didn't. A message
that reached nobody is also an error the model sees.

## Message kinds

| Kind | Tool | What it does |
|---|---|---|
| `post` | `board.post` | Adds to the shared board, which anyone reads with `board.read`. A post with no `addressed_to` is a channel post. Posts are not pushed into turns |
| `direct` | `msg.send` | Lands in each recipient's inbox |
| `broadcast` | `msg.send` to `broadcast` | Appended to the broadcast log; every participant reads it once |
| `directive` | `board.directive` | A standing instruction, re-shown to the participants it addresses on every turn until it is lifted |
| `retract` | `board.retract` | Lifts a directive. Only its author can |
| `operator` | — | From the operator (F1b) |

The sender's `from` is always its own address. A tool call can't set it: the messaging tools
refuse arguments their schema doesn't list.

`board.directive` and `board.retract` carry authority over other agents, so `*` never grants
them. An agent gets them by naming them (or `board.*`), and an agentiflow lead has them.

## Delivery

A participant's inbox, the broadcasts it hasn't seen and the directives that apply to it are
delivered before each of its model calls. A tool call in progress is never interrupted.

Direct messages come before broadcasts. Both stay in the conversation. Standing directives are
re-sent at the end of each request and are not kept. Messages from other agents are framed as
data, not instructions.

Each delivered message is recorded in the recipient's transcript as a `message` event
(`direction: received`) and an `injected` event, right before the model call that sees it.
Once that call returns, the space's log gets a `seen` receipt. The sender's transcript has the
same message as `direction: sent`.

A standing directive is recorded once, when the participant first sees it, not every turn.

Continuing a run (`rupu run --continue`, resuming a workflow) rebuilds exactly what the model
was sent, delivered messages included.

## Turning it on in a workflow

```yaml
name: parallel-review
messaging: true          # board.claim/release/post/read and msg.send for every agent step
steps:
  - id: review
    max_parallel: 2
    parallel:
      - id: api
        agent: reviewer
        prompt: Review crates/rupu-cp. Tell role:reviewer what you find that crosses crates.
      - id: core
        agent: reviewer
        prompt: Review crates/rupu-runtime. Tell role:reviewer what you find that crosses crates.
```

`messaging: { tools: [board.post, board.read, msg.send] }` grants only the tools named. Only
`board.*` and `msg.*` tools are accepted. The grant is recorded in each step's `tool_grant`
as `ambient:workflow_messaging`. It covers the workflow's own agent steps and units, not the
children they launch: a child gets the tools its own `tools:` names.

An agent can also opt in itself by naming a messaging tool in `tools:`. It joins the space
whatever run it is in.

A lead-shaped step is just an agent granted `dispatch`, `board.directive` and `msg.send`. Its
children join the workflow's space, see its directives, and can message `parent`.

## What is kept

The log is the record. It is append-only and never trimmed. Past `[messaging].log_max_mb`
(default 64), further sends fail to their sender with an error and a `messaging_full` notice
in its transcript; receipts are still written. See
[configuration.md](configuration.md#messaging).

An agentiflow recorded before the log existed has no `log.jsonl`. Its board and inboxes still
work, and its units are still reached by the `<agent>#<n>` names they were given then.

## How a child joins

A process child is started with `rupu run --message-space <dir> --participant <address>`.
The flags are internal. An agentiflow's run dir makes the run a pool unit of that flow; any
other space makes it a child sharing its parent's. `--fleet-run-dir` and
`--fleet-participant` are their old spellings and still work.
