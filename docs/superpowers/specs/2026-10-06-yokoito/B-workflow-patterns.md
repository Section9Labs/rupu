# Appendix B. Workflow Patterns coverage

This appendix uses the 43 control-flow patterns of the Workflow Patterns initiative (Russell, ter Hofstede, van der Aalst, Mulyar, 2006; <http://www.workflowpatterns.com/patterns/control/>) as the expressiveness yardstick. For each pattern it gives:
- the yokoito construct, with a minimal snippet;
- whether **legacy rupu** supports it (✓ full · ◐ partial · ✗ none).

**Summary:**

| | Full | Partial | None |
|---|---|---|---|
| legacy rupu workflows | 12 | 7 | 24 |
| yokoito | **43** | 0 | 0 |

Plan 2 turns each row into a golden test (`tests/patterns/wcp-NN.yoko`).

## Basic control flow

| # | Pattern | yokoito | Legacy |
|---|---|---|---|
| 1 | Sequence | statements in order | ✓ |
| 2 | Parallel Split | `fork { a { … } b { … } }` | ✓ |
| 3 | Synchronization | `fork (join: all)` | ✓ |
| 4 | Exclusive Choice | `if … else if … else` · `match` | ✓ |
| 5 | Simple Merge | the steps after an `if`/`match` | ✓ |

## Advanced branching and synchronization

| # | Pattern | yokoito | Legacy |
|---|---|---|---|
| 6 | Multi-Choice | guarded fork branches: `fork { a if x { … } b if y { … } }` | ◐ (`when:` on split successors) |
| 7 | Structured Synchronizing Merge | guarded branches + `join: all` (skipped branches don't count) | ◐ |
| 8 | Multi-Merge | the downstream runs once per arriving branch: `fork { a { x; tail(x) } b { y; tail(y) } }` with `flow tail(…)` | ✗ |
| 9 | Structured Discriminator | `fork (join: any, losers: wait)` | ◐ (`any` always cancels) |
| 28 | Blocking Discriminator | `fork (join: any, losers: wait)` inside a `loop`: the next pass can't start until the losers finish | ✗ |
| 29 | Cancelling Discriminator | `fork (join: any, losers: cancel)` · `race` | ✓ |
| 30 | Structured Partial Join | `fork (join: 2, losers: wait)` | ◐ (count always cancels) |
| 31 | Blocking Partial Join | `fork (join: 2, losers: wait)` inside a `loop` | ✗ |
| 32 | Cancelling Partial Join | `fork (join: 2, losers: cancel)` | ✓ |
| 33 | Generalized AND-Join | handles across passes: `h = async step; pending = pending + [h]` … `await all pending` | ✗ |
| 37 | Local Synchronizing Merge | guarded branches + `join: all` (only activated branches are awaited) | ◐ |
| 38 | General Synchronizing Merge | a dynamic handle set: start work conditionally in loops, then `await all pending` (waits for exactly what was started) | ✗ |
| 41 | Thread Merge | `map _ in range(n) { … }`, then continue once (n threads merge) | ✗ |
| 42 | Thread Split | `map i in range(n) (concurrency: n) { … }` | ✗ |

## Multiple instances

| # | Pattern | yokoito | Legacy |
|---|---|---|---|
| 12 | MI without Synchronization | `for x in xs { h = async agent @a "…{{ x }}"; detach h }` | ✗ |
| 13 | MI with a priori design-time knowledge | `map r in [@a, @b, @c] { … }` · `fork` | ✓ (`parallel:`) |
| 14 | MI with a priori run-time knowledge | `map x in computed_list (concurrency: n) { … }` | ✓ (`for_each`) |
| 15 | MI without a priori run-time knowledge | `worklist x in seeds (max_items: …) { …; push more }` | ✗ |
| 34 | Static Partial Join for MI | `map … (join: 3, losers: wait)` | ✗ |
| 35 | Cancelling Partial Join for MI | `map … (join: 3, losers: cancel)` | ✗ |
| 36 | Dynamic Partial Join for MI | `worklist … (until: results.count(r => r.ok) >= k)` · `map … (until: …)` | ✗ |

## State-based

| # | Pattern | yokoito | Legacy |
|---|---|---|---|
| 16 | Deferred Choice | `race { a { wait for x } b { wait for y } c { sleep 1h; yield null } }` · a state with several `on` transitions | ✗ |
| 17 | Interleaved Parallel Routing | `fork (concurrency: 1) { … }`: any order, never concurrently; sequences inside branches give the partial order | ✗ |
| 18 | Milestone | guard on another region: `if in_state("ci.green") { … }` · state layer `on x if in_state(…)` | ✗ |
| 39 | Critical Section | `with lock "name" { … }` | ✗ |
| 40 | Interleaved Routing | `fork (concurrency: 1)` | ✗ |

## Cancellation and force completion

| # | Pattern | yokoito | Legacy |
|---|---|---|---|
| 19 | Cancel Task | `during { step } on operator.skip { cancel }` · `race` with an event branch · the `timeout` modifier | ✗ |
| 20 | Cancel Case | instance cancel (operator) · `end cancelled` | ✓ |
| 25 | Cancel Region | `during { region of steps } on e { cancel }` · `within d { … }` | ✗ |
| 26 | Cancel MI Activity | `during { map … } on e { cancel }` | ✗ |
| 27 | Complete MI Activity | `map … (until: cond)` · `break` inside `map`/`worklist` (keeps the finished results) | ✗ |

## Iteration

| # | Pattern | yokoito | Legacy |
|---|---|---|---|
| 10 | Arbitrary Cycles | `goto` (backward jumps) · state-layer transitions | ✗ (acyclic DAG) |
| 21 | Structured Loop | `loop (max, until)` · `while` · `for` | ✓ (`loops:`, not nestable) |
| 22 | Recursion | `flow f(…) (max_depth: n) { … f(…) … }` · `call machine` | ✗ |

## Termination

| # | Pattern | yokoito | Legacy |
|---|---|---|---|
| 11 | Implicit Termination | the flow ends when nothing is left to run | ✓ |
| 43 | Explicit Termination | `end [outcome]` cancels everything and completes | ◐ (only via gate rejection) |

## Triggers

| # | Pattern | yokoito | Legacy |
|---|---|---|---|
| 23 | Transient Trigger | `wait for e` with `inbox drop` (only events while waiting count) | ✗ (events only start new runs) |
| 24 | Persistent Trigger | `wait for e` with `inbox queue` (earlier unconsumed events count) | ◐ (autoflow wake queue) |

---

## Beyond the catalogue

yokoito also covers the following, which the control-flow catalogue does not address:

| Area | Constructs |
|---|---|
| Exception patterns (Russell et al., 2006) | typed errors, ordered `retry`/`catch`, `try`/`finally`, `saga` compensation, interrupting and non-interrupting boundary timers (`after`) |
| Cost and time | `budget` (core: any usage dimension plus wall-clock), `within`, `limits` |
| Data patterns | typed scoped variables, block outputs, `yield`, a concurrent-write prohibition |
| Resource patterns | `with semaphore`, `throttle`, `place`, `approve (quorum, approvers)`, `ask (from)` |
| Agentic | `best_of`, `vote`, `pursue` |
