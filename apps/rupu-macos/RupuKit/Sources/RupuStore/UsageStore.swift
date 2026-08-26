import Foundation
import Observation
import RupuAPI
import RupuUsageKit

/// Owns the Usage screen's (Phase 5B, Task 6) three independent blocks —
/// `usage` (fleet-wide summary + server-grouped breakdown, `GET /api/usage`),
/// `usageRuns` (the flat per-`(run × model)` rows that feed the client-side
/// spend chart via `RupuUsageKit.buildSpendTimeline`, `GET /api/usage/runs`),
/// and `outliers` (`GET /api/usage/outliers`) — plus the active `pivot`
/// (`RupuUsageKit.UsagePivot`), which only `usage`'s own fetch depends on
/// (its `group_by` query param; `usageRuns`/`outliers` are pivot-independent
/// flat rows, pivoted client-side on demand via `RupuUsageKit.aggregateRows`).
/// (`RupuUsageKit` was `RupuUsage` in Task 5 — renamed in Task 6 so the
/// screen module could take the screen-convention name; see
/// `UsageAggregation.swift`'s file-header doc comment for the full
/// rationale.)
///
/// **Superseded ruling (perf & interaction arc, Plan 5 Task 2): local-first,
/// remote-progressive, same shape as `ActivityStore`/`DashboardStore`.**
/// The prior "CONTROLLER RULING (single server-side fan-out fetch)" this
/// doc comment used to carry relied on the server's own no-`host` fan-out
/// for `GET /api/usage` — but that fan-out is sequential-enough server-side
/// (`join_all` still means the ONE JSON response waits on the SLOWEST
/// registered host) to turn one offline Fleet node into the exact
/// multi-second stall `ActivityStore`'s per-host progressive loading exists
/// to avoid (measured 2.5-4.0s against a fleet with one offline host vs
/// 60-70ms local-only — see `CPClient.runs(offset:limit:host:)`'s doc
/// comment). `loadUsageLocalFirst(generation:now:)` now issues its own
/// `host="local"`-scoped call first (this is what `activate(range:)`/
/// `setRange(_:)`/`setPivot(_:)` await before returning — `usage` already
/// shows local truth by then), then discovers the fleet (`GET /api/hosts`)
/// and fires every other `status == "online"` host's own `?host=`-scoped
/// fetch as an independent background task, merging each in as it lands via
/// `mergeUsage(_:groupBy:)` — a client-side port of the exact aggregation
/// `crates/rupu-cp/src/api/usage.rs`'s `get_usage` handler already performs
/// server-side (`rollup`/`merge_breakdown_rows`/`merge_unpriced`), so the
/// eventual fully-merged result is byte-for-byte the same set the old
/// no-`host` call would have produced — only its arrival order/latency
/// differs. A local failure still surfaces as `.failed` immediately (no
/// remote host is worth trying if the operator's own machine can't even
/// answer); a REMOTE host's own failure (thrown fetch, or a 200 response
/// whose OWN `hosts[]` entry already reports `"offline"`/`"unavailable"` —
/// the server already zeroes that host's numeric contribution before
/// returning, so folding its response into the merge is always safe) never
/// blanks `usage` — it just adds an `"unavailable"`/passed-through
/// `APIHostFreshness` entry to the merged `hosts` array, same "one host's
/// trouble is never this store's trouble" contract `ActivityStore`/
/// `DashboardStore` already establish. `pendingHosts` mirrors
/// `ActivityStore.pendingHosts` — a "+N hosts loading…" signal, never part
/// of `usage`'s own `BlockState`.
///
/// Because the three blocks are still independent `BlockState`s, a slow or
/// offline host stalling `usage`'s own merge can never blank `usageRuns`/
/// `outliers` (both local-only, no fan-out at all, unchanged by this task) —
/// they resolve on their own schedule regardless of how long `usage`'s
/// remote hosts take.
///
/// **NO reconcile loop** — deliberate parity with the web: the `/usage`
/// page only refetches on an explicit window (or pivot) change, never on a
/// timer or a firehose signal (unlike `DashboardStore`'s 60s reconcile +
/// debounced local refresh). A stale usage number sits until the operator
/// changes the range/pivot or revisits the screen.
///
/// **Generation-guarded refetch** — same idiom `DashboardStore`/`FleetStore`
/// already establish, split across TWO independent counters (review fix,
/// round 1 — Important) so a partial refresh can never strand an unrelated
/// block at `.loading` forever (the PR #501 spins-forever class):
/// - `generation` guards `usageRuns`/`outliers` — bumped only by
///   `activate(range:)`/`setRange(_:)`/`deactivate()` (a genuine "everything
///   changed" cycle).
/// - `usageGeneration` guards `usage` (and its own per-host progressive
///   merge state — `okUsageResponses`/`failedHostFreshness`/`pendingHosts`/
///   `usageHostTasks`, all cleared and reset together at the top of every
///   cycle that bumps it) alone — bumped by `activate(range:)`/
///   `setRange(_:)`/`deactivate()` (kept in lockstep with `generation` for
///   those) AND ADDITIONALLY by `setPivot(_:)`, which must invalidate a
///   stale in-flight `usage` fetch (the one dispatched under the OLD pivot)
///   without touching `generation` at all — otherwise a pivot change landing
///   while a `setRange`'s `usageRuns`/`outliers` fetches are still in flight
///   would bump the counter THEY'RE guarded by, and since `setPivot` never
///   re-dispatches them, their late-arriving real results would fail the
///   (now-stale) guard and get silently dropped, stranding both blocks at
///   `.loading` with nothing left to ever resolve them — same
///   "refreshLocalOnly must not touch a generation it doesn't own"
///   principle `DashboardStore.refreshLocalOnly`'s doc comment already
///   documents, applied here as two counters instead of one shared one (a
///   single un-bumped shared counter would instead let a stale OLD-pivot
///   `usage` fetch and a fresh NEW-pivot one race under the same
///   generation, with whichever the network returns last winning — wrong
///   content instead of a stranding, but still wrong). `setPivot(_:)` also
///   cancels any in-flight remote-host usage tasks and clears the per-host
///   merge state before re-dispatching `local` under the new pivot — a
///   remote host's response grouped under the OLD pivot's `group_by` can
///   never be folded into a merge keyed by the new one.
///
/// Each dispatched fetch captures its relevant generation at dispatch and
/// applies its result only if that generation is still current when it
/// resolves.
@MainActor
@Observable
public final class UsageStore {
    public private(set) var usage: BlockState<APIUsageResponse> = .loading
    public private(set) var usageRuns: BlockState<[APIUsageRunRow]> = .loading
    public private(set) var outliers: BlockState<[APIOutlierRun]> = .loading
    public private(set) var pivot: UsagePivot = .model

    /// Count of hosts (fleet nodes other than `local`) whose usage fetch for
    /// the currently in-flight cycle hasn't answered yet — same "+N hosts
    /// loading…" signal `ActivityStore.pendingHosts` exposes. `0` at rest
    /// (nothing pending, or the fleet has no online remote hosts at all).
    public private(set) var pendingHosts: Int = 0

    private let fetchUsage: @Sendable (_ since: String?, _ until: String?, _ groupBy: String?, _ host: String?) async throws -> APIUsageResponse
    private let fetchHosts: @Sendable () async throws -> [APIHostRow]
    private let fetchUsageRuns: @Sendable (_ since: String?, _ until: String?) async throws -> [APIUsageRunRow]
    private let fetchOutliers: @Sendable (_ since: String?, _ until: String?) async throws -> [APIOutlierRun]

    private var range: TimeRange = .d30

    /// See the type doc comment's "Generation-guarded refetch" section —
    /// guards `usageRuns`/`outliers` only.
    private var generation = 0
    /// See the type doc comment's "Generation-guarded refetch" section —
    /// guards `usage` (and its per-host progressive-merge state) alone;
    /// bumped independently by `setPivot(_:)`.
    private var usageGeneration = 0

    /// Every host that has produced a usable `APIUsageResponse` in the
    /// current `usageGeneration` cycle — `local`, always fetched (and
    /// merged) first, then every other online host progressively.
    /// `recomputeUsage()` re-merges `Array(okUsageResponses.values)` every
    /// time one lands, so `usage` grows the same way `ActivityStore.rows`/
    /// `DashboardStore.merged` grow, rather than jumping once at the end.
    private var okUsageResponses: [String: APIUsageResponse] = [:]
    /// Synthesized freshness for a REMOTE host whose own `fetchUsage` call
    /// threw (transport/decoding/non-2xx) — there is no server-reported
    /// `APIHostFreshness` to read one off in that case (the response body
    /// never existed), unlike a host that answered 200 with its own
    /// `hosts[].state == "offline"`/`"unavailable"` (that response still
    /// lands in `okUsageResponses` — the server already zeroes its numeric
    /// contribution before returning, so merging it in is always safe, and
    /// its own `hosts[]` entry already carries the right state). Cleared for
    /// a host id the moment that host answers successfully again.
    private var failedHostFreshness: [String: APIHostFreshness] = [:]
    /// Every in-flight remote-host `Task` from the current `usageGeneration`
    /// cycle (the discovery task plus one per online host), so a new cycle
    /// (`setPivot`/`setRange`/`activate`/`deactivate`) can cancel them
    /// outright rather than letting a now-superseded fetch land later.
    private var usageHostTasks: [Task<Void, Never>] = []

    private static let localHost = "local"

    /// Production entry point — the Usage screen calls this.
    public convenience init(client: CPClient) {
        self.init(
            fetchUsage: { since, until, groupBy, host in
                try await client.usage(since: since, until: until, groupBy: groupBy, host: host)
            },
            fetchHosts: { try await client.hosts() },
            fetchUsageRuns: { since, until in try await client.usageRuns(since: since, until: until) },
            fetchOutliers: { since, until in try await client.usageOutliers(since: since, until: until) }
        )
    }

    /// Designated init — plain fetch closures, same "fake client closures"
    /// seam every other store in this module already established. `internal`,
    /// not `public` — reached from tests via `@testable import RupuStore`.
    /// `fetchHosts` defaults to "no fleet" (`[]`) so every pre-existing test
    /// that never cared about remote hosts keeps working unchanged — `local`
    /// alone still fully populates `usage`.
    init(
        fetchUsage: @escaping @Sendable (_ since: String?, _ until: String?, _ groupBy: String?, _ host: String?) async throws -> APIUsageResponse,
        fetchHosts: @escaping @Sendable () async throws -> [APIHostRow] = { [] },
        fetchUsageRuns: @escaping @Sendable (_ since: String?, _ until: String?) async throws -> [APIUsageRunRow],
        fetchOutliers: @escaping @Sendable (_ since: String?, _ until: String?) async throws -> [APIOutlierRun]
    ) {
        self.fetchUsage = fetchUsage
        self.fetchHosts = fetchHosts
        self.fetchUsageRuns = fetchUsageRuns
        self.fetchOutliers = fetchOutliers
    }

    // MARK: - Window

    /// `TimeRange` -> concrete `[since, until]` `Date` bounds ending at the
    /// passed `now` — exact port of the web's `presetWindow`/
    /// `usageRangeSince` (`lib/api.ts`). `.all` is `[epoch, now]`, NOT an
    /// omitted `since` — the server defaults an ABSENT `since` to "last 30
    /// days" (`resolve_window`/`resolve_since` in the Rust handlers), which
    /// would silently narrow `.all` down to 30 days instead of broadening
    /// it; the web source's own doc comment on `usageRangeSince` says this
    /// explicitly, and this ports it exactly.
    ///
    /// **`now` has a default only for caller convenience (tests, one-off
    /// callers) — it is NOT memoized or shared across calls.** Each call
    /// with the default evaluates `Date()` independently, so two calls
    /// milliseconds apart (e.g. this store's OWN three concurrent
    /// `async let` fetches, before this fix) would each get a very
    /// slightly different `until`. `refetchAll()`/`setPivot(_:)` below
    /// avoid that by capturing ONE `Date()` per dispatch cycle and passing
    /// it explicitly to every loader that cycle dispatches, so every fetch
    /// in the same cycle shares bit-identical bounds — pass `now` yourself
    /// for that same guarantee outside this store (e.g. Task 6 computing
    /// the chart's fill window to match a specific `usageRuns` fetch).
    public nonisolated static func windowBounds(for range: TimeRange, now: Date = Date()) -> (since: Date, until: Date) {
        switch range {
        case .d7: return (now.addingTimeInterval(-7 * 86_400), now)
        case .d30: return (now.addingTimeInterval(-30 * 86_400), now)
        case .all: return (Date(timeIntervalSince1970: 0), now)
        }
    }

    /// RFC-3339 with fractional seconds, matching the wire format every
    /// other `startedAt`/`capturedAt` field in this API already uses.
    /// Formats via `RupuAPI.ISO8601Parsing.fractional` — the same shared,
    /// `static let`-cached `Date.ISO8601FormatStyle` the parsing direction
    /// (`ActivityRow.parseISO`, etc.) uses, since `ISO8601FormatStyle` is a
    /// `FormatStyle` as well as a `ParseStrategy`.
    private static func rfc3339(_ date: Date) -> String {
        ISO8601Parsing.fractional.format(date)
    }

    // MARK: - Activation

    /// Seeds `range` and runs the full three-block fetch cycle. Safe to call
    /// more than once — every call fully re-dispatches all three. Returns
    /// once `usage`'s own `"local"` fetch has landed (see
    /// `loadUsageLocalFirst(generation:now:)`'s doc comment) — the screen
    /// never blocks on a remote host at all.
    public func activate(range: TimeRange) async {
        self.range = range
        await refetchAll()
    }

    /// Sets the range and refetches all three blocks from scratch — same
    /// full cycle `activate(range:)` runs.
    public func setRange(_ newRange: TimeRange) async {
        self.range = newRange
        await refetchAll()
    }

    /// Sets the active pivot and refetches ONLY `usage` (the one block whose
    /// fetch depends on it, via `group_by`) — `usageRuns`/`outliers`, and
    /// the shared `generation` guarding them, are left exactly as they are
    /// (review fix, round 1 — Important; see the type doc comment's
    /// "Generation-guarded refetch" section for why touching `generation`
    /// here would strand them at `.loading` if either had a fetch still in
    /// flight from a concurrent `activate`/`setRange`). Bumps
    /// `usageGeneration` instead, cancels any in-flight remote-host usage
    /// tasks, and clears the per-host merge state (a remote response grouped
    /// under the OLD pivot can never be folded into the new one's merge) —
    /// so a `usage` fetch still in flight from before this call (a rapid
    /// pivot double-click, or one dispatched by an in-flight `activate`/
    /// `setRange` under the OLD pivot) is dropped rather than racing this
    /// one to apply last.
    public func setPivot(_ newPivot: UsagePivot) async {
        pivot = newPivot
        usageGeneration += 1
        cancelUsageHostTasks()
        okUsageResponses.removeAll()
        failedHostFreshness.removeAll()
        pendingHosts = 0
        await loadUsageLocalFirst(generation: usageGeneration, now: Date())
    }

    /// Invalidates any in-flight fetch from the current cycle (via both
    /// generation bumps) and cancels any in-flight remote-host usage tasks —
    /// there is no timer/subscription to tear down (see the type doc
    /// comment's "NO reconcile loop"), so this is otherwise a no-op.
    /// Idempotent, safe before `activate(range:)` ever ran.
    public func deactivate() {
        generation += 1
        usageGeneration += 1
        cancelUsageHostTasks()
    }

    // MARK: - Fetch cycle

    private func refetchAll() async {
        generation += 1
        usageGeneration += 1
        let currentGeneration = generation
        let currentUsageGeneration = usageGeneration
        cancelUsageHostTasks()
        okUsageResponses.removeAll()
        failedHostFreshness.removeAll()
        pendingHosts = 0
        // One `now` for the whole cycle (see `windowBounds(for:now:)`'s doc
        // comment) — every loader below shares bit-identical `[since,
        // until]` bounds rather than each capturing its own `Date()`
        // moments apart.
        let now = Date()
        async let usageLoad: Void = loadUsageLocalFirst(generation: currentUsageGeneration, now: now)
        async let runsLoad: Void = loadUsageRuns(generation: currentGeneration, now: now)
        async let outliersLoad: Void = loadOutliers(generation: currentGeneration, now: now)
        _ = await (usageLoad, runsLoad, outliersLoad)
    }

    // MARK: - Usage (local-first, remote-progressive)

    /// Fetches `"local"` first — this is what every caller above awaits, so
    /// `usage` already shows local truth by the time `activate(range:)`/
    /// `setRange(_:)`/`setPivot(_:)` returns — then discovers the fleet
    /// (`GET /api/hosts`) and fires every other `status == "online"` host's
    /// own `?host=`-scoped fetch as an independent background task, off this
    /// call's critical path. See the type doc comment's "Superseded ruling"
    /// section for the full rationale.
    ///
    /// A `"local"` failure sets `usage = .failed(...)` directly and skips
    /// remote discovery entirely — there is no reason to fan out to the
    /// fleet if the operator's own machine can't even answer, and this
    /// keeps the failure path simple and immediate rather than racing a
    /// remote host's later success against an already-reported failure.
    private func loadUsageLocalFirst(generation: Int, now: Date) async {
        let bounds = Self.windowBounds(for: range, now: now)
        let since = Self.rfc3339(bounds.since)
        let until = Self.rfc3339(bounds.until)
        let groupBy = pivot.rawValue

        do {
            let response = try await fetchUsage(since, until, groupBy, Self.localHost)
            guard generation == usageGeneration else { return }
            okUsageResponses[Self.localHost] = response
            recomputeUsage()
        } catch {
            guard !isCancellation(error) else { return }
            guard generation == usageGeneration else { return }
            usage = .failed(mutationErrorMessage(error))
            return
        }

        loadRemoteUsageHosts(generation: generation, since: since, until: until, groupBy: groupBy)
    }

    /// Discovers the fleet (`GET /api/hosts`) and, for every `status ==
    /// "online"` host other than `local`, fires its own `?host=`-scoped
    /// usage fetch as an independent `Task` — one slow or erroring host can
    /// never delay or fail another's, same contract `ActivityStore.
    /// loadRemoteHosts`/`DashboardStore.refetchAll` already establish.
    /// `pendingHosts` is set to the online-remote count as soon as it's
    /// known (`0` if discovery itself fails or the fleet has no remote
    /// hosts).
    private func loadRemoteUsageHosts(generation: Int, since: String, until: String, groupBy: String) {
        let task = Task { [weak self] in
            guard let self else { return }
            let onlineRemoteHosts: [APIHostRow]
            do {
                let hosts = try await self.fetchHosts()
                onlineRemoteHosts = hosts.filter { $0.status == "online" && $0.id != Self.localHost }
            } catch {
                // Discovery itself failing must never fail `usage` — local
                // truth is already showing; just nothing more to add.
                onlineRemoteHosts = []
            }
            guard generation == self.usageGeneration else { return }
            self.pendingHosts = onlineRemoteHosts.count
            for host in onlineRemoteHosts {
                let hostTask = Task { [weak self] in
                    guard let self else { return }
                    await self.loadRemoteUsageHost(host, since: since, until: until, groupBy: groupBy, generation: generation)
                }
                self.usageHostTasks.append(hostTask)
            }
        }
        usageHostTasks.append(task)
    }

    /// Fetches one remote host's `?host=`-scoped usage and merges it in on
    /// success; on failure, synthesizes an `"unavailable"` `APIHostFreshness`
    /// entry for it instead (see `failedHostFreshness`'s doc comment) —
    /// either way `usage` keeps whatever it already had, never blanked by
    /// this host alone. Decrements `pendingHosts` and recomputes exactly
    /// once, whether this host contributed rows or not.
    private func loadRemoteUsageHost(_ host: APIHostRow, since: String, until: String, groupBy: String, generation: Int) async {
        do {
            let response = try await fetchUsage(since, until, groupBy, host.id)
            guard generation == usageGeneration else { return }
            okUsageResponses[host.id] = response
            failedHostFreshness.removeValue(forKey: host.id)
        } catch {
            guard !isCancellation(error) else { return }
            guard generation == usageGeneration else { return }
            failedHostFreshness[host.id] = APIHostFreshness(
                hostID: host.id, name: host.name, transportKind: host.transportKind,
                state: "unavailable", capturedAt: nil, reason: mutationErrorMessage(error)
            )
        }
        pendingHosts = max(0, pendingHosts - 1)
        recomputeUsage()
    }

    /// Re-merges every currently-`ok` host's response (`mergeUsage(_:groupBy:)`)
    /// and appends every currently-failed remote host's synthesized
    /// freshness entry to the result's `hosts` array, then publishes it as
    /// `usage`'s content. A no-op while `okUsageResponses` is still empty
    /// (nothing to show yet — `usage` stays whatever it already was, i.e.
    /// `.loading` on the very first call).
    private func recomputeUsage() {
        let responses = Array(okUsageResponses.values)
        guard !responses.isEmpty else { return }
        let merged = Self.mergeUsage(responses, groupBy: pivot.rawValue)
        let hosts = merged.hosts + Array(failedHostFreshness.values)
        usage = .content(
            APIUsageResponse(summary: merged.summary, breakdown: merged.breakdown, unpriced: merged.unpriced, hosts: hosts)
        )
    }

    private func cancelUsageHostTasks() {
        for task in usageHostTasks { task.cancel() }
        usageHostTasks.removeAll()
    }

    // MARK: - Merge (pure — ports `crates/rupu-cp/src/api/usage.rs`'s
    // `get_usage` aggregation: `rollup` (`crates/rupu-cp/src/usage.rs`),
    // `merge_breakdown_rows`, and `merge_unpriced`)

    /// Merges N already-`host=`-scoped `APIUsageResponse`s the exact way the
    /// server would have aggregated a full (no-`host`) fan-out — see
    /// `rollup(_:)`/`mergeBreakdownRows(_:groupBy:)`/`mergeUnpriced(_:)`'s
    /// own doc comments for the field-by-field semantics ported. `hosts` is
    /// a plain concatenation: each per-host-scoped response's own `hosts`
    /// array already carries exactly that host's freshness entry (mirrors
    /// the Rust source's `targets: vec![found]` single-host branch), so
    /// there is nothing left to merge there beyond appending.
    nonisolated static func mergeUsage(_ responses: [APIUsageResponse], groupBy: String) -> APIUsageResponse {
        APIUsageResponse(
            summary: rollup(responses.map(\.summary)),
            breakdown: mergeBreakdownRows(responses.flatMap(\.breakdown), groupBy: groupBy),
            unpriced: mergeUnpriced(responses.map(\.unpriced)),
            hosts: responses.flatMap(\.hosts)
        )
    }

    /// Ports `rupu_cp::usage::rollup` field-by-field: token counts and
    /// `runs` are plain sums; `costUSD` sums only the non-`nil`
    /// contributions and is `nil` iff EVERY contributing summary was itself
    /// unpriced (never a fabricated `0`); `priced` is `true` only if every
    /// contributing summary was.
    nonisolated static func rollup(_ summaries: [APIUsageSummary]) -> APIUsageSummary {
        var inputTokens: UInt64 = 0
        var outputTokens: UInt64 = 0
        var cachedTokens: UInt64 = 0
        var runs: UInt64 = 0
        var anyCost = false
        var costAcc: Double = 0
        var priced = true
        for s in summaries {
            inputTokens += s.inputTokens
            outputTokens += s.outputTokens
            cachedTokens += s.cachedTokens
            runs += s.runs
            if let cost = s.costUSD {
                anyCost = true
                costAcc += cost
            }
            if !s.priced { priced = false }
        }
        return APIUsageSummary(
            inputTokens: inputTokens,
            outputTokens: outputTokens,
            cachedTokens: cachedTokens,
            totalTokens: inputTokens + outputTokens,
            costUSD: anyCost ? costAcc : nil,
            priced: priced,
            runs: runs
        )
    }

    /// Ports `merge_unpriced`: the models set is a UNION across hosts
    /// (distinct, sorted for determinism — the Rust side folds into a
    /// `BTreeSet`); `rows` is a plain sum (each host's own rows are
    /// disjoint — its own runs).
    nonisolated static func mergeUnpriced(_ gaps: [APIUnpricedGap]) -> APIUnpricedGap {
        var models: Set<String> = []
        var rows: UInt64 = 0
        for gap in gaps {
            models.formUnion(gap.models)
            rows += gap.rows
        }
        return APIUnpricedGap(models: models.sorted(), rows: rows)
    }

    /// Ports `merge_breakdown_rows`: re-groups already-grouped rows from
    /// multiple hosts by the SAME key `group_by` selects server-side
    /// (needed because a remote host's response arrives pre-aggregated —
    /// its raw per-run rows never cross the wire), summing tokens/`runs`,
    /// summing `costUSD` only across non-`nil` contributions (never
    /// poisoning an already-summed value to `nil`), and AND-ing `priced`
    /// (one unpriced contributor makes the merged row unpriced). Sorted by
    /// `totalTokens` descending, then `model` ascending — exact tie-break
    /// order the Rust source uses. `order` preserves first-seen-key order
    /// for the (rare, tie-only) case the sort itself can't disambiguate,
    /// since iterating a Swift `Dictionary`'s values has no guaranteed
    /// order.
    nonisolated static func mergeBreakdownRows(_ rows: [APIUsageBreakdownRow], groupBy: String) -> [APIUsageBreakdownRow] {
        func key(_ row: APIUsageBreakdownRow) -> String {
            switch groupBy {
            case "provider": return row.provider
            case "model": return row.model
            case "agent": return row.agent
            case "workflow": return row.workflow
            case "host": return row.hostID
            case "project": return row.workspaceID
            default: return row.model
            }
        }

        var order: [String] = []
        var groups: [String: APIUsageBreakdownRow] = [:]
        for row in rows {
            let k = key(row)
            if let existing = groups[k] {
                let cost: Double?
                switch (existing.costUSD, row.costUSD) {
                case let (a?, b?): cost = a + b
                case let (a?, nil): cost = a
                case let (nil, b?): cost = b
                case (nil, nil): cost = nil
                }
                groups[k] = APIUsageBreakdownRow(
                    provider: existing.provider,
                    model: existing.model,
                    agent: existing.agent,
                    workflow: existing.workflow,
                    hostID: existing.hostID,
                    workspaceID: existing.workspaceID,
                    inputTokens: existing.inputTokens + row.inputTokens,
                    outputTokens: existing.outputTokens + row.outputTokens,
                    cachedTokens: existing.cachedTokens + row.cachedTokens,
                    totalTokens: existing.totalTokens + row.totalTokens,
                    costUSD: cost,
                    priced: existing.priced && row.priced,
                    runs: existing.runs + row.runs
                )
            } else {
                groups[k] = row
                order.append(k)
            }
        }
        return order.compactMap { groups[$0] }.sorted {
            if $0.totalTokens != $1.totalTokens { return $0.totalTokens > $1.totalTokens }
            return $0.model < $1.model
        }
    }

    // MARK: - usageRuns / outliers (local-only, unchanged)

    private func loadUsageRuns(generation: Int, now: Date) async {
        usageRuns = .loading
        let bounds = Self.windowBounds(for: range, now: now)
        do {
            let rows = try await fetchUsageRuns(Self.rfc3339(bounds.since), Self.rfc3339(bounds.until))
            guard generation == self.generation else { return }
            usageRuns = rows.isEmpty ? .empty : .content(rows)
        } catch {
            guard !isCancellation(error) else { return }
            guard generation == self.generation else { return }
            usageRuns = .failed(String(describing: error))
        }
    }

    private func loadOutliers(generation: Int, now: Date) async {
        outliers = .loading
        let bounds = Self.windowBounds(for: range, now: now)
        do {
            let rows = try await fetchOutliers(Self.rfc3339(bounds.since), Self.rfc3339(bounds.until))
            guard generation == self.generation else { return }
            outliers = rows.isEmpty ? .empty : .content(rows)
        } catch {
            guard !isCancellation(error) else { return }
            guard generation == self.generation else { return }
            outliers = .failed(String(describing: error))
        }
    }
}
