import Foundation
import Observation
import RupuAPI
import RupuStore

/// The `MenuBarExtra`'s live-attention data source: **local-host-only**
/// active counts (running/awaiting/paused/pending, from a `host: "local"`
/// `GET /api/dashboard`'s `active` field) plus a top-5 needs-you triage
/// list derived from a `host: "local"` `GET /api/runs`, both refreshed on
/// the same 60s poll-loop idiom `HostsFooterStore` established — a
/// `Task`-based loop guarded by `task == nil` for idempotence, cancelled
/// and nilled out on `deactivate()`. Review fix (round 1): this store's own
/// doc comments previously (wrongly) called this data "fleet-wide" — see
/// the "Local-only, never fleet-wide" section below and `MenuBarView`'s
/// in-app disclosure (the footer caption + stat-tile `.help` tooltips) for
/// where that scope limitation is now surfaced honestly to the operator.
///
/// **App-level lifetime, deliberately** (see `RupuApp.swift`'s activation
/// wiring, alongside `RunNotifier`'s): this store activates once, from the
/// same backend-healthy `.onChange(of: backend.health)` seam `RunNotifier`
/// already uses — **not** from the `MenuBarExtra` scene's own appear/
/// disappear. The attention dot overlaid on the menu-bar label has to have
/// live data available even while the popover itself is closed (that is
/// the entire point of a menu-bar attention indicator) — a popover-scoped
/// activation would leave the dot stuck at whatever it last showed
/// whenever the operator hasn't opened the menu recently, which defeats
/// the feature. One lightweight, local-only (`host: "local"`) 60s poll for
/// the app's whole lifetime is an accepted, deliberate cost of that
/// requirement, not an oversight.
///
/// **Local-only, never fleet-wide** (matches `HostsFooterStore`/
/// `ActivityStore`'s own `host: "local"` calls): both `dashboard(range:
/// host:)` and `runs(offset:limit:host:)` are called with an explicit
/// `host: "local"`, never an omitted `host:` — see `CPClient.runs(offset:
/// limit:host:)`'s doc comment on the multi-second fan-out cost an omitted
/// `host` pays across a fleet with a slow or offline node. A menu-bar
/// glance is not the place to pay that cost every 60s for the app's entire
/// runtime.
@MainActor
@Observable
public final class MenuBarStore {
    /// `nil` until the first successful poll lands — the view renders every
    /// stat tile as `—` for that pre-first-poll window rather than a
    /// misleading `0`.
    public private(set) var counts: APIActiveCounts?
    /// Always the menu bar's own top-5 (gates first, per `deriveNeedsYou`'s
    /// ordering) — see `apply(counts:rows:now:)`'s doc comment for how this
    /// differs from `deriveNeedsYou`'s own cap of 6.
    public private(set) var needsYou: [NeedsYouItem] = []
    /// Everything that didn't make the top-5 — `deriveNeedsYou`'s own
    /// overflow (beyond its cap of 6) plus whatever this store additionally
    /// trimmed getting from 6 down to 5. Backs the "N more in rupu" footer
    /// line.
    public private(set) var overflow: Int = 0

    private var client: CPClient?
    private var task: Task<Void, Never>?
    private let pollInterval: Duration

    private static let runsPageSize = 50
    private static let menuBarCap = 5
    private static let localHost = "local"

    /// The widest available `TimeRange` — `.all` imposes no recency window
    /// at all (see `deriveNeedsYou`'s `fallsInsideRange` doc comment: `.all`
    /// "imposes no window at all — every failed row passes"). A menu-bar
    /// triage list has no range picker of its own (unlike the Overview
    /// screen's `NeedsYouCard`, which reads `model.range`) to make this
    /// choice contextually, so the widest, least-surprising option wins: a
    /// failed run should never silently vanish from the menu bar's queue
    /// just because it happened more than 7/30 days ago. Gates (`.awaiting`
    /// rows) are unaffected either way — `deriveNeedsYou` never
    /// time-windows those.
    private static let needsYouRange: TimeRange = .all

    /// `pollInterval` defaults to the real 60s cadence; tests inject a
    /// shorter one (same seam `BackendController.healthInterval` and
    /// `DashboardStore.reconcileInterval` already use for the identical
    /// reason) so a poll-loop test doesn't have to wait a full minute for a
    /// second tick.
    public init(pollInterval: Duration = .seconds(60)) {
        self.pollInterval = pollInterval
    }

    /// Pure seam (Step 1's tested half — no I/O): stores `counts` verbatim,
    /// derives the local-host needs-you queue (`rows` is always this
    /// store's own local-only `GET /api/runs` fetch — never fleet-wide, see
    /// this type's doc comment) via `deriveNeedsYou(rows:range:now:)`
    /// (capped at 6 there), then trims to this store's own top-5 cap for
    /// the menu bar's compact list.
    ///
    /// **Overflow composition**: `deriveNeedsYou`'s own `overflow` already
    /// counts everything beyond its cap of 6; this adds whatever this store
    /// additionally trims getting from (at most) 6 down to 5, so the "N
    /// more in rupu" footer line stays honest about the true total left
    /// out, not just what `deriveNeedsYou` itself left out.
    public func apply(counts: APIActiveCounts, rows: [ActivityRow], now: Date) {
        self.counts = counts
        let derived = deriveNeedsYou(rows: rows, range: Self.needsYouRange, now: now)
        needsYou = Array(derived.items.prefix(Self.menuBarCap))
        overflow = derived.overflow + max(0, derived.items.count - Self.menuBarCap)
    }

    /// Idempotent — same idiom as `HostsFooterStore.activate(client:)`: a
    /// second call while already polling just updates `client` for the next
    /// tick rather than spawning a second loop. The loop polls immediately
    /// on activation (matching `HostsFooterStore`'s own shape), then every
    /// `pollInterval` thereafter, until `deactivate()` cancels it.
    public func activate(client: CPClient) {
        self.client = client
        guard task == nil else { return }
        task = Task { [weak self, pollInterval] in
            while !Task.isCancelled {
                guard let self else { return }
                await self.pollOnce()
                try? await Task.sleep(for: pollInterval)
            }
        }
    }

    public func deactivate() {
        task?.cancel()
        task = nil
        client = nil
    }

    /// Review fix (round 1): triggers one immediate poll outside the normal
    /// `pollInterval` cadence — `MenuBarGateActions` calls this right after
    /// a gate approve/reject POST succeeds, mirroring `ActivityStore`'s own
    /// post-mutation `scheduleDebouncedRefresh()`. Shrinks this store's own
    /// confirmation latency from up to a full `pollInterval` (60s in
    /// production) down to roughly one request round trip — the gate row
    /// disappears from `needsYou` (once the server no longer reports the
    /// run as `.awaiting`) almost immediately instead of on the next
    /// scheduled tick. A no-op if the store was never `activate(client:)`d
    /// — matches `pollOnce`'s own `guard let client` behavior — and, like
    /// every tick, all-or-nothing and silent on failure.
    public func refreshNow() async {
        await pollOnce()
    }

    /// One tick: fetches `dashboard(range:host:)` and `runs(offset:limit:
    /// host:)` — both local-only — and, only if BOTH succeed, maps the run
    /// rows through `ActivityRow.init(_:APIRunListRow)` (the exact mapping
    /// `ActivityStore` uses for its own workflow-run source — reused here,
    /// not duplicated) and calls `apply`. A failure on either request is
    /// silently swallowed, leaving `counts`/`needsYou`/`overflow` exactly as
    /// they were — same "keep last good data, never blank on a hiccup"
    /// contract `HostsFooterStore.pollOnce` follows. Deliberately
    /// all-or-nothing rather than applying whichever request happened to
    /// succeed: a `counts` update with a stale `needsYou` (or vice versa)
    /// would silently disagree with itself for up to a full `pollInterval`.
    private func pollOnce() async {
        guard let client else { return }
        async let dashboardResult = try? client.dashboard(range: Self.needsYouRange.rawValue, host: Self.localHost)
        async let runsResult = try? client.runs(offset: 0, limit: Self.runsPageSize, host: Self.localHost)
        guard let dashboard = await dashboardResult, let runRows = await runsResult else { return }
        let rows = runRows.map(ActivityRow.init)
        apply(counts: dashboard.active, rows: rows, now: Date())
    }
}
