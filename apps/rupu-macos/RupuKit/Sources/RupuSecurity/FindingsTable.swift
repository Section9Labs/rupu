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

/// Maps a finding's `declaredBy.surface`/`runID` to the route its row
/// should navigate to, if any. Pure, and the sole place this decision is
/// made — see `FindingsTabView`'s doc comment for the full per-surface
/// rationale (why `workflow`/`autoflow` route and `agent`/`session` don't).
///
/// An empty `runID` (the `APIFinding` memberwise init's all-empty
/// `declaredBy` default, for call sites built before this field existed —
/// see that type's doc comment) never routes either, regardless of
/// `surface` — an empty id is not a real run to navigate to.
func findingNavigationRoute(surface: String, runID: String) -> Route? {
    guard !runID.isEmpty else { return nil }
    switch surface {
    case "workflow", "autoflow":
        return .runDetail(id: runID, host: nil)
    default:
        // "agent"/"session" (and any future/unrecognized surface value) —
        // never a guessed route for data this client can't actually
        // resolve. See the tracked follow-up in `FindingsTabView`'s doc
        // comment.
        return nil
    }
}

/// The Findings tab's content: a severity summary strip (straight off
/// `APIFindingsSummary`, no client-side recount — same "no fake data"
/// posture `FindingsTabContent`/`ProjectFindingsTabContent` already take)
/// above a sortable table, one row per finding across every registered
/// workspace.
///
/// **Rows navigate where honest, per `declaredBy.surface` — not uniformly.**
/// `APIFinding.declaredBy` (`FindingOut`'s flattened `FindingRecord.
/// declared_by: Attribution { run_id, model, surface }` — see `APIFinding`'s
/// own doc comment; a first pass at this table wrongly believed no run
/// linkage existed at all and shipped every row non-navigating on that false
/// premise) IS real run linkage. But not every `surface` value resolves to a
/// route `RunDetailScreen` can actually serve:
/// - `workflow`/`autoflow` → `declared_by.run_id` is an orchestrator run id,
///   exactly what `GET /api/runs/:id` (`RunDetailScreen`'s data source)
///   expects. `host: nil` — the findings registry (`rupu-coverage`'s ledger)
///   is local-only, same as every other `/api/coverage`/`/api/findings`
///   route in this client (no `?host=` fan-out anywhere on `CPClient` for
///   these), so `nil` here is truthful, not a punt.
/// - `agent`/`session` → the run id is an agent-run or session-scoped id,
///   which structurally 404s against `/api/runs/:id` (the Phase 2 "standalone
///   agent run" lesson `AppModel.Route.agentRunDetail`'s own doc comment
///   documents) — and `APIFinding` carries neither the `transcriptPath` an
///   `.agentRunDetail` route needs nor a `sessionID` a `.sessionDetail` route
///   needs, only `declared_by.run_id`. Wiring these to `.runDetail` would
///   reintroduce exactly the dead-end-404 bug Phase 2 fixed; they stay
///   non-navigating instead. **Tracked follow-up, not solved here**: closing
///   this gap needs a richer server-side row (a resolved surface-specific
///   route, or the missing transcript/session fields), not a client-side
///   guess.
///
/// `findingNavigationRoute(surface:runID:)` is the pure, tested mapping this
/// table's rows delegate to — a row only gets tap chrome (hover cursor,
/// `onTapGesture`) when it returns non-`nil`.
struct FindingsTabView: View {
    let findings: BlockState<APIFindings>
    @Binding var sort: ListSort<FindingsSortKey>
    let onSelect: (Route) -> Void

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
                    FindingRow(
                        finding: row,
                        route: findingNavigationRoute(surface: row.declaredBy.surface, runID: row.declaredBy.runID),
                        onSelect: onSelect
                    )
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
    /// `nil` when `findingNavigationRoute(surface:runID:)` couldn't resolve
    /// one for this row's `declaredBy` — see `FindingsTabView`'s doc
    /// comment. Only a non-`nil` route gets tap chrome.
    let route: Route?
    let onSelect: (Route) -> Void

    private var severity: Severity { Severity(wireString: finding.severity) }

    var body: some View {
        if let route {
            securityRowTapModifiers(rowContent, onSelect: { onSelect(route) })
        } else {
            rowContent
        }
    }

    private var rowContent: some View {
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
