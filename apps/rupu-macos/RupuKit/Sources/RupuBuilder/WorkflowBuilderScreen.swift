import SwiftUI
import RupuAPI
import RupuStore
import RupuDesign

/// The Workflow Builder screen (macOS design plan, Task 10) — the SHELL:
/// fixed 46pt header (`BuilderHeader`), a left column with the canvas over
/// a collapsible YAML source pane, and a fixed 320pt right inspector rail
/// with a Blocks/Step/Settings tab strip. Canvas node/edge rendering + drag/
/// connect/keyboard interactions land in Task 11, the Blocks palette in
/// Task 12, the Step form + Settings tab content in Task 13, and the
/// Run-mode overlay + Launch auto-save in Task 14 — this task wires the
/// round-trip `BuilderStore` (Tasks 1-9) into a real screen with every
/// fixed-chrome piece the spec's §1 layout calls for, ahead of the pieces
/// that fill it in.
///
/// Replaces `RupuLibrary.WorkflowDetailScreen` at the `.workflowDefinition`
/// route (deleted this task): Design mode's read affordances (YAML source,
/// Launch) are a strict superset of what the old detail screen offered; the
/// autoflow enable/disable toggle it carried moves to this screen's own
/// Settings tab in Task 13.
public struct WorkflowBuilderScreen: View {
    @Bindable var model: AppModel
    let backend: BackendController
    let name: String
    /// The tapped row's own scope (mirrors `WorkflowDetailScreen`'s
    /// `scopeKind`/`scopeID` — see its retired doc comment for why matching
    /// on these, not `name` alone, matters when the same workflow name
    /// exists at two scopes). Threaded straight through to `BuilderStore`'s
    /// init and to `presentLauncher` — this screen doesn't yet rebuild the
    /// `LibraryStore` row-lookup machinery `WorkflowDetailScreen` used
    /// (that lands with Task 13's Settings tab), so Launch pins the ROUTE's
    /// own scope rather than a freshly-resolved definition row.
    let scopeKind: String?
    let scopeID: String?

    @State private var store: BuilderStore?
    @State private var storeClientID: ObjectIdentifier?

    @AppStorage("rupu.builder.sourceOpen") private var sourceOpen = true
    @AppStorage("rupu.builder.railTab") private var railTab: RailTab = .blocks

    public init(model: AppModel, backend: BackendController, name: String, scopeKind: String? = nil, scopeID: String? = nil) {
        self.model = model
        self.backend = backend
        self.name = name
        self.scopeKind = scopeKind
        self.scopeID = scopeID
    }

    public var body: some View {
        VStack(spacing: 0) {
            if let store {
                BuilderHeader(
                    store: store, name: name, sourceOpen: $sourceOpen,
                    onBack: { model.navigateBack() },
                    onSave: { Task { await store.save() } },
                    onLaunch: {
                        model.presentLauncher(kind: .workflow, name: store.graph.meta.name, scopeKind: scopeKind, scopeID: scopeID)
                    }
                )
                Divider()
            }
            content
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color.rupuBg)
        // Same "keyed on `name` alone" rationale `WorkflowDetailScreen.body`'s
        // doc comment gave — `activateStore()` below has its own independent
        // `storeClientID` guard for the client-swap case.
        .task(id: name) {
            await activateStore()
        }
        .onChange(of: store?.selectedID) { _, newValue in
            railTab = Self.railTab(afterSelecting: newValue, current: railTab)
        }
    }

    private func activateStore() async {
        guard let client = backend.client() else { return }
        let clientID = backend.clientIdentity()
        if store == nil || storeClientID != clientID {
            store = BuilderStore(name: name, scopeKind: scopeKind, scopeID: scopeID, client: client, pendingActions: backend.pendingActions)
            storeClientID = clientID
        }
        guard let store else { return }
        await store.activate()
    }

    /// Pure helper backing "selecting a node flips the rail to Step" (spec
    /// §1): selecting (non-nil) always shows the Step tab; clearing
    /// selection leaves whichever tab was already showing — mirrors the web
    /// editor, which never forces the panel back to Blocks on a plain
    /// deselect (only an explicit tab click does).
    static func railTab(afterSelecting id: String?, current: RailTab) -> RailTab {
        id != nil ? .step : current
    }

    // MARK: - Content

    @ViewBuilder
    private var content: some View {
        if let store {
            switch store.phase {
            case .loading:
                loadingView
            case .failed(let message):
                FailedBlock(subject: "workflow", message: message, retry: { await store.activate() })
                    .padding(16)
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
            case .unsupported(let message):
                // A real, distinct error state (`BuilderStore.activate()`'s
                // doc comment) — the workflow's on-disk YAML uses a feature
                // outside this app's supported subset. Never silently
                // degrades to an empty canvas.
                FailedBlock(subject: "workflow (unsupported YAML)", message: message, retry: { await store.activate() })
                    .padding(16)
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
            case .ready:
                readyBody(store: store)
            }
        } else {
            loadingView
        }
    }

    private var loadingView: some View {
        ProgressView()
            .controlSize(.small)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func readyBody(store: BuilderStore) -> some View {
        HStack(spacing: 0) {
            VStack(spacing: 0) {
                ZStack(alignment: .top) {
                    CanvasView(store: store)
                    if let commitError = store.commitError {
                        commitErrorBanner(commitError, store: store)
                            .padding(12)
                    }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                if sourceOpen {
                    yamlPane(store: store)
                        .frame(height: 192)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)

            Divider()

            railPanel(store: store)
                .frame(width: 320)
        }
    }

    // MARK: - Commit-error banner

    /// A rejected canvas edit (`store.connect`'s self-loop/duplicate/cycle
    /// rejections, or any other mutating verb's rejection) MUST be visible
    /// — `store.commitError` is otherwise a purely transient, easy-to-miss
    /// piece of state. Dismissible so a user who's read it can clear it
    /// without waiting for the next edit to overwrite it.
    private func commitErrorBanner(_ message: String, store: BuilderStore) -> some View {
        TintBanner(tone: .rupuErr, toneBg: .rupuErrBg) {
            HStack(spacing: 10) {
                Icon(.xCircle, size: 14)
                    .foregroundStyle(Color.rupuErr)
                Text(message)
                    .font(.noteText)
                    .foregroundStyle(Color.rupuInk)
                Spacer(minLength: 8)
                Button {
                    store.dismissCommitError()
                } label: {
                    Text("×")
                        .font(.system(size: 15, weight: .medium))
                        .foregroundStyle(Color.rupuDim)
                }
                .buttonStyle(.plain)
            }
        }
        .frame(maxWidth: 420)
    }

    // MARK: - YAML pane

    private func yamlPane(store: BuilderStore) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Eyebrow("WORKFLOW.YAML")
                Text("canonical · round-trips with the canvas")
                    .font(.noteText)
                    .foregroundStyle(Color.rupuMute)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 12)
            .padding(.top, 8)

            ScrollView {
                Text(store.canonicalYAML)
                    .font(.dataMono(11))
                    .foregroundStyle(Color.rupuInk)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 12)
                    .padding(.bottom, 12)
            }
        }
        .frame(maxWidth: .infinity, alignment: .top)
        .background(Color.rupuBg)
        .overlay(alignment: .top) {
            Rectangle().fill(Color.rupuBorder).frame(height: 1)
        }
    }

    // MARK: - Inspector rail

    private func railPanel(store: BuilderStore) -> some View {
        VStack(spacing: 0) {
            railTabStrip
            Divider()
            railContent(store: store)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        }
        .background(Color.rupuPanel)
    }

    private var railTabStrip: some View {
        HStack(spacing: 18) {
            ForEach(RailTab.allCases, id: \.self) { candidate in
                railTabButton(candidate)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12)
        .padding(.top, 10)
    }

    private func railTabButton(_ candidate: RailTab) -> some View {
        let active = railTab == candidate
        return Button {
            railTab = candidate
        } label: {
            VStack(spacing: 6) {
                Text(candidate.title)
                    .font(.uiText)
                    .foregroundStyle(active ? Color.rupuInk : Color.rupuDim)
                Rectangle()
                    .fill(active ? Color.rupuBrand : Color.clear)
                    .frame(height: 2)
            }
        }
        .buttonStyle(.plain)
    }

    @ViewBuilder
    private func railContent(store: BuilderStore) -> some View {
        switch railTab {
        case .blocks:
            placeholderTabContent("Blocks coming in Task 12")
        case .step:
            placeholderTabContent("Step coming in Task 13")
        case .settings:
            placeholderTabContent("Settings coming in Task 13")
        }
    }

    private func placeholderTabContent(_ text: String) -> some View {
        Text(text)
            .font(.noteText)
            .foregroundStyle(Color.rupuMute)
            .padding(16)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// The inspector rail's top tab strip (spec §1: "Blocks | Step | Settings").
/// Persisted via `@AppStorage("rupu.builder.railTab")` — a `String`-raw-value
/// enum gets `@AppStorage`'s `RawRepresentable` overload for free.
enum RailTab: String, CaseIterable, Sendable {
    case blocks, step, settings

    var title: String {
        switch self {
        case .blocks: "Blocks"
        case .step: "Step"
        case .settings: "Settings"
        }
    }
}
