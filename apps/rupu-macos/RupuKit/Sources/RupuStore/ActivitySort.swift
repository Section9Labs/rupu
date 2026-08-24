import Foundation

/// The Activity table's current sort — one column key plus direction, held
/// as view `@State` in `ActivityTable` and applied over `store.rows` in the
/// view body (so a live-tail patch that changes `store.rows` re-sorts
/// naturally, with no separate re-sort trigger needed).
public struct ActivitySort: Equatable, Sendable {
    public enum Key: String, CaseIterable, Sendable {
        case status, kind, subject, project, host, trigger, duration, cost, started
    }

    public var key: Key
    public var ascending: Bool

    public init(key: Key, ascending: Bool) {
        self.key = key
        self.ascending = ascending
    }
}

extension ActivitySort.Key {
    /// First-tap direction when a header is newly selected (the column
    /// wasn't already the active sort). Deliberate macOS-native divergence
    /// from the web `SortableTable` (whose `toggleSort` always starts a new
    /// column ascending): numeric/time columns start descending (biggest /
    /// most-recent first) the way Finder and Mail date/size columns do,
    /// text columns start ascending (A→Z). "Web look, native feel."
    public var defaultAscending: Bool {
        switch self {
        case .duration, .cost, .started: return false
        case .status, .kind, .subject, .project, .host, .trigger: return true
        }
    }
}

/// Sorts `rows` by `sort.key`/`sort.ascending`. Pure and stable (relies on
/// `Array.sorted(by:)`'s documented stability): equal-valued rows keep their
/// relative input order, so re-sorting after a live-tail patch never
/// reshuffles rows that didn't change on the active column.
///
/// Contract (matches the web `SortableTable.tsx` this mirrors):
/// - `started` compares `ActivityRow.startedAt`; `duration`/`cost` compare
///   numerically; every other key compares its text case-insensitively.
/// - Nulls (`project`/`trigger`/`durationMS`/`costUSD`/`startedAt` — the
///   optional columns) always sort last, **regardless of direction**: a
///   still-`nil` cost row belongs at the bottom whether cost is sorted
///   ascending or descending, exactly as the row's blank cell suggests.
public func sortActivityRows(_ rows: [ActivityRow], by sort: ActivitySort) -> [ActivityRow] {
    rows.sorted { (lhs: ActivityRow, rhs: ActivityRow) -> Bool in
        switch sort.key {
        case .status:
            return orderedBeforeText(lhs.status.displayLabel, rhs.status.displayLabel, ascending: sort.ascending)
        case .kind:
            // `ActivityKindTag.displayLabel` (RupuActivity) isn't visible
            // from here — `rawValue` ("agent"/"workflow"/"autoflow"/
            // "session") sorts case-insensitively to the same relative
            // order and is what this module can see.
            return orderedBeforeText(lhs.kind.rawValue, rhs.kind.rawValue, ascending: sort.ascending)
        case .subject:
            return orderedBeforeText(lhs.subject, rhs.subject, ascending: sort.ascending)
        case .project:
            return orderedBeforeOptionalText(lhs.project, rhs.project, ascending: sort.ascending)
        case .host:
            return orderedBeforeText(lhs.host, rhs.host, ascending: sort.ascending)
        case .trigger:
            return orderedBeforeOptionalText(lhs.trigger, rhs.trigger, ascending: sort.ascending)
        case .duration:
            return orderedBeforeOptional(lhs.durationMS, rhs.durationMS, ascending: sort.ascending, base: orderedBefore)
        case .cost:
            return orderedBeforeOptional(lhs.costUSD, rhs.costUSD, ascending: sort.ascending, base: orderedBefore)
        case .started:
            return orderedBeforeOptional(lhs.startedAt, rhs.startedAt, ascending: sort.ascending, base: orderedBefore)
        }
    }
}

// MARK: - Comparators

/// True if `lhs` sorts strictly before `rhs` under `ascending`, for a
/// non-optional `Comparable` value. Equal values return `false` (a tie —
/// `sorted(by:)`'s stability then preserves input order).
private func orderedBefore<T: Comparable>(_ lhs: T, _ rhs: T, ascending: Bool) -> Bool {
    guard lhs != rhs else { return false }
    return ascending ? lhs < rhs : lhs > rhs
}

/// Case-insensitive text version of `orderedBefore`.
private func orderedBeforeText(_ lhs: String, _ rhs: String, ascending: Bool) -> Bool {
    let cmp = lhs.compare(rhs, options: .caseInsensitive)
    guard cmp != .orderedSame else { return false }
    return ascending ? cmp == .orderedAscending : cmp == .orderedDescending
}

/// Wraps a non-optional comparator with a nulls-last rule that ignores
/// `ascending` entirely for the nil/non-nil cases — only two non-nil values
/// get the direction applied, via `base`.
private func orderedBeforeOptional<T>(
    _ lhs: T?, _ rhs: T?, ascending: Bool, base: (T, T, Bool) -> Bool
) -> Bool {
    switch (lhs, rhs) {
    case (nil, nil):
        return false
    case (nil, _):
        // lhs is nil: never sorts before a non-nil rhs, in either direction.
        return false
    case (_, nil):
        // rhs is nil: a non-nil lhs always sorts before it, in either direction.
        return true
    case let (l?, r?):
        return base(l, r, ascending)
    }
}

private func orderedBeforeOptionalText(_ lhs: String?, _ rhs: String?, ascending: Bool) -> Bool {
    orderedBeforeOptional(lhs, rhs, ascending: ascending, base: orderedBeforeText)
}
