# Durable session orchestration

A session can opt into the reusable Agentiflow lead coordination services without adopting a flow run or changing permission mode. Configuration lives in `sessions/<id>/orchestration.json`; it does not extend the strict settings payload or rewrite the agent definition.

`GET /api/sessions/:id/orchestration?host=local` reports configuration and the lead-tool inventory. `PUT` takes exactly `{"revision":0,"enabled":true}`. Both fields are required. Successful updates increment revision; stale edits return 409. A remote host returns 501 and must be addressed directly. Read-only control planes reject writes, unknown fields return 422, missing sessions return 404, and archived sessions return 409. Updates apply on the next turn. The response includes `session_id`, `host_id`, `context_id` (`session:<id>`), `enabled`, `revision`, `applies`, `execution`, `tools`, `permission_mode` and `limits`.

The tool rows distinguish explicit `declared` grants, configured `granted` and `grant_reasons`, `service_available`, `permission_allowed`, `available`, `effect` and `reason`. This is a next-turn configuration inspection, not a provider credential check or permission to dispatch any arbitrary agent. Dispatch still validates target allowlists, depth, inputs and current catalog definitions. Spawn permissions inherit the session ceiling; recontinued work receives no more permission than either its original or current parent.

| Lead tools | Session service |
| --- | --- |
| `dispatch`, `join` | Shared RunLauncher scheduler, child request/result ledger, checkpoint and continuation |
| `workflows.generate` | Shared workflow generator; files remain dispatchable by the owning session on later turns |
| `board.claim`, `board.release` | Shared board; session claims retain their original TTL and can be released after reopening |
| `board.post`, `board.read`, `board.directive`, `board.retract` | Durable session message space and standing directives |
| `msg.send` | Shared mailbox, participant registry and per-turn message collectors |
| `agents.list`, `agents.get`, `workflows.list`, `workflows.get`, `catalog.search` | Shared global/project catalog |
| `findings.report` | Existing session-scoped findings service |
| `goal.status`, `goal.coverage`, `budget.status` | Unavailable: ordinary sessions have no supervising Agentiflow envelope |

With orchestration disabled, the existing session tool grant and turn-local launch behavior remain in effect. Enabling explicitly adds the meaningful lead coordination grant. An agent's `messaging:false` still removes the message-bus tools; no permission mode is widened.

Each enabled turn is recorded in a session-owned orchestration journal before continuation begins. A worker lease prevents overlapping owners. Actual child ledgers stay under their real turn run IDs, preserving provenance and usage accounting. At turn end, unfinished or queued children checkpoint as interrupted through the normal launcher. A later enabled turn reopens their records, re-vets work that would restart, inherits prior/current permission bounds and continues with stable child handles. Completed results are restored without rerunning their tasks. The shared ledger's claim/continuation protocol prevents double dispatch during recovery. Disable retains records and suspends continuation; re-enable recovers them. Generated workflow authorization is limited to this session's journaled turns and canonical generated directories.

Scheduling is **turn-bound**. Nothing autonomously advances between turns; there is no wall-clock/cron scheduler. Child process recovery follows the shared launcher's recorded continuation rules, not an exactly-once guarantee for external side effects. Board claims are leases, not permanent locks: their original TTL still expires. Goal evaluation, flow-wide budget enforcement and flow aggregate coverage require an explicit Agentiflow run and are not fabricated here. Workflow generation still needs a supported configured provider.

Configured requests use queue protocol version 4 so older workers refuse to execute them silently. Install the matching backend when integrating the native client. No production CLI installation is part of this change.

Integration order: **#937 → #938 → #939**, then the native consumer. This branch is stacked against `codex/session-command-parity` at `d6e33ad2`, which includes the runtime settings foundation. Both command and orchestration API routes are registered. Cooperative cancellation uses the current run token, checkpoints unfinished children, and keeps the worker available for the next turn; that next enabled turn resumes durable child records. Cancellation requests for an old run cannot cancel a later turn. Cancellation is observed by the runner; admission and orchestration recovery complete before the runner consumes that signal. Configured orchestration retains queue protocol version 4, while settings-only requests retain version 3.

Validation uses disposable homes, scripted child processes, mock session providers, HTTP handler tests and filesystem reopen tests. No production provider, installed CLI or live session is needed.

Validated after rebasing onto command parity with offline builds and PDF features disabled: CLI/control-plane compilation; all 82 launcher integration tests (including seven session durability tests); 38 session API tests through the combined router and three command API tests; 14 runtime session tests; three worker cancellation tests; two orchestration-filtered CLI tests (including enable/disable/re-enable and cancellation); four command adapter tests; and two settings/compaction regression tests. Cancellation and orchestration filters overlap by one test. The existing 67 fleet tests and scoped Clippy passed before the rebase. Scoped Clippy used `double_must_use` and `result_large_err` forced to warnings; the earlier strict run stopped in unchanged dependency crates on the known `async_trait`/`double_must_use` baseline. The CLI test linker still emits its large-unwind-table warning. Full workspace validation and live-provider acceptance are not claimed.
