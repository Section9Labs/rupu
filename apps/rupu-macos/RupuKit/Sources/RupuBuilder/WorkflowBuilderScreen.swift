import SwiftUI
import RupuAPI
import RupuStore
import RupuDesign
import RupuFlowKit

/// The Workflow Builder screen (macOS design plan, Task 10) — the SHELL:
/// fixed 46pt header (`BuilderHeader`), a left column with the canvas over
/// a collapsible YAML source pane, and a fixed 320pt right inspector rail
/// with a Blocks/Step/Settings tab strip. Canvas node/edge rendering + drag/
/// connect/keyboard interactions landed in Task 11, the Blocks palette in
/// Task 12, the Step form + Settings tab content in Task 13 (`StepFormTab`/
/// `SettingsTab`), and the Run-mode overlay + Launch auto-save land in
/// Task 14 — this task wires the round-trip `BuilderStore` (Tasks 1-9) into
/// a real screen with every fixed-chrome piece the spec's §1 layout calls
/// for, ahead of the pieces that fill it in.
///
/// Replaces `RupuLibrary.WorkflowDetailScreen` at the `.workflowDefinition`
/// route (deleted this task): Design mode's read affordances (YAML source,
/// Launch) are a strict superset of what the old detail screen offered; the
/// autoflow enable/disable toggle it carried moved to this screen's own
/// Settings tab (`SettingsTab.swift`, Task 13).
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

    /// In-flight palette drag-to-canvas state: which kind is being dragged
    /// and the pointer's current point in the screen-wide `"builder"` named
    /// coordinate space (see `.coordinateSpace(name: "builder")` on `body`
    /// below). Lives here, not in `PaletteTab`, because the drop-target
    /// decision needs `canvasFrame` — captured from the CANVAS side of the
    /// screen — which a rail-only view has no way to see.
    @State private var paletteDrag: (kind: RupuFlowKit.StepKind, point: CGPoint)?
    /// The canvas `ZStack`'s own frame in `"builder"` space, kept current by
    /// `.onGeometryChange` in `readyBody` — the drop-target test
    /// `handlePaletteDragEnded` runs against.
    @State private var canvasFrame: CGRect = .zero

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
                    onLaunch: { handleLaunch(store: store) }
                )
                Divider()
            }
            content
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color.rupuBg)
        // Screen-wide named space: the palette card's `DragGesture` and the
        // canvas `ZStack`'s `.onGeometryChange` (both below) both resolve
        // their points/frame against THIS space, so the drop-target test in
        // `handlePaletteDragEnded` compares like-for-like with no manual
        // ancestor-chain coordinate conversion on either side.
        .coordinateSpace(name: "builder")
        .overlay(alignment: .topLeading) {
            if let paletteDrag {
                PaletteDragGhost(kind: paletteDrag.kind)
                    .position(paletteDrag.point)
                    .allowsHitTesting(false)
            }
        }
        // Same "keyed on `name` alone" rationale `WorkflowDetailScreen.body`'s
        // doc comment gave — `activateStore()` below has its own independent
        // `storeClientID` guard for the client-swap case.
        .task(id: name) {
            await activateStore()
        }
        .onChange(of: store?.selectedID) { _, newValue in
            railTab = Self.railTab(afterSelecting: newValue, current: railTab)
        }
        // Task 14: the segmented control writes `store.mode` directly
        // (`BuilderHeader`'s `Picker(selection: $store.mode)`) and
        // `handleLaunch` flips it the same way on a successful save+launch
        // — this ONE observer is what actually starts/stops the followed
        // run for BOTH triggers, so neither has to duplicate the
        // enter/exit call itself. Entering `.run` activates a fresh
        // `RunDetailStore` (or reuses the already-followed one); leaving it
        // deactivates and releases it, mirroring `RunDetailScreen`'s own
        // activate/deactivate contract.
        .onChange(of: store?.mode) { old, new in
            guard let store else { return }
            if new == .run {
                Task { await store.enterRunMode(backend: backend) }
            } else if old == .run {
                store.exitRunMode()
            }
        }
        .onDisappear {
            store?.exitRunMode()
        }
    }

    /// Launch button: save first (`store.save()`'s own `saveError` already
    /// surfaces a failure — see `BuilderStore.save()`'s doc comment), and
    /// only open the Launcher sheet + flip to Run mode on success. A failed
    /// save must never open the sheet against stale, un-persisted YAML.
    private func handleLaunch(store: BuilderStore) {
        Task {
            guard await store.save() else { return }
            model.presentLauncher(kind: .workflow, name: store.graph.meta.name, scopeKind: scopeKind, scopeID: scopeID)
            store.mode = .run
        }
    }

    /// Drag-to-canvas release: the ghost is cleared unconditionally; a drop
    /// point inside `canvasFrame` converts to canvas content coordinates
    /// (`canvasPoint(fromBuilderPoint:canvasFrame:scrollOffset:)` —
    /// `scrollOffset` is always `.zero` here, see that function's doc
    /// comment for why) and adds the node. A drop outside the canvas —
    /// released back over the rail or anywhere else on the screen — is
    /// silently discarded, the same "no-op on a miss" contract `CanvasView.
    /// handlePortDragEnded` already follows for a port-drag release outside
    /// any node.
    private func handlePaletteDragEnded(kind: RupuFlowKit.StepKind, point: CGPoint, store: BuilderStore) {
        paletteDrag = nil
        guard let target = canvasPoint(fromBuilderPoint: point, canvasFrame: canvasFrame, scrollOffset: .zero) else { return }
        store.addNode(kind: kind, at: target)
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
                    // Task 14: Run mode resolved to nothing — no run
                    // launched from here this session, and no prior run
                    // for this workflow either (`enterRunMode`'s full
                    // resolution chain came up empty). The canvas itself
                    // still renders (plain, un-overlaid) underneath.
                    if store.mode == .run, store.followedRunID == nil {
                        noRunBanner
                            .padding(12)
                    }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                // Keeps `canvasFrame` current in the `"builder"` space —
                // the drop-target test `handlePaletteDragEnded` runs
                // against. This captures the visible VIEWPORT frame, not
                // the (possibly larger, scrolled) content size — correct
                // for the "no scroll-offset plumbing" contract documented
                // on `canvasPoint(fromBuilderPoint:canvasFrame:
                // scrollOffset:)`.
                .onGeometryChange(for: CGRect.self, of: { $0.frame(in: .named("builder")) }) { canvasFrame = $0 }
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
                        .font(.uiText)
                        .foregroundStyle(Color.rupuDim)
                }
                .buttonStyle(.plain)
            }
        }
        .frame(maxWidth: 420)
    }

    // MARK: - No-run banner (Task 14)

    /// Run mode's empty state — `enterRunMode(backend:)` found nothing to
    /// follow (no run launched from this screen this session, and no prior
    /// run for this workflow name either).
    private var noRunBanner: some View {
        TintBanner(tone: .rupuDim, toneBg: .rupuSurface) {
            Text("No runs yet — Launch one")
                .font(.noteText)
                .foregroundStyle(Color.rupuInk)
        }
        .frame(maxWidth: 260)
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
            PaletteTab(
                store: store,
                onDragChanged: { kind, point in paletteDrag = (kind, point) },
                onDragEnded: { kind, point in handlePaletteDragEnded(kind: kind, point: point, store: store) }
            )
        case .step:
            StepFormTab(store: store)
        case .settings:
            SettingsTab(store: store, backend: backend, scopeKind: scopeKind, scopeID: scopeID)
        }
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
