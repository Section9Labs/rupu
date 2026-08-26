import Foundation

// MARK: - Pure seam (the tested derivation)
//
// Lifted here from `RupuOverview/NeedsYou.swift` (perf & interaction arc,
// Plan 5 Task 4) — `RupuActivity`'s new `.all`-kind stats surface needs this
// exact derivation for its own compact needs-attention list, but
// `RupuActivity` must not import `RupuOverview` (that would be a screen
// module depending on a sibling screen module purely for a data helper, and
// `RupuOverview` doesn't depend on `RupuActivity` either — there was no
// existing edge to lean on). Every type this derivation touches
// (`ActivityRow`, `TimeRange`) already lives in `RupuStore`, which both
// `RupuActivity` and `RupuOverview` already depend on, so this is the one
// shared home the module graph actually supports without a new dependency
// edge in either direction. `RupuOverview/NeedsYou.swift` keeps its
// `NeedsYouCard`/`NeedsYouRow` View machinery — only the pure, non-View
// derivation (this file's contents) moved; the View layer still calls
// `deriveNeedsYou`/`NeedsYouItem` exactly as before, resolved from `RupuStore`
// instead of locally, with no call-site changes needed since it already
// `import`s this module.

/// One entry in the needs-you queue — either a gate parked on the operator's
/// approval, or a recently failed run. Carries the full `ActivityRow` (not
/// just the fields the card renders) so `NeedsYouCard` has everything it
/// needs — subject, host/project breadcrumb, `navigation` for the "Open"
/// action — without a second lookup back into `ActivityStore.rows`.
public struct NeedsYouItem: Equatable, Identifiable, Sendable {
    public enum Kind: String, Equatable, Sendable {
        case gate, failedRun
    }

    public let id: String
    public let kind: Kind
    public let row: ActivityRow

    /// `id` is kind-prefixed rather than just `row.id` — a row can never
    /// actually be both `.awaiting` and `.failed` at once (today's single
    /// `ActivityStatus`), so a collision can't happen in practice, but a
    /// `ForEach`-safe `Identifiable` shouldn't depend on that invariant
    /// holding forever.
    public init(kind: Kind, row: ActivityRow) {
        self.kind = kind
        self.row = row
        self.id = "\(kind.rawValue):\(row.id)"
    }
}

/// The client-side aggregate behind the Overview screen's needs-you queue
/// (spec: "what needs the operator's attention right now" — gates parked on
/// approval, plus runs that recently failed), and behind the Activity
/// screen's `.all`-kind stats surface's compact needs-attention list (perf &
/// interaction arc, Plan 5 Task 4). Pure and free-standing (not a `View`
/// member, not a store method) so it's testable without `@MainActor` per
/// this phase's CI rule: only tests that touch a `View`-type member need it.
///
/// **Two sources, two orders, one cap:**
/// - Every `.awaiting` row, oldest-first (`startedAt` ascending, unknown
///   start times sorted last — an unknown age can't be claimed as "oldest",
///   the most urgent position in this list).
/// - Every `.failed` row whose `startedAt` "provably belongs" to `range`
///   (see `fallsInsideRange(_:range:now:)` below), newest-first.
///
/// Gates are listed before failures regardless of either group's own
/// timestamps — an open gate is a standing ask for input the operator must
/// resolve to unblock a run; a failed run is already terminal and asks only
/// to be looked at. Concatenation, then a flat cap of 6: `overflow` is
/// whatever didn't fit, floored at 0 (a `.d7`/`.d30` window can shrink the
/// failed side to nothing without ever producing a negative count).
public func deriveNeedsYou(rows: [ActivityRow], range: TimeRange, now: Date) -> (items: [NeedsYouItem], overflow: Int) {
    let gateItems = rows
        .filter { $0.status == .awaiting }
        .sorted(by: isOrderedOldestFirst)
        .map { NeedsYouItem(kind: .gate, row: $0) }

    let failedItems = rows
        .filter { $0.status == .failed && fallsInsideRange($0.startedAt, range: range, now: now) }
        .sorted(by: isOrderedNewestFirst)
        .map { NeedsYouItem(kind: .failedRun, row: $0) }

    let combined = gateItems + failedItems
    let cap = 6
    return (items: Array(combined.prefix(cap)), overflow: max(0, combined.count - cap))
}

/// Whether a failed row's `startedAt` "provably belongs" to `range`,
/// relative to `now`:
/// - `.all` imposes no window at all — every failed row passes, including
///   one with `startedAt == nil` (there is nothing to exclude it *from*).
/// - `.d7`/`.d30` impose an actual window. A `nil` `startedAt` **fails** it
///   — an unknown start time is not evidence the row belongs inside the
///   last 7/30 days, so honesty means excluding it rather than guessing.
///   A `startedAt` after `now` (clock skew) also fails — "within the last N
///   days" doesn't stretch to cover the future just because the raw
///   interval magnitude is small.
private func fallsInsideRange(_ startedAt: Date?, range: TimeRange, now: Date) -> Bool {
    switch range {
    case .all:
        return true
    case .d7, .d30:
        guard let startedAt, startedAt <= now else { return false }
        return now.timeIntervalSince(startedAt) <= range.windowSeconds
    }
}

private extension TimeRange {
    var windowSeconds: TimeInterval {
        switch self {
        case .d7: return 7 * 86_400
        case .d30: return 30 * 86_400
        case .all: return .infinity
        }
    }
}

/// Ascending by `startedAt`; a `nil` start time sorts last in either
/// direction (matches `ActivityStore.isOrderedByStartedAtDescending`'s own
/// "unknown is never the extreme" convention, mirrored here for the
/// opposite direction).
private func isOrderedOldestFirst(_ lhs: ActivityRow, _ rhs: ActivityRow) -> Bool {
    switch (lhs.startedAt, rhs.startedAt) {
    case let (l?, r?): return l < r
    case (nil, nil): return false
    case (nil, _): return false
    case (_, nil): return true
    }
}

/// Descending by `startedAt`; `nil` sorts last, same convention as
/// `ActivityStore.isOrderedByStartedAtDescending` (that comparator is
/// `private` to `ActivityStore`, so this is a deliberate, small
/// reimplementation rather than a shared dependency — see the Task 5 report
/// for why lifting it wasn't worth a cross-module seam for one comparator).
private func isOrderedNewestFirst(_ lhs: ActivityRow, _ rhs: ActivityRow) -> Bool {
    switch (lhs.startedAt, rhs.startedAt) {
    case let (l?, r?): return l > r
    case (nil, nil): return false
    case (nil, _): return false
    case (_, nil): return true
    }
}
