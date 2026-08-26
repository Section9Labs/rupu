import SwiftUI
import AppKit
import RupuStore
import RupuDesign

/// Sortable keys for the dedicated Sessions kind table — column set ported
/// verbatim from the web's `SESSION_BASE_COLUMNS` (`pages/Sessions.tsx`):
/// Status · Agent(subject) · Session · Host · Model(mono) · In · Out ·
/// Cached · Cost · Turns · Duration(created→updated) · Started · actions.
///
/// `.none` is a sentinel, never assigned to any column's `key:` — see
/// `SessionsTable`'s "NO initial client sort" doc-comment section for why it
/// exists: it lets `sort.key` hold a value that matches no real column,
/// so `SortableHeaderRow`'s chevron-highlight (`sort.key == key`) shows no
/// column as active before the operator has actually chosen one.
public enum SessionsSortKey: Hashable, CaseIterable, Sendable {
    case none, status, agent, host, model, inTokens, outTokens, cachedTokens, cost, turns, duration, started
}

private enum SessionsLayout {
    static let model: CGFloat = 128
}

/// The Sessions kind page's dedicated table (route: `.activity(.sessions)`).
///
/// **NO initial client sort — approximates server `updated_at desc` order**
/// (the brief's explicit requirement, unlike every other kind table here,
/// which defaults to Started-descending): `GET /api/sessions` itself already
/// returns rows in `updated_at`-descending order, and this table shouldn't
/// impose its own default reordering on top of that until the operator
/// actually clicks a header. The complication: `ActivityStore.rows` (this
/// table's `rows` input) is NOT that raw server order — `ActivityStore.
/// recompute()` unconditionally presorts every kind's merged rows by
/// `startedAt` descending (`startedAt` carries `created_at` for a session
/// row, not `updated_at`) before this table ever sees them, a store-wide
/// behavior change to fix would ripple into the merged `.all` view and
/// every existing `ActivityStore` test. Rather than silently accepting that
/// substitution (created_at order masquerading as "no sort"), this table
/// re-derives the honest `updated_at`-descending order itself from
/// `ActivityRow.updatedAt` (added for exactly this — Task 4's per-kind
/// field extension) whenever the operator hasn't chosen an explicit sort
/// yet (`hasExplicitSort == false`). Once any header is tapped, `sort`
/// takes over completely, same as every other kind table.
struct SessionsTable: View {
    let rows: [ActivityRow]
    let onSelect: (ActivityRow) -> Void

    @State private var sort = ListSort<SessionsSortKey>(key: .none, ascending: false)
    @State private var hasExplicitSort = false

    var body: some View {
        let _ = RenderMeter.tick("SessionsTable")
        let now = Date()
        let sorted = hasExplicitSort ? sortRows(rows, sort: sort, value: Self.sortValue) : Self.updatedAtDescending(rows)
        VStack(alignment: .leading, spacing: 0) {
            SortableHeaderRow(columns: columns, sort: $sort)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .onChange(of: sort) { _, _ in hasExplicitSort = true }
            Divider()
            List(sorted) { row in
                SessionRowView(row: row, now: now, onSelect: onSelect)
                    .listRowSeparator(.visible)
                    .listRowInsets(EdgeInsets(top: 0, leading: 0, bottom: 0, trailing: 0))
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
        }
        .panelStyle(.panel)
    }

    private var columns: [SortableColumn<SessionsSortKey>] {
        [
            SortableColumn(key: .status, label: "Status", width: KindTableLayout.status),
            SortableColumn(key: .agent, label: "Agent", width: nil),
            SortableColumn(key: nil, label: "Session", width: KindTableLayout.run, alignment: .trailing),
            SortableColumn(key: .host, label: "Host", width: KindTableLayout.host, alignment: .trailing, firstTapAscending: true),
            SortableColumn(key: .model, label: "Model", width: SessionsLayout.model, alignment: .trailing, firstTapAscending: true),
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

    static func sortValue(_ row: ActivityRow, _ key: SessionsSortKey) -> ListSortValue {
        switch key {
        case .none: .text(nil)
        case .status: .text(row.status.displayLabel)
        case .agent: .text(row.subject)
        case .host: .text(row.host)
        case .model: .text(row.model)
        case .inTokens: .number(row.inputTokens.map { Double($0) })
        case .outTokens: .number(row.outputTokens.map { Double($0) })
        case .cachedTokens: .number(row.cachedTokens.map { Double($0) })
        case .cost: .number(row.costUSD)
        case .turns: .number(row.turns.map { Double($0) })
        case .duration: .number(Self.durationMS(row).map { Double($0) })
        case .started: .date(row.startedAt)
        }
    }

    /// Session duration — `created→updated`, ported from the web's
    /// `sessionDurationMs` (`Sessions.tsx`): `ActivityRow.startedAt` carries
    /// `created_at` for a session row, `updatedAt` carries `updated_at`
    /// (both added/reused for exactly this). `nil` when either timestamp
    /// failed to parse — an unparseable pair renders `—`, same null
    /// discipline every other duration cell in this app follows.
    static func durationMS(_ row: ActivityRow) -> UInt64? {
        guard let start = row.startedAt, let end = row.updatedAt, end >= start else { return nil }
        return UInt64(end.timeIntervalSince(start) * 1000)
    }

    /// Nulls-last descending by `updatedAt` — see the type doc comment's
    /// "NO initial client sort" section for why this (not `ActivityStore`'s
    /// own startedAt-descending presort) is this table's default order.
    static func updatedAtDescending(_ rows: [ActivityRow]) -> [ActivityRow] {
        rows.enumerated()
            .sorted { lhs, rhs in
                switch (lhs.element.updatedAt, rhs.element.updatedAt) {
                case let (l?, r?):
                    if l != r { return l > r }
                    return lhs.offset < rhs.offset
                case (nil, nil): return lhs.offset < rhs.offset
                case (nil, _): return false
                case (_, nil): return true
                }
            }
            .map(\.element)
    }
}

private struct SessionRowView: View {
    let row: ActivityRow
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
            Text(row.host)
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuDim)
                .lineLimit(1)
                .frame(width: KindTableLayout.host, alignment: .trailing)
                .padding(.trailing, 8)
            Text(row.model ?? "—")
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuDim)
                .lineLimit(1)
                .truncationMode(.tail)
                .frame(width: SessionsLayout.model, alignment: .trailing)
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
            Text(SessionsTable.durationMS(row).map { Fmt.duration(ms: $0) } ?? "—")
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuInk)
                .frame(width: KindTableLayout.duration, alignment: .trailing)
                .padding(.trailing, 8)
            Text(startedLabel)
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuDim)
                .frame(width: KindTableLayout.started, alignment: .trailing)
            // No awaiting-actions cell — a session never carries `.awaiting`
            // (`ActivityRow.init(_: APISessionRow)`'s status is derived from
            // `activeRunID`/`lastError` only), but the column is still
            // width-reserved for alignment with every other kind table.
            Spacer(minLength: 0).frame(width: KindTableLayout.actions)
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
