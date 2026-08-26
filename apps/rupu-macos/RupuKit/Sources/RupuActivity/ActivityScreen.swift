import SwiftUI
import RupuAPI
import RupuStore
import RupuDesign

/// The real Activity screen (replaces the Phase 1 `PlaceholderScreen` for
/// `.activity` routes). **Restructured in perf & interaction arc, Plan 5
/// Task 4** (matt's direct feedback: stop showing one combined table with a
/// kind-picker): the `.all` kind renders `ActivityStatsView` (KPI cards +
/// a compact needs-attention list, no table, no kind picker); every other
/// kind (`agents`/`workflows`/`autoflows`/`sessions`) renders its OWN
/// dedicated table (`RupuActivity/KindTables/`) fed by the exact same
/// `ActivityStore`, with `FilterBar` (status chips/live-tail/"+N new runs"
/// pill) alongside it. The merged `ActivityTable` this screen used to show
/// unconditionally, and the kind segmented picker that selected into it,
/// are both deleted — every kind is reached via the sidebar's disclosure
/// children (Task 0) instead.
///
/// **Shared store, not owned** (perf & interaction arc, Plan 5 Task 2):
/// `activityStore` is built once at `RootView` and injected here — this
/// screen no longer constructs its own instance (see `RootView.
/// activityStore`'s doc comment for why: `OverviewScreen` used to build a
/// SECOND, independent `ActivityStore` purely for its needs-you queue, each
/// with its own resting SSE connection). This screen still fully owns
/// DRIVING it, though: `activate(kind:)`d on every kind change (and on a
/// backend client swap — see `ActivityTaskID`) via `.task(id:)`,
/// `deactivate()`d `.onDisappear` — the same symmetric restart pair
/// `ActivityStore` documents itself as needing, just against an
/// externally-owned instance rather than a locally-built one.
///
/// Flows-composition Task 2: the top bar's `AppModel.scopeWsID` reaches
/// `ActivityStore.scopeFilter` the same way `kind` reaches
/// `activate(kind:)` — this screen is the single site that already owns
/// both "read from `model`" and "drive the store", so it owns this
/// relationship too. Unlike `kind`, `scopeFilter` needs no `.task(id:)`/
/// async `activate` — it's a synchronous, no-refetch narrowing (see
/// `ActivityStore.scopeFilter`'s doc comment) — so a plain `.onChange(of:
/// model.scopeWsID)` is enough to keep it live; `activate(kind:)`
/// additionally seeds the store with the model's *current* value on every
/// activation, since `.onChange` only fires on a change from here on, not
/// on activation itself.
///
/// **Does NOT need `OverviewScreen`'s old `.onChange(of: backend.health)`
/// cold-launch fix**: that fix existed because `.overview` is
/// `AppModel.route`'s default and never persisted, so it is *always* the
/// very first screen a cold launch renders — sometimes before
/// `backend.client()` resolves. This screen can only ever be reached by
/// navigating here (sidebar click, `AppModel.navigate(to:)`) — by the time
/// that happens, the shell has already been up and running for at least
/// one prior screen's worth of time. If `route` ever becomes persisted/
/// restored across launches (it isn't today — see `AppModel.swift`), this
/// reasoning would need revisiting.
public struct ActivityScreen: View {
    @Bindable var model: AppModel
    let backend: BackendController

    /// The single shared instance `RootView` constructs and injects (perf &
    /// interaction arc, Plan 5 Task 2) — no longer built by this screen.
    /// `OverviewScreen` shares the exact same instance for its own
    /// needs-you derivation; the two are mutually exclusive routes, so each
    /// safely reconfigures it (`kind`/`scopeFilter`) to its own needs on
    /// activation without conflicting. See `RootView.activityStore`'s doc
    /// comment for the full rationale (dropping a second, independent SSE
    /// connection `OverviewScreen` used to open purely for this).
    let activityStore: ActivityStore?

    /// Phase 6B, Task 3: the autoflows-kind Runs/Claims sub-toggle — a
    /// screen-local `@State`, never persisted, never read by anything
    /// outside this screen (unlike `kind`, which the sidebar/command palette
    /// also care about via `model.route`). Defaults to `.runs` — the
    /// existing federated-feed view every other kind already shows, so
    /// switching TO the autoflows kind never surprises a returning user with
    /// an unfamiliar screen.
    @State private var autoflowsSubTab: AutoflowsSubTab = .runs
    @State private var claimsStore: ClaimsStore?
    /// Same rebuild-on-client-swap rationale as `storeClientID` above,
    /// tracked independently — `claimsStore` has its own lazy-build
    /// lifecycle (only ever built once the Claims sub-tab is first
    /// selected), so it needs its own client-identity watermark rather than
    /// reusing `storeClientID`.
    @State private var claimsStoreClientID: ObjectIdentifier?

    /// Cycles sub-tab (perf & interaction arc, Plan 5 Task 4b) — same
    /// lazy-build-on-first-selection lifecycle as `claimsStore`/
    /// `claimsStoreClientID` above, kept as its own pair rather than
    /// generalizing the two into one mechanism: they're independent stores
    /// with independent activate/deactivate contracts (`CyclesStore` is
    /// generation-guarded and progressively remote-loading; `ClaimsStore` is
    /// a plain `load()`), so sharing bookkeeping between them would only
    /// couple two things that don't actually need to change together.
    @State private var cyclesStore: CyclesStore?
    @State private var cyclesStoreClientID: ObjectIdentifier?

    public init(model: AppModel, backend: BackendController, activityStore: ActivityStore?) {
        self.model = model
        self.backend = backend
        self.activityStore = activityStore
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
        let claimsActive = Self.isClaimsActive(kind: kind, subTab: autoflowsSubTab)
        let cyclesActive = Self.isCyclesActive(kind: kind, subTab: autoflowsSubTab)
        VStack(alignment: .leading, spacing: 12) {
            if let store = activityStore {
                // `FilterBar` (status chips / live-tail / "+N new runs" pill)
                // only ever renders on a KIND page now (perf & interaction
                // arc, Plan 5 Task 4 — matt's restructure: the `.all` parent
                // shows `ActivityStatsView` instead, which has no table for
                // these controls to act on) — same no-dead-controls reasoning
                // `showRunsChrome` already applied to the Claims sub-tab, now
                // also applied to Cycles (Task 4b): both sub-tabs sit over
                // their OWN store, never `ActivityStore`'s `ActivityTable`-
                // successor, so this bar/these banners would be inert chrome
                // for either.
                if kind != .all, !claimsActive, !cyclesActive {
                    FilterBar(store: store)
                }
                if !claimsActive, !cyclesActive {
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
                }
                if kind == .autoflows {
                    autoflowsSubTabPicker
                }
                if kind == .all {
                    // The Activity parent (perf & interaction arc, Plan 5 Task
                    // 4 restructure): a stats surface, never a table or kind
                    // picker — every kind's own table lives one level down at
                    // the sidebar's disclosure children (already wired since
                    // Task 0; unaffected by this change).
                    ActivityStatsView(store: store, backend: backend, range: model.range, onNavigate: { model.navigate(to: $0) })
                } else if claimsActive {
                    claimsBody
                } else if cyclesActive {
                    cyclesBody
                } else {
                    stateBody(store: store)
                }
            } else {
                blockView(label: "Backend not connected")
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(Color.rupuBg)
        .task(id: ActivityTaskID(kind: kind, clientID: backend.clientIdentity())) {
            await activate(kind: kind)
        }
        .task(id: ClaimsTaskID(kind: kind, subTab: autoflowsSubTab, clientID: backend.clientIdentity())) {
            await activateClaimsIfNeeded()
        }
        .task(id: CyclesTaskID(kind: kind, subTab: autoflowsSubTab, clientID: backend.clientIdentity())) {
            await activateCyclesIfNeeded()
        }
        .onChange(of: model.scopeWsID) { _, newScope in
            activityStore?.scopeFilter = newScope
        }
        .onDisappear {
            activityStore?.deactivate()
            cyclesStore?.deactivate()
        }
    }

    /// **Only ever called for a KIND page** (`kind != .all` — the `.all`
    /// parent renders `ActivityStatsView` instead, never this). Routes to
    /// the dedicated per-kind table (perf & interaction arc, Plan 5 Task 4)
    /// — the merged `ActivityTable` this used to show unconditionally was
    /// deleted along with the kind picker that selected into it; `.all`
    /// itself is unreachable here (`fatalError` would be defensive
    /// overkill for a `switch` this method's own doc comment already rules
    /// out — see the caller in `body`, which never reaches this for `.all`).
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
            switch kind {
            case .all:
                blockView(label: "No executions in range")
            case .agents:
                AgentRunsTable(rows: store.rows, store: store, backend: backend, onSelect: handleSelect)
            case .workflows:
                WorkflowRunsTable(rows: store.rows, store: store, backend: backend, onSelect: handleSelect)
            case .autoflows:
                AutoflowRunsTable(rows: store.rows, store: store, backend: backend, onSelect: handleSelect)
            case .sessions:
                SessionsTable(rows: store.rows, onSelect: handleSelect)
            }
        }
    }

    // MARK: - Autoflows Runs/Claims sub-toggle (Phase 6B, Task 3)

    /// `true` exactly when the Claims sub-tab is the one currently showing
    /// (review fix, round 1) — every Runs-only control (`FilterBar`'s status
    /// chips / live-tail toggle / "+N new runs" pill, and this screen's own
    /// stale/pending-hosts stream banners) sits over an invisible
    /// `ActivityTable` while this is `true` and must be suppressed (the
    /// no-dead-controls rule) rather than staying live-and-inert; the kind
    /// picker itself stays regardless — it's the only way back out to
    /// `.runs`. Pure and `static` (not a computed instance property) so
    /// `ActivityScreenClaimsChromeTests` can assert it flips with `subTab`
    /// without standing up a full view render.
    static func isClaimsActive(kind: RunKindFilter, subTab: AutoflowsSubTab) -> Bool {
        kind == .autoflows && subTab == .claims
    }

    /// Same contract as `isClaimsActive` above, for the Cycles sub-tab (perf
    /// & interaction arc, Plan 5 Task 4b) — suppresses the same Runs-only
    /// chrome (`FilterBar`, stale/pending-hosts banners) while Cycles is
    /// showing, since that sub-tab sits over its own `CyclesStore`, not
    /// `ActivityStore`'s per-kind table.
    static func isCyclesActive(kind: RunKindFilter, subTab: AutoflowsSubTab) -> Bool {
        kind == .autoflows && subTab == .cycles
    }

    private var autoflowsSubTabPicker: some View {
        Picker("View", selection: $autoflowsSubTab) {
            ForEach(AutoflowsSubTab.allCases, id: \.self) { tab in
                Text(tab.label).tag(tab)
            }
        }
        .pickerStyle(.segmented)
        .frame(width: 160)
        .labelsHidden()
    }

    /// The inner `claimsStore == nil` branch (review fix, round 1) renders
    /// `loadingView`, not `blockView(label: "Backend not connected")` — by
    /// the time this ever evaluates, the OUTER `if let store` has already
    /// proven the backend IS connected (this whole `claimsBody` is only ever
    /// reached from inside that branch); a nil `claimsStore` here can only
    /// mean the one-frame gap before `activateClaimsIfNeeded()`'s `.task(id:)`
    /// finishes building and assigning it, i.e. genuinely loading, not
    /// disconnected. The OUTER `else` (line ~112, `store == nil`) keeps its
    /// own "Backend not connected" text — that one really is describing a
    /// disconnected backend.
    @ViewBuilder
    private var claimsBody: some View {
        if let claimsStore {
            switch claimsStore.claims {
            case .loading:
                loadingView
            case .failed(let message):
                claimsFailedView(message: message, store: claimsStore)
            case .empty:
                blockView(label: "No tracked claims")
            case .content(let rows):
                ClaimsTable(rows: rows, store: claimsStore)
            }
        } else {
            loadingView
        }
    }

    private func claimsFailedView(message: String, store: ClaimsStore) -> some View {
        VStack(spacing: 10) {
            Spacer(minLength: 0)
            Text("Failed to load claims")
                .font(.noteText)
                .foregroundStyle(Color.status(.failed))
            Text(message)
                .font(.uiText)
                .foregroundStyle(Color.rupuDim)
                .multilineTextAlignment(.center)
                .padding(.horizontal, 40)
            Button("Retry") {
                Task { await store.load() }
            }
            .buttonStyle(RupuButtonStyle.outline)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .panelStyle(.panel)
    }

    /// Lazy-builds (and, on a backend client swap, rebuilds — same
    /// `storeClientID` recipe `activate(kind:)` above already establishes)
    /// `claimsStore` and loads it, but ONLY once the Claims sub-tab is
    /// actually selected — `.task(id:)`'s `ClaimsTaskID` folds in `kind` AND
    /// `autoflowsSubTab` AND the backend's client identity, so this fires
    /// exactly on first selection, on a later re-selection after a kind
    /// round-trip, and on a client swap while Claims happens to already be
    /// selected — never eagerly for every other kind/sub-tab combination.
    /// Folding `clientID` into the task id (not just checking it inside the
    /// body, PR #501's exact bug class) is what makes a client swap actually
    /// restart this task rather than leaving a stale closure captured over
    /// an abandoned `CPClient` running to completion against a connection
    /// nothing else is using any more.
    private func activateClaimsIfNeeded() async {
        guard kind == .autoflows, autoflowsSubTab == .claims else { return }
        guard let client = backend.client() else { return }
        let clientID = backend.clientIdentity()
        let activeStore: ClaimsStore
        if let existing = claimsStore, claimsStoreClientID == clientID {
            activeStore = existing
        } else {
            let newStore = ClaimsStore(client: client, pendingActions: backend.pendingActions)
            claimsStore = newStore
            claimsStoreClientID = clientID
            activeStore = newStore
        }
        await activeStore.load()
    }

    // MARK: - Autoflows Cycles sub-tab (perf & interaction arc, Plan 5 Task 4b)

    /// Same `claimsStore == nil` reasoning as `claimsBody`'s doc comment —
    /// by the time this is reached, the outer `if let store` has already
    /// proven the backend is connected, so a `nil` `cyclesStore` here is
    /// only ever the one-frame gap before `activateCyclesIfNeeded()`'s
    /// `.task(id:)` finishes building and assigning it.
    @ViewBuilder
    private var cyclesBody: some View {
        if let cyclesStore {
            VStack(alignment: .leading, spacing: 8) {
                if cyclesStore.pendingHosts > 0 {
                    Text("+\(cyclesStore.pendingHosts) host\(cyclesStore.pendingHosts == 1 ? "" : "s") loading…")
                        .font(.noteText)
                        .foregroundStyle(Color.rupuMute)
                }
                switch cyclesStore.state {
                case .loading:
                    loadingView
                case .failed(let message):
                    cyclesFailedView(message: message, store: cyclesStore)
                case .empty:
                    blockView(label: "No autoflow cycles yet")
                case .content:
                    AutoflowCyclesTable(rows: cyclesStore.rows, onSelectRun: { model.navigate(to: $0) })
                }
            }
        } else {
            loadingView
        }
    }

    private func cyclesFailedView(message: String, store: CyclesStore) -> some View {
        VStack(spacing: 10) {
            Spacer(minLength: 0)
            Text("Failed to load cycles")
                .font(.noteText)
                .foregroundStyle(Color.status(.failed))
            Text(message)
                .font(.uiText)
                .foregroundStyle(Color.rupuDim)
                .multilineTextAlignment(.center)
                .padding(.horizontal, 40)
            Button("Retry") {
                Task { await store.refresh() }
            }
            .buttonStyle(RupuButtonStyle.outline)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .panelStyle(.panel)
    }

    /// Lazy-builds (and, on a backend client swap, rebuilds) `cyclesStore`
    /// and activates it, but ONLY once the Cycles sub-tab is actually
    /// selected — same `CyclesTaskID`-folds-in-everything contract
    /// `activateClaimsIfNeeded()`'s doc comment documents for its sibling.
    /// `activate()` (not a plain `load()`/`refresh()`) — `CyclesStore` needs
    /// its generation bumped on every (re)activation so a prior selection's
    /// still-in-flight remote-host fetches never land on top of this one.
    private func activateCyclesIfNeeded() async {
        guard kind == .autoflows, autoflowsSubTab == .cycles else { return }
        guard let client = backend.client() else { return }
        let clientID = backend.clientIdentity()
        let activeStore: CyclesStore
        if let existing = cyclesStore, cyclesStoreClientID == clientID {
            activeStore = existing
        } else {
            let newStore = CyclesStore(client: client)
            cyclesStore = newStore
            cyclesStoreClientID = clientID
            activeStore = newStore
        }
        await activeStore.activate()
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

    /// (Re)activates the shared `activityStore` for the current `kind` —
    /// the only correct way to (re)start `ActivityStore`'s live stream, per
    /// Task 5's report, whether this is the very first activation, a kind
    /// switch mid-session, or a re-activation after a client swap (the
    /// `.task(id:)` this drives folds `backend.clientIdentity()` into
    /// `ActivityTaskID`, so a swap — an embedded/remote mode switch, a
    /// manual reconnect, a restart — always re-runs this, same as before
    /// this screen stopped building its own store — see `RootView.
    /// activityStore`'s doc comment for who builds it now). A `nil`
    /// `activityStore` (backend not connected yet) is a no-op; `body`'s own
    /// `if let store = activityStore` already renders the right "Backend
    /// not connected" fallback for that case.
    ///
    /// Seeds the store's scope with whatever the top bar's picker already
    /// has selected (e.g. this is a relaunch that restored a persisted
    /// `scopeWsID`, or the shared store was just rebuilt by `RootView` after
    /// a client swap) — `.onChange(of: model.scopeWsID)` only fires on a
    /// *change* from here on, not on activation, so this is the initial
    /// sync every activation needs regardless.
    private func activate(kind: RunKindFilter) async {
        guard let activityStore else { return }
        activityStore.scopeFilter = model.scopeWsID
        await activityStore.activate(kind: kind)
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

}

/// `.task(id:)` identity for the main kind-scoped activation
/// (`ActivityScreen.activate(kind:)`). Folds in `kind` (fires on every kind
/// switch) AND `clientID` (perf & interaction arc, Plan 5 Task 2 — since
/// this screen no longer builds its own store, a client swap — embedded/
/// remote toggle, reconnect, restart, which rebuilds the SHARED
/// `activityStore` at `RootView` — must still re-run `activate(kind:)`
/// against the fresh instance even when `kind` itself hasn't changed; same
/// "fold the client identity into the task id" contract `ClaimsTaskID`
/// right below already establishes for the Claims sub-tab).
private struct ActivityTaskID: Equatable {
    let kind: RunKindFilter
    let clientID: ObjectIdentifier?
}

/// The autoflows-kind sub-toggle (Phase 6B, Task 3; Cycles added perf &
/// interaction arc Plan 5 Task 4b) — screen-local, not a `RunKindFilter`
/// case: every other kind (`.all`/`.agents`/`.workflows`/`.sessions`) has no
/// equivalent second view, so this narrower toggle lives entirely inside
/// `ActivityScreen`, only ever rendered when `kind == .autoflows`. Order
/// matches the web's own Autoflows page tab order (Runs/Cycles/Claims).
enum AutoflowsSubTab: String, CaseIterable, Sendable {
    case runs, cycles, claims

    var label: String {
        switch self {
        case .runs: "Runs"
        case .cycles: "Cycles"
        case .claims: "Claims"
        }
    }
}

/// `.task(id:)` identity for the Claims sub-tab's lazy load
/// (`ActivityScreen.activateClaimsIfNeeded()`). Folds in every value that
/// load depends on — `kind`/`autoflowsSubTab` (so it fires exactly once per
/// distinct "landed on Claims" transition, not on every unrelated body
/// re-render) AND `clientID` (so a backend client swap — embedded/remote
/// toggle, reconnect, restart — actually restarts this task against the new
/// client rather than a stale closure quietly finishing out its await
/// against an abandoned connection, the exact class of bug PR #501 fixed for
/// run-state closures).
private struct ClaimsTaskID: Equatable {
    let kind: RunKindFilter
    let subTab: AutoflowsSubTab
    let clientID: ObjectIdentifier?
}

/// `.task(id:)` identity for the Cycles sub-tab's lazy load
/// (`ActivityScreen.activateCyclesIfNeeded()`) — same "fold in kind/subTab/
/// clientID" contract `ClaimsTaskID` documents for its own sub-tab, applied
/// to the third one (perf & interaction arc, Plan 5 Task 4b).
private struct CyclesTaskID: Equatable {
    let kind: RunKindFilter
    let subTab: AutoflowsSubTab
    let clientID: ObjectIdentifier?
}
