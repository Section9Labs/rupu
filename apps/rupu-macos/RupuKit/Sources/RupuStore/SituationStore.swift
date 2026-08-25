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
/// ~10 lines of arithmetic" rather than "import a whole module" (that
/// arithmetic itself now lives in `SituationSelection.swift`'s
/// `sparkTick(current:eventsInWindow:windowMS:)`, extracted for testability
/// — review fix round 1, ruling 5).
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
/// **Reconnect-replay dedup, and why a reconnect no longer truncates the
/// wall** (review fix round 1, ruling 1 — HIGH): the CP's SSE stream
/// replays an active run's `events.jsonl` from offset 0 on every
/// (re)connect (see `RupuSituation/StreamCards.swift`'s file header for the
/// citation this store can't literally cross-reference via `import`). The
/// first pass at this store's `resnapshot` handler (`loadHistory`) called
/// `eventRows = rows` — a wholesale REPLACE with the fresh 200-row backfill
/// page — on every reconnect, which silently collapsed up to `maxEventRows`
/// (5,000) accumulated rows down to 200 every time the SSE connection
/// blipped. `loadHistory` now MERGES the fresh backfill page into
/// `eventRows` instead (`mergeIncoming(_:)`): it recovers the offline gap
/// (rows this store never saw while disconnected — strictly better than the
/// web, which has no equivalent re-backfill on reconnect at all) while
/// keeping every row already accumulated.
///
/// **Dedup, incrementally** (review fix round 1, ruling 2 — HIGH,
/// perf): `seenEventKeys: Set<CPEvent>` is the "already rendered this exact
/// content" ledger both `mergeIncoming(_:)` and `applyLive(_:)` consult and
/// maintain — an O(1) membership check/insert per row instead of the first
/// pass's O(n) `eventRows.contains(where:)` linear scan (which, at up to
/// 5,000 rows, made every single live event cost a full backlog scan on the
/// main actor). `CPEvent`'s `Hashable` conformance (`RupuAPI/CPEvent.swift`)
/// makes the set member exactly the "everything except the server-injected
/// `ts`/`pos`" content identity `RupuSituation`'s `contentIdentityKey`/the
/// web's `identityOf` use — the same shape as the web's `seenRef`, without
/// this module needing to import `RupuSituation` to get there. A row
/// trimmed off either end when `maxEventRows` is exceeded has its key
/// removed from `seenEventKeys` too, so the set's size tracks `eventRows`'s
/// 1:1, not the whole session's history.
///
/// **`newestTSByRun` is maintained incrementally, not rebuilt per call**
/// (also ruling 2): every insert (`mergeIncoming`/`applyLive`) updates the
/// per-run high-water mark directly (`max(existing, new)`) rather than
/// `resolveRuns` re-scanning all of `eventRows` on every call. A stale
/// entry can linger for a run whose every row has since been trimmed off
/// the backlog — accepted: the worst case is one wasted `GET /api/runs/:id`
/// call for a run that's no longer displayed, not a correctness issue.
///
/// **`resolveRuns` is throttled to the aggregate poll tick, not called per
/// live event** (also ruling 2): the first pass called it from
/// `applyLive(_:)` too, meaning every single live event paid for a full
/// budgeted-selection pass. It now runs only from `pollOnce(generation:)` —
/// once immediately inside `activate()` (so the very first render already
/// has whatever resolution a single pass can give it) and again every 60s
/// poll tick thereafter. A reconnect that merges in rows for a brand-new
/// run has to wait for the next poll tick (≤60s) before that run's
/// workspace label resolves — an acceptable latency trade for an already
/// low-priority, cosmetic (not correctness-affecting) concern on a screen
/// whose aggregate poll is deliberately slow-cadence to begin with.
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
/// live stream never outlives the Situation Room window. `activate()`
/// re-checks `generation == self.generation` after EACH of its two `await`
/// points (review fix round 1, ruling 9) — without that, a window closed
/// (and `deactivate()` called) WHILE `activate()`'s `loadHistory`/
/// `pollOnce` call was still in flight would resume and unconditionally
/// start a brand-new stream/poll/tick loop on a store the caller had
/// already torn down, leaking a live connection nothing will ever stop.
@MainActor
@Observable
public final class SituationStore {
    /// History backfill + live-tail fold, newest-first (ties broken by
    /// Swift's stable `sort(by:)`, which preserves each equal-`ts` group's
    /// existing relative order — see `mergeIncoming(_:)`).
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
    /// web's `SPARK_TICK_MS`) via `sparkTick(current:eventsInWindow:
    /// windowMS:)` (`SituationSelection.swift`).
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

    /// Content-identity dedup ledger — see the type's doc comment ("Dedup,
    /// incrementally"). One entry per row currently represented in
    /// `eventRows`; kept in exact 1:1 sync with it (inserted alongside a
    /// row, removed alongside its trim).
    private var seenEventKeys: Set<CPEvent> = []
    /// Per-run high-water mark, maintained incrementally on every insert —
    /// see the type's doc comment ("`newestTSByRun` is maintained
    /// incrementally").
    private var newestTSByRun: [String: Int64] = [:]

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
    private static let resolveBudget = 12 // Events.tsx's per-pass `budget` (lines 205, 212, 215)
    private static let tickInterval: Duration = .seconds(5) // Events.tsx SPARK_TICK_MS (line 35)
    private static let tickIntervalMS: Double = 5_000
    fileprivate static let sparkLength = 16 // Events.tsx SPARK_LEN (line 36)
    private static let terminalStatuses: Set<String> = ["completed", "failed", "cancelled", "rejected"]
    /// `dashboard(range:host:)`'s range — review fix round 1, ruling 10:
    /// this store only ever reads `.active.running`/`.active.awaitingApproval`
    /// off the response (see `pollOnce`), both range-INDEPENDENT counts; the
    /// first pass requested `.all`, which makes the server compute (and
    /// serialize) the full terminal/throughput bucket grid on every 60s poll
    /// for fields this store never looks at. `.d7` is the smallest range
    /// this app's `TimeRange` vocabulary offers, same cost-minimizing intent.
    private static let dashboardRange = TimeRange.d7.rawValue

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
    /// while this is in flight), one immediate aggregate poll (which also
    /// runs the first `resolveRuns` pass — see the type's doc comment), then
    /// starts the live tail, the 60s aggregate poll loop, and the 5s tick
    /// sampler. Every call — not just the first — rebuilds the three loops
    /// from scratch; `runToWorkspace`/`runTerminalStatus`/`seenEventKeys`/
    /// `newestTSByRun` are deliberately NOT cleared (a re-activation of the
    /// same store instance, e.g. the Situation Room window closing and
    /// reopening with no backend swap in between, has no reason to forget
    /// already-accumulated state).
    public func activate() async {
        generation += 1
        let generation = generation
        eventsSinceTick = 0
        spark = Array(repeating: 0, count: Self.sparkLength)
        eventsPerMin = 0

        await loadHistory(generation: generation)
        guard generation == self.generation else { return } // ruling 9: torn down mid-await
        restartStream()
        await pollOnce(generation: generation)
        guard generation == self.generation else { return } // ruling 9: torn down mid-await
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
        mergeIncoming(rows)
    }

    /// Merges a freshly-fetched backfill page into `eventRows` — see the
    /// type's doc comment ("why a reconnect no longer truncates the wall").
    /// `rows` is itself newest-first (the server's own order); only rows
    /// this store hasn't already seen (`seenEventKeys`) are actually new
    /// information — an unconditional merge-then-resort on every reconnect
    /// would otherwise re-touch (and potentially reorder, on a tie) rows
    /// already in place for no reason. A no-op (no resort, no `eventRows`
    /// reassignment) when every row in `rows` is already known.
    private func mergeIncoming(_ rows: [CPEventRow]) {
        var merged = eventRows
        var didChange = false
        for row in rows where !seenEventKeys.contains(row.event) {
            seenEventKeys.insert(row.event)
            merged.append(row)
            if let runID = row.event.runID {
                newestTSByRun[runID] = max(newestTSByRun[runID] ?? Int64.min, row.ts)
            }
            didChange = true
        }
        guard didChange else { return }

        merged.sort { $0.ts > $1.ts } // stable — preserves each tied group's existing relative order
        trimAndAssign(merged)
    }

    /// Caps `rows` at `maxEventRows`, dropping the OLDEST (tail, since
    /// callers keep the array newest-first) overflow and removing each
    /// dropped row's identity from `seenEventKeys` so the set never
    /// outgrows what's actually displayed.
    private func trimAndAssign(_ rows: [CPEventRow]) {
        var rows = rows
        if rows.count > maxEventRows {
            let overflow = rows.count - maxEventRows
            for dropped in rows.suffix(overflow) {
                seenEventKeys.remove(dropped.event)
            }
            rows.removeLast(overflow)
        }
        eventRows = rows
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
        // Reconnect-replay dedup — O(1) via `seenEventKeys`, see the type's
        // doc comment ("Dedup, incrementally").
        guard !seenEventKeys.contains(event) else { return }
        seenEventKeys.insert(event)

        let tsMS = Int64((Date().timeIntervalSince1970 * 1000).rounded())
        if let runID = event.runID {
            newestTSByRun[runID] = max(newestTSByRun[runID] ?? Int64.min, tsMS)
        }
        // `pos: -1` is a sentinel: `CPEventRow.pos` only matters for the
        // web's "load older" pagination cursor, a feature this fullscreen
        // ambient wall deliberately doesn't implement (see
        // `SituationRoomScreen`'s doc comment) — nothing here ever reads it.
        var rows = eventRows
        rows.insert(CPEventRow(event: event, ts: tsMS, pos: -1), at: 0)
        trimAndAssign(rows)
        eventsSinceTick += 1

        // `resolveRuns` is NOT called here — throttled to the aggregate
        // poll tick (see the type's doc comment).

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
        async let dashboardResult = try? client.dashboard(range: Self.dashboardRange, host: nil)
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
        // Every poll tick is also `resolveRuns`'s own cadence now (ruling 2)
        // — a stale-looking run gets re-checked here even with no new
        // events arriving to trigger it another way, same rationale
        // Events.tsx's own `dashboard` poll-tick dependency documents
        // (line 224's comment).
        resolveRuns(generation: generation)
    }

    // MARK: - Lazy run → workspace + terminal-status resolution

    /// Port of Events.tsx lines 189-224's `getRun` effect: two budgeted
    /// passes over `newestTSByRun` (maintained incrementally — see the
    /// type's doc comment), sharing one 12-slot budget, each spent
    /// newest-first via `selectNewestFirst` (`SituationSelection.swift`,
    /// ruling 4/5) — first every run that's never been resolved or
    /// requested, then every run whose newest event has gone quiet for
    /// `staleRunInterval` and hasn't been rechecked within
    /// `staleRecheckInterval` (closing the "the event log ended mid-step but
    /// the run never really told us it was over" gap). Every fetch is
    /// fire-and-forget, not `Task`-tracked for explicit cancellation
    /// (unlike `ActivityStore.remoteHostTasks`) — the `generation` guard
    /// inside each fetch's continuation already makes a stray in-flight
    /// call from a torn-down cycle a harmless no-op, and this store's
    /// fetches are cheap enough (`GET /api/runs/:id`, budgeted at 12, and
    /// only run once per 60s poll tick — see the type's doc comment) that
    /// the extra bookkeeping isn't worth it here.
    private func resolveRuns(generation: Int) {
        let now = Date()

        let unresolved = selectNewestFirst(candidates: newestTSByRun, budget: Self.resolveBudget) { runID in
            runToWorkspace[runID] == nil && !requestedRuns.contains(runID)
        }
        for runID in unresolved {
            requestedRuns.insert(runID)
            statusCheckedAt[runID] = now
        }

        let remainingBudget = Self.resolveBudget - unresolved.count
        let stale = selectNewestFirst(candidates: newestTSByRun, budget: remainingBudget) { runID in
            guard runTerminalStatus[runID] == nil else { return false } // already known-terminal
            guard let ts = newestTSByRun[runID] else { return false }
            let eventAge = now.timeIntervalSince(Date(timeIntervalSince1970: Double(ts) / 1000))
            guard eventAge >= Self.staleRunInterval else { return false } // still chatty
            let checked = statusCheckedAt[runID] ?? .distantPast
            return now.timeIntervalSince(checked) >= Self.staleRecheckInterval
        }
        for runID in stale {
            statusCheckedAt[runID] = now
        }

        for runID in unresolved + stale {
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

    private func tick() {
        let n = eventsSinceTick
        eventsSinceTick = 0
        let result = sparkTick(current: spark, eventsInWindow: n, windowMS: Self.tickIntervalMS)
        spark = result.spark
        eventsPerMin = result.eventsPerMin
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
