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
/// **CONTROLLER RULING (single server-side fan-out fetch)**: unlike
/// `DashboardStore`, there is no per-host client-side fan-out here — `GET
/// /api/usage[?host=]` omitted-`host` already fans out server-side across
/// every registered host (`APIUsageResponse.hosts` reports each host's own
/// reporting state, same `APIHostFreshness` shape Phase 4 already renders
/// via `FreshnessStrip`) and this store just fetches it once per cycle.
/// Because the three blocks are independent `BlockState`s, a slow or
/// offline host stalling `usage`'s own fan-out can never blank `usageRuns`/
/// `outliers` (both local-only, no fan-out at all) — they resolve on their
/// own schedule regardless of how long `usage` takes.
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
/// - `usageGeneration` guards `usage` alone — bumped by
///   `activate(range:)`/`setRange(_:)`/`deactivate()` (kept in lockstep with
///   `generation` for those) AND ADDITIONALLY by `setPivot(_:)`, which must
///   invalidate a stale in-flight `usage` fetch (the one dispatched under
///   the OLD pivot) without touching `generation` at all — otherwise a
///   pivot change landing while a `setRange`'s `usageRuns`/`outliers`
///   fetches are still in flight would bump the counter THEY'RE guarded by,
///   and since `setPivot` never re-dispatches them, their late-arriving
///   real results would fail the (now-stale) guard and get silently
///   dropped, stranding both blocks at `.loading` with nothing left to ever
///   resolve them — same "refreshLocalOnly must not touch a generation it
///   doesn't own" principle `DashboardStore.refreshLocalOnly`'s doc comment
///   already documents, applied here as two counters instead of one shared
///   one (a single un-bumped shared counter would instead let a stale
///   OLD-pivot `usage` fetch and a fresh NEW-pivot one race under the same
///   generation, with whichever the network returns last winning — wrong
///   content instead of a stranding, but still wrong).
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

    private let fetchUsage: @Sendable (_ since: String?, _ until: String?, _ groupBy: String?) async throws -> APIUsageResponse
    private let fetchUsageRuns: @Sendable (_ since: String?, _ until: String?) async throws -> [APIUsageRunRow]
    private let fetchOutliers: @Sendable (_ since: String?, _ until: String?) async throws -> [APIOutlierRun]

    private var range: TimeRange = .d30

    /// See the type doc comment's "Generation-guarded refetch" section —
    /// guards `usageRuns`/`outliers` only.
    private var generation = 0
    /// See the type doc comment's "Generation-guarded refetch" section —
    /// guards `usage` only; bumped independently by `setPivot(_:)`.
    private var usageGeneration = 0

    /// Production entry point — the Usage screen calls this.
    public convenience init(client: CPClient) {
        self.init(
            fetchUsage: { since, until, groupBy in try await client.usage(since: since, until: until, groupBy: groupBy) },
            fetchUsageRuns: { since, until in try await client.usageRuns(since: since, until: until) },
            fetchOutliers: { since, until in try await client.usageOutliers(since: since, until: until) }
        )
    }

    /// Designated init — plain fetch closures, same "fake client closures"
    /// seam every other store in this module already established. `internal`,
    /// not `public` — reached from tests via `@testable import RupuStore`.
    init(
        fetchUsage: @escaping @Sendable (_ since: String?, _ until: String?, _ groupBy: String?) async throws -> APIUsageResponse,
        fetchUsageRuns: @escaping @Sendable (_ since: String?, _ until: String?) async throws -> [APIUsageRunRow],
        fetchOutliers: @escaping @Sendable (_ since: String?, _ until: String?) async throws -> [APIOutlierRun]
    ) {
        self.fetchUsage = fetchUsage
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
    /// more than once — every call fully re-dispatches all three.
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
    /// `usageGeneration` instead, so a `usage` fetch still in flight from
    /// before this call (a rapid pivot double-click, or one dispatched by
    /// an in-flight `activate`/`setRange` under the OLD pivot) is dropped
    /// rather than racing this one to apply last.
    public func setPivot(_ newPivot: UsagePivot) async {
        pivot = newPivot
        usageGeneration += 1
        await loadUsage(generation: usageGeneration, now: Date())
    }

    /// Invalidates any in-flight fetch from the current cycle (via both
    /// generation bumps) — there is no timer/subscription to tear down (see
    /// the type doc comment's "NO reconcile loop"), so this is otherwise a
    /// no-op. Idempotent, safe before `activate(range:)` ever ran.
    public func deactivate() {
        generation += 1
        usageGeneration += 1
    }

    // MARK: - Fetch cycle

    private func refetchAll() async {
        generation += 1
        usageGeneration += 1
        let currentGeneration = generation
        let currentUsageGeneration = usageGeneration
        // One `now` for the whole cycle (see `windowBounds(for:now:)`'s doc
        // comment) — every loader below shares bit-identical `[since,
        // until]` bounds rather than each capturing its own `Date()`
        // moments apart.
        let now = Date()
        async let usageLoad: Void = loadUsage(generation: currentUsageGeneration, now: now)
        async let runsLoad: Void = loadUsageRuns(generation: currentGeneration, now: now)
        async let outliersLoad: Void = loadOutliers(generation: currentGeneration, now: now)
        _ = await (usageLoad, runsLoad, outliersLoad)
    }

    private func loadUsage(generation: Int, now: Date) async {
        usage = .loading
        let bounds = Self.windowBounds(for: range, now: now)
        do {
            let response = try await fetchUsage(Self.rfc3339(bounds.since), Self.rfc3339(bounds.until), pivot.rawValue)
            guard generation == self.usageGeneration else { return }
            usage = .content(response)
        } catch {
            guard !isCancellation(error) else { return }
            guard generation == self.usageGeneration else { return }
            usage = .failed(String(describing: error))
        }
    }

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
