import SwiftUI
import Foundation
import RupuStore
import RupuDesign

// MARK: - Pure derivation (the tested seam)

/// The `.all`-kind Activity parent's stats surface (perf & interaction arc,
/// Plan 5 Task 4 — matt's direct feedback: the Activity parent stops
/// showing one combined table with a kind-picker; it becomes a stats
/// overview instead, with every kind's real table living one level down at
/// the sidebar's disclosure children). Pure and free-standing, same
/// "testable without `@MainActor`" rule `deriveNeedsYou` follows.
///
/// **Honest about coverage** (the brief's explicit "label the numbers
/// honestly" requirement): every count here is over whatever rows
/// `ActivityStore` has actually loaded for the current scope — page-0 of
/// each of the four federated sources, same data `unscopedRows`/`rows`
/// already hold for every other consumer — never a server-computed
/// aggregate. `ActivityStatsView`'s footer states this plainly ("over N
/// loaded executions") rather than implying a fleet-wide total.
public struct ActivityStatsSummary: Equatable, Sendable {
    public var totalLoaded: Int
    public var agentCount: Int
    public var workflowCount: Int
    public var autoflowCount: Int
    public var sessionCount: Int
    /// Rows whose `startedAt` falls on `now`'s calendar day, split by
    /// status. A row with no `startedAt` never counts as "today" — an
    /// unknown start time isn't evidence it happened today.
    public var runningToday: Int
    public var awaitingToday: Int
    public var failedToday: Int
    /// The oldest currently-`.awaiting` row's age (`now - startedAt`), or
    /// `nil` when there's no awaiting row at all, or every awaiting row has
    /// an unknown `startedAt` (an unknown age can't be claimed as "oldest").
    public var oldestAwaitingAge: TimeInterval?
}

public func deriveActivityStats(rows: [ActivityRow], now: Date, calendar: Calendar = .current) -> ActivityStatsSummary {
    var agentCount = 0, workflowCount = 0, autoflowCount = 0, sessionCount = 0
    var runningToday = 0, awaitingToday = 0, failedToday = 0
    var oldestAwaitingStart: Date?

    for row in rows {
        switch row.kind {
        case .agent: agentCount += 1
        case .workflow: workflowCount += 1
        case .autoflow: autoflowCount += 1
        case .session: sessionCount += 1
        }

        let isToday = row.startedAt.map { calendar.isDate($0, inSameDayAs: now) } ?? false
        if isToday {
            switch row.status {
            case .running: runningToday += 1
            case .awaiting: awaitingToday += 1
            case .failed: failedToday += 1
            default: break
            }
        }

        if row.status == .awaiting, let startedAt = row.startedAt {
            if oldestAwaitingStart == nil || startedAt < oldestAwaitingStart! {
                oldestAwaitingStart = startedAt
            }
        }
    }

    return ActivityStatsSummary(
        totalLoaded: rows.count,
        agentCount: agentCount, workflowCount: workflowCount, autoflowCount: autoflowCount, sessionCount: sessionCount,
        runningToday: runningToday, awaitingToday: awaitingToday, failedToday: failedToday,
        oldestAwaitingAge: oldestAwaitingStart.map { now.timeIntervalSince($0) }
    )
}

// MARK: - View

/// The `.all`-kind Activity parent screen — KPI cards over `deriveActivity
/// Stats`, plus a compact needs-attention list reusing `deriveNeedsYou`
/// (`RupuStore/NeedsYou.swift` — moved there from `RupuOverview` for exactly
/// this reuse; see that file's doc comment). Reads `store.rows` (this
/// screen's own scope-filtered projection, same as every kind page reads for
/// its own table) rather than `unscopedRows` — unlike `NeedsYouCard`
/// (`RupuOverview`), which deliberately ignores the top bar's project scope
/// for fleet-wide coverage, THIS page is itself scope-aware chrome (matt's
/// restructure keeps the top bar's project-scope picker meaningful
/// everywhere), so its own KPIs and attention list narrow the same way any
/// other Activity page would.
struct ActivityStatsView: View {
    let store: ActivityStore
    let backend: BackendController
    let range: TimeRange
    let onNavigate: (Route) -> Void

    var body: some View {
        let _ = RenderMeter.tick("ActivityStatsView")
        let stats = deriveActivityStats(rows: store.rows, now: Date())
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                kpiGrid(stats)
                needsAttentionCard
            }
            .padding(.bottom, 4)
        }
    }

    // MARK: - KPI cards

    private func kpiGrid(_ stats: ActivityStatsSummary) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                kindCountCard(stats)
                statCard(label: "Running today", value: "\(stats.runningToday)", tone: .running)
                statCard(label: "Awaiting today", value: "\(stats.awaitingToday)", tone: .awaiting)
                statCard(label: "Failed today", value: "\(stats.failedToday)", tone: .failed)
                oldestAwaitingCard(stats)
            }
            Text("Over \(stats.totalLoaded) loaded execution\(stats.totalLoaded == 1 ? "" : "s")")
                .font(.noteText)
                .foregroundStyle(Color.rupuMute)
        }
    }

    private func kindCountCard(_ stats: ActivityStatsSummary) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Eyebrow("By kind")
            HStack(spacing: 10) {
                kindFigure("Agents", stats.agentCount)
                kindFigure("Workflows", stats.workflowCount)
                kindFigure("Autoflows", stats.autoflowCount)
                kindFigure("Sessions", stats.sessionCount)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(12)
        .panelStyle(.panel)
    }

    private func kindFigure(_ label: String, _ count: Int) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text("\(count)").font(.dataMono(15)).foregroundStyle(Color.rupuInk)
            Text(label).font(.noteText).foregroundStyle(Color.rupuMute)
        }
    }

    private func statCard(label: String, value: String, tone: StatusTone) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Circle().fill(Color.status(tone)).frame(width: 6, height: 6)
                Eyebrow(label)
            }
            Text(value).font(.dataMono(20)).foregroundStyle(Color.rupuInk)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(12)
        .panelStyle(.panel)
    }

    private func oldestAwaitingCard(_ stats: ActivityStatsSummary) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Eyebrow("Oldest awaiting")
            Text(Self.ageLabel(stats.oldestAwaitingAge))
                .font(.dataMono(20))
                .foregroundStyle(stats.oldestAwaitingAge == nil ? Color.rupuMute : Color.status(.awaiting))
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(12)
        .panelStyle(.panel)
    }

    static func ageLabel(_ age: TimeInterval?) -> String {
        guard let age else { return "—" }
        return Fmt.duration(ms: UInt64(max(0, age) * 1000))
    }

    // MARK: - Needs-attention

    private var needsAttentionCard: some View {
        let result = deriveNeedsYou(rows: store.rows, range: range, now: Date())
        return VStack(alignment: .leading, spacing: 0) {
            Eyebrow("Needs attention")
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
            Divider()
            if result.items.isEmpty {
                Text("Nothing needs you")
                    .font(.uiText)
                    .foregroundStyle(Color.rupuMute)
                    .frame(maxWidth: .infinity, minHeight: 36, alignment: .leading)
                    .padding(.horizontal, 12)
            } else {
                VStack(spacing: 0) {
                    ForEach(result.items) { item in
                        NeedsAttentionRow(item: item, store: store, backend: backend, onOpen: onNavigate)
                        if item.id != result.items.last?.id {
                            Divider()
                        }
                    }
                }
                if result.overflow > 0 {
                    Text("+\(result.overflow) more not shown")
                        .font(.metaText)
                        .foregroundStyle(Color.rupuDim)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 8)
                        .overlay(alignment: .top) {
                            Rectangle().fill(Color.rupuBorder).frame(height: 1)
                        }
                }
            }
        }
        .panelStyle(.panel)
    }
}

/// One needs-attention row: kind tag + subject + age, with inline gate
/// actions for a `.gate` item (via `KindTableAwaitingActions`, the same
/// resolve-then-post pair the per-kind tables use) or an "Open" button for a
/// `.failedRun` item. A compact analog of `RupuOverview.NeedsYouCard`'s row
/// — that view can't be reused directly (`RupuActivity` doesn't import
/// `RupuOverview`; see `deriveNeedsYou`'s doc comment for why), so this is a
/// deliberately smaller, Activity-local rendering of the same
/// `NeedsYouItem` data, not a byte-for-byte port of that card's chrome.
private struct NeedsAttentionRow: View {
    let item: NeedsYouItem
    let store: ActivityStore
    let backend: BackendController
    let onOpen: (Route) -> Void

    var body: some View {
        HStack(spacing: 10) {
            Circle()
                .fill(Color.status(item.kind == .gate ? .awaiting : .failed))
                .frame(width: 6, height: 6)
            Text(item.row.subject)
                .font(.uiText)
                .foregroundStyle(Color.rupuInk)
                .lineLimit(1)
                .truncationMode(.tail)
            Spacer(minLength: 8)
            Text(ageLabel)
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuDim)
            actions
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }

    private var ageLabel: String {
        guard let date = item.row.startedAt else { return "—" }
        return KindTableFormat.relative.localizedString(for: date, relativeTo: Date())
    }

    @ViewBuilder
    private var actions: some View {
        switch item.kind {
        case .gate:
            kindTableAwaitingActionsCell(item.row, store: store, backend: backend)
        case .failedRun:
            if let route = item.row.navigation.route {
                Button("Open") { onOpen(route) }
                    .buttonStyle(RupuButtonStyle.outline)
                    .controlSize(.small)
            }
        }
    }
}
