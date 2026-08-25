import SwiftUI
import RupuAPI
import RupuStore
import RupuDesign

/// Definition list for the Launcher's `kind` — `agents` for
/// `.agentRun`/`.session`, `workflows` for `.workflow`. Each row shows the
/// definition's name plus `Badge` tags (an agent's `model`/`scope`, a
/// workflow's `scope` — workflows have no `model` field). Tapping a row
/// calls `store.selectDefinition(_:scopeKind:scopeID:)`, passing the tapped
/// row's own `scopeKind`/`scopeID` (Phase 5A Task 4) so `launch()` can pin
/// the launch to the exact definition this row displays — not a
/// name-based re-lookup, which would be ambiguous for a name that exists in
/// two scopes at once (see `agentList`'s indexed-ID comment below). For a
/// workflow this also fetches its declared inputs in the background (see
/// `LauncherStore`'s doc comment) — this view never awaits that itself, it
/// just fires the `Task` and lets `store.inputsLoadError`/`workflowInputs`
/// update the form once it lands.
///
/// Chrome-only touch (flows-composition Task 6): the row selection
/// highlight's corner radius was 5 — off the v2 scale (panel 7 / inner
/// card 6) — now 6, matching every other nested-row radius in this module.
struct DefinitionPicker: View {
    @Bindable var store: LauncherStore

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Eyebrow(store.kind == .workflow ? "Workflow" : "Agent")

            switch store.kind {
            case .agentRun, .session:
                agentList
            case .workflow:
                workflowList
            }
        }
    }

    @ViewBuilder
    private var agentList: some View {
        switch store.agents {
        case .loading:
            loadingRow
        case .failed(let message):
            failedRow(message)
        case .empty:
            emptyRow("No agents defined")
        case .content(let agents):
            // Indexed IDs, not `id: \.name` — definition names are only
            // unique per scope, and the flat list can carry the same name
            // twice (e.g. a project-scoped and a global agent both named
            // "reviewer"). Duplicate ForEach IDs are undefined behavior in
            // SwiftUI; an index is unique by construction. Selection is
            // still name-keyed (the store's `selectedDefinition` contract),
            // so both same-named rows highlight together — an honest
            // rendering of that ambiguity rather than a view-layer guess.
            listShell {
                ForEach(Array(agents.enumerated()), id: \.offset) { _, agent in
                    row(
                        name: agent.name,
                        tags: [agent.model, agent.scope].compactMap { $0 },
                        isSelected: store.selectedDefinition == agent.name,
                        scopeKind: agent.scopeKind,
                        scopeID: agent.scopeID
                    )
                }
            }
        }
    }

    @ViewBuilder
    private var workflowList: some View {
        switch store.workflows {
        case .loading:
            loadingRow
        case .failed(let message):
            failedRow(message)
        case .empty:
            emptyRow("No workflows defined")
        case .content(let workflows):
            // Indexed IDs for the same duplicate-name reason as `agentList`
            // above (observed live: `dispatch-demo` exists in two scopes).
            listShell {
                ForEach(Array(workflows.enumerated()), id: \.offset) { _, workflow in
                    row(
                        name: workflow.name,
                        tags: [workflow.scope],
                        isSelected: store.selectedDefinition == workflow.name,
                        scopeKind: workflow.scopeKind,
                        scopeID: workflow.scopeID
                    )
                }
            }
        }
    }

    private func listShell<Content: View>(@ViewBuilder content: () -> Content) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 2) {
                content()
            }
        }
        .frame(maxHeight: 140)
        .panelStyle(.innerCard)
    }

    private func row(name: String, tags: [String], isSelected: Bool, scopeKind: String, scopeID: String?) -> some View {
        Button {
            Task { await store.selectDefinition(name, scopeKind: scopeKind, scopeID: scopeID) }
        } label: {
            HStack(spacing: 8) {
                Text(name)
                    .font(.leadText)
                    .foregroundStyle(Color.rupuInk)
                Spacer(minLength: 8)
                HStack(spacing: 6) {
                    ForEach(tags, id: \.self) { tag in
                        Badge(tag)
                    }
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .frame(maxWidth: .infinity)
            .background(isSelected ? Color.rupuBrand.opacity(0.12) : Color.clear)
            .clipShape(RoundedRectangle(cornerRadius: 6))
        }
        .buttonStyle(.plain)
    }

    private var loadingRow: some View {
        listShell {
            HStack {
                Spacer(minLength: 0)
                ProgressView().controlSize(.small)
                Spacer(minLength: 0)
            }
            .padding(.vertical, 20)
        }
    }

    private func failedRow(_ message: String) -> some View {
        listShell {
            Text("Failed to load")
                .font(.noteText)
                .foregroundStyle(Color.status(.failed))
                .padding(10)
            Text(message)
                .font(.noteText)
                .foregroundStyle(Color.rupuDim)
                .padding(.horizontal, 10)
        }
    }

    private func emptyRow(_ label: String) -> some View {
        listShell {
            Text(label)
                .font(.noteText)
                .foregroundStyle(Color.rupuMute)
                .padding(10)
        }
    }
}
