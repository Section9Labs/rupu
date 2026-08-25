import SwiftUI
import RupuStore
import RupuBackend
import RupuDesign
import RupuActivity
import RupuRunDetail
import RupuLauncher
import RupuOverview
import RupuProjects

/// The app's window content: fixed sidebar + detail pane that switches on
/// `model.route`. Phase 2+ swaps each `PlaceholderScreen` branch for a real
/// screen one route at a time; the switch shape is deliberate so those
/// swaps stay one-line diffs.
///
/// Also owns the backend wiring for Phase 1's end-to-end proof: on first
/// appearance it asks `backend` to reconnect a persisted mode (skipping
/// onboarding on relaunch), presents `OnboardingView` as a sheet while
/// `!model.onboardingComplete`, mirrors `backend.health` into
/// `model.backendHealth` (the sidebar footer dot), and — once healthy —
/// starts the single event-stream consumer that drives the toolbar's live
/// pill.
public struct RootView: View {
    @Bindable var model: AppModel
    @Bindable var backend: BackendController

    @State private var liveEventTask: Task<Void, Never>?
    @State private var showLauncher = false
    @State private var hostsFooter = HostsFooterStore()

    /// Flows-composition Task 3: built lazily in `handleHealthChange`, the
    /// first time `backend.client()` exists — `PaletteStore`'s designated
    /// init takes a concrete `CPClient`, unlike `HostsFooterStore`'s
    /// two-phase `activate(client:)`, so this can't be built eagerly in
    /// `init`. `nil` until then; `ShellToolbar`'s search button and the
    /// hidden ⌘K button below both tolerate that (silent no-op), same
    /// "no client yet" degrade every other backend-dependent affordance in
    /// this view already uses.
    @State private var palette: PaletteStore?

    public init(model: AppModel, backend: BackendController) {
        self.model = model
        self.backend = backend
        // Set before any `configureEmbedded`/`configureRemote`/
        // `reconnectIfNeeded` call can run (those all happen later, from
        // `.task`/`OnboardingView`'s buttons), so the very first
        // `EventStreamClient` this controller creates already has it.
        // Connection-level signal, not frame-level: a healthy-but-idle
        // stream (no events, only SSE keep-alives) never decodes a frame,
        // so this is what actually lights the pill up in that case.
        backend.onLiveConnectionChange = { connected in
            Task { @MainActor in
                model.liveConnected = connected
            }
        }
    }

    public var body: some View {
        NavigationSplitView {
            Sidebar(model: model, hostsFooter: hostsFooter)
                .navigationSplitViewColumnWidth(204)
        } detail: {
            detail
                .toolbar { ShellToolbar(model: model, showLauncher: $showLauncher, backend: backend, palette: palette) }
        }
        // Hidden, zero-visual buttons rather than the toolbar controls
        // themselves carrying `.keyboardShortcut` — this keeps ⌘N/⌘K live
        // even when the toolbar isn't the responder chain's target, the
        // same trick `NavigationSplitView`/menu-command apps use for
        // shortcuts that must work window-wide. `.hidden()` removes them
        // from layout/paint but leaves their actions and shortcut
        // registrations intact.
        .background(
            Group {
                // Close the palette first: a launcher sheet can open OVER
                // an open palette, and the palette's own Esc monitor is
                // app-wide while it's up — leaving it open would swallow
                // the sheet's Esc on the buried palette instead.
                Button("New run") { palette?.close(); showLauncher = true }
                    .keyboardShortcut("n", modifiers: .command)
                Button("Command palette") { Task { await palette?.open() } }
                    .keyboardShortcut("k", modifiers: .command)
            }
            .hidden()
        )
        .overlay {
            if let palette, palette.isOpen {
                CommandPaletteView(store: palette)
            }
        }
        .background(Color.rupuBg)
        .task {
            await backend.reconnectIfNeeded()
        }
        .onAppear {
            // Re-activate on reappearance too, not just on a health
            // transition below: a RootView that disappears/reappears
            // without `backend.health` ever changing (still `.healthy`)
            // would otherwise show "— hosts" until the next flap.
            // `activate(client:)` is idempotent, so this is a no-op when
            // already running.
            if let client = backend.client() {
                hostsFooter.activate(client: client)
            }
        }
        .sheet(isPresented: onboardingSheetBinding) {
            OnboardingView(backend: backend, model: model)
                .interactiveDismissDisabled()
        }
        .sheet(isPresented: $showLauncher) {
            LauncherSheet(model: model, backend: backend)
                .interactiveDismissDisabled(false)
        }
        .onChange(of: backend.health) { _, newHealth in
            handleHealthChange(newHealth)
        }
        .onDisappear {
            hostsFooter.deactivate()
        }
    }

    /// `true` while onboarding isn't complete. The setter is intentionally
    /// a no-op: the only path that's allowed to complete onboarding is
    /// `OnboardingView`'s own `.onChange(of: backend.health)`, which sets
    /// `model.onboardingComplete = true` explicitly once the connection is
    /// actually healthy. Esc / Cmd-. are blocked from reaching this sheet
    /// at all by `.interactiveDismissDisabled()` on the sheet's content in
    /// the `.sheet` closure above (it must be applied to the presented
    /// content, not chained after `.sheet` itself — chaining it after
    /// `.sheet` has no effect on the sheet's own dismissal); this setter
    /// being a no-op is the defense in depth — even if some future SwiftUI
    /// dismissal path reaches here, it can never strand the app in
    /// "onboarded" state with nothing actually connected (no reconnect
    /// path, no re-present, no Settings recovery until Phase 6).
    var onboardingSheetBinding: Binding<Bool> {
        Binding(
            get: { !model.onboardingComplete },
            set: { _ in }
        )
    }

    /// Mirrors backend health into the model the existing sidebar
    /// footer/toolbar pill already read (Task 8), and — the Phase 1
    /// end-to-end proof — starts exactly one task consuming the live event
    /// stream the first time health reaches `.healthy`. Dropping all the
    /// way to `.down`/`.incompatible` is a backstop that also flips
    /// `liveConnected` false (the stream client itself keeps reconnecting,
    /// and normally `backend.onLiveConnectionChange` — set in `init`,
    /// connection-level, not frame-level — already caught the disconnect);
    /// `.degraded` (a single failed health poll) deliberately does NOT
    /// force it false, since the SSE stream can still be connected against
    /// a momentarily-flaky health endpoint — forcing it here would fight
    /// `onLiveConnectionChange`'s ownership of that signal. None of this
    /// tears down or respawns the consumer, per the brief's "don't spawn a
    /// second consumer on re-health" — `liveEventTask == nil` is the guard.
    func handleHealthChange(_ health: BackendHealth) {
        model.backendHealth = health

        switch health {
        case .down, .incompatible:
            model.liveConnected = false
        case .starting, .degraded, .healthy:
            break
        }

        guard case .healthy = health else { return }

        // `CPClient` is only available once `backend` has an active
        // connection (`BackendController.client()` is `nil` until then), so
        // the sidebar's `HostsFooterStore` is activated here — the first
        // healthy transition — rather than from an `.onAppear`, mirroring
        // how `ActivityScreen` waits on `backend.client()` before building
        // its own store. `activate(client:)` is idempotent (guards its own
        // poll task), so a later re-healthy transition after a `.degraded`
        // blip just refreshes the client rather than spawning a second loop.
        if let client = backend.client() {
            hostsFooter.activate(client: client)
            if palette == nil {
                palette = PaletteStore(client: client, pendingActions: backend.pendingActions, onNavigate: { [model] route in
                    model.navigate(to: route)
                })
            }
        }

        guard liveEventTask == nil, let stream = backend.eventStream() else { return }
        liveEventTask = Task { @MainActor in
            for await _ in stream.events() {
                model.liveEventCount += 1
            }
        }
    }

    @ViewBuilder
    private var detail: some View {
        switch model.route {
        case .overview:
            OverviewScreen(model: model, backend: backend)
        case .activity:
            ActivityScreen(model: model, backend: backend)
        case .runDetail(let id, let host):
            RunDetailScreen(model: model, backend: backend, runID: id, host: host)
        case .sessionDetail(let id):
            SessionDetailScreen(model: model, backend: backend, sessionID: id)
        case .agentRunDetail(let id, let transcriptPath, let host):
            AgentRunDetailScreen(model: model, backend: backend, runID: id, transcriptPath: transcriptPath, host: host)
        case .projects:
            ProjectsScreen(model: model, backend: backend)
        case .projectDetail(let wsID):
            ProjectDetailScreen(model: model, backend: backend, wsID: wsID)
        case .security:
            PlaceholderScreen(title: model.route.screenTitle, phase: model.route.placeholderPhase)
        case .library:
            PlaceholderScreen(title: model.route.screenTitle, phase: model.route.placeholderPhase)
        case .fleet:
            PlaceholderScreen(title: model.route.screenTitle, phase: model.route.placeholderPhase)
        case .usage:
            PlaceholderScreen(title: model.route.screenTitle, phase: model.route.placeholderPhase)
        }
    }
}
