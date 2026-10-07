# 01. Rationale and research

Before designing weft we surveyed rupu's three definition languages and four bodies of prior art. This chapter records what we found and what we took from each source, so later readers can see why each design choice was made.

## 1.1 What rupu has today

### Workflows (`rupu-orchestrator`)
- **Size.** About 41k lines. `runner.rs` alone is 20,388 lines, roughly 46% of it tests.
- **Step kinds:** linear agent, `action:` (MCP tool), `run:` (command), `for_each:`, `parallel:`, `panel:` (with a fix-loop gate), `branch:`, `split:`/`join:`, gate nodes (`approval:`), and bounded `loops:`.
- **Two engines.** The `is_nonlinear` heuristic picks between a declaration-order walker and a Kahn ready-set DAG scheduler. They differ in semantics:
  - **Branching:** the walker uses a skip-set and requires *transitive* arm lists; the DAG scheduler prunes by reachability.
  - **Rejection:** the walker fails the whole run; the DAG scheduler scopes it to the gate's path.
- **Limitations found** (complete list in the survey):
  - no error edges, sub-workflows, nested loops or single-node loops
  - no per-step retry or timeout for agent or action steps
  - fan-out units are a single agent
  - `parallel` sub-steps are bare `{id, agent, prompt}`
  - branch targets must be declared forward
  - data edges are inferred from only some fields
  - every output is a string
  - permissive templates render a missing variable as `""`
  - one trigger per workflow
  - `host:` placement only on linear steps
  - a panel gate that never clears still "succeeds"
  - a hard DAG failure drops in-flight siblings without persisting them
  - agent-spawned sub-agents are invisible to the language
- **Consumers.** The serde output of `Workflow` is a public API (CP `GET /api/workflows/:name`). The CP web editor keeps a 1,628-line TypeScript mirror of the schema (`workflowGraph.ts`). `rupu-app-canvas` and the CLI live view each walk `Step` with their own kind-detection logic.

### Autoflows (`rupu-cli` autoflow tick)
- **Definition.** An `autoflow:` block on a workflow covers the entity (issue or PR), selector, priority, `wake_on`, `reconcile_every`, claim (key, ttl), workspace and outcome contract.
- **Hidden state machine.** The 9-state claim machine (`ClaimStatus`: Eligible, Claimed, Running, AwaitHuman, AwaitExternal, RetryBackoff, Blocked, Complete, Released) lives in roughly 300 lines of if/else.
- **Problems:**
  - leases are never renewed
  - there is no retry attempt cap
  - `Blocked` has no automatic exit
  - follow-up wakes always use the issue kind, even for PR claims
  - `autoflow.enabled` doubles as the cron on/off switch, so a cron workflow carrying `autoflow: {enabled: true}` probably matches every issue (found by reading the code, not by testing)

### Agentiflows (`rupu-agentiflow`)
- **Definition:** lead, pool, goals (findings or asset targets, with verification), coverage, budget, scope, round config.
- **Runtime:** `Envelope::run` is a fixed loop: assess, then decide (GoalsMet → CoverageReached → BudgetExhausted → OperatorStop → Ceiling), then a lead round.
- **Gaps:**
  - no resume or pause
  - `trigger` and `scope.mode` are parsed but ignored
  - no final "bank your evidence" turn
  - roles are only an address string

**Conclusion.** All three are state machines. Two of the three hide their machine in Rust, where authors can't see, change or test it.

## 1.2 Statecharts: XState v5 and SCXML

Sources:
- [XState actors](https://stately.ai/docs/actors), [setup](https://stately.ai/docs/setup), [transitions](https://stately.ai/docs/transitions), [persistence](https://stately.ai/docs/persistence)
- [XState v6 durable execution](https://stately.ai/docs/xstate/v6/durable-execution), [W3C SCXML](https://www.w3.org/TR/scxml/)

**Taken:**
- **The definition is data, and behaviour attaches by name from a registry.** No closures appear in a persisted definition.
- **SCXML's execution algorithm, exactly:**
  - run-to-completion macrosteps made of microsteps
  - internal events handled before external ones
  - transition conflict resolution by exit-set intersection, descendant first, then document order
  - exit deepest-first, enter outermost-first
- **Invocations start only at the end of a macrostep.** A state passed through within one macrostep never launches its work. This matters when that work is an expensive LLM call.
- **`done.*` and `error.*` as ordinary events.** Joins and error routing then need no special machinery.
- **Invoke vs spawn.** Invoke is scoped to its state; spawn creates a dynamic collection. weft's *dynamic regions* (ch. 08) generalise spawn within one instance.
- **Three transition flavours:** targetless, targeted, and `reenter`.
- **XState v6's durable contract:** a pure stepper, effects with stable IDs, host-owned timers, single-writer instances, and deduplication of stale completions.

**Avoided:**
- XState's fragile persistence: an invalid snapshot yields an unusable actor, and versioning was bolted on later.
- Positional effect IDs, which silently misalign when a definition changes.
- Hand-writing statecharts for simple pipelines. Their main criticism is verbosity, which weft's flow layer exists to fix.

## 1.3 Agent-graph frameworks

Studied: LangGraph, Apache Burr, pydantic-graph, LlamaIndex Workflows, CrewAI Flows, Mastra, Google ADK 2.0, the OpenAI Agents SDK, and Meta's labgraph.

Sources:
- [LangGraph graph API](https://docs.langchain.com/oss/python/langgraph/graph-api), [checkpointers](https://docs.langchain.com/oss/python/langgraph/checkpointers), [interrupts](https://docs.langchain.com/oss/python/langgraph/interrupts)
- [Burr](https://burr.apache.org/concepts/), [pydantic-graph joins](https://pydantic.dev/docs/ai/graph/builder/joins/), [Mastra](https://mastra.ai/docs/workflows/control-flow)

What the field converged on: typed state with merge rules, checkpoints at step boundaries keyed by instance, interrupt-then-resume for human-in-the-loop, dynamic fan-out with a reducing join, per-node retry/timeout/error handlers, and a step-count recursion limit.

**Pitfalls weft designs out:**
- **Unscoped joins.** LangGraph has three subtly different join behaviours:
  - an AND-join that silently never fires when a branch was skipped
  - a join that fires once per super-step
  - a global `defer` barrier

  In weft, every join belongs to the block that opened it, and skipped branches don't count (pydantic-graph's fork-stack lesson).
- **Re-running the node on resume.** LangGraph and Mastra re-execute the whole node and match interrupts by call order. weft journals effects and resumes them by structural ID.
- **Durability as a performance knob.** weft's default is correct (journal before dispatch), with an opt-in `ephemeral` mode.
- **Parallelism and persistence pulling apart.** pydantic-graph's beta graph isn't persisted at all, and Burr can't combine parallelism with typed state. In weft, map items are journaled units with their own identity.
- **State bloat.** Large values go to a blob store, and the journal holds references.
- **Weak timers and concurrent input.** Most frameworks have no durable timers and no policy for input that arrives mid-run. weft has durable timers and an `inbox` policy per instance (LangGraph's "double-texting").

**labgraph** is an archived (November 2024) real-time sensor pub/sub framework with no control flow or persistence. It is not relevant.

## 1.4 Durable execution and workflow DSLs

Sources:
- [Temporal workflow definition](https://docs.temporal.io/workflow-definition), [continue-as-new](https://docs.temporal.io/workflow-execution/continue-as-new), [worker versioning](https://docs.temporal.io/worker-versioning)
- [Restate services](https://docs.restate.dev/concepts/services)
- [Step Functions variables + JSONata](https://aws.amazon.com/blogs/compute/simplifying-developer-experience-with-variables-and-jsonata-in-aws-step-functions/)
- [Serverless Workflow DSL](https://github.com/serverlessworkflow/specification/blob/main/dsl.md)
- [Camunda inclusive gateways](https://docs.camunda.io/docs/components/modeler/bpmn/inclusive-gateways/), [multi-instance](https://docs.camunda.io/docs/components/modeler/bpmn/multi-instance/)
- [Argo enhanced depends](https://argo-workflows.readthedocs.io/en/latest/enhanced-depends-logic/), [Inngest flow control](https://www.inngest.com/docs/guides/flow-control), [Workflow Patterns](http://www.workflowpatterns.com/patterns/control/)

**Taken:**

| From | Idea | Where in weft |
|---|---|---|
| Temporal | event-sourced history; effects recorded as scheduled then completed; continue-as-new; per-instance version pinning | ch. 08 journal, `restart with`, versioning |
| Restate | keyed single-writer *virtual objects*; awakeables vs named signals | `instance keyed …`, inbox; `wait for` vs `send` |
| Step Functions | ordered `Retry`/`Catch` rules on typed errors; scoped variables; idempotent instance names; callback tokens with heartbeats | modifiers (ch. 05), `var` scoping, instance keys |
| Serverless Workflow | typed errors addressed by type (RFC 7807); lifecycle events; `fork compete` | dotted error types, the observer stream, `losers: cancel` |
| BPMN | interrupting vs non-interrupting boundary events; event-based gateway; multi-instance with completion condition; compensation | `after … keep_waiting`, `race`, `map (until:)`, `saga` |
| Argo | boolean `depends` over predecessor outcomes | `join: when(…)` |
| Inngest | debounce, singleton, concurrency keys, throttling | `instance { debounce, singleton }`, `throttle` |
| Kestra / Windmill | scoped `errors`/`finally`; approvals with N-of-M, forms, no self-approval | `try`/`finally`; `approve (quorum, approvers, form)` |
| Workflow Patterns | the expressiveness yardstick | Appendix B |

**The trade-off of a declarative language.** A declarative language gives up arbitrary code, functions and types. It gains static analysis, visualisation, safe versioning and migration checks, no determinism bugs in user code, and definitions that can be stored and diffed. weft keeps those gains and closes most of the losses:
- typed expressions with a rich standard library
- reusable `flow`s and imports
- in-language types
- `call machine` for independently owned work
- extensions for domain power

## 1.5 The Rust ecosystem

| Concern | Finding | Decision |
|---|---|---|
| Statechart crates | No crate interprets a statechart defined as data, asynchronously, with parallel regions and history. `statig` is macro-defined with no regions. `scxml` (2026) is a document model and validator, not an executor. | Build our own interpreter. Read `scxml`'s transition-resolution and liveness checks. |
| Durable engines | Temporal and Restate need a server (Restate's server is BSL). Obelisk is AGPL. `duroxide` (Microsoft, MIT) is code-first with event-sourced history behind a provider trait. `sayiir` uses checkpoints. | Build our own. Study `duroxide`'s history events and provider trait. |
| Expressions | `cel` (cel-rust) is mature but its type-checking is unconfirmed. `jaq` is the best JSON transformer but termination isn't guaranteed. `jsonata-rs` is alpha. minijinja is weakly typed. | Our own expression syntax and evaluator, with CEL-style semantics (pure, total, typed). No second transform language. |
| Schema / TypeScript | `schemars` 1.x for JSON Schema; `ts-rs` for TypeScript types | The IR is schemars-described; the TypeScript types for the CP come from the IR. The parser itself reaches the web through WASM. |
| Storage | JSONL plus snapshots matches rupu's run directory and its SSH/tunnel/bucket mirroring; SQLite gives indexed queries | A `JournalStore` port, with JSONL first and SQLite as an optional adapter. |

## 1.6 Why a purpose-built language rather than YAML

The same kitchen-sink workflow, counted in non-comment lines:

| Form | Lines |
|---|---|
| legacy YAML | 178 |
| the new semantics expressed in YAML | 189 |
| weft text | 109 |

Line count isn't the main point. YAML forces expressions into strings, gives no visual sense of the flow, and makes nested blocks hard to follow. A purpose-built syntax reads in the shape of the flow.

The cost is a parser, formatter, LSP and highlighter, which is bounded and paid once. It is offset by deleting the TypeScript schema mirror and the YAML-specific validation code.

**Alternatives rejected:**
- **HCL, KDL, Pkl, CUE.** No sequential statements, so pipelines degrade into nested blocks.
- **Starlark or TypeScript code that builds a machine.** One-way: the visual editor can't write changes back, and authors can break determinism.
- **The Dagger lesson.** Dagger dropped CUE because users would not learn a *general* language. weft is narrow and shaped to its domain, closer to SQL or HCL.
