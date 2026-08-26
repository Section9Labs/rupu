import SwiftUI
import RupuAPI
import RupuStore
import RupuBackend
import RupuDesign
import RupuActivity
import RupuRunDetail
import RupuLauncher
import RupuOverview
import RupuProjects
import RupuFleet
import RupuLibrary
import RupuSecurity
import RupuUsage

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

    /// The single shared `ActivityStore` (perf & interaction arc, Plan 5
    /// Task 2): built once here — not by `ActivityScreen`/`OverviewScreen`
    /// each building their own — and injected into both. Before this,
    /// `OverviewScreen` built a SECOND, independent `ActivityStore` purely
    /// to derive its needs-you queue, each with its own resting SSE
    /// connection replaying history on connect; this collapses that to
    /// one. Built (and rebuilt on a client swap) the same lazy,
    /// client-identity-tracked way `hostsFooter`/`palette` already are —
    /// see `activateOnClientAvailable()`. Whichever screen is currently
    /// visible reconfigures it to its own needs on activation (`kind`/
    /// `scopeFilter` for `ActivityScreen`; `kind: .all` with no scope for
    /// `OverviewScreen`'s needs-you derivation) — safe since the two
    /// screens are mutually exclusive routes, never simultaneously visible.
    @State private var activityStore: ActivityStore?
    /// Same rebuild-on-client-swap rationale as `storeClientID` in
    /// `ActivityScreen`/`OverviewScreen` — tracked independently of
    /// `palette`'s own construction since a future divergence in when each
    /// needs rebuilding shouldn't accidentally couple them.
    @State private var activityStoreClientID: ObjectIdentifier?

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
                // The one screen title: the toolbar's leading slot AND the
                // window title, in sync — `ShellToolbar` carries no title
                // item of its own (see its doc comment).
                .navigationTitle(model.route.screenTitle)
                // `.toolbar(id:)`, not plain `.toolbar {}`: the id'd form
                // is what makes the toolbar user-customizable (right-click
                // → "Customize Toolbar…"). Never add a second plain
                // `.toolbar {}` here — see `ShellToolbar`'s doc comment.
                .toolbar(id: "rupu.shell") { ShellToolbar(model: model, showLauncher: $model.showLauncher, backend: backend, palette: palette) }
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
                Button("New run") { palette?.close(); model.showLauncher = true }
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
        .sheet(isPresented: onboardingSheetBinding) {
            OnboardingView(backend: backend, model: model)
                .interactiveDismissDisabled()
        }
        // No `interactiveDismissDisabled` here, deliberately: the launcher
        // gates its own dismissal (`LauncherSheet` applies
        // `.interactiveDismissDisabled(store.isLaunchInFlight)` internally —
        // Esc/click-outside stays available while idle but is blocked
        // mid-launch, matching its Cancel button). An unconditional value at
        // this call site would fight that store-aware gate, and the store it
        // reads is the sheet's own `@State` — invisible from here.
        .sheet(isPresented: $model.showLauncher) {
            LauncherSheet(model: model, backend: backend)
        }
        .task(id: backend.clientGeneration) {
            activateOnClientAvailable()
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
    /// footer/toolbar pill already read (Task 8) — STATUS DISPLAY only.
    /// Dropping all the way to `.down`/`.incompatible` is a backstop that
    /// also flips `liveConnected` false (the stream client itself keeps
    /// reconnecting, and normally `backend.onLiveConnectionChange` — set in
    /// `init`, connection-level, not frame-level — already caught the
    /// disconnect); `.degraded` (a single failed health poll) deliberately
    /// does NOT force it false, since the SSE stream can still be connected
    /// against a momentarily-flaky health endpoint — forcing it here would
    /// fight `onLiveConnectionChange`'s ownership of that signal.
    ///
    /// **No longer where client-dependent activation lives** (perf &
    /// interaction arc, Plan 5 Task 2): `hostsFooter`/`palette`/
    /// `liveEventTask` used to be built here, gated on `health == .healthy`
    /// — but `backend.client()`/`backend.eventStream()` are both usable the
    /// instant `BackendController` wires them up, well before its
    /// `HealthMonitor`'s own first probe round-trip resolves `health` to
    /// `.healthy`. That extra round trip bought nothing but delay, so
    /// `activateOnClientAvailable()` (driven by `.task(id:
    /// backend.clientGeneration)` in `body`) now owns all of that instead —
    /// see its own doc comment.
    func handleHealthChange(_ health: BackendHealth) {
        model.backendHealth = health

        switch health {
        case .down, .incompatible:
            model.liveConnected = false
        case .starting, .degraded, .healthy:
            break
        }
    }

    /// Activates everything that depends on a live `CPClient`/
    /// `EventStreamClient` pair — `hostsFooter`, `palette`, and the one
    /// live-event-count consumer — directly off `backend.clientGeneration`
    /// (perf & interaction arc, Plan 5 Task 2), collapsing the old
    /// `health -> onChange -> onChange` relay: this now runs the instant a
    /// client becomes available (or is swapped — an embedded/remote
    /// switch, a manual reconnect, a restart), independent of whether
    /// `backend.health` has reached `.healthy` yet. `.task(id:
    /// backend.clientGeneration)` in `body` re-runs this on every
    /// generation bump (including the first, on initial appearance) and is
    /// otherwise a no-op if the generation hasn't changed — so this being
    /// idempotent (every check below is a guarded "build/refresh, don't
    /// duplicate") is what makes calling it unconditionally on every bump
    /// safe, same contract `activate(kind:)`-style methods elsewhere in
    /// this codebase already follow.
    func activateOnClientAvailable() {
        guard let client = backend.client() else { return }
        let clientID = backend.clientIdentity()

        hostsFooter.activate(client: client)
        if palette == nil {
            palette = PaletteStore(client: client, pendingActions: backend.pendingActions, onNavigate: { [model] route in
                model.navigate(to: route)
            })
        }

        // The shared `ActivityStore` (see that property's doc comment) —
        // rebuilt on a client swap exactly like `palette`, just tracked
        // with its own watermark. Not activated here: whichever of
        // `ActivityScreen`/`OverviewScreen` is currently visible calls
        // `activate(kind:)` itself, with its own `kind`/`scopeFilter`.
        if activityStore == nil || activityStoreClientID != clientID {
            activityStore?.deactivate()
            activityStore = ActivityStore(
                client: client,
                signalsFactory: Self.makeActivitySignalsFactory(backend: backend),
                pendingActions: backend.pendingActions
            )
            activityStoreClientID = clientID
        }

        guard liveEventTask == nil, let stream = backend.eventStream() else { return }
        liveEventTask = Task { @MainActor in
            for await _ in stream.events() {
                model.liveEventCount += 1
            }
        }
    }

    /// Builds the shared `ActivityStore`'s required `signalsFactory` around
    /// its OWN fully independent connection — not `RootView`'s shared
    /// `eventStream()` firehose, whose `onConnectionChange` slot is already
    /// claimed (forwarded into `model.liveConnected`). Lifted here from
    /// `ActivityScreen`/`OverviewScreen`, which each used to duplicate an
    /// identical private copy of this same helper to build their own
    /// (separate) `ActivityStore` instances — now there is exactly one
    /// instance, so exactly one copy of this helper.
    private static func makeActivitySignalsFactory(
        backend: BackendController
    ) -> @Sendable () -> AsyncStream<StreamSignal<CPEvent>> {
        {
            let (onChange, continuation, signals) = MainActor.assumeIsolated {
                StreamLifecycle.makeSignalBridge(CPEvent.self)
            }
            guard let stream = MainActor.assumeIsolated({
                backend.makeFirehoseStream(onConnectionChange: onChange)
            }) else {
                continuation.finish()
                return signals
            }

            let pump = Task {
                for await event in stream.events() {
                    continuation.yield(.event(event))
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in pump.cancel() }
            return signals
        }
    }

    @ViewBuilder
    private var detail: some View {
        switch model.route {
        case .overview:
            OverviewScreen(model: model, backend: backend, activityStore: activityStore)
        case .activity:
            ActivityScreen(model: model, backend: backend, activityStore: activityStore)
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
            SecurityScreen(model: model, backend: backend)
        case .coverageDetail(let target, let wsID):
            CoverageDetailScreen(model: model, backend: backend, target: target, wsID: wsID)
        case .library:
            LibraryScreen(model: model, backend: backend)
        case .agentDefinition(let name, let scopeKind, let scopeID):
            AgentDetailScreen(model: model, backend: backend, name: name, scopeKind: scopeKind, scopeID: scopeID)
        case .workflowDefinition(let name, let scopeKind, let scopeID):
            WorkflowDetailScreen(model: model, backend: backend, name: name, scopeKind: scopeKind, scopeID: scopeID)
        case .fleet:
            FleetScreen(model: model, backend: backend)
        case .usage:
            UsageScreen(model: model, backend: backend)
        }
    }
}
