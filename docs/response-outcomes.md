# Response outcomes and recovery

A model's reply does not always give the agent what it needs. The reply can be
cut off at the output limit, refused, blocked by a safety filter, empty, or
stopped on a tool call that cannot be parsed. The provider can also fail the
request outright: rate limits, overload, a quota, a model that is gone.

rupu treats each of these as a typed **outcome**. It records the outcome in the
run's transcript, tries to recover from it in a fixed order (the **ladder**), and
fails the run with a message that says what to do next only when nothing is
left to try. This page describes what ships today.

Design background: `docs/superpowers/specs/2026-10-01-rupu-response-outcomes-design.md`.

---

## 1. What an outcome is

Every reply and every provider error is classified. A normal reply (an
`end_turn`, `tool_use` or `stop_sequence` stop with usable text or a tool call)
has no outcome. Anything else gets one of these classes:

| Class | Severity | What happened |
| --- | --- | --- |
| `pause_turn` | info | The provider paused a server-side tool loop and expects the conversation to be sent back to continue |
| `max_tokens` | error | The reply hit the output limit. When it cut a tool call off mid-arguments, the outcome says which tool |
| `context_window_exceeded` | error | The reply stopped because the context window is full |
| `refusal` | error | The model refused. The title carries the provider's category when it gave one (`refused · cyber`) |
| `safety` | error | A safety filter blocked the reply |
| `malformed_tool_call` | error | The model called a tool with arguments that could not be used |
| `incomplete` | error | The provider reported the reply as incomplete. Also what a reply that is still paused after every continuation becomes (handled slightly differently; see the notes below the ladder table) |
| `empty_reply` | error | A normal stop with no text and no tool call |
| `unrecognized_stop` | warning | The provider sent a stop reason rupu does not know. The raw value is kept |
| `unreported_stop` | warning | The provider reported no stop reason at all |
| `provider_error` | error | The request failed. The `error_class` field carries the normalized class: `rate_limited`, `overloaded`, `server`, `timeout`, `context_overflow`, `quota`, `not_found`, `policy`, `auth`, `permission`, `invalid_request`, `too_large` or `unrecognized` |

Severity decides what the run does next. A **warning** outcome never fails a run
on its own: the reply is kept, the outcome is recorded so the odd stop is
visible, and the run carries on. The one **info** outcome, `pause_turn`, is
handled in place (section 2). An **error** outcome enters the ladder.

### How an outcome appears in a transcript

Each outcome is one `outcome` line, written ahead of the turn's content. Every
recovery step taken for it is one `recovery` line, pointing back to the outcome
by id. Read in order, the pair is a timeline:

```json
{"type":"outcome","data":{"turn_idx":3,"outcome":{"id":"oc_1","class":"max_tokens","severity":"error","title":"truncated · output limit","wire":{"provider":"anthropic","value":"max_tokens"}}}}
{"type":"recovery","data":{"outcome_id":"oc_1","rung":0,"action":"continued","attempt":1,"budget":3,"continues_output":true}}
{"type":"outcome","data":{"turn_idx":5,"outcome":{"id":"oc_2","class":"refusal","severity":"error","title":"refused · cyber","wire":{"provider":"anthropic","value":"refusal"}}}}
{"type":"recovery","data":{"outcome_id":"oc_2","rung":1,"action":"fell_back","provider":"anthropic","model":"claude-opus-5"}}
```

Outcome ids are run-local (`oc_1`, `oc_2`, ...). The final `run_complete` line of
a run that failed on an outcome carries that outcome too. The field-level
reference is in [transcript-schema.md](transcript-schema.md).

---

## 2. The ladder

When an error outcome occurs, rupu works down four rungs and stops at the first
one that gets the run moving again.

- **Rung 0: in place.** Retry on the same model with a note or a smaller fix.
- **Rung 1: a fallback model on the same provider.**
- **Rung 2: a fallback model on another provider.**
- **Rung 3: fail the run**, with a hint that names the next step (section 5).

What each outcome does on rung 0, and which later rungs it may use:

| Outcome | Rung 0 | Rung-0 budget | Rung 1 | Rung 2 | The partial reply |
| --- | --- | --- | --- | --- | --- |
| `pause_turn` | Send the paused conversation back and join the continuation to it | 5 | no | no | kept |
| `max_tokens` | Tell the model to continue exactly where it stopped | 3 | yes | yes | kept |
| `max_tokens` that cut a tool call off | Compact the history, then retry at the model's full output cap. Applies only when the cap had been lowered to make input and output fit; otherwise rung 0 is skipped | 1 | yes | yes | discarded |
| `context_window_exceeded` | Compact the history, then continue | 3 | yes | yes | kept |
| `refusal`, `safety` | none | 0 | yes | yes | discarded |
| `malformed_tool_call` | Tell the model what was wrong with the call and ask for it again | 2 | yes | yes | kept |
| `empty_reply` | Tell the model its reply was empty and ask it to continue | 1 | yes | yes | none to keep |
| `incomplete` | Retry the turn | 1 | yes | yes | discarded |
| `unrecognized_stop`, `unreported_stop` | none; a warning, so the run carries on | 0 | no | no | kept |
| `provider_error`: `rate_limited`, `overloaded`, `server`, `timeout`, `context_overflow` | The existing retry, backoff and overflow compaction run first; the ladder adds a hop only after they give up | n/a | no | yes | n/a |
| `provider_error`: `quota` | none | 0 | no | yes | n/a |
| `provider_error`: `not_found`, `policy` | none | 0 | yes | yes | n/a |
| `provider_error`: `auth`, `permission`, `invalid_request`, `too_large`, `unrecognized` | none; retrying elsewhere would not help | 0 | no | no | n/a |

Notes on the budgets:

- A rung-0 budget is per logical turn. A turn that is continued three times and
  then moves on through tool calls starts the next turn with fresh budgets.
- A reply still paused after its five continuations is reclassified as
  `incomplete` (titled `incomplete reply · still paused after 5 continuations`).
  It skips the rung-0 retry and goes straight to the fallback hops (rungs 1 and
  2), and the paused partial reply is kept in the conversation, not discarded.
  With no fallback left, the run fails.
- A **discarded** partial reply is written to the transcript but left out of the
  conversation: it is not sent back to the model, and a later `rupu run
  --continue` does not replay it.
- Rung 0 and every hop count against one ceiling of **20 recovery actions per
  run**. At the ceiling the run goes straight to rung 3.
- A fallback that cannot be built (its provider is not configured, say) is
  recorded as `skipped` with the reason, counts as an action, and the next
  fallback is tried.
- A hop is sticky: once the run has moved to a fallback, later turns stay on it.
  When the conversation ends on an assistant message, the fallback model is told
  that a previous attempt stopped and why, and is asked to check the state and
  continue.
- Rungs 1 and 2 need a fallback chain and a way to build a provider for it. A
  run with no chain still gets rung 0 and then rung 3.

---

## 3. Configuring fallbacks

A fallback chain is an ordered list of models. Each entry names a `model` and,
optionally, a `provider`.

### In an agent file

```yaml
---
name: dependency-auditor
provider: anthropic
model: claude-opus-5
fallbacks:
  - model: claude-sonnet-5-5          # same provider as the run
  - provider: openai-codex            # another provider
    model: gpt-5.6-cyber
---
```

### In `config.toml`

```toml
[recovery]
fallbacks = [
  { model = "claude-sonnet-5-5" },
  { provider = "openai-codex", model = "gpt-5.6-cyber" },
]
```

Both forms reject unknown keys, so a typo in an entry is a load error and not a
silent no-op.

### Precedence

An agent's `fallbacks:` wins over `[recovery].fallbacks`. They are not merged:
when the agent declares a chain, the config table is not consulted for that
agent. An agent that declares `fallbacks: []` has an empty chain. No chain in
either place means rungs 1 and 2 are unavailable.

### Which rung an entry belongs to

An entry without a `provider` means **the provider the run started on**, even if
the run has since moved to another provider. It is a rung-1 candidate. An entry
whose provider is the origin's provider is also rung 1. Every other entry is
rung 2. Within a rung, the chain's order decides. The model the run started on
and any entry already tried are never offered again.

The split matters because an outcome decides which rungs it may use. A
`rate_limited` error skips the same-provider models (the provider is the
problem) and goes to rung 2. A `refusal` may use both.

### What a hop uses

A hop is built through the same credential resolver, `[providers.<name>]` table
and network logging as the run's own provider, so a same-provider hop differs
only in model. Three details:

- The agent's `anthropicOauthPrefix` and `anthropicPromptCache` settings carry
  over to every hop. Providers they do not apply to ignore them.
- The agent's `auth:` mode applies only to a hop on the provider the run started
  on. It says how the agent reaches that provider and says nothing about another
  provider's credentials.
- A hop does not inherit the agent's `maxTokens` or `contextWindowTokens`.
  Those were chosen for the original model. The hop's limits come from
  `[[providers.X.models]]` and the provider's model list.
- A hop to a different model does not send the agent's `contextWindow` or
  `anthropicSpeed` either: the 1M beta and fast mode exist only on some
  models. A hop that keeps the original model's name, on another provider,
  keeps both.

### Cross-provider fallback sends the conversation to another vendor

A rung-2 hop sends the whole conversation, including file contents and tool
output the agent has read, to a different company's API under that provider's
terms. Put a provider in a chain only when the data in the run is allowed to go
there. If it is not, leave it out of `fallbacks:`.

---

## 4. Server-side fallback

Anthropic can fall back to another model on its own side when the model you
asked for refuses. rupu opts in for you, which saves a round trip and keeps the
reply in one request.

- **Models:** `claude-fable-5`, `claude-fable-5-1`, `claude-opus-5`,
  `claude-opus-5-5` and `claude-sonnet-5-5`. A `[1m]` suffix on the model id is
  ignored when matching. The list is exact; a newer model is added deliberately.
- **Auth:** API-key auth only. A run on Anthropic SSO (OAuth) never sends the
  opt-in.
- **On the wire:** the request carries the `server-side-fallback-2026-07-01`
  beta and `"fallbacks": "default"`.
- **In the transcript:** when the fallback answers, the turn proceeds normally.
  The refusal is recorded as an `outcome` titled `refused · served by <model>`,
  followed by a `recovery` with action `served_by_fallback` on rung 1. The
  reply's `fallback` boundary is written in place as an `assistant_block`
  event, and replay puts it back where it was: Anthropic requires it echoed in
  that position on the next request.

### Turning it off

```toml
[recovery]
server_side_fallback = false
```

The default is `true`. The setting applies to the run's own provider and to any
Anthropic hop.

### The disable notice

If the API refuses the opt-in (a 400 that names `fallbacks`, which an account or
gateway that does not accept the beta returns), rupu stops sending it and retries
the request once. The transcript gets a notice of kind
`server_side_fallback_disabled`:

> server-side fallback refused by the API — disabled for this run

The run is not affected otherwise. To avoid the refused request on every run,
set `server_side_fallback = false` for that account.

---

## 5. When nothing is left

When the ladder runs out, the run ends with status `error`. The failure message
is the outcome's title followed by a hint. For a provider error it is instead the
provider's own error text (`provider: <error>`, as before the ladder existed)
followed by the hint. The hint depends on where the run was started:

- **`rupu run <agent>`** (a dispatched sub-agent gets the same hint):

  ```
  refused · cyber; no recovery left — continue with another model: rupu run dependency-auditor --continue <run id> --model <model> [--provider <provider>]
  ```

- **A workflow step:**

  ```
  no recovery left — add fallbacks: to the agent or [recovery].fallbacks to try other models
  ```

- **A session turn:**

  ```
  no recovery left — send another message to continue, or start a new session on another model
  ```

  A failed session turn keeps its conversation, so the next message continues it.

A failed run's final output is empty: its last text is an interim message or a
cut-off reply, not an answer. A workflow step tolerated by `continue_on_error:`
publishes `steps.<id>.output` as `""` and the reason as `steps.<id>.error`; a
failed dispatched sub-agent returns `"ok": false`, `"output": ""` and the reason
as `"error"` to the agent that dispatched it.

### Continuing a failed run on another model

```
rupu run <agent> --continue <agent_run_id> --model <model> [--provider <provider>]
```

A run that failed on an outcome has an intact conversation, so it can be picked
up on a different model. The rebuilt conversation leaves out any discarded
partial reply. The new model is told that the earlier attempt stopped, why, and
which model it is continuing on.

- It applies only to a run that ended in failure **with** an outcome recorded.
  A run that failed for another reason (max turns, say) cannot be continued this
  way. Without `--model` or `--provider`, `--continue` still treats a failed run
  as finished with an error and refuses it, as before.
- `--model` and `--provider` override the agent's own `model:` and `provider:`
  for any `rupu run`, not only a continuation.
- They also drop the agent's settings that were chosen for its own model. A
  different `--model` drops the agent's `maxTokens`, `contextWindowTokens`,
  `compactAtPercent` and `contextWindow`. A different
  `--provider` drops the agent's `auth:` mode. The new model's limits are
  discovered instead.
- Run it from the same project as the original run, and give it a fresh run id.

[agent-format.md](agent-format.md#continuing-an-interrupted-run) covers
`--continue` for runs that were interrupted rather than failed.

---

## 6. Where it shows

**CLI.** The transcript viewers (`rupu transcript show`, `rupu session`,
`rupu watch`), the live run view, the workflow printer and `rupu autoflow serve`
print each event as one line, with a glyph for the outcome severity and a
recovery arrow:

```
✗ outcome   truncated · output limit
↺ recovery  rung 0 · continued 1/3
✗ outcome   refused · cyber
↺ recovery  rung 1 · fell back to anthropic/claude-opus-5
```

The exact columns follow each viewer's own layout. The CLI rows use `✗` for an
error, `!` for a warning, `●` for info and `↺` for a recovery. The plain-text
one-line form that `rupu-transcript` provides for other renderers uses `·` for
info instead of `●`.

Outcome and recovery rows are cut at 96 columns. An event type this version of
rupu does not know, written by a newer rupu, prints as an `event` row reading
`unrecognized event · <type>` followed by its data, cut at 240 columns. Its `type`
and `data` are preserved when a transcript is copied or re-written; other
top-level keys on that line are not.

A reply block that has no event of its own prints as a `block` row:
`served by fallback · <from> → <to>` for a server-side fallback boundary and
`unrecognized block · <type>` followed by the provider's raw JSON (cut at 240
columns) for a block rupu does not model. A block marked `abandoned` says so.

**Control plane.** The transcript view shows an outcome as a block with a
severity-colored edge, its title and its detail, and a recovery as a one-line
timeline entry. An unknown event type shows as an "unrecognized event" block with
its raw payload. A reply block with no event of its own shows as a one-line row
with the same text as the CLI's, and expands to its raw JSON.

Beyond the transcript, a failed workflow step, fan-out unit or run records the
outcome that caused it (its `cause`) on the run record, so a failure is stored as
"refused · cyber" and not only as an error string. The control plane does not
display `cause` yet.

The one-line forms are deliberately minimal. A fuller presentation (tones and
chips, an awaiting-recovery card, run-graph tooltips, `cause` in the run views)
is planned for a later release; the transcript data it will read is already what
is recorded today.
