import Testing
import Foundation
@testable import RupuStore

/// Pure comparator coverage for `sortActivityRows` — no store, no network,
/// just `ActivityRow` values built directly via the module-internal
/// memberwise init (visible here through `@testable import`). Placement:
/// there is no `RupuActivityTests` target in `Package.swift` (`RupuActivity`
/// has no `.testTarget` at all), so this comparator — and the `ActivitySort`
/// type it operates on — live in `RupuStore` instead, alongside
/// `ActivityRow` itself, which `RupuActivity` already depends on.
private func row(
    id: String, subject: String = "s", project: String? = nil, host: String = "local",
    trigger: String? = nil, status: ActivityStatus = .completed, durationMS: UInt64? = nil,
    costUSD: Double? = nil, startedAt: Date? = nil
) -> ActivityRow {
    ActivityRow(
        id: id, kind: .workflow, subject: subject, project: project, host: host,
        trigger: trigger, status: status, durationMS: durationMS, costUSD: costUSD,
        startedAt: startedAt, navigation: .none
    )
}

private let t0 = Date(timeIntervalSince1970: 1_700_000_000)
private let t1 = t0.addingTimeInterval(60)
private let t2 = t0.addingTimeInterval(120)

// MARK: - Default sort reproduces store order

// `ActivityStore.isOrderedByStartedAtDescending` (the merge-time comparator
// today's store already uses): descending by startedAt, nil last regardless
// of direction. `ActivitySort(key: .started, ascending: false)` must
// reproduce it byte-for-byte on the same fixture-shaped rows.
@Test func defaultSortReproducesStoreOrderOnMergedFixtureRows() {
    let rows = [
        row(id: "run-wf-1", startedAt: t1),
        row(id: "sess-1", startedAt: t0),
        row(id: "run-ag-1", startedAt: t2),
        row(id: "evt-1", startedAt: t1.addingTimeInterval(30)),
    ]
    let sorted = sortActivityRows(rows, by: ActivitySort(key: .started, ascending: false))
    #expect(sorted.map(\.id) == ["run-ag-1", "evt-1", "run-wf-1", "sess-1"])
}

@Test func startedSortNullsLastRegardlessOfDirection() {
    let rows = [
        row(id: "no-date-1", startedAt: nil),
        row(id: "has-date", startedAt: t0),
        row(id: "no-date-2", startedAt: nil),
    ]
    let descending = sortActivityRows(rows, by: ActivitySort(key: .started, ascending: false))
    #expect(descending.map(\.id) == ["has-date", "no-date-1", "no-date-2"])

    let ascending = sortActivityRows(rows, by: ActivitySort(key: .started, ascending: true))
    #expect(ascending.map(\.id) == ["has-date", "no-date-1", "no-date-2"])
}

// MARK: - Nulls last on other optional columns (cost, project, trigger)

@Test func costSortNullsLastBothDirections() {
    let rows = [
        row(id: "no-cost-1", costUSD: nil),
        row(id: "cheap", costUSD: 0.10),
        row(id: "no-cost-2", costUSD: nil),
        row(id: "pricey", costUSD: 4.00),
    ]
    let ascending = sortActivityRows(rows, by: ActivitySort(key: .cost, ascending: true))
    #expect(ascending.map(\.id) == ["cheap", "pricey", "no-cost-1", "no-cost-2"])

    let descending = sortActivityRows(rows, by: ActivitySort(key: .cost, ascending: false))
    #expect(descending.map(\.id) == ["pricey", "cheap", "no-cost-1", "no-cost-2"])
}

@Test func projectSortNullsLastBothDirections() {
    let rows = [
        row(id: "none-1", project: nil),
        row(id: "beta", project: "beta"),
        row(id: "alpha", project: "alpha"),
        row(id: "none-2", project: nil),
    ]
    let ascending = sortActivityRows(rows, by: ActivitySort(key: .project, ascending: true))
    #expect(ascending.map(\.id) == ["alpha", "beta", "none-1", "none-2"])
}

// MARK: - Stable tie-break

@Test func tiesPreserveOriginalRelativeOrder() {
    let rows = [
        row(id: "a", status: .completed),
        row(id: "b", status: .completed),
        row(id: "c", status: .completed),
    ]
    let sorted = sortActivityRows(rows, by: ActivitySort(key: .status, ascending: true))
    #expect(sorted.map(\.id) == ["a", "b", "c"])
}

@Test func durationTiesPreserveOriginalRelativeOrderAmongEqualValues() {
    let rows = [
        row(id: "first", durationMS: 5000),
        row(id: "second", durationMS: 1000),
        row(id: "third", durationMS: 5000),
    ]
    let sorted = sortActivityRows(rows, by: ActivitySort(key: .duration, ascending: true))
    // 1000 sorts before both 5000s; the two 5000s keep their relative order.
    #expect(sorted.map(\.id) == ["second", "first", "third"])
}

// MARK: - Text case-insensitivity

@Test func subjectSortIsCaseInsensitive() {
    let rows = [
        row(id: "upper", subject: "Zebra"),
        row(id: "lower", subject: "apple"),
        row(id: "mixed", subject: "Banana"),
    ]
    let sorted = sortActivityRows(rows, by: ActivitySort(key: .subject, ascending: true))
    #expect(sorted.map(\.id) == ["lower", "mixed", "upper"])
}

@Test func hostSortIsCaseInsensitive() {
    let rows = [
        row(id: "a", host: "Mini"),
        row(id: "b", host: "local"),
    ]
    let sorted = sortActivityRows(rows, by: ActivitySort(key: .host, ascending: true))
    #expect(sorted.map(\.id) == ["b", "a"])
}

// MARK: - Numeric keys (duration)

@Test func durationSortsNumericallyNotLexically() {
    let rows = [
        row(id: "two-thousand", durationMS: 2000),
        row(id: "ninety", durationMS: 90),
        row(id: "three-hundred", durationMS: 300),
    ]
    // Lexical string comparison would order "2000" < "300" < "90"; numeric
    // must order 90 < 300 < 2000.
    let sorted = sortActivityRows(rows, by: ActivitySort(key: .duration, ascending: true))
    #expect(sorted.map(\.id) == ["ninety", "three-hundred", "two-thousand"])
}

// MARK: - First-tap direction (`defaultAscending`)

@Test func defaultDirectionIsAscendingForTextKeysDescendingForNumericTimeKeys() {
    #expect(ActivitySort.Key.status.defaultAscending == true)
    #expect(ActivitySort.Key.kind.defaultAscending == true)
    #expect(ActivitySort.Key.subject.defaultAscending == true)
    #expect(ActivitySort.Key.project.defaultAscending == true)
    #expect(ActivitySort.Key.host.defaultAscending == true)
    #expect(ActivitySort.Key.trigger.defaultAscending == true)
    #expect(ActivitySort.Key.duration.defaultAscending == false)
    #expect(ActivitySort.Key.cost.defaultAscending == false)
    #expect(ActivitySort.Key.started.defaultAscending == false)
}

// MARK: - Purity (no mutation of input)

@Test func sortDoesNotMutateInputArray() {
    let rows = [row(id: "b", startedAt: t0), row(id: "a", startedAt: t1)]
    let originalOrder = rows.map(\.id)
    _ = sortActivityRows(rows, by: ActivitySort(key: .started, ascending: false))
    #expect(rows.map(\.id) == originalOrder)
}
