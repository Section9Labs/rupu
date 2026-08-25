import AppKit
import RupuAPI
import RupuDesign
import RupuOverview
import RupuStore
import RupuUsageKit
import SwiftUI

/// The Usage screen (Phase 5B, Task 6), replacing the `.usage` placeholder —
/// the app's last remaining one. Vertical stack, per spec §4/brief, mapped
/// 1:1 onto `UsageStore`'s three independent `BlockState`s (same "one gate
/// per block, never a merged fourth" discipline `OverviewScreen`'s own doc
/// comment documents for its `merged` gate — here there is no merge at all,
/// just three parallel sections):
/// - `usage` (`GET /api/usage`, fleet-wide, host-fanned-out) gates the
///   unpriced banner (shown only when non-empty) + `FreshnessStrip` (REUSED
///   from Phase 4, over an `APIHostFreshness -> HostSlice` adapter —
///   `usageHostSlice(_:)` below) + a small headline spend/token/run line.
/// - `usageRuns` (`GET /api/usage/runs`, local-only) gates the pivot picker
///   + `SpendChart` + `BreakdownTable` — both built from the SAME rows (see
///   `BreakdownTable`'s doc comment for why that matters).
/// - `outliers` (`GET /api/usage/outliers`, local-only) gates `OutlierPanel`
///   alone.
///
/// Owns a `UsageStore` lifecycle via the same lazy-build/`storeClientID`-
/// rebuild convention `SecurityScreen`/`ProjectsScreen`/`FleetScreen`/
/// `LibraryScreen` already established.
///
/// **Does NOT need `OverviewScreen`'s cold-launch `.onChange(of: backend.
/// health)` fix** — same reasoning `SecurityScreen`'s own doc comment gives
/// for itself: `.usage` is only ever reached by a sidebar click, well after
/// the shell's own connection attempt has already resolved (only `.overview`,
/// `AppModel.route`'s default and never-persisted starting route, can be the
/// cold-launch screen).
///
/// **No `.onDisappear`/`deactivate()`** — same "one-shot `activate()`,
/// nothing running in the background to tear down" reasoning `SecurityScreen`
/// documents for itself: `UsageStore` has no reconcile loop either (see its
/// own doc comment's "NO reconcile loop" section) — unlike `OverviewScreen`'s
/// `DashboardStore`/`ActivityStore` pair, which DO run background work an
/// `onDisappear` has to cancel.
///
/// **Range reactivity DOES still need `OverviewScreen`'s guard**, despite
/// the point above — a `TimeRange` change is a distinct trigger from
/// screen-teardown, and can land mid-flight while this exact screen instance
/// is still showing (the toolbar's range picker is always visible while
/// `.usage` is current). `.onChange(of: model.range)` below re-fires
/// `UsageStore.setRange(_:)`, guarded with the identical captured-store-
/// identity check `OverviewScreen`'s own `.onChange(of: model.range)` uses
/// (see that handler's doc comment for the exact race it closes) — a
/// superseded task becomes a no-op instead of an untracked refetch on a
/// store nothing will ever `deactivate()`.
///
/// **The pivot picker calls `UsageStore.setPivot(_:)` on every change**,
/// even though this screen never reads the `usage` block's own `breakdown`
/// field (`BreakdownTable`'s doc comment explains why: the table/chart pivot
/// entirely client-side, from `usageRuns`). `setPivot` still refetches
/// `usage` under the new `group_by` regardless — matching the web's OWN
/// `/usage` page (its `getUsage` effect also depends on `[pivot]`, and its
/// `data.breakdown` field goes unused for the identical reason: `Usage.tsx`
/// builds its own table from `aggregateRuns(runs, pivot)`, not from
/// `data.breakdown`) — and honors `UsageStore`'s own designed contract:
/// Task 5 built `setPivot`'s dedicated `usageGeneration` counter (split from
/// `generation`, which guards `usageRuns`/`outliers`) specifically so a
/// pivot change could refetch `usage` alone without stranding the other two
/// blocks — leaving that call unmade would strand a fully-built, fully-
/// tested mechanism as dead code for no benefit.
public struct UsageScreen: View {
    @Bindable var model: AppModel
    let backend: BackendController

    @State private var store: UsageStore?
    @State private var storeClientID: ObjectIdentifier?
    @State private var breakdownSort = ListSort<BreakdownSortKey>(key: .cost, ascending: false)

    public init(model: AppModel, backend: BackendController) {
        self.model = model
        self.backend = backend
    }

    public var body: some View {
        Group {
            if let store {
                content(store: store)
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
            let capturedStore = store
            Task {
                guard let capturedStore, capturedStore === store else { return }
                await capturedStore.setRange(newRange)
            }
        }
    }

    /// Builds (or rebuilds, on a backend client swap) the store and
    /// activates it — same `storeClientID` recipe `SecurityScreen`/
    /// `ProjectsScreen`/`FleetScreen`/`LibraryScreen` use (see
    /// `BackendController.clientIdentity()`'s doc comment).
    private func activate() async {
        guard let client = backend.client() else { return }
        let clientID = backend.clientIdentity()
        if store == nil || storeClientID != clientID {
            store = UsageStore(client: client)
            storeClientID = clientID
        }
        guard let store else { return }
        await store.activate(range: model.range)
    }

    /// `UsageStore.windowBounds(for:now:)` — the SAME bounds `UsageStore`'s
    /// own `usageRuns` fetch used for this render's data, so `SpendChart`'s
    /// gap-filled buckets cover exactly the window that data was fetched
    /// for. Recomputed per render rather than cached: cheap (pure arithmetic
    /// over `Date()`), and `windowBounds`'s own doc comment is explicit that
    /// its default-`now` convenience is deliberately NOT memoized — a fresh
    /// `Date()` on every call. A few-millisecond skew between this and the
    /// store's own fetch-time `now` only ever WIDENS the chart's `until` by
    /// that same handful of milliseconds — never a mismatch a viewer could
    /// notice.
    private var windowBounds: (since: Date, until: Date) {
        UsageStore.windowBounds(for: model.range)
    }

    // MARK: - Layout

    private func content(store: UsageStore) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                usageSection(store: store)
                usageRunsSection(store: store)
                outlierSection(store: store)
            }
            .padding(16)
            .frame(maxWidth: .infinity, alignment: .top)
        }
    }

    // MARK: - `usage` block: unpriced banner + freshness strip + headline

    @ViewBuilder
    private func usageSection(store: UsageStore) -> some View {
        switch store.usage {
        case .loading:
            usageLoadingBlock()
        case .failed(let message):
            usageFailedBlock(message, subject: "usage")
        case .empty:
            // Never actually produced — `UsageStore.loadUsage` always
            // resolves `.content` on success (`GET /api/usage` always
            // returns a summary, even an all-zero one; see that store's own
            // field doc comment). Handled anyway so this `switch` stays
            // exhaustive rather than silently dropping a case the store's
            // contract promises never fires.
            EmptyView()
        case .content(let response):
            VStack(alignment: .leading, spacing: 12) {
                if !response.unpriced.models.isEmpty {
                    UnpricedBanner(unpriced: response.unpriced)
                }
                FreshnessStrip(slices: response.hosts.map(usageHostSlice))
                headlineLine(response.summary)
            }
        }
    }

    private func headlineLine(_ summary: APIUsageSummary) -> some View {
        HStack(spacing: 6) {
            Text(Fmt.cost(summary.costUSD))
                .font(.dataMono(15))
                .foregroundStyle(Color.rupuInk)
            Text("·").font(.noteText).foregroundStyle(Color.rupuMute)
            Text("\(Fmt.count(Int(summary.totalTokens))) tokens")
                .font(.noteText)
                .foregroundStyle(Color.rupuDim)
            Text("·").font(.noteText).foregroundStyle(Color.rupuMute)
            Text("\(Fmt.count(Int(summary.runs))) runs")
                .font(.noteText)
                .foregroundStyle(Color.rupuDim)
            if !summary.priced {
                Text("· partial")
                    .font(.noteText)
                    .foregroundStyle(Color.status(.awaiting))
            }
        }
    }

    // MARK: - `usageRuns` block: pivot picker + spend chart + breakdown table

    @ViewBuilder
    private func usageRunsSection(store: UsageStore) -> some View {
        switch store.usageRuns {
        case .loading:
            usageLoadingBlock()
        case .failed(let message):
            usageFailedBlock(message, subject: "usage runs")
        case .empty:
            usageRunsContent(store: store, rows: [])
        case .content(let rows):
            usageRunsContent(store: store, rows: rows)
        }
    }

    private func usageRunsContent(store: UsageStore, rows: [APIUsageRunRow]) -> some View {
        let bounds = windowBounds
        let buckets = buildSpendTimeline(rows: rows, since: bounds.since, until: bounds.until)
        return VStack(alignment: .leading, spacing: 12) {
            pivotPicker(store: store)
            SpendChart(buckets: buckets, pivot: store.pivot)
                .padding(12)
                .panelStyle(.panel)
            VStack(alignment: .leading, spacing: 8) {
                Eyebrow("Breakdown by \(pivotTitle(store.pivot))")
                BreakdownTable(rows: rows, pivot: store.pivot, sort: $breakdownSort)
            }
        }
    }

    private func pivotPicker(store: UsageStore) -> some View {
        Picker("Pivot", selection: pivotBinding(store: store)) {
            ForEach(UsagePivot.allCases, id: \.self) { pivot in
                Text(pivotTitle(pivot)).tag(pivot)
            }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .frame(maxWidth: 440)
    }

    /// A `Binding` over `store.pivot` — reads the store's own source of
    /// truth, writes through `UsageStore.setPivot(_:)` (async, so the write
    /// side dispatches a `Task`) rather than shadowing it with a second,
    /// screen-local `@State` the two could drift out of sync with.
    private func pivotBinding(store: UsageStore) -> Binding<UsagePivot> {
        Binding(
            get: { store.pivot },
            set: { newValue in
                Task { await store.setPivot(newValue) }
            }
        )
    }

    // MARK: - `outliers` block

    private func outlierSection(store: UsageStore) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Eyebrow("Cost outliers")
            switch store.outliers {
            case .loading:
                usageLoadingBlock()
            case .failed(let message):
                usageFailedBlock(message, subject: "outliers")
            case .empty:
                OutlierPanel(outliers: [], onSelect: { model.navigate(to: $0) })
            case .content(let rows):
                OutlierPanel(outliers: rows, onSelect: { model.navigate(to: $0) })
            }
        }
    }

    // MARK: - Chrome

    private func centeredLabel(_ label: String) -> some View {
        VStack {
            Spacer(minLength: 0)
            Text(label).font(.noteText).foregroundStyle(Color.rupuMute)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

// MARK: - Host freshness adapter

/// Adapts a server-resolved `APIHostFreshness` (`GET /api/usage`'s `hosts`
/// field) into the `HostSlice` shape `RupuOverview.FreshnessStrip` renders —
/// REUSING that view (brief: "REUSE Phase 4's `FreshnessStrip`") rather than
/// forking a parallel one, at the cost of this one small adapter.
///
/// `FreshnessStrip`'s `HostSlice.state` was designed for `DashboardStore`'s
/// own CLIENT-SIDE per-host fetch progress (`.loading` while a fetch is
/// still in flight, resolving to `.ok`/`.offline`/`.unavailable` once it
/// completes). `APIHostFreshness` carries no such notion — it's already a
/// SERVER-RESOLVED terminal value (`GET /api/usage`'s host fan-out has
/// already happened by the time this response exists) — so `.loading` is
/// never produced here; every `APIHostFreshness.state` maps directly to its
/// `SliceState` counterpart instead. The three wire values handled
/// (`"ok"`/`"offline"`/`"unavailable"`) are the exact three
/// `HostFreshness.state` emits on the Rust side (`crates/rupu-cp/src/api/
/// usage.rs`, the same three-value contract `dashboard.rs`'s identical field
/// already established for Phase 4); an unrecognized future value falls
/// back to `.unavailable(reason:)` carrying the raw wire string as the
/// reason, rather than silently misreading it as `.offline` — an honest
/// "don't know what this means yet" over a guessed tone.
func usageHostSlice(_ freshness: APIHostFreshness) -> HostSlice {
    let state: SliceState
    switch freshness.state {
    case "ok":
        state = .ok(capturedAt: freshness.capturedAt)
    case "offline":
        state = .offline
    case "unavailable":
        state = .unavailable(reason: freshness.reason)
    default:
        state = .unavailable(reason: freshness.state)
    }
    return HostSlice(id: freshness.hostID, name: freshness.name, transportKind: freshness.transportKind, state: state)
}

// MARK: - Shared block chrome (used by this file + `BreakdownTable.swift`/
// `OutlierPanel.swift` — reproduced here since this screen is split across
// files, same "hoisted, not per-file duplicated" reasoning `SecurityScreen`
// gives for its own `securityLoadingBlock`/`securityEmptyBlock`/
// `securityFailedBlock` trio.)

@MainActor
func usageLoadingBlock() -> some View {
    HStack {
        Spacer(minLength: 0)
        ProgressView().controlSize(.small)
        Spacer(minLength: 0)
    }
    .frame(maxWidth: .infinity, minHeight: 120)
    .panelStyle(.panel)
}

@MainActor
func usageFailedBlock(_ message: String, subject: String) -> some View {
    TintBanner(tone: Color.status(.failed), toneBg: Color.status(.failed).opacity(0.08)) {
        VStack(alignment: .leading, spacing: 4) {
            Text("Failed to load \(subject)")
                .font(.noteText.weight(.semibold))
                .foregroundStyle(Color.status(.failed))
            Text(message)
                .font(.noteText)
                .foregroundStyle(Color.status(.failed))
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}
