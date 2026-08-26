import SwiftUI
import AppKit
import RupuAPI
import RupuStore
import RupuDesign

/// Sortable keys for the Autoflows kind page's Cycles sub-table — column set
/// ported verbatim from the web's `CYCLE_COLUMNS` (`pages/runs/
/// AutoflowRuns.tsx`): Cycle · Mode · Worker · Started · Duration · Ran ·
/// Skipped · Failed · Usage · Host. `.usage` deliberately has no case here —
/// the web's own `usage` column carries no `sortable`/`sortValue` either
/// (a token+cost pair has no single natural sort order), so its header cell
/// is plain (`SortableColumn(key: nil, ...)`), same as this table's trailing
/// filler column.
public enum AutoflowCyclesSortKey: Hashable, CaseIterable, Sendable {
    case cycle, mode, worker, started, duration, ran, skipped, failed, host
}

private enum AutoflowCyclesLayout {
    static let chevron: CGFloat = 20
    static let cycle: CGFloat = 90
    static let mode: CGFloat = 76
    static let worker: CGFloat = 96
    static let ran: CGFloat = 44
    static let skipped: CGFloat = 56
    static let failed: CGFloat = 56
    static let usage: CGFloat = 150
}

/// The Autoflows kind page's Cycles sub-table (perf & interaction arc, Plan
/// 5 Task 4b — the named remainder from Task 4's report). One row per
/// autoflow-worker batch tick (`GET /api/runs/autoflows`, `CyclesStore`) —
/// distinct from the Runs sub-table (`AutoflowRunsTable`, launched-run-or-
/// signal *events*) and the pre-existing Claims sub-table.
///
/// **Deliberately no subject/free-text column — every column is
/// fixed-width ("fit")**, mirroring the web's own `CYCLE_COLUMNS` comment
/// verbatim ("No column here is a natural free-text 'subject' … a cycle has
/// no single describing name — it spans `workflow_count` workflows —
/// every column is `fit`"). A trailing blank filler column (`key: nil,
/// width: nil`) absorbs any leftover width so the table still fills its
/// container, the same way `FindingsTable`'s leading blank gutter column
/// reserves space without being a truncating subject.
///
/// **No row-level navigation** — unlike every other kind table, tapping a
/// row does nothing (a cycle itself has no detail screen). The only
/// navigable elements are the individual spawned-run-id links inside the
/// expanded detail row, each pushing `.runDetail(id:host:)` via
/// `onSelectRun`.
///
/// **Always expandable** (unlike `AutoflowRunsTable`'s chevron, which only
/// appears for a `detail`-bearing event): every cycle either lists its
/// spawned run ids or, for a cycle that launched none, says so plainly —
/// mirrors the web's `CycleDetail`, which always renders one of the two.
struct AutoflowCyclesTable: View {
    let rows: [APIAutoflowCycleRow]
    let onSelectRun: (Route) -> Void

    @State private var sort = ListSort<AutoflowCyclesSortKey>(key: .started, ascending: false)
    @State private var expandedIDs: Set<String> = []

    var body: some View {
        let _ = RenderMeter.tick("AutoflowCyclesTable")
        let now = Date()
        let sorted = sortRows(rows, sort: sort, value: Self.sortValue)
        VStack(alignment: .leading, spacing: 0) {
            SortableHeaderRow(columns: columns, sort: $sort)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
            Divider()
            List(sorted) { row in
                VStack(alignment: .leading, spacing: 0) {
                    AutoflowCycleRowView(
                        row: row, now: now,
                        isExpanded: expandedIDs.contains(row.id),
                        onToggleExpand: { toggleExpand(row.id) }
                    )
                    if expandedIDs.contains(row.id) {
                        detailView(for: row)
                    }
                }
                .listRowSeparator(.visible)
                .listRowInsets(EdgeInsets(top: 0, leading: 0, bottom: 0, trailing: 0))
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
        }
        .panelStyle(.panel)
    }

    private func toggleExpand(_ id: String) {
        if expandedIDs.contains(id) {
            expandedIDs.remove(id)
        } else {
            expandedIDs.insert(id)
        }
    }

    /// Spawned-run-id links (or a plain "no runs launched" note) — ported
    /// from the web's `CycleDetail`. Each run id navigates via `onSelectRun`
    /// to `.runDetail(id:host:)`, carrying the CYCLE's own `hostID` (a
    /// cycle's spawned runs execute on the same host the cycle itself ran
    /// on — there is no per-run host field on this row to read instead).
    @ViewBuilder
    private func detailView(for row: APIAutoflowCycleRow) -> some View {
        if row.runIDs.isEmpty {
            Text("No runs launched in this cycle.")
                .font(.noteText)
                .foregroundStyle(Color.rupuMute)
                .padding(.horizontal, 12)
                .padding(.bottom, 8)
                .padding(.leading, AutoflowCyclesLayout.chevron + 8)
        } else {
            // Plain horizontally-scrolling `HStack`, not a wrapping flow
            // layout — `RupuRunDetail`'s own `FlowLayout` is `private` to
            // that module and this is the only call site in `RupuActivity`
            // that would want one; a cycle's `workflow_count` (the practical
            // upper bound on how many run ids one cycle spawns) is small
            // enough in practice that a scrolling row, not a new shared
            // cross-module `Layout` type, is the proportionate fix here.
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 8) {
                    ForEach(row.runIDs, id: \.self) { runID in
                        Button {
                            onSelectRun(.runDetail(id: runID, host: row.hostID))
                        } label: {
                            Text(KindTableFormat.shortID(runID))
                                .font(.dataMono(11))
                                .foregroundStyle(Color.rupuBrand700)
                                .underline()
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
            .padding(.horizontal, 12)
            .padding(.bottom, 8)
            .padding(.leading, AutoflowCyclesLayout.chevron + 8)
        }
    }

    private var columns: [SortableColumn<AutoflowCyclesSortKey>] {
        [
            SortableColumn(key: nil, label: "", width: AutoflowCyclesLayout.chevron),
            SortableColumn(key: .cycle, label: "Cycle", width: AutoflowCyclesLayout.cycle),
            SortableColumn(key: .mode, label: "Mode", width: AutoflowCyclesLayout.mode),
            SortableColumn(key: .worker, label: "Worker", width: AutoflowCyclesLayout.worker),
            SortableColumn(key: .started, label: "Started", width: KindTableLayout.started, alignment: .trailing),
            SortableColumn(key: .duration, label: "Duration", width: KindTableLayout.duration, alignment: .trailing),
            SortableColumn(key: .ran, label: "Ran", width: AutoflowCyclesLayout.ran, alignment: .trailing),
            SortableColumn(key: .skipped, label: "Skipped", width: AutoflowCyclesLayout.skipped, alignment: .trailing),
            SortableColumn(key: .failed, label: "Failed", width: AutoflowCyclesLayout.failed, alignment: .trailing),
            SortableColumn(key: nil, label: "Usage", width: AutoflowCyclesLayout.usage, alignment: .trailing),
            SortableColumn(key: .host, label: "Host", width: KindTableLayout.host, alignment: .trailing, firstTapAscending: true),
            // Trailing filler — deliberately NOT a subject column (see the
            // type doc comment); just absorbs leftover width.
            SortableColumn(key: nil, label: "", width: nil),
        ]
    }

    static func sortValue(_ row: APIAutoflowCycleRow, _ key: AutoflowCyclesSortKey) -> ListSortValue {
        switch key {
        case .cycle: .text(row.cycleID)
        case .mode: .text(row.mode)
        case .worker: .text(row.workerName)
        case .started: .date(ISO8601Parsing.parse(row.startedAt))
        case .duration: .number(durationMS(row).map { Double($0) })
        case .ran: .number(Double(row.ranCycles))
        case .skipped: .number(Double(row.skippedCycles))
        case .failed: .number(Double(row.failedCycles))
        case .host: .text(row.hostID ?? "local")
        }
    }

    /// Wall-clock cycle duration — `finished_at - started_at`. `nil` when
    /// either timestamp fails to parse (never happens for a real server row
    /// — both are non-optional on the wire, per `APIAutoflowCycleRow`'s doc
    /// comment — but this stays honest rather than force-unwrapping).
    static func durationMS(_ row: APIAutoflowCycleRow) -> UInt64? {
        guard let start = ISO8601Parsing.parse(row.startedAt),
              let end = ISO8601Parsing.parse(row.finishedAt),
              end >= start
        else { return nil }
        return UInt64(end.timeIntervalSince(start) * 1000)
    }

    /// Mode chip tone — ported from the web's `MODE_CLS`: `ask`=warn,
    /// `bypass`=ok, `readonly`/`tick`=neutral, `serve`=info. An unrecognized
    /// mode string falls back to neutral rather than guessing.
    static func modeTone(_ mode: String) -> Color {
        switch mode {
        case "ask": Color.rupuWarn
        case "bypass": Color.rupuOk
        case "serve": Color.rupuInfo
        default: Color.rupuMute
        }
    }

    /// "`{tok} tok · {cost}`", `*`-suffixed when the cost is partial
    /// (present but unpriced) — ported from the web's `UsageChip`.
    static func usageLabel(_ usage: APIUsageSummary) -> String {
        let partial = usage.costUSD != nil && !usage.priced
        let cost = Fmt.cost(usage.costUSD)
        return "\(Fmt.count(Int(exactly: usage.totalTokens) ?? 0)) tok · \(cost)\(partial ? "*" : "")"
    }
}

private struct AutoflowCycleRowView: View {
    let row: APIAutoflowCycleRow
    let now: Date
    let isExpanded: Bool
    let onToggleExpand: () -> Void

    var body: some View {
        HStack(spacing: 0) {
            Button(action: onToggleExpand) {
                Icon(.chevronDown, size: 9)
                    .foregroundStyle(Color.rupuDim)
                    .rotationEffect(.degrees(isExpanded ? 0 : -90))
            }
            .buttonStyle(.plain)
            .frame(width: AutoflowCyclesLayout.chevron, alignment: .leading)
            .padding(.trailing, 8)

            Text(KindTableFormat.shortID(row.cycleID))
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuInk)
                .frame(width: AutoflowCyclesLayout.cycle, alignment: .leading)
                .padding(.trailing, 8)

            Badge(row.mode.uppercased(), tone: AutoflowCyclesTable.modeTone(row.mode))
                .frame(width: AutoflowCyclesLayout.mode, alignment: .leading)
                .padding(.trailing, 8)

            Text(row.workerName ?? "—")
                .font(.metaText)
                .foregroundStyle(row.workerName == nil ? Color.rupuMute : Color.rupuDim)
                .lineLimit(1)
                .truncationMode(.tail)
                .frame(width: AutoflowCyclesLayout.worker, alignment: .leading)
                .padding(.trailing, 8)

            Text(startedLabel)
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuMute)
                .frame(width: KindTableLayout.started, alignment: .trailing)
                .padding(.trailing, 8)

            Text(AutoflowCyclesTable.durationMS(row).map { Fmt.duration(ms: $0) } ?? "—")
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuDim)
                .frame(width: KindTableLayout.duration, alignment: .trailing)
                .padding(.trailing, 8)

            Text("\(row.ranCycles)")
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuInk)
                .frame(width: AutoflowCyclesLayout.ran, alignment: .trailing)
                .padding(.trailing, 8)

            Text("\(row.skippedCycles)")
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuDim)
                .frame(width: AutoflowCyclesLayout.skipped, alignment: .trailing)
                .padding(.trailing, 8)

            Text("\(row.failedCycles)")
                .font(.dataMono(11).weight(row.failedCycles > 0 ? .semibold : .regular))
                .foregroundStyle(row.failedCycles > 0 ? Color.status(.failed) : Color.rupuDim)
                .frame(width: AutoflowCyclesLayout.failed, alignment: .trailing)
                .padding(.trailing, 8)

            Text(AutoflowCyclesTable.usageLabel(row.usage))
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuMute)
                .frame(width: AutoflowCyclesLayout.usage, alignment: .trailing)
                .padding(.trailing, 8)

            Text(row.hostID ?? "local")
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuDim)
                .lineLimit(1)
                .frame(width: KindTableLayout.host, alignment: .trailing)

            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }

    private var startedLabel: String {
        guard let date = ISO8601Parsing.parse(row.startedAt) else { return "—" }
        return KindTableFormat.relative.localizedString(for: date, relativeTo: now)
    }
}
