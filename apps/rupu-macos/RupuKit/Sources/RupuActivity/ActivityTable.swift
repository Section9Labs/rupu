import SwiftUI
import AppKit
import RupuAPI
import RupuStore
import RupuDesign

/// Column widths shared by `ActivityTable`'s header row and every
/// `ActivityTableRow` — declared once so the two can never drift out of
/// alignment with each other.
private enum ActivityTableLayout {
    static let status: CGFloat = 100
    static let kind: CGFloat = 84
    static let project: CGFloat = 120
    static let host: CGFloat = 96
    static let trigger: CGFloat = 92
    static let duration: CGFloat = 64
    static let cost: CGFloat = 64
    static let started: CGFloat = 88
    /// Phase 3, Task 5: the compact Approve/Reject column for `.awaiting`
    /// rows — empty (but still width-reserved, so every row stays aligned)
    /// for every other status.
    static let actions: CGFloat = 52
}

/// The merged execution table. Built on `List` rather than SwiftUI's stock
/// `Table`: this screen needs a per-row background tint (`.awaiting` rows)
/// and a per-row conditional tap affordance (`.none`-navigation rows are
/// inert, no hover/cursor change) — `Table` on macOS has no supported hook
/// for either, `List` does (`.listRowBackground`, a plain `.onTapGesture` +
/// `.onHover` per row).
struct ActivityTable: View {
    let rows: [ActivityRow]
    let store: ActivityStore
    let backend: BackendController
    let onSelect: (ActivityRow) -> Void

    private typealias Layout = ActivityTableLayout

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider()
            List(rows) { row in
                ActivityTableRow(row: row, store: store, backend: backend, onSelect: onSelect)
                    .listRowBackground(rowBackground(row))
                    .listRowSeparator(.visible)
                    .listRowInsets(EdgeInsets(top: 0, leading: 0, bottom: 0, trailing: 0))
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
        }
        .panelStyle(.panel)
    }

    private func rowBackground(_ row: ActivityRow) -> Color {
        row.status == .awaiting ? Color.status(.waiting).opacity(0.04) : .clear
    }

    private var header: some View {
        HStack(spacing: 0) {
            headerCell("Status", width: Layout.status)
            headerCell("Kind", width: Layout.kind)
            headerCell("Subject", width: nil)
            headerCell("Project", width: Layout.project)
            headerCell("Host", width: Layout.host)
            headerCell("Trigger", width: Layout.trigger)
            headerCell("Dur", width: Layout.duration, alignment: .trailing)
            headerCell("Cost", width: Layout.cost, alignment: .trailing)
            headerCell("Started", width: Layout.started, alignment: .trailing)
            headerCell("", width: Layout.actions, alignment: .trailing)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }

    @ViewBuilder
    private func headerCell(_ title: String, width: CGFloat?, alignment: Alignment = .leading) -> some View {
        if let width {
            MicroLabel(title)
                .foregroundStyle(Color.rupuMute)
                .frame(width: width, alignment: alignment)
                .padding(.trailing, 8)
        } else {
            MicroLabel(title)
                .foregroundStyle(Color.rupuMute)
                .frame(maxWidth: .infinity, alignment: alignment)
                .padding(.trailing, 8)
        }
    }
}

/// One merged-feed row: status glyph + label, kind, subject, project, host,
/// trigger, duration, cost, and a relative "started" timestamp. Tappable
/// only when `row.navigation != .none` — a session-less autoflow event (no
/// run yet materialized) has nothing to navigate to, so it gets no tap
/// handler and no hover cursor, per the brief's "no dead controls" rule.
private struct ActivityTableRow: View {
    let row: ActivityRow
    let store: ActivityStore
    let backend: BackendController
    let onSelect: (ActivityRow) -> Void

    private typealias Layout = ActivityTableLayout

    private var isClickable: Bool { row.navigation != .none }

    var body: some View {
        HStack(spacing: 0) {
            statusCell.frame(width: Layout.status, alignment: .leading).padding(.trailing, 8)
            MicroLabel(row.kind.displayLabel)
                .foregroundStyle(Color.rupuDim)
                .frame(width: Layout.kind, alignment: .leading)
                .padding(.trailing, 8)
            Text(row.subject)
                .foregroundStyle(Color.rupuInk)
                .lineLimit(1)
                .truncationMode(.tail)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.trailing, 8)
            Text(row.project ?? "—")
                .foregroundStyle(Color.rupuDim)
                .lineLimit(1)
                .frame(width: Layout.project, alignment: .leading)
                .padding(.trailing, 8)
            Text(row.host)
                .foregroundStyle(Color.rupuDim)
                .lineLimit(1)
                .frame(width: Layout.host, alignment: .leading)
                .padding(.trailing, 8)
            Text(row.trigger ?? "—")
                .foregroundStyle(Color.rupuDim)
                .lineLimit(1)
                .frame(width: Layout.trigger, alignment: .leading)
                .padding(.trailing, 8)
            Text(durationLabel)
                .font(.numeral(size: 11))
                .foregroundStyle(Color.rupuInk)
                .frame(width: Layout.duration, alignment: .trailing)
                .padding(.trailing, 8)
            Text(costLabel)
                .font(.numeral(size: 11))
                .foregroundStyle(Color.rupuInk)
                .frame(width: Layout.cost, alignment: .trailing)
                .padding(.trailing, 8)
            Text(startedLabel)
                .font(.numeral(size: 11))
                .foregroundStyle(Color.rupuDim)
                .frame(width: Layout.started, alignment: .trailing)
            awaitingActions
                .frame(width: Layout.actions, alignment: .trailing)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .contentShape(Rectangle())
        .onTapGesture {
            guard isClickable else { return }
            onSelect(row)
        }
        .onHover { hovering in
            guard isClickable else { return }
            if hovering {
                NSCursor.pointingHand.push()
            } else {
                NSCursor.pop()
            }
        }
    }

    // MARK: - Awaiting-row inline actions (Phase 3, Task 5)

    /// Compact ✓/✕ pair for an `.awaiting` row — `.none` for every other
    /// status, still occupying `Layout.actions`' width so every row stays
    /// column-aligned. Only rendered when `row.navigation` is `.run` (an
    /// orchestrator run — the only kind this phase's approve/reject routes
    /// can target); an autoflow row with no run materialized yet
    /// (`.none` navigation) can read `.awaiting` from its own source but
    /// has nothing to approve/reject against.
    @ViewBuilder
    private var awaitingActions: some View {
        if row.status == .awaiting, case .run(let runID, let host) = row.navigation {
            let approveKey = ActionKey(runID, .approve)
            let rejectKey = ActionKey(runID, .reject)
            let approvePending = isPending(store.pendingActions.state(approveKey))
            let rejectPending = isPending(store.pendingActions.state(rejectKey))
            let anyPending = approvePending || rejectPending

            HStack(spacing: 6) {
                Button {
                    Task { await resolveGateAndApprove(runID: runID, host: host) }
                } label: {
                    if approvePending {
                        ProgressView().controlSize(.mini)
                    } else {
                        Image(systemName: "checkmark.circle.fill")
                    }
                }
                .buttonStyle(.plain)
                .foregroundStyle(Color.status(.done))
                .disabled(anyPending)
                .help("Approve")

                Button {
                    Task { await resolveGateAndReject(runID: runID, host: host) }
                } label: {
                    if rejectPending {
                        ProgressView().controlSize(.mini)
                    } else {
                        Image(systemName: "xmark.circle.fill")
                    }
                }
                .buttonStyle(.plain)
                .foregroundStyle(Color.status(.fail))
                .disabled(anyPending)
                .help("Reject")
            }
        }
    }

    private func isPending(_ state: ActionState) -> Bool {
        if case .pending = state { return true }
        return false
    }

    /// `GET /api/runs/workflows` (the source this row came from) carries no
    /// `awaiting[]` detail — only `GET /api/runs/:id` does — so a compact
    /// row's tap resolves the gate id itself before calling
    /// `store.approve`, same "gate targeting always explicit" contract
    /// every write route in this phase follows (never omitted, never
    /// guessed). A run with no resolvable single gate (already resolved
    /// server-side by the time this lands, or the API otherwise disagrees
    /// with the row's own `.awaiting` status) is a silent no-op — the row's
    /// next live-patch or refresh will correct its status either way.
    private func resolveGateAndApprove(runID: String, host: String?) async {
        guard let gate = await resolveSoleAwaitingGate(runID: runID, host: host) else { return }
        await store.approve(runID: runID, gate: gate, host: host)
    }

    private func resolveGateAndReject(runID: String, host: String?) async {
        guard let gate = await resolveSoleAwaitingGate(runID: runID, host: host) else { return }
        await store.reject(runID: runID, gate: gate, host: host)
    }

    private func resolveSoleAwaitingGate(runID: String, host: String?) async -> String? {
        guard let client = backend.client() else { return nil }
        guard let detail = try? await client.runDetail(id: runID, host: host) else { return nil }
        return detail.run.awaiting.first?.stepID
    }

    private var statusCell: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(Color.status(row.status.tone))
                .frame(width: 6, height: 6)
            MicroLabel(row.status.displayLabel)
                .foregroundStyle(Color.rupuDim)
        }
    }

    private var durationLabel: String {
        row.durationMS.map { Fmt.duration(ms: $0) } ?? "—"
    }

    private var costLabel: String {
        Fmt.cost(row.costUSD)
    }

    private var startedLabel: String {
        guard let date = row.startedAt else { return "—" }
        return Self.relativeFormatter.localizedString(for: date, relativeTo: Date())
    }

    private static let relativeFormatter: RelativeDateTimeFormatter = {
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter
    }()
}

extension ActivityKindTag {
    var displayLabel: String {
        switch self {
        case .agent: "Agent"
        case .workflow: "Workflow"
        case .autoflow: "Autoflow"
        case .session: "Session"
        }
    }
}

extension ActivityStatus {
    /// Display text for both the table's status cell and `FilterBar`'s
    /// chips. `.unknown` renders its carried raw string directly —
    /// `ActivityStatus.normalize` already puts an explicit `"—"` there for a
    /// genuinely absent status, so there's nothing further to null-guard
    /// here.
    var displayLabel: String {
        switch self {
        case .pending: "Pending"
        case .running: "Running"
        case .completed: "Completed"
        case .failed: "Failed"
        case .awaiting: "Awaiting"
        case .rejected: "Rejected"
        case .cancelled: "Cancelled"
        case .paused: "Paused"
        case .unknown(let raw): raw
        }
    }
}
