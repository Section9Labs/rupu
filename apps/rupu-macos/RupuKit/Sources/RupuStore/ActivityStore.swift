import Foundation
import Observation
import RupuAPI

/// Federated, live-patched view over the four run-history sources the
/// Activity screen shows (workflow runs, agent runs, autoflow events,
/// sessions). Each source is its own `PagedSnapshot<ActivityRow>`;
/// `activate(kind:)` decides which of the four are in scope for the merged
/// `rows` (one source for `.agents`/`.workflows`/`.autoflows`/`.sessions`,
/// all four for `.all`) and sorts the union by `startedAt` descending
/// (`nil` dates sort last, matching `ActivityRow.startedAt`'s "unknown"
/// meaning rather than treating it as "oldest").
///
/// **`kind` vs `activate(kind:)`**: `kind` is a plain stored property (its
/// current value is what `loadMore()`/`recompute()`/the live-patch/resnapshot
/// paths all read), but changing it *only* takes effect by calling
/// `activate(kind:)` again — an async network load can't happen inside a
/// synchronous property observer. `statusFilter`, by contrast, narrows rows
/// already sitting in memory, so its `didSet` recomputes synchronously with
/// no refetch.
///
/// Live updates ride Task 4's `StreamLifecycle`: `ActivityDelta.reduce(_:)`
/// turns each `CPEvent` into either a `.statusPatch` (an in-place field
/// update on an already-visible row — no refetch) or a `.newRun` (a run
/// this store has never fetched, which it deliberately never synthesizes
/// from event fields alone — an event doesn't carry enough to build an
/// honest `ActivityRow`, per the brief). A `.newRun` while `liveTail` is
/// off just increments `pendingNewRuns` (a "N new" pill the user opts into
/// via `applyPendingRefresh()`); while `liveTail` is on it instead schedules
/// a debounced (`debounceInterval`, default 500ms — coalescing a burst of
/// several `newRun`s into one round trip) page-0 refresh of the active
/// sources, since a refetch is the only way to get an honest row for it.
/// `StreamLifecycle`'s reconnect resnapshot reuses the exact same
/// "refresh the active sources, then recompute" path.
///
/// **Idempotence under a stale post-reconnect event** (Task 4's noted
/// tolerance: after a reconnect's resnapshot, one leftover event from the
/// dead connection may still arrive): a `.statusPatch` for a `runID` no
/// longer present in the freshly-resnapshotted `rows` is a harmless no-op
/// (`patchRow` just doesn't find a match); a stray `.newRun` for a run the
/// resnapshot already picked up is a harmless redundant
/// increment-or-refresh, not a correctness issue.
///
/// **Deviation from the brief's literal `init(client:stream: any
/// EventStreaming<CPEvent>)`**: Task 4's fix report replaced that protocol
/// seam with a concrete `AsyncStream<StreamSignal<T>>` — a settable
/// `onConnectionChange` on `JSONEventStream` doesn't compile under Swift 6
/// strict concurrency (see that report's "Streaming seam decision"). This
/// type's `init` compiles against the shape Task 4 actually shipped.
@MainActor
@Observable
public final class ActivityStore {
    public var kind: RunKindFilter = .all

    public var statusFilter: Set<ActivityStatus> = [] {
        didSet {
            guard statusFilter != oldValue else { return }
            recompute()
        }
    }

    public var liveTail: Bool = true

    public private(set) var rows: [ActivityRow] = []
    public private(set) var state: BlockState<Void> = .loading
    public private(set) var pendingNewRuns: Int = 0
    public private(set) var freshness: StreamLifecycle.Freshness = .idle

    private let client: CPClient
    private var pendingSignals: AsyncStream<StreamSignal<CPEvent>>?
    private let lifecycle = StreamLifecycle()
    private let debounceInterval: Duration
    private var debounceTask: Task<Void, Never>?

    // Not `lazy`: the `@Observable` macro's storage-transform generates an
    // init accessor for every stored property, and init accessors can only
    // refer to other *stored* properties — `lazy var` desugars to a
    // computed property backed by a hidden optional, which the macro can't
    // thread through. Built eagerly in `init` instead, each capturing the
    // `client` *parameter* (not `self.client`) so nothing here needs `self`
    // before every stored property is set.
    private let workflowSnapshot: PagedSnapshot<ActivityRow>
    private let agentSnapshot: PagedSnapshot<ActivityRow>
    private let autoflowSnapshot: PagedSnapshot<ActivityRow>
    private let sessionSnapshot: PagedSnapshot<ActivityRow>

    public init(
        client: CPClient,
        signals: AsyncStream<StreamSignal<CPEvent>>,
        debounceInterval: Duration = .milliseconds(500)
    ) {
        self.client = client
        self.pendingSignals = signals
        self.debounceInterval = debounceInterval
        self.workflowSnapshot = PagedSnapshot { offset, limit in
            try await client.workflowRuns(offset: offset, limit: limit).map(ActivityRow.init)
        }
        self.agentSnapshot = PagedSnapshot { offset, limit in
            try await client.agentRuns(offset: offset, limit: limit).map(ActivityRow.init)
        }
        self.autoflowSnapshot = PagedSnapshot { offset, limit in
            try await client.autoflowEvents(offset: offset, limit: limit).map(ActivityRow.init)
        }
        self.sessionSnapshot = PagedSnapshot { offset, limit in
            try await client.sessions(offset: offset, limit: limit).map(ActivityRow.init)
        }
    }

    /// Sets `kind`, refreshes page 0 of the sources it implies, recomputes
    /// the merged `rows`, and (the first time only — `signals` can only be
    /// consumed once) starts the live-patch stream.
    public func activate(kind: RunKindFilter) async {
        self.kind = kind
        await refreshActiveSources()
        startStreamIfNeeded()
    }

    /// Stops the live-patch stream and cancels any pending debounced
    /// refresh. Idempotent — safe to call more than once, or before
    /// `activate(kind:)` ever ran.
    public func deactivate() {
        debounceTask?.cancel()
        debounceTask = nil
        lifecycle.stop()
    }

    public func loadMore() async {
        for snapshot in activeSnapshots() {
            await snapshot.loadMore()
        }
        recompute()
    }

    /// Applies the "N new" pill: refreshes page 0 of the active sources
    /// (the only honest way to materialize the runs `pendingNewRuns` was
    /// counting) and zeroes the counter.
    public func applyPendingRefresh() async {
        await refreshActiveSources()
        pendingNewRuns = 0
    }

    // MARK: - Sources

    private func activeSnapshots() -> [PagedSnapshot<ActivityRow>] {
        switch kind {
        case .all: [workflowSnapshot, agentSnapshot, autoflowSnapshot, sessionSnapshot]
        case .workflows: [workflowSnapshot]
        case .agents: [agentSnapshot]
        case .autoflows: [autoflowSnapshot]
        case .sessions: [sessionSnapshot]
        }
    }

    /// Refreshes page 0 of every currently-active source and recomputes the
    /// merged view. Shared by `activate(kind:)`, `applyPendingRefresh()`,
    /// the debounced live-tail refresh, and `StreamLifecycle`'s reconnect
    /// resnapshot — one path, four callers. Each underlying
    /// `PagedSnapshot.refresh()` already no-ops if it's mid-fetch (Task 4's
    /// `inFlight` guard), so an overlapping caller here just accepts the
    /// skip rather than double-fetching or queuing.
    private func refreshActiveSources() async {
        for snapshot in activeSnapshots() {
            await snapshot.refresh()
        }
        recompute()
    }

    private func recompute() {
        var merged = activeSnapshots().flatMap(\.rows)
        if !statusFilter.isEmpty {
            merged = merged.filter { statusFilter.contains($0.status) }
        }
        merged.sort(by: Self.isOrderedByStartedAtDescending)
        rows = merged
        state = Self.aggregateState(activeSnapshots().map(\.state), rowsAreEmpty: merged.isEmpty)
    }

    /// Descending by `startedAt`; rows with no `startedAt` (a status this
    /// store's kind never guarantees, e.g. an unstarted agent row) sort
    /// last rather than first — an unknown start time isn't "oldest".
    private static func isOrderedByStartedAtDescending(_ lhs: ActivityRow, _ rhs: ActivityRow) -> Bool {
        switch (lhs.startedAt, rhs.startedAt) {
        case let (l?, r?): return l > r
        case (nil, nil): return false
        case (nil, _): return false
        case (_, nil): return true
        }
    }

    /// Aggregates the merged view's `state` from its active sources': any
    /// failure wins outright (surfacing the first one found), else any
    /// still-loading source keeps the merged view `.loading`, else it's
    /// `.content`/`.empty` based on the merged row count post-filter.
    private static func aggregateState(_ states: [BlockState<Void>], rowsAreEmpty: Bool) -> BlockState<Void> {
        for case .failed(let message) in states { return .failed(message) }
        if states.contains(where: { if case .loading = $0 { return true }; return false }) {
            return .loading
        }
        return rowsAreEmpty ? .empty : .content(())
    }

    // MARK: - Live stream

    private func startStreamIfNeeded() {
        guard let signals = pendingSignals else { return } // already started by an earlier activate()
        pendingSignals = nil
        lifecycle.start(
            signals: signals,
            resnapshot: { [weak self] in
                guard let self else { return }
                await self.refreshActiveSources()
            },
            apply: { [weak self] event in
                self?.apply(event)
            }
        )
        observeFreshness()
    }

    private func apply(_ event: CPEvent) {
        switch ActivityDelta.reduce(event) {
        case .statusPatch(let runID, let status, let durationMS):
            patchRow(runID: runID, status: status, durationMS: durationMS)
        case .newRun:
            if liveTail {
                scheduleDebouncedRefresh()
            } else {
                pendingNewRuns += 1
            }
        case .none:
            break
        }
    }

    /// Matched by `navigation`'s run id, not `row.id`: an autoflow row's
    /// `id` is its *event* id (`eventID`), not the run id a `CPEvent`
    /// carries. `navigation`'s `.run(id:host:)` case is the one field every
    /// run-bearing row kind (workflow/agent/autoflow) populates with the
    /// actual run id, so it's the only key that's correct across all three.
    /// Session rows never carry `.run` navigation, so they're correctly
    /// excluded — a session's status is derived from its
    /// `activeRunID`/`lastError` at fetch time, not live-patched from run
    /// events.
    private func patchRow(runID: String, status: ActivityStatus, durationMS: UInt64?) {
        guard let index = rows.firstIndex(where: {
            if case .run(let id, _) = $0.navigation { return id == runID }
            return false
        }) else { return }
        rows[index] = rows[index].patchingStatus(status, durationMS: durationMS)
    }

    /// Coalesces a burst of `.newRun` deltas into one refresh: each new
    /// delta cancels and restarts the wait, so only the burst's last event
    /// actually pays for a round trip.
    private func scheduleDebouncedRefresh() {
        debounceTask?.cancel()
        debounceTask = Task { [weak self, debounceInterval] in
            try? await Task.sleep(for: debounceInterval)
            guard !Task.isCancelled, let self else { return }
            await self.refreshActiveSources()
        }
    }

    /// Bridges `StreamLifecycle`'s own `@Observable` `freshness` into this
    /// store's — same `withObservationTracking` re-subscribe pattern as
    /// `BackendController.observe(_:)`.
    private func observeFreshness() {
        withObservationTracking {
            _ = lifecycle.freshness
        } onChange: { [weak self] in
            Task { @MainActor in
                guard let self else { return }
                self.freshness = self.lifecycle.freshness
                self.observeFreshness()
            }
        }
    }
}
