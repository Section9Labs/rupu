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
///
/// **`activate`/`deactivate` are fully symmetric and repeatable**
/// (review fix): `init` takes a `signalsFactory` — not a single
/// already-built `AsyncStream` — because an `AsyncStream` can only be
/// consumed once. Task 6's natural `.onAppear`/`.onDisappear` wiring calls
/// `activate`/`deactivate` every time the screen appears/disappears, not
/// just once per store lifetime; a single-consumption stream would make
/// the *second* `activate()` a silent no-op (no stream, no live patches,
/// `freshness` stuck wherever it was left). Every `activate(kind:)` call —
/// not just the first — calls `signalsFactory()` for a fresh stream and
/// builds a fresh `StreamLifecycle` around it, stopping any prior one
/// first (so calling `activate` again *without* an intervening
/// `deactivate` — e.g. switching `kind` while the screen stays visible —
/// is also safe, not just the deactivate-then-reactivate case).
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
    private let signalsFactory: @Sendable () -> AsyncStream<StreamSignal<CPEvent>>
    private var lifecycle: StreamLifecycle?
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

    /// Review fix: an in-place live patch (`patchRow`) was being silently
    /// reverted the next time anything called `recompute()` for an
    /// unrelated reason (e.g. `statusFilter`'s `didSet`) — `recompute()`
    /// rebuilds `rows` from scratch via `activeSnapshots().flatMap(\.rows)`,
    /// which are the *unpatched* `PagedSnapshot` rows; the direct mutation
    /// `rows[index] = ...` in `patchRow` only ever touched that one
    /// already-materialized `rows` array, not the snapshots underneath it.
    /// This overlay is the fix: `patchRow` records into it (keyed by run
    /// id, the same key `patchRow` already resolves rows by), and
    /// `recompute()` reapplies every recorded override on top of the fresh
    /// merge — so a live patch survives any number of filter-driven or
    /// otherwise-triggered recomputes. Cleared on every real REST refresh
    /// (`refreshActiveSources()`): fresh server data is definitionally
    /// current, so a stale override must not go on shadowing it forever.
    private var statusOverrides: [String: (status: ActivityStatus, durationMS: UInt64?)] = [:]

    public init(
        client: CPClient,
        signalsFactory: @escaping @Sendable () -> AsyncStream<StreamSignal<CPEvent>>,
        debounceInterval: Duration = .milliseconds(500)
    ) {
        self.client = client
        self.signalsFactory = signalsFactory
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
    /// the merged `rows`, and (re)starts the live-patch stream from a fresh
    /// `signalsFactory()` call — every call, not just the first; see the
    /// type's doc comment on why `activate`/`deactivate` must be repeatable.
    public func activate(kind: RunKindFilter) async {
        self.kind = kind
        await refreshActiveSources()
        restartStream()
    }

    /// Stops the live-patch stream, cancels any pending debounced refresh,
    /// and resets `freshness` to `.idle` (there's no stream to be live or
    /// stale about anymore). Idempotent — safe to call more than once, or
    /// before `activate(kind:)` ever ran.
    public func deactivate() {
        debounceTask?.cancel()
        debounceTask = nil
        lifecycle?.stop()
        lifecycle = nil
        freshness = .idle
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
    /// resnapshot — one path, four callers.
    ///
    /// Returns `true` only if *every* active source's `refresh()` actually
    /// ran — `false` if any of them were skipped because they were already
    /// mid-fetch (Task 4's `PagedSnapshot.inFlight` guard). Most callers
    /// (`activate`, `applyPendingRefresh`, the resnapshot closure) ignore
    /// this and just accept the skip — a concurrent caller's in-flight
    /// fetch will land soon enough. The debounced live-tail refresh is the
    /// one caller that acts on it: a skip there means the burst of
    /// `newRun`s it was trying to materialize would otherwise go
    /// unrefreshed with zero signal, so it retries once (see
    /// `scheduleDebouncedRefresh`).
    @discardableResult
    private func refreshActiveSources() async -> Bool {
        var allPerformed = true
        for snapshot in activeSnapshots() {
            let performed = await snapshot.refresh()
            allPerformed = allPerformed && performed
        }
        // Real REST data supersedes any live-patch overlay standing in for
        // it — clear before recomputing so a stale override can't shadow
        // fresh server truth.
        statusOverrides.removeAll()
        recompute()
        return allPerformed
    }

    /// Rebuilds `rows` from the active `PagedSnapshot`s and reapplies
    /// `statusOverrides` on top — every caller of `recompute()` (not just
    /// the live-patch path itself) must see patched rows stay patched; see
    /// `statusOverrides`'s doc comment for why the overlay exists at all.
    private func recompute() {
        var merged = activeSnapshots().flatMap(\.rows)
        for index in merged.indices {
            guard case .run(let runID, _) = merged[index].navigation,
                  let override = statusOverrides[runID]
            else { continue }
            merged[index] = merged[index].patchingStatus(override.status, durationMS: override.durationMS)
        }
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

    /// Stops any lifecycle from a previous `activate()` (a no-op the very
    /// first time), builds a fresh one around a fresh `signalsFactory()`
    /// stream, and starts it. Called unconditionally from every
    /// `activate(kind:)` — see the type's doc comment.
    private func restartStream() {
        lifecycle?.stop()
        freshness = .idle

        let newLifecycle = StreamLifecycle()
        lifecycle = newLifecycle
        newLifecycle.start(
            signals: signalsFactory(),
            resnapshot: { [weak self] in
                guard let self else { return }
                await self.refreshActiveSources()
            },
            apply: { [weak self] event in
                self?.apply(event)
            }
        )
        observeFreshness(newLifecycle)
    }

    private func apply(_ event: CPEvent) {
        switch ActivityDelta.reduce(event) {
        case .statusPatch(let runID, let status, let durationMS):
            // Not kind-guarded: `patchRow` only ever finds a match inside
            // the currently-scoped `rows`, so a patch for a run outside
            // the active kind is already a harmless no-op.
            patchRow(runID: runID, status: status, durationMS: durationMS)
        case .newRun(let runID):
            guard canReceiveNewRunNotifications else { break }
            // Review fix: the server firehose replays every active run's
            // `events.jsonl` from offset 0 on each (re)connect, so a
            // `.newRun` routinely fires for a run this store already has —
            // most obviously right after `StreamLifecycle`'s own reconnect
            // resnapshot, which just re-fetched it honestly. Without this
            // guard, that replay inflates `pendingNewRuns`/re-triggers a
            // refresh for a run that was never actually new.
            guard !isRunAlreadyVisible(runID) else { break }
            if liveTail {
                scheduleDebouncedRefresh()
            } else {
                pendingNewRuns += 1
            }
        case .none:
            break
        }
    }

    /// `CPEvent.runStarted` describes an orchestrator-executed run — it
    /// carries no kind tag, so there is no way to tell from the event
    /// itself whether the new run belongs to a source this store's current
    /// `kind` even includes. Scoping honestly to the sources a
    /// `runStarted` event *can* describe (`.all`/`.workflows`/
    /// `.autoflows` — all backed by the orchestrator) rather than guessing
    /// keeps the "N new" pill honest: an `.agents`/`.sessions`-scoped view
    /// would otherwise count (or refresh for) a run that can never appear
    /// in it, and `applyPendingRefresh()` would then zero the counter
    /// while visibly changing nothing — breaking the pill's promise.
    private var canReceiveNewRunNotifications: Bool {
        switch kind {
        case .all, .workflows, .autoflows: true
        case .agents, .sessions: false
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
        statusOverrides[runID] = (status, durationMS)
        guard let index = rows.firstIndex(where: {
            if case .run(let id, _) = $0.navigation { return id == runID }
            return false
        }) else { return }
        rows[index] = rows[index].patchingStatus(status, durationMS: durationMS)
    }

    /// True if `runID` is already represented in this store — either in the
    /// current (possibly `statusFilter`-narrowed) `rows`, or in one of the
    /// active snapshots' unfiltered rows (a run the filter is currently
    /// hiding is still one this store has, just not one it's showing).
    /// Matched by the same `.run(id:_)` navigation key `patchRow` resolves
    /// rows by.
    private func isRunAlreadyVisible(_ runID: String) -> Bool {
        func matches(_ row: ActivityRow) -> Bool {
            if case .run(let id, _) = row.navigation { return id == runID }
            return false
        }
        if rows.contains(where: matches) { return true }
        return activeSnapshots().contains { $0.rows.contains(where: matches) }
    }

    /// Coalesces a burst of `.newRun` deltas into one refresh: each new
    /// delta cancels and restarts the wait, so only the burst's last event
    /// actually pays for a round trip.
    ///
    /// `isRetry` guards against an unbounded retry loop: if the debounced
    /// refresh collides with some other concurrent refresh already in
    /// flight (`refreshActiveSources()` returns `false` — see that
    /// method's doc comment), the burst it was trying to materialize would
    /// otherwise be silently dropped, so it's rescheduled once more. If
    /// *that* retry also collides, this gives up rather than retrying
    /// forever — the next `newRun`/reconnect/manual refresh self-heals
    /// beyond that, and an unbounded retry loop against a source that's
    /// somehow always busy would be worse than one stale pill.
    private func scheduleDebouncedRefresh(isRetry: Bool = false) {
        debounceTask?.cancel()
        debounceTask = Task { [weak self, debounceInterval] in
            try? await Task.sleep(for: debounceInterval)
            guard !Task.isCancelled, let self else { return }
            let allPerformed = await self.refreshActiveSources()
            if !allPerformed && !isRetry {
                self.scheduleDebouncedRefresh(isRetry: true)
            }
        }
    }

    /// Bridges `StreamLifecycle`'s own `@Observable` `freshness` into this
    /// store's — same `withObservationTracking` re-subscribe pattern as
    /// `BackendController.observe(_:)`. Takes the specific instance to
    /// track (rather than reading `self.lifecycle` inside the tracked
    /// closure) and re-checks `self.lifecycle === lc` before each
    /// re-subscribe, so a `deactivate()`/`activate()` cycle that replaces
    /// `lifecycle` out from under an in-flight observation doesn't leave a
    /// stale subscription forwarding a now-defunct instance's changes.
    private func observeFreshness(_ lc: StreamLifecycle) {
        withObservationTracking {
            _ = lc.freshness
        } onChange: { [weak self, weak lc] in
            Task { @MainActor in
                guard let self, let lc, self.lifecycle === lc else { return }
                self.freshness = lc.freshness
                self.observeFreshness(lc)
            }
        }
    }
}
