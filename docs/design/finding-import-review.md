# Immutable operator finding-report import

This additive contract preserves the legacy `POST /api/findings/import` and its CLI/MCP operation. The new routes are operator HTTP only and excluded from MCP route parity; they do not add a model tool/grant. They use existing CP authentication middleware when configured, plus the same local-only/writable deployment gates as existing operator operations. A deliberately tokenless local CP remains supported. Unix writable instances advertise `findings.import_review.v1`.

## Routes

| Method/path | Input | Result |
|---|---|---|
| POST `/api/findings/import/preview?host=local` | `{ "files": ["/absolute/server/report.md"] }` **or** `{ "directory": "/absolute/server/reports" }` | `Preview`: immutable review ID/digest, expiry, captured files, qualified targets, planned summary/severity/evidence changes and captured artifact metadata, ready/skipped/failed counts, explicit multi-project scope. Writes only private review staging, not findings or live artifacts. |
| POST `/api/findings/import/commit?host=local` | `{ "review_id": "…", "review_digest": "…", "operation_id": "client-generated-safe-id" }` | `Operation`: original reviewed public manifest, per-file outcomes and per-ledger results/backups, overall state and durable-receipt qualification. No file paths or replacement targets accepted. |
| GET `/api/findings/import/operations/:id?host=local` | Existing exact operation ID | Original bound receipt, or an explicit unknown observation of an interrupted operation. Polling never executes/retries/rewrites journal history. |

All successful response shapes are typed in `crates/rupu-cp/src/api/findings_import_review/model.rs`, version1, snake_case JSON. `Preview.rows[].target` carries `ws_id`, `target_id`, `finding_id`, canonical workspace/ledger paths, physical identities, registry snapshot and exact ledger SHA. Source original/canonical path, bytes/digest and normalized report digest are separate. No selected native project filter is inferred to constrain this instance-wide request. Each batch report must contain exactly one Finding ID; use existing single-report attach for unlabelled reports.

HTTP200 does not mean every report attached. `Operation.status=completed` means all attempted outcomes are known and may include failures/conflicts/skips. Row states are `attached`, `skipped`, `failed`, `conflict`, `unknown`, `not_attempted`. Preview uses `ready`, `skipped`, `failed`. Interrupted reserved/running journal state is observed as `unknown` without modifying persisted evidence. `receipt_durable=false` retains known outcomes in the response when latest journal publication failed; preserve this response and never downgrade it to a safe-to-retry failure. A typed parent-directory-sync uncertainty keeps its backup path, stops later ledgers and cannot finish/release its review snapshot.

A repeated operation ID with the same binding returns the existing receipt without executing. Another review digest conflicts; a different operation ID cannot reuse an already-reserved review. Known terminal operation records are immutable and retained as replay tombstones. A missing operation is not proof that some other attempt did not run.

## Exact scope, content and write behavior

Discovery reads held no-follow regular files and validates their identities/locators. It rejects explicit symlink components, hardlinks, physical workspace/ledger aliases, malformed registry/ledger rows and unavailable inventory. Missing ledgers are empty targets; commit performs a complete discovery again before effects. It preserves workspace/target IDs and raw ledger fields; one visible target in an incomplete inventory is never authorized as globally unique.

Preparation retains captured Markdown bytes and normalized report content. Artifacts/evidence-block inputs are captured from bounded handles into private storage; v1 refuses external-artifact mode. Commit reads verified captured blobs only, never the original Markdown, directory or workspace artifact paths. Claims remain unhashed: captured input identity does not validate historical claims against current code. Cross-reference normalization and effective findings policy are frozen into the manifest digest. Policy changes refuse the attempt before reservation; target changes report per-ledger conflicts.

Every mutation is preceded by durable operation reservation and an unknown per-ledger boundary. The prepared writer checks the exact ledger digest and physical identity under the existing cooperative findings lock, retains full-report no-overwrite behavior and unknown fields/line endings, and makes backup/temp/replacement relative to a held parent. A replaced/renamed pathname cannot redirect that held-parent write to another directory. Writers ignoring the lock are outside the exclusion guarantee; no universal filesystem transaction is claimed.

**Effects are per ledger, not per project or batch.** One project may have several coverage target ledgers. Installed content-addressed blobs can survive a later ledger refusal; receipts retain that possibility. A successful replacement requires directory synchronization; post-rename sync failure is `Unconfirmed`, not success or `NotWritten`, and its backup is preserved.

## Bounds and lifecycle

- 100 report files; 4MiB/report and16MiB aggregate; explicit files or one directory.
- Directory traversal:5000 entries,128 depth; paths4096 bytes. Broad/over-budget discovery is refused, never silently truncated into a ready set.
- Complete inventory:1000 workspace records,32KiB/registry record and16MiB aggregate;5000 cumulative target entries;16MiB/ledger,64MiB aggregate,100000 finding records.
- Dependencies: at most100 per report and1000 per batch,64MiB aggregate or tighter configured limits. Remaining aggregate byte/file budgets are passed into preparation before it reads/copies. Failed preparations remove partial private captures.
- Public preview/operation response budget2MiB; reservation retains768KiB response headroom for all bounded per-file diagnostics and backup paths; private manifest24MiB. A too-large review is rejected before commit. There is no implicit cursor or hidden eligible subset.
- Review lifetime15minutes; private directories0700/files0600; maximum8 stages, each bounded to112MiB/4096 entries/depth16. Expired unreferenced/explicitly known-terminal staging may be removed by a later preview; original inputs/live artifacts are never deleted. Unknown operations pin staging. Public review metadata remains in terminal operation receipts after stage expiry.
- At most1024 operation receipts. They are not automatically erased; capacity exhaustion refuses new attempts. Operator cleanup/archive beyond this bounded first version is not implemented.

## Native integration boundary

Do not start the batch native write journey until this contract is compiled/tested and advertised by the selected CP. Preserve the single import disclosure. New entry belongs in Findings actions, uses server paths (no Mac upload), visibly groups targets across projects and confirms the exact fixed set. Persist the captured client/review/operation intent before sending. Refresh Status must issue GET only. Existing project filters and current connection changes cannot silently alter an attempt.

## Validation status

Source fixtures cover complete/partial discovery, bounds/symlinks/hardlinks/aliases, immutable source disappearance and new directory files, mixed-project conflicts, no overwrite/raw preservation, private artifact tampering/capture cleanup, same-size ledger drift/inode replacement, parent substitution, journal publication faults, restart/idempotency/tombstones/expiry/capacity and post-rename sync uncertainty. These are source fixtures, not executed acceptance. No real reports or services were used during implementation.
