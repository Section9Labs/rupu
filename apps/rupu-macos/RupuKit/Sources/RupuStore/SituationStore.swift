import Foundation
import Observation
import RupuAPI

/// Situation Room (Phase 6B, Task 7): the fullscreen live-wall's data
/// engine. Owns everything the screen needs *raw* — the event backlog
/// (history backfill + live firehose fold), the findings/projects/dashboard
/// poll, lazy run→workspace resolution, and the events/min sampler — but
/// deliberately does **not** itself build `StreamCard`/`RosterEntry`/
/// `Vitals` view models.
///
/// **Why the fold isn't here** (module-boundary note, not an oversight):
/// this file lives in `RupuStore`, and Task 6's pure derivations
/// (`cardForEvent`/`cardForFinding`/`mergeStream`/`deriveActivity`/
/// `reconcileActivity`/`foldRoster`/`buildVitals`, plus the `StreamCard`/
/// `RosterEntry`/`Vitals` types themselves) live in `RupuSituation`. The
/// task-7 brief's own Package.swift instruction is "`RupuSituation` gains a
/// `RupuStore` dependency for the screen layer" — ONE direction only.
/// `RupuStore` already has plenty of consumers (`RupuActivity`,
/// `RupuOverview`, `RupuShell`, ...); if this file also imported
/// `RupuSituation`, the two modules would depend on each other and Swift
/// Package Manager would refuse to build the target graph at all. So the
/// split is: this store fetches/tails/caches raw `RupuAPI` wire types, and
/// `RupuSituation`'s screen layer (`SituationRoomScreen` and friends, which
/// *can* import both) is what calls Task 6's pure functions over this
/// store's `eventRows`/`findings`/`projects`/`dashboard` snapshot on every
/// render to build the actual view models. `eventsPerMin`/`spark` are the
/// one exception worth calling out: they're plain `Int`/`[Int]`, not
/// `RupuSituation.EventRateRing` — same rationale, expressed as "duplicate
/// ~10 lines of arithmetic" rather than "import a whole module".
///
/// **Live tail**: its own independent firehose connection (`signalsFactory`,
/// same `backend.makeFirehoseStream(onConnectionChange:)` seam
/// `ActivityStore`/`DashboardStore`/`OverviewScreen` each already build their
/// own instance of — see those types' doc comments on why a shared single
/// connection can't serve two `onConnectionChange` callbacks at once). The
/// CP's `/api/events/stream` is always the LOCAL backend's own firehose
/// (this Mac IS the control plane — see `RunNotifier`'s doc comment for the
/// same "host: nil is always correct here, not a shortcut" reasoning), so
/// every `CPClient` call this store makes for a run discovered off that
/// stream (`runDetail`, `approveRun`, `rejectRun`) passes `host: nil` too.
///
/// **Reconnect-replay dedup**: the CP's SSE stream replays an active run's
/// `events.jsonl` from offset 0 on every (re)connect (see
/// `RupuSituation/StreamCards.swift`'s file header for the citation this
/// store can't literally cross-reference via `import`). Blindly prepending
/// every event `applyLive(_:)` receives would re-insert an already-seen
/// event at the front of `eventRows`, stamped with the arrival time rather
/// than its real historical `ts` — corrupting the stream's ordering on every
/// reconnect. `applyLive` guards against this with a linear
/// `eventRows.contains(where: { $0.event == event })` check: `CPEvent`'s
/// synthesized `Equatable` (every case's associated values are themselves
/// `Equatable`) makes this an exact content-identity test — the same
/// "everything except the server-injected `ts`/`pos`" identity
/// `RupuSituation`'s `contentIdentityKey`/the web's `identityOf` use — with
/// no separate identity-key function to keep in lockstep with either of
/// those (and, again, no `RupuSituation` import needed to get there).
///
/// **Poll cadence**: findings/projects/dashboard refresh every 60s (the
/// task-7 brief's own explicit value) — not the web's 15s `AGG_POLL_MS`
/// (`Events.tsx` line 34). A fullscreen ambient wall already gets its
/// immediacy from the live event tail; the aggregate poll is just there so
/// vitals/roster don't go stale over a long-running session, and 60s is a
/// deliberately slower cadence for what's the least latency-sensitive part
/// of this screen.
///
/// **`activate`/`deactivate` are fully symmetric and repeatable** — same
/// contract `ActivityStore`/`DashboardStore` already document: a fresh
/// `signalsFactory()` stream, poll loop, and tick loop are (re)built on
/// every `activate()` call, and `deactivate()` tears all three down and is
/// safe to call more than once or before `activate()` ever ran. The scene
/// (`SituationRoomScreen`) calls `deactivate()` from `.onDisappear` so the
/// live stream never outlives the Situation Room window.
@MainActor
@Observable
public final class SituationStore {
    /// History backfill + live-tail fold, newest-first (the CP's
    /// `GET /api/events` — see `collect_recent_events`'s own
    /// `recent_events_returns_newest_first_limited` test — already returns
    /// newest-first, and every live event is prepended, so the invariant
    /// holds without this store needing to sort).
    public private(set) var eventRows: [CPEventRow] = []
    public private(set) var findings: [APIFinding] = []
    public private(set) var findingsSummary: APIFindingsSummary?
    public private(set) var projects: [APIProjectRow] = []
    public private(set) var dashboard: APIDashboardResponse?
    public private(set) var freshness: StreamLifecycle.Freshness = .idle

    /// Lazy `run_id` → `workspace_id` resolution (Events.tsx lines 179-224's
    /// `getRun` effect, ported below in `resolveRuns(generation:)`) — events
    /// carry no project of their own, so the screen's `resolveProject`
    /// equivalent reads through this cache (falling back to a finding
    /// card's own `projectName`, which needs no resolution at all).
    public private(set) var runToWorkspace: [String: String] = [:]
    /// Authoritative terminal `run.json` status for a run whose event log
    /// went quiet mid-step (cancelled before a terminal event was ever
    /// appended, or a crashed runner) — same "an event stream alone can
    /// lie about whether a run is really over" rationale as the web's
    /// `runStatus` map.
    public private(set) var runTerminalStatus: [String: String] = [:]

    /// Events-per-minute, sampled every `tickInterval` (5s, matching the
    /// web's `SPARK_TICK_MS`) from a per-tick counter reset each tick — see
    /// the type's doc comment on why this is a plain `Int`/`[Int]` rather
    /// than `RupuSituation.EventRateRing`.
    public private(set) var eventsPerMin: Int = 0
    /// Fixed-length (`sparkLength` = 16, matching `SPARK_LEN`), newest-last
    /// ring of per-tick event counts, for the PulseStrip's mini sparkline.
    public private(set) var spark: [Int] = Array(repeating: 0, count: SituationStore.sparkLength)

    public let pendingActions: PendingActions

    private let client: CPClient
    private let signalsFactory: @Sendable () -> AsyncStream<StreamSignal<CPEvent>>
    private var lifecycle: StreamLifecycle?
    private var pollTask: Task<Void, Never>?
    private var tickTask: Task<Void, Never>?

    /// Bumped at the top of every `activate()`/`deactivate()` and captured
    /// by every async hop (poll cycle, tick loop, run-resolution fetch) at
    /// the point it starts — same "a completion from a superseded cycle is
    /// discarded, not applied" idiom `ActivityStore.remoteGeneration`/
    /// `DashboardStore.generation` already use.
    private var generation = 0

    private var requestedRuns: Set<String> = []
    private var statusCheckedAt: [String: Date] = [:]

    /// Instance-level, not `Self.maxEventRows`, so `SituationStoreTests` can
    /// inject a small cap and prove enforcement in milliseconds rather than
    /// needing 5,000+ live events to reach the production default — same
    /// injectable-constant convention `DashboardStore`'s `debounceInterval`/
    /// `reconcileInterval` init parameters already use for the same reason.
    private let maxEventRows: Int
    private static let historyPageSize = 200 // Events.tsx PAGE_SIZE (line 32)
    private static let pollInterval: Duration = .seconds(60) // brief's explicit cadence — see type doc comment
    private static let staleRunInterval: TimeInterval = 120 // Events.tsx STALE_RUN_MS (line 39)
    private static let staleRecheckInterval: TimeInterval = 60 // Events.tsx STALE_RECHECK_MS (line 40)
    private static let resolveBudget = 12 // Events.tsx's per-pass `budget` (lines 205, 212, 216)
    private static let tickInterval: Duration = .seconds(5) // Events.tsx SPARK_TICK_MS (line 35)
    private static let tickIntervalMS: Double = 5_000
    fileprivate static let sparkLength = 16 // Events.tsx SPARK_LEN (line 36)
    private static let terminalStatuses: Set<String> = ["completed", "failed", "cancelled", "rejected"]

    public init(
        client: CPClient,
        signalsFactory: @escaping @Sendable () -> AsyncStream<StreamSignal<CPEvent>>,
        pendingActions: PendingActions = PendingActions(),
        maxEventRows: Int = 5_000 // Events.tsx MAX_EVENTS (line 33)
    ) {
        self.client = client
        self.signalsFactory = signalsFactory
        self.pendingActions = pendingActions
        self.maxEventRows = maxEventRows
    }

    // MARK: - Lifecycle

    /// History backfill (awaited — the screen never renders a blank stream
    /// while this is in flight), one immediate aggregate poll, then starts
    /// the live tail, the 60s aggregate poll loop, and the 5s tick sampler.
    /// Every call — not just the first — rebuilds the three loops from
    /// scratch; `runToWorkspace`/`runTerminalStatus` are deliberately NOT
    /// cleared (a re-activation of the same store instance, e.g. the
    /// Situation Room window closing and reopening with no backend swap in
    /// between, has no reason to forget an already-resolved run→workspace
    /// mapping).
    public func activate() async {
        generation += 1
        let generation = generation
        eventsSinceTick = 0
        spark = Array(repeating: 0, count: Self.sparkLength)
        eventsPerMin = 0

        await loadHistory(generation: generation)
        restartStream()
        await pollOnce(generation: generation)
        startPolling(generation: generation)
        startTicking(generation: generation)
    }

    /// Stops the live tail, the poll loop, and the tick loop. Idempotent —
    /// safe to call more than once, or before `activate()` ever ran.
    public func deactivate() {
        generation += 1
        lifecycle?.stop()
        lifecycle = nil
        pollTask?.cancel()
        pollTask = nil
        tickTask?.cancel()
        tickTask = nil
        freshness = .idle
    }

    // MARK: - History + live tail

    private func loadHistory(generation: Int) async {
        guard let rows = try? await client.recentEvents(limit: Self.historyPageSize) else { return }
        guard generation == self.generation else { return }
        eventRows = rows
        resolveRuns(generation: generation)
    }

    private func restartStream() {
        lifecycle?.stop()
        let newLifecycle = StreamLifecycle()
        lifecycle = newLifecycle
        let generation = generation
        newLifecycle.start(
            signals: signalsFactory(),
            resnapshot: { [weak self] in
                guard let self else { return }
                await self.loadHistory(generation: generation)
            },
            apply: { [weak self] event in
                self?.applyLive(event)
            }
        )
        observeFreshness(newLifecycle)
    }

    private func applyLive(_ event: CPEvent) {
        // Reconnect-replay dedup — see the type's doc comment.
        guard !eventRows.contains(where: { $0.event == event }) else { return }

        let tsMS = Int64((Date().timeIntervalSince1970 * 1000).rounded())
        // `pos: -1` is a sentinel: `CPEventRow.pos` only matters for the
        // web's "load older" pagination cursor, a feature this fullscreen
        // ambient wall deliberately doesn't implement (see
        // `SituationRoomScreen`'s doc comment) — nothing here ever reads it.
        var rows = eventRows
        rows.insert(CPEventRow(event: event, ts: tsMS, pos: -1), at: 0)
        if rows.count > maxEventRows {
            rows.removeLast(rows.count - maxEventRows)
        }
        eventRows = rows
        eventsSinceTick += 1

        resolveRuns(generation: generation)

        // Confirms any pending approve/reject/cancel/pause/resume this
        // store itself fired (or that fired from another screen sharing
        // the same `pendingActions` ledger) — same reduction
        // `ActivityStore.apply(_:)` already uses for the identical purpose.
        if case .statusPatch(let runID, let status, _) = ActivityDelta.reduce(event) {
            pendingActions.resolve(runID: runID, observedStatus: status)
        }
    }

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

    // MARK: - Aggregate poll (findings / projects / dashboard)

    private func startPolling(generation: Int) {
        pollTask?.cancel()
        pollTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: Self.pollInterval)
                guard !Task.isCancelled, let self else { return }
                await self.pollOnce(generation: generation)
            }
        }
    }

    private func pollOnce(generation: Int) async {
        guard generation == self.generation else { return }
        async let findingsResult = try? client.findings()
        async let projectsResult = try? client.projects()
        async let dashboardResult = try? client.dashboard(range: TimeRange.all.rawValue, host: nil)
        let (f, p, d) = await (findingsResult, projectsResult, dashboardResult)
        guard generation == self.generation else { return }
        if let f {
            findings = f.findings
            findingsSummary = f.summary
        }
        if let p {
            projects = p
        }
        if let d {
            dashboard = d
        }
        // Same rationale as Events.tsx's own `dashboard` poll-tick dependency
        // (line 224's comment): a stale-looking run gets re-checked on every
        // aggregate poll tick even with no new events arriving to trigger it
        // another way.
        resolveRuns(generation: generation)
    }

    // MARK: - Lazy run → workspace + terminal-status resolution

    /// Port of Events.tsx lines 189-224's `getRun` effect: two budgeted
    /// passes sharing one `budget` (12, matching the web) — first every
    /// run seen in `eventRows` that has never been resolved or requested,
    /// then every run whose newest event has gone quiet for
    /// `staleRunInterval` and hasn't been rechecked within
    /// `staleRecheckInterval` (closing the "the event log ended mid-step but
    /// the run never really told us it was over" gap). Every fetch is
    /// fire-and-forget, not `Task`-tracked for explicit cancellation
    /// (unlike `ActivityStore.remoteHostTasks`) — the `generation` guard
    /// inside each fetch's continuation already makes a stray in-flight
    /// call from a torn-down cycle a harmless no-op, and this store's
    /// fetches are cheap enough (`GET /api/runs/:id`, budgeted at 12) that
    /// the extra bookkeeping isn't worth it here.
    private func resolveRuns(generation: Int) {
        var newestTSByRun: [String: Int64] = [:]
        for row in eventRows {
            guard let runID = row.event.runID else { continue }
            if newestTSByRun[runID] == nil {
                newestTSByRun[runID] = row.ts // eventRows is newest-first
            }
        }

        let now = Date()
        var budget = Self.resolveBudget
        var toFetch: [String] = []

        for runID in newestTSByRun.keys {
            guard budget > 0 else { break }
            guard runToWorkspace[runID] == nil, !requestedRuns.contains(runID) else { continue }
            requestedRuns.insert(runID)
            statusCheckedAt[runID] = now
            toFetch.append(runID)
            budget -= 1
        }
        for (runID, ts) in newestTSByRun {
            guard budget > 0 else { break }
            guard runTerminalStatus[runID] == nil else { continue } // already known-terminal
            let eventAge = now.timeIntervalSince(Date(timeIntervalSince1970: Double(ts) / 1000))
            guard eventAge >= Self.staleRunInterval else { continue } // still chatty
            let checked = statusCheckedAt[runID] ?? .distantPast
            guard now.timeIntervalSince(checked) >= Self.staleRecheckInterval else { continue }
            statusCheckedAt[runID] = now
            toFetch.append(runID)
            budget -= 1
        }

        for runID in toFetch {
            Task { [weak self] in
                guard let self else { return }
                guard let detail = try? await self.client.runDetail(id: runID, host: nil) else { return }
                guard generation == self.generation else { return }
                self.runToWorkspace[runID] = detail.run.workspaceID
                if Self.terminalStatuses.contains(detail.run.status) {
                    self.runTerminalStatus[runID] = detail.run.status
                }
            }
        }
    }

    // MARK: - Events/min sampler

    private var eventsSinceTick = 0

    private func startTicking(generation: Int) {
        tickTask?.cancel()
        tickTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: Self.tickInterval)
                guard !Task.isCancelled, let self, generation == self.generation else { return }
                self.tick()
            }
        }
    }

    /// Exact port of Events.tsx line 174's
    /// `Math.round((n * 60_000) / SPARK_TICK_MS)` arithmetic — see the type
    /// doc comment on why this is inline rather than
    /// `RupuSituation.eventsPerMinute(_:windowMS:)`.
    private func tick() {
        let n = eventsSinceTick
        eventsSinceTick = 0
        spark = Array(spark.dropFirst()) + [n]
        eventsPerMin = Int((Double(n) * 60_000 / Self.tickIntervalMS).rounded())
    }

    // MARK: - Mutations (await-card Approve/Reject)

    /// Marker-only, same contract as `ActivityStore.approve(runID:gate:
    /// host:)`/`RunDetailStore.approve(gate:mode:)` — the POST's 200 means
    /// *recorded*, not *done*; confirmation rides this store's own live
    /// tail via `applyLive(_:)`'s `pendingActions.resolve` call above, once
    /// the run is observed to have actually left the gate. `host: nil` —
    /// see the type's doc comment on why every mutation here targets the
    /// local backend.
    public func approve(runID: String, stepID: String) async {
        let key = ActionKey.gate(runID: runID, stepID: stepID, verb: .approve)
        pendingActions.begin(key)
        do {
            _ = try await client.approveRun(id: runID, host: nil, gate: stepID)
        } catch {
            pendingActions.fail(key, mutationErrorMessage(error))
        }
    }

    /// Same shape as `approve(runID:stepID:)`, confirmed the same way.
    public func reject(runID: String, stepID: String) async {
        let key = ActionKey.gate(runID: runID, stepID: stepID, verb: .reject)
        pendingActions.begin(key)
        do {
            _ = try await client.rejectRun(id: runID, host: nil, gate: stepID, body: RejectBody(reason: "Rejected from Situation Room"))
        } catch {
            pendingActions.fail(key, mutationErrorMessage(error))
        }
    }
}
