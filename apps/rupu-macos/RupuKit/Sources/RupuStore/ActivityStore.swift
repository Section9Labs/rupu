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

    /// Count of hosts (fleet nodes other than `local`) whose per-source
    /// fetch for the currently active sources hasn't answered yet — driven
    /// by `loadRemoteHosts`. `0` at rest (nothing pending) and always `0`
    /// while a fleet has no online remote hosts at all. The UI (`FilterBar`/
    /// `ActivityScreen`) reads this to show a subtle "+N hosts loading…"
    /// label; it is never part of `state`, which reflects the local load
    /// only — see that property's doc comment.
    public private(set) var pendingHosts: Int = 0

    private let client: CPClient
    private let signalsFactory: @Sendable () -> AsyncStream<StreamSignal<CPEvent>>
    private var lifecycle: StreamLifecycle?
    private let debounceInterval: Duration
    private var debounceTask: Task<Void, Never>?

    /// One entry per federated source, pairing its local `PagedSnapshot`
    /// (host=local, paged, live-patchable — the existing machinery) with a
    /// `remoteFetch` closure for the progressive per-host enrichment
    /// (`loadRemoteHost`) added for the "lazy load per host, never block on
    /// a slow/offline one" fix (matt's directive, verbatim in the fix
    /// report: "lazy load things as you gather them ... you should not
    /// fail all for a single host"). Built once in `init`; `activeSources()`
    /// is the kind-filtered view every other method reads through.
    //
    // Not `lazy`: the `@Observable` macro's storage-transform generates an
    // init accessor for every stored property, and init accessors can only
    // refer to other *stored* properties — `lazy var` desugars to a
    // computed property backed by a hidden optional, which the macro can't
    // thread through. Built eagerly in `init` instead, capturing the
    // `client` *parameter* (not `self.client`) so nothing here needs `self`
    // before every stored property is set.
    private let sources: [Source]

    /// Rows fetched from a non-local host, keyed by which source they came
    /// from (a host can be online for one active source's endpoint and
    /// erroring for another; this keeps them independent). Populated
    /// incrementally by `loadRemoteHost` as each host answers, cleared at
    /// the top of every `activate(kind:)` — `recompute()` folds these in
    /// alongside the local snapshots' rows, but `state` (see that
    /// property's doc comment) never depends on them.
    private var remoteRowsBySource: [ActivityKindTag: [ActivityRow]] = [:]

    /// Bumped at the top of every `activate(kind:)` and captured by
    /// `loadRemoteHosts`/`loadRemoteHost` at the point each async hop
    /// starts: a completion whose captured generation no longer matches
    /// `remoteGeneration` belongs to a superseded load (a `kind` switch, or
    /// a `deactivate()`) and is discarded rather than mutating
    /// `remoteRowsBySource`/`pendingHosts` out from under the current one.
    private var remoteGeneration = 0

    /// Every in-flight remote-host `Task` from the current `activate(kind:)`
    /// cycle, so `deactivate()` (and the next `activate(kind:)`) can cancel
    /// them outright rather than letting a now-irrelevant fetch run to
    /// completion in the background.
    private var remoteHostTasks: [Task<Void, Never>] = []

    /// One federated source: its kind tag, its local (host=local) paged
    /// snapshot, and a closure for fetching one page of it from an
    /// arbitrary remote host. `remoteFetch` deliberately takes a plain
    /// `(host, offset, limit)` tuple rather than reusing `PagedSnapshot` —
    /// remote paging is out of scope this phase (`loadMore()` stays
    /// local-only; see its doc comment), so a remote fetch is always a
    /// single page-0 call, never something that needs `PagedSnapshot`'s
    /// offset bookkeeping or live-patch integration.
    private struct Source {
        let kind: ActivityKindTag
        let snapshot: PagedSnapshot<ActivityRow>
        let remoteFetch: @Sendable (_ host: String, _ offset: Int, _ limit: Int) async throws -> [ActivityRow]
    }

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
    /// otherwise-triggered recomputes. Cleared wholesale on every real REST
    /// refresh (`refreshActiveSources()`): fresh server data is
    /// definitionally current, so a stale override must not go on shadowing
    /// it forever.
    ///
    /// Carry-over (Phase 3, Task 4): `recompute()` also prunes *individual*
    /// entries — an override whose runID appears in the freshly merged rows
    /// with the identical status is dropped right there, not just on the
    /// next full refresh. This matters for the recompute() call sites that
    /// aren't a full refresh (`loadMore()`, `loadRemoteHost`, `statusFilter`'s
    /// `didSet`): without per-key pruning, an override could keep shadowing
    /// (redundantly, but indefinitely) fresher merged data that already
    /// agrees with it, right up until the next `refreshActiveSources()`
    /// call happens to clear everything. `internal`, not `private` — reached
    /// directly from `ActivityStoreTests` via `@testable import RupuStore`,
    /// same seam `RunDetailStore`'s doc comment documents for its own
    /// designated init.
    internal var statusOverrides: [String: (status: ActivityStatus, durationMS: UInt64?)] = [:]

    public init(
        client: CPClient,
        signalsFactory: @escaping @Sendable () -> AsyncStream<StreamSignal<CPEvent>>,
        debounceInterval: Duration = .milliseconds(500)
    ) {
        self.client = client
        self.signalsFactory = signalsFactory
        self.debounceInterval = debounceInterval
        self.sources = [
            Source(
                kind: .workflow,
                snapshot: PagedSnapshot { offset, limit in
                    try await client.workflowRuns(offset: offset, limit: limit, host: Self.localHost).map(ActivityRow.init)
                },
                remoteFetch: { host, offset, limit in
                    try await client.workflowRuns(offset: offset, limit: limit, host: host).map(ActivityRow.init)
                }
            ),
            Source(
                kind: .agent,
                snapshot: PagedSnapshot { offset, limit in
                    try await client.agentRuns(offset: offset, limit: limit, host: Self.localHost).map(ActivityRow.init)
                },
                remoteFetch: { host, offset, limit in
                    try await client.agentRuns(offset: offset, limit: limit, host: host).map(ActivityRow.init)
                }
            ),
            Source(
                kind: .autoflow,
                snapshot: PagedSnapshot { offset, limit in
                    try await client.autoflowEvents(offset: offset, limit: limit, host: Self.localHost).map(ActivityRow.init)
                },
                remoteFetch: { host, offset, limit in
                    try await client.autoflowEvents(offset: offset, limit: limit, host: host).map(ActivityRow.init)
                }
            ),
            Source(
                kind: .session,
                snapshot: PagedSnapshot { offset, limit in
                    try await client.sessions(offset: offset, limit: limit, host: Self.localHost).map(ActivityRow.init)
                },
                remoteFetch: { host, offset, limit in
                    try await client.sessions(offset: offset, limit: limit, host: host).map(ActivityRow.init)
                }
            ),
        ]
    }

    private static let localHost = "local"
    private static let remotePageSize = 50

    /// Sets `kind`, refreshes page 0 of the *local* sources it implies
    /// (parallel across sources, never blocking on a remote host — see
    /// `refreshActiveSources`), recomputes the merged `rows`, (re)starts the
    /// live-patch stream from a fresh `signalsFactory()` call, and then
    /// kicks off progressive per-host enrichment in the background
    /// (`loadRemoteHosts`) — every call, not just the first; see the type's
    /// doc comment on why `activate`/`deactivate` must be repeatable.
    ///
    /// **Local-first, remote-progressive** (the fix for matt's live-tested
    /// bug: a fleet with one offline host turned every Activity load into a
    /// several-second stall): this method *returns* once the local load
    /// finishes — `state`/`rows` are already showing local truth by the
    /// time an `await activate(kind:)` call resumes. Remote hosts are
    /// discovered and fetched afterward, off this call's critical path,
    /// each independently merging its rows in (or contributing nothing, on
    /// error) as it answers; `pendingHosts` is the only signal a caller
    /// gets that more may still arrive.
    public func activate(kind: RunKindFilter) async {
        self.kind = kind
        remoteGeneration += 1
        let generation = remoteGeneration
        cancelRemoteHostTasks()
        remoteRowsBySource.removeAll()
        pendingHosts = 0

        await refreshActiveSources()
        restartStream()

        loadRemoteHosts(generation: generation)
    }

    /// Stops the live-patch stream, cancels any pending debounced refresh
    /// and any still-in-flight remote-host fetches, and resets `freshness`
    /// to `.idle` (there's no stream to be live or stale about anymore).
    /// Idempotent — safe to call more than once, or before `activate(kind:)`
    /// ever ran.
    public func deactivate() {
        debounceTask?.cancel()
        debounceTask = nil
        lifecycle?.stop()
        lifecycle = nil
        freshness = .idle
        remoteGeneration += 1
        cancelRemoteHostTasks()
        pendingHosts = 0
    }

    /// Local-only this phase: each active source's `PagedSnapshot` (host=
    /// local) advances by one page. Remote-host rows are fetched once per
    /// `activate(kind:)`, page 0 only — paging a remote host's own history
    /// is deferred; a fleet host answering `loadRemoteHosts` always
    /// contributes at most one page's worth of rows.
    public func loadMore() async {
        for snapshot in activeSnapshots() {
            await snapshot.loadMore()
        }
        recompute()
    }

    /// Applies the "N new" pill: refreshes page 0 of the active local
    /// sources (the only honest way to materialize the runs
    /// `pendingNewRuns` was counting) and zeroes the counter. Remote hosts
    /// are not re-fetched here — they were already loaded once by the
    /// current `activate(kind:)` cycle; re-discovering the fleet on every
    /// pill click isn't worth the extra round trips this phase.
    public func applyPendingRefresh() async {
        await refreshActiveSources()
        pendingNewRuns = 0
    }

    // MARK: - Sources

    private func activeSources() -> [Source] {
        switch kind {
        case .all: sources
        case .workflows: sources.filter { $0.kind == .workflow }
        case .agents: sources.filter { $0.kind == .agent }
        case .autoflows: sources.filter { $0.kind == .autoflow }
        case .sessions: sources.filter { $0.kind == .session }
        }
    }

    private func activeSnapshots() -> [PagedSnapshot<ActivityRow>] {
        activeSources().map(\.snapshot)
    }

    /// Refreshes page 0 of every currently-active source's **local**
    /// snapshot — never a remote host; see `loadRemoteHosts` for that —
    /// and recomputes the merged view. Shared by `activate(kind:)`,
    /// `applyPendingRefresh()`, the debounced live-tail refresh, and
    /// `StreamLifecycle`'s reconnect resnapshot — one path, four callers.
    /// Every active source's `refresh()` is fired concurrently (final-review
    /// fix: a sequential `for` loop here paid each source's own network
    /// round trip serially, multiplying the same "why does this take
    /// seconds" symptom `host=` fan-out avoidance is fixing at the
    /// per-request level) rather than one at a time.
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
        let snapshots = activeSnapshots()
        let allPerformed: Bool
        if snapshots.isEmpty {
            allPerformed = true
        } else {
            allPerformed = await withTaskGroup(of: Bool.self) { group in
                for snapshot in snapshots {
                    group.addTask { await snapshot.refresh() }
                }
                var performedAll = true
                for await performed in group {
                    performedAll = performedAll && performed
                }
                return performedAll
            }
        }
        // Real REST data supersedes any live-patch overlay standing in for
        // it — clear before recomputing so a stale override can't shadow
        // fresh server truth.
        statusOverrides.removeAll()
        recompute()
        return allPerformed
    }

    /// Rebuilds `rows` from the active sources' local snapshots *and* any
    /// remote-host rows gathered so far (`remoteRowsBySource`), reapplies
    /// `statusOverrides` on top, and re-sorts the union — every caller of
    /// `recompute()` (not just the live-patch path itself, and not just the
    /// local-refresh path) must see the merged, patched, sorted view;
    /// `loadRemoteHost` calls this too, every time a host's rows land, so
    /// the table grows progressively rather than jumping once at the end.
    /// See `statusOverrides`'s doc comment for why the overlay exists at
    /// all.
    ///
    /// `state` (see that property's doc comment) is derived from the local
    /// snapshots' states only — a remote host answering, erroring, or never
    /// answering at all never changes whether this store reads as
    /// `.loading`/`.content`/`.empty`/`.failed`.
    private func recompute() {
        var merged = activeSnapshots().flatMap(\.rows)
        merged.append(contentsOf: activeSources().flatMap { remoteRowsBySource[$0.kind] ?? [] })

        // Carry-over (Phase 3, Task 4; review fix): pruning must be
        // order-independent. Two passes, not one: a single combined
        // prune-or-patch pass over `merged.indices` makes the outcome
        // depend on which row for a given runID is *encountered first* —
        // if that row happens to already match the override, the override
        // is removed before a later, still-mismatched row for the same
        // runID ever gets patched, silently leaving it on stale raw data.
        // (This was safe only because local rows precede remote rows in
        // `merged` above — an undocumented invariant, not a guarantee this
        // logic should lean on.) Pass 1 decides, for every overridden
        // runID, whether *any* row already agrees with it — "server caught
        // up" — and prunes exactly those, independent of row order. Pass 2
        // then applies whatever overrides are left (i.e. weren't just
        // pruned) to every row they match.
        var runIDsToPrune: Set<String> = []
        for row in merged {
            guard case .run(let runID, _) = row.navigation,
                  let override = statusOverrides[runID],
                  row.status == override.status
            else { continue }
            runIDsToPrune.insert(runID)
        }
        for runID in runIDsToPrune {
            statusOverrides.removeValue(forKey: runID)
        }
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

    // MARK: - Remote hosts (progressive per-host loading)

    /// Discovers the fleet (`GET /api/hosts`) and, for every `status ==
    /// "online"` host other than `local`, fires `loadRemoteHost` for it —
    /// each an independent `Task`, so one slow or erroring host can never
    /// delay or fail another's. `pendingHosts` is set to the online-remote
    /// count as soon as it's known (`0` if discovery itself fails or the
    /// fleet has no remote hosts — matt's directive: never block, and never
    /// let a single host's trouble read as this store's trouble).
    ///
    /// `generation` is the `remoteGeneration` captured by the `activate(kind:)`
    /// call that started this — every mutation below re-checks it before
    /// touching `pendingHosts`/`remoteRowsBySource`, so a `deactivate()` or
    /// a `kind` switch that landed while `GET /api/hosts` (or a per-host
    /// fetch) was still in flight can't have its now-irrelevant result
    /// silently applied on top of newer state.
    private func loadRemoteHosts(generation: Int) {
        let sourcesAtStart = activeSources()
        let task = Task { [weak self] in
            guard let self else { return }
            let onlineRemoteHosts: [APIHostRow]
            do {
                let hosts = try await self.client.hosts()
                onlineRemoteHosts = hosts.filter { $0.status == "online" && $0.id != Self.localHost }
            } catch {
                // Discovery itself failing must never fail the table — just
                // nothing more to add this cycle.
                onlineRemoteHosts = []
            }
            guard generation == self.remoteGeneration else { return }
            await self.beginRemoteHostLoads(onlineRemoteHosts, sources: sourcesAtStart, generation: generation)
        }
        remoteHostTasks.append(task)
    }

    private func beginRemoteHostLoads(_ hosts: [APIHostRow], sources: [Source], generation: Int) async {
        pendingHosts = hosts.count
        for host in hosts {
            let task = Task { [weak self] in
                guard let self else { return }
                await self.loadRemoteHost(host, sources: sources, generation: generation)
            }
            remoteHostTasks.append(task)
        }
    }

    /// Fetches page 0 of every active source's endpoint from `host`, in
    /// parallel across sources, and merges whatever succeeds into
    /// `remoteRowsBySource` — a source that errors for this host
    /// contributes nothing (not a store-wide failure; matt's directive
    /// verbatim: "you should not fail all for a single host"), and neither
    /// does the whole host if every one of its sources errors. Decrements
    /// `pendingHosts` and recomputes exactly once, whether this host
    /// contributed rows or not, so the "+N hosts loading…" indicator always
    /// drains to zero even for a host that turns out to have nothing usable.
    private func loadRemoteHost(_ host: APIHostRow, sources: [Source], generation: Int) async {
        var collected: [(ActivityKindTag, [ActivityRow])] = []
        await withTaskGroup(of: (ActivityKindTag, [ActivityRow])?.self) { group in
            for source in sources {
                group.addTask {
                    guard let rows = try? await source.remoteFetch(host.id, 0, Self.remotePageSize) else { return nil }
                    return (source.kind, rows)
                }
            }
            for await result in group {
                guard let result else { continue }
                collected.append(result)
            }
        }

        guard generation == remoteGeneration else { return }
        for (kind, rows) in collected {
            remoteRowsBySource[kind, default: []].append(contentsOf: rows)
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
    /// *orchestrator*-run-bearing row kind (workflow/autoflow) populates
    /// with the actual run id, so it's the only key that's correct across
    /// both.
    ///
    /// Agent rows are deliberately excluded (hotfix root cause C): an agent
    /// row is never an orchestrator run — `ActivityRow.init(_:
    /// APIAgentRunRow)` navigates it to `.session`/`.agentRun`, never
    /// `.run` — so it's correctly unreachable here too. That's not just
    /// harmless; it's *correct*: `CPEvent.runStarted`/`runCompleted` (what
    /// this patches from) describe orchestrator runs, and an agent row's
    /// run id was never guaranteed to line up with one of those anyway.
    /// Session rows likewise never carry `.run` navigation — a session's
    /// status is derived from its `activeRunID`/`lastError` at fetch time,
    /// not live-patched from run events.
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
