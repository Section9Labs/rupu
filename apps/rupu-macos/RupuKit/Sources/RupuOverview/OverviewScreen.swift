import Foundation
import RupuAPI
import RupuDesign
import RupuStore
import SwiftUI

// MARK: - Widget visibility persistence

/// The Overview screen's block-visibility toggles (spec §4, "Customize &
/// persistence" — visibility persistence now, layout editing later; see
/// `docs/superpowers/specs/2026-08-24-rupu-macos-phase-4-dashboard-design.md`).
/// A plain `Codable`/`Equatable` struct, not a `View` member or an
/// `@Observable` store — CI rule: only a test touching a `View`-type member
/// needs `@MainActor`, and this stays testable without it.
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

    public static let storageKey = "overview.widgets"

    public init(
        needsYou: Bool = true,
        instruments: Bool = true,
        charts: Bool = true,
        cycles: Bool = true,
        fleet: Bool = true
    ) {
        self.needsYou = needsYou
        self.instruments = instruments
        self.charts = charts
        self.cycles = cycles
        self.fleet = fleet
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
/// Phase 1 `PlaceholderScreen` for `.overview`): host freshness strip →
/// needs-you queue → instrument strip → charts row → cycle summary line →
/// fleet strip, in one vertical `ScrollView` stack.
///
/// Owns two independent store lifecycles, mirroring `RunDetailScreen`'s
/// pattern (build lazily once `backend.client()` exists, activate on
/// appear, deactivate `.onDisappear`) rather than `ActivityScreen`'s
/// `.task(id:)`-per-kind shape — this screen has no per-instance identity
/// that changes while mounted, so a single `.task` (built once, reused on
/// any transient re-render) is enough, EXCEPT that `activate()` still
/// rebuilds both stores if `backend.client()`'s identity has changed since
/// they were built (an embedded/remote switch, reconnect, or restart) —
/// see `activate()`'s doc comment:
/// - `DashboardStore` — the merged fleet aggregate behind blocks 3-6.
/// - `ActivityStore` (kind `.all`, `liveTail` on by default, `scopeFilter`
///   **never set** — see `NeedsYouCard`'s doc comment: the needs-you queue
///   is fleet-wide by design, deliberately blind to the top bar's project
///   scope) — feeds the needs-you queue alone.
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
    /// `@Bindable`, not a plain `let`, so `.onChange(of: backend.health)`
    /// below observes it the same way `RootView`/`OnboardingView` already
    /// do — see the cold-launch fix note on `body`.
    @Bindable var backend: BackendController

    @State private var dashboardStore: DashboardStore?
    @State private var activityStore: ActivityStore?

    /// Tracked so `activate()` rebuilds both stores on a backend client swap
    /// (embedded/remote switch, reconnect, restart) — see that method's doc
    /// comment and `BackendController.clientIdentity()`.
    @State private var storeClientID: ObjectIdentifier?

    @AppStorage(OverviewWidgets.storageKey) private var widgetsData: Data = Data()

    public init(model: AppModel, backend: BackendController) {
        self.model = model
        self.backend = backend
    }

    private var widgets: OverviewWidgets {
        OverviewWidgets.decode(widgetsData)
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
        .task {
            await activate()
        }
        // Cold-launch fix: `.overview` is `AppModel.route`'s default and
        // never persisted (see `AppModel.swift` — only `range`/`scopeWsID`/
        // `onboardingComplete` are), so this is *always* the very first
        // screen a launch renders — before onboarding has necessarily
        // finished attaching/spawning the embedded server. The `.task`
        // above fires immediately on that first render, while
        // `backend.health` is still `.starting` and `backend.client()` is
        // `nil` — `activate()` early-returns, and with no *second* trigger,
        // this screen was stuck showing `notReadyView` forever even after
        // health reached `.healthy` and the toolbar/footer went live
        // (`ActivityScreen`/`RunDetailScreen` never surfaced this because
        // neither can be the cold-launch screen — see their own doc
        // comments — so their `.task` always ran again on a subsequent,
        // already-connected navigation). Re-running `activate()` on every
        // transition to `.healthy` closes the gap; `activate()`'s
        // client-identity rebuild (fix round 1) already makes a repeat call
        // idempotent (reuses the existing pair unless the client actually
        // changed), so firing this on every healthy transition — not just
        // the first — is safe.
        .onChange(of: backend.health) { _, newHealth in
            guard case .healthy = newHealth else { return }
            Task { await activate() }
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
            activityStore?.deactivate()
            // Cleared, not just deactivated: this is what the range-change
            // guard above checks, and it's what makes `activate()` rebuild
            // rather than silently no-op if this exact screen instance ever
            // reappears (see that method's `dashboardStore == nil` clause).
            dashboardStore = nil
            activityStore = nil
        }
    }

    /// Builds both stores lazily (once `backend.client()` exists — shouldn't
    /// stay `nil` by the time the shell can route here, same reasoning
    /// `ActivityScreen`/`RunDetailScreen` already document) and activates
    /// them. Reuses an existing pair on a later call rather than rebuilding
    /// — this screen has no per-instance identity (like `RunDetailScreen`'s
    /// `runID`) to rebuild around — UNLESS either store is currently `nil`
    /// (nothing built yet, or `onDisappear` cleared a torn-down screen's
    /// pair) or `backend.client()`'s identity has changed since the current
    /// pair was built: an embedded/remote mode switch, a manual reconnect,
    /// or a restart all swap `backend.client()` to a brand-new `CPClient`
    /// directly (never through `nil` in between — see
    /// `BackendController.clientIdentity()`'s doc comment), so a plain "do I
    /// already have a pair" check would never notice and would keep running
    /// both stores against the abandoned connection.
    private func activate() async {
        guard let client = backend.client() else { return }
        let clientID = backend.clientIdentity()

        if dashboardStore == nil || activityStore == nil || storeClientID != clientID {
            dashboardStore?.deactivate()
            activityStore?.deactivate()
            dashboardStore = DashboardStore(client: client, signalsFactory: Self.makeSignalsFactory(backend: backend))
            activityStore = ActivityStore(
                client: client,
                signalsFactory: Self.makeSignalsFactory(backend: backend),
                pendingActions: backend.pendingActions
            )
            storeClientID = clientID
        }

        guard let dStore = dashboardStore, let aStore = activityStore else { return }
        async let dashboardActivation: Void = dStore.activate(range: model.range)
        async let activityActivation: Void = aStore.activate(kind: .all)
        _ = await (dashboardActivation, activityActivation)
    }

    // MARK: - Layout

    private func content(dashboardStore: DashboardStore, activityStore: ActivityStore) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                FreshnessStrip(slices: dashboardStore.hostStates)

                if widgets.needsYou {
                    NeedsYouCard(
                        store: activityStore,
                        backend: backend,
                        range: model.range,
                        onNavigate: { model.navigate(to: $0) }
                    )
                }

                mergedSection(dashboardStore: dashboardStore)
            }
            .padding(16)
            .frame(maxWidth: .infinity, alignment: .top)
        }
    }

    /// The four blocks that share `dashboardStore.merged` as their common
    /// gate — see the type's doc comment on why this is one shared
    /// three-state gate (loading / failed / content) rather than four
    /// independent ones.
    @ViewBuilder
    private func mergedSection(dashboardStore: DashboardStore) -> some View {
        let showAnyMergedWidget = widgets.instruments || widgets.charts || widgets.cycles || widgets.fleet
        if showAnyMergedWidget {
            if let merged = dashboardStore.merged {
                VStack(alignment: .leading, spacing: 16) {
                    if widgets.instruments {
                        InstrumentStrip(merged: merged)
                    }
                    if widgets.charts {
                        chartsRow(merged: merged)
                    }
                    if widgets.cycles {
                        CycleSummaryLine(cycles: merged.cycles, partial: merged.cyclesPartial)
                    }
                    if widgets.fleet {
                        FleetStrip(fleet: merged.fleet, partial: merged.fleetPartial)
                    }
                }
            } else if let pageError = dashboardStore.pageError {
                TintBanner(tone: Color.status(.failed), toneBg: Color.status(.failed).opacity(0.08)) {
                    VStack(alignment: .leading, spacing: 4) {
                        Text("Failed to load dashboard")
                            .font(.noteText.weight(.semibold))
                            .foregroundStyle(Color.status(.failed))
                        Text(pageError)
                            .font(.noteText)
                            .foregroundStyle(Color.status(.failed))
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            } else {
                loadingBlock
            }
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

    /// Duplicated, deliberately, from `ActivityScreen.makeSignalsFactory`
    /// (private to that type, so not a shared symbol to begin with) —
    /// builds this screen's own independent firehose connection via
    /// `backend.makeFirehoseStream(onConnectionChange:)`, same rationale as
    /// that type's doc comment: `backend.eventStream()`'s
    /// `onConnectionChange` slot is already claimed by `RootView`, so any
    /// second consumer (here, feeding both `DashboardStore` and this
    /// screen's own `ActivityStore`) needs its own connection paired
    /// honestly with its own connection-state callback. Both stores call
    /// this same static helper — each gets its OWN stream instance (the
    /// closure is re-invoked per store, not shared), matching
    /// `ActivityScreen`'s "captures only `backend`" `@Sendable` shape.
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
