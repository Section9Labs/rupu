import Foundation
import Observation
import RupuAPI

/// Owns the Fleet screen's (Phase 5A, Task 6) two independent read blocks —
/// registered hosts (`GET /api/hosts`) and local execution identities
/// (`GET /api/workers`) — plus the one write path this phase's Fleet screen
/// offers: removing a host (`DELETE /api/hosts/:id`).
///
/// **Merge of two independent blocks**: `hosts`/`workers` are fetched
/// concurrently by `activate()`/the reconcile loop below, and — same
/// per-block-independence contract every other multi-block detail store in
/// this module (`ProjectDetailStore`'s tabs) already follows — one block
/// failing never blanks the other. Concretely: `workers` (a fast, local
/// call — see the audit note below) is never blocked by `hosts` (which live-
/// probes every registered remote host and can be slow with one offline —
/// `list_hosts`'s own doc comment, `crates/rupu-cp/src/api/hosts.rs`), since
/// `reconcile(includeWorkers:)` below fires both via `async let`, not a
/// sequential await.
///
/// **Audited, not converted, for local-first (perf & interaction arc, Plan
/// 5 Task 2)**: `workers` was on that task's list of fetches suspected of
/// blocking first paint on a fleet-wide fan-out. Verified against
/// `list_workers` (`crates/rupu-cp/src/api/workers.rs`): no `Query`
/// extractor at all, reads a local `WorkerStore` off `s.global_dir` — a
/// worker record only ever lives on the host that registered it (see
/// `CPClient.workers()`'s own doc comment), with no host-fan-out mechanism
/// server-side to converge multiple hosts' workers into one list. There is
/// no per-host progressive merge to build for `workers` here; the ACTUAL
/// fan-out cost in this screen is `hosts`'s own live probe, which is
/// already fired concurrently with (never blocking) `workers` — see the
/// paragraph above. Converting `workers` itself would mean fabricating a
/// `host` param the server ignores, which the task brief explicitly
/// forbids. Left unchanged.
///
/// **60s reconcile loop**: `activate()` performs an immediate load (so the
/// screen never sits on a blank grid for up to 60s on first appearance —
/// unlike `HostsFooterStore`'s own fire-and-forget first poll, this store is
/// built fresh per screen appearance via `FleetScreen`'s lazy-build/
/// `storeClientID` convention rather than shared app-wide, so blocking the
/// caller on the first load is the right tradeoff here) and then starts the
/// recurring loop idempotently (`task == nil` guard, weak-self, cancelled
/// and nilled out on `deactivate()`) — the same idiom `HostsFooterStore.
/// activate(client:)`/`.deactivate()` already established, just with the
/// immediate load hoisted out of the loop body and into `activate()` itself.
///
/// **`removeHost(id:)` is confirm-first, not optimistic** (CLAUDE.md rule
/// 9): the `DELETE` succeeding only proves the mutation was *recorded* — a
/// pending `.remove` key stays pending until a subsequent `/api/hosts`
/// batch (this method's own follow-up reconcile, or the next 60s tick)
/// actually stops listing the host. `applyHosts(_:)` performs that
/// confirmation — see its own doc comment for the "row disappearing IS the
/// confirmation" contract. A `DELETE` that itself errors (network failure,
/// or the server's 400 for `id == "local"`) fails the key immediately
/// instead — that failure is known the moment the request returns, no
/// refetch needed to learn it.
///
/// **Generation guard (review fix)**: `reconcile(includeWorkers:)` can run
/// concurrently more than once — the periodic 60s tick, `removeHost(id:)`'s
/// own post-success refresh, and a second `activate()` (screen
/// reappearance) can all be in flight at once, same "more than one cycle
/// alive at a time" scenario `DashboardStore.refetchAll()`/`performFetch`
/// already guard against. Without a guard, a STALE cycle's slow `GET
/// /api/hosts` (this screen's own core scenario: a probe hanging on a
/// faulted host) can resolve AFTER a newer, faster cycle already applied —
/// e.g. `removeHost`'s own confirming reconcile — and silently resurrect a
/// just-removed host with no pending indicator at all, directly
/// contradicting the "row disappearing IS the confirmation" contract above.
/// `generation` is bumped once per `reconcile(includeWorkers:)` call
/// (mirroring `DashboardStore.refetchAll()`'s `generation += 1`); each
/// fetch's result is applied only if `self.generation` still matches the
/// generation captured when that fetch started — a stale cycle's result is
/// silently dropped rather than applied, same drop-not-clobber behavior
/// `DashboardStore.performFetch` already establishes.
@MainActor
@Observable
public final class FleetStore {
    public private(set) var hosts: BlockState<[APIHostRow]> = .loading
    public private(set) var workers: BlockState<[APIWorkerRow]> = .loading

    /// Shared with the rest of the app the same way `RunDetailStore`/
    /// `SessionDetailStore` share `BackendController.pendingActions` —
    /// `FleetScreen` passes that instance explicitly; the default here only
    /// exists so every test in this file (none of which cares about
    /// cross-screen sharing) can build a store without threading one
    /// through.
    public let pendingActions: PendingActions

    private let fetchHosts: @Sendable () async throws -> [APIHostRow]
    private let fetchWorkers: @Sendable () async throws -> [APIWorkerRow]
    private let postRemoveHost: @Sendable (String) async throws -> Void

    private var task: Task<Void, Never>?

    /// Bumped once per `reconcile(includeWorkers:)` call — see the type doc
    /// comment's "Generation guard" section. Never read/written outside
    /// `reconcile(includeWorkers:)`/`loadHosts(generation:)`/
    /// `loadWorkers(generation:)`.
    private var generation = 0

    /// Production entry point — `FleetScreen` calls this.
    public convenience init(client: CPClient, pendingActions: PendingActions) {
        self.init(
            fetchHosts: { try await client.hosts() },
            fetchWorkers: { try await client.workers() },
            postRemoveHost: { id in try await client.removeHost(id: id) },
            pendingActions: pendingActions
        )
    }

    /// Designated init — plain fetch/mutate closures, the same "fake client
    /// closures" seam every other store in this module already established.
    /// `internal`, not `public` — reached from tests via `@testable import
    /// RupuStore`.
    init(
        fetchHosts: @escaping @Sendable () async throws -> [APIHostRow],
        fetchWorkers: @escaping @Sendable () async throws -> [APIWorkerRow],
        postRemoveHost: @escaping @Sendable (String) async throws -> Void,
        pendingActions: PendingActions = PendingActions()
    ) {
        self.fetchHosts = fetchHosts
        self.fetchWorkers = fetchWorkers
        self.postRemoveHost = postRemoveHost
        self.pendingActions = pendingActions
    }

    /// Immediate load of both blocks, then starts the 60s reconcile loop
    /// (idempotent — a second `activate()` call, e.g. the screen
    /// reappearing, never spawns a second loop). Repeatable, like every
    /// other store's `activate()` in this codebase.
    public func activate() async {
        await reconcile()
        startReconcileLoop()
    }

    /// Cancels the reconcile loop. Safe to call whether or not one is
    /// running (idempotent, mirrors `HostsFooterStore.deactivate()`).
    public func deactivate() {
        task?.cancel()
        task = nil
    }

    private func startReconcileLoop() {
        guard task == nil else { return }
        task = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(60))
                guard let self else { return }
                await self.reconcile()
            }
        }
    }

    /// Bumps `generation` (see the type doc comment's "Generation guard"
    /// section) and fans the fetch(es) out concurrently — `hosts`/`workers`
    /// are independent blocks, so there is no reason for one to wait on the
    /// other when both are requested. `includeWorkers: false` — used by
    /// `removeHost(id:)`'s post-success refresh (review fix: narrows what
    /// that call actually needs, shrinking the race surface rather than
    /// refetching a block a host removal has no bearing on) — fetches only
    /// `hosts`.
    private func reconcile(includeWorkers: Bool = true) async {
        generation += 1
        let currentGeneration = generation
        if includeWorkers {
            async let hostsLoad: Void = loadHosts(generation: currentGeneration)
            async let workersLoad: Void = loadWorkers(generation: currentGeneration)
            _ = await (hostsLoad, workersLoad)
        } else {
            await loadHosts(generation: currentGeneration)
        }
    }

    /// `generation` is the value captured by the `reconcile(includeWorkers:)`
    /// call that started this fetch — applied only if `self.generation`
    /// still matches once the fetch resolves; a newer, since-started cycle
    /// (`self.generation` having moved on) means this result is stale and
    /// must be dropped, never applied over whatever the newer cycle already
    /// established. See the type doc comment's "Generation guard" section.
    private func loadHosts(generation: Int) async {
        do {
            let rows = try await fetchHosts()
            guard generation == self.generation else { return }
            applyHosts(rows)
        } catch {
            guard !isCancellation(error) else { return }
            guard generation == self.generation else { return }
            hosts = .failed(String(describing: error))
        }
    }

    /// Same generation-guard contract as `loadHosts(generation:)` above.
    private func loadWorkers(generation: Int) async {
        do {
            let rows = try await fetchWorkers()
            guard generation == self.generation else { return }
            workers = rows.isEmpty ? .empty : .content(rows)
        } catch {
            guard !isCancellation(error) else { return }
            guard generation == self.generation else { return }
            workers = .failed(String(describing: error))
        }
    }

    /// Applies a fresh `/api/hosts` batch: updates `hosts`, and — the "row
    /// disappearing IS the confirmation" contract `removeHost(id:)` relies
    /// on — confirms any currently-`.pending` `.remove` key whose target
    /// host id is no longer present. Diffs against whatever `hosts` held
    /// *before* this call (`.loading`/`.failed` contribute no ids, same as
    /// an empty batch would), so the very first call this store ever makes
    /// can never spuriously confirm anything — there is no `previousIDs` set
    /// yet for a host to have disappeared from.
    ///
    /// Public and side-effecting (touches `pendingActions`, so not purely
    /// functional) but still the deliberately testable seam — mirrors
    /// `HostsFooterStore.apply`'s own "pure mapping, timing loop untested"
    /// convention, minus the purity (this one has exactly one necessary
    /// side effect: the confirmation).
    public func applyHosts(_ rows: [APIHostRow]) {
        let previousIDs = Set(hosts.value?.map(\.id) ?? [])
        let currentIDs = Set(rows.map(\.id))
        for goneID in previousIDs.subtracting(currentIDs) {
            let key = ActionKey(goneID, .remove)
            if case .pending = pendingActions.state(key) {
                pendingActions.confirm(key)
            }
        }
        hosts = rows.isEmpty ? .empty : .content(rows)
    }

    /// Confirm-first host removal — see the type doc comment's "removeHost"
    /// section for the full contract. `begin()`s the key, fires the
    /// `DELETE`; on success, triggers one immediate hosts-only reconcile
    /// (`includeWorkers: false` — a host removal has no bearing on
    /// `workers`, so refetching it here would just be an unnecessary extra
    /// request widening the race surface for no reason) so the confirmation
    /// (via `applyHosts(_:)`) lands as soon as realistically possible rather
    /// than waiting for the next 60s tick — but the key stays `.pending`
    /// until that reconcile actually stops listing `id`, same as every
    /// other pending-state mutation in this codebase that can't confirm off
    /// the write response alone. This reconcile also bumps `generation`
    /// (see the type doc comment's "Generation guard" section), so it wins
    /// against — and correctly invalidates — any older, still-in-flight
    /// cycle's stale result. A `DELETE` failure fails the key with the
    /// server's message (`mutationErrorMessage`, same helper every other
    /// mutation in this module uses) and leaves `hosts` untouched — nothing
    /// was actually removed.
    public func removeHost(id: String) async {
        let key = ActionKey(id, .remove)
        pendingActions.begin(key)
        do {
            try await postRemoveHost(id)
            await reconcile(includeWorkers: false)
        } catch {
            pendingActions.fail(key, mutationErrorMessage(error))
        }
    }
}
