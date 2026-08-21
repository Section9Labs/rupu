import SwiftUI
import Observation
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
    @State private var connectivity: LiveConnectivityBridge?

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
            ActivityTable(rows: store.rows, onSelect: handleSelect)
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
            let bridge = LiveConnectivityBridge(model: model)
            connectivity = bridge
            let newStore = ActivityStore(
                client: client,
                signalsFactory: Self.makeSignalsFactory(
                    eventStream: backend.eventStream(),
                    connectivity: bridge
                )
            )
            store = newStore
            activeStore = newStore
        }
        await activeStore.activate(kind: kind)
    }

    private func handleSelect(_ row: ActivityRow) {
        switch row.navigation {
        case .run(let id, let host):
            model.route = .runDetail(id: id, host: host)
        case .session(let id):
            model.route = .sessionDetail(id: id)
        case .none:
            break
        }
    }

    /// Composes `ActivityStore`'s required `signalsFactory` from the app's
    /// existing plumbing, without touching `BackendController`/`CPClient`
    /// (out of this task's scope) to add a second `onConnectionChange`
    /// subscriber slot:
    ///
    /// - Events: pumps `backend.eventStream()`'s (Phase 1's shared firehose
    ///   client) `.events()` into `.event(...)` signals. `.events()` spins
    ///   up an independent reconnecting SSE connection each time it's
    ///   called, so re-invoking this on every `activate()` (kind switch,
    ///   screen revisit) is safe — it doesn't reuse or interfere with
    ///   `RootView`'s own separate `.events()` consumer.
    /// - Connection state: `JSONEventStream`'s `onConnectionChange` closure
    ///   is fixed at construction (already claimed by `RootView`, forwarded
    ///   into `model.liveConnected`) — there's no slot left to attach a
    ///   second one for this screen's own `.events()` call. Rather than
    ///   silently never emitting `.connection(false)` (which would make
    ///   `freshness` incapable of ever showing `.stale`), `.connection(_:)`
    ///   signals are sourced from `model.liveConnected` itself via
    ///   `LiveConnectivityBridge` — the exact same connectivity truth
    ///   already driving the toolbar's live pill, so Activity's staleness
    ///   banner and the toolbar dot can never disagree.
    ///
    /// Captures only `Sendable` values (`EventStreamClient` is
    /// unconditionally `Sendable`; `LiveConnectivityBridge` is
    /// `@unchecked Sendable` by construction, see its doc comment) —
    /// `model`/`backend` themselves are `@MainActor` reference types and
    /// cannot be captured directly in this `@Sendable` closure.
    private static func makeSignalsFactory(
        eventStream: EventStreamClient?,
        connectivity: LiveConnectivityBridge
    ) -> @Sendable () -> AsyncStream<StreamSignal<CPEvent>> {
        {
            // `makeSignalBridge` is declared on `StreamLifecycle`
            // (`@MainActor`) even though its body touches no actor-isolated
            // state — it's a pure `AsyncStream.makeStream()` factory. This
            // closure's own type (`@Sendable () -> ...`) can't carry
            // isolation, but every real caller (`ActivityStore.
            // restartStream()`) is itself `@MainActor`, so this assumption
            // always holds in practice.
            let (_, continuation, signals) = MainActor.assumeIsolated {
                StreamLifecycle.makeSignalBridge(CPEvent.self)
            }
            guard let eventStream else {
                continuation.finish()
                return signals
            }

            let eventPump = Task {
                for await event in eventStream.events() {
                    continuation.yield(.event(event))
                }
            }
            let connectionPump = Task {
                for await connected in connectivity.stream() {
                    continuation.yield(.connection(connected))
                }
            }
            continuation.onTermination = { _ in
                eventPump.cancel()
                connectionPump.cancel()
            }
            return signals
        }
    }
}

/// Bridges `AppModel.liveConnected` (an `@Observable`, `@MainActor`
/// property) into a `Sendable`-capturable factory of `AsyncStream<Bool>`,
/// so `ActivityScreen`'s `@Sendable` `signalsFactory` closure can observe
/// it without capturing the non-`Sendable` `AppModel` class directly.
///
/// `@unchecked Sendable`: the only stored state is a `weak` reference to a
/// `@MainActor`-isolated class, and every access to it — both the initial
/// read and the `withObservationTracking` re-subscribe loop — happens
/// inside an explicit `Task { @MainActor in ... }` hop (`stream()`'s
/// producer closure, and `observe`'s `onChange` continuation). Nothing here
/// ever reads or writes `model` from off the main actor.
final class LiveConnectivityBridge: @unchecked Sendable {
    private weak var model: AppModel?

    init(model: AppModel) {
        self.model = model
    }

    /// A fresh stream each call — mirrors `ActivityStore`'s own
    /// `signalsFactory` contract (single-consumption `AsyncStream`, called
    /// fresh on every `activate()`), so a kind switch or screen revisit
    /// that rebuilds the whole `signals` stream also gets a fresh
    /// connectivity feed rather than reusing an already-terminated one.
    func stream() -> AsyncStream<Bool> {
        AsyncStream { continuation in
            let task = Task { @MainActor [weak model] in
                guard let model else {
                    continuation.finish()
                    return
                }
                Self.observe(model: model, continuation: continuation)
            }
            continuation.onTermination = { _ in
                task.cancel()
            }
        }
    }

    @MainActor
    private static func observe(model: AppModel, continuation: AsyncStream<Bool>.Continuation) {
        continuation.yield(model.liveConnected)
        withObservationTracking {
            _ = model.liveConnected
        } onChange: { [weak model] in
            Task { @MainActor in
                guard let model else {
                    continuation.finish()
                    return
                }
                continuation.yield(model.liveConnected)
                observe(model: model, continuation: continuation)
            }
        }
    }
}
