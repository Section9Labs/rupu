# Operator Board controls — source qualification

This change supplies an explicit operator domain seam and local HTTP routes. It does not add a worker identity, start a generation, schedule a job, advertise an MCP capability, or enable native controls before their end-to-end integration is validated.

## Wire contract

`GET /api/{runs,sessions,agentiflows}/:id/board/operator?host=local` returns:

```json
{"owner":{"kind":"session","id":"ses_…","host":"local","token":"<opaque 64-character precondition>"},"actor":"operator","live":true,"writable":true,"notice":null,"operator_leases":[],"live_directives":[]}
```

Owner kinds are `session`, `workflow`, `standalone`, and `agentiflow`. Capture the full server/host/owner identity. The token binds the canonical active owner directory's device/inode plus recorded identity, creation/start time, workflow name, and recorded project fields; it is a precondition, not a credential. Normal record lifecycle updates do not replace it. On non-Unix platforms the directory creation metadata replaces device/inode. Standalone runs without a completed run record require their own live roster member and live PID. Existing runtime liveness semantics apply: active non-stopped sessions may accept operator input while idle; workflows must be running/pending and Agentiflows running, with their recorded runner alive when present.

`POST` to the same path accepts an exact owner token and one nested operation:

```json
{"owner_token":"<captured token>","operation":{"action":"claim","work_unit":"audit:parser","ttl_seconds":300}}
```

| Action | Operation fields | Successful receipt |
| --- | --- | --- |
| `claim` | `work_unit`, `ttl_seconds` | `{status:"granted",lease:{work_unit,owner,token,acquired_at,lease_expires_at}}` or `{status:"denied",holder}` |
| `release` | `work_unit`, `claim_token` | `{released:true,work_unit,claim_token}` |
| `post` | `kind`, `body`, optional `addressed_to`, optional `expected_run_id` | Canonical `LogMessage` |
| `directive` | `body`, optional `addressed_to`, optional `expected_run_id` | Canonical `LogMessage` |
| `retract` | `expected`: full captured `Directive` (`id`, `author`, `ts`, `body`, optional `addressed_to`) | Canonical retraction `LogMessage` |

Every success envelope contains `{owner,actor:"operator",via:"cp",action,receipt}`. The server supplies attribution; unknown operation fields, including caller-supplied author/actor, are refused. `post.kind` is the existing observation/question/answer/vote/note enum. An omitted or broadcast recipient is an unaddressed Board entry. Specific current-run targeting uses the exact named roster address or concrete registered `lead` slot plus `expected_run_id`; selector targets do not accept that guard. A lead target must match the exact current live roster run; no legacy-mailbox or alias fallback is used. Parent/operator recipients are refused. Addressed posts retain normal inbox delivery semantics. Directives retain normal standing-Board semantics; a receipt does not claim model consumption.

Claim/release state persists in the existing claim file/journal with owner `operator`; these actions do not invent a transcript message. Posts/directives/retractions use the canonical message log with `from:operator`, `via:cp`, no worker codename, and no operator roster membership. A trusted MCP request extension can supply `client:mcp`; this patch exposes no MCP tool.

## Guards and limits

- Existing CP authentication, cookie same-origin checks, and server writable adapter gate remain in force. Remote hosts return 501. No root is inferred from a worker, parent, or project directory.
- Only the exact active local owning collection is writable. Owner records must match their directory id; archived roots, replaced owners, unreadable lifecycle records, symlinked owner directories, and symlinks/special files in Board/mailbox stores are refused. Store path inspection is bounded at 20,000 entries and depth 3, excluding transcript/artifact trees. Owner records are bounded at 1 MiB. GET also returns the exact live directive snapshots required for retraction, including session-owned Boards; both GET and the locked retraction precondition read the directive journal through one file descriptor with a 32 MiB +1-byte bound, rejecting oversize data before parsing or log append rather than reopening after a metadata check.
- Owner and liveness checks run again under the message log lock before state application. This is a locally owned store protocol, not a hostile-filesystem transaction or a cross-lifecycle lock: an independently stopping runner may stop immediately after a valid check.
- Operator claims use a dedicated persisted acquisition result, never a leaked or retained worker RAII guard. TTL is 30–3600 seconds; at most 128 live operator leases. Keys contain 1–512 bytes, no controls or outer whitespace. Receipt recovery reads at most 10,000 claim directory entries and 64 KiB per claim record; excess or corrupt stores fail explicitly. There is no automatic renewal.
- Release compares exact operator owner and acquisition token under the same per-key lock used by expiry/regrant. Missing, expired, worker-owned, and replaced claims are never released.
- Retraction compares the entire captured live directive under the canonical Bus log lock before appending its retraction. The original directive author is preserved. Operator may retract an exact captured directive by another author; the retraction actor remains operator. Concurrent guarded operator retractions admit one write. Existing worker retraction semantics remain unchanged. Legacy direct Board-file writers do not participate in the Bus lock.
- Body size is 1–65,536 bytes (nonblank); the existing message-log cap still applies. Successful receipts do not imply the worker read the message, acted on it, or completed a job.

## Failure and UI behavior

POST application failures contain `code`, `mutation_outcome`, and optional `logged_message_id`. Preconditions normally return 400/404/409/501 with `operator_board_refused` and `not_applied`. Unknown outcomes return 500 with `operator_board_write_uncertain` and `reconcile`. A partly applied canonical message supplies its recorded ID; no automatic mutation retry is safe. JSON extraction/authentication errors retain their existing framework shape.

A native compact Board action menu should inspect first, capture the exact owner, and enable writes only when both `writable` and `live` are true. Claim shows the persisted operator lease and expiration; Release uses its exact token. Retract asks about the exact displayed directive and author. On an uncertain write, preserve the draft and inspect the operator leases plus live directives and existing message reads before offering another explicit action. Changing selected transcript must not retarget an in-flight operation.

## Historical validation status

The original a2f64b47 implementation on base 4a340cde passed warm validation: fleet 78 tests; CP operator Board 9, Board filter 21 (including the same 9 operator tests), and messages 10; MCP route registry 5. These are disposable-store fixture results, not actual-store mutation or native acceptance. No provider launch or service restart occurred. The six dynamic operator Board GET/POST routes are explicitly inventoried and excluded from MCP parity because this slice exposes only HTTP; no MCP tool count or capability is added.

Focused commands (run sequentially in the assigned warm checkout):

```sh
cargo test -p rupu-fleet --lib -j 2
cargo test -p rupu-cp --no-default-features --test it operator_board:: -j 2
cargo test -p rupu-cp --no-default-features --test it board:: -j 2
cargo test -p rupu-cp --no-default-features --test it messages:: -j 2
```

New fleet tests cover operator attribution/worker guard preservation, exact concurrent retraction, owner/recipient preconditions, durable claim receipt recovery, stale/worker token refusal, expiration/regrant, TTL/count/record bounds, and partial apply without replay. New CP fixtures cover all four owner families, no implicit launch, canonical receipts, readonly/remote/terminal/replaced/root identity refusal, existing auth, actor injection, owner/store symlinks, exact directive comparison, and structured partial-write reconciliation. They use disposable temporary stores only. The growth regression captures an earlier owner token and directive, grows the journal past 32 MiB, and requires both GET and retraction to refuse with byte-for-byte unchanged message log.


## ec963 integration qualification

This recovered lane is rebased source-only onto `ec9633ad4c68943bb3da95ccc5b32ee73b6d8f6c`. The historical validation above is not validation of this new base. Existing operator attribution, exact active-owner token, liveness rechecks, bounded directive/lease readers, receipt fields and uncertain-outcome handling are retained. Six HTTP-only route exclusions and dynamic inventory entries are merged additively with newer registry entries; the existing MCP tool names/count fixture is unchanged. No Board feature or model tool is advertised.

The one compatibility correction permits guarded `Address::Lead` alongside `Address::Named` in the Board wrapper, matching the current shared Bus's exact registered-address/run guard. Native28 still excludes the literal lead address in its common recipient admission helper; a separate native companion is required before the UI can select this exact registered lead. It does not permit selectors, operator/parent impersonation or retry. New disposable fixtures cover registered lead post/directive receipts and missing, stale or departed lead refusal without another log append.

Current native `OperatorBoard` request/response models match these routes: owner family/token, operator leases, whole directive expectations, body/TTL bounds, nested operation payload, canonical log receipts and `logged_message_id` on partial writes. Native's 16 MiB GET bound is deliberately stricter than the backend's 32 MiB directive-file bound; it fails explicitly without using a partial snapshot. Older/untyped pre-dispatch HTTP refusals may remain conservatively uncertain in native; no automatic replay is enabled.

Required validation before integration/release (not run in this source lane):

```sh
cargo test -p rupu-fleet --no-default-features --lib -- --test-threads=2
cargo test -p rupu-cp --no-default-features --test it operator_board:: -- --test-threads=2
cargo test -p rupu-cp --no-default-features --test it board:: -- --test-threads=2
cargo test -p rupu-cp --no-default-features --test it messages:: -- --test-threads=2
cargo test -p rupu-cp --no-default-features --test it mcp_routes:: -- --test-threads=2
```

Run these sequentially only in the assigned validation lane. Include current guarded messaging tests through the fleet/messages suites and preserve pinned MCP inventory. After source and fixture validation, separately qualify native GET-only owner/readonly/terminal/absent presentations; any mutation acceptance must use explicit disposable canonical owners and exact captured receipts, without provider launches or existing user-store changes.
