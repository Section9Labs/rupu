import Testing
import Foundation
@testable import RupuDesign

/// Pure comparator coverage for `sortRows`/`ListSort` — generic groundwork
/// for Phase 5A's sortable Projects/Fleet/Library screens. Mirrors
/// `RupuStoreTests/ActivitySortTests.swift`'s coverage shape (the semantics
/// this generalizes) but over a throwaway fixture row/key, not `ActivityRow`.
private enum TestKey: Hashable, CaseIterable {
    case name, score, when
}

private struct TestRow {
    let id: String
    let name: String?
    let score: Double?
    let when: Date?
}

private func row(id: String, name: String? = "s", score: Double? = nil, when: Date? = nil) -> TestRow {
    TestRow(id: id, name: name, score: score, when: when)
}

private func value(_ row: TestRow, _ key: TestKey) -> ListSortValue {
    switch key {
    case .name: return .text(row.name)
    case .score: return .number(row.score)
    case .when: return .date(row.when)
    }
}

private let t0 = Date(timeIntervalSince1970: 1_700_000_000)
private let t1 = t0.addingTimeInterval(60)
private let t2 = t0.addingTimeInterval(120)

// MARK: - Text case-insensitivity

@Test func textSortIsCaseInsensitive() {
    let rows = [
        row(id: "upper", name: "Zebra"),
        row(id: "lower", name: "apple"),
        row(id: "mixed", name: "Banana"),
    ]
    let sorted = sortRows(rows, sort: ListSort(key: .name, ascending: true), value: value)
    #expect(sorted.map(\.id) == ["lower", "mixed", "upper"])
}

// MARK: - Nulls last, both directions

@Test func textSortNullsLastBothDirections() {
    let rows = [
        row(id: "none-1", name: nil),
        row(id: "beta", name: "beta"),
        row(id: "alpha", name: "alpha"),
        row(id: "none-2", name: nil),
    ]
    let ascending = sortRows(rows, sort: ListSort(key: .name, ascending: true), value: value)
    #expect(ascending.map(\.id) == ["alpha", "beta", "none-1", "none-2"])

    let descending = sortRows(rows, sort: ListSort(key: .name, ascending: false), value: value)
    #expect(descending.map(\.id) == ["beta", "alpha", "none-1", "none-2"])
}

@Test func numberSortNullsLastBothDirections() {
    let rows = [
        row(id: "no-score-1", score: nil),
        row(id: "low", score: 1.0),
        row(id: "high", score: 9.0),
        row(id: "no-score-2", score: nil),
    ]
    let ascending = sortRows(rows, sort: ListSort(key: .score, ascending: true), value: value)
    #expect(ascending.map(\.id) == ["low", "high", "no-score-1", "no-score-2"])

    let descending = sortRows(rows, sort: ListSort(key: .score, ascending: false), value: value)
    #expect(descending.map(\.id) == ["high", "low", "no-score-1", "no-score-2"])
}

@Test func dateSortNullsLastBothDirections() {
    let rows = [
        row(id: "no-date", when: nil),
        row(id: "earlier", when: t0),
        row(id: "later", when: t2),
    ]
    let ascending = sortRows(rows, sort: ListSort(key: .when, ascending: true), value: value)
    #expect(ascending.map(\.id) == ["earlier", "later", "no-date"])

    let descending = sortRows(rows, sort: ListSort(key: .when, ascending: false), value: value)
    #expect(descending.map(\.id) == ["later", "earlier", "no-date"])
}

// MARK: - Stable tiebreak

@Test func tiesPreserveOriginalRelativeOrder() {
    let rows = [
        row(id: "a", name: "same"),
        row(id: "b", name: "same"),
        row(id: "c", name: "same"),
    ]
    let sorted = sortRows(rows, sort: ListSort(key: .name, ascending: true), value: value)
    #expect(sorted.map(\.id) == ["a", "b", "c"])
}

@Test func numericTiesPreserveOriginalRelativeOrderAmongEqualValues() {
    let rows = [
        row(id: "first", score: 5),
        row(id: "second", score: 1),
        row(id: "third", score: 5),
    ]
    let sorted = sortRows(rows, sort: ListSort(key: .score, ascending: true), value: value)
    #expect(sorted.map(\.id) == ["second", "first", "third"])
}

@Test func nilTiesPreserveOriginalRelativeOrder() {
    let rows = [
        row(id: "has-value", score: 1),
        row(id: "nil-1", score: nil),
        row(id: "nil-2", score: nil),
    ]
    let sorted = sortRows(rows, sort: ListSort(key: .score, ascending: true), value: value)
    #expect(sorted.map(\.id) == ["has-value", "nil-1", "nil-2"])
}

// MARK: - Numeric sorts numerically, not lexically

@Test func numberSortsNumericallyNotLexically() {
    let rows = [
        row(id: "two-thousand", score: 2000),
        row(id: "ninety", score: 90),
        row(id: "three-hundred", score: 300),
    ]
    // Lexical string comparison would order "2000" < "300" < "90"; numeric
    // must order 90 < 300 < 2000.
    let sorted = sortRows(rows, sort: ListSort(key: .score, ascending: true), value: value)
    #expect(sorted.map(\.id) == ["ninety", "three-hundred", "two-thousand"])
}

// MARK: - Empty input

@Test func emptyRowsSortsToEmpty() {
    let rows: [TestRow] = []
    let sorted = sortRows(rows, sort: ListSort(key: .name, ascending: true), value: value)
    #expect(sorted.isEmpty)
}

// MARK: - Purity (no mutation of input)

@Test func sortDoesNotMutateInputArray() {
    let rows = [row(id: "b", when: t0), row(id: "a", when: t1)]
    let originalOrder = rows.map(\.id)
    _ = sortRows(rows, sort: ListSort(key: .when, ascending: false), value: value)
    #expect(rows.map(\.id) == originalOrder)
}

// MARK: - ListSort equality

@Test func listSortEqualityComparesKeyAndDirection() {
    #expect(ListSort(key: TestKey.name, ascending: true) == ListSort(key: TestKey.name, ascending: true))
    #expect(ListSort(key: TestKey.name, ascending: true) != ListSort(key: TestKey.name, ascending: false))
    #expect(ListSort(key: TestKey.name, ascending: true) != ListSort(key: TestKey.score, ascending: true))
}
