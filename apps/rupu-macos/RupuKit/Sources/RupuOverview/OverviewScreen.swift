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
/// any transient re-render) is enough:
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
    let backend: BackendController

    @State private var dashboardStore: DashboardStore?
    @State private var activityStore: ActivityStore?

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
                centeredLabel("Backend not connected")
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(Color.rupuBg)
        .task {
            await activate()
        }
        .onChange(of: model.range) { _, newRange in
            Task { await dashboardStore?.setRange(newRange) }
        }
        .onDisappear {
            dashboardStore?.deactivate()
            activityStore?.deactivate()
        }
    }

    /// Builds both stores lazily (once `backend.client()` exists — shouldn't
    /// stay `nil` by the time the shell can route here, same reasoning
    /// `ActivityScreen`/`RunDetailScreen` already document) and activates
    /// them. Reuses an existing pair on a second call rather than rebuilding
    /// — this screen has no changing identity to rebuild around, unlike
    /// `RunDetailScreen`'s per-`runID` store.
    private func activate() async {
        guard let client = backend.client() else { return }

        let dStore: DashboardStore
        if let existing = dashboardStore {
            dStore = existing
        } else {
            let newStore = DashboardStore(client: client, signalsFactory: Self.makeSignalsFactory(backend: backend))
            dashboardStore = newStore
            dStore = newStore
        }

        let aStore: ActivityStore
        if let existing = activityStore {
            aStore = existing
        } else {
            let newStore = ActivityStore(
                client: client,
                signalsFactory: Self.makeSignalsFactory(backend: backend),
                pendingActions: backend.pendingActions
            )
            activityStore = newStore
            aStore = newStore
        }

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
