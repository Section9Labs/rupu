import SwiftUI
import AppKit
import RupuStore
import RupuDesign

/// Sortable keys for the dedicated Workflows kind table (perf & interaction
/// arc, Plan 5 Task 4) — `ListSort`'s generic `Key`, same "screen-owned, not
/// a shared base type" convention every other Phase 5A/5B sortable table
/// (`FindingsSortKey` et al.) already follows. Column set ported verbatim
/// from the web's `WORKFLOW_RUN_COLUMNS` (`pages/runs/WorkflowRuns.tsx`):
/// Status · Workflow(subject) · Run · Trigger · Host · In · Out · Cached ·
/// Cost · Turns · Duration · Started · actions.
public enum WorkflowRunsSortKey: Hashable, CaseIterable, Sendable {
    case status, workflow, trigger, host, inTokens, outTokens, cachedTokens, cost, turns, duration, started
}

private enum WorkflowRunsLayout {
    static let trigger: CGFloat = 92
}

/// The Workflows kind page's dedicated table (route: `.activity(.workflows)`)
/// — replaces the merged `ActivityTable` + kind-picker combination the old
/// Activity screen showed for this kind (matt's direct feedback: no more
/// generic table with a kind switcher). Reuses `SortableHeaderRow`/
/// `ListSort`/`sortRows` (the same generic sortable-table machinery
/// `RupuSecurity`/`RupuFleet`/`RupuLibrary`/`RupuUsage` already standardize
/// on) rather than `RupuStore.ActivitySort` — that type is `ActivityStore`'s
/// own single shared sort descriptor for the merged `.all` view (`Activity
/// Table`), and giving every kind page its OWN local sort avoids fighting
/// over that one shared field (a Sessions-kind requirement below — "no
/// initial sort" — would be impossible to express through a store-wide
/// descriptor another kind page's toggle can also mutate).
struct WorkflowRunsTable: View {
    let rows: [ActivityRow]
    let store: ActivityStore
    let backend: BackendController
    let onSelect: (ActivityRow) -> Void

    @State private var sort = ListSort<WorkflowRunsSortKey>(key: .started, ascending: false)
    @State private var query = ""
    @State private var debouncedQuery = ""

    var body: some View {
        let _ = RenderMeter.tick("WorkflowRunsTable")
        let now = Date()
        let sorted = sortRows(rows, sort: sort, value: Self.sortValue)
        let q = debouncedQuery.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        let visible = q.isEmpty ? sorted : sorted.filter { Self.matches($0, query: q) }
        VStack(alignment: .leading, spacing: 8) {
            KindTableSearchField(placeholder: "Find workflows…", query: $query)
            VStack(alignment: .leading, spacing: 0) {
                SortableHeaderRow(columns: columns, sort: $sort)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                Divider()
                List {
                    ForEach(visible) { row in
                        WorkflowRunRowView(row: row, store: store, backend: backend, now: now, onSelect: onSelect)
                            .listRowBackground(row.status == .awaiting ? Color.status(.awaiting).opacity(0.04) : .clear)
                            .listRowSeparator(.visible)
                            .listRowInsets(EdgeInsets(top: 0, leading: 0, bottom: 0, trailing: 0))
                    }
                    KindTableFooter(
                        visibleCount: visible.count,
                        loadedCount: store.loadedCount,
                        hasMore: store.hasMore,
                        isLoadingMore: store.isLoadingMore,
                        isSearchActive: !q.isEmpty || !store.statusFilter.isEmpty,
                        onLoadMore: { Task { await store.loadMore() } }
                    )
                    .listRowSeparator(.hidden)
                    .listRowInsets(EdgeInsets())
                }
                .listStyle(.plain)
                .scrollContentBackground(.hidden)
            }
            .panelStyle(.panel)
        }
        .debouncedKindTableSearch(query: query, into: $debouncedQuery)
    }

    /// Find — case-insensitive substring across the fields the web's own
    /// `WorkflowRuns.tsx` Find box matches (`[r.workflow_name, r.id,
    /// r.host_id]`): workflow name (`subject`), run id (`id`), host.
    static func matches(_ row: ActivityRow, query: String) -> Bool {
        [row.subject, row.id, row.host].contains { $0.lowercased().contains(query) }
    }

    private var columns: [SortableColumn<WorkflowRunsSortKey>] {
        [
            SortableColumn(key: .status, label: "Status", width: KindTableLayout.status),
            SortableColumn(key: .workflow, label: "Workflow", width: nil),
            SortableColumn(key: nil, label: "Run", width: KindTableLayout.run, alignment: .trailing),
            SortableColumn(key: .trigger, label: "Trigger", width: WorkflowRunsLayout.trigger, alignment: .trailing, firstTapAscending: true),
            SortableColumn(key: .host, label: "Host", width: KindTableLayout.host, alignment: .trailing, firstTapAscending: true),
            SortableColumn(key: .inTokens, label: "In", width: KindTableLayout.tokenCell, alignment: .trailing),
            SortableColumn(key: .outTokens, label: "Out", width: KindTableLayout.tokenCell, alignment: .trailing),
            SortableColumn(key: .cachedTokens, label: "Cached", width: KindTableLayout.tokenCell, alignment: .trailing),
            SortableColumn(key: .cost, label: "Cost", width: KindTableLayout.cost, alignment: .trailing),
            SortableColumn(key: .turns, label: "Turns", width: KindTableLayout.turns, alignment: .trailing),
            SortableColumn(key: .duration, label: "Duration", width: KindTableLayout.duration, alignment: .trailing),
            SortableColumn(key: .started, label: "Started", width: KindTableLayout.started, alignment: .trailing),
            SortableColumn(key: nil, label: "", width: KindTableLayout.actions, alignment: .trailing),
        ]
    }

    /// Pure row→sort-value mapper — the only place this table decides what
    /// each column sorts on. `TDD`'d in `WorkflowRunsTableTests`.
    static func sortValue(_ row: ActivityRow, _ key: WorkflowRunsSortKey) -> ListSortValue {
        switch key {
        case .status: .text(row.status.displayLabel)
        case .workflow: .text(row.subject)
        case .trigger: .text(row.trigger)
        case .host: .text(row.host)
        case .inTokens: .number(row.inputTokens.map { Double($0) })
        case .outTokens: .number(row.outputTokens.map { Double($0) })
        case .cachedTokens: .number(row.cachedTokens.map { Double($0) })
        case .cost: .number(row.costUSD)
        case .turns: .number(row.turns.map { Double($0) })
        case .duration: .number(row.durationMS.map { Double($0) })
        case .started: .date(row.startedAt)
        }
    }
}

/// Trigger tone mapping ported from the web's `TRIGGER_CHIP_CLS`
/// (`WorkflowRuns.tsx`): manual is neutral/outline, cron reads brand-tinted,
/// event reads info-tinted. An unrecognized trigger string (any value other
/// than these three) falls back to the neutral tone rather than guessing.
enum TriggerTone {
    static func color(_ trigger: String) -> Color {
        switch trigger {
        case "cron": Color.rupuBrand
        case "event": Color.rupuInfo
        default: Color.rupuMute
        }
    }
}

private struct WorkflowRunRowView: View {
    let row: ActivityRow
    let store: ActivityStore
    let backend: BackendController
    let now: Date
    let onSelect: (ActivityRow) -> Void

    private var isClickable: Bool { row.navigation != .none }

    var body: some View {
        HStack(spacing: 0) {
            KindTableStatusCell(status: row.status)
                .frame(width: KindTableLayout.status, alignment: .leading)
                .padding(.trailing, 8)
            Text(row.subject)
                .foregroundStyle(Color.rupuInk)
                .lineLimit(1)
                .truncationMode(.tail)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.trailing, 8)
                .help(row.subject)
            Text(KindTableFormat.shortID(row.id))
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuDim)
                .frame(width: KindTableLayout.run, alignment: .trailing)
                .padding(.trailing, 8)
            Badge(row.trigger ?? "—", tone: row.trigger.map(TriggerTone.color) ?? Color.rupuMute)
                .frame(width: WorkflowRunsLayout.trigger, alignment: .trailing)
                .padding(.trailing, 8)
            Text(row.host)
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuDim)
                .lineLimit(1)
                .frame(width: KindTableLayout.host, alignment: .trailing)
                .padding(.trailing, 8)
            Text(KindTableFormat.count(row.inputTokens))
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuDim)
                .frame(width: KindTableLayout.tokenCell, alignment: .trailing)
                .padding(.trailing, 8)
            Text(KindTableFormat.count(row.outputTokens))
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuDim)
                .frame(width: KindTableLayout.tokenCell, alignment: .trailing)
                .padding(.trailing, 8)
            Text(KindTableFormat.count(row.cachedTokens))
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuDim)
                .frame(width: KindTableLayout.tokenCell, alignment: .trailing)
                .padding(.trailing, 8)
            Text(Fmt.cost(row.costUSD))
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuInk)
                .frame(width: KindTableLayout.cost, alignment: .trailing)
                .padding(.trailing, 8)
            Text(KindTableFormat.count(row.turns))
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuInk)
                .frame(width: KindTableLayout.turns, alignment: .trailing)
                .padding(.trailing, 8)
            Text(row.durationMS.map { Fmt.duration(ms: $0) } ?? "—")
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuInk)
                .frame(width: KindTableLayout.duration, alignment: .trailing)
                .padding(.trailing, 8)
            Text(startedLabel)
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuDim)
                .frame(width: KindTableLayout.started, alignment: .trailing)
            kindTableAwaitingActionsCell(row, store: store, backend: backend)
                .frame(width: KindTableLayout.actions, alignment: .trailing)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .contentShape(Rectangle())
        .onTapGesture {
            guard isClickable else { return }
            onSelect(row)
        }
        .onHover { hovering in
            guard isClickable else { return }
            if hovering { NSCursor.pointingHand.push() } else { NSCursor.pop() }
        }
    }

    private var startedLabel: String {
        guard let date = row.startedAt else { return "—" }
        return KindTableFormat.relative.localizedString(for: date, relativeTo: now)
    }
}
