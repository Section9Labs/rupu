import SwiftUI
import AppKit
import RupuAPI
import RupuStore
import RupuDesign

public enum AgentsSortKey: Hashable, CaseIterable, Sendable {
    case name, scope, model, runs, lastRun
}

public enum WorkflowsSortKey: Hashable, CaseIterable, Sendable {
    case name, scope, autoflow, runs, lastRun
}

public enum AutoflowsSortKey: Hashable, CaseIterable, Sendable {
    case name, trigger, scope, enabled
}

private enum LibraryLayout {
    static let scope: CGFloat = 76
    static let model: CGFloat = 128
    static let runs: CGFloat = 56
    static let lastRun: CGFloat = 104
    static let trigger: CGFloat = 72
    static let enabled: CGFloat = 92
    static let launch: CGFloat = 64
}

/// The Library screen (Phase 5A, Task 7), replacing the `.library`
/// placeholder: three definition tabs (agents/workflows/autoflows), never
/// run lists (`docs/superpowers/specs/2026-08-24-rupu-macos-phase-5-breadth-
/// design.md`'s "Library" section — `docs/macOS_design/HANDOFF.md`'s older
/// "Library" line says the same, but that doc is superseded, see its own
/// header note). Owns a `LibraryStore` lifecycle the same lazy-build/
/// `storeClientID`-rebuild convention `ProjectsScreen`/`FleetScreen` already
/// established.
///
/// **Does NOT need `OverviewScreen`'s cold-launch `.onChange(of: backend.
/// health)` fix** — same reasoning `ProjectsScreen`/`FleetScreen` document:
/// `.library` is only ever reached by a sidebar click, well after the
/// shell's own connection attempt has already resolved.
///
/// **No `.onDisappear`/`deactivate()`** — unlike `FleetScreen`, `LibraryStore`
/// has no reconcile loop to stop (a one-shot `activate()` per appearance,
/// same as `ProjectsStore`); there is nothing running in the background to
/// tear down.
///
/// **Route-driven tab (Task 0, sidebar disclosure sub-items)**: `tab` is no
/// longer local `@State` — `LibraryTab` moved to `RupuStore.Route`, same
/// "sidebar children and the in-page picker share `model.route`" contract
/// `SecurityScreen`'s doc comment documents for `SecurityTab`.
public struct LibraryScreen: View {
    @Bindable var model: AppModel
    let backend: BackendController

    @State private var store: LibraryStore?
    @State private var storeClientID: ObjectIdentifier?
    @State private var agentsSort = ListSort<AgentsSortKey>(key: .name, ascending: true)
    @State private var workflowsSort = ListSort<WorkflowsSortKey>(key: .name, ascending: true)
    @State private var autoflowsSort = ListSort<AutoflowsSortKey>(key: .name, ascending: true)

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
    }

    /// Builds (or rebuilds, on a backend client swap) the store and
    /// activates it. Same `storeClientID` recipe `ProjectsScreen`/
    /// `FleetScreen` use — see `BackendController.clientIdentity()`'s doc
    /// comment. `pendingActions` is the shared app-wide ledger, same
    /// rationale `FleetStore`'s own `activate()` doc comment gives.
    private func activate() async {
        guard let client = backend.client() else { return }
        let clientID = backend.clientIdentity()
        if store == nil || storeClientID != clientID {
            store = LibraryStore(client: client, pendingActions: backend.pendingActions)
            storeClientID = clientID
        }
        guard let store else { return }
        await store.activate()
    }

    // MARK: - Layout

    private func content(store: LibraryStore) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                tabPicker
                tabContent(store: store)
            }
            .padding(16)
        }
    }

    /// `model.route`'s associated tab — see `SecurityScreen.tab`'s identical
    /// doc comment for the fallback rationale.
    private var tab: LibraryTab {
        if case .library(let tab) = model.route { return tab }
        return .agents
    }

    private var tabBinding: Binding<LibraryTab> {
        Binding(get: { tab }, set: { model.route = .library($0) })
    }

    private var tabPicker: some View {
        // `.labelsHidden()` (review fix, final wave) — without it the
        // `Picker`'s "Tab" label string rendered visibly above the
        // segmented control (GUI-only nit; `make macos-test` stayed green
        // throughout since nothing asserted the label's visibility).
        Picker("Tab", selection: tabBinding) {
            ForEach(LibraryTab.allCases, id: \.self) { candidate in
                Text(candidate.title).tag(candidate)
            }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .frame(maxWidth: 360)
    }

    @ViewBuilder
    private func tabContent(store: LibraryStore) -> some View {
        switch tab {
        case .agents:
            agentsSection(store: store)
        case .workflows:
            workflowsSection(store: store)
        case .autoflows:
            autoflowsSection(store: store)
        }
    }

    // MARK: - Agents tab

    @ViewBuilder
    private func agentsSection(store: LibraryStore) -> some View {
        switch store.agents {
        case .loading:
            loadingBlock
        case .failed(let message):
            FailedBlock(subject: "agents", message: message, retry: { await store.loadAgents() })
        case .empty:
            emptyBlock("No agent definitions")
        case .content(let rows):
            agentsTable(rows: rows)
        }
    }

    private func agentsTable(rows: [AgentDefinition]) -> some View {
        let sorted = sortRows(rows, sort: agentsSort, value: Self.agentSortValue)
        return VStack(alignment: .leading, spacing: 0) {
            SortableHeaderRow(columns: agentColumns, sort: $agentsSort)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
            Divider()
            VStack(spacing: 0) {
                // `id: \.rowIdentity` (backlog row 20 fix), not `\.offset` —
                // an offset id ties a row's SwiftUI identity to its array
                // position, so a header sort re-ordering `sorted` would
                // reassign every row's identity to whichever definition
                // lands on its old offset instead of following it — see
                // `AgentDefinition.rowIdentity`'s doc comment for why the
                // identity itself must be composite (name alone collides
                // across scopes).
                ForEach(sorted, id: \.rowIdentity) { def in
                    AgentDefRow(def: def, onSelect: {
                        model.navigate(to: .agentDefinition(name: def.name, scopeKind: def.scopeKind, scopeID: def.scopeID))
                    }, onLaunch: {
                        model.presentLauncher(kind: .agentRun, name: def.name, scopeKind: def.scopeKind, scopeID: def.scopeID)
                    })
                    Divider()
                }
            }
        }
        .panelStyle(.panel)
    }

    private var agentColumns: [SortableColumn<AgentsSortKey>] {
        [
            SortableColumn(key: .name, label: "Name", width: nil),
            // Text columns, trailing-aligned for metadata visual consistency
            // (review fix I-3): `firstTapAscending: true` overrides the
            // alignment heuristic (which would otherwise default a
            // trailing column to descending-first) since scope/model are
            // text, not numeric/date.
            SortableColumn(key: .scope, label: "Scope", width: LibraryLayout.scope, alignment: .trailing, firstTapAscending: true),
            SortableColumn(key: .model, label: "Model", width: LibraryLayout.model, alignment: .trailing, firstTapAscending: true),
            SortableColumn(key: .runs, label: "Runs", width: LibraryLayout.runs, alignment: .trailing),
            SortableColumn(key: .lastRun, label: "Last run", width: LibraryLayout.lastRun, alignment: .trailing),
            SortableColumn(key: nil, label: "", width: LibraryLayout.launch, alignment: .trailing),
        ]
    }

    // Not `private` (widened for the reorder-under-sort regression test in
    // `RupuLibraryTests` — `@testable import RupuLibrary` reaches `internal`,
    // never `private`): still module-internal, never `public` API.
    static func agentSortValue(_ row: AgentDefinition, _ key: AgentsSortKey) -> ListSortValue {
        switch key {
        case .name: .text(row.name)
        case .scope: .text(row.scope)
        case .model: .text(row.model)
        case .runs: .number(Double(row.runCount))
        case .lastRun: .date(ActivityRow.parseISO(row.lastRun))
        }
    }

    // MARK: - Workflows tab

    @ViewBuilder
    private func workflowsSection(store: LibraryStore) -> some View {
        switch store.workflows {
        case .loading:
            loadingBlock
        case .failed(let message):
            FailedBlock(subject: "workflows", message: message, retry: { await store.loadWorkflows() })
        case .empty:
            emptyBlock("No workflow definitions")
        case .content(let rows):
            workflowsTable(rows: rows)
        }
    }

    private func workflowsTable(rows: [WorkflowDefinition]) -> some View {
        let sorted = sortRows(rows, sort: workflowsSort, value: Self.workflowSortValue)
        return VStack(alignment: .leading, spacing: 0) {
            SortableHeaderRow(columns: workflowColumns, sort: $workflowsSort)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
            Divider()
            VStack(spacing: 0) {
                // See `agentsTable`'s identical `id: \.rowIdentity` comment
                // above — same fix, same rationale.
                ForEach(sorted, id: \.rowIdentity) { def in
                    WorkflowDefRow(def: def, onSelect: {
                        model.navigate(to: .workflowDefinition(name: def.name, scopeKind: def.scopeKind, scopeID: def.scopeID))
                    }, onLaunch: {
                        model.presentLauncher(kind: .workflow, name: def.name, scopeKind: def.scopeKind, scopeID: def.scopeID)
                    })
                    Divider()
                }
            }
        }
        .panelStyle(.panel)
    }

    private var workflowColumns: [SortableColumn<WorkflowsSortKey>] {
        [
            SortableColumn(key: .name, label: "Name", width: nil),
            SortableColumn(key: .scope, label: "Scope", width: LibraryLayout.scope, alignment: .trailing),
            SortableColumn(key: .autoflow, label: "Autoflow", width: LibraryLayout.enabled, alignment: .trailing),
            SortableColumn(key: .runs, label: "Runs", width: LibraryLayout.runs, alignment: .trailing),
            SortableColumn(key: .lastRun, label: "Last run", width: LibraryLayout.lastRun, alignment: .trailing),
            SortableColumn(key: nil, label: "", width: LibraryLayout.launch, alignment: .trailing),
        ]
    }

    /// `.autoflow` sorts `Bool?` as `1`/`0`/nil (never-declared last, per
    /// `sortRows`'s null-discipline contract) — there is no third numeric
    /// state, so `true` outranks `false` ascending.
    // Not `private` — see `agentSortValue`'s identical comment above.
    static func workflowSortValue(_ row: WorkflowDefinition, _ key: WorkflowsSortKey) -> ListSortValue {
        switch key {
        case .name: .text(row.name)
        case .scope: .text(row.scope)
        case .autoflow: .number(row.autoflowEnabled.map { $0 ? 1 : 0 })
        case .runs: .number(Double(row.runCount))
        case .lastRun: .date(ActivityRow.parseISO(row.lastRun))
        }
    }

    // MARK: - Autoflows tab

    @ViewBuilder
    private func autoflowsSection(store: LibraryStore) -> some View {
        switch store.autoflows {
        case .loading:
            loadingBlock
        case .failed(let message):
            FailedBlock(subject: "autoflows", message: message, retry: { await store.loadAutoflows() })
        case .empty:
            emptyBlock("No autoflow definitions")
        case .content(let rows):
            autoflowsTable(rows: rows, store: store)
        }
    }

    private func autoflowsTable(rows: [AutoflowDefinition], store: LibraryStore) -> some View {
        let sorted = sortRows(rows, sort: autoflowsSort, value: Self.autoflowSortValue)
        return VStack(alignment: .leading, spacing: 0) {
            SortableHeaderRow(columns: autoflowColumns, sort: $autoflowsSort)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
            Divider()
            VStack(spacing: 0) {
                // See `agentsTable`'s identical `id: \.rowIdentity` comment
                // above — same fix, same rationale.
                ForEach(sorted, id: \.rowIdentity) { def in
                    AutoflowDefRow(
                        def: def,
                        pendingActions: store.pendingActions,
                        onSelect: {
                            model.navigate(to: .workflowDefinition(name: def.name, scopeKind: def.scopeKind, scopeID: def.scopeID))
                        },
                        onLaunch: {
                            model.presentLauncher(kind: .workflow, name: def.name, scopeKind: def.scopeKind, scopeID: def.scopeID)
                        },
                        onToggle: { newValue in
                            Task {
                                await store.setAutoflowEnabled(
                                    name: def.name, scopeKind: def.scopeKind, scopeID: def.scopeID, enabled: newValue
                                )
                            }
                        }
                    )
                    Divider()
                }
            }
        }
        .panelStyle(.panel)
    }

    private var autoflowColumns: [SortableColumn<AutoflowsSortKey>] {
        [
            SortableColumn(key: .name, label: "Name", width: nil),
            // Text columns, trailing-aligned for metadata visual consistency
            // (review fix I-3) — see `agentColumns`' identical override.
            SortableColumn(key: .trigger, label: "Trigger", width: LibraryLayout.trigger, alignment: .trailing, firstTapAscending: true),
            SortableColumn(key: .scope, label: "Scope", width: LibraryLayout.scope, alignment: .trailing, firstTapAscending: true),
            SortableColumn(key: .enabled, label: "Enabled", width: LibraryLayout.enabled, alignment: .trailing),
            SortableColumn(key: nil, label: "", width: LibraryLayout.launch, alignment: .trailing),
        ]
    }

    // Not `private` — see `agentSortValue`'s identical comment above.
    static func autoflowSortValue(_ row: AutoflowDefinition, _ key: AutoflowsSortKey) -> ListSortValue {
        switch key {
        case .name: .text(row.name)
        case .trigger: .text(row.trigger)
        case .scope: .text(row.scope)
        case .enabled: .number(row.enabled ? 1 : 0)
        }
    }

    // MARK: - Shared blocks

    private var loadingBlock: some View {
        HStack {
            Spacer(minLength: 0)
            ProgressView().controlSize(.small)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, minHeight: 120)
        .panelStyle(.panel)
    }

    private func emptyBlock(_ label: String) -> some View {
        HStack {
            Spacer(minLength: 0)
            Text(label).font(.noteText).foregroundStyle(Color.rupuMute)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, minHeight: 120)
        .panelStyle(.panel)
    }

    private func centeredLabel(_ label: String) -> some View {
        VStack {
            Spacer(minLength: 0)
            Text(label).font(.noteText).foregroundStyle(Color.rupuMute)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

// MARK: - Shared row chrome

/// A row's trailing "Launch" text button — shared shape across all three
/// tabs' rows. Plain `Button` (not the row's own tap target) so it composes
/// with each row's `.onTapGesture` row-navigation exactly the way
/// `ActivityTable`'s inline gate actions coexist with its own row tap (see
/// that type's `awaitingActions`).
struct LaunchButton: View {
    let action: () -> Void

    var body: some View {
        Button("Launch", action: action)
            .buttonStyle(.plain)
            .font(.metaText.weight(.semibold))
            .foregroundStyle(Color.rupuBrand)
    }
}

/// Permission-tone badge — `agentPermissionTone(mode:)`'s pure mapping,
/// rendered. `nil` (no frontmatter override — see that function's doc
/// comment) renders nothing at all rather than a fabricated neutral badge.
struct PermissionBadge: View {
    let mode: String?

    var body: some View {
        if let mode, let tone = agentPermissionTone(mode: mode) {
            Badge(mode, tone: Color.status(tone))
        }
    }
}

private func rowTapModifiers<V: View>(_ view: V, onSelect: @escaping () -> Void) -> some View {
    view
        .contentShape(Rectangle())
        .onTapGesture(perform: onSelect)
        .onHover { hovering in
            if hovering {
                NSCursor.pointingHand.push()
            } else {
                NSCursor.pop()
            }
        }
}

// MARK: - Agent row

private struct AgentDefRow: View {
    let def: AgentDefinition
    let onSelect: () -> Void
    let onLaunch: () -> Void

    var body: some View {
        rowTapModifiers(
            HStack(spacing: 0) {
                HStack(spacing: 6) {
                    Text(def.name)
                        .foregroundStyle(Color.rupuInk)
                        .lineLimit(1)
                        .truncationMode(.tail)
                    PermissionBadge(mode: def.mode)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.trailing, 8)

                Text(def.scope)
                    .font(.dataMono(10))
                    .foregroundStyle(Color.rupuMute)
                    .lineLimit(1)
                    .frame(width: LibraryLayout.scope, alignment: .trailing)
                    .padding(.trailing, 8)

                Text(def.model ?? "—")
                    .font(.dataMono(10))
                    .foregroundStyle(Color.rupuDim)
                    .lineLimit(1)
                    .frame(width: LibraryLayout.model, alignment: .trailing)
                    .padding(.trailing, 8)

                Text(Fmt.count(def.runCount))
                    .font(.dataMono(11))
                    .foregroundStyle(Color.rupuInk)
                    .frame(width: LibraryLayout.runs, alignment: .trailing)
                    .padding(.trailing, 8)

                Text(relativeLabel(def.lastRun))
                    .font(.dataMono(11))
                    .foregroundStyle(Color.rupuDim)
                    .frame(width: LibraryLayout.lastRun, alignment: .trailing)
                    .padding(.trailing, 8)

                LaunchButton(action: onLaunch)
                    .frame(width: LibraryLayout.launch, alignment: .trailing)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8),
            onSelect: onSelect
        )
    }
}

// MARK: - Workflow row

private struct WorkflowDefRow: View {
    let def: WorkflowDefinition
    let onSelect: () -> Void
    let onLaunch: () -> Void

    var body: some View {
        rowTapModifiers(
            HStack(spacing: 0) {
                Text(def.name)
                    .foregroundStyle(Color.rupuInk)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.trailing, 8)

                Text(def.scope)
                    .font(.dataMono(10))
                    .foregroundStyle(Color.rupuMute)
                    .lineLimit(1)
                    .frame(width: LibraryLayout.scope, alignment: .trailing)
                    .padding(.trailing, 8)

                autoflowCell
                    .frame(width: LibraryLayout.enabled, alignment: .trailing)
                    .padding(.trailing, 8)

                Text(Fmt.count(def.runCount))
                    .font(.dataMono(11))
                    .foregroundStyle(Color.rupuInk)
                    .frame(width: LibraryLayout.runs, alignment: .trailing)
                    .padding(.trailing, 8)

                Text(relativeLabel(def.lastRun))
                    .font(.dataMono(11))
                    .foregroundStyle(Color.rupuDim)
                    .frame(width: LibraryLayout.lastRun, alignment: .trailing)
                    .padding(.trailing, 8)

                LaunchButton(action: onLaunch)
                    .frame(width: LibraryLayout.launch, alignment: .trailing)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8),
            onSelect: onSelect
        )
    }

    @ViewBuilder
    private var autoflowCell: some View {
        switch def.autoflowEnabled {
        case .some(true):
            Badge("enabled", tone: Color.status(.running))
        case .some(false):
            Badge("disabled", tone: Color.rupuMute)
        case .none:
            Text("—").font(.dataMono(10)).foregroundStyle(Color.rupuMute)
        }
    }
}

// MARK: - Autoflow row

private struct AutoflowDefRow: View {
    let def: AutoflowDefinition
    let pendingActions: PendingActions
    let onSelect: () -> Void
    let onLaunch: () -> Void
    let onToggle: (Bool) -> Void

    /// Composite key (review fix, round 1) — a bare `ActionKey(def.name,
    /// .setEnabled)` would show a differently-scoped, same-named autoflow's
    /// pending/failure state on this row too. See `ActionKey.autoflow(...)`'s
    /// doc comment.
    private var key: ActionKey { ActionKey.autoflow(name: def.name, scopeKind: def.scopeKind, scopeID: def.scopeID, verb: .setEnabled) }

    private var isPending: Bool {
        if case .pending = pendingActions.state(key) { return true }
        return false
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            rowTapModifiers(
                HStack(spacing: 0) {
                    Text(def.name)
                        .foregroundStyle(Color.rupuInk)
                        .lineLimit(1)
                        .truncationMode(.tail)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.trailing, 8)

                    Text(def.trigger)
                        .font(.dataMono(10))
                        .foregroundStyle(Color.rupuDim)
                        .lineLimit(1)
                        .frame(width: LibraryLayout.trigger, alignment: .trailing)
                        .padding(.trailing, 8)

                    Text(def.scope)
                        .font(.dataMono(10))
                        .foregroundStyle(Color.rupuMute)
                        .lineLimit(1)
                        .frame(width: LibraryLayout.scope, alignment: .trailing)
                        .padding(.trailing, 8)

                    toggleCell
                        .frame(width: LibraryLayout.enabled, alignment: .trailing)
                        .padding(.trailing, 8)

                    LaunchButton(action: onLaunch)
                        .frame(width: LibraryLayout.launch, alignment: .trailing)
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 8),
                onSelect: onSelect
            )
            if case .failed(let message) = pendingActions.state(key) {
                failureNote(message)
            }
        }
    }

    /// A plain `Toggle` bound to a get/set pair — the `get:` reads the LIVE
    /// `def.enabled` the store's own `applyAutoflowEnabled(...)` patches in
    /// place once the mutation confirms (see `LibraryStore.
    /// setAutoflowEnabled`'s doc comment), so this is NOT optimistic: while
    /// `isPending`, the switch stays at its old position (disabled, so it
    /// can't be re-tapped mid-flight) and only actually moves once the
    /// server's response has been applied.
    private var toggleCell: some View {
        Toggle(
            "",
            isOn: Binding(get: { def.enabled }, set: { onToggle($0) })
        )
        .labelsHidden()
        .toggleStyle(.switch)
        .controlSize(.mini)
        .disabled(isPending)
        .opacity(isPending ? 0.6 : 1)
    }

    /// Same message+Retry convention `HostCard.failureNote` (`RupuFleet`)
    /// establishes for a failed mutation.
    private func failureNote(_ message: String) -> some View {
        HStack(alignment: .top, spacing: 6) {
            Text("Toggle failed: \(message)")
                .font(.metaText)
                .foregroundStyle(Color.rupuErr)
                .lineLimit(2)
                .frame(maxWidth: .infinity, alignment: .leading)
            Button("Retry") { onToggle(!def.enabled) }
                .buttonStyle(.plain)
                .font(.metaText.weight(.semibold))
                .foregroundStyle(Color.rupuBrand)
        }
        .padding(.horizontal, 12)
    }
}

// MARK: - Shared formatting

/// `@MainActor` (matching this file's other formatter instances, e.g.
/// `WorkerListRow.relativeFormatter` in `RupuFleet/FleetScreen.swift`, which
/// gets it implicitly from living inside a `View`-conforming type) — a
/// top-level global needs it explicitly: `RelativeDateTimeFormatter` isn't
/// `Sendable`, and every call site here is already MainActor (called from
/// row `View` bodies).
@MainActor
private let relativeFormatter: RelativeDateTimeFormatter = {
    let formatter = RelativeDateTimeFormatter()
    formatter.unitsStyle = .abbreviated
    return formatter
}()

@MainActor
private func relativeLabel(_ iso: String?) -> String {
    guard let iso, let date = ActivityRow.parseISO(iso) else { return "—" }
    return relativeFormatter.localizedString(for: date, relativeTo: Date())
}
