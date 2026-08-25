import RupuAPI
import RupuDesign
import RupuStore
import SwiftUI

/// Situation Room (Phase 6B, Task 7) — the fullscreen live-wall scene:
/// `PulseStrip` (vitals) top, a newest-first live stream center, a project
/// roster right. Opened via `RupuApp`'s "Enter Situation Room" View-menu
/// command / `Window(id: "situation")` scene, which enters fullscreen on
/// appear (see `RupuApp.swift` — window-chrome/fullscreen is that file's
/// concern, not this View's).
///
/// **Dark always**: `.preferredColorScheme(.dark)` is applied by the SCENE
/// root in `RupuApp.swift`, not here — a deliberate, doc-commented exception
/// to the rest of the app's light/dark/system appearance setting. A
/// fullscreen ambient wall reads as an ops "situation room" display, not a
/// document window — matching the web's own Situation Room, which has no
/// light-mode CSS variant at all.
///
/// **No "load older events" pagination** (unlike `EventStream.tsx`'s
/// `hasMoreOlder`/`loadOlder`/bottom sentinel): the 200-row history backfill
/// plus a 5,000-row live-tail cap (`SituationStore`) already give this
/// ambient wall a substantial window, and archival browsing of older
/// history already has a home — the Activity screen and a run's own
/// Transcript/Events tabs. A second, parallel "load more" affordance on a
/// screen whose whole point is "what's happening right now" was judged out
/// of scope; deliberate, not an oversight.
public struct SituationRoomScreen: View {
    @Bindable var model: AppModel
    @Bindable var backend: BackendController
    /// Fronts the MAIN window without closing this one — the task-7 brief's
    /// own requirement ("deep-links from SR must front the main window ...
    /// WITHOUT closing the SR window"). This View has no direct access to
    /// `AppDelegate.frontMainWindow()` (an `NSApplicationDelegate`/App-target
    /// concern this module can't reach), so `RupuApp.swift` hands the
    /// closure in at construction — same seam `MenuBarView`'s
    /// `openMainWindow` parameter already uses for the identical need.
    let frontMainWindow: () -> Void

    @State private var store: SituationStore?
    @State private var storeClientID: ObjectIdentifier?
    @State private var filter: StreamFilter = .all

    public init(model: AppModel, backend: BackendController, frontMainWindow: @escaping () -> Void) {
        self.model = model
        self.backend = backend
        self.frontMainWindow = frontMainWindow
    }

    public var body: some View {
        Group {
            if let store {
                content(store: store)
            } else {
                notReadyView
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color.rupuBg)
        .task {
            await activate()
        }
        .onChange(of: backend.health) { _, newHealth in
            guard case .healthy = newHealth else { return }
            Task { await activate() }
        }
        // Review fix round 1, ruling 8: a client swap that stays healthy
        // throughout (a remote reconnect to a DIFFERENT CP that never dips
        // unhealthy) fires no `backend.health` change at all, so the
        // `.onChange` above alone would leave this scene's store bound to
        // the abandoned backend. Same hazard, same fix `RupuApp.swift`'s
        // `MenuBarExtra` label already applies for its own always-alive
        // observers (`RupuApp.swift` around line 457) — `activate()`'s own
        // `storeClientID != clientID` check (below) makes a redundant call
        // here a no-op, so firing on every identity change, not just a
        // "real" swap, is safe.
        .onChange(of: backend.clientIdentity()) { _, _ in
            Task { await activate() }
        }
        .onDisappear {
            // The live tail must not outlive this window — see
            // `SituationStore.deactivate()`'s doc comment.
            store?.deactivate()
            store = nil
        }
    }

    /// Builds the store lazily (once `backend.client()` exists) and
    /// rebuilds it on a client-identity change (embedded/remote switch,
    /// reconnect, restart) — same pattern `OverviewScreen.activate()`
    /// documents in full.
    private func activate() async {
        guard let client = backend.client() else { return }
        let clientID = backend.clientIdentity()
        if store == nil || storeClientID != clientID {
            store?.deactivate()
            store = SituationStore(
                client: client,
                signalsFactory: Self.makeSignalsFactory(backend: backend),
                pendingActions: backend.pendingActions
            )
            storeClientID = clientID
        }
        await store?.activate()
    }

    private func content(store: SituationStore) -> some View {
        let snapshot = assembleSituation(
            eventRows: store.eventRows,
            findings: store.findings,
            findingsSummary: store.findingsSummary,
            projects: store.projects,
            runToWorkspace: store.runToWorkspace,
            runTerminalStatus: store.runTerminalStatus,
            dashboard: store.dashboard,
            eventsPerMin: store.eventsPerMin
        )
        let projectsByWorkspace = Dictionary(uniqueKeysWithValues: store.projects.map { ($0.wsID, $0) })

        return VStack(spacing: 0) {
            PulseStrip(vitals: snapshot.vitals, freshness: store.freshness, spark: store.spark)
            HStack(spacing: 0) {
                EventStreamColumn(
                    cards: snapshot.cards,
                    filter: $filter,
                    projectsByWorkspace: projectsByWorkspace,
                    runToWorkspace: store.runToWorkspace,
                    pendingActions: store.pendingActions,
                    onApprove: { runID, stepID in await store.approve(runID: runID, stepID: stepID) },
                    onReject: { runID, stepID in await store.reject(runID: runID, stepID: stepID) },
                    onOpenRun: { runID, host in openRun(runID: runID, host: host) }
                )
                RosterColumn(roster: snapshot.roster, onSelect: { wsID in openProject(wsID: wsID) })
            }
        }
    }

    private var notReadyView: some View {
        VStack(spacing: 8) {
            ProgressView().controlSize(.small)
            Text("Connecting").font(.noteText).foregroundStyle(Color.rupuMute)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    // MARK: - Deep links

    private func openRun(runID: String, host: String?) {
        model.navigate(to: .runDetail(id: runID, host: host))
        frontMainWindow()
    }

    private func openProject(wsID: String) {
        model.navigate(to: .projectDetail(wsID: wsID))
        frontMainWindow()
    }

    // MARK: - Firehose plumbing

    /// Duplicated, deliberately, from `OverviewScreen.makeSignalsFactory` /
    /// `ActivityScreen.makeSignalsFactory` — every consumer of
    /// `backend.eventStream()`'s single already-claimed `onConnectionChange`
    /// slot needs its own independent connection via
    /// `backend.makeFirehoseStream(onConnectionChange:)`, paired with its own
    /// callback. Situation Room gets its own, separate from every other
    /// screen's, same rationale those two types' doc comments give in full.
    private static func makeSignalsFactory(
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
}
