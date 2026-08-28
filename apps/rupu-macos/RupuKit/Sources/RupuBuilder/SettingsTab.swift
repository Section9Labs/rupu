import SwiftUI
import RupuAPI
import RupuStore
import RupuDesign
import RupuFlowKit

// The inspector rail's Settings tab (macOS Workflow Builder, Task 13):
// workflow-level NAME/DESCRIPTION editors, a read-only TRIGGER card, a
// read-only INPUTS card, and (when this workflow carries an `autoflow:`
// block) the enable/disable toggle ported verbatim from the deleted
// `RupuLibrary.WorkflowDetailScreen.autoflowToggleRow` — same
// `LibraryStore.setAutoflowEnabled` call, same `PendingActions` contract.

// MARK: - Pure helpers (meta.rest["trigger"]/["inputs"] readers)

/// `meta.rest["trigger"]`'s trigger kind — `manual` when absent, or when
/// present but the `on:` key is missing/unrecognized (mirrors workflow.rs's
/// `#[serde(default)]` on `Trigger.on`, which defaults to `Manual`).
func settingsTriggerKind(_ trigger: YAMLValue?) -> TriggerKind {
    guard let raw = trigger?["on"]?.stringValue else { return .manual }
    return TriggerKind(rawValue: raw) ?? .manual
}

/// `meta.rest["trigger"]`'s `cron:` expression, if any — only meaningful
/// when `settingsTriggerKind(_:) == .cron`, but reads whatever's there
/// regardless (a malformed document might carry both).
func settingsTriggerCron(_ trigger: YAMLValue?) -> String? {
    trigger?["cron"]?.stringValue
}

/// `meta.rest["inputs"]`'s declared inputs, in source order: name + type
/// (`"string"` default, mirrors `InputDef::default_type`) + required flag.
func settingsInputRows(_ inputs: YAMLValue?) -> [(name: String, type: String, required: Bool)] {
    guard let entries = inputs?.mappingValue else { return [] }
    return entries.map { entry in
        (
            name: entry.key,
            type: entry.value["type"]?.stringValue ?? "string",
            required: entry.value["required"]?.boolValue ?? false
        )
    }
}

// MARK: - SettingsTab

struct SettingsTab: View {
    @Bindable var store: BuilderStore
    let backend: BackendController
    let scopeKind: String?
    let scopeID: String?

    @State private var nameText: String
    @State private var libraryStore: LibraryStore?
    @State private var libraryStoreClientID: ObjectIdentifier?

    init(store: BuilderStore, backend: BackendController, scopeKind: String?, scopeID: String?) {
        self.store = store
        self.backend = backend
        self.scopeKind = scopeKind
        self.scopeID = scopeID
        _nameText = State(initialValue: store.graph.meta.name)
    }

    /// This workflow's own row from `libraryStore.workflows`, matched on
    /// name + scope (not name alone) — same rationale `WorkflowDetailScreen.
    /// definitionRow`'s retired doc comment gave: a bare name match would
    /// silently pick whichever same-named row comes first when the same
    /// workflow name exists at two different scopes.
    private var definitionRow: WorkflowDefinition? {
        guard case .content(let rows) = libraryStore?.workflows else { return nil }
        guard let scopeKind else { return rows.first(where: { $0.name == store.graph.meta.name }) }
        return rows.first(where: { $0.name == store.graph.meta.name && $0.scopeKind == scopeKind && $0.scopeID == scopeID })
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                if let definitionRow {
                    Badge(definitionRow.scope)
                }
                nameField
                descriptionField
                triggerCard
                inputsCard
                if let def = definitionRow, def.autoflowEnabled != nil {
                    autoflowCard(def)
                }
            }
            .padding(12)
        }
        .task {
            await activateLibraryStore()
        }
    }

    // MARK: - Name / description

    private var nameField: some View {
        CommitOnBlurField(text: $nameText, font: .uiText) { newValue in
            let trimmed = newValue.trimmingCharacters(in: .whitespaces)
            guard !trimmed.isEmpty, trimmed != store.graph.meta.name else {
                nameText = store.graph.meta.name
                return
            }
            store.updateName(trimmed)
        } label: { Eyebrow("NAME") }
    }

    private var descriptionField: some View {
        DebouncedTextEditor(label: "DESCRIPTION", initial: store.graph.meta.description ?? "", font: .uiText, minHeight: 60) { newValue in
            store.updateDescription(newValue.isEmpty ? nil : newValue)
        }
    }

    // MARK: - Trigger (read-only)

    private var triggerCard: some View {
        let trigger = store.graph.meta.rest.first(where: { $0.key == "trigger" })?.value
        let kind = settingsTriggerKind(trigger)
        return VStack(alignment: .leading, spacing: 6) {
            Eyebrow("TRIGGER")
            VStack(alignment: .leading, spacing: 6) {
                Badge(kind.rawValue, tone: Color.trigger(kind))
                if kind == .cron, let cron = settingsTriggerCron(trigger) {
                    Text(cron)
                        .font(.dataMono(11))
                        .foregroundStyle(Color.rupuInk)
                }
            }
            .padding(8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .panelStyle(.innerCard)
        }
    }

    // MARK: - Inputs (read-only)

    private var inputsCard: some View {
        let inputs = store.graph.meta.rest.first(where: { $0.key == "inputs" })?.value
        let rows = settingsInputRows(inputs)
        return VStack(alignment: .leading, spacing: 6) {
            Eyebrow("INPUTS")
            if rows.isEmpty {
                Text("No declared inputs")
                    .font(.noteText)
                    .foregroundStyle(Color.rupuMute)
            } else {
                VStack(spacing: 0) {
                    ForEach(rows, id: \.name) { row in
                        HStack(spacing: 8) {
                            Text(row.name)
                                .font(.dataMono(11))
                                .foregroundStyle(Color.rupuInk)
                            Badge(row.type)
                            if row.required {
                                Badge("required", tone: Color.status(.awaiting))
                            }
                            Spacer(minLength: 0)
                        }
                        .padding(.horizontal, 8)
                        .padding(.vertical, 6)
                        if row.name != rows.last?.name {
                            Divider()
                        }
                    }
                }
                .panelStyle(.innerCard)
            }
        }
    }

    // MARK: - Autoflow toggle (ported from the deleted WorkflowDetailScreen)

    private func autoflowCard(_ def: WorkflowDefinition) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Eyebrow("AUTOFLOW")
            AutoflowToggleRow(
                def: def,
                pendingActions: libraryStore?.pendingActions ?? PendingActions(),
                onToggle: { newValue in
                    Task {
                        await libraryStore?.setAutoflowEnabled(name: def.name, scopeKind: def.scopeKind, scopeID: def.scopeID, enabled: newValue)
                    }
                }
            )
        }
    }

    private func activateLibraryStore() async {
        guard let client = backend.client() else { return }
        let clientID = backend.clientIdentity()
        if libraryStore == nil || libraryStoreClientID != clientID {
            libraryStore = LibraryStore(client: client, pendingActions: backend.pendingActions)
            libraryStoreClientID = clientID
        }
        guard let libraryStore else { return }
        await libraryStore.loadWorkflows()
    }
}

/// Same get/set/pending contract `RupuLibrary`'s deleted
/// `WorkflowDetailScreen.AutoflowToggleRow` established — reading the LIVE
/// `def.autoflowEnabled`, non-optimistic: only moves once `LibraryStore.
/// setAutoflowEnabled`'s response has been applied.
private struct AutoflowToggleRow: View {
    let def: WorkflowDefinition
    let pendingActions: PendingActions
    let onToggle: (Bool) -> Void

    private var key: ActionKey { ActionKey.autoflow(name: def.name, scopeKind: def.scopeKind, scopeID: def.scopeID, verb: .setEnabled) }

    private var isPending: Bool {
        if case .pending = pendingActions.state(key) { return true }
        return false
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 8) {
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
