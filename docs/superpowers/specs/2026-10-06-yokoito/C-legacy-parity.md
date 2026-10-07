# Appendix C. Legacy parity: every rupu feature and its yokoito form

This table drives the migration skill (§11.9) and the parity corpus. The worked end-to-end example is `examples/legacy/kitchen-sink.yaml` (which parses under rupu 0.82) against `examples/kitchen_sink.yoko`.

## C.1 Workflows

| Legacy (YAML) | yokoito | Behaviour change |
|---|---|---|
| `name:` / `description:` | `machine <name>` + `///` doc comment | |
| `trigger: {on: manual}` | `trigger manual` | Multiple triggers are now allowed. |
| `trigger: {on: cron, cron: "…"}` | `trigger cron "…"` | Optional `tz`. |
| `trigger: {on: event, event: e, filter: "{{ … }}"}` | `trigger on e if <expr>` (+ `with { … }`) | The filter is a typed `bool` expression; there is no truthiness. |
| `inputs: {x: {type, required, default, enum}}` | `input { x: Type [= default] }`; enum → `enum` type | Inputs keep their types inside expressions (they used to be strings). |
| `defaults.continue_on_error` | `defaults { on_error continue\|fail }` | |
| `defaults.findings_profile` / step `findings_profile` | `defaults { findings full }` / agent option `findings:` | |
| `defaults.workspace` | `defaults { place host(…, workspace: sync) }` or per step | |
| `contracts.outputs.<n>: {from_step, format, schema}` | `output <expr> : Type`; step `-> Type` | Validated by the engine on every run, not only by autoflow. |
| step `contract: {emits, format}` | `-> Type` on the step | Enforced, not just metadata. |
| `notifyIssue: true` | `on start { tool issues.comment(…) }` + `finally { … }` | Explicit and editable. |
| `max_concurrency:` | `limits { concurrency N }` | Applies to every machine, not only DAG mode. |
| linear step `agent:` + `prompt:` | `x = agent @a "…"` | |
| `actions: [..]` (narrowing) | agent option `tools: [..]` | Also allowed on remote steps. The tool roster now travels with the placement. |
| `action: t` + `with: {…}` | `x = tool t(…)` | Typed arguments and output; no `fromjson`; no quoting rules. |
| `run: {cmd, args, cwd, env, parse, timeout_seconds, allow_exit_codes}` | ``run `cmd args` (parse:, cwd:, env:, ok_exit:)`` + `timeout` modifier | |
| `when: "{{ … }}"` | `if <expr> { step }` | The binding becomes `T?`. A `bool` is required. |
| `continue_on_error: true` | `on_error continue` | The binding becomes `T?`. |
| `branch: {condition, then: [..], else: [..]}` | `if … { … } else { … }` (arms are nested blocks) | The transitive-arm footgun disappears; there is no forward-only rule. |
| `next:` / `depends_on:` | statement order; `fork`; `async`/`await` for arbitrary DAGs | Data edges are no longer inferred from templates; dependencies are explicit. |
| `split: [a, b, c]` | `fork { a { … } b { … } c { … } }` | |
| `join: {wait: all \| any \| {count: N}}` | `fork (join: all \| any \| N, losers: …)` | Adds `losers: wait\|detach`, `when(…)` joins and guarded branches. |
| implicit wait-all reconvergence | the step after a `fork` | |
| `for_each:` + `max_parallel:` + `item`/`loop.*` | `map item in <expr> (concurrency: N) { … }`; `index`, `count` | Units may be multi-step. A stable `key` replaces index identity. Adds `until`. |
| `for_each` + `run:` | `map x in xs { run `…` }` | |
| `distribute: {hosts: [..]}` + `workspace:` | `place distribute([..], workspace: sync)` on the map | Also valid on `pipeline`/`worklist`/`best_of`. |
| `parallel: [{id, agent, prompt}]` | `fork { id { agent @a "…" } … }` | Sub-steps gain every step feature. |
| `panel: {panelists, subject, prompt, max_parallel}` | `map p in panelists (concurrency:) { agent p -> Findings "…" }` | Typed findings; no "first parseable object" heuristic. |
| `panel.gate: {until_no_findings_at_severity_or_above, fix_with, max_iterations}` | `loop (max:) { panel; if blocking { fix } }` or `panel.review(…)` after `import agentic/panel as panel` (a library flow) | An unresolved review is visible in the output (`resolved: false`). With `exhausted: fail` it can now fail the run. |
| `loops: {name: {nodes, until, max_iterations, on_max}}` | `name = loop (max:, until:, exhausted: fail\|continue) { … }` | Nestable. `prev.x` replaces the implicit feedback edge. `loops.<n>.iteration` becomes `loop.iteration`. |
| gate node `approval: {prompt, timeout_seconds, on_timeout, auto_approve, notify, on_reject}` | `approve "…" (skip_if:, notify:)` + `after <d> raise approval.timeout` + `catch approval.rejected \| approval.timeout { … }` | Timeouts no longer depend on the `cp serve` sweep running; adds `quorum`, `approvers` and `form`. |
| `on_timeout: approve` | `after <d> { yield null }` on the `approve` step: the interrupting body completes the step, so its binding is `Approval?` (null meaning timed out and treated as approved) | Downstream code reads `sign_off?.form.base ?? "main"`. |
| inline `approval:` on a step | an `approve` step before it | |
| `steps.<id>.decision` | the `approve` binding (`Approval<F>`) | |
| `host:` + `workspace:` | `place host(h, workspace: sync)` | Valid on any agent, tool or run step. |
| `steps.<id>.output` | the binding itself (typed) | |
| `steps.<id>.success` / `.skipped` / `.error` | `ok(x)` / `skipped(x)` / `error(x)` | |
| `steps.<id>.results` / `.sub_results.<s>` | `map` output list / `fork` output record `x.s` | |
| `steps.<id>.findings` / `.max_severity` / `.iterations` / `.resolved` | fields of the review flow's typed output | |
| `steps.<id>.json` / `.stdout` / `.stderr` / `.exit_code` | `run … -> T` value / `Run` fields | |
| `{{ x \| fromjson }}` | not needed (values are typed) | |
| `read_file(path)` template function | `tool fs.read(path: …)` (host tool) | Reading a file is an effect, so it's journaled. |
| permissive templates (missing → `""`) | strict checking | A missing name is a compile error. |
| `--mode ask\|bypass\|readonly` | host `Policy` + agent option `mode:` | |
| run resume (`rupu workflow resume`) | automatic journal recovery; `rupu workflow resume` kept as an operator command | Every construct resumes at its finest unit. |
| `--restart-interrupted` | `rupu workflow resume --restart-effects` | |

## C.2 Autoflows

| Legacy | yokoito |
|---|---|
| `autoflow.enabled: true` on a workflow | a separate machine with `instance per <entity> keyed entity.ref { … }` |
| `autoflow.entity: issue \| pull_request`, `claim.key: pr_head_sha` | `instance per issue` / `per pull_request` / `per pr_head` |
| `autoflow.source` | `select { source "…" }` |
| `autoflow.priority` | `priority N` |
| `autoflow.selector.*` | `select { … }` (same field names, §11.4) |
| `wake_on: [events]` | `on <event> -> working` transitions in a waiting state |
| `reconcile_every: 30m` | `after 30m -> working` |
| `claim.ttl` | `lease 4h, renew while active` (now actually renewed) |
| `workspace: {strategy, branch}` | `workspace worktree, branch "…"` |
| `outcome.output` + `autoflow_outcome_v1` | `do call machine work(…)` + `on done if output.status == "…" -> state` (`Outcome` type in `lib/autoflow.yoko`) |
| `ClaimStatus` Claimed / Running | `working` state (activity running) |
| AwaitHuman | implicit: the child machine is waiting at its `approve`. The owner stays `working`. |
| AwaitExternal | an `awaiting_external` state with `on` transitions and an `after` give-up |
| RetryBackoff + `next_retry_at` | a `backoff` state with `after retry_after -> working` plus an attempt cap |
| Blocked | a `blocked` state with `on operator.repair` and a reminder `after` |
| Complete / Released | `final state complete` / `final state released` |
| `pending_dispatch` (runs next tick) | a `dispatching(d)` state: `do call machine (d.workflow)(d.inputs)` |
| contenders / yielding | `priority` + `yield unless in [states]` |
| `[autoflow].max_active` | host config: the per-repo cap on entity instances, in `InstanceRegistry` |
| `[autoflow].cleanup_after` | `retain 7d` |
| `rupu autoflow claims / release / requeue` | `rupu workflow instances` / `send <id> operator.release` / `send <id> operator.repair` |

## C.3 Agentiflows

| Legacy (`AgentiflowDef`) | yokoito |
|---|---|
| `name`, `description` | `machine` + `///` |
| `lead` | the agent in `pursue`'s `round { agent @lead … }` |
| `engagement_profiles` | `engagement_profiles [..]` (agentic top-level) |
| `goals[]` (`id`, `objective`, `target`, `required`, `verify_with`) | `goals { goal id { objective …; findings …; verified …; verify_with @v; required … } }` |
| `coverage: {reach, depth, kinds}` | `pursue goals [.., coverage(reach: …)]` |
| `budget: {usd, tokens, wall_clock, rounds, soft_at}` | `pursue (budget: { usd, tokens, time }, max_rounds:)` + `on soft_budget` |
| `scope: {authorized, mode, roots}` | `scope { authorized true; roots [..] }`; mode → agent `mode:` |
| `pool: {agents, workflows}` | `pool { agents [..]; workflows all\|[..] }` |
| `round: {lead_max_turns, ceiling}` | agent option `max_turns`; `max_rounds`; `limits { wall_clock }` |
| `trigger` (ignored today) | `trigger …` (works) |
| stop order GoalsMet → Coverage → Budget → OperatorStop → Ceiling | `pursue`'s documented order (§9.4); fully custom via raw states |
| `steering/*.json` files | `rupu workflow send <id> operator.steer --payload '{"body": "…"}'` → `steering` var |
| no resume | journal recovery |

## C.4 Worked example

See [`examples/legacy/kitchen-sink.yaml`](examples/legacy/kitchen-sink.yaml) next to [`examples/kitchen_sink.yoko`](examples/kitchen_sink.yoko). The yokoito file ends with the `test` blocks that pin the legacy behaviour (parity corpus entry #1).
