# Session command API contract

Base: reviewed foundation commit 59619be2 (PR #937), branch codex/session-command-parity. Local owning-host connection only; host defaults to local. A non-local host returns 501, never silently operates locally.

GET /api/sessions/:id/controls?host=local
200 {session_id,host_id:"local",active_run_id:string|null,commands:[{name,available,reason?:string,mutating,usage,description}]}
Names (without slash): cancel, compact, coverage, detach, help, history, issues, quit, runs, session, status, stop, transcript, workflow. Capability reason is human-readable. Refresh after run/state changes. Old deployments may return 404/501: do not forward slash text as a prompt.

POST /api/sessions/:id/commands?host=local
JSON {command:string, expected_run_id?:string}. Unknown fields rejected. command is the full slash string (max 8192 bytes); routed args preserve CLI quoting, read-only subcommands and CLI validation. /cancel REQUIRES expected_run_id from current session detail/controls, rejects stale identity with 409. Other commands reject a supplied stale expected_run_id too.
200 {session_id,host_id:"local",command:string,output:string,stderr:string,exit_code:number, truncated:boolean, data?:any, action?:string}
/cancel returns action:"cancel_requested", data:{run_id}, meaning accepted cooperative cancellation, not completion. Poll session detail until active_run_id clears; don't retry against a new run. Running tools finish at a safe boundary; worker remains usable. Old workers without cancellation support advertise an unavailable reason.
/compact returns 202 with the same envelope, action:"compact_started", data containing existing compact endpoint response (compaction_id, etc). Poll existing session detail for completion. /stop and /quit invoke existing stop behavior; /quit additionally action:"detach". /detach changes no server state, returns action:"detach" for the client to leave the session view.
/help, /status, /history, /runs, /coverage return meaningful text plus structured data where useful. /transcript shows current/last transcript. Routed /workflow show|list|show-run|runs, /issues show|list, /session show|list and /transcript show|list preserve supported CLI arguments. current/last aliases resolve in the selected session. Output is plain text, bounded to 256 KiB per stream; commands have a 30s deadline. Nonzero CLI exit is represented in exit_code/stderr. CLI parser failures return 400; missing session 404; stale/busy state 409; unsupported/read-only deployment 501. No shell evaluation, arbitrary executable, prompt forwarding, provider launch or mutation through routed commands.

Existing /stop, /compact and /settings endpoints remain unchanged. Mutating commands enforce the existing CP writable policy. Coverage without declared concerns/ledger advertises an unavailable reason. No remote transport support is claimed. A 504 deadline response is an ambiguous mutation outcome: reconcile session state before retrying. Unknown JSON body fields return 422.

## Registry and execution notes

`/workflow` and `/issues` without a subcommand are invalid (400), matching the session parser; clients may insert `list`. `/h` and `/?` alias help; `/exit` aliases quit. `/transcript show|list` is accepted even though the completion registry advertises the plain `/transcript` action.

Routed commands use the real Clap grammar, including supported output formats, filters, view density, archive flags, SCM accounts, and quoted arguments. They run the same binary from the session workspace with the owning `RUPU_HOME`, stdin closed, colors disabled, and pagers disabled. CLI session show/list stale-record reconciliation and workflow-runs gate expiry are suppressed in this inspection mode; explicit lifecycle mutation remains behind the writable guard. The process is killed if the 30-second deadline expires. Captured stdout and stderr each retain at most 256 KiB while continuing to drain pipes; oversized structured data is omitted and `truncated` is true.

`/history` and `/runs` retain the CLI's five-most-recent-turn window. Coverage reads the selected session's target ledger, returns audit data and gap summaries, and does not invoke a provider. Compact/Stop keep their established endpoints and semantics. A worker compaction pseudo-turn can be cancelled cooperatively; an offline compact operation remains guarded by its existing compaction marker.

Cancellation is a run-addressed signal file consumed by a monitor owned by that turn. The owner marker includes process identity/start stamp. The requester validates the active run and owner, never signals a PID, never rewrites session.json, and cannot affect a later run. The runner's existing cooperative pause mechanism writes an aborted transcript, retains conversation state and returns the worker to idle. Running tools finish before cancellation completes. The cancellation receipt acknowledges a request only; the UI must poll for completion. Existing workers must be restarted after their current turn to gain this capability.

## Integration order

Merge runtime foundation PR #937 (`codex/session-runtime-controls`, including `59619be2`) first, then this stacked command PR, then native client integration. No deployment or live session migration is part of this change. Tests use temporary homes, fixture executables, and the scripted mock provider.

## Validation

`cargo check -p rupu-cli --no-default-features --offline` passed. Twenty-seven targeted tests passed:

- 9 shared-parser, adapter and HTTP-route tests, including CLI argument validation, literal quoted arguments, selected workspace/home, bounded output, coverage, wrong-host rejection, strict bodies, stale run identity and writable policy.
- 2 worker-path mock-provider tests: cancellation leaves the worker usable for a successful next turn; cancelling worker compaction preserves conversation history.
- 1 runtime signal test proving run-specific cancellation without rewriting the session record or cancelling a subsequent run.
- 14 existing slash-completion regressions.
- 1 real CLI integration test proving inspection mode leaves a stale disposable record byte-for-byte unchanged while ordinary CLI inspection still reconciles it.

Tests used disposable homes and mock providers. No full workspace, release/PDF build, remote transport acceptance or production-session test is claimed. macOS emitted a debug linker unwind-table size warning; compilation and the tests succeeded.
