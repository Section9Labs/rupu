import SwiftUI
import RupuStore
import RupuBackend
import RupuDesign
import RupuActivity
import RupuRunDetail
import RupuLauncher

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
            Sidebar(model: model)
                .navigationSplitViewColumnWidth(216)
        } detail: {
            detail
                .toolbar { ShellToolbar(model: model, showLauncher: $showLauncher) }
        }
        // A hidden, zero-visual button rather than the toolbar button
        // itself carrying `.keyboardShortcut` — it keeps ⌘N live even when
        // the toolbar isn't the responder chain's target, the same trick
        // `NavigationSplitView`/menu-command apps use for shortcuts that
        // must work window-wide. `.hidden()` removes it from layout/paint
        // but leaves its action and shortcut registration intact.
        .background(
            Button("New run") { showLauncher = true }
                .keyboardShortcut("n", modifiers: .command)
                .hidden()
        )
        .background(Color.rupuBg)
        .task {
            await backend.reconnectIfNeeded()
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
            PlaceholderScreen(title: model.route.screenTitle, phase: model.route.placeholderPhase)
        case .activity:
            ActivityScreen(model: model, backend: backend)
        case .runDetail(let id, let host):
            RunDetailScreen(model: model, backend: backend, runID: id, host: host)
        case .sessionDetail(let id):
            SessionDetailScreen(model: model, backend: backend, sessionID: id)
        case .agentRunDetail(let id, let transcriptPath, let host):
            AgentRunDetailScreen(model: model, backend: backend, runID: id, transcriptPath: transcriptPath, host: host)
        case .projects:
            PlaceholderScreen(title: model.route.screenTitle, phase: model.route.placeholderPhase)
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
