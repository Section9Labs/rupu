# Session tool approvals v1 — isolated source candidate

Base: `a42e3beaa5736998fabcf9513976545044e326de`. This is a source implementation awaiting compilation, tests and coordinated native/runtime acceptance. It does not change the normal CP startup, existing permission modes or the new-session default.

## Activation and authority

`rupu cp serve --enable-session-tool-approvals` is an explicit, default-off staging option. It requires the existing authenticated CP configuration and a successfully opened private broker. Tokenless CP remains available with its existing behavior; attempting this flag without authentication fails startup. No CP token is sent to the worker or model. Existing `readonly`/`bypass` behavior is unchanged. Ask sessions without the private channel remain unavailable.

Host info contains `session_tool_approvals` with:

- `feature: "session.tool_approvals.v1"`, `enabled`, `authenticated`, `broker_ready`;
- `reason`: `authentication_required`, `disabled`, `broker_unavailable`, `worker_channel_unavailable`, `ready`, or `unsupported_host`;
- `scope: "root_and_in_process_children"` on supported hosts, otherwise `none`;
- `default_mode_changed: false`.

The feature is advertised only when authentication, starter support and broker readiness all hold. These runtime fields prove availability, not that this candidate has passed acceptance. Do not infer readiness from a version string. Ordinary builtin tool grants, ambient grants, runtime permission effects and host MCP effects remain separate. Approval does not add a tool to a grant. Approval endpoints are explicitly excluded from operator MCP discovery, so a model cannot grant itself permission through CP tools.

The supported worker path is the builtin session root and its in-process descendants. Their exact ancestry is registered over the private channel before provider execution. Process agent/workflow child dispatch from an async Ask context is explicitly refused before spawn until those processes have proven broker ownership. It is not silently converted to readonly.

## Private worker path

CP supplies a Unix socket as private child stdin with a hidden handoff flag. The receiving CLI immediately duplicates it with CLOEXEC and replaces stdin with `/dev/null`; only the intended worker gets an explicit handoff. Ordinary subprocesses do not inherit the broker descriptor. No public listener, environment token or URL grants worker authority.

The channel binds to CP instance/epoch, worker PID/generation, exact active builtin session and canonical project path. Before a root turn or in-process child executes, it registers exact run/root/parent ownership. Each request binds turn index, call sequence and tool-call ID as well as tool identity, immutable input, descriptor/grant fingerprints, settings/orchestration revisions and host authority metadata. Reused provider call IDs alone are insufficient identity.

The asynchronous permission boundary runs after ordinary grant resolution and before tool invocation. It emits a typed `tool_approval` event with `data.record`; a normal `tool_call` can already exist. A pause is checked again immediately after receiving a permit and before invoking the tool. Cancellation permanently closes the private channel; dropping a reserved permit leaves an unknown outcome rather than minting a replacement. The runner reports completed approval only after the broker durably records completion. An unconfirmed completion preserves the actual tool result but terminates the run without retrying the effect.

A broken channel is never healed within the affected turn. The worker preserves its turn evidence and retires, allowing a later explicit send to create a new worker. Idle broker EOF is discovered on the next channel operation, so the first later turn can fail safely before retirement; there is no proactive idle reconnection or automatic replay. CP restart expires/cancels undispatched records and marks reserved records unknown; old worker channels cannot reuse an epoch.

## Public authenticated routes

All routes use existing CP authentication/origin checks, only accept the local host, and return `Cache-Control: no-store`. Request body cap is8KiB.

| Method | Route | Request |
| --- | --- | --- |
| GET | `/api/sessions/:id/approvals` | `host=local`, optional `limit`1–50, issued `cursor` up to8KiB |
| GET | `/api/sessions/:id/approvals/:approval` | `host=local` |
| PUT | `/api/sessions/:id/approvals/:approval/decision` | `{decision_id, expected_revision, intent_fingerprint, decision:"allow_once"\|"deny"}` |
| POST | `/api/sessions/:id/approvals/:approval/revoke` | `{operation_id, expected_revision, intent_fingerprint}` |

No allow-always option exists. Decision IDs are durable. Repeating the identical decision returns its exact receipt without a second permission. Changed decisions, stale revisions, foreign owners and consumed attempts fail. Native clients should reconcile an ambiguous write using exact detail GET, not automatically resubmit it. A503 or disconnected write can follow a durable decision; it does not prove no decision occurred.409 means state/authority changed;410 is expiry. Historical GET may persist expiry housekeeping but never dispatches a tool.

Every response envelope includes `schema_version:1`, `cp_instance_id`, `broker_epoch`, `host_id:"local"`, `session_id`, `authenticated:true`, `broker_ready`. Lists add `approvals` and nullable `next_cursor`; details and mutations add `approval`.

`ApprovalView` includes:

- Exact `approval_id`, instance/epoch/worker generation, session/root-turn/run/project identity, optional parent and root run;
- `turn_index`, `call_sequence`, `tool_call_id`, `tool`, `called_as`, `effect`;
- `intent_fingerprint`, `input_fingerprint`, `review_version:1`, `review_fingerprint`, `review_complete:true`;
- creation/expiry/update timestamps, revision and state;
- optional exact decision/revoke receipts, dispatch identity/time and completion metadata;
- advisory `can_decide` and `can_revoke`, recomputed from current ownership;
- `review` only on detail/mutation responses: `{version:1,tool,called_as,effect,arguments}`.

Review inputs are bounded to256KiB/depth32 and known credential-bearing field names are refused rather than redacted into an approvable incomplete review. This is not a promise to recognize secrets embedded inside arbitrary command strings. UI must avoid automatic clipboard/log copies. Tool input remains untrusted content; render it as data. List/event views omit arguments. Completion contains only bounded safe status metadata/result fingerprint, not a tool result dump.

The exact wire source is `crates/rupu-runtime/src/session_approvals/view.rs` and `store.rs`. Native must validate its captured transcript owner against authenticated detail before showing active buttons. A raw custom/model event never mints an actionable request. Descendant navigation separately requires the existing bounded child catalog/prologue proof; approval lineage does not authorize filesystem navigation.

## State, durability and bounds

`pending → allowed|denied`; an allowed decision can be revoked or expire before reservation. `dispatch_reserved` means **execution may have started**, not verified running. Only a confirmed completion becomes `completed`; lost completion or worker loss after reservation becomes `unknown`. A reservation is never issued twice, even across CP restart, changed tool/input or a new epoch for the same exact call.

Private short store leases, bounded no-follow regular-file reads, atomic no-replace publication, fsync and permanent call/approval claims precede permission delivery. Active capacity is4096 records/64MiB, individual record1MiB; waiting capacity is16 per session/256 total. TTL is at most10 minutes. Expired waiting records are reconciled before admission even if channel cleanup previously failed; reserved/unknown records are never expired away to make room.

Known terminal records rotate into immutable bounded pages with permanent consumed-ID tombstones and owner-bound HMAC cursors. Interrupted publication is reconciled without duplicate history, hardlink windows or losing active physical counts. Terminal archive/tombstone total disk retention is uncapped;4096 is an active bound, not a lifetime quota. Unknown records remain protected and may exhaust active capacity until an explicit future reconciliation policy exists. There is no generic acknowledge-to-unlock action.

A reservation job rechecks exact active worker/session ownership, host authority metadata and fresh monotonic-constrained time inside its store job after queueing. External project/config/credential files are not locked transactionally with the store; the implementation does not claim atomic cross-file configuration freeze. Revoke races through the same durable store CAS.

For remote tools, authority metadata freezes the trusted registry plus safe credential-store binding revision/presence before client assembly, checks again afterward and before permission reservation. Rotation/removal with the same binding generation invalidates pending permission. Environment-backed remote Write/External tools cannot obtain Ask permission because they lack a revocable revision; the worker receives a specific unsupported reason and can continue with other tools. Existing modes are unchanged. No credential value enters this metadata fingerprint, approval record, HTTP DTO or event.

## Validation receipt and next acceptance

Prepared source fixtures cover exact one-decision/one-dispatch flow, denial, owner replacement, credential rotation/removal, caller cancellation, immediate-pause/ready-permit race, private frame and identity mismatch, channel failure/worker retirement, completion uncertainty, tokenless/auth/local route gates, sensitive-field review refusal, expiry capacity recovery, filesystem special entries, crash publication/archives, stable cursors, byte pressure and an opt-in lifetime count above4096.

Independent source review is recorded separately. Rustfmt parsing and whitespace/export checks do not establish type correctness or runtime behavior. No build, test execution, service launch, provider call, live API call, actual credential read, native install or GUI action was performed for this candidate.

Before enabling normal Ask defaults, root must compile/run the focused suites and native suite, then accept a disposable authenticated CP with one bounded write denied, one exact write allowed once, pause/revocation, lost decision/receipt reconciliation, credential rotation, worker/CP restart and nested in-process ownership. Verify no duplicate effect and no operator-MCP self-approval. Tokenless/default-off behavior must be accepted separately. Process children remain explicitly unavailable until their ownership path is implemented and accepted.
