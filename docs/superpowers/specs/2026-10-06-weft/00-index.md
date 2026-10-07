# weft: a state-machine language for complex agentic workflows

**Status:** design spec. Approved section by section on 2026-10-06; awaiting a full read.
**Working name:** "weft". On a loom, the *warp* is the fixed structure and the *weft* is the thread woven through it. Here the machine definition is the warp, and execution forks and rejoins through it like the weft. The name is a placeholder until the repo is created (see [Open questions](#open-questions)).
**Home:** this spec lives in the rupu repo until the library's own repository exists, and moves there when it does.

---

## 1. Why

rupu has three ways to define automated work, and each hard-codes a different slice of the same idea:

| Today | What it really is | Where it hits a wall |
|---|---|---|
| **Workflows** (`.rupu/workflows/*.yaml`) | a DAG of agent/tool/command steps | no nested loops, no error routing, no step retry/timeout, fan-out units are a single agent, two engines (linear vs DAG) with different semantics, outputs are strings, templates fail silently |
| **Autoflows** (`autoflow:` block) | a long-lived, per-issue lifecycle | a 9-state machine hidden in ~300 lines of if/else in the autoflow tick; leases never renew; no attempt cap; `autoflow.enabled` is overloaded |
| **Agentiflows** (`.rupu/agentiflows/*.yaml`) | a goal-directed round loop | a fixed Rust loop with a fixed stop order; no phases, checkpoints or human steps; no resume |

All three are state machines. **weft** is one language, with one engine, that expresses all three, and expresses far more than any of them can today.

## 2. Goals

1. **Expressiveness.** Cover 100% of what rupu's workflows, autoflows and agentiflows can express, and cover the Workflow Patterns catalogue (Appendix B). Today's rupu covers roughly a third of the control-flow patterns.
2. **Readability.** A flow reads top to bottom as a story. A lifecycle reads as a set of states. Loops, fan-out and fan-in are visible in the block that opens them.
3. **Static safety.** Types, agent and tool references, joins, unreachable code and possible deadlocks are all checked before anything runs.
4. **Durability.** Every construct survives a crash and resumes at its finest unit (a map item, a loop pass, an agent turn), with exactly-once state transitions.
5. **One source of truth.** One parser and one analysis core serve the CLI, the VS Code extension and the CP web editor (via WASM). No hand-maintained schema mirrors.
6. **A reusable library.** The core knows nothing about LLMs. Agents, tools, approvals and fleets come from an **agentic extension**, and the host (rupu) implements the effects.

## 3. Non-goals

- No implementation code in this spec. That comes in the plans listed in §6.
- No general-purpose programming language. weft has no user-defined recursion in expressions, no I/O in expressions, and no unbounded loops.
- No compatibility layer for legacy YAML. This is a deliberate clean break (§5, decision 1).
- No macOS app work. The app is deprecated.

## 4. Reading guide

| Chapter | Contents |
|---|---|
| [01 Rationale & research](01-rationale-and-research.md) | what we studied (XState/SCXML, LangGraph and agent frameworks, Temporal/Restate/Step Functions/Serverless Workflow/BPMN, the Rust ecosystem) and what we took from each |
| [02 Lexical structure & types](02-lexical-and-types.md) | tokens, literals, names, the type system |
| [03 Expressions & templates](03-expressions-and-templates.md) | the expression language, standard library, prose templates |
| [04 Program structure](04-program-structure.md) | files, imports, machine anatomy, inputs/outputs, triggers, instances, defaults, hooks, flows vs machines |
| [05 Steps](05-steps.md) | the uniform step shape, every leaf step, modifiers, the error model |
| [06 Blocks](06-blocks.md) | every block construct, each with semantics, example, Mermaid diagram and lowering |
| [07 The state layer](07-state-layer.md) | states, transitions, regions, history, activities, execution semantics |
| [08 Runtime semantics & IR](08-semantics-and-ir.md) | the pure stepper, durable engine, journal, cancellation, versioning, the IR |
| [09 Extensions & the agentic extension](09-extensions-and-agentic.md) | the extension contract and the full agentic catalogue |
| [10 Tooling](10-tooling.md) | CLI, formatter, lint catalogue, LSP, VS Code extension, tests, rendering, WASM |
| [11 rupu integration](11-rupu-integration.md) | how rupu adopts weft; CP editor and graph; migration |
| [A Grammar](A-grammar.md) | the complete EBNF grammar |
| [B Workflow Patterns coverage](B-workflow-patterns.md) | all 43 control-flow patterns, in weft |
| [C Legacy parity](C-legacy-parity.md) | every legacy rupu feature → its weft form |
| [examples/](examples/) | `kitchen_sink.weft`, `security_issue_owner.weft`, `pr_shepherd.weft`, `vuln_hunt.weft`, and the legacy kitchen-sink fixture |

## 5. Decisions log

These were made during the 2026-10-06 design session. Each one is load-bearing.

| # | Decision | Why |
|---|---|---|
| 1 | **A clean break, not a compatibility layer.** Legacy files are migrated with a Claude skill that loops on `weft check`. The legacy kitchen-sink becomes entry #1 of a behavioural-equivalence test corpus. | A permanent legacy compiler costs more than one migration. The corpus replaces the "compiler as coverage proof" argument. |
| 2 | **A clean core plus an optional agentic extension, shipped from the same repo.** | The core forces the right abstractions; the extension stops every host from reinventing "an agent step". |
| 3 | **A purpose-built text language, with a canonical JSON IR.** YAML is not a surface. | YAML hid the flow and forced `${ }`/quoting rules. A purpose-built syntax reads like the flow it describes, in about 40% fewer lines than YAML for the kitchen-sink. |
| 4 | **Two layers in one language.** `flow { }` for pipelines, `state` for lifecycles. They nest both ways. | Pipelines and lifecycles read differently. Forcing either into the other's shape costs readability. |
| 5 | **The core semantics are SCXML's, executed by a pure stepper with a separate durable host.** | Validated by XState v6's durable design, Restate+XState, and `harel`. The pure core makes tests, replay and simulation nearly free. |
| 6 | **One expression language** with readable operators (`and`/`or`/`not`, `x => …`), CEL-style semantics (pure, total, typed), implemented in-house. Prose uses `{{ }}` over the same expressions. | One syntax everywhere. Strict typing ends today's silent empty-string templates. |
| 7 | **In-language types, test blocks and saga/compensate are included.** | Each is cheap given the pure stepper, and each adds real safety. |
| 8 | **Structured concurrency is a language rule.** Nothing outlives the scope that started it, unless explicitly detached. | It eliminates the orphaned-run and "spins forever" bug classes. |
| 9 | **Effect IDs are structural** (path + iteration + item key + attempt), not positional. | Stable across replays and changes; maps directly onto graph nodes. |
| 10 | **Plan order:** syntax + IR + checker, then core stepper, then durable engine, then agentic extension, then tooling (LSP, VS Code, WASM), then rupu integration (6a–6f). | The language is the product; each layer is testable on its own. |

## 6. Deliverables and plan decomposition

This spec is sub-project 1. Each item below gets its own implementation plan (`superpowers:writing-plans`), executed in order:

1. **Syntax, IR and checker.** `weft-syntax` (lexer, lossless CST, parser, formatter), `weft-ir`, `weft-check`, `weft-cli` (`fmt`/`check`/`lower`/`ir`/`render`).
2. **Core stepper.** `weft-core`: SCXML semantics plus dynamic regions; deterministic; property-tested.
3. **Durable engine.** `weft-engine`: journal, snapshots, recovery, timers, instances, locks; JSONL store adapter.
4. **Agentic extension.** `weft-agentic`: manifest, lowering rules, handler traits, `agent`/`tool`/`run`/`approve`/`ask`, `best_of`/`vote`/`pursue`, usage dimensions for core `budget`.
5. **Tooling.** `weft-ide` (shared analysis), `weft-lsp`, `weft-wasm`, `weft-test` (`test` blocks + `simulate`), the VS Code extension (TextMate grammar + LSP client + live graph preview + Test Explorer), the tree-sitter grammar, and the lint catalogue.
6. **rupu integration:**
   - **6a:** `rupu-weft` host (ports over rupu-agent, rupu-mcp, run steps, fleet, claim store, `cp serve`)
   - **6b:** workflow cut-over and deletion of the legacy engine
   - **6c:** autoflows as instance machines
   - **6d:** agentiflows as `pursue` machines
   - **6e:** the CP editor language mode (highlighting + autocomplete + diagnostics), and a new graph engine and UI covering every construct, both authoring and live
   - **6f:** the migration skill, the parity corpus, and docs

## 7. Glossary

| Term | Meaning |
|---|---|
| **machine** | a compiled weft program; its definition is identified by the hash of its IR |
| **instance** | one running (or waiting) execution of a machine, with its own journal |
| **flow** | a top-to-bottom pipeline body, or a reusable named sub-flow (`flow name(...)`) |
| **step** | one statement in a flow; a leaf step performs a single operation |
| **block** | a step containing other steps (`fork`, `map`, `loop`, ...) |
| **state** | a node in the state layer; it can run an activity and react to events |
| **activity** | the work a state performs while active (`flow { }` or `do <step>`) |
| **effect** | a request from the stepper to the host (invoke, timer, emit, send, lock) |
| **macrostep** | the run-to-completion processing of one external input |
| **IR** | the JSON intermediate representation the engine executes |
| **extension** | a package of keywords, blocks, functions, types and lowering rules (e.g. `agentic`) |
| **host** | the application embedding the engine and implementing its ports (rupu) |

## Open questions

1. **Name.** "weft" is a placeholder; check crates.io and GitHub before choosing.
2. **Repository.** Where it lives (e.g. `Section9Labs/<name>`) and its license. MIT/Apache-2.0 dual licensing is suggested, to match the Rust ecosystem.
3. **rupu CLI surface.** Keep `rupu workflow …` as the umbrella for all machines, or introduce `rupu machine …` / `rupu instance …`. To be decided in plan 6a.
4. **Numeric defaults.** Snapshot interval, cancel grace period, the per-map item ceiling and the blob threshold. To be decided in plans 2–3, with benchmarks.
