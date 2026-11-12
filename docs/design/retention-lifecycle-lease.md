# Retention lifecycle lease

Isolated source at base `365bb3b09a8e9478d8daeb4c545c6a2dd91c1967`. No build or test execution performed.

`rupu_runtime::retention_lease::RetentionLease::try_acquire(global)` acquires a nonblocking exclusive advisory flock at `<canonical-global>/retention.lifecycle.lock`. It opens the canonical root component by component with directory/no-follow flags, then creates/opens a no-follow/nonblocking empty regular single-link lock file. Root and lock entry identities are checked after acquisition. A configured root alias is resolved once deliberately. An absent root is not created. Busy returns `WouldBlock`; no caller proceeds unlocked after an acquisition failure. The lock descriptor is close-on-exec and held until the lease drops.

The stable global lock sits outside sessions, transcripts, runs and their archives. Existing `SessionLock` protects compaction marker/state within a moving session directory and reacquires internally in `live`; it cannot safely be reused as an outer archive/retention lock. The new lifecycle lease is always outermost. It does not replace existing compaction or settings locks.

Integrated entry points:

- CLI session archive/restore/delete: lease before session resolution, held through artifact moves/deletion and record publication.
- CLI standalone transcript archive/delete: lease before transcript resolution, held through their existing ownership/liveness checks and mutations.
- Runtime archived-session prune and orchestrator archived-transcript prune: lease for destructive calls only, before source selection. Existing dry-run calls take no lease and retain their report shape.
- Cleanup invokes those two stage entry points sequentially without an outer lease; there is no recursive lock acquisition. Its existing stage-partial semantics remain unchanged.

Lower-level `delete_session_owned_artifacts`, `remove_stream_only_run_dirs`, file helpers and metadata helpers remain lock-free. The new policy executor must take one lease around its own bounded selection/execution and must not call legacy prune entry points while holding it. It must never hold an operation lock while waiting for this lease: acquisition is immediate success or error.

On non-Unix targets `try_acquire` returns Unsupported. `for_legacy` preserves existing behavior there, returning no lease; the new destructive capability must be disabled. Unix legacy operations use the same fail-closed lease. Thus legacy operations can now fail explicitly when another lifecycle operation owns the lease or the root/lock cannot be trusted; normal successful request/response/report semantics are unchanged.

Coverage limits: cooperative updated Rupu actors only. It does not coordinate arbitrary same-user filesystem writes, older binaries, direct calls to lower-level artifact helpers, RunStore workflow archive/restore/delete, active transcript writers, netflow writers, or miscellaneous settings changes. The new policy executor must still reject active/ambiguous ownership, validate immutable roots and artifacts, and preserve per-artifact partial/unknown receipts. In particular this lease does not authorize deleting active stream-only directories or netflow ledgers. Session compaction is active-only; its marker checks remain in session archive/delete as before.

Prepared fixtures: exclusive/released lease; symlink, hard-link, directory, nonempty lock and FIFO refusal; independent roots/no missing-root creation; separate-process exclusion/release; legacy session dry-run availability while mutation is blocked; legacy standalone transcript preview remains available while deletion is blocked, followed by deletion only in a disposable fixture after release. No production cleanup, provider call or live API request occurred.
