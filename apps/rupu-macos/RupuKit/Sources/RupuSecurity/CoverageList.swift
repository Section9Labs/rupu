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
///
/// **Contained + lazy (review fix, same root cause `FindingsTable.swift`'s
/// table hit — see that type's "Contained, lazy, windowed" doc-comment
/// section for the full incident)**: this table had the identical missing-
/// `ScrollView`/eager-`VStack` shape, checked for and fixed alongside it.
/// No explicit numeric cap (`FindingsWindow`'s counterpart) was added here,
/// deliberately: coverage targets are grouped by project, and a flat "first
/// 200" cut would either truncate a project's targets mid-group or require
/// group-aware truncation logic this table doesn't have proven scale to
/// justify yet (unlike Findings' confirmed 385+-row workspace) — tracked as
/// a follow-up if a fleet's registered-target count ever grows large enough
/// to need it. The `LazyVStack`'s virtualization here is per-project-group,
/// not per-row (each group's own `ForEach` is realized whole once that
/// group scrolls into range) — acceptable at today's per-project target
/// counts, which run far below the row count that broke Findings.
///
/// **Rows keyed by `APICoverageSummary.rowID`, not a positional offset
/// (review fix 3 — a second live-GUI bug, distinct from the containment
/// fix above)**: the inner row `ForEach` originally used `id: \.offset`
/// (from `Array(group.rows.enumerated())`), reset to `0` at the start of
/// EVERY project group's own closure invocation. Nested inside the outer
/// `ForEach(groups, id: \.project)`, itself inside a `LazyVStack`, that
/// per-group-local numbering is not a safe view identity: SwiftUI's
/// diffing for a `LazyVStack`'s flattened child list does not reliably
/// re-scope a nested `ForEach`'s ids per outer-closure invocation the way
/// a naive read of "nested `ForEach` = nested identity scope" would
/// suggest — two rows in DIFFERENT groups that happen to land on the same
/// local offset (e.g. both the first row of their own group, offset `0`)
/// can collide, and SwiftUI silently keeps only the first one, leaving
/// reserved-but-blank space where every colliding duplicate should have
/// rendered. On a real fleet this manifested as 8-9 project headers with
/// almost every row beneath them blank except a couple of survivors.
/// `APICoverageSummary.rowID` (`wsID/targetID`, see that property's own
/// doc comment — it was ALSO the right fix for the reverse-but-related
/// hazard this type's own top doc comment already warned about:
/// `targetID` genuinely does collide across workspaces on a real fleet)
/// is what the `ForEach` keys off now instead — content-derived and
/// globally unique across the ENTIRE table, not scoped to (and therefore
/// not vulnerable to colliding across) any one group's local numbering.
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
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    /// Header pinned outside the scroll region, same "sortable column
    /// labels shouldn't scroll away" reasoning `FindingsTable.swift`'s
    /// table gives for its own pinned header.
    private func table(_ rows: [APICoverageSummary]) -> some View {
        let groups = Self.groupedByProject(rows, sort: sort)
        return VStack(alignment: .leading, spacing: 0) {
            SortableHeaderRow(columns: columns, sort: $sort)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
            Divider()
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(groups, id: \.project) { group in
                        Eyebrow(group.project)
                            .padding(.horizontal, 12)
                            .padding(.top, 10)
                            .padding(.bottom, 4)
                        ForEach(group.rows, id: \.rowID) { row in
                            CoverageRow(row: row, onSelect: {
                                onSelect(.coverageDetail(target: row.targetID, wsID: row.wsID))
                            })
                            Divider()
                        }
                    }
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .panelStyle(.panel)
    }

    private var columns: [SortableColumn<CoverageSortKey>] {
        [
            SortableColumn(key: .target, label: "Target", width: nil),
            SortableColumn(key: .assertions, label: "Assertions", width: CoverageLayout.assertions, alignment: .trailing),
            // No `firstTapAscending` override (review fix — an earlier draft
            // had one, unintentionally copied from `FindingsTable.swift`'s
            // TEXT columns): `.catalog`'s sort value is `.number(hasCatalog
            // ? 1 : 0)`, not text, so the default trailing-aligned heuristic
            // (descending-first) is already correct here — same as the
            // un-overridden `.assertions`/`.findings` columns either side of
            // it — putting catalog-having targets first on the first tap.
            SortableColumn(key: .catalog, label: "Catalog", width: CoverageLayout.catalog, alignment: .trailing),
            SortableColumn(key: .findings, label: "Findings", width: CoverageLayout.findings, alignment: .trailing),
        ]
    }

    /// Groups by `project`, orders the groups alphabetically, and sorts each
    /// group's rows independently by the active `sort` — a target in one
    /// project never gets interleaved with another's regardless of which
    /// column is active.
    ///
    /// `internal`, not `private` (review fix) — reached directly from
    /// `RupuSecurityTests` via `@testable import RupuSecurity` for the
    /// "uniqueness seam" test: the actual `ForEach`/`LazyVStack` rendering
    /// isn't unit-testable, but this pure transform — the thing that
    /// actually feeds the `ForEach` — is, and is exactly where a
    /// same-`targetID`-different-`wsID` pair needs to be proven to survive
    /// intact rather than merge or drop.
    static func groupedByProject(
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
