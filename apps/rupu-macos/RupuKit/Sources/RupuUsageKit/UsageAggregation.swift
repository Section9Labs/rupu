import Foundation
import RupuAPI

// Port of `crates/rupu-cp/web/src/lib/usage/buildTimeline.ts`'s `buildTimeline`/
// `aggregateRuns` (day bucketing, pivot keys, unpriced handling) PLUS
// `crates/rupu-cp/src/api/usage.rs`'s server-side `build_timeline` (explicit
// gap-fill across a `[fill_start, fill_end]` window) — the macOS app's
// `CPClient` surface (Task 2) has no `GET /api/usage/timeline` method (only
// `usage`/`usageRuns`/`usageOutliers`), so `buildSpendTimeline` below IS the
// client-side substitute for that endpoint, built from the same flat
// `usageRuns` rows the web's OWN client-side `buildTimeline` consumes. Pure,
// no I/O, no `Date.now()` (both `now`-derived bounds are passed in) — same
// contract as the web source.
//
// **Module boundary note**: this target depends ONLY on `RupuAPI` (never
// `RupuStore`, which owns `Route.TimeRange`) so that `RupuStore`'s own
// `UsageStore.swift` can depend on this module without a cycle. Where the
// web's `presetWindow`/`buildTimeline(rows, pivot, filter, 'day')` take a
// `DashboardRange`/no window at all, the functions here take plain `Date`
// bounds instead — `UsageStore` (which DOES own `TimeRange`) computes those
// bounds via `UsageStore.windowBounds(for:now:)`, mirroring `presetWindow`
// exactly, and passes them in.
//
// **Renamed `RupuUsage` -> `RupuUsageKit` (Task 6, screen composition).**
// The Phase 5B plan named this target `RupuUsage`, matching the "one module
// per screen" convention `CLAUDE.md` documents for every other screen
// (`RupuSecurity`/`RupuFleet`/`RupuLibrary`/...). Task 6's Usage SCREEN
// needs that same name for the exact same reason every other screen module
// has it — but the screen also needs to depend on `RupuStore` (for
// `UsageStore`) AND this pure module (for `UsagePivot`/`PivotRow`/
// `SpendBucket`/`aggregateRows`/`buildSpendTimeline`), and `RupuStore`
// already depends on this pure module — so the screen target cannot ALSO be
// named `RupuUsage` without either colliding with this target's name or
// creating `RupuUsage -> RupuStore -> RupuUsage`. Renamed this pure module
// to `RupuUsageKit` (logic unchanged, identifier only) so the new
// `RupuUsage` screen target (`Sources/RupuUsage/UsageScreen.swift` etc.,
// depending on `RupuAPI`/`RupuStore`/`RupuDesign`/`RupuUsageKit`) can take
// the screen-convention name every other screen module already has, rather
// than permanently squatting on it with the pure logic module.

/// The six `/usage` pivot dimensions — mirrors the web's `Pivot` union
/// (`lib/api.ts`) exactly, same rawValues as the `group_by` query param
/// `CPClient.usage(since:until:groupBy:)` sends server-side.
public enum UsagePivot: String, CaseIterable, Sendable {
    case model, provider, agent, workflow, host, project
}

/// One grouped line of a pivot aggregation — mirrors `UsageBreakdownRow`
/// (`APIUsageBreakdownRow` in `RupuAPI`) exactly, including its "only the
/// field matching the active pivot is ever non-empty" convention (ported
/// from `toBreakdownRow` in `buildTimeline.ts` — see that function's doc
/// comment for why there is no `model`-mirroring exception for the other
/// five pivots).
public struct PivotRow: Equatable, Sendable {
    public let provider: String
    public let model: String
    public let agent: String
    public let workflow: String
    public let hostID: String
    public let workspaceID: String
    public let inputTokens: UInt64
    public let outputTokens: UInt64
    public let cachedTokens: UInt64
    public let totalTokens: UInt64
    /// `nil` only when NO contributing row was priced — mirrors
    /// `UsageSummary.cost_usd`'s documented meaning. Otherwise the sum of
    /// the priced contributions only, even when some rows in the group were
    /// unpriced (an unpriced row contributes 0 to this sum, never poisons
    /// it to `nil`).
    public let costUSD: Double?
    /// `false` whenever ANY contributor was unpriced, even though `costUSD`
    /// still carries a (partial) sum in that case.
    public let priced: Bool
    /// Count of DISTINCT contributing `run_id`s (not row count — a run can
    /// contribute more than one row, one per model it used).
    public let runs: Int

    public init(
        provider: String = "",
        model: String = "",
        agent: String = "",
        workflow: String = "",
        hostID: String = "",
        workspaceID: String = "",
        inputTokens: UInt64,
        outputTokens: UInt64,
        cachedTokens: UInt64,
        totalTokens: UInt64,
        costUSD: Double?,
        priced: Bool,
        runs: Int
    ) {
        self.provider = provider
        self.model = model
        self.agent = agent
        self.workflow = workflow
        self.hostID = hostID
        self.workspaceID = workspaceID
        self.inputTokens = inputTokens
        self.outputTokens = outputTokens
        self.cachedTokens = cachedTokens
        self.totalTokens = totalTokens
        self.costUSD = costUSD
        self.priced = priced
        self.runs = runs
    }
}

/// One day bucket of the client-built spend timeline: the UTC calendar-day
/// key (`YYYY-MM-DD`, matching `bucketKeyOf`'s `'day'` branch in
/// `buildTimeline.ts` and `bucket_key`'s `Granularity::Day` branch in the
/// Rust source — both are the UTC calendar day) plus every `APIUsageRunRow`
/// whose `startedAt` falls in it. Deliberately NOT pre-pivoted — unlike the
/// web's `UsageTimelineBucket` (which stacks by ONE active pivot at build
/// time), a `SpendBucket` carries its raw contributing rows so a caller can
/// call `aggregateRows(rows:pivot:)` on `rows` per-bucket for whichever
/// pivot is currently selected (or sum `rows` directly for a single total
/// series) without rebuilding the whole timeline on a pivot change.
public struct SpendBucket: Equatable, Sendable {
    public let day: String
    public let rows: [APIUsageRunRow]

    public init(day: String, rows: [APIUsageRunRow]) {
        self.day = day
        self.rows = rows
    }
}

// MARK: - Timestamp parsing

/// RFC-3339 parsing for `APIUsageRunRow.startedAt`, same two-formatter
/// fallback (fractional-then-plain seconds) as the canonical
/// `ActivityRow.parseISO` (`RupuStore`) — duplicated here rather than
/// imported because this module cannot depend on `RupuStore` (see the file
/// header's module-boundary note); `DashboardStore.parseTimestamp`'s own
/// doc comment already documents this exact duplication happening more than
/// once in this codebase.
private func parseUsageTimestamp(_ s: String) -> Date? {
    let fractional = ISO8601DateFormatter()
    fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    if let date = fractional.date(from: s) { return date }

    let plain = ISO8601DateFormatter()
    plain.formatOptions = [.withInternetDateTime]
    return plain.date(from: s)
}

private let utcCalendar: Calendar = {
    var cal = Calendar(identifier: .gregorian)
    cal.timeZone = TimeZone(identifier: "UTC")!
    return cal
}()

/// The UTC calendar-day key for `date` — `YYYY-MM-DD`, zero-padded. Exact
/// port of `ymd`/`bucketKeyOf`'s `'day'` branch in `buildTimeline.ts` (which
/// reads `getUTCFullYear`/`getUTCMonth`/`getUTCDate`) and `bucket_key`'s
/// `Granularity::Day` branch (`dt.date_naive()`, itself UTC since
/// `DateTime<Utc>`) in the Rust source — all three are the same UTC
/// calendar day, just computed via different date APIs.
func utcDayKey(_ date: Date) -> String {
    let c = utcCalendar.dateComponents([.year, .month, .day], from: date)
    return String(format: "%04d-%02d-%02d", c.year!, c.month!, c.day!)
}

private func utcStartOfDay(_ date: Date) -> Date {
    utcCalendar.startOfDay(for: date)
}

// MARK: - Pivot key extraction

/// The pivot-key value for one row, per the active pivot dimension. Exact
/// port of `pivotKeyOf` in `buildTimeline.ts`.
private func pivotKey(of row: APIUsageRunRow, pivot: UsagePivot) -> String {
    switch pivot {
    case .model: return row.model
    case .provider: return row.provider
    case .agent: return row.agent
    case .workflow: return row.workflowName
    case .host: return row.hostID
    case .project: return row.workspaceID
    }
}

/// Display label for a `PivotRow`, keyed by the active pivot dimension —
/// exact port of `pivotLabel` (`components/dashboard/modelColors.ts`),
/// including `modelLabel`'s `model || provider || agent || "—"` fallback
/// chain for the `.model` pivot specifically (every other pivot reads
/// exactly one field, falling back to `"—"` only when it's empty). For a
/// `PivotRow` produced BY `aggregateRows` itself, only the field matching
/// that row's own pivot is ever non-empty (see `PivotRow`'s doc comment),
/// so the `.model` fallback chain is inert on this module's own output —
/// it's still ported exactly, since `pivotLabel` on the web side is also
/// called on server-grouped `UsageBreakdownRow`s (`APIUsageResponse.
/// breakdown`) which don't carry that same guarantee.
public func pivotLabel(_ row: PivotRow, pivot: UsagePivot) -> String {
    switch pivot {
    case .model:
        let label = [row.model, row.provider, row.agent].first(where: { !$0.isEmpty })
        return label ?? "—"
    case .provider: return row.provider.isEmpty ? "—" : row.provider
    case .agent: return row.agent.isEmpty ? "—" : row.agent
    case .workflow: return row.workflow.isEmpty ? "—" : row.workflow
    case .host: return row.hostID.isEmpty ? "—" : row.hostID
    case .project: return row.workspaceID.isEmpty ? "—" : row.workspaceID
    }
}

// MARK: - Aggregation

private struct Agg {
    var inputTokens: UInt64 = 0
    var outputTokens: UInt64 = 0
    var cachedTokens: UInt64 = 0
    var totalTokens: UInt64 = 0
    /// Sum of non-`nil` `costUSD` contributions only.
    var costSum: Double = 0
    var sawPriced = false
    var sawUnpriced = false
    var runIDs = Set<String>()
}

private func accumulate(_ agg: inout Agg, _ row: APIUsageRunRow) {
    agg.inputTokens += row.inputTokens
    agg.outputTokens += row.outputTokens
    agg.cachedTokens += row.cachedTokens
    agg.totalTokens += row.totalTokens
    if let cost = row.costUSD {
        agg.sawPriced = true
        agg.costSum += cost
    } else {
        agg.sawUnpriced = true
    }
    agg.runIDs.insert(row.runID)
}

/// Builds one `PivotRow` for a pivot-key's aggregate — exact port of
/// `toBreakdownRow` in `buildTimeline.ts`, including its doc comment's
/// `cost_usd`/`priced` contract (see `PivotRow`'s own doc comment).
private func toPivotRow(pivot: UsagePivot, key: String, agg: Agg) -> PivotRow {
    var provider = "", model = "", agentField = "", workflow = "", hostID = "", workspaceID = ""
    switch pivot {
    case .model: model = key
    case .provider: provider = key
    case .agent: agentField = key
    case .workflow: workflow = key
    case .host: hostID = key
    case .project: workspaceID = key
    }
    return PivotRow(
        provider: provider,
        model: model,
        agent: agentField,
        workflow: workflow,
        hostID: hostID,
        workspaceID: workspaceID,
        inputTokens: agg.inputTokens,
        outputTokens: agg.outputTokens,
        cachedTokens: agg.cachedTokens,
        totalTokens: agg.totalTokens,
        costUSD: agg.sawPriced ? agg.costSum : nil,
        priced: agg.sawPriced && !agg.sawUnpriced,
        runs: agg.runIDs.count
    )
}

/// Groups every row by its pivot-key and sums — exact port of
/// `aggregateRuns` in `buildTimeline.ts`: date-independent (no bucketing),
/// no exclusion filter (every row contributes; this is what feeds a
/// breakdown table's own exclusion checkboxes, so filtering here would make
/// an excluded row's own checkbox permanently unreachable — same rationale
/// the web source documents). Output rows are sorted by key ascending, one
/// row per distinct pivot-key. Empty input -> empty output.
///
/// **Sort deviation**: the web sorts with `.localeCompare` (locale-aware);
/// this uses Swift's plain `String` `<` (Unicode canonical ordering, not
/// locale-aware). Every real pivot key today (model/provider/agent/
/// workflow/host ids, workspace ids) is ASCII, where the two orderings
/// agree — this would only diverge for a future non-ASCII pivot key, which
/// none of the six dimensions currently produce.
public func aggregateRows(rows: [APIUsageRunRow], pivot: UsagePivot) -> [PivotRow] {
    var byKey: [String: Agg] = [:]
    for row in rows {
        let key = pivotKey(of: row, pivot: pivot)
        var agg = byKey[key] ?? Agg()
        accumulate(&agg, row)
        byKey[key] = agg
    }
    return byKey.keys.sorted().map { toPivotRow(pivot: pivot, key: $0, agg: byKey[$0]!) }
}

/// Buckets `rows` by UTC calendar day and gap-fills every day of
/// `[fillStart, until]` inclusive — a composite port: the per-row
/// day-keying is `bucketKeyOf`'s `'day'` branch from the web's client-side
/// `buildTimeline` (`buildTimeline.ts`); the gap-fill-across-an-explicit-
/// window behavior is `timeline_fill_start`/`enumerate_bucket_keys`/
/// `build_timeline` from the Rust server (`crates/rupu-cp/src/api/
/// usage.rs`) — necessary because there is no client method for `GET
/// /api/usage/timeline` (Task 2's `CPClient` surface exposes only
/// `usage`/`usageRuns`/`usageOutliers`), so this function IS the
/// client-side substitute for that endpoint, built from the same flat
/// `usageRuns` rows the web's own `buildTimeline` consumes.
///
/// **`fillStart` (review fix, round 1 — Important)**: the Rust source's
/// `timeline_fill_start` clamps `since` up against the STORE-WIDE earliest
/// run — computed over EVERY run ever, before the `[since, until]` filter
/// — specifically so a bounded 7d/30d window still renders its FULL span
/// with leading zero-buckets whenever the project has any history at all
/// predating the window (`window_start.max(earliest_run)` resolves to
/// `window_start` in that ordinary case). This function only ever sees
/// `rows` already filtered to the window, so it has no store-wide earliest
/// run to clamp against — using the window-scoped earliest row instead (the
/// original implementation) would silently DROP the lead-in empty days
/// whenever the first activity inside the window isn't on day one, which is
/// wrong for the overwhelmingly common case of an established project.
/// Fixed by mirroring the value the Rust clamp actually RESOLVES TO for a
/// bounded window rather than reproducing the clamp itself:
/// - **Bounded window** (`since` after the epoch, i.e. 7d/30d): fill from
///   `since` UNCONDITIONALLY — the value `timeline_fill_start` resolves to
///   for any project with history predating the window.
/// - **Unbounded `.all` window** (`since == epoch`): keep the
///   `max(since, earliestSeen)` clamp against the WINDOW-scoped earliest
///   row — for `.all`, `usageRuns` was fetched with no lower bound at all,
///   so its window-scoped earliest row IS the store-wide earliest run,
///   exactly what the Rust clamp needs. Skipping this clamp would chart
///   potentially decades of empty days back to the epoch — same rationale
///   `timeline_fill_start`'s own doc comment gives.
///
/// A row with an unparseable `startedAt` is dropped rather than guessed —
/// same convention `RupuOverview.chartRows` already establishes for
/// unparseable server timestamps.
///
/// `rows` is expected to already be scoped to `[since, until]` (the same
/// bounds the caller fetched `GET /api/usage/runs` with) — a row outside
/// that range still contributes to its own day's bucket if that day happens
/// to fall within the generated range, but otherwise silently has no bucket
/// to land in. Empty `rows` (or every row unparseable) -> empty output,
/// mirroring the Rust source's "no runs at all -> empty series"
/// short-circuit — a divergence from the Rust source in the OTHER
/// direction worth naming: the Rust source's "no runs at all" check is also
/// store-wide (predates the window filter), so a bounded window with
/// genuinely zero activity but a non-empty project history would still
/// gap-fill server-side; this function can't distinguish that case from a
/// truly empty store without a store-wide fetch outside this surface, so it
/// returns empty for both.
public func buildSpendTimeline(rows: [APIUsageRunRow], since: Date, until: Date) -> [SpendBucket] {
    var byDay: [String: [APIUsageRunRow]] = [:]
    var earliest: Date?
    for row in rows {
        guard let started = parseUsageTimestamp(row.startedAt) else { continue }
        let key = utcDayKey(started)
        byDay[key, default: []].append(row)
        if earliest == nil || started < earliest! { earliest = started }
    }
    guard let earliestSeen = earliest else { return [] }

    let epoch = Date(timeIntervalSince1970: 0)
    let fillStart = since > epoch
        ? utcStartOfDay(since)
        : utcStartOfDay(max(since, earliestSeen))
    let fillEnd = utcStartOfDay(until)
    guard fillStart <= fillEnd else { return [] }

    var buckets: [SpendBucket] = []
    var cursor = fillStart
    while cursor <= fillEnd {
        let key = utcDayKey(cursor)
        buckets.append(SpendBucket(day: key, rows: byDay[key] ?? []))
        guard let next = utcCalendar.date(byAdding: .day, value: 1, to: cursor) else { break }
        cursor = next
    }
    return buckets
}
