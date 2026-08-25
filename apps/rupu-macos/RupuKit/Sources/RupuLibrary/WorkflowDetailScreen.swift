import SwiftUI
import RupuAPI
import RupuStore
import RupuDesign

/// The Workflow detail screen (Phase 5A, Task 7), pushed from a Library
/// workflows-tab OR autoflows-tab row tap (`.workflowDefinition(name:)` —
/// there is no separate autoflow detail route, an autoflow definition IS a
/// workflow definition): back chevron + breadcrumb, declared inputs, the
/// YAML source in a mono scroll block, the autoflow enable/disable toggle
/// (when this workflow carries an `autoflow:` block), and a page Launch
/// button.
///
/// **Two independent data sources, fetched concurrently**:
/// 1. `client.workflowDetail(name:)` (`GET /api/workflows/:name`) —
///    `inputs`/`yaml`. No scope fields on the wire (confirmed reading
///    `load_detail` in `crates/rupu-cp/src/api/workflows.rs`: the response
///    has `scope`/`scope_kind`/`scope_id` alongside `yaml`, `WorkflowDetail`
///    just doesn't decode them — not needed for inputs/yaml rendering).
/// 2. A `LibraryStore` instance this screen owns and lazily builds — same
///    lifecycle recipe every other screen's store gets — used ONLY for its
///    `workflows` list (via `loadWorkflows()`, not a full `activate()`) to
///    find THIS workflow's own row (`scope`/`scopeKind`/`scopeID`/
///    `autoflowEnabled` — none of which `workflowDetail(name:)` carries),
///    and for its already-tested `setAutoflowEnabled(...)` mutation when the
///    toggle fires. Reusing `LibraryStore` here (rather than inventing a
///    second, narrower fetch) means the toggle shares the exact same
///    confirm-on-response/`PendingActions` logic the Library list screen's
///    own row toggle uses, with no duplicated mutation code — the tradeoff
///    is this screen fetches the full workflow list just to find one row in
///    it, accepted as the smallest honest seam over adding scope fields to
///    `WorkflowDetail`'s decode for a single call site.
///
/// **Does NOT need `OverviewScreen`'s cold-launch fix** — same reasoning
/// `ProjectDetailScreen`/`AgentDetailScreen` document: only ever reached by
/// pushing from `.library`, never a cold-launch route.
public struct WorkflowDetailScreen: View {
    @Bindable var model: AppModel
    let backend: BackendController
    let name: String

    @State private var detail: BlockState<WorkflowDetail> = .loading
    @State private var store: LibraryStore?
    @State private var storeClientID: ObjectIdentifier?

    public init(model: AppModel, backend: BackendController, name: String) {
        self.model = model
        self.backend = backend
        self.name = name
    }

    public var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                header
                switch detail {
                case .loading:
                    ProgressView().controlSize(.small)
                case .failed(let message):
                    FailedNote(message: message)
                case .empty:
                    Text("Not found").font(.noteText).foregroundStyle(Color.rupuMute)
                case .content(let value):
                    content(value)
                }
            }
            .padding(16)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(Color.rupuBg)
        // Same "keyed on `name` alone, no client-swap guard" rationale
        // `AgentDetailScreen.body`'s doc comment gives — `activateStore()`
        // below has its own independent `storeClientID` guard for the
        // `LibraryStore` it owns.
        .task(id: name) {
            async let detailLoad: Void = loadDetail()
            async let storeLoad: Void = activateStore()
            _ = await (detailLoad, storeLoad)
        }
    }

    private func loadDetail() async {
        guard let client = backend.client() else { return }
        detail = .loading
        do {
            detail = .content(try await client.workflowDetail(name: name))
        } catch {
            guard !isCancellation(error) else { return }
            detail = .failed(String(describing: error))
        }
    }

    private func activateStore() async {
        guard let client = backend.client() else { return }
        let clientID = backend.clientIdentity()
        if store == nil || storeClientID != clientID {
            store = LibraryStore(client: client, pendingActions: backend.pendingActions)
            storeClientID = clientID
        }
        guard let store else { return }
        await store.loadWorkflows()
    }

    /// This workflow's own row from `store.workflows`, once loaded — `nil`
    /// while still loading/failed/empty, or if this name genuinely isn't in
    /// the list (a race with a concurrent delete, or a name typo reaching
    /// this route directly). Every caller below already tolerates `nil`
    /// (scope-dependent chrome simply doesn't render), never crashes on it.
    private var definitionRow: WorkflowDefinition? {
        guard case .content(let rows) = store?.workflows else { return nil }
        return rows.first(where: { $0.name == name })
    }

    // MARK: - Header

    private var header: some View {
        HStack(spacing: 10) {
            Button {
                model.navigateBack()
            } label: {
                Icon(.arrowLeft)
                    .foregroundStyle(Color.rupuDim)
            }
            .buttonStyle(.plain)

            Text("Library ▸ \(name)")
                .font(.leadText)
                .foregroundStyle(Color.rupuInk)
            Spacer(minLength: 0)
        }
    }

    // MARK: - Content

    private func content(_ detail: WorkflowDetail) -> some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 10) {
                metaChips
                Spacer(minLength: 0)
                Button("Launch") {
                    model.presentLauncher(
                        kind: .workflow, name: detail.name,
                        scopeKind: definitionRow?.scopeKind, scopeID: definitionRow?.scopeID
                    )
                }
                .buttonStyle(RupuButtonStyle.primary)
            }

            if let def = definitionRow, def.autoflowEnabled != nil {
                autoflowToggleRow(def)
            }

            inputsSection(detail)
            yamlBlock(detail.yaml)
        }
    }

    @ViewBuilder
    private var metaChips: some View {
        if let def = definitionRow {
            Badge(def.scope)
        }
    }

    // MARK: - Autoflow toggle

    private func autoflowToggleRow(_ def: WorkflowDefinition) -> some View {
        AutoflowToggleRow(
            def: def,
            pendingActions: store?.pendingActions ?? PendingActions(),
            onToggle: { newValue in
                Task {
                    await store?.setAutoflowEnabled(name: def.name, scopeKind: def.scopeKind, scopeID: def.scopeID, enabled: newValue)
                }
            }
        )
    }

    // MARK: - Inputs

    @ViewBuilder
    private func inputsSection(_ detail: WorkflowDetail) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Eyebrow("Inputs")
            if detail.inputs.isEmpty {
                Text("No declared inputs").font(.noteText).foregroundStyle(Color.rupuMute)
            } else {
                VStack(spacing: 0) {
                    ForEach(detail.inputs.keys.sorted(), id: \.self) { inputName in
                        if let input = detail.inputs[inputName] {
                            inputRow(name: inputName, input: input)
                            Divider()
                        }
                    }
                }
                .panelStyle(.innerCard)
            }
        }
    }

    private func inputRow(name: String, input: WorkflowInputDef) -> some View {
        HStack(spacing: 8) {
            Text(name)
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuInk)
            Badge(input.type)
            if input.required {
                Badge("required", tone: Color.status(.awaiting))
            }
            if !input.allowedValues.isEmpty {
                Text(input.allowedValues.joined(separator: " | "))
                    .font(.metaText)
                    .foregroundStyle(Color.rupuMute)
                    .lineLimit(1)
            }
            Spacer(minLength: 8)
            Text(input.default ?? "—")
                .font(.dataMono(10))
                .foregroundStyle(Color.rupuDim)
                .lineLimit(1)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
    }

    // MARK: - YAML

    /// Same "mono block" idiom `AgentDetailScreen.rawBlock`/`TranscriptFeed.
    /// jsonBlock` (`RupuRunDetail`) establish — re-derived locally per that
    /// idiom's own precedent (each is private to its own module).
    private func yamlBlock(_ yaml: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Eyebrow("YAML")
            Text(yaml)
                .font(.dataMono(11.5))
                .foregroundStyle(Color.rupuInk)
                .textSelection(.enabled)
                .padding(8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.rupuSurface)
                .clipShape(RoundedRectangle(cornerRadius: 4))
        }
    }
}

/// The detail page's autoflow toggle — same get/set/pending contract
/// `RupuLibrary.LibraryScreen`'s `AutoflowDefRow.toggleCell` establishes
/// (not shared — that type is private to `LibraryScreen.swift`), reading the
/// LIVE `def.enabled`-equivalent (`def.autoflowEnabled == true`) so it stays
/// non-optimistic: only moves once `LibraryStore.setAutoflowEnabled`'s
/// response has been applied.
private struct AutoflowToggleRow: View {
    let def: WorkflowDefinition
    let pendingActions: PendingActions
    let onToggle: (Bool) -> Void

    private var key: ActionKey { ActionKey(def.name, .setEnabled) }

    private var isPending: Bool {
        if case .pending = pendingActions.state(key) { return true }
        return false
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 8) {
                Eyebrow("Autoflow")
                Toggle("", isOn: Binding(get: { def.autoflowEnabled ?? false }, set: { onToggle($0) }))
                    .labelsHidden()
                    .toggleStyle(.switch)
                    .controlSize(.mini)
                    .disabled(isPending)
                    .opacity(isPending ? 0.6 : 1)
                Text(def.autoflowEnabled == true ? "Enabled" : "Disabled")
                    .font(.noteText)
                    .foregroundStyle(Color.rupuDim)
            }
            if case .failed(let message) = pendingActions.state(key) {
                Text("Toggle failed: \(message)")
                    .font(.metaText)
                    .foregroundStyle(Color.rupuErr)
            }
        }
    }
}

private struct FailedNote: View {
    let message: String

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("Failed to load")
                .font(.noteText)
                .foregroundStyle(Color.status(.failed))
            Text(message)
                .font(.noteText)
                .foregroundStyle(Color.rupuDim)
                .lineLimit(3)
        }
    }
}
