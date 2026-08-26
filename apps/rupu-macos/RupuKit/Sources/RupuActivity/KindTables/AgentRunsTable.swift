import SwiftUI
import AppKit
import RupuStore
import RupuDesign

/// Sortable keys for the dedicated Agents kind table — column set ported
/// verbatim from the web's `AGENT_RUN_COLUMNS` (`pages/runs/AgentRuns.tsx`):
/// Status · Agent(subject, two-line: name + "via {trigger} · session
/// {shortId}") · Run · Source · Host · In · Out · Cached · Cost · Turns ·
/// Duration · Started · actions.
public enum AgentRunsSortKey: Hashable, CaseIterable, Sendable {
    case status, agent, source, host, inTokens, outTokens, cachedTokens, cost, turns, duration, started
}

private enum AgentRunsLayout {
    static let source: CGFloat = 76
}

/// The Agents kind page's dedicated table (route: `.activity(.agents)`).
/// See `WorkflowRunsTable`'s doc comment for why this owns a local `ListSort`
/// rather than sharing `ActivityStore.sort`.
struct AgentRunsTable: View {
    let rows: [ActivityRow]
    let store: ActivityStore
    let backend: BackendController
    let onSelect: (ActivityRow) -> Void

    @State private var sort = ListSort<AgentRunsSortKey>(key: .started, ascending: false)
    @State private var query = ""
    @State private var debouncedQuery = ""

    var body: some View {
        let _ = RenderMeter.tick("AgentRunsTable")
        let now = Date()
        let sorted = sortRows(rows, sort: sort, value: Self.sortValue)
        let q = debouncedQuery.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        let visible = q.isEmpty ? sorted : sorted.filter { Self.matches($0, query: q) }
        VStack(alignment: .leading, spacing: 8) {
            KindTableSearchField(placeholder: "Find agents…", query: $query)
            VStack(alignment: .leading, spacing: 0) {
                SortableHeaderRow(columns: columns, sort: $sort)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                Divider()
                List {
                    ForEach(visible) { row in
                        AgentRunRowView(row: row, store: store, backend: backend, now: now, onSelect: onSelect)
                            .listRowBackground(row.status == .awaiting ? Color.status(.awaiting).opacity(0.04) : .clear)
                            .listRowSeparator(.visible)
                            .listRowInsets(EdgeInsets(top: 0, leading: 0, bottom: 0, trailing: 0))
                    }
                    KindTableFooter(
                        visibleCount: visible.count,
                        loadedCount: store.loadedCount,
                        hasMore: store.hasMore,
                        isLoadingMore: store.isLoadingMore,
                        // Brief: the honesty footer switches to "N matches
                        // of M loaded" whenever ANY client-side narrowing is
                        // active — Find text OR the FilterBar status chips
                        // (both narrow `rows` before this table ever sees
                        // it), not just Find.
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
    /// `AgentRuns.tsx` Find box matches (`[r.agent, r.run_id, r.session_id,
    /// r.host_id]`): agent name (`subject`), run id (`id`), session id (only
    /// resolvable when `navigation` is `.session` — see `ActivityRow.init(_:
    /// APIAgentRunRow)`), host.
    static func matches(_ row: ActivityRow, query: String) -> Bool {
        var fields = [row.subject, row.id, row.host]
        if case .session(let sessionID) = row.navigation {
            fields.append(sessionID)
        }
        return fields.contains { $0.lowercased().contains(query) }
    }

    private var columns: [SortableColumn<AgentRunsSortKey>] {
        [
            SortableColumn(key: .status, label: "Status", width: KindTableLayout.status),
            SortableColumn(key: .agent, label: "Agent", width: nil),
            SortableColumn(key: nil, label: "Run", width: KindTableLayout.run, alignment: .trailing),
            SortableColumn(key: .source, label: "Source", width: AgentRunsLayout.source, alignment: .trailing),
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

    static func sortValue(_ row: ActivityRow, _ key: AgentRunsSortKey) -> ListSortValue {
        switch key {
        case .status: .text(row.status.displayLabel)
        case .agent: .text(row.subject)
        case .source: .text(row.source)
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

    /// The Agent subject cell's second line — `"via {trigger} · session
    /// {shortId}"` — ported from the web's `AGENT_RUN_COLUMNS` `agent` cell
    /// (`AgentRuns.tsx:454-467`). Both halves are independently optional: a
    /// row with a trigger but no resolvable session (any agent row whose
    /// `navigation` isn't `.session`) shows only the trigger half; a row
    /// with neither shows no second line at all (`nil`, not an empty
    /// string — the view only reserves space for this line when it's
    /// present). Pure and `static` — `TDD`'d in `AgentRunsTableTests`.
    static func subtitle(_ row: ActivityRow) -> String? {
        var parts: [String] = []
        if let trigger = row.trigger { parts.append("via \(trigger)") }
        if case .session(let id) = row.navigation {
            parts.append("session \(KindTableFormat.shortID(id))")
        }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    /// Source badge tone — web parity (`AgentRuns.tsx:491`): `source ==
    /// "session"` gets the info tone, every other source (cli/cron/etc.,
    /// or a row with no `source` at all — never populated for a row this
    /// table wouldn't show) gets the neutral tone.
    static func sourceTone(_ row: ActivityRow) -> Color {
        row.source == "session" ? Color.rupuInfo : Color.rupuMute
    }
}

private struct AgentRunRowView: View {
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
            subjectCell
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.trailing, 8)
            Text(KindTableFormat.shortID(row.id))
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuDim)
                .frame(width: KindTableLayout.run, alignment: .trailing)
                .padding(.trailing, 8)
            sourceCell
                .frame(width: AgentRunsLayout.source, alignment: .trailing)
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

    @ViewBuilder
    private var subjectCell: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(row.subject)
                .foregroundStyle(Color.rupuInk)
                .lineLimit(1)
                .truncationMode(.tail)
                .help(row.subject)
            if let subtitle = AgentRunsTable.subtitle(row) {
                Text(subtitle)
                    .font(.noteText)
                    .foregroundStyle(Color.rupuDim)
                    .lineLimit(1)
            }
        }
    }

    @ViewBuilder
    private var sourceCell: some View {
        if let source = row.source {
            Badge(source.uppercased(), tone: AgentRunsTable.sourceTone(row))
        } else {
            Text("—").font(.dataMono(11)).foregroundStyle(Color.rupuMute)
        }
    }

    private var startedLabel: String {
        guard let date = row.startedAt else { return "—" }
        return KindTableFormat.relative.localizedString(for: date, relativeTo: now)
    }
}
