import Foundation
import Observation
import RupuAPI

/// Owns the Activity screen's autoflows-kind "Claims" sub-tab (Phase 6B,
/// Task 3): the tracked-claim list (`GET /api/autoflows/claims`) plus its two
/// write routes, release and requeue.
///
/// **`issue_ref` is the ForEach identity, unverified-composite** — verified
/// by reading `AutoflowClaimStore` (`crates/rupu-workspace/src/
/// autoflow_claim_store.rs`): `save(rec)` always persists into `root.join(
/// sanitize_component(rec.issue_ref))`, i.e. a claim's own directory is
/// always derived from its own `issue_ref`, so two `save()` calls for the
/// same `issue_ref` land in the same directory (last write wins — never two
/// entries) and `list()` walks one `claim.toml` per directory entry. A
/// single `list()` result can therefore never contain two records reporting
/// the same `issue_ref` — the filesystem itself is the uniqueness
/// enforcement, not an application-level check. (Two DIFFERENT `issue_ref`
/// strings colliding after `sanitize_component`'s escaping is a real but
/// separate hazard — data loss for whichever claim loses the race to
/// `save()` last, not a duplicate-`issue_ref` row — outside this store's
/// remit to fix.) No composite key needed for either `ClaimsTable`'s
/// `ForEach` or `ActionKey` below.
///
/// **`client` is captured directly at `init`, no fake-closure seam** —
/// unlike `FleetStore`/`LibraryStore`'s dual convenience/designated-init
/// split, this store's tests drive it through a real `CPClient` pointed at a
/// stub `URLProtocol` (`ClaimsStubURLProtocol` in `ClaimsStoreTests.swift`,
/// the same generation-token-isolated rig `ConfigStoreTests`/
/// `DashboardStoreTests` already establish) — there is no per-call branching
/// this store needs a closure seam for, and testing through the real client
/// also exercises `CPClient.autoflowClaims`/`releaseClaim`/`requeueClaim`'s
/// actual request shape, not just this store's own call sequencing.
///
/// **Release/requeue confirm off the RESPONSE + a refreshed list — not the
/// run-mutation pending-state contract** (CLAUDE.md rule 9 / every other
/// mutation in this module): CLAUDE.md's "mutations are pending-state, not
/// optimistic" rule exists because `cp serve` runs are detached
/// subprocesses — a run mutation's 200 means *recorded*, and real
/// confirmation only ever arrives later, off the run's own observed status
/// transition via the live feed (`PendingActions.resolve(runID:
/// observedStatus:)`). Claims have no such asynchronous execution to wait
/// on: `POST /api/autoflows/claims/release` synchronously deletes the
/// on-disk claim file before it returns, and `POST /api/autoflows/claims/
/// requeue` synchronously writes the manual-wake file — by the time either
/// response lands, the mutation is already fully applied, not merely
/// recorded. So both `release(issueRef:)` and `requeue(issueRef:)` follow a
/// simpler shape: `begin()` the key, fire the POST, and on success `confirm()`
/// it directly (same "confirm off the response, no refetch needed to prove
/// it" idiom `ActionVerb.setEnabled` already uses for exactly this reason) —
/// then reload the list so the operator sees the claim's new state (or its
/// absence, for release) without a manual refresh. The reload is for
/// **display freshness**, not confirmation — unlike `FleetStore.removeHost`'s
/// "row disappearing IS the confirmation" contract, where the pending key
/// stays pending until a follow-up fetch proves the effect landed.
///
/// **`release` is idempotent — confirms on ANY successful response, `released:
/// true` or `false` alike.** `CPClient.releaseClaim(issueRef:)` returns that
/// bool; this store deliberately ignores it for the confirm decision (a 200
/// is a 200, per the server's own idempotent contract — see `ActionVerb.
/// release`'s doc comment) but the reload still runs, so a `released: false`
/// (already-untracked issue) simply re-loads a list that was already missing
/// that row.
///
/// **A failed `requeue` never touches `claims`** — the catch branch calls
/// `pendingActions.fail(key, ...)` and returns without reloading, so the
/// operator's already-visible rows are never blanked or replaced by a
/// mutation that didn't succeed.
@MainActor
@Observable
public final class ClaimsStore {
    public private(set) var claims: BlockState<[APIClaimRow]> = .loading

    /// Shared with the rest of the app the same way `FleetStore`/
    /// `LibraryStore` share `BackendController.pendingActions` — the default
    /// here only exists so tests can build a store without threading one
    /// through.
    public let pendingActions: PendingActions

    private let client: CPClient

    public init(client: CPClient, pendingActions: PendingActions = PendingActions()) {
        self.client = client
        self.pendingActions = pendingActions
    }

    /// Reloads the full claim list from scratch — repeatable, same "reload
    /// from scratch" contract every other store's load path follows.
    public func load() async {
        do {
            let rows = try await client.autoflowClaims()
            claims = rows.isEmpty ? .empty : .content(rows)
        } catch {
            guard !isCancellation(error) else { return }
            claims = .failed(String(describing: error))
        }
    }

    /// Confirm-first from the call site (`ClaimsTable`'s `confirmationDialog`
    /// — release is destructive-ish, it forgets the claim entirely), but NOT
    /// pending-state-mutation-confirmed like a run action — see the type doc
    /// comment's "Release/requeue confirm off the RESPONSE" section for why
    /// this confirms immediately on any successful response rather than
    /// waiting on a later refetch to prove it.
    public func release(issueRef: String) async {
        let key = ActionKey(issueRef, .release)
        pendingActions.begin(key)
        do {
            _ = try await client.releaseClaim(issueRef: issueRef)
            await load()
            pendingActions.confirm(key)
        } catch {
            pendingActions.fail(key, mutationErrorMessage(error))
        }
    }

    /// No confirm-first UI (`ClaimsTable` fires this directly, no
    /// `confirmationDialog`) — requeuing only enqueues a wake, it doesn't
    /// discard anything. Same confirm-off-the-response shape as `release`
    /// above; see the type doc comment for the full rationale.
    public func requeue(issueRef: String) async {
        let key = ActionKey(issueRef, .requeue)
        pendingActions.begin(key)
        do {
            try await client.requeueClaim(issueRef: issueRef)
            await load()
            pendingActions.confirm(key)
        } catch {
            pendingActions.fail(key, mutationErrorMessage(error))
        }
    }
}
