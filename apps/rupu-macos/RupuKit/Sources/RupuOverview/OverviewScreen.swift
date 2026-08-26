import Foundation
import RupuAPI
import RupuDesign
import RupuStore
import SwiftUI

// MARK: - Widget visibility persistence

/// The Overview screen's block-visibility toggles AND block order (spec §4,
/// "Customize & persistence"; order added by the redesign pass's Task 3 —
/// see `docs/superpowers/specs/2026-08-24-rupu-macos-phase-4-dashboard-design.md`
/// for the original visibility-only cut). A plain `Codable`/`Equatable`
/// struct, not a `View` member or an `@Observable` store — CI rule: only a
/// test touching a `View`-type member needs `@MainActor`, and this stays
/// testable without it.
///
/// **Absent → all visible**: every field defaults `true`, so a fresh install
/// (nothing ever saved under `storageKey`) shows the full dashboard, never a
/// blank one. The host freshness strip (composition item 1) has no toggle
/// here — it's chrome, not a widget the operator would ever want to hide.
///
/// **Two independent live paths to the same key**: `ShellToolbar`'s
/// Customize menu and this screen's block gating each declare their own
/// `@AppStorage(OverviewWidgets.storageKey)` (same trick `"appearance"`
/// already uses across `SettingsView`/`ShellToolbar` — two unrelated views,
/// one shared key, kept in sync by `@AppStorage` itself, no custom
/// observation code needed) storing the raw `Data`, decoded/encoded through
/// `decode(_:)`/`encoded` below. `load(defaults:)`/`save(defaults:)` are a
/// second, SwiftUI-free entry point to the exact same key — the seam
/// `WidgetConfigTests` exercises directly (mirrors `AppModel.init(defaults:)`
/// — an injectable `UserDefaults` rather than a hardcoded `.standard`), not
/// something production code needs to call given the `@AppStorage` path
/// above already round-trips through the same `decode`/`encoded` logic.
public struct OverviewWidgets: Codable, Equatable, Sendable {
    public var needsYou: Bool
    public var instruments: Bool
    public var charts: Bool
    public var cycles: Bool
    public var fleet: Bool

    /// Block-id order for the Customize menu's reorder affordance (Task 3).
    /// After `init`/`decode(_:)` (which route through `normalized(_:)`):
    /// exactly `defaultOrder`'s set of ids, in whatever order the
    /// operator dragged them to — see `normalized(_:)` for the forward-compat
    /// contract that guarantees that invariant on every decode.
    public var order: [String]

    public static let storageKey = "overview.widgets"

    /// Canonical id set and their ship-default order. `OverviewOrderEditor`'s
    /// "Reset Order" restores exactly this array; `normalized(_:)` treats it
    /// as both the allowlist (unrecognized persisted ids are dropped) and the
    /// fallback ordering (ids missing from a persisted array — a block that
    /// didn't exist yet when it was saved — are appended in this relative
    /// order, never spliced into the middle of what the operator arranged).
    public static let defaultOrder = ["needsYou", "instruments", "charts", "cycles", "fleet"]

    public init(
        needsYou: Bool = true,
        instruments: Bool = true,
        charts: Bool = true,
        cycles: Bool = true,
        fleet: Bool = true,
        order: [String] = OverviewWidgets.defaultOrder
    ) {
        self.needsYou = needsYou
        self.instruments = instruments
        self.charts = charts
        self.cycles = cycles
        self.fleet = fleet
        self.order = Self.normalized(order)
    }

    private enum CodingKeys: String, CodingKey {
        case needsYou, instruments, charts, cycles, fleet, order
    }

    /// Custom decode so a `Data` blob saved before this field existed (every
    /// install's persisted state prior to Task 3) decodes cleanly instead of
    /// failing the whole struct: a missing `order` key falls back to
    /// `defaultOrder`, then `normalized(_:)` still runs over it (a no-op for
    /// the default array, but keeps this one code path as the sole source of
    /// the "always exactly the canonical id set" invariant).
    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        needsYou = try container.decodeIfPresent(Bool.self, forKey: .needsYou) ?? true
        instruments = try container.decodeIfPresent(Bool.self, forKey: .instruments) ?? true
        charts = try container.decodeIfPresent(Bool.self, forKey: .charts) ?? true
        cycles = try container.decodeIfPresent(Bool.self, forKey: .cycles) ?? true
        fleet = try container.decodeIfPresent(Bool.self, forKey: .fleet) ?? true
        let persisted = try container.decodeIfPresent([String].self, forKey: .order) ?? Self.defaultOrder
        order = Self.normalized(persisted)
    }

    /// Forward-compat normalization (Task 3's binding rule): drop any id
    /// `defaultOrder` doesn't recognize (a downgrade, or a retired block —
    /// keeps a stray id from silently reserving a rendering slot forever),
    /// preserving the persisted relative order of everything that survives;
    /// then append any canonical id absent from `persisted` — a block added
    /// in a release after this operator's array was saved — at the end, in
    /// `defaultOrder`'s relative order. "Append, don't vanish": a new block
    /// always still renders for existing users, just at the tail rather than
    /// wherever `defaultOrder` would have put it from scratch.
    static func normalized(_ persisted: [String]) -> [String] {
        var seen = Set<String>()
        var result: [String] = []
        for id in persisted where defaultOrder.contains(id) && !seen.contains(id) {
            result.append(id)
            seen.insert(id)
        }
        for id in defaultOrder where !seen.contains(id) {
            result.append(id)
            seen.insert(id)
        }
        return result
    }

    /// Decodes the raw `Data` shape `@AppStorage(storageKey)` stores.
    /// Anything that doesn't decode — corrupt data, or the empty `Data()`
    /// `@AppStorage` hands back before anything has ever been saved — falls
    /// back to all-visible rather than crashing or half-populating.
    public static func decode(_ data: Data) -> OverviewWidgets {
        (try? JSONDecoder().decode(OverviewWidgets.self, from: data)) ?? OverviewWidgets()
    }

    public var encoded: Data {
        (try? JSONEncoder().encode(self)) ?? Data()
    }

    /// Display label for a block id, shared by the Customize menu's reorder
    /// sheet and anywhere else an id needs a human-readable name. Falls back
    /// to the raw id rather than a bogus label if `defaultOrder` and this
    /// switch ever drift.
    public static func label(for id: String) -> String {
        switch id {
        case "needsYou": return "Needs you"
        case "instruments": return "Instruments"
        case "charts": return "Charts"
        case "cycles": return "Cycle summary"
        case "fleet": return "Fleet strip"
        default: return id
        }
    }

    /// Reads straight from `defaults` — no key at all (nothing ever saved)
    /// decodes the same honest way `decode(_:)` handles bad data: all
    /// visible.
    public static func load(defaults: UserDefaults) -> OverviewWidgets {
        guard let data = defaults.data(forKey: storageKey) else { return OverviewWidgets() }
        return decode(data)
    }

    public func save(defaults: UserDefaults) {
        defaults.set(encoded, forKey: Self.storageKey)
    }
}

// MARK: - Screen

/// The Overview screen (spec §1's top-to-bottom composition, replacing the
/// Phase 1 `PlaceholderScreen` for `.overview`): host freshness strip (fixed
/// — chrome, not a widget) followed by needs-you queue / instrument strip /
/// charts row / cycle summary line / fleet strip in `widgets.order`'s
/// persisted sequence (defaults to that same needs-you → instruments →
/// charts → cycles → fleet order — see `OverviewWidgets.defaultOrder`), in
/// one vertical `ScrollView` stack.
///
/// Owns `DashboardStore`'s lifecycle, mirroring `RunDetailScreen`'s pattern
/// (build lazily once `backend.client()` exists, activate on appear,
/// deactivate `.onDisappear`) rather than `ActivityScreen`'s
/// `.task(id:)`-per-kind shape — this screen has no per-instance identity
/// that changes while mounted, so a single `.task(id: backend.
/// clientGeneration)` (fires on first appearance and on any client swap) is
/// enough, EXCEPT that `activate()` still rebuilds `dashboardStore` if
/// `backend.client()`'s identity has changed since it was built (an
/// embedded/remote switch, reconnect, or restart) — see `activate()`'s doc
/// comment:
/// - `DashboardStore` — the merged fleet aggregate behind blocks 3-6. Built
///   and owned here.
/// - `ActivityStore` (kind `.all`, `liveTail` on by default, `scopeFilter`
///   **never set** — see `NeedsYouCard`'s doc comment: the needs-you queue
///   is fleet-wide by design, deliberately blind to the top bar's project
///   scope) — feeds the needs-you queue alone. **Shared, not owned** (perf
///   & interaction arc, Plan 5 Task 2): `activityStore` is the SAME
///   instance `ActivityScreen` drives, built once at `RootView` and
///   injected here — this screen used to build a SECOND, independent
///   instance purely for this, each with its own resting SSE connection;
///   see `RootView.activityStore`'s doc comment. This screen still
///   `activate(kind: .all)`s/`deactivate()`s it on its own appear/disappear
///   — reconfiguring the shared instance to its own needs is safe since
///   `ActivityScreen`/`OverviewScreen` are mutually exclusive routes.
///
/// **Range reactivity**: `model.range` already drives the toolbar's
/// segmented picker; this screen has no range selector of its own.
/// `.onChange(of: model.range)` re-fires `dashboardStore.setRange(_:)` (a
/// full per-host refetch) — the needs-you derivation needs no equivalent
/// push, since `deriveNeedsYou(rows:range:now:)` takes `range` as a plain
/// input and `NeedsYouCard` recomputes it fresh on every render.
///
/// **Per-block rendering** (spec §2's `BlockState` discipline): the host
/// freshness strip and needs-you queue each paint from their own store,
/// independent of the dashboard fetch. The four `MergedDashboard`-derived
/// blocks (instruments/charts/cycles/fleet) share one underlying fetch, so
/// they share one gate — `dashboardStore.merged` — rather than four
/// independent `BlockState`s: `merged` is genuinely `nil` (not "zero of
/// everything") until at least one host has answered `"ok"`, exactly the
/// state `DashboardStore.pageError` flags once every seeded host has
/// answered with none `"ok"`. A page-level failure note therefore renders
/// only in that state (nothing has resolved) — a `nil` `merged` before
/// `pageError` fires (still loading) shows a quiet spinner instead, same as
/// every other screen's initial-load state.
public struct OverviewScreen: View {
    @Bindable var model: AppModel
    let backend: BackendController

    @State private var dashboardStore: DashboardStore?

    /// Tracked so `activate()` rebuilds `dashboardStore` on a backend client
    /// swap (embedded/remote switch, reconnect, restart) — see that
    /// method's doc comment and `BackendController.clientIdentity()`.
    @State private var dashboardStoreClientID: ObjectIdentifier?

    /// The single shared instance `RootView` constructs and injects — see
    /// the type doc comment's "ActivityStore" bullet and `RootView.
    /// activityStore`'s own doc comment. `nil` only while the backend isn't
    /// connected yet (mirrors every other backend-dependent seam in this
    /// screen).
    let activityStore: ActivityStore?

    @AppStorage(OverviewWidgets.storageKey) private var widgetsData: Data = Data()

    /// Decode-cached mirror of `widgetsData` (Task 0 perf fix): the old
    /// `widgets` computed property re-ran `JSONDecoder` on every access —
    /// ~9× per `body` pass. `widgetsData` (the `@AppStorage`, which is also
    /// what `ShellToolbar`'s own `@AppStorage(OverviewWidgets.storageKey)`
    /// writes through — same key, so `UserDefaults`/`@AppStorage` itself
    /// carries that external write here) stays the single source of truth;
    /// this `@State` is just a cached decode of it, refreshed by
    /// `.onChange(of: widgetsData)` below. Seeded from `UserDefaults`
    /// directly in `init` (via the existing `OverviewWidgets.load(defaults:)`
    /// seam — review round 1 caught that seeding only in `.onAppear` let the
    /// very first `body` render for a returning operator flash the
    /// all-default `OverviewWidgets()` for one frame before `onAppear`
    /// corrected it); `.onAppear`/`.onChange` stay as the ongoing sync path
    /// for later writes (this screen's own re-appearance, or `ShellToolbar`'s
    /// Customize menu writing the same key from elsewhere).
    @State private var widgets: OverviewWidgets

    public init(model: AppModel, backend: BackendController, activityStore: ActivityStore?) {
        self.model = model
        self.backend = backend
        self.activityStore = activityStore
        _widgets = State(initialValue: OverviewWidgets.load(defaults: .standard))
    }

    public var body: some View {
        Group {
            if let dashboardStore, let activityStore {
                content(dashboardStore: dashboardStore, activityStore: activityStore)
            } else {
                notReadyView
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(Color.rupuBg)
        .onAppear { widgets = OverviewWidgets.decode(widgetsData) }
        .onChange(of: widgetsData) { _, newValue in
            widgets = OverviewWidgets.decode(newValue)
        }
        // Direct client-availability signal (perf & interaction arc, Plan 5
        // Task 2) — replaces the old `.task { }` + `.onChange(of: backend.
        // health)` cold-launch relay. `.overview` is `AppModel.route`'s
        // default and never persisted (see `AppModel.swift` — only
        // `range`/`scopeWsID`/`onboardingComplete` are), so this is
        // *always* the very first screen a launch renders — often before
        // `backend.client()` resolves. `backend.clientGeneration` bumps the
        // instant a client becomes usable, well before `backend.health`
        // reaches `.healthy` (see that property's doc comment on
        // `BackendController`) — waiting for the extra health round trip
        // bought this screen nothing but delay. `.task(id:)` fires on this
        // screen's first appearance (whatever the current generation is)
        // AND on every later bump (an embedded/remote switch, reconnect,
        // restart); `activate()`'s own client-identity rebuild makes a
        // repeat call idempotent (reuses `dashboardStore` unless the client
        // actually changed), so firing this on every bump — not just the
        // first — is safe.
        .task(id: backend.clientGeneration) {
            await activate()
        }
        .onChange(of: model.range) { _, newRange in
            // Fix round 1: a bare `Task { await dashboardStore?.setRange(...) }`
            // reads `dashboardStore` fresh when the task actually runs (it's
            // an `@State` box, not a frozen snapshot), so it isn't the race
            // — the race is a range change firing this right before the
            // screen disappears, where `onDisappear` below already
            // deactivated (and nil'd) the store by the time this task's
            // turn comes up. Capturing the store here and re-checking its
            // identity against the *current* `dashboardStore` right before
            // calling `setRange` makes a superseded task a no-op instead of
            // kicking off an untracked refetch on a store nothing will ever
            // deactivate again.
            let capturedStore = dashboardStore
            Task {
                guard let capturedStore, capturedStore === dashboardStore else { return }
                await capturedStore.setRange(newRange)
            }
        }
        .onDisappear {
            dashboardStore?.deactivate()
            // Cleared, not just deactivated: this is what the range-change
            // guard above checks, and it's what makes `activate()` rebuild
            // rather than silently no-op if this exact screen instance ever
            // reappears (see that method's `dashboardStore == nil` clause).
            dashboardStore = nil
            // `activityStore` is NOT owned here (see the type doc comment's
            // "ActivityStore" bullet) — deactivated (this screen is done
            // with it), but never nilled; it's `RootView`'s injected
            // reference, and the next appearance's `activate()` below
            // re-`activate(kind: .all)`s the SAME instance rather than
            // rebuilding it.
            activityStore?.deactivate()
        }
    }

    /// Builds/rebuilds `dashboardStore` lazily (once `backend.client()`
    /// exists — shouldn't stay `nil` by the time the shell can route here,
    /// same reasoning `ActivityScreen`/`RunDetailScreen` already document)
    /// and activates it, and (re)activates the SHARED `activityStore` — see
    /// the type doc comment's "ActivityStore" bullet — with THIS screen's
    /// own `kind: .all`, no scope. Reuses an existing `dashboardStore` on a
    /// later call rather than rebuilding — this screen has no per-instance
    /// identity (like `RunDetailScreen`'s `runID`) to rebuild around —
    /// UNLESS it's currently `nil` (nothing built yet, or `onDisappear`
    /// cleared a torn-down screen's store) or `backend.client()`'s identity
    /// has changed since it was built: an embedded/remote mode switch, a
    /// manual reconnect, or a restart all swap `backend.client()` to a
    /// brand-new `CPClient` directly (never through `nil` in between — see
    /// `BackendController.clientIdentity()`'s doc comment), so a plain "do I
    /// already have a store" check would never notice and would keep
    /// running it against the abandoned connection. `activityStore` needs
    /// no such rebuild check here — `RootView` already handles that.
    private func activate() async {
        guard let client = backend.client() else { return }
        let clientID = backend.clientIdentity()

        if dashboardStore == nil || dashboardStoreClientID != clientID {
            dashboardStore?.deactivate()
            dashboardStore = DashboardStore(client: client, signalsFactory: Self.makeSignalsFactory(backend: backend))
            dashboardStoreClientID = clientID
        }

        guard let dStore = dashboardStore else { return }
        async let dashboardActivation: Void = dStore.activate(range: model.range)
        async let activityActivation: Void? = activityStore?.activate(kind: .all)
        _ = await (dashboardActivation, activityActivation)
    }

    // MARK: - Layout

    /// The merged-gate block ids — everything in `OverviewWidgets.order`
    /// except `needsYou`, which has its own independent load path (see the
    /// type doc comment) and isn't part of `dashboardStore.merged`'s gate.
    static let mergedBlockIDs: Set<String> = ["instruments", "charts", "cycles", "fleet"]

    /// Order-driven rendering, pure half (Task 3): the four merged blocks'
    /// relative order, filtered out of the full persisted `order`. Visibility
    /// is applied separately in `mergedSection` — this only decides sequence.
    static func mergedBlockOrder(_ order: [String]) -> [String] {
        order.filter { mergedBlockIDs.contains($0) }
    }

    /// Order-driven rendering, pure half (Task 3): whether `needsYou` renders
    /// above the merged block group or below it. The merged group always
    /// renders as one contiguous unit (it shares one loading/error gate —
    /// see `mergedSection`'s doc comment), so full interleaving of `needsYou`
    /// between individual merged blocks isn't supported; this is the coarser
    /// two-position ordering that's actually representable without splitting
    /// `dashboardStore.merged` into four independent `BlockState`s, which is
    /// out of scope here. Compares `needsYou`'s index against the first
    /// merged id's index in `order`; either id missing from `order` (should
    /// never happen — `OverviewWidgets.normalized(_:)` guarantees the full
    /// canonical set) defaults to "needsYou first", matching `defaultOrder`.
    static func needsYouPrecedesMergedGroup(_ order: [String]) -> Bool {
        guard let needsYouIndex = order.firstIndex(of: "needsYou") else { return true }
        guard let firstMergedIndex = order.firstIndex(where: { mergedBlockIDs.contains($0) }) else { return true }
        return needsYouIndex < firstMergedIndex
    }

    private func content(dashboardStore: DashboardStore, activityStore: ActivityStore) -> some View {
        let needsYouFirst = Self.needsYouPrecedesMergedGroup(widgets.order)

        return ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                FreshnessStrip(slices: dashboardStore.hostStates)

                if needsYouFirst, widgets.needsYou {
                    NeedsYouCard(
                        store: activityStore,
                        backend: backend,
                        range: model.range,
                        onNavigate: { model.navigate(to: $0) }
                    )
                }

                mergedSection(dashboardStore: dashboardStore)

                if !needsYouFirst, widgets.needsYou {
                    NeedsYouCard(
                        store: activityStore,
                        backend: backend,
                        range: model.range,
                        onNavigate: { model.navigate(to: $0) }
                    )
                }
            }
            .padding(16)
            .frame(maxWidth: .infinity, alignment: .top)
        }
    }

    /// The four blocks that share `dashboardStore.merged` as their common
    /// gate — see the type's doc comment on why this is one shared
    /// three-state gate (loading / failed / content) rather than four
    /// independent ones. Internal sequence of the four (when all are
    /// visible and loaded) follows `mergedBlockOrder(_:)` — the Customize
    /// menu's reorder sheet.
    @ViewBuilder
    private func mergedSection(dashboardStore: DashboardStore) -> some View {
        let showAnyMergedWidget = widgets.instruments || widgets.charts || widgets.cycles || widgets.fleet
        if showAnyMergedWidget {
            if let merged = dashboardStore.merged {
                VStack(alignment: .leading, spacing: 16) {
                    ForEach(Self.mergedBlockOrder(widgets.order), id: \.self) { id in
                        mergedBlock(id: id, merged: merged)
                    }
                }
            } else if let pageError = dashboardStore.pageError {
                FailedBlock(subject: "dashboard", message: pageError, retry: { await activate() })
            } else {
                loadingBlock
            }
        }
    }

    /// Renders one merged-gate block by id, still gated by its own
    /// visibility toggle (`widgets.<field>`) — `mergedBlockOrder(_:)` only
    /// decides sequence, not whether a hidden block's id (still present in
    /// `order` — order and visibility are independent, same as before this
    /// task) renders at all. An id `mergedBlockIDs` doesn't recognize can't
    /// reach here (`mergedBlockOrder(_:)` already filtered to that set), so
    /// there's no "unknown id" fallback branch to omit silently.
    @ViewBuilder
    private func mergedBlock(id: String, merged: MergedDashboard) -> some View {
        switch id {
        case "instruments":
            if widgets.instruments {
                InstrumentStrip(merged: merged)
            }
        case "charts":
            if widgets.charts {
                chartsRow(merged: merged)
            }
        case "cycles":
            if widgets.cycles {
                CycleSummaryLine(cycles: merged.cycles, partial: merged.cyclesPartial)
            }
        case "fleet":
            if widgets.fleet {
                FleetStrip(fleet: merged.fleet, partial: merged.fleetPartial)
            }
        default:
            EmptyView()
        }
    }

    private func chartsRow(merged: MergedDashboard) -> some View {
        HStack(alignment: .top, spacing: 16) {
            OutcomesChart(buckets: merged.terminalBuckets)
                .frame(maxWidth: .infinity)
                .panelStyle(.panel)
            ThroughputChart(buckets: merged.throughputBuckets)
                .frame(maxWidth: .infinity)
                .panelStyle(.panel)
        }
    }

    private var loadingBlock: some View {
        HStack {
            Spacer(minLength: 0)
            ProgressView().controlSize(.small)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, minHeight: 80)
        .panelStyle(.panel)
    }

    /// Pre-store placeholder, chosen from `backend.health` rather than a
    /// single fixed string — a cold launch spends real, visible time with
    /// `dashboardStore`/`activityStore` still `nil` while `configureEmbedded`/
    /// `connectRemote` are attaching or spawning, and that is not the same
    /// state as a genuinely failed connection. "Backend not connected" is
    /// reserved for `.down`/`.incompatible` (health has actually resolved to
    /// a failure); every other health value (`.starting`, `.degraded` — a
    /// single flaky poll, not a verdict — and `.healthy` itself during the
    /// brief window before `activate()`'s async work lands) shows the
    /// standard connecting affordance instead, matching
    /// `LauncherSheet.connectingView`'s spinner + "Connecting" (`RupuLauncher/
    /// LauncherSheet.swift`) rather than this screen's own previously
    /// invented, permanently-wrong-looking copy.
    @ViewBuilder
    private var notReadyView: some View {
        switch backend.health {
        case .down, .incompatible:
            centeredLabel("Backend not connected")
        case .starting, .degraded, .healthy:
            connectingView
        }
    }

    private var connectingView: some View {
        VStack(spacing: 8) {
            ProgressView().controlSize(.small)
            Text("Connecting")
                .font(.noteText)
                .foregroundStyle(Color.rupuMute)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func centeredLabel(_ label: String) -> some View {
        VStack {
            Spacer(minLength: 0)
            Text(label).font(.noteText).foregroundStyle(Color.rupuMute)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    // MARK: - Firehose plumbing

    /// Builds `dashboardStore`'s own independent firehose connection via
    /// `backend.makeFirehoseStream(onConnectionChange:)` — `backend.
    /// eventStream()`'s `onConnectionChange` slot is already claimed by
    /// `RootView`, so this consumer needs its own connection paired
    /// honestly with its own connection-state callback, same rationale
    /// `RootView.makeActivitySignalsFactory` documents for the shared
    /// `ActivityStore`'s equivalent connection (that store no longer builds
    /// its firehose connection here — see the type doc comment's
    /// "ActivityStore" bullet — so this helper now backs `dashboardStore`
    /// alone).
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
