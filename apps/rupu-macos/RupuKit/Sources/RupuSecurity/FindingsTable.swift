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

/// The Findings table's client-side render cap (review fix — GUI validation
/// on a real 385+-row workspace found the un-windowed table blowing past its
/// scroll container; see `FindingsTabView`'s "Contained, lazy, windowed"
/// doc-comment section for the full incident). `GET /api/findings` has no
/// server-side pagination — every finding for every registered workspace
/// comes back in one response — so "windowing" here is purely a render-time
/// slice over data that's already fully in memory, not a second fetch the
/// way `ProjectDetailStore`'s `windowSize`/`showAllLimit` are (that pair
/// caps an actual re-fetch at a bigger limit; this one never re-fetches
/// anything). Pure and tested (`FindingsWindowTests.swift`,
/// `RupuSecurityTests`) — `window(_:showingAll:)` and `WindowFooter.resolve`
/// below have no `View` dependency.
enum FindingsWindow {
    static let size = 200

    /// `sorted` MUST already be sort-ordered — windowing always applies
    /// AFTER `sortRows`, so the visible top-N respects whichever column/
    /// direction is currently active rather than the server's insertion
    /// order. `showingAll` bypasses the cap entirely: once the footer
    /// button is tapped, every already-fetched row renders — safe because
    /// the row list is `LazyVStack`-backed (only on-screen rows actually
    /// instantiate), the same virtualization `ActivityTable`'s `List`
    /// gets for free.
    static func window<T>(_ sorted: [T], showingAll: Bool) -> [T] {
        showingAll ? sorted : Array(sorted.prefix(size))
    }
}

/// The windowed table's footer state — `.hidden` when there's nothing
/// beyond the first `FindingsWindow.size` rows to reveal (or it's already
/// revealed), `.button` otherwise. Only two states (unlike `ProjectDetail
/// Screen`'s three-state `ShowAllFooterState`, which also has a `.note`
/// "capped short of the real total" state): there is no unreachable data
/// here — every finding is already client-side, so tapping the button can
/// always show literally everything, never just a bigger-but-still-partial
/// page.
enum FindingsWindowFooterState: Equatable {
    case hidden
    case button(label: String)
}

enum FindingsWindowFooter {
    static func resolve(total: Int, showingAll: Bool) -> FindingsWindowFooterState {
        guard !showingAll, total > FindingsWindow.size else { return .hidden }
        return .button(label: "Show all \(Fmt.count(total)) findings")
    }
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
///
/// **Contained, lazy, windowed (review fix)**: the first cut of this table
/// had no `ScrollView` anywhere in its hierarchy at all — just a plain
/// `VStack` wrapping a plain `ForEach`. A `VStack` reports its children's
/// full natural (ideal) size upward regardless of what its ancestors
/// propose, unlike a `ScrollView`, which accepts the proposed size and
/// clips/scrolls its content internally — so on a real workspace with 385+
/// findings, that `VStack` (with every row eagerly instantiated) blew past
/// its window's bounds top and bottom, scrolling did nothing (there was no
/// scroll region to scroll), and the oversized detail-pane content broke
/// `RootView`'s `NavigationSplitView` layout badly enough that the sidebar
/// rail painted blank. The fix has two parts, same "make the scroll region
/// own the space" idiom `ProjectDetailScreen`'s per-tab `ScrollView`s
/// already establish:
/// - The row list (not the `SortableHeaderRow`, which stays pinned above
///   it — better for a 385-row *sortable* table than letting column labels
///   scroll away, the one deliberate deviation from `ProjectDetailScreen`'s
///   "everything scrolls together" shape) now lives in a real `ScrollView`
///   over a `LazyVStack`, matching the virtualization `ActivityTable`'s
///   `List` gets natively — only on-screen rows actually instantiate.
/// - `FindingsWindow`/`FindingsWindowFooter` additionally cap the render at
///   `FindingsWindow.size` (200) rows until the footer's "Show all N
///   findings" button is tapped — belt-and-suspenders on top of the lazy
///   container, matching this app's established capped-list idiom
///   (`ProjectDetailStore`'s Runs/Sessions windowing) rather than silently
///   relying on virtualization alone for an unbounded render.
struct FindingsTabView: View {
    let findings: BlockState<APIFindings>
    @Binding var sort: ListSort<FindingsSortKey>
    let onSelect: (Route) -> Void

    /// See the type doc comment's "Contained, lazy, windowed" section.
    /// Deliberately local `@State`, not routed through `SecurityStore` — a
    /// pure render-time cap over data the store already fully holds, not
    /// fetch state.
    @State private var showAll = false

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
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
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

    /// Header pinned outside the scroll region (see the type doc comment's
    /// "Contained, lazy, windowed" section); windowing is applied AFTER
    /// `sortRows`, so the visible top-N always respects whichever column/
    /// direction is currently active.
    private func table(_ rows: [APIFinding]) -> some View {
        let sorted = sortRows(rows, sort: sort, value: Self.sortValue)
        let windowed = FindingsWindow.window(sorted, showingAll: showAll)
        return VStack(alignment: .leading, spacing: 0) {
            SortableHeaderRow(columns: columns, sort: $sort)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
            Divider()
            ScrollView {
                LazyVStack(spacing: 0) {
                    ForEach(Array(windowed.enumerated()), id: \.offset) { _, row in
                        FindingRow(
                            finding: row,
                            route: findingNavigationRoute(surface: row.declaredBy.surface, runID: row.declaredBy.runID),
                            onSelect: onSelect
                        )
                        Divider()
                    }
                    windowFooter(total: sorted.count)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .panelStyle(.panel)
    }

    @ViewBuilder
    private func windowFooter(total: Int) -> some View {
        switch FindingsWindowFooter.resolve(total: total, showingAll: showAll) {
        case .hidden:
            EmptyView()
        case .button(let label):
            Button(label) { showAll = true }
                .buttonStyle(.plain)
                .font(.metaText.weight(.semibold))
                .foregroundStyle(Color.rupuBrand)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 10)
        }
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
