import Foundation
import Observation
import RupuAPI
import RupuUsage

/// Owns the Usage screen's (Phase 5B, Task 6) three independent blocks —
/// `usage` (fleet-wide summary + server-grouped breakdown, `GET /api/usage`),
/// `usageRuns` (the flat per-`(run × model)` rows that feed the client-side
/// spend chart via `RupuUsage.buildSpendTimeline`, `GET /api/usage/runs`),
/// and `outliers` (`GET /api/usage/outliers`) — plus the active `pivot`
/// (`RupuUsage.UsagePivot`), which only `usage`'s own fetch depends on (its
/// `group_by` query param; `usageRuns`/`outliers` are pivot-independent flat
/// rows, pivoted client-side on demand via `RupuUsage.aggregateRows`).
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
/// already establish: `generation` is bumped by `activate(range:)`,
/// `setRange(_:)`, `setPivot(_:)`, and `deactivate()`; each dispatched fetch
/// captures the generation current at dispatch and applies its result only
/// if that generation is still current when it resolves, so a `setRange`
/// that lands while an older cycle's fetches are still in flight can never
/// have the stale results clobber the newer ones.
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

    /// See the type doc comment's "Generation-guarded refetch" section.
    private var generation = 0

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

    /// `TimeRange` -> concrete `[since, until]` `Date` bounds, ending `now`
    /// (or the passed `now`, for deterministic tests) — exact port of the
    /// web's `presetWindow`/`usageRangeSince` (`lib/api.ts`). `.all` is
    /// `[epoch, now]`, NOT an omitted `since` — the server defaults an
    /// ABSENT `since` to "last 30 days" (`resolve_window`/`resolve_since` in
    /// the Rust handlers), which would silently narrow `.all` down to 30
    /// days instead of broadening it; the web source's own doc comment on
    /// `usageRangeSince` says this explicitly, and this ports it exactly.
    ///
    /// Public so the Usage screen (Task 6) can compute the SAME bounds this
    /// store fetched `usageRuns` with before calling `RupuUsage.
    /// buildSpendTimeline(rows:since:until:)` over `usageRuns.value` — the
    /// chart's fill window must match the fetch window exactly, or its
    /// gap-fill would extend past (or fall short of) the data it actually has.
    public nonisolated static func windowBounds(for range: TimeRange, now: Date = Date()) -> (since: Date, until: Date) {
        switch range {
        case .d7: return (now.addingTimeInterval(-7 * 86_400), now)
        case .d30: return (now.addingTimeInterval(-30 * 86_400), now)
        case .all: return (Date(timeIntervalSince1970: 0), now)
        }
    }

    /// RFC-3339 with fractional seconds, matching the wire format every
    /// other `startedAt`/`capturedAt` field in this API already uses. Built
    /// fresh per call rather than cached — `ISO8601DateFormatter` isn't
    /// `Sendable`, same rationale `ActivityRow.parseISO`'s doc comment
    /// already documents for the parsing direction.
    private static func rfc3339(_ date: Date) -> String {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f.string(from: date)
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
    /// fetch depends on it, via `group_by`) — `usageRuns`/`outliers` are
    /// left exactly as they are, per the type doc comment. Still bumps the
    /// shared `generation` so a `usage` fetch still in flight from before
    /// this call (a rapid pivot double-click) is dropped rather than racing
    /// this one to apply last.
    public func setPivot(_ newPivot: UsagePivot) async {
        pivot = newPivot
        generation += 1
        await loadUsage(generation: generation)
    }

    /// Invalidates any in-flight fetch from the current cycle (via the
    /// generation bump) — there is no timer/subscription to tear down (see
    /// the type doc comment's "NO reconcile loop"), so this is otherwise a
    /// no-op. Idempotent, safe before `activate(range:)` ever ran.
    public func deactivate() {
        generation += 1
    }

    // MARK: - Fetch cycle

    private func refetchAll() async {
        generation += 1
        let currentGeneration = generation
        async let usageLoad: Void = loadUsage(generation: currentGeneration)
        async let runsLoad: Void = loadUsageRuns(generation: currentGeneration)
        async let outliersLoad: Void = loadOutliers(generation: currentGeneration)
        _ = await (usageLoad, runsLoad, outliersLoad)
    }

    private func loadUsage(generation: Int) async {
        usage = .loading
        let bounds = Self.windowBounds(for: range)
        do {
            let response = try await fetchUsage(Self.rfc3339(bounds.since), Self.rfc3339(bounds.until), pivot.rawValue)
            guard generation == self.generation else { return }
            usage = .content(response)
        } catch {
            guard !isCancellation(error) else { return }
            guard generation == self.generation else { return }
            usage = .failed(String(describing: error))
        }
    }

    private func loadUsageRuns(generation: Int) async {
        usageRuns = .loading
        let bounds = Self.windowBounds(for: range)
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

    private func loadOutliers(generation: Int) async {
        outliers = .loading
        let bounds = Self.windowBounds(for: range)
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
