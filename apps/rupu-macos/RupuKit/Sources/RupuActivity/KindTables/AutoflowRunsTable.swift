import SwiftUI
import AppKit
import RupuStore
import RupuDesign

/// Sortable keys for the dedicated Autoflows kind page's Runs sub-table
/// (events) — column set ported verbatim from the web's `EVENT_COLUMNS`
/// (`pages/runs/AutoflowRuns.tsx`): chevron(expand) · Status ·
/// Workflow(subject) · Run · Event(kind badge) · Issue Ref · Worker · Host ·
/// In · Out · Cached · Cost · Turns · Duration · Started · actions.
///
/// **Cycles is a separate sub-table, not built here** (matt's mid-task
/// addendum): the web's Autoflows page actually has three sub-tables — Runs
/// (this one) / Cycles / Claims — but `GET /api/runs/autoflows` (the
/// cycle-rows endpoint) has no client-side model yet (`CPClient` only wraps
/// `autoflowEvents`/`autoflowClaims`), and adding a third fetch source +
/// wire type + store slice on top of everything else this task already
/// covers would risk rushing all of it. Named explicitly as the deferred
/// remainder in the Task 4 report — Runs/Claims (this table + the existing
/// `ClaimsTable`) ship; Cycles does not, this pass.
public enum AutoflowRunsSortKey: Hashable, CaseIterable, Sendable {
    case status, workflow, event, issueRef, worker, host, inTokens, outTokens, cachedTokens, cost, turns, duration, started
}

private enum AutoflowRunsLayout {
    static let chevron: CGFloat = 20
    static let event: CGFloat = 128
    static let issueRef: CGFloat = 96
    static let worker: CGFloat = 96
}

/// The Autoflows kind page's Runs sub-table (the default view under the
/// existing Runs/Claims `AutoflowsSubTab` toggle — `ActivityScreen` still
/// owns that toggle; this only replaces what used to render for `.runs`).
/// See `WorkflowRunsTable`'s doc comment for why this owns a local
/// `ListSort` rather than sharing `ActivityStore.sort`.
struct AutoflowRunsTable: View {
    let rows: [ActivityRow]
    let store: ActivityStore
    let backend: BackendController
    let onSelect: (ActivityRow) -> Void

    @State private var sort = ListSort<AutoflowRunsSortKey>(key: .started, ascending: false)
    @State private var expandedIDs: Set<String> = []
    @State private var query = ""
    @State private var debouncedQuery = ""

    var body: some View {
        let _ = RenderMeter.tick("AutoflowRunsTable")
        let now = Date()
        let sorted = sortRows(rows, sort: sort, value: Self.sortValue)
        let q = debouncedQuery.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        let visible = q.isEmpty ? sorted : sorted.filter { Self.matches($0, query: q) }
        VStack(alignment: .leading, spacing: 8) {
            KindTableSearchField(placeholder: "Find autoflow activity…", query: $query)
            VStack(alignment: .leading, spacing: 0) {
                SortableHeaderRow(columns: columns, sort: $sort)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                Divider()
                List {
                    ForEach(visible) { row in
                        VStack(alignment: .leading, spacing: 0) {
                            AutoflowRunRowView(
                                row: row, store: store, backend: backend, now: now,
                                isExpanded: expandedIDs.contains(row.id),
                                onToggleExpand: { toggleExpand(row.id) },
                                onSelect: onSelect
                            )
                            if expandedIDs.contains(row.id), let detail = row.detail {
                                Text(detail)
                                    .font(.dataMono(11))
                                    .foregroundStyle(Color.status(.failed))
                                    .padding(.horizontal, 12)
                                    .padding(.bottom, 8)
                                    .padding(.leading, AutoflowRunsLayout.chevron + 8)
                            }
                        }
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
    /// `AutoflowRuns.tsx` `matchesAutoflowQuery` call matches for events:
    /// `[e.workflow, KIND_LABEL[e.kind], e.run_id, e.issue_display_ref,
    /// e.host_id, e.worker_name]`. `subject` already carries `workflow ??
    /// kind` (see `ActivityRow.init(_: APIAutoflowEventRow)`) and
    /// `AutoflowEventBadge.label` is this table's own `KIND_LABEL`
    /// equivalent, so both are matched against directly rather than
    /// re-deriving the raw `workflow` field separately.
    static func matches(_ row: ActivityRow, query: String) -> Bool {
        var fields = [row.subject, AutoflowEventBadge.label(row.eventKind ?? ""), row.host]
        if let issueRef = row.issueRef { fields.append(issueRef) }
        if let worker = row.worker { fields.append(worker) }
        if case .run(let runID, _) = row.navigation { fields.append(runID) }
        return fields.contains { $0.lowercased().contains(query) }
    }

    private func toggleExpand(_ id: String) {
        if expandedIDs.contains(id) {
            expandedIDs.remove(id)
        } else {
            expandedIDs.insert(id)
        }
    }

    private var columns: [SortableColumn<AutoflowRunsSortKey>] {
        [
            SortableColumn(key: nil, label: "", width: AutoflowRunsLayout.chevron),
            SortableColumn(key: .status, label: "Status", width: KindTableLayout.status),
            SortableColumn(key: .workflow, label: "Workflow", width: nil),
            SortableColumn(key: nil, label: "Run", width: KindTableLayout.run, alignment: .trailing),
            SortableColumn(key: .event, label: "Event", width: AutoflowRunsLayout.event, alignment: .trailing),
            SortableColumn(key: .issueRef, label: "Issue Ref", width: AutoflowRunsLayout.issueRef, alignment: .trailing, firstTapAscending: true),
            SortableColumn(key: .worker, label: "Worker", width: AutoflowRunsLayout.worker, alignment: .trailing, firstTapAscending: true),
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

    static func sortValue(_ row: ActivityRow, _ key: AutoflowRunsSortKey) -> ListSortValue {
        switch key {
        case .status: isRunEvent(row) ? .text(row.status.displayLabel) : .text(nil)
        case .workflow: .text(row.subject)
        case .event: .text(row.eventKind)
        case .issueRef: .text(row.issueRef)
        case .worker: isRunEvent(row) ? .text(row.worker) : .text(nil)
        case .host: .text(row.host)
        case .inTokens: isRunEvent(row) ? .number(row.inputTokens.map { Double($0) }) : .number(nil)
        case .outTokens: isRunEvent(row) ? .number(row.outputTokens.map { Double($0) }) : .number(nil)
        case .cachedTokens: isRunEvent(row) ? .number(row.cachedTokens.map { Double($0) }) : .number(nil)
        case .cost: isRunEvent(row) ? .number(row.costUSD) : .number(nil)
        case .turns: isRunEvent(row) ? .number(row.turns.map { Double($0) }) : .number(nil)
        case .duration: isRunEvent(row) ? .number(row.durationMS.map { Double($0) }) : .number(nil)
        case .started: .date(row.startedAt)
        }
    }

    /// A "run event" is one that actually launched a run — the web's
    /// `isRunEvent(e) = Boolean(e.run_id)` (`AutoflowRuns.tsx:163-165`),
    /// mirrored here via `navigation`: `ActivityRow.init(_:
    /// APIAutoflowEventRow)` only ever produces `.run(id:host:)` when
    /// `r.runID` is non-nil (`.none` otherwise). Every run-shaped column
    /// (Status/Worker/token triplet/Cost/Turns/Duration — NOT Host/Issue
    /// Ref/Event, which describe the scheduling event itself, not a launched
    /// run) renders blank, not a dash, for a scheduling-only event (awaiting/
    /// cycle_failed/etc.) — web parity, and the brief's explicit "autoflow
    /// non-run events: BLANK, not dash" rule.
    static func isRunEvent(_ row: ActivityRow) -> Bool {
        if case .run = row.navigation { return true }
        return false
    }
}

/// Event-kind badge — ported from the web's `KindBadge`/`CycleFailedPill`
/// (`AutoflowRuns.tsx:68-110`): a per-kind label + tone, with `cycle_failed`
/// deliberately borrowing the shared `StatusTone.failed` color (not the
/// plain neutral badge treatment every other kind gets) so a cycle-level
/// failure reads as visually distinct from a scheduling signal, never
/// mistaken for a run's own status.
enum AutoflowEventBadge {
    static func label(_ kind: String) -> String {
        switch kind {
        case "run_launched": "launched"
        case "awaiting_human": "awaiting human"
        case "awaiting_external": "awaiting external"
        case "cycle_failed": "failed"
        default: kind.replacingOccurrences(of: "_", with: " ")
        }
    }

    static func tone(_ kind: String) -> Color {
        switch kind {
        case "run_launched": Color.rupuOk
        case "awaiting_human": Color.rupuWarn
        case "awaiting_external": Color.rupuInfo
        case "cycle_failed": Color.status(.failed)
        default: Color.rupuMute
        }
    }
}

private struct AutoflowRunRowView: View {
    let row: ActivityRow
    let store: ActivityStore
    let backend: BackendController
    let now: Date
    let isExpanded: Bool
    let onToggleExpand: () -> Void
    let onSelect: (ActivityRow) -> Void

    private var isClickable: Bool { row.navigation != .none }
    private var isRunEvent: Bool { AutoflowRunsTable.isRunEvent(row) }
    private var isExpandable: Bool { row.detail != nil }

    var body: some View {
        HStack(spacing: 0) {
            chevronCell.frame(width: AutoflowRunsLayout.chevron, alignment: .leading).padding(.trailing, 8)
            statusCell.frame(width: KindTableLayout.status, alignment: .leading).padding(.trailing, 8)
            Text(row.subject)
                .foregroundStyle(Color.rupuInk)
                .lineLimit(1)
                .truncationMode(.tail)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.trailing, 8)
                .help(row.subject)
            runCell.frame(width: KindTableLayout.run, alignment: .trailing).padding(.trailing, 8)
            eventCell.frame(width: AutoflowRunsLayout.event, alignment: .trailing).padding(.trailing, 8)
            issueRefCell.frame(width: AutoflowRunsLayout.issueRef, alignment: .trailing).padding(.trailing, 8)
            workerCell.frame(width: AutoflowRunsLayout.worker, alignment: .trailing).padding(.trailing, 8)
            Text(row.host)
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuDim)
                .lineLimit(1)
                .frame(width: KindTableLayout.host, alignment: .trailing)
                .padding(.trailing, 8)
            tokenCell(row.inputTokens).frame(width: KindTableLayout.tokenCell, alignment: .trailing).padding(.trailing, 8)
            tokenCell(row.outputTokens).frame(width: KindTableLayout.tokenCell, alignment: .trailing).padding(.trailing, 8)
            tokenCell(row.cachedTokens).frame(width: KindTableLayout.tokenCell, alignment: .trailing).padding(.trailing, 8)
            costCell.frame(width: KindTableLayout.cost, alignment: .trailing).padding(.trailing, 8)
            tokenCell(row.turns).frame(width: KindTableLayout.turns, alignment: .trailing).padding(.trailing, 8)
            durationCell.frame(width: KindTableLayout.duration, alignment: .trailing).padding(.trailing, 8)
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

    @ViewBuilder
    private var chevronCell: some View {
        if isExpandable {
            Button(action: onToggleExpand) {
                Icon(.chevronDown, size: 9)
                    .foregroundStyle(Color.rupuDim)
                    .rotationEffect(.degrees(isExpanded ? 0 : -90))
            }
            .buttonStyle(.plain)
        }
    }

    @ViewBuilder
    private var statusCell: some View {
        if isRunEvent {
            KindTableStatusCell(status: row.status)
        }
    }

    @ViewBuilder
    private var runCell: some View {
        if case .run(let runID, _) = row.navigation {
            Text(KindTableFormat.shortID(runID)).font(.dataMono(11)).foregroundStyle(Color.rupuDim)
        }
    }

    private var eventCell: some View {
        Badge(AutoflowEventBadge.label(row.eventKind ?? "").uppercased(), tone: AutoflowEventBadge.tone(row.eventKind ?? ""))
    }

    private var issueRefCell: some View {
        Group {
            if let issueRef = row.issueRef {
                Badge(issueRef, tone: Color.rupuMute)
            } else {
                Text("—").font(.dataMono(11)).foregroundStyle(Color.rupuMute)
            }
        }
    }

    @ViewBuilder
    private var workerCell: some View {
        if isRunEvent {
            Text(row.worker ?? "—")
                .font(.dataMono(11))
                .foregroundStyle(row.worker == nil ? Color.rupuMute : Color.rupuDim)
                .lineLimit(1)
        }
    }

    @ViewBuilder
    private func tokenCell(_ value: UInt64?) -> some View {
        if isRunEvent {
            Text(KindTableFormat.count(value)).font(.dataMono(11)).foregroundStyle(Color.rupuDim)
        }
    }

    @ViewBuilder
    private var costCell: some View {
        if isRunEvent {
            Text(Fmt.cost(row.costUSD)).font(.dataMono(11)).foregroundStyle(Color.rupuInk)
        }
    }

    @ViewBuilder
    private var durationCell: some View {
        if isRunEvent {
            Text(row.durationMS.map { Fmt.duration(ms: $0) } ?? "—").font(.dataMono(11)).foregroundStyle(Color.rupuInk)
        }
    }

    private var startedLabel: String {
        guard let date = row.startedAt else { return "—" }
        return KindTableFormat.relative.localizedString(for: date, relativeTo: now)
    }
}
