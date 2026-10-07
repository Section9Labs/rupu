# 05. Steps

A **step** is one statement in a flow. A **leaf step** performs a single operation. A **block** (chapter 06) contains other steps.

## 5.1 The uniform step shape

```
[name =] KEYWORD subject [-> Type] [(options)] [prose] [{ block }]
    modifier
    modifier
```

| Part | Rule |
|---|---|
| `name =` | Binds the step's typed result for later steps. The name must be unique within the enclosing `flow` or state, including nested blocks: every step is addressable by name, as in the graph. Unnamed steps get a generated, stable name (`step_3`) for the journal and graph. |
| `KEYWORD` | Always first on the line. Core and extension keywords share this one shape. |
| `subject` | Keyword-specific: an agent ref, a tool call, a command literal, an event pattern, a machine call, a prose string. |
| `-> Type` | Output contract. The raw result is parsed and validated at runtime against `Type`; failure raises `<kind>.output_invalid`. |
| `(options)` | `name: expr` pairs that configure the operation itself. Each keyword's option schema (core, or the extension manifest) is checked at compile time. |
| `prose` | A string or `"""` template, where the keyword takes one (an agent prompt, an approval question). |
| `{ block }` | For keywords that wrap other steps. |
| modifiers | Control *around* the operation (§5.4). They go on the same line, or on following lines starting with a modifier keyword. |

**Visibility of bindings.** A binding made inside a block stays visible after the block, because names are unique per flow. Its type reflects whether it is guaranteed to have run:

| Binding made inside | Type after the block |
|---|---|
| a `loop` (it always runs at least one pass), a `try` body with no falling-through `catch`, a `within` without `else`, a lock/semaphore/throttle/budget block | `T` (the last pass's value, for loops) |
| `while` and `for` bodies (they may run zero passes), `if`/`match` arms, `fork`/`race` branches (except unguarded branches of `join: all` + `branch_error: fail_fast`; a guarded branch is always `T?`), `try` bodies that can fall through, `during` bodies | `T?` |
| `map`, `pipeline`, `worklist` and `best_of` bodies | not visible: per-item values are reached only through the block's output |

The formatter rules:
- Options stay inline when they fit in 100 columns; otherwise one per line.
- A prose template goes last.
- Each modifier goes on its own indented line, unless the step has exactly one short modifier.

## 5.2 Core leaf steps

The core steps touch nothing outside the engine.

### `let`: a pure binding
```
let blocking = findings.filter(f => f.severity >= high)
```
- Immutable. Evaluated when reached, and recomputed on replay. It is never an effect.

### Assignment: update a `var`
```
attempts = attempts + 1
steering = steering + [c.body]
```

### `wait for`: wait durably for an event
```
merged = wait for github.pr.merged if event.pr.number == pr.number (timeout: 7d)
```
- The binding is typed by the event declaration.
- **Timeout:** raises `wait.timeout` (catchable).
- **What it matches:** events delivered to this instance (§4.5), following the event-processing order and the inbox policy table there.
  - Under `queue` (the default) or `latest`, a matching retained event that arrived *before* the wait began is taken immediately. This is a persistent trigger.
  - Under `drop`, only events arriving while the wait is active count. This is a transient trigger.
  - A consumed event is removed.
- **Cost:** none while waiting. The instance is unloaded until the event or timer arrives.

### `sleep`: a durable delay
```
sleep 2h
sleep until next("mon 09:00")
sleep until pr.created_at + 24h
```

### `emit`: publish an event to the host's event bus
```
emit release.ready { version: v, sha: build.sha }
```
Any machine may `trigger on` an emitted event. Emit is fire-and-forget, journaled, and delivered at least once.

### `send`: deliver to one live instance
```
send security_issue_owner[issue.ref] repair { reason: "manual fix landed" }
```
- `machine[key]` addresses an instance by its instance key.
- **No live instance:** if the target machine's `instance` policy would start one for this key, the message starts it; otherwise it raises `send.no_instance`.

### `call machine`: run a child instance
```
d = call machine deploy(env: "prod", sha: build.sha)
scan = call machine nightly_scan(repo: repo) detach      // independent; scan: ChildRef { id }
```
- **Result.** When the child ends with any outcome other than `failed` or `cancelled` (`completed`, `rejected`, `not_actionable`, …), the step succeeds and its value is the child's typed `output`. `outcome(d)` gives the child's outcome name.
- **Failure.** If the child fails, `call.failed` is raised, with the child's error as `cause`. If the child is cancelled, `call.cancelled` is raised.
- **`detach`** starts a **fully independent** instance. It isn't cancelled with the caller, isn't awaited, and doesn't block the caller's completion. The step returns `ChildRef { id: string }` immediately. This is the one deliberate exception to structured concurrency, for "start and forget". (It differs from `detach h` on an `async` handle; see §6.7.)
- **Dynamic calls.** `call machine (d.workflow)(d.inputs)` names the machine with an expression of type `string`. It is checked at runtime against the host catalogue, and its result is `json`.

### Flow call
```
rv = panel_review(subject: patch, panelists: reviewers, floor: high)
```
Inlined (§4.10).

### `raise` and `fail`
```
raise triage.ambiguous { issue: n, reason: "two components" }
fail "no files to audit"                                        // raises user.failed
```

### `assert`
```
assert files.size() <= max_files, "too many files: {{ files.size() }}"
```
Raises `assertion.failed` with the message.

### `note`: a visible annotation in the run timeline and graph
```
note "skipping {{ vendored.size() }} vendored files"
```

## 5.3 Extension leaf steps

Extensions register leaf steps with the same shape. The agentic extension adds `agent`, `tool`, `run`, `approve` and `ask`; chapter 09 specifies every option of each. A preview:

```
triage = agent @security-triager -> Triage (tools: [issues.get, issues.comments], max_turns: 40) """
  Issue: {{ fetch.title }}
  {{ fetch.body | truncate(8000) }}
"""

fetch  = tool issues.get(project: repo, number: issue)

audit  = run `cargo audit --json` -> AuditReport (parse: json, ok_exit: [0, 1], cwd: root)

ship   = approve "Ship the fix for #{{ n }}?" (quorum: 1, approvers: maintainers - author,
                                               form: { base: string = "main" })

scope  = ask "Which services are in scope?" (form: { services: list<string> }, from: security-team)
```

## 5.4 Modifiers

Modifiers attach to the step above them. They control timing, retries, failure routing, placement, compensation and caching. Any step can carry them: leaf steps, flow calls and blocks.

```
pr = tool scm.prs.create(owner: o, repo: r, title: t, body: b, head: h, base: base)
  place      host("worker-1", workspace: sync)
  timeout    2m
  retry      5x on tool.transient, backoff exp(5s, max: 2m), jitter
  catch tool.conflict { goto refine }
  catch any as e      { note "PR failed: {{ e.message }}"; yield null }
  after 24h keep_waiting { tool issues.comment(…, body: "still working on it") }
  after 72h  raise step.overdue
  compensate { tool scm.prs.close(number: pr.number) }
  cache      by [o, r, t] for 7d
```

### Order of application
From the inside out: **cache, then timeout, then retry, then catch, then after**.

1. **cache** is consulted before any attempt.
2. **timeout** bounds each attempt.
3. **retry** repeats failed attempts that match its filter.
4. **catch** handles the error that is left after the retries are exhausted.
5. **after** timers run over the whole step, including its retries.

`place` and `compensate` don't nest: `place` configures every attempt, and `compensate` is registered after success.

### `place`
```
place host("worker-1", workspace: sync)
place distribute(["w1", "w2"], workspace: sync)  // for map/pipeline/worklist/best_of item scopes
place any(pool: "linux-builders")               // host picks a host from a named pool
```
- **Where it's valid:** any extension step whose kind the host marks as placeable (agentic: `agent`, `tool`, `run`), and block item scopes.
- **`distribute`** assigns items round-robin. A failed placement is retried once on the next host before the item fails.
- **`workspace: sync | none`** is a host-defined materialisation; the default is `none`.

### `timeout <duration>`
Bounds each attempt. Expiry cancels the attempt (§8.6) and raises `timeout`.

### `retry`
```
retry 3x
retry 5x on tool.transient | provider.*, backoff exp(5s, max: 2m), jitter
retry 2x on agent.output_invalid, backoff fixed(0s)          // re-ask for valid JSON
```
| Part | Default |
|---|---|
| count | required |
| `on <pattern>` | `any` minus non-retryable kinds: `cancelled`, `assertion.failed`, `approval.*`, `budget.exhausted`, `limits.*` |
| `backoff fixed(d) \| linear(d) \| exp(d, max: d2)` | `exp(1s, max: 1m)` |
| `jitter` | off; when on, up to ±25% randomness (seeded, so replay is deterministic) |

- Each retry is a **new attempt**, with a new effect id suffix (`#a2`) and the **same idempotency key base**. A handler can therefore tell a retry from a resumed attempt. For an agent, a new attempt is a fresh conversation.
- **Agent output repair vs retry.** In-conversation correction of invalid output is the agent's `repair` option (§9.3), which happens *inside* one attempt. `agent.output_invalid` is raised only after the repairs are used up, and `retry on agent.output_invalid` then starts a fresh attempt.

### `catch`
```
catch tool.conflict          { goto refine }
catch approval.rejected | approval.timeout as e { …; end rejected }
catch provider.*             { yield fallback_value }
catch any as e               { note "{{ e.type }}: {{ e.message }}"; raise e }
```
- Clauses are tried **in order**, and the first match wins.
- A pattern is an exact type, a prefix (`provider.*`), alternatives (`a | b`), or `any`.
- **How a catch body can end:**

  | Ending | Effect |
  |---|---|
  | `goto` | jump to another step or state |
  | `end` | finish the machine |
  | `raise` | rethrow, or throw a new error |
  | `cancel` | cancel the enclosing scope |
  | **fall through** | the step "succeeds" with the catch body's `yield` value (or `null`); its binding becomes `T?`; `status(x)` reports `failed` and `recovered(x)` is `true` |

### `after`: boundary timers
```
after 24h keep_waiting { tool issues.comment(…) }   // non-interrupting: side branch, step continues
after 72h raise approval.timeout                     // interrupting: cancels the step, raises
after 72h { note "giving up"; end abandoned }        // interrupting with a body
```
- An interrupting body that completes without `goto`, `end`, `raise` or `cancel` makes the step **complete with the body's `yield` value** (or `null`), and the binding becomes `T?`. That is how a timeout turns into a default value.
- Timers start when the step starts, and are cancelled when the step finishes.
- `keep_waiting` bodies run concurrently with the step. They are scoped to it, so they're cancelled if the step ends first.
- Each `after` duration may be an expression: `after review_sla(severity) …`.

### `compensate { … }`
- Registers an undo flow once the step succeeds.
- Only meaningful inside a `saga` (§6.16). Outside one it is a compile warning (`compensate-outside-saga`) and has no effect.
- The compensation body may refer to the step's own binding.

### `cache by <expr> [for <duration>]`
- The cache key is `(machine, step path, hash(expr))`.
- **On a hit:** the step's recorded result is reused, without running an effect. The hit is journaled as `cache_hit`, so replay is exact.
- **Storage:** the cache lives in the host's cache store, scoped to the machine across instances.
- Use it for expensive idempotent work, such as re-asking a triage agent about the same issue body.

### `on_error continue | fail`
- `continue` is shorthand for `catch any { }`: the binding becomes `T?` and the flow goes on.
- `fail` overrides a `defaults { on_error continue }`.

## 5.5 The error model

Every failure is an `Error` value (§2.7) with a dotted `type`. Error types form families, and a pattern matches by prefix.

| Family | Types | Raised by |
|---|---|---|
| `timeout` | `timeout` | the `timeout` modifier |
| `cancelled` | `cancelled` | scope cancellation; never retried |
| `expression.*` | `index`, `key`, `convert`, `budget`, `division` | expression evaluation |
| `assertion.failed` | — | `assert` |
| `user.*` | `user.failed` and anything raised with `raise user.…` | `fail`, `raise` |
| *custom* | any dotted name, e.g. `triage.ambiguous` | `raise` |
| `wait.timeout` | — | `wait for … (timeout:)` |
| `send.no_instance` | — | `send` |
| `call.*` | `failed`, `not_found` | `call machine` |
| `flow.depth` | — | recursive flow calls |
| `lock.timeout`, `semaphore.timeout` | — | `with lock` / `with semaphore` |
| `budget.exhausted` | — | `budget` blocks (crossing the soft mark is a hook, not an error) |
| `map.duplicate_key` | — | `map` start |
| `race.all_failed` | — (`data` holds every branch error) | `race` |
| `loop.exhausted` | — | `loop`/`while` with `until` and `exhausted: fail` |
| `saga.compensation_failed` | — (`cause` is the original error) | `saga` unwinding |
| `call.cancelled` | — | `call machine` |
| `limits.*` | `wall_clock`, `steps` | machine `limits` |
| `output.invalid` | — | machine `output` validation |
| `join.*` | `unsatisfiable` | a `fork`/`map` join that can no longer be met |
| `error.action` | — | a failure inside fire-and-forget actions (state layer); delivered as an event, not raised |
| `error.livelock` | — | more than 100 microsteps in a macrostep |
| **agentic** | `agent.*` (`max_turns`, `output_invalid`, `refused`, `aborted`), `provider.*` (`transient`, `rate_limited`, `overloaded`, `context_overflow`, `auth`, `unavailable`), `tool.*` (`transient`, `conflict`, `denied`, `invalid_args`, `not_found`, `failed`, `output_invalid`), `run.*` (`exit_code`, `denied`, `parse`, `output_invalid`), `approval.*` (`rejected`, `timeout`), `ask.timeout`, `best_of.insufficient`, `vote.no_consensus` | ch. 09 |

**Not errors:**
- `hook.failed` is *recorded* in the journal when a lifecycle hook fails, and never raised (§4.8).
- The `timeout` modifier always raises plain `timeout`, whatever the step kind. There is no `run.timeout`; retry on `timeout`.

**Propagation.** An error that is not caught propagates to the enclosing block:
- `fork` and `map` apply their `branch_error` / `item_error` policy (§6).
- Every other block re-raises it to its own parent.
- At the machine root, it fails the instance: `run.status = failed`, `run.outcome = failed`, and the error is recorded.

**The rupu mapping.** The agentic families correspond one-to-one with rupu's `OutcomeClass`. rupu's recovery ladder (retry on the same provider, then a fallback provider, then fail with a hint) runs **inside** one attempt of an agent step. Only what the ladder cannot recover reaches yokoito as an error, and that error carries the ladder's hint in `data`.

## 5.6 Step results

| Step | Result type |
|---|---|
| `let`, assignment | — (no result) |
| `wait for E` | `E`'s payload type |
| `sleep`, `emit`, `send`, `note`, `assert` | `null` |
| `call machine m(…)` | `m`'s output type (`json` for dynamic calls) |
| `call machine … detach` | `ChildRef { id: string }` |
| flow call | the flow's declared return type |
| `agent … -> T` | `T`; `string` without a contract |
| `tool t(…)` | the tool's output schema type; `json` if it has none; `-> T` narrows it |
| `run … -> T` | `T`; `Run` without a contract, with `lines` / `json` set according to `parse` |
| `approve …` | `Approval<F>`, where `F` is the form type (`{}` if there is no form) |
| `ask …` | the form type (`string` for free text) |

Use the metadata functions (`ok`, `status`, `recovered`, `error`, `attempts`, `elapsed`; §3.4) for anything besides the value.

## 5.7 Example: a short flow using every leaf step kind

```
// assumes: input { repo: string, issue: int, issue_ref: string, max_files: int }
flow {
  fetch  = tool issues.get(project: repo, number: issue)
  triage = agent @security-triager -> Triage """
    {{ fetch.title }}
    {{ fetch.body | truncate(8000) }}
  """
    retry 2x on agent.output_invalid, backoff fixed(0s)
    cache by fetch.body for 1d

  assert triage.files.size() <= max_files, "too broad"
  let worst: Severity = triage.files.size() > 3 ? high : medium

  scope = ask "Confirm the files to audit" (form: { files: list<string> = triage.files })
  audit = run `cargo audit --json` -> AuditReport (parse: json, ok_exit: [0, 1])
    timeout 5m
    on_error continue

  note "audit found {{ audit?.vulnerabilities.size() ?? 0 }} advisories"
  ok_to_ship = approve "Proceed with {{ scope.files.size() }} files?" (form: { base: string = "main" })
    after 24h keep_waiting { tool issues.comment(project: repo, number: issue, body: "⏰ waiting on sign-off") }
    after 72h raise approval.timeout

  emit security.fix_started { issue: issue }
  merged = wait for github.pr.merged if event.pr.head == "rupu/sec-{{ issue }}" (timeout: 14d)   // event = the candidate
  send security_issue_owner[issue_ref] repair { reason: "merged" }
}
```
