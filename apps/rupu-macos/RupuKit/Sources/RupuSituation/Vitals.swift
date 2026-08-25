import Foundation
import RupuAPI

// Situation Room — pulse-strip vitals. Port of the tail of
// `crates/rupu-cp/web/src/lib/situationRoom/roster.ts` (lines 189-226:
// `Vitals`/`EMPTY_SUMMARY`/`buildVitals`), plus a small `eventsPerMin`
// ring-buffer sampler with no single web pure-function counterpart to port
// (see that section's own doc comment below for exactly what's original vs.
// ported).

/// Assembled pulse-strip vitals. Port of `roster.ts` lines 189-197's
/// `Vitals` interface. `findings` reuses `RupuAPI.APIFindingsSummary`
/// (`FindingsModels.swift`) rather than a parallel struct — it already
/// mirrors `FindingsSummary` field-for-field (`total`/`critical`/`high`/
/// `medium`/`low`/`info`).
public struct Vitals: Equatable, Sendable {
    public let activeRuns: Int
    public let projectsLive: Int
    public let projectsTotal: Int
    public let awaiting: Int
    public let errors: Int
    public let eventsPerMin: Int
    public let findings: APIFindingsSummary

    public init(
        activeRuns: Int, projectsLive: Int, projectsTotal: Int, awaiting: Int,
        errors: Int, eventsPerMin: Int, findings: APIFindingsSummary
    ) {
        self.activeRuns = activeRuns
        self.projectsLive = projectsLive
        self.projectsTotal = projectsTotal
        self.awaiting = awaiting
        self.errors = errors
        self.eventsPerMin = eventsPerMin
        self.findings = findings
    }
}

/// Port of `roster.ts` line 199's `EMPTY_SUMMARY`.
private let emptyFindingsSummary = APIFindingsSummary(total: 0, critical: 0, high: 0, medium: 0, low: 0, info: 0)

/// Assemble the pulse-strip vitals from real aggregates, degrading any
/// missing source to zero rather than fabricating a number. Port of
/// `roster.ts` lines 208-226's `buildVitals`.
public func buildVitals(
    activeRuns: Int?,
    awaiting: Int?,
    findings: APIFindingsSummary?,
    projectsLive: Int,
    projectsTotal: Int,
    errors: Int,
    eventsPerMin: Int
) -> Vitals {
    Vitals(
        activeRuns: activeRuns ?? 0,
        projectsLive: projectsLive,
        projectsTotal: projectsTotal,
        awaiting: awaiting ?? 0,
        errors: errors,
        eventsPerMin: eventsPerMin,
        findings: findings ?? emptyFindingsSummary
    )
}

// MARK: - events/min sampling

// `eventsPerMin` on the web is not a pure `situationRoom` lib function — it's
// computed inline by `Events.tsx`'s own `setInterval` sampler (lines 98,
// 106, 169-177): a `sparkCounterRef` tally is reset every `SPARK_TICK_MS`
// (5000ms) tick, pushed onto a fixed-length `spark` array (`SPARK_LEN`=16,
// line 173's `setSpark(prev => [...prev.slice(1), n])`), and converted to a
// per-minute rate via `Math.round((n * 60_000) / SPARK_TICK_MS)` (line 174).
// Per the task-6 brief ("events-per-minute over a session-local ring
// buffer"), that inline page logic is lifted into a small pure, testable
// unit here — `EventRateRing` is the `spark` ring buffer, `eventsPerMinute`
// is the exact line-174 arithmetic. Neither has a single web function to
// cite beyond the `Events.tsx` line numbers above; both are new pure Swift
// written to match that page's documented behavior, not a line-by-line port
// of an existing `situationRoom` lib function.

/// A fixed-length, newest-last ring of per-tick event counts — the `spark`
/// sparkline state (`Events.tsx` lines 98, 173).
public struct EventRateRing: Equatable, Sendable {
    public private(set) var samples: [Int]
    public let capacity: Int

    public init(capacity: Int = 16) {
        precondition(capacity > 0)
        self.capacity = capacity
        self.samples = Array(repeating: 0, count: capacity)
    }

    /// Push one tick's event count, dropping the oldest sample. Port of
    /// `Events.tsx` line 173's `setSpark(prev => [...prev.slice(1), n])`.
    public mutating func tick(_ eventsInWindow: Int) {
        samples.removeFirst()
        samples.append(eventsInWindow)
    }
}

/// Convert an events-in-window count to an events-per-minute rate. Exact
/// port of `Events.tsx` line 174's
/// `Math.round((n * 60_000) / SPARK_TICK_MS)`.
public func eventsPerMinute(_ eventsInWindow: Int, windowMS: Double) -> Int {
    Int((Double(eventsInWindow) * 60_000 / windowMS).rounded())
}
