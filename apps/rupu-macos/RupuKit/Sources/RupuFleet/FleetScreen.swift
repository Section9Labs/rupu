import SwiftUI
import RupuAPI
import RupuStore
import RupuDesign

/// Sortable keys for the workers table (Phase 5A, Task 6). `ListSort`'s
/// generic `Key` — screen-owned, mirroring `ProjectsSortKey`'s own "not a
/// shared base type" convention (`ListSort.swift`'s doc comment).
public enum WorkersSortKey: Hashable, CaseIterable, Sendable {
    case name, status, active, total, lastRun
}

private enum FleetLayout {
    static let minCardWidth: CGFloat = 268
    static let cardSpacing: CGFloat = 12
    /// Matches the web's `Workers.tsx` `STALE_MS` exactly — `APIWorkerRow`
    /// carries no server-computed status field at all (`WorkerView` on the
    /// Rust side is a bare `WorkerRecord` + run-activity counts, see that
    /// type's doc comment), so "stale" is a client-derived freshness read
    /// on `lastSeenAt`, same derivation the web page already performs.
    static let workerStaleInterval: TimeInterval = 5 * 60
    static let workerStatus: CGFloat = 64
    static let workerActive: CGFloat = 64
    static let workerTotal: CGFloat = 64
    static let workerLastRun: CGFloat = 104
}

/// The Fleet screen (Phase 5A, Task 6), replacing the `.fleet` placeholder:
/// host cards (auto-fill grid, min 268pt) above a sortable workers table.
/// Owns a `FleetStore` lifecycle the same lazy-build/`storeClientID`-rebuild
/// convention `ProjectsScreen`/`RunDetailScreen` already established, plus
/// the 60s reconcile loop's own activate/deactivate pair (`.task` starts it,
/// `.onDisappear` stops it — mirrors `RootView`'s `HostsFooterStore` usage,
/// scoped to this screen's own lifetime instead of the whole app's).
///
/// **Does NOT need `OverviewScreen`'s cold-launch `.onChange(of: backend.
/// health)` fix** — same reasoning `ProjectsScreen`'s own doc comment gives:
/// `.fleet` is only ever reached by a sidebar click, well after the shell's
/// own connection attempt has already resolved.
///
/// **No add-host forms** — the umbrella spec's disposition for this phase
/// (`docs/macOS_design/HANDOFF.md` §6, "Fleet"): a quiet footer note points
/// at the real CLI command instead of building a write form this phase
/// doesn't need. `POST /api/hosts` (and its `/ssh`/`/bucket`/`/node`
/// siblings) stay unconsumed by this screen.
public struct FleetScreen: View {
    @Bindable var model: AppModel
    let backend: BackendController

    @State private var store: FleetStore?
    @State private var storeClientID: ObjectIdentifier?
    @State private var workerSort = ListSort<WorkersSortKey>(key: .name, ascending: true)
    @State private var pendingRemoval: APIHostRow?

    public init(model: AppModel, backend: BackendController) {
        self.model = model
        self.backend = backend
    }

    public var body: some View {
        Group {
            if let store {
                content(store: store)
            } else {
                centeredLabel("Backend not connected")
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(Color.rupuBg)
        .task {
            await activate()
        }
        .onDisappear {
            store?.deactivate()
        }
        .confirmationDialog(
            "Remove \(pendingRemoval?.name ?? "host")?",
            isPresented: removalDialogBinding,
            presenting: pendingRemoval
        ) { host in
            Button("Remove", role: .destructive) {
                let removedID = host.id
                pendingRemoval = nil
                Task { await store?.removeHost(id: removedID) }
            }
            Button("Cancel", role: .cancel) { pendingRemoval = nil }
        } message: { host in
            Text("\(host.name) leaves the fleet immediately — new runs can no longer target it. This does not affect runs already placed there.")
        }
    }

    private var removalDialogBinding: Binding<Bool> {
        Binding(get: { pendingRemoval != nil }, set: { if !$0 { pendingRemoval = nil } })
    }

    /// Builds (or rebuilds, on a backend client swap) the store and
    /// activates it — same `storeClientID` recipe `ProjectsScreen`/
    /// `RunDetailScreen` already use (see `BackendController.
    /// clientIdentity()`'s doc comment). `pendingActions` is the shared
    /// app-wide ledger (`BackendController.pendingActions`), not a private
    /// one — a remove fired from Fleet must read as `.pending` consistently
    /// wherever else this codebase might one day surface it, same rationale
    /// `RunDetailStore`/`ActivityStore` already document for sharing it.
    private func activate() async {
        guard let client = backend.client() else { return }
        let clientID = backend.clientIdentity()
        if store == nil || storeClientID != clientID {
            store = FleetStore(client: client, pendingActions: backend.pendingActions)
            storeClientID = clientID
        }
        guard let store else { return }
        await store.activate()
    }

    // MARK: - Layout

    private func content(store: FleetStore) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                hostsSection(store: store)
                workersSection(store: store)
                footerNote
            }
            .padding(16)
        }
    }

    // MARK: - Hosts (cards)

    @ViewBuilder
    private func hostsSection(store: FleetStore) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Eyebrow("Hosts")
            switch store.hosts {
            case .loading:
                loadingBlock
            case .failed(let message):
                failedBlock(message, subject: "hosts")
            case .empty:
                emptyBlock("No hosts registered")
            case .content(let rows):
                hostsGrid(rows: rows, store: store)
            }
        }
    }

    private func hostsGrid(rows: [APIHostRow], store: FleetStore) -> some View {
        LazyVGrid(
            columns: [GridItem(.adaptive(minimum: FleetLayout.minCardWidth), spacing: FleetLayout.cardSpacing)],
            spacing: FleetLayout.cardSpacing
        ) {
            ForEach(rows, id: \.id) { host in
                HostCard(
                    host: host,
                    pendingActions: store.pendingActions,
                    onRequestRemove: { pendingRemoval = host },
                    onRetryRemove: { Task { await store.removeHost(id: host.id) } }
                )
            }
        }
    }

    // MARK: - Workers (sortable table)

    @ViewBuilder
    private func workersSection(store: FleetStore) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Eyebrow("Workers")
            switch store.workers {
            case .loading:
                loadingBlock
            case .failed(let message):
                failedBlock(message, subject: "workers")
            case .empty:
                emptyBlock("No workers registered")
            case .content(let rows):
                workersTable(rows: rows)
            }
        }
    }

    private func workersTable(rows: [APIWorkerRow]) -> some View {
        let sorted = sortRows(rows, sort: workerSort, value: Self.workerSortValue)
        return VStack(alignment: .leading, spacing: 0) {
            SortableHeaderRow(columns: workerColumns, sort: $workerSort)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
            Divider()
            ForEach(sorted, id: \.workerID) { row in
                WorkerListRow(row: row)
                Divider()
            }
        }
        .panelStyle(.panel)
    }

    private var workerColumns: [SortableColumn<WorkersSortKey>] {
        [
            SortableColumn(key: .name, label: "Name", width: nil),
            SortableColumn(key: .status, label: "Status", width: FleetLayout.workerStatus, alignment: .trailing),
            SortableColumn(key: .active, label: "Active", width: FleetLayout.workerActive, alignment: .trailing),
            SortableColumn(key: .total, label: "Total", width: FleetLayout.workerTotal, alignment: .trailing),
            SortableColumn(key: .lastRun, label: "Last run", width: FleetLayout.workerLastRun, alignment: .trailing),
        ]
    }

    /// `.text`/`.number`/`.date` per `sortRows`'s null-discipline contract.
    /// `.status` sorts fresh-before-stale ascending (`0`/`1`) — there is no
    /// server field to sort on (see `FleetLayout.workerStaleInterval`'s doc
    /// comment), so this is the same derived freshness read the row itself
    /// renders, just expressed as a number `sortRows` can order.
    private static func workerSortValue(_ row: APIWorkerRow, _ key: WorkersSortKey) -> ListSortValue {
        switch key {
        case .name: .text(row.name)
        case .status: .number(workerIsStale(row.lastSeenAt) ? 1 : 0)
        case .active: .number(Double(row.activeRunCount))
        case .total: .number(Double(row.totalRunCount))
        case .lastRun: .date(ActivityRow.parseISO(row.lastRunAt))
        }
    }

    // MARK: - Shared blocks

    private var loadingBlock: some View {
        HStack {
            Spacer(minLength: 0)
            ProgressView().controlSize(.small)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, minHeight: 100)
        .panelStyle(.panel)
    }

    private func emptyBlock(_ label: String) -> some View {
        HStack {
            Spacer(minLength: 0)
            Text(label).font(.noteText).foregroundStyle(Color.rupuMute)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, minHeight: 100)
        .panelStyle(.panel)
    }

    private func failedBlock(_ message: String, subject: String) -> some View {
        TintBanner(tone: Color.status(.failed), toneBg: Color.status(.failed).opacity(0.08)) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Failed to load \(subject)")
                    .font(.noteText.weight(.semibold))
                    .foregroundStyle(Color.status(.failed))
                Text(message)
                    .font(.noteText)
                    .foregroundStyle(Color.status(.failed))
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private func centeredLabel(_ label: String) -> some View {
        VStack {
            Spacer(minLength: 0)
            Text(label).font(.noteText).foregroundStyle(Color.rupuMute)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    /// The "no add-host forms" disposition, made visible rather than silent
    /// — points at the real CLI surface (`crates/rupu-cli/src/cmd/host.rs`'s
    /// `Action::Add`) instead of a form this phase doesn't build.
    private var footerNote: some View {
        Text("Add hosts from the CLI — \u{2068}rupu host add <name> --url <url>\u{2069}")
            .font(.noteText)
            .foregroundStyle(Color.rupuMute)
            .padding(.top, 4)
    }
}

/// `true` when `lastSeenAt` predates `FleetLayout.workerStaleInterval`
/// (5 minutes, matching the web's `Workers.tsx`) or fails to parse — an
/// unparseable timestamp is treated the same as "definitely not fresh"
/// rather than silently reading as fresh.
private func workerIsStale(_ lastSeenAt: String) -> Bool {
    guard let date = ActivityRow.parseISO(lastSeenAt) else { return true }
    return Date().timeIntervalSince(date) > FleetLayout.workerStaleInterval
}

// MARK: - Host card

/// One host: name + overflow menu, status/transport/version meta line,
/// active-run count — plus, when faulted (`status != "online"`), a fail
/// border, an inset 3px fail-tone edge, and an err-bg cause panel (umbrella
/// spec §6 / `docs/macOS_design/HANDOFF.md`'s Fleet line). The overflow
/// menu (and the whole remove path) is omitted entirely for `"local"` — the
/// server hard-rejects removing it (`crates/rupu-cp/src/api/hosts.rs::
/// remove_host`), so there is no honest control to offer here.
private struct HostCard: View {
    let host: APIHostRow
    let pendingActions: PendingActions
    let onRequestRemove: () -> Void
    let onRetryRemove: () -> Void

    private var isOnline: Bool { host.status == "online" }
    private var isLocal: Bool { host.id == "local" }
    private var removeKey: ActionKey { ActionKey(host.id, .remove) }

    private var isPending: Bool {
        if case .pending = pendingActions.state(removeKey) { return true }
        return false
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            header
            metaLine
            activeRunsLine
            if !isOnline {
                causePanel
            }
            if case .failed(let message) = pendingActions.state(removeKey) {
                failureNote(message)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .opacity(isPending ? 0.6 : 1)
        .background(Color.rupuPanel)
        .clipShape(RoundedRectangle(cornerRadius: 6))
        .overlay(
            RoundedRectangle(cornerRadius: 6)
                .stroke(isOnline ? Color.rupuBorder : Color.rupuErr, lineWidth: 1)
        )
        .overlay(alignment: .leading) {
            if !isOnline {
                RoundedRectangle(cornerRadius: 1.5)
                    .fill(Color.rupuErr)
                    .frame(width: 3)
                    .padding(.vertical, 6)
                    .padding(.leading, 3)
            }
        }
    }

    private var header: some View {
        HStack(alignment: .top, spacing: 8) {
            Text(host.name)
                .font(.leadText.weight(.semibold))
                .foregroundStyle(Color.rupuInk)
                .lineLimit(1)
                .truncationMode(.tail)
            Spacer(minLength: 0)
            if !isLocal {
                overflowMenu
            }
        }
    }

    private var overflowMenu: some View {
        Menu {
            Button("Remove…", role: .destructive) { onRequestRemove() }
                .disabled(isPending)
        } label: {
            Icon(.moreHorizontal)
                .foregroundStyle(Color.rupuDim)
        }
        .menuStyle(.borderlessButton)
        .frame(width: 20)
        .disabled(isPending)
    }

    private var metaLine: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(isOnline ? Color.status(.done) : Color.rupuErr)
                .frame(width: 6, height: 6)
            Text(host.status.uppercased())
                .font(.dataMono(9))
                .kerning(1.0)
                .foregroundStyle(isOnline ? Color.rupuDim : Color.rupuErr)
            Text("·").font(.metaText).foregroundStyle(Color.rupuMute)
            Text(host.transportKind)
                .font(.dataMono(10))
                .foregroundStyle(Color.rupuMute)
            Spacer(minLength: 0)
            Text(host.version.map { "v\($0)" } ?? "—")
                .font(.dataMono(10))
                .foregroundStyle(Color.rupuMute)
        }
    }

    private var activeRunsLine: some View {
        HStack(spacing: 4) {
            Text(Fmt.count(host.activeRunCount))
                .font(.dataMono(13))
                .foregroundStyle(Color.rupuInk)
            Text("active runs")
                .font(.noteText)
                .foregroundStyle(Color.rupuDim)
            Spacer(minLength: 0)
        }
    }

    /// `HostView` on the Rust side (`crates/rupu-cp/src/api/hosts.rs`) never
    /// reports WHY a probe failed — only the resulting `status`. Rather than
    /// fabricate an explanation the server never sent, this panel is built
    /// entirely from fields `APIHostRow` genuinely carries: `status` plus
    /// `lastSeenAt`, the closest honest proxy for "cause" this API surface
    /// actually offers.
    private var causePanel: some View {
        TintBanner(tone: Color.rupuErr, toneBg: Color.rupuErrBg) {
            Text(causeText)
                .font(.noteText)
                .foregroundStyle(Color.rupuErr)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private var causeText: String {
        guard let lastSeenAt = host.lastSeenAt, let date = ActivityRow.parseISO(lastSeenAt) else {
            return "\(host.status.capitalized) — never seen"
        }
        let relative = Self.relativeFormatter.localizedString(for: date, relativeTo: Date())
        return "\(host.status.capitalized) — last seen \(relative)"
    }

    /// Mirrors `RunDetailScreen.runVerbFailureNotes`'s message+Retry
    /// convention: a failed removal must stay visible (not just silently
    /// re-enable the menu), and Retry re-fires the mutation directly — no
    /// second confirmation dialog, same as every other retry-a-failed-
    /// mutation control in this codebase.
    private func failureNote(_ message: String) -> some View {
        HStack(alignment: .top, spacing: 6) {
            Text("Remove failed: \(message)")
                .font(.metaText)
                .foregroundStyle(Color.rupuErr)
                .lineLimit(2)
                .frame(maxWidth: .infinity, alignment: .leading)
            Button("Retry") { onRetryRemove() }
                .buttonStyle(.plain)
                .font(.metaText.weight(.semibold))
                .foregroundStyle(Color.rupuBrand)
        }
    }

    private static let relativeFormatter: RelativeDateTimeFormatter = {
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter
    }()
}

// MARK: - Worker row

/// One worker row: name + short id, freshness (fresh/stale, derived —
/// see `workerIsStale(_:)`), active/total run counts, last-run relative
/// timestamp.
///
/// **`row.capabilities` (backends/scmHosts/permissionModes) is deliberately
/// unrendered** — the brief's column set is name/status/active/total/
/// last-run only, and there is no sixth column here for it (unlike the
/// web's `Workers.tsx`, which has room for a dedicated Capabilities column
/// alongside Kind/Host/Version). Surfacing it is a legitimate follow-up for
/// a future pass, not an oversight.
private struct WorkerListRow: View {
    let row: APIWorkerRow

    private var stale: Bool { workerIsStale(row.lastSeenAt) }

    var body: some View {
        HStack(spacing: 0) {
            nameCell
            statusCell
            countCell(Fmt.count(Int(row.activeRunCount)), width: FleetLayout.workerActive)
            countCell(Fmt.count(Int(row.totalRunCount)), width: FleetLayout.workerTotal)
            lastRunCell
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }

    private var nameCell: some View {
        VStack(alignment: .leading, spacing: 1) {
            Text(row.name)
                .foregroundStyle(Color.rupuInk)
                .lineLimit(1)
                .truncationMode(.tail)
            Text("\(row.kind) · \(row.host)")
                .font(.metaText)
                .foregroundStyle(Color.rupuMute)
                .lineLimit(1)
                .truncationMode(.tail)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.trailing, 8)
    }

    private var statusCell: some View {
        Text(stale ? "stale" : "fresh")
            .font(.dataMono(10))
            .foregroundStyle(stale ? Color.rupuWarn : Color.rupuDim)
            .frame(width: FleetLayout.workerStatus, alignment: .trailing)
            .padding(.trailing, 8)
    }

    private func countCell(_ text: String, width: CGFloat) -> some View {
        Text(text)
            .font(.dataMono(11))
            .foregroundStyle(Color.rupuInk)
            .frame(width: width, alignment: .trailing)
            .padding(.trailing, 8)
    }

    private var lastRunCell: some View {
        Text(lastRunLabel)
            .font(.dataMono(11))
            .foregroundStyle(Color.rupuDim)
            .frame(width: FleetLayout.workerLastRun, alignment: .trailing)
    }

    private var lastRunLabel: String {
        guard let date = ActivityRow.parseISO(row.lastRunAt) else { return "—" }
        return Self.relativeFormatter.localizedString(for: date, relativeTo: Date())
    }

    private static let relativeFormatter: RelativeDateTimeFormatter = {
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter
    }()
}
