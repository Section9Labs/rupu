import SwiftUI
import RupuStore
import RupuBackend
import RupuDesign

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
                .toolbar { ShellToolbar(model: model) }
        }
        .background(Color.rupuBg)
        .task {
            await backend.reconnectIfNeeded()
        }
        .sheet(isPresented: onboardingSheetBinding) {
            OnboardingView(backend: backend, model: model)
        }
        .onChange(of: backend.health) { _, newHealth in
            handleHealthChange(newHealth)
        }
    }

    /// `true` while onboarding isn't complete; writing `false` back (a
    /// user-driven sheet dismissal) marks it complete too, so there's no
    /// path that leaves the sheet closed but the flag unset.
    private var onboardingSheetBinding: Binding<Bool> {
        Binding(
            get: { !model.onboardingComplete },
            set: { isPresented in
                if !isPresented { model.onboardingComplete = true }
            }
        )
    }

    /// Mirrors backend health into the model the existing sidebar
    /// footer/toolbar pill already read (Task 8), and — the Phase 1
    /// end-to-end proof — starts exactly one task consuming the live event
    /// stream the first time health reaches `.healthy`. Health flapping
    /// back down is a backstop that also flips `liveConnected` false (the
    /// stream client itself keeps reconnecting, and normally
    /// `backend.onLiveConnectionChange` — set in `init`, connection-level,
    /// not frame-level — already caught the disconnect); it does not tear
    /// down or respawn the consumer, per the brief's "don't spawn a second
    /// consumer on re-health" — `liveEventTask == nil` is the guard.
    /// `liveConnected` itself is owned by `onLiveConnectionChange`; this
    /// loop only counts frames.
    private func handleHealthChange(_ health: BackendHealth) {
        model.backendHealth = health

        guard case .healthy = health else {
            model.liveConnected = false
            return
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
            PlaceholderScreen(title: model.route.screenTitle, phase: model.route.placeholderPhase)
        case .activity:
            PlaceholderScreen(title: model.route.screenTitle, phase: model.route.placeholderPhase)
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
