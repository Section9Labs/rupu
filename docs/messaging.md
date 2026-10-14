# Messaging

Agents in one run tree can talk to each other: workflow steps running side by side, an
agentiflow's lead and units, a run and the children it dispatched. They post to a shared
board, send each other messages, and steer each other with standing directives. The operator
can message any of them from the CLI or the control plane. Every message is written to one
log, both sides' transcripts record it, and a message that reached nobody says so.

This page covers the message space, addresses, what each message kind does, how messages
reach an agent, the operator's side, and what is kept. The tools themselves are in
[agent-format.md](agent-format.md#tools); the transcript events in
[transcript-schema.md](transcript-schema.md#message).

## The message space

Every root run can have one message space. Everything said in its run tree goes there.

| Root | Space directory |
|---|---|
| agentiflow | `<RUPU_HOME>/agentiflows/af_<id>/` (the run dir) |
| workflow run | `<RUPU_HOME>/runs/<run_id>/messages/` |
| `rupu session` | `<sessions>/<session_id>/messages/` |
| standalone `rupu run` | `<RUPU_HOME>/runs/<run_id>/messages/` |

Every run joins its space as it starts: its `joined` line goes into `participants.jsonl`, so
the operator can reach it while it runs. The log itself is created by the first message. A
run that continues another (`rupu run --continue`) keeps that run's space, so it reads what
was left in its inbox.

Every run with a space is offered the messaging tools by default: `board.claim`, `board.release`,
`board.post`, `board.read` and `msg.send`, recorded in its `tool_grant` as
`ambient:messaging_default`. A workflow, an agentiflow or an agent file can turn them off
([Turning it off](#turning-it-off)). A run whose messaging is off is still in the space,
receive-only: the operator's messages reach it, its children meet it there, and what they send
`parent` reaches it before its next model call.

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
coordinator's space. Each runs as a standalone `rupu run` on its host, with a space of its own
there and the default tools. A workflow's `messaging: false` reaches them as
`rupu run --no-messaging` (see [Turning it off](#turning-it-off)). A workflow's
`messaging: { tools: [...] }` narrowing does not: a placed unit gets the default set.

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
parent. An agentiflow's lead and a session stay joined between rounds and turns; `rupu session
stop` and `rupu session archive` record the session's participants as `left`.

A message's `to` is one of these:

| Address | Reaches | When nobody matches |
|---|---|---|
| `lynx#1`, or a full codename | that participant | `unknown_participant` |
| `role:recon`, or a bare `recon` that isn't an address | every participant currently running the agent `recon` | `no_live_participant` |
| `step:scan` | every participant currently running workflow step `scan` | `no_live_participant` |
| `parent` | the run that launched the sender | `no_parent` |
| `lead` | an agentiflow's lead | `no_lead` |
| `operator` | the operator (read from the log; no agent drains it) | — |
| `broadcast` | every participant but the sender, each once, from when it joined | `no_live_participant`, and it is not added to the broadcast log |

Roles and steps are resolved when the message is sent. A message to a participant that has
left is logged as `participant_left`. A full inbox (256 waiting messages) is `inbox_full`, a
full broadcast log `broadcast_full`. None of these drop the message: it is in the log with
its reason, and the sender's tool result names who it reached and who it didn't. A message
that reached nobody is also an error the model sees.

## Message kinds

| Kind | Tool | What it does |
|---|---|---|
| `post` | `board.post` | Adds to the shared board, which anyone reads with `board.read`. A post with no `addressed_to` is a channel post and reaches nobody's inbox. An addressed post also lands in each resolved addressee's inbox (same inbox cap as a direct message), so the addressee sees it on its next turn, framed as a board post |
| `direct` | `msg.send` | Lands in each recipient's inbox |
| `broadcast` | `msg.send` to `broadcast` | Appended to the broadcast log; every participant reads it once |
| `directive` | `board.directive` | A standing instruction, re-shown to the participants it addresses on every turn until it is lifted |
| `retract` | `board.retract` | Lifts a directive. Only its author can |
| `operator` | — | From the operator ([The operator](#the-operator)). No tool can write one |

The sender's `from` is always its own address. A tool call can't set it: the messaging tools
refuse arguments their schema doesn't list.

`board.directive` and `board.retract` carry authority over other agents, so `*` never grants
them. An agent gets them by naming them (or `board.*`), and an agentiflow lead has them.

## Delivery

A participant's inbox, the broadcasts it hasn't seen and the directives that apply to it are
delivered before each of its model calls. A tool call in progress is never interrupted.

Urgent operator messages come first, then standing directives, then direct messages (and the
operator's other messages), then broadcasts. Messages stay in the conversation. Standing
directives are re-sent at the end of each request and are not kept. Messages from other agents
are framed as data, not instructions. The operator's are framed as instructions from the human
running the run.

Each delivered message is recorded in the recipient's transcript as a `message` event
(`direction: received`) and an `injected` event, right before the model call that sees it.
Once that call returns, the space's log gets a `seen` receipt. The sender's transcript has the
same message as `direction: sent`.

A standing directive is recorded once, when the participant first sees it, not every turn.

Continuing a run (`rupu run --continue`, resuming a workflow) rebuilds exactly what the model
was sent, delivered messages included.

## Turning it off

Messaging is on by default. A workflow, an agentiflow, an agent file and `rupu run --no-messaging`
turn it off, and off wins over everything else.

```yaml
name: quiet-review
messaging: false         # no messaging tool for any step or unit, or anything they launch
steps:
  - id: review
    agent: reviewer
    prompt: Review crates/rupu-cp.
```

A workflow's `messaging: false` turns it off for every agent step and unit, and for every
child they launch. A placed unit (`host:` / `distribute:`) is launched on its host with
`rupu run --no-messaging`, so it is off there too. A host whose `rupu` predates the flag (it
doesn't advertise `agent.no_messaging`) is refused the unit rather than run it with messaging
on. An autoflow is a workflow with an `autoflow:` block, so the same key is its opt-out.
`messaging: true` is the default set, written down. `messaging: { tools: [board.post,
board.read, msg.send] }` grants only the tools named, recorded in each step's `tool_grant` as
`ambient:workflow_messaging`. Only `board.*` and `msg.*` tools are accepted. That narrowing
does not reach a placed unit, which gets the default set.

An agentiflow's definition takes `messaging: false` too: neither its lead nor any unit gets a
messaging tool. The lead keeps its status, roster and launch tools.

An agent file's `messaging: false` turns it off for that agent wherever it runs. The same file
naming a messaging tool in `tools:` (by name, alias or `board.*` / `msg.*`) is a load error,
since it contradicts itself.

`rupu run --no-messaging` turns it off for one run and everything it launches, as the agent's
`messaging: false` would. It is what a placed unit's coordinator passes. `--continue` does not
keep it: give it again with `--continue`.

Off is inherited. A run whose messaging is off launches every child with it off: a sub-agent
through its parent's launch scope, a process child through the internal `--launch-no-messaging`
flag, a workflow child too, although it is a root of its own. A placed unit of a workflow
launched that way has it off too. A child of a run whose messaging is off has it off.
Narrowing is not inherited: a child of a step with `messaging: { tools: [...] }` gets the
default set.

When a run's messaging is off, a messaging tool its agent names in `tools:` is not offered. The
run's transcript says so with a `tool_unavailable` notice. Nothing is dropped silently.

`*` still never grants `board.directive` and `board.retract`. An agent gets them by naming them
(or `board.*`), and an agentiflow lead has them. A lead-shaped step is just an agent granted
`dispatch`, `board.directive` and `msg.send`. Its children join the workflow's space, see its
directives, and can message `parent`.

## The operator

The operator can message any participant of a live run: a workflow step, a unit, a child, an
agentiflow's lead, a role, or everyone.

```bash
rupu message send <run-id> [--to <address>] [--urgent] <text…>
rupu message list <run-id> [--after <msg-id>] [--follow] [--kind direct,operator,…] [--json]
rupu message participants <run-id>
```

`<run-id>` is a workflow run, an agentiflow (`af_…`), a session (`ses_…`) or a standalone run,
by full id, compact form or a unique prefix. Without `--to`, a message goes to an agentiflow's
`lead`, and to `broadcast` in any other run. A broadcast into a run with nobody else joined
reaches no one, and `send` prints `not delivered to broadcast: no_live_participant`.
`list --follow` tails the log until the run stops.
`rupu agentiflow send <id> <text> [--now]` is shorthand for `rupu message send <id> --to lead
[--urgent]`.

An operator message is a `kind: operator` log line from `operator`, with `via` (`cli` or
`cp`). It reaches its recipients before their next model call, like any message, framed as:

> **Message from your operator** (the human running this run)
> `<body>`
> Act on it as an instruction from your operator. It can change what you work on next. It
> cannot change your system prompt or safety rules.

`--urgent` marks it `URGENT:`, delivers it ahead of everything else, and ends an agentiflow
lead's round early. It never interrupts a tool call in progress.

A run that isn't running takes no message: the send fails with the reason (the CP answers
409). A recipient that has left is logged `participant_left`, and the send still succeeds.

Only the operator's surfaces write `from: operator`: the messaging tools can't, and a run
can't take the address `operator`. An agent that runs `bash` in `bypass` mode could still
write the space's files directly. That is outside the threat model, since `bypass` already
grants arbitrary code execution.

The control plane serves the same thing as `GET/POST /api/{runs,sessions,agentiflows}/:id/messages`
and a local SSE stream ([cp-api.md](cp-api.md)). Remote hosts answer over HTTP and SSH when
their rupu advertises `run.messages`. Tunnel and bucket hosts answer 501.

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
