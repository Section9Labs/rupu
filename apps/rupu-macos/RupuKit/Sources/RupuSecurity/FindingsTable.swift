import SwiftUI
import RupuAPI
import RupuStore
import RupuDesign

/// Sortable keys for the Security screen's Findings table. `ListSort`'s
/// generic `Key` — screen-owned, same "not a shared base type" convention
/// `ListSort.swift`'s doc comment documents.
public enum FindingsSortKey: Hashable, CaseIterable, Sendable {
    case severity, title, project, target, workflow
}

private enum FindingsLayout {
    static let severity: CGFloat = 64
    static let project: CGFloat = 96
    static let target: CGFloat = 104
    static let workflow: CGFloat = 128
}

/// The Findings tab's content: a severity summary strip (straight off
/// `APIFindingsSummary`, no client-side recount — same "no fake data"
/// posture `FindingsTabContent`/`ProjectFindingsTabContent` already take)
/// above a sortable table, one row per finding across every registered
/// workspace.
///
/// **Rows are deliberately non-navigating.** `APIFinding` (the `GET /api/
/// findings` row shape — see that type's doc comment) carries `wsID`/
/// `project`/`targetID`/`workflowName`, but no `runID`/run linkage at all:
/// `FindingOut` on the Rust side (`crates/rupu-cp/src/api/findings.rs`) has
/// no run association to give one — a finding is declared against a
/// coverage target, not a specific run. Wiring a tap here would either
/// silently no-op (dead affordance) or navigate somewhere this data doesn't
/// actually point, so this table renders no row-tap chrome (no hover
/// cursor, no `onTapGesture`) at all — contrast `CoverageList.swift`'s
/// `CoverageRow`, which genuinely does have a target/wsID pair to navigate
/// with.
struct FindingsTabView: View {
    let findings: BlockState<APIFindings>
    @Binding var sort: ListSort<FindingsSortKey>

    var body: some View {
        Group {
            switch findings {
            case .loading:
                securityLoadingBlock()
            case .failed(let message):
                securityFailedBlock(message, subject: "findings")
            case .empty:
                securityEmptyBlock("No findings")
            case .content(let value):
                VStack(alignment: .leading, spacing: 12) {
                    summaryStrip(value.summary)
                    if value.findings.isEmpty {
                        securityEmptyBlock("No findings")
                    } else {
                        table(value.findings)
                    }
                }
            }
        }
    }

    // MARK: - Summary strip

    private func summaryStrip(_ summary: APIFindingsSummary) -> some View {
        HStack(spacing: 14) {
            severityFigure("Critical", summary.critical, .crit)
            severityFigure("High", summary.high, .high)
            severityFigure("Medium", summary.medium, .med)
            severityFigure("Low", summary.low, .low)
            severityFigure("Info", summary.info, .info)
            Spacer(minLength: 0)
        }
        .padding(12)
        .panelStyle(.panel)
    }

    private func severityFigure(_ label: String, _ count: Int, _ severity: Severity) -> some View {
        HStack(spacing: 6) {
            Circle()
                .fill(Color.severity(severity))
                .frame(width: 6, height: 6)
            Text(label).font(.metaText).foregroundStyle(Color.rupuMute)
            Text("\(count)").font(.dataMono(13)).foregroundStyle(Color.rupuInk)
        }
        .opacity(count == 0 ? 0.4 : 1)
    }

    // MARK: - Table

    private func table(_ rows: [APIFinding]) -> some View {
        let sorted = sortRows(rows, sort: sort, value: Self.sortValue)
        return VStack(alignment: .leading, spacing: 0) {
            SortableHeaderRow(columns: columns, sort: $sort)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
            Divider()
            VStack(spacing: 0) {
                ForEach(Array(sorted.enumerated()), id: \.offset) { _, row in
                    FindingRow(finding: row)
                    Divider()
                }
            }
        }
        .panelStyle(.panel)
    }

    private var columns: [SortableColumn<FindingsSortKey>] {
        [
            // A blank leading column reserves the 2px severity-edge gutter
            // `FindingRow` paints, so the header's own text columns line up
            // with the rows beneath them.
            SortableColumn(key: nil, label: "", width: 10),
            SortableColumn(key: .severity, label: "Severity", width: FindingsLayout.severity, alignment: .trailing),
            // The ONE flexible, truncating column — every other column is
            // fixed-width (`FindingsLayout`), same "exactly one `width: nil`
            // column" contract `SortableColumn`'s doc comment documents.
            SortableColumn(key: .title, label: "Title", width: nil),
            SortableColumn(key: .project, label: "Project", width: FindingsLayout.project, alignment: .trailing, firstTapAscending: true),
            SortableColumn(key: .target, label: "Target", width: FindingsLayout.target, alignment: .trailing, firstTapAscending: true),
            SortableColumn(key: .workflow, label: "Workflow", width: FindingsLayout.workflow, alignment: .trailing, firstTapAscending: true),
        ]
    }

    /// `.severity` sorts by rank (critical highest) rather than text, so the
    /// column reads "most severe first" under its trailing-aligned,
    /// descending-by-default heuristic — the same "numeric so it orders by
    /// meaning, not alphabetically" reasoning `WorkersSortKey.status`
    /// (`FleetScreen`) already applies to a derived non-numeric column.
    private static func sortValue(_ row: APIFinding, _ key: FindingsSortKey) -> ListSortValue {
        switch key {
        case .severity: .number(Double(severityRank(row.severity)))
        case .title: .text(row.summary)
        case .project: .text(row.project)
        case .target: .text(row.targetID)
        case .workflow: .text(row.workflowName)
        }
    }

    private static func severityRank(_ wire: String) -> Int {
        switch Severity(wireString: wire) {
        case .crit: 4
        case .high: 3
        case .med: 2
        case .low: 1
        case .info: 0
        }
    }
}

// MARK: - Finding row

private struct FindingRow: View {
    let finding: APIFinding

    private var severity: Severity { Severity(wireString: finding.severity) }

    var body: some View {
        HStack(spacing: 0) {
            Rectangle()
                .fill(Color.severity(severity))
                .frame(width: 2)
                .frame(width: 10, alignment: .leading)
                .padding(.trailing, 8)

            Text(severityLabel)
                .font(.dataMono(10))
                .foregroundStyle(Color.severity(severity))
                .frame(width: FindingsLayout.severity, alignment: .trailing)
                .padding(.trailing, 8)

            Text(finding.summary)
                .foregroundStyle(Color.rupuInk)
                .lineLimit(1)
                .truncationMode(.tail)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.trailing, 8)

            Text(finding.project)
                .font(.dataMono(10))
                .foregroundStyle(Color.rupuMute)
                .lineLimit(1)
                .truncationMode(.tail)
                .frame(width: FindingsLayout.project, alignment: .trailing)
                .padding(.trailing, 8)

            Text(finding.targetID)
                .font(.dataMono(10))
                .foregroundStyle(Color.rupuDim)
                .lineLimit(1)
                .truncationMode(.tail)
                .frame(width: FindingsLayout.target, alignment: .trailing)
                .padding(.trailing, 8)

            Text(finding.workflowName ?? "—")
                .font(.dataMono(10))
                .foregroundStyle(Color.rupuDim)
                .lineLimit(1)
                .truncationMode(.tail)
                .frame(width: FindingsLayout.workflow, alignment: .trailing)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }

    private var severityLabel: String {
        switch severity {
        case .crit: "Critical"
        case .high: "High"
        case .med: "Medium"
        case .low: "Low"
        case .info: "Info"
        }
    }
}
