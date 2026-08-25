import RupuAPI
import RupuDesign
import RupuStore
import RupuUsageKit
import SwiftUI

/// Sortable keys for the Usage screen's breakdown table. `ListSort`'s
/// generic `Key` — screen-owned, same "not a shared base type" convention
/// `ListSort.swift`'s doc comment documents.
enum BreakdownSortKey: Hashable, CaseIterable, Sendable {
    case key, runs, tokens, cost
}

private enum BreakdownLayout {
    static let runs: CGFloat = 64
    static let tokens: CGFloat = 84
    static let cost: CGFloat = 84
}

/// Display title for a pivot dimension — mirrors the web's `PIVOT_LABEL`
/// (`PivotPicker.tsx`). Shared by the pivot picker's own segment labels and
/// this table's "Breakdown by <Pivot>" section header (`UsageScreen`).
func pivotTitle(_ pivot: UsagePivot) -> String {
    switch pivot {
    case .model: "Model"
    case .provider: "Provider"
    case .agent: "Agent"
    case .workflow: "Workflow"
    case .host: "Host"
    case .project: "Project"
    }
}

/// The Usage screen's breakdown table (spec §4/brief): `aggregateRows(rows:
/// pivot:)` (Task 5) over `UsageStore.usageRuns` — the SAME flat run rows
/// `SpendChart` builds its stacked areas from — never `APIUsageResponse.
/// breakdown` (`GET /api/usage`'s own server-`group_by`'d field).
///
/// **Why not `UsageResponse.breakdown`, deliberately.** Mirrors the web's
/// own documented fix (`Usage.tsx`'s file-header "TABLE/GRAPH SHARED
/// SOURCE" note): building the table from a *different* dataset than the
/// chart used to mean a table row could name a pivot key the chart had
/// never grouped, and the two could disagree about a window's totals for no
/// principled reason — just because one endpoint's grouping and the other's
/// client-side aggregation happened to diverge. Sourcing both from the same
/// `usageRuns` rows makes that structurally impossible. The cost is scope:
/// this table therefore shares `usageRuns`' documented scope too —
/// **local-only, no host fan-out** (`APIUsageRunRow`'s doc comment) —
/// unlike the fleet-wide `summary`/`unpriced`/`hosts` the rest of this
/// screen renders from `GET /api/usage` itself. `UsageResponse.breakdown`
/// therefore goes deliberately unused by this screen; naming that
/// explicitly here (rather than silently never reading the field) is the
/// point of this doc comment.
///
/// Unpriced rows are flagged (an "unpriced" `Badge`, not just a `—` cost —
/// a `—` alone reads as "no cost", not "unknown cost") and never crash-sort:
/// `ListSort`'s null-payload contract already sorts a `nil` `costUSD` last
/// regardless of direction, so an unpriced row never needs special-casing
/// in the sort comparator itself.
struct BreakdownTable: View {
    let rows: [APIUsageRunRow]
    let pivot: UsagePivot
    @Binding var sort: ListSort<BreakdownSortKey>

    private var pivoted: [PivotRow] {
        aggregateRows(rows: rows, pivot: pivot)
    }

    var body: some View {
        Group {
            if pivoted.isEmpty {
                emptyBlock
            } else {
                table
            }
        }
    }

    private var emptyBlock: some View {
        HStack {
            Spacer(minLength: 0)
            Text("No usage in this window")
                .font(.noteText)
                .foregroundStyle(Color.rupuMute)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, minHeight: 80)
        .panelStyle(.panel)
    }

    private var table: some View {
        let sorted = sortRows(pivoted, sort: sort, value: sortValue)
        return VStack(alignment: .leading, spacing: 0) {
            SortableHeaderRow(columns: columns, sort: $sort)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
            Divider()
            VStack(spacing: 0) {
                ForEach(Array(sorted.enumerated()), id: \.offset) { _, row in
                    BreakdownRow(row: row, pivot: pivot)
                    Divider()
                }
            }
        }
        .panelStyle(.panel)
    }

    private var columns: [SortableColumn<BreakdownSortKey>] {
        [
            // The one flexible, truncating column — every other column is
            // fixed-width, same "exactly one `width: nil` column" contract
            // `SortableColumn`'s doc comment documents. Explicit
            // `firstTapAscending: true` (not left to the alignment
            // heuristic) so this column's first-tap direction is
            // self-documenting regardless of its own alignment, same
            // explicitness `FindingsTable`'s trailing-aligned text columns
            // use it for.
            SortableColumn(key: .key, label: pivotTitle(pivot), width: nil, firstTapAscending: true),
            SortableColumn(key: .runs, label: "Runs", width: BreakdownLayout.runs, alignment: .trailing),
            SortableColumn(key: .tokens, label: "Tokens", width: BreakdownLayout.tokens, alignment: .trailing),
            SortableColumn(key: .cost, label: "Cost", width: BreakdownLayout.cost, alignment: .trailing),
        ]
    }

    /// Instance method (not `static`, unlike `FindingsTabView.sortValue`) —
    /// the `.key` case needs this table's own `pivot` to compute
    /// `pivotLabel`, which a `static` function bound before `sortRows` is
    /// called couldn't close over.
    private func sortValue(_ row: PivotRow, _ key: BreakdownSortKey) -> ListSortValue {
        switch key {
        case .key: .text(pivotLabel(row, pivot: pivot))
        case .runs: .number(Double(row.runs))
        case .tokens: .number(Double(row.totalTokens))
        case .cost: .number(row.costUSD)
        }
    }
}

// MARK: - Row

private struct BreakdownRow: View {
    let row: PivotRow
    let pivot: UsagePivot

    var body: some View {
        HStack(spacing: 0) {
            HStack(spacing: 6) {
                Text(pivotLabel(row, pivot: pivot))
                    .foregroundStyle(Color.rupuInk)
                    .lineLimit(1)
                    .truncationMode(.tail)
                if !row.priced {
                    Badge("unpriced", tone: Color.rupuWarn)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.trailing, 8)

            Text(Fmt.count(row.runs))
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuInk)
                .frame(width: BreakdownLayout.runs, alignment: .trailing)
                .padding(.trailing, 8)

            Text(Fmt.count(Int(row.totalTokens)))
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuDim)
                .frame(width: BreakdownLayout.tokens, alignment: .trailing)
                .padding(.trailing, 8)

            Text(Fmt.cost(row.costUSD))
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuInk)
                .frame(width: BreakdownLayout.cost, alignment: .trailing)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }
}
