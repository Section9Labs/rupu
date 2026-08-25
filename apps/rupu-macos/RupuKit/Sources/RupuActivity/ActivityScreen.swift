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
///
/// Flows-composition Task 2: the top bar's `AppModel.scopeWsID` reaches
/// `ActivityStore.scopeFilter` the same way `kind` reaches
/// `activate(kind:)` — this screen is the single site that already owns
/// both "read from `model`" and "drive the store", so it owns this
/// relationship too rather than splitting it into `RootView` (which has no
/// reason to know about `ActivityStore` at all) or `ShellToolbar` (which
/// has no reference to the store — it only ever touches `model`). Unlike
/// `kind`, `scopeFilter` needs no `.task(id:)`/async `activate` — it's a
/// synchronous, no-refetch narrowing (see `ActivityStore.scopeFilter`'s
/// doc comment) — so a plain `.onChange(of: model.scopeWsID)` is enough to
/// keep it live; `activate(kind:)` additionally seeds a freshly-built
/// store with the model's *current* value, since `.onChange` only fires on
/// a change from here on, not on first appearance.
public struct ActivityScreen: View {
    @Bindable var model: AppModel
    let backend: BackendController

    @State private var store: ActivityStore?

    /// Tracked so `activate(kind:)` rebuilds on a backend client swap
    /// (embedded/remote switch, reconnect, restart), not just the first
    /// build — see that method's doc comment and
    /// `BackendController.clientIdentity()`.
    @State private var storeClientID: ObjectIdentifier?

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
                    Text("Stream stale — reconnecting")
                        .font(.noteText)
                        .foregroundStyle(Color.rupuMute)
                }
                if store.pendingHosts > 0 {
                    // Progressive per-host loading (hotfix): local rows are
                    // already showing by the time this ever renders —
                    // `store.state` never waits on remote hosts (see
                    // `ActivityStore`'s doc comment) — this is purely an
                    // "more may still show up" signal, never a blocking one.
                    Text("+\(store.pendingHosts) host\(store.pendingHosts == 1 ? "" : "s") loading…")
                        .font(.noteText)
                        .foregroundStyle(Color.rupuMute)
                }
                stateBody(store: store)
            } else {
                blockView(label: "Backend not connected")
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(Color.rupuBg)
        .task(id: kind) {
            await activate(kind: kind)
        }
        .onChange(of: model.scopeWsID) { _, newScope in
            store?.scopeFilter = newScope
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
            blockView(label: "No executions in range")
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
            Text("Failed to load activity")
                .font(.noteText)
                .foregroundStyle(Color.status(.failed))
            Text(message)
                .font(.uiText)
                .foregroundStyle(Color.rupuDim)
                .multilineTextAlignment(.center)
                .padding(.horizontal, 40)
            Button("Retry") {
                Task { await store.activate(kind: kind) }
            }
            .buttonStyle(RupuButtonStyle.outline)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .panelStyle(.panel)
    }

    private func blockView(label: String) -> some View {
        VStack {
            Spacer(minLength: 0)
            Text(label)
                .font(.noteText)
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
    ///
    /// Also rebuilds — deactivating the old store first — whenever
    /// `backend.client()`'s identity has changed since the store currently
    /// held was built: an embedded/remote mode switch, a manual reconnect,
    /// or a restart all swap `backend.client()` to a brand-new `CPClient`
    /// directly (never through `nil` in between — see
    /// `BackendController.clientIdentity()`'s doc comment), so a plain "do I
    /// already have a store" check would never notice and would keep
    /// running `store` against the abandoned connection until this screen
    /// happened to be torn down and rebuilt some other way (e.g. navigating
    /// away and back).
    private func activate(kind: RunKindFilter) async {
        guard let client = backend.client() else { return }
        let clientID = backend.clientIdentity()

        let activeStore: ActivityStore
        if let existing = store, storeClientID == clientID {
            activeStore = existing
        } else {
            store?.deactivate()
            let newStore = ActivityStore(
                client: client,
                signalsFactory: Self.makeSignalsFactory(backend: backend),
                pendingActions: backend.pendingActions
            )
            // Seed the store's scope with whatever the top bar's picker
            // already has selected (e.g. this is a relaunch that restored
            // a persisted `scopeWsID`) — `.onChange(of: model.scopeWsID)`
            // below only fires on a *change*, so a store built after the
            // picker already has a non-nil selection needs this initial
            // sync too.
            newStore.scopeFilter = model.scopeWsID
            store = newStore
            storeClientID = clientID
            activeStore = newStore
        }
        await activeStore.activate(kind: kind)
    }

    private func handleSelect(_ row: ActivityRow) {
        // Row activation pushes onto `AppModel`'s navigation stack (Phase 3,
        // Task 4) rather than assigning `route` directly — so a chevron back
        // from the pushed screen returns here, to Activity, not past an
        // intermediate screen a later push might stack on top of this one.
        // The kind-discrimination itself (hotfix root cause C: a standalone
        // agent run is never an orchestrator run — `.runDetail` would 404
        // against `GET /api/runs/:id`) now lives in
        // `ActivityRow.Navigation.route` (flows-composition Task 3), shared
        // with the command palette's run search.
        guard let route = row.navigation.route else { return }
        model.navigate(to: route)
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
