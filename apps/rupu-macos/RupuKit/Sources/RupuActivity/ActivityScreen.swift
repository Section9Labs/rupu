import SwiftUI
import RupuAPI
import RupuStore
import RupuDesign

/// The real Activity table (replaces the Phase 1 `PlaceholderScreen` for
/// `.activity` routes): `FilterBar` + a merged `ActivityTable` fed by
/// `ActivityStore`, with loading/failed/empty states and a staleness
/// banner. Owns the store's lifecycle — built lazily on first appearance
/// (once `backend.client()` is available), `activate(kind:)`d on every kind
/// change via `.task(id:)`, `deactivate()`d `.onDisappear` — matching the
/// symmetric restart pair `ActivityStore` documents itself as needing.
public struct ActivityScreen: View {
    @Bindable var model: AppModel
    let backend: BackendController

    @State private var store: ActivityStore?

    public init(model: AppModel, backend: BackendController) {
        self.model = model
        self.backend = backend
    }

    /// The kind this screen renders — read from `model.route`, never a
    /// local mirror, so `FilterBar`'s segmented control (which writes
    /// `model.route` directly) and this screen's data-loading always agree
    /// on which kind is current.
    private var kind: RunKindFilter {
        if case .activity(let kind) = model.route { return kind }
        return .all
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if let store {
                FilterBar(model: model, store: store)
                if store.freshness == .stale {
                    MicroLabel("STREAM STALE — RECONNECTING")
                        .foregroundStyle(Color.rupuMute)
                }
                if store.pendingHosts > 0 {
                    // Progressive per-host loading (hotfix): local rows are
                    // already showing by the time this ever renders —
                    // `store.state` never waits on remote hosts (see
                    // `ActivityStore`'s doc comment) — this is purely an
                    // "more may still show up" signal, never a blocking one.
                    MicroLabel("+\(store.pendingHosts) HOST\(store.pendingHosts == 1 ? "" : "S") LOADING…")
                        .foregroundStyle(Color.rupuMute)
                }
                stateBody(store: store)
            } else {
                blockView(label: "BACKEND NOT CONNECTED")
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(Color.rupuBg)
        .task(id: kind) {
            await activate(kind: kind)
        }
        .onDisappear {
            store?.deactivate()
        }
    }

    @ViewBuilder
    private func stateBody(store: ActivityStore) -> some View {
        switch store.state {
        case .loading:
            loadingView
        case .failed(let message):
            failedView(message: message, store: store)
        case .empty:
            blockView(label: "NO EXECUTIONS IN RANGE")
        case .content:
            ActivityTable(rows: store.rows, store: store, backend: backend, onSelect: handleSelect)
        }
    }

    private var loadingView: some View {
        VStack {
            Spacer(minLength: 0)
            ProgressView()
                .controlSize(.small)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .panelStyle(.panel)
    }

    private func failedView(message: String, store: ActivityStore) -> some View {
        VStack(spacing: 10) {
            Spacer(minLength: 0)
            MicroLabel("FAILED TO LOAD ACTIVITY")
                .foregroundStyle(Color.status(.fail))
            Text(message)
                .font(.system(size: 12))
                .foregroundStyle(Color.rupuDim)
                .multilineTextAlignment(.center)
                .padding(.horizontal, 40)
            Button("Retry") {
                Task { await store.activate(kind: kind) }
            }
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .panelStyle(.panel)
    }

    private func blockView(label: String) -> some View {
        VStack {
            Spacer(minLength: 0)
            MicroLabel(label)
                .foregroundStyle(Color.rupuMute)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .panelStyle(.panel)
    }

    /// Builds the store on first successful call (once `backend.client()`
    /// exists — by the time the shell can route here, onboarding has
    /// already gated on a healthy connection, so this should never stay
    /// nil in practice) and always (re)activates it for the current
    /// `kind` — the only correct way to (re)start `ActivityStore`'s live
    /// stream, per Task 5's report, whether this is the very first
    /// activation or a kind switch mid-session.
    private func activate(kind: RunKindFilter) async {
        let activeStore: ActivityStore
        if let existing = store {
            activeStore = existing
        } else {
            guard let client = backend.client() else { return }
            let newStore = ActivityStore(
                client: client,
                signalsFactory: Self.makeSignalsFactory(backend: backend),
                pendingActions: backend.pendingActions
            )
            store = newStore
            activeStore = newStore
        }
        await activeStore.activate(kind: kind)
    }

    private func handleSelect(_ row: ActivityRow) {
        // Row activation pushes onto `AppModel`'s navigation stack (Phase 3,
        // Task 4) rather than assigning `route` directly — so a chevron back
        // from the pushed screen returns here, to Activity, not past an
        // intermediate screen a later push might stack on top of this one.
        switch row.navigation {
        case .run(let id, let host):
            model.navigate(to: .runDetail(id: id, host: host))
        case .session(let id):
            model.navigate(to: .sessionDetail(id: id))
        case .agentRun(let id, let transcriptPath, let host):
            // Hotfix root cause C: a standalone agent run is never an
            // orchestrator run — `.runDetail` would 404 against `GET
            // /api/runs/:id`. `AgentRunDetailScreen` is the honest
            // destination: transcript-only, via `GET /api/transcript`.
            model.navigate(to: .agentRunDetail(id: id, transcriptPath: transcriptPath, host: host))
        case .none:
            break
        }
    }

    /// Composes `ActivityStore`'s required `signalsFactory` around the
    /// store's **own**, fully independent connection — not `RootView`'s
    /// shared `eventStream()` firehose. `BackendController.eventStream()`'s
    /// `EventStreamClient` has its `onConnectionChange` fixed at
    /// construction (already claimed by `RootView`, forwarded into
    /// `model.liveConnected`), so a consumer that just pumped that
    /// instance's `.events()` a second time would ride a *different*
    /// physical SSE connection than the one `onConnectionChange` is
    /// actually reporting on — this store's own stream could drop and
    /// reconnect while `model.liveConnected` stays blissfully true, and
    /// `freshness` would never notice.
    ///
    /// Fix: `backend.makeFirehoseStream(onConnectionChange:)` builds a
    /// brand-new `JSONEventStream<CPEvent>` end-to-end for this store —
    /// same endpoint/credentials, its own connection, its own callback —
    /// bridged through `StreamLifecycle.makeSignalBridge`'s documented
    /// two-phase recipe: `onChange` is threaded straight into that new
    /// stream's `init`, so every connect/disconnect for *this specific*
    /// connection lands in the same ordered `continuation` as that
    /// connection's own frames, with the synchronous-yield ordering
    /// `makeSignalBridge` guarantees (a connection signal for a given
    /// attempt is always yielded before any frame of that same attempt).
    /// No coalescing, no second independently-scheduled observer to race.
    ///
    /// This does not add a connection versus a naive "just reuse
    /// eventStream()" approach — pumping `eventStream()`'s `.events()` a
    /// second time (the prior design) already opened its own independent
    /// underlying SSE connection; that connection is now just honestly
    /// paired with its own connection-state callback instead of borrowing
    /// `model.liveConnected` (which describes `RootView`'s connection).
    ///
    /// `store.freshness` (the staleness banner) and `model.liveConnected`
    /// (the toolbar pill) can therefore disagree briefly — they now
    /// describe two different physical connections to the same endpoint,
    /// not one shared truth. That's intentional, not a bug: each is
    /// truthful about the connection it actually owns.
    ///
    /// Captures only `backend` (a `@MainActor` reference) inside this
    /// `@Sendable` closure; every access to it is wrapped in
    /// `MainActor.assumeIsolated`, safe because the only real caller,
    /// `ActivityStore.restartStream()`, is itself `@MainActor`.
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
