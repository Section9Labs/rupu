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

    /// View-local sort state (Phase 3, Task 5) — applied over `rows` (which
    /// is `store.rows`) in the body below, so a live-tail patch that
    /// mutates `store.rows` re-sorts on the next render with no separate
    /// trigger needed. Defaults to today's merge-time order
    /// (`ActivityStore.isOrderedByStartedAtDescending`, reproduced exactly
    /// by `sortActivityRows(_:by:)`'s `.started`/descending case).
    @State private var sort = ActivitySort(key: .started, ascending: false)

    private typealias Layout = ActivityTableLayout

    private var sortedRows: [ActivityRow] {
        sortActivityRows(rows, by: sort)
    }

    var body: some View {
        // RenderMeter seam (Plan 5, Task 1) — one line, safe to delete.
        let _ = RenderMeter.tick("ActivityTable")
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider()
            List(sortedRows) { row in
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
        row.status == .awaiting ? Color.status(.awaiting).opacity(0.04) : .clear
    }

    private var header: some View {
        HStack(spacing: 0) {
            headerCell("Status", width: Layout.status, key: .status)
            headerCell("Kind", width: Layout.kind, key: .kind)
            headerCell("Subject", width: nil, key: .subject)
            headerCell("Project", width: Layout.project, key: .project)
            headerCell("Host", width: Layout.host, key: .host)
            headerCell("Trigger", width: Layout.trigger, key: .trigger)
            headerCell("Dur", width: Layout.duration, alignment: .trailing, key: .duration)
            headerCell("Cost", width: Layout.cost, alignment: .trailing, key: .cost)
            headerCell("Started", width: Layout.started, alignment: .trailing, key: .started)
            headerCell("", width: Layout.actions, alignment: .trailing)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }

    /// A header cell, plain (`key == nil`, e.g. the actions column's blank
    /// header) or sortable — tapping a sortable header makes it the active
    /// sort key (first tap uses `key.defaultAscending`) or, if it's already
    /// the active key, flips direction.
    @ViewBuilder
    private func headerCell(_ title: String, width: CGFloat?, alignment: Alignment = .leading, key: ActivitySort.Key? = nil) -> some View {
        if let width {
            headerCellContent(title, alignment: alignment, key: key)
                .frame(width: width, alignment: alignment)
                .padding(.trailing, 8)
        } else {
            headerCellContent(title, alignment: alignment, key: key)
                .frame(maxWidth: .infinity, alignment: alignment)
                .padding(.trailing, 8)
        }
    }

    @ViewBuilder
    private func headerCellContent(_ title: String, alignment: Alignment, key: ActivitySort.Key?) -> some View {
        if let key {
            Button {
                toggleSort(key)
            } label: {
                HStack(spacing: 4) {
                    if alignment == .trailing {
                        sortIndicator(for: key)
                        Eyebrow(title)
                    } else {
                        Eyebrow(title)
                        sortIndicator(for: key)
                    }
                }
            }
            .buttonStyle(.plain)
        } else {
            Eyebrow(title)
        }
    }

    /// The active header's direction chevron — space is always reserved
    /// (an invisible chevron on every other sortable header) so toggling
    /// the active column never jiggles the header row's layout.
    private func sortIndicator(for key: ActivitySort.Key) -> some View {
        Icon(sort.key == key && !sort.ascending ? .chevronDown : .chevronUp, size: 9)
            .foregroundStyle(Color.rupuDim)
            .opacity(sort.key == key ? 1 : 0)
    }

    private func toggleSort(_ key: ActivitySort.Key) {
        if sort.key == key {
            sort.ascending.toggle()
        } else {
            sort = ActivitySort(key: key, ascending: key.defaultAscending)
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

    /// Local busy flag for the compact ✓/✕ pair — deliberately NOT read
    /// from `store.pendingActions` (fix round 1): this row doesn't know
    /// which gate it's targeting until `resolveSoleAwaitingGate` answers,
    /// so there is no `ActionKey.gate(runID:stepID:verb:)` to look up yet
    /// at the moment the button is tapped. This flag only covers the
    /// resolve-then-post round trip's own UI feedback (spinner + disable,
    /// double-tap guard); the mutation itself still lands in the shared
    /// `pendingActions` ledger once `store.approve`/`store.reject` runs
    /// with the now-known gate id, via the exact same composite key
    /// `RunDetailStore`'s own banner uses for that gate.
    @State private var isBusy = false

    private typealias Layout = ActivityTableLayout

    private var isClickable: Bool { row.navigation != .none }

    var body: some View {
        HStack(spacing: 0) {
            statusCell.frame(width: Layout.status, alignment: .leading).padding(.trailing, 8)
            Text(row.kind.displayLabel)
                .font(.metaText)
                .foregroundStyle(Color.rupuDim)
                .frame(width: Layout.kind, alignment: .leading)
                .padding(.trailing, 8)
            Text(row.subject)
                .foregroundStyle(Color.rupuInk)
                .lineLimit(1)
                .truncationMode(.tail)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.trailing, 8)
                .help(row.subject)
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
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuInk)
                .frame(width: Layout.duration, alignment: .trailing)
                .padding(.trailing, 8)
            Text(costLabel)
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuInk)
                .frame(width: Layout.cost, alignment: .trailing)
                .padding(.trailing, 8)
            Text(startedLabel)
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuDim)
                .frame(width: Layout.started, alignment: .trailing)
            awaitingActions
                .frame(width: Layout.actions, alignment: .trailing)
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
            HStack(spacing: 6) {
                Button {
                    Task { await resolveGateAndApprove(runID: runID, host: host) }
                } label: {
                    if isBusy {
                        ProgressView().controlSize(.mini)
                    } else {
                        Icon(.checkCircle2, size: 14)
                    }
                }
                .buttonStyle(.plain)
                .foregroundStyle(Color.status(.done))
                .disabled(isBusy)
                .help("Approve")

                Button {
                    Task { await resolveGateAndReject(runID: runID, host: host) }
                } label: {
                    if isBusy {
                        ProgressView().controlSize(.mini)
                    } else {
                        Icon(.xCircle, size: 14)
                    }
                }
                .buttonStyle(.plain)
                .foregroundStyle(Color.status(.failed))
                .disabled(isBusy)
                .help("Reject")
            }
        }
    }

    /// `GET /api/runs/workflows` (the source this row came from) carries no
    /// `awaiting[]` detail — only `GET /api/runs/:id` does — so a compact
    /// row's tap resolves the gate id itself (via `ActivityStore.
    /// resolveSoleAwaitingGate(client:runID:host:)` — Phase 4, Task 5 fix
    /// round 1 lifted this out of a local private method here, since
    /// `NeedsYouCard` needed the identical resolution and this is the one
    /// shared home for it now) before calling `store.approve`, same "gate
    /// targeting always explicit" contract every write route in this phase
    /// follows (never omitted, never guessed). A run with no resolvable
    /// single gate (already resolved server-side by the time this lands, or
    /// the API otherwise disagrees with the row's own `.awaiting` status) is
    /// a silent no-op — the row's next live-patch or refresh will correct
    /// its status either way. `isBusy` brackets the whole resolve-then-post
    /// round trip so a double-tap can't fire two overlapping gate lookups.
    private func resolveGateAndApprove(runID: String, host: String?) async {
        isBusy = true
        defer { isBusy = false }
        guard let client = backend.client() else { return }
        guard let gate = await ActivityStore.resolveSoleAwaitingGate(client: client, runID: runID, host: host) else { return }
        await store.approve(runID: runID, gate: gate, host: host)
    }

    private func resolveGateAndReject(runID: String, host: String?) async {
        isBusy = true
        defer { isBusy = false }
        guard let client = backend.client() else { return }
        guard let gate = await ActivityStore.resolveSoleAwaitingGate(client: client, runID: runID, host: host) else { return }
        await store.reject(runID: runID, gate: gate, host: host)
    }

    private var statusCell: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(Color.status(row.status.tone))
                .frame(width: 6, height: 6)
            Text(row.status.displayLabel)
                .font(.metaText)
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

// `ActivityStatus.displayLabel` moved to `RupuStore/ActivityRow.swift`
// (flows-composition Task 3): the command palette's run search needed it
// too, and RupuStore is the shared base both this module and `PaletteStore`
// depend on.
