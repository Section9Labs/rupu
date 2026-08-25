import SwiftUI
import RupuAPI
import RupuStore
import RupuDesign

/// Sortable keys for the Security screen's Coverage table. `project` is
/// deliberately absent — it's the group boundary (see `CoverageTabView`'s
/// doc comment), not a sortable column of its own.
public enum CoverageSortKey: Hashable, CaseIterable, Sendable {
    case target, assertions, catalog, findings
}

private enum CoverageLayout {
    static let assertions: CGFloat = 88
    static let catalog: CGFloat = 84
    static let findings: CGFloat = 84
}

/// The Coverage tab's content: every coverage target across every
/// registered workspace, **grouped by project** (a plain `Eyebrow` section
/// label per project, projects ordered alphabetically) — the brief's
/// "grouped-by-project summary rows". One shared `SortableHeaderRow`/
/// `ListSort` (same "ListSort kit for both tables" contract `FindingsTable.
/// swift`'s table follows) orders the rows WITHIN each project group by
/// target/assertions/catalog/findings; the project grouping itself is a
/// fixed alphabetical order, not a sortable column (there is no single
/// column position that could represent "group by project" as a table
/// header without either duplicating the section label into every row or
/// making the grouping itself feel like it silently changes when a column
/// is tapped).
struct CoverageTabView: View {
    let coverage: BlockState<[APICoverageSummary]>
    @Binding var sort: ListSort<CoverageSortKey>
    let onSelect: (Route) -> Void

    var body: some View {
        Group {
            switch coverage {
            case .loading:
                securityLoadingBlock()
            case .failed(let message):
                securityFailedBlock(message, subject: "coverage")
            case .empty:
                securityEmptyBlock("No coverage targets")
            case .content(let rows):
                table(rows)
            }
        }
    }

    private func table(_ rows: [APICoverageSummary]) -> some View {
        let groups = Self.groupedByProject(rows, sort: sort)
        return VStack(alignment: .leading, spacing: 0) {
            SortableHeaderRow(columns: columns, sort: $sort)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
            Divider()
            VStack(alignment: .leading, spacing: 0) {
                ForEach(groups, id: \.project) { group in
                    Eyebrow(group.project)
                        .padding(.horizontal, 12)
                        .padding(.top, 10)
                        .padding(.bottom, 4)
                    ForEach(Array(group.rows.enumerated()), id: \.offset) { _, row in
                        CoverageRow(row: row, onSelect: {
                            onSelect(.coverageDetail(target: row.targetID, wsID: row.wsID))
                        })
                        Divider()
                    }
                }
            }
        }
        .panelStyle(.panel)
    }

    private var columns: [SortableColumn<CoverageSortKey>] {
        [
            SortableColumn(key: .target, label: "Target", width: nil),
            SortableColumn(key: .assertions, label: "Assertions", width: CoverageLayout.assertions, alignment: .trailing),
            SortableColumn(key: .catalog, label: "Catalog", width: CoverageLayout.catalog, alignment: .trailing, firstTapAscending: true),
            SortableColumn(key: .findings, label: "Findings", width: CoverageLayout.findings, alignment: .trailing),
        ]
    }

    /// Groups by `project`, orders the groups alphabetically, and sorts each
    /// group's rows independently by the active `sort` — a target in one
    /// project never gets interleaved with another's regardless of which
    /// column is active.
    private static func groupedByProject(
        _ rows: [APICoverageSummary], sort: ListSort<CoverageSortKey>
    ) -> [(project: String, rows: [APICoverageSummary])] {
        let grouped = Dictionary(grouping: rows, by: \.project)
        return grouped.keys
            .sorted { $0.localizedCaseInsensitiveCompare($1) == .orderedAscending }
            .map { project in (project, sortRows(grouped[project] ?? [], sort: sort, value: sortValue)) }
    }

    private static func sortValue(_ row: APICoverageSummary, _ key: CoverageSortKey) -> ListSortValue {
        switch key {
        case .target: .text(row.targetID)
        case .assertions: .number(Double(row.assertionLines))
        case .catalog: .number(row.hasCatalog ? 1 : 0)
        case .findings: .number(Double(row.findings))
        }
    }
}

// MARK: - Coverage row

private struct CoverageRow: View {
    let row: APICoverageSummary
    let onSelect: () -> Void

    var body: some View {
        securityRowTapModifiers(
            HStack(spacing: 0) {
                Text(row.targetID)
                    .foregroundStyle(Color.rupuInk)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.trailing, 8)

                Text(Fmt.count(row.assertionLines))
                    .font(.dataMono(11))
                    .foregroundStyle(Color.rupuInk)
                    .frame(width: CoverageLayout.assertions, alignment: .trailing)
                    .padding(.trailing, 8)

                catalogCell
                    .frame(width: CoverageLayout.catalog, alignment: .trailing)
                    .padding(.trailing, 8)

                Text(Fmt.count(row.findings))
                    .font(.dataMono(11))
                    .foregroundStyle(row.findings > 0 ? Color.rupuWarn : Color.rupuDim)
                    .frame(width: CoverageLayout.findings, alignment: .trailing)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8),
            onSelect: onSelect
        )
    }

    @ViewBuilder
    private var catalogCell: some View {
        if row.hasCatalog {
            Badge("catalog", tone: Color.rupuBrand)
        } else {
            Text("—").font(.dataMono(10)).foregroundStyle(Color.rupuMute)
        }
    }
}
