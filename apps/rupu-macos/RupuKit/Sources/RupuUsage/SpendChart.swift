import Charts
import Foundation
import RupuAPI
import RupuDesign
import RupuUsageKit
import SwiftUI

// MARK: - Pure adapter (the testable seam, same split `RupuOverview.Charts`
// establishes: a pure `[...] -> [(...)]` reshape function, then a thin View
// over it)

/// Reshapes a client-built spend timeline into stacked-area chart rows, one
/// row per `(bucket day, pivot-key series)` pair — the spend-chart analog of
/// `RupuOverview.chartRows`/`throughputRows`. Exact-semantics port of the
/// web's `toChartData`'s `'cost'` branch (`UsageTimelineStacked.tsx`):
/// - The series key is `pivotLabel(_:pivot:)` (so an empty pivot value on a
///   `PivotRow` reads `"—"`, same series the breakdown table shows for it,
///   rather than an unlabeled blank series the legend can't name).
/// - The value is `PivotRow.costUSD ?? 0` — a pivot-key whose EVERY
///   contributing row on a given day was unpriced draws as a zero-height
///   slice for that day rather than a gap. `aggregateRows` only nils
///   `costUSD` when every contributor was unpriced (a mix of priced/
///   unpriced still sums the priced share) — for the SUM this chart plots,
///   that is exactly the same net result as the web's own per-row "unpriced
///   contributes 0" reducer, just computed via `aggregateRows`'s already-
///   tested summation instead of re-summing raw rows here.
///
/// A bucket with zero contributing rows produces zero series entries for
/// that day (not one zero-valued entry per known series) — Swift Charts
/// still stacks correctly across day gaps, because a stacked `AreaMark`
/// series only needs *some* point at a given x to contribute there; a day
/// entirely absent from every series renders as a zero-height gap in the
/// stack, the correct rendering for "no runs at all that day". A bucket's
/// `day` key is always `buildSpendTimeline`'s well-formed `YYYY-MM-DD`
/// output — this function does not re-validate it (that parsing happens in
/// `SpendChart` itself, which drops an unparseable day defensively; none is
/// ever actually produced).
public func spendChartRows(buckets: [SpendBucket], pivot: UsagePivot) -> [(day: String, series: String, value: Double)] {
    buckets.flatMap { bucket in
        aggregateRows(rows: bucket.rows, pivot: pivot).map { row in
            (day: bucket.day, series: pivotLabel(row, pivot: pivot), value: row.costUSD ?? 0)
        }
    }
}

/// Parses a `buildSpendTimeline`/`SpendBucket.day` key (`YYYY-MM-DD`, always
/// the UTC calendar day per that function's doc comment) back into a `Date`
/// at UTC midnight, for the chart's x-axis. A malformed key (never actually
/// produced, but this stays defensive rather than force-unwrapping) returns
/// `nil` and its row is dropped by the caller — same "never crash on
/// unexpected data" posture `buildSpendTimeline`'s own row-parsing takes.
func parseSpendDayKey(_ key: String) -> Date? {
    let formatter = DateFormatter()
    formatter.calendar = Calendar(identifier: .gregorian)
    formatter.timeZone = TimeZone(identifier: "UTC")
    formatter.dateFormat = "yyyy-MM-dd"
    formatter.isLenient = false
    return formatter.date(from: key)
}

// MARK: - Series → color

/// Fixed categorical palette, ported verbatim (same ten hex values, same
/// order) from the web's `MODEL_PALETTE` (`modelColors.ts`) — theme-
/// invariant by design on both sides (a chart series color doesn't need to
/// answer to light/dark the way semantic status tones do, and the web's own
/// palette isn't defined as a light/dark pair either).
let spendSeriesPalette: [Color] = [
    Color(red: 0x18 / 255, green: 0x60 / 255, blue: 0xF2 / 255), // brand blue
    Color(red: 0x22 / 255, green: 0xC5 / 255, blue: 0x5E / 255), // green
    Color(red: 0xF5 / 255, green: 0x9E / 255, blue: 0x0B / 255), // amber
    Color(red: 0xA8 / 255, green: 0x55 / 255, blue: 0xF7 / 255), // purple
    Color(red: 0xEC / 255, green: 0x48 / 255, blue: 0x99 / 255), // pink
    Color(red: 0x06 / 255, green: 0xB6 / 255, blue: 0xD4 / 255), // cyan
    Color(red: 0xEF / 255, green: 0x44 / 255, blue: 0x44 / 255), // red
    Color(red: 0x84 / 255, green: 0xCC / 255, blue: 0x16 / 255), // lime
    Color(red: 0x63 / 255, green: 0x66 / 255, blue: 0xF1 / 255), // indigo
    Color(red: 0x14 / 255, green: 0xB8 / 255, blue: 0xA6 / 255), // teal
]

/// Assigns each distinct series label a stable color by cycling
/// `spendSeriesPalette` over the SORTED label set — exact port of the web's
/// `assignModelColors` (same "sort first, so the mapping is deterministic
/// regardless of input order" contract, cycled with `%` past 10 distinct
/// keys). Deliberately index-based over sorted labels, never a per-model or
/// per-brand hardcode (there is no "claude = orange" table here, matching
/// the web source, which reserves an actual identity palette for the model
/// pivot specifically and uses this same generic cycle for the other five —
/// this app doesn't distinguish the two, an honest simplification: an
/// arbitrary stable categorical cycle for every pivot, not invented brand
/// identity for one of them).
func assignSeriesColors(_ labels: [String]) -> [String: Color] {
    let sorted = Array(Set(labels)).sorted()
    var map: [String: Color] = [:]
    for (index, label) in sorted.enumerated() {
        map[label] = spendSeriesPalette[index % spendSeriesPalette.count]
    }
    return map
}

// MARK: - Chart chrome

private let spendChartPlotHeight: CGFloat = 164
private let spendChartFillOpacity: Double = 0.25

/// Same chrome shape `RupuOverview.Charts`' (private, unexported)
/// `StackedAreaChrome` establishes — fixed plot height, inline bottom
/// legend, day-formatted x labels, a leading-gridline y axis, no
/// `.animation(...)` (this chart is as static as those two; see that type's
/// doc comment for the liveness rationale) — re-derived locally rather than
/// imported since `StackedAreaChrome` is `private` to `RupuOverview` and
/// this module cannot reach into another screen module's internals. The y
/// axis differs from that chrome's plain integer format: this chart plots
/// dollars, so its ticks are USD-formatted instead.
private struct SpendChartChrome: ViewModifier {
    func body(content: Content) -> some View {
        content
            .chartLegend(position: .bottom, alignment: .leading, spacing: 8)
            .chartXAxis {
                AxisMarks(values: .automatic) { _ in
                    AxisValueLabel(format: .dateTime.month(.abbreviated).day())
                }
            }
            .chartYAxis {
                AxisMarks(position: .leading) { _ in
                    AxisGridLine()
                    AxisValueLabel(format: .currency(code: "USD").precision(.fractionLength(0)))
                }
            }
            .frame(height: spendChartPlotHeight)
    }
}

// MARK: - View

/// The Usage screen's spend-over-time chart (spec §4/brief): a stacked area
/// per pivot key, built from `UsageStore.usageRuns` via `buildSpendTimeline`
/// (Task 5) + `aggregateRows` (Task 5) — the SAME two functions
/// `BreakdownTable` below composes, so the chart and the table always agree
/// on totals for a given window/pivot (see `BreakdownTable`'s doc comment
/// for why that shared-source property matters). `buckets` is the caller's
/// responsibility to build (via `UsageStore.windowBounds(for:)` +
/// `buildSpendTimeline`) — this view only reshapes+renders, mirroring
/// `OutcomesChart`/`ThroughputChart` taking already-fetched buckets rather
/// than a store reference.
public struct SpendChart: View {
    private let buckets: [SpendBucket]
    private let pivot: UsagePivot

    public init(buckets: [SpendBucket], pivot: UsagePivot) {
        self.buckets = buckets
        self.pivot = pivot
    }

    private var rows: [(day: Date, series: String, value: Double)] {
        spendChartRows(buckets: buckets, pivot: pivot).compactMap { row in
            guard let date = parseSpendDayKey(row.day) else { return nil }
            return (day: date, series: row.series, value: row.value)
        }
    }

    private var seriesDomain: [String] {
        Array(Set(rows.map(\.series))).sorted()
    }

    public var body: some View {
        let domain = seriesDomain
        let colors = assignSeriesColors(domain)
        Chart {
            ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                AreaMark(
                    x: .value("Date", row.day),
                    y: .value("Spend", row.value)
                )
                .foregroundStyle(by: .value("Series", row.series))
                .opacity(spendChartFillOpacity)
            }
        }
        .chartForegroundStyleScale(
            domain: domain,
            range: domain.map { colors[$0] ?? Color.rupuDim }
        )
        .modifier(SpendChartChrome())
    }
}
