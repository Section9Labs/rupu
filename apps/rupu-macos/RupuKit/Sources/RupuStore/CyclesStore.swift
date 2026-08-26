import Foundation
import Observation
import RupuAPI

/// Owns the Activity screen's autoflows-kind "Cycles" sub-tab (perf &
/// interaction arc, Plan 5 Task 4b): `GET /api/runs/autoflows` (autoflow
/// worker cycles — one row per batch tick — distinct from `ActivityStore`'s
/// `.autoflow` source, which is `GET /api/runs/autoflows/events`, one
/// launched-run-or-signal event).
///
/// **Its own store, not folded into `ActivityStore`** — same precedent
/// `ClaimsStore` already set for the sibling "Claims" sub-tab: cycles are
/// not one of the four kinds `ActivityStore.rows` federates
/// (agent/workflow/autoflow-event/session), they're a fifth, narrower data
/// source that only ever shows on ONE sub-tab of ONE kind page, so giving
/// `ActivityStore` a fifth `Source` entry it fetches for every OTHER kind
/// too would be pure waste.
///
/// **Local-first + generation-guarded progressive remote loading, unlike
/// `ClaimsStore`'s plain single `load()`** — this endpoint IS host-aware
/// (`crates/rupu-cp/src/api/run_streams.rs`'s top-of-file fan-out note
/// covers `/api/runs/autoflows` explicitly, unlike `/api/autoflows/claims`,
/// which stays local-only), so this store follows `ActivityStore`'s own
/// "local truth first, remote hosts progressively, never block on one" idiom
/// instead: `activate()` returns once the LOCAL page loads (`rows`/`state`
/// already showing local truth), then discovers the fleet and merges each
/// online remote host's page in as it answers, one host's failure never
/// affecting another's contribution or this store's own `state`.
/// `remoteGeneration` is bumped on every `activate()`/`deactivate()` so a
/// straggling remote fetch from a superseded activation can never mutate
/// `rows`/`pendingHosts` out from under a newer cycle — same "fold the
/// generation into every async continuation's guard" contract
/// `ActivityStore.remoteGeneration` documents for itself.
///
/// **No live-tail stream** — same choice `ClaimsStore` already made for its
/// own sub-tab: a cycle is a finished, already-persisted history record
/// (`AutoflowCycleRecord` is only ever written once a cycle completes — see
/// `APIAutoflowCycleRow`'s doc comment), so there is nothing "live" to patch
/// in-place the way a workflow run's still-running status is.
@MainActor
@Observable
public final class CyclesStore {
    public private(set) var rows: [APIAutoflowCycleRow] = []
    public private(set) var state: BlockState<Void> = .loading

    /// Count of online remote hosts whose cycle fetch hasn't answered yet —
    /// same "+N hosts loading…" signal `ActivityStore.pendingHosts`
    /// documents; never part of `state`, which reflects the local load only.
    public private(set) var pendingHosts: Int = 0

    private let client: CPClient
    private let local: PagedSnapshot<APIAutoflowCycleRow>
    private var remoteRows: [APIAutoflowCycleRow] = []
    private var remoteGeneration = 0
    private var remoteHostTasks: [Task<Void, Never>] = []

    private static let localHost = "local"
    private static let remotePageSize = 50

    public init(client: CPClient) {
        self.client = client
        self.local = PagedSnapshot { offset, limit in
            try await client.autoflowCycles(offset: offset, limit: limit, host: Self.localHost)
        }
    }

    /// (Re)activates: refreshes local page 0 (returns once that lands —
    /// `rows`/`state` are already local truth by then), then kicks off
    /// progressive per-host discovery/fetch in the background. Repeatable —
    /// same "activate/deactivate is fully symmetric" contract `ActivityStore.
    /// activate(kind:)` documents — a second call (e.g. re-selecting the
    /// Cycles sub-tab after visiting another one) cancels any still-pending
    /// remote work from the prior cycle first.
    public func activate() async {
        remoteGeneration += 1
        let generation = remoteGeneration
        cancelRemoteHostTasks()
        remoteRows.removeAll()
        pendingHosts = 0

        await refresh()
        loadRemoteHosts(generation: generation)
    }

    /// Cancels any in-flight remote-host fetches and resets `pendingHosts` —
    /// idempotent, safe to call more than once or before `activate()` ever
    /// ran.
    public func deactivate() {
        remoteGeneration += 1
        cancelRemoteHostTasks()
        pendingHosts = 0
    }

    /// Refreshes local page 0 and recomputes `rows`/`state` — shared by
    /// `activate()` and any manual "Retry" affordance.
    @discardableResult
    public func refresh() async -> Bool {
        let performed = await local.refresh()
        recompute()
        return performed
    }

    /// Local-only paging (same scope limit `ActivityStore.loadMore()`
    /// documents for itself — a remote host contributes at most one page,
    /// fetched once per `activate()`).
    public func loadMore() async {
        await local.loadMore()
        recompute()
    }

    private func recompute() {
        rows = local.rows + remoteRows
        state = Self.aggregateState(local.state, rowsAreEmpty: rows.isEmpty)
    }

    private static func aggregateState(_ localState: BlockState<Void>, rowsAreEmpty: Bool) -> BlockState<Void> {
        if case .failed(let message) = localState { return .failed(message) }
        if case .loading = localState { return .loading }
        return rowsAreEmpty ? .empty : .content(())
    }

    // MARK: - Remote hosts (progressive per-host loading)

    /// Discovers the fleet and fires one fetch per online, non-local host —
    /// each an independent `Task`, so one slow/erroring host never delays or
    /// fails another's. See `ActivityStore.loadRemoteHosts`'s doc comment
    /// for the full "never block, never let one host's trouble read as this
    /// store's trouble" rationale this mirrors.
    private func loadRemoteHosts(generation: Int) {
        let task = Task { [weak self] in
            guard let self else { return }
            let onlineRemoteHosts: [APIHostRow]
            do {
                let hosts = try await self.client.hosts()
                onlineRemoteHosts = hosts.filter { $0.status == "online" && $0.id != Self.localHost }
            } catch {
                onlineRemoteHosts = []
            }
            guard generation == self.remoteGeneration else { return }
            await self.beginRemoteHostLoads(onlineRemoteHosts, generation: generation)
        }
        remoteHostTasks.append(task)
    }

    private func beginRemoteHostLoads(_ hosts: [APIHostRow], generation: Int) async {
        pendingHosts = hosts.count
        for host in hosts {
            let task = Task { [weak self] in
                guard let self else { return }
                await self.loadRemoteHost(host, generation: generation)
            }
            remoteHostTasks.append(task)
        }
    }

    /// Fetches page 0 of `host`'s cycles; a failure contributes nothing (not
    /// a store-wide failure) — matches `ActivityStore.loadRemoteHost`'s
    /// per-host isolation. Decrements `pendingHosts` and recomputes exactly
    /// once regardless of outcome, so "+N hosts loading…" always drains to
    /// zero.
    private func loadRemoteHost(_ host: APIHostRow, generation: Int) async {
        let fetched = try? await client.autoflowCycles(offset: 0, limit: Self.remotePageSize, host: host.id)
        guard generation == remoteGeneration else { return }
        if let fetched {
            remoteRows.append(contentsOf: fetched)
        }
        pendingHosts = max(0, pendingHosts - 1)
        recompute()
    }

    private func cancelRemoteHostTasks() {
        for task in remoteHostTasks {
            task.cancel()
        }
        remoteHostTasks.removeAll()
    }
}
