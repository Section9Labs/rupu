import Foundation

/// A column key + direction pair for a sortable list — the generic
/// counterpart to `RupuStore.ActivitySort`. `ActivitySort` stays exactly as
/// it is (its own tests untouched): `RupuDesign` has no `RupuStore`
/// dependency (see `Package.swift`), so this is a deliberate parallel
/// implementation of the same contract for any other sortable list (Phase
/// 5A's Projects/Fleet/Library screens), not a shared base type the two
/// funnel through.
///
/// `Sendable` (and the `Key: Sendable` requirement it drags along): under
/// Swift 6 mode, a store's `@Published`/`@State`-held sort value has to
/// cross actor boundaries with the store itself (e.g. a screen's
/// `AppModel`/`@MainActor` store handing its current `ListSort` to a
/// background task) — a non-`Sendable` value there is a compile error, not
/// a runtime one, so this is conformance the type needs from day one rather
/// than bolted on once a call site trips over its absence.
public struct ListSort<Key: Hashable & CaseIterable & Sendable>: Equatable, Sendable {
    public var key: Key
    public var ascending: Bool

    public init(key: Key, ascending: Bool) {
        self.key = key
        self.ascending = ascending
    }
}

/// The value one row contributes for one sortable column, tagged by kind so
/// `sortRows` knows which comparator to apply — case-insensitive text,
/// numeric, or date — independent of the caller's row and key types. A
/// `nil` payload in any case means "this row has no value for this column"
/// and always sorts last (see `sortRows` below). `Sendable` for the same
/// reason as `ListSort` above — every payload type (`String?`, `Double?`,
/// `Date?`) already is, so this is free.
public enum ListSortValue: Sendable {
    case text(String?)
    case number(Double?)
    case date(Date?)
}

/// Sorts `rows` by `sort.key`/`sort.ascending`, using `value` to pull each
/// row's sort-relevant value for the active key. Pure: does not mutate
/// `rows`, and — for a fixed `sort` and `value` — is a deterministic
/// function of `rows` alone.
///
/// Semantics, generalized from `RupuStore.sortActivityRows`/`ActivitySort`
/// (see that type's doc comment for the original contract this mirrors):
/// - `.text` values compare case-insensitively.
/// - Nulls (a `nil` payload in any `ListSortValue` case) always sort last,
///   **regardless of `sort.ascending`** — a still-empty cell belongs at the
///   bottom whichever direction the column is sorted, exactly as its blank
///   display suggests.
/// - Ties (including nil-vs-nil) preserve the rows' original relative
///   order. Unlike `sortActivityRows` (which leans on `Array.sorted(by:)`'s
///   documented-stable behavior), this is implemented via an explicit index
///   tiebreak, so the stability guarantee holds independent of that detail.
///
/// A `value` closure that returns different `ListSortValue` cases for the
/// same `key` across rows (e.g. `.text` for one row, `.number` for another)
/// is a caller bug; such a pair compares as a tie rather than crashing or
/// producing a confusing partial order — see
/// `mixedKindPairComparesAsATieAndPreservesOriginalOrder` in
/// `ListSortTests.swift` for the locked-in regression.
public func sortRows<Row, Key: Hashable & CaseIterable & Sendable>(
    _ rows: [Row], sort: ListSort<Key>, value: (Row, Key) -> ListSortValue
) -> [Row] {
    rows.enumerated()
        .sorted { lhs, rhs in
            switch orderedBefore(value(lhs.element, sort.key), value(rhs.element, sort.key), ascending: sort.ascending) {
            case .before: return true
            case .after: return false
            case .tie: return lhs.offset < rhs.offset
            }
        }
        .map(\.element)
}

// MARK: - Comparators

private enum Ordering {
    case before, after, tie
}

private func orderedBefore(_ lhs: ListSortValue, _ rhs: ListSortValue, ascending: Bool) -> Ordering {
    switch (lhs, rhs) {
    case let (.text(l), .text(r)):
        return orderedBeforeOptionalText(l, r, ascending: ascending)
    case let (.number(l), .number(r)):
        return orderedBeforeOptionalComparable(l, r, ascending: ascending)
    case let (.date(l), .date(r)):
        return orderedBeforeOptionalComparable(l, r, ascending: ascending)
    default:
        // Mismatched cases for the same key: a caller bug (see the doc
        // comment above) — treat as a tie rather than crash or misorder.
        return .tie
    }
}

/// Nulls-last (regardless of `ascending`), case-insensitive text ordering.
private func orderedBeforeOptionalText(_ lhs: String?, _ rhs: String?, ascending: Bool) -> Ordering {
    switch (lhs, rhs) {
    case (nil, nil):
        return .tie
    case (nil, _):
        return .after
    case (_, nil):
        return .before
    case let (l?, r?):
        let cmp = l.compare(r, options: .caseInsensitive)
        guard cmp != .orderedSame else { return .tie }
        return (ascending ? cmp == .orderedAscending : cmp == .orderedDescending) ? .before : .after
    }
}

/// Nulls-last (regardless of `ascending`) ordering for any `Comparable`
/// (used for `.number`/`.date`).
private func orderedBeforeOptionalComparable<T: Comparable>(_ lhs: T?, _ rhs: T?, ascending: Bool) -> Ordering {
    switch (lhs, rhs) {
    case (nil, nil):
        return .tie
    case (nil, _):
        return .after
    case (_, nil):
        return .before
    case let (l?, r?):
        guard l != r else { return .tie }
        return (ascending ? l < r : l > r) ? .before : .after
    }
}
