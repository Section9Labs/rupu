import SwiftUI
import RupuAPI
import RupuBackend
import RupuStore
import RupuDesign

/// The `MenuBarExtra` scene's own label content. There is no dedicated
/// app-icon/asset catalog yet (`apps/rupu-macos/App` has none — icon work is
/// deferred to Phase 7's signing/notarization pass), so this renders the
/// same "rupu" string `Sidebar.brandHeader` uses, just sized for the
/// status bar — a status-bar label is rendered at menu-bar height by AppKit,
/// not at `leadText`'s 13pt, so this is its own `Font.system(size:weight:)`
/// call, not literally `Sidebar.brandHeader` reused (review fix, round 1:
/// the prior wording overclaimed "reuses the exact wordmark"). A trailing
/// dot — the `.awaiting` status tone, matching the gate rows it's warning
/// about — appears inline (not overlaid/offset past the label's own layout
/// bounds, which risked clipping by the status-bar's own rendering — review
/// fix, round 1) when at least one run is awaiting approval **on the local
/// host** (see `MenuBarStore`'s doc comment on why this is local-only, never
/// fleet-wide). `hasAttention` is read from `MenuBarStore.counts` by the
/// caller (`RupuApp`'s `MenuBarExtra` label closure); this view has no store
/// dependency of its own so it stays a plain, easily-previewed leaf.
public struct MenuBarStatusLabel: View {
    let hasAttention: Bool

    public init(hasAttention: Bool) {
        self.hasAttention = hasAttention
    }

    public var body: some View {
        HStack(spacing: 4) {
            Text("rupu")
                .font(.system(size: 12, weight: .semibold))
            if hasAttention {
                Circle()
                    .fill(Color.status(.awaiting))
                    .frame(width: 6, height: 6)
            }
        }
    }
}

/// The `MenuBarExtra`'s popover content (`.menuBarExtraStyle(.window)`):
/// four live stat tiles, a top-5 needs-you triage list with inline gate
/// actions, and a footer (Open rupu / New run / Settings…). Every count and
/// row shown here is **local-host only** — the footer caption and stat-tile
/// tooltips say so explicitly (review fix, round 1: earlier drafts of this
/// module's doc comments wrongly called the data "fleet-wide"; see
/// `MenuBarStore`'s own doc comment for the full local-only rationale).
///
/// `store` is expected to already be activated at the app level (see
/// `MenuBarStore`'s own doc comment on why) — this view only ever reads
/// it, never calls `activate`/`deactivate` itself, so opening/closing the
/// popover has no effect on the poll loop.
public struct MenuBarView: View {
    let store: MenuBarStore
    let model: AppModel
    let backend: BackendController
    let openMainWindow: () -> Void

    public init(store: MenuBarStore, model: AppModel, backend: BackendController, openMainWindow: @escaping () -> Void) {
        self.store = store
        self.model = model
        self.backend = backend
        self.openMainWindow = openMainWindow
    }

    /// Final-review fix (M4): `MenuBarStore.pollOnce` deliberately keeps
    /// its last good data when a poll fails ("never blank on a hiccup"), so
    /// with the backend actually down this popover renders a full set of
    /// counts and gate rows that are simply the last thing that was true —
    /// presented, before this fix, exactly as if they were live. This is
    /// the minimal honest correction: say so in a footer line, and disable
    /// the gate Approve/Reject buttons, which cannot succeed anyway
    /// (`backend.client()` is nil or its POST will fail) and whose whole
    /// premise — that the run is still parked on that gate — is the part
    /// that can no longer be checked. Everything else stays visible; stale
    /// counts labeled as stale are still useful, and blanking them would
    /// throw away the only information available.
    var isBackendHealthy: Bool {
        Self.isHealthy(backend.health)
    }

    /// Pure seam over `isBackendHealthy` — non-`private` so
    /// `MenuBarViewTests` can pin the per-`BackendHealth`-case decision
    /// without constructing a `BackendController` whose `health` is
    /// `private(set)` and only movable by a real health monitor.
    /// `.starting` deliberately counts as NOT healthy: at launch there is
    /// no data yet, so the caption is accurate rather than premature, and
    /// a gate action posted before the first successful probe has nothing
    /// to post through.
    static func isHealthy(_ health: BackendHealth) -> Bool {
        if case .healthy = health { return true }
        return false
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            statTiles
            Divider()
            needsYouSection
            Divider()
            if !isBackendHealthy {
                unreachableCaption
            }
            localOnlyCaption
            footer
        }
        .frame(width: 320)
        .background(Color.rupuBg)
    }

    private var unreachableCaption: some View {
        HStack(spacing: 5) {
            Circle()
                .fill(Color.status(.failed))
                .frame(width: 6, height: 6)
            Text("Backend unreachable — showing last known")
                .font(.metaText)
                .foregroundStyle(Color.rupuErr)
                .lineLimit(1)
        }
        .padding(.horizontal, 12)
        .padding(.top, 8)
    }

    // MARK: - Stat tiles

    private var statTiles: some View {
        HStack(spacing: 0) {
            statTile(label: "Running", value: store.counts?.running, tone: .running)
            statTile(label: "Awaiting", value: store.counts?.awaitingApproval, tone: .awaiting)
            statTile(label: "Paused", value: store.counts?.paused, tone: .paused)
            statTile(label: "Pending", value: store.counts?.pending, tone: .pending)
        }
        .padding(.vertical, 10)
    }

    /// `value == nil` (pre-first-poll, per `MenuBarStore.counts`'s doc
    /// comment) renders `"—"` rather than a misleading `0`. `.help(...)`
    /// tooltip — same disclosure pattern `NeedsYouCard.header` already uses
    /// for its own coverage-limitation note — states the local-only scope
    /// on every tile, not just once in the footer caption, since a tile can
    /// be glanced at without ever reading down to the footer.
    private func statTile(label: String, value: Int?, tone: StatusTone) -> some View {
        VStack(spacing: 3) {
            Text(value.map(String.init) ?? "—")
                .font(.dataMono(15))
                .foregroundStyle(Color.status(tone))
            Eyebrow(label)
        }
        .frame(maxWidth: .infinity)
        .help("Local host only — see rupu for fleet-wide counts.")
    }

    // MARK: - Needs you

    private var needsYouSection: some View {
        VStack(alignment: .leading, spacing: 0) {
            if store.needsYou.isEmpty {
                Text("Nothing needs you")
                    .font(.uiText)
                    .foregroundStyle(Color.rupuMute)
                    .frame(maxWidth: .infinity, minHeight: 32, alignment: .leading)
                    .padding(.horizontal, 12)
            } else {
                ForEach(store.needsYou) { item in
                    MenuBarNeedsYouRow(
                        item: item,
                        store: store,
                        backend: backend,
                        gateActionsEnabled: isBackendHealthy,
                        onOpen: openRoute
                    )
                    if item.id != store.needsYou.last?.id {
                        Divider()
                    }
                }
            }
            if store.overflow > 0 {
                Divider()
                overflowFooter
            }
        }
    }

    /// Always routes to the unfiltered Activity screen — same "no
    /// status-scoped route exists" rule `NeedsYouCard.footer` already
    /// documents (`RupuOverview/NeedsYou.swift`).
    private var overflowFooter: some View {
        Button {
            openRoute(.activity(.all))
        } label: {
            Text("\(store.overflow) more in rupu")
                .font(.metaText)
                .foregroundStyle(Color.rupuDim)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .buttonStyle(.plain)
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }

    private func openRoute(_ route: Route) {
        model.navigate(to: route)
        openMainWindow()
    }

    // MARK: - Footer

    /// Review fix (round 1): explicit in-app disclosure that everything
    /// above is local-host only, never fleet-wide — matches the
    /// `NeedsYouCard.header` precedent (`RupuOverview/NeedsYou.swift`) of
    /// stating a coverage limitation directly in the surface that has it,
    /// not just in a doc comment nobody using the app ever reads.
    private var localOnlyCaption: some View {
        Text("Local host only — open rupu for fleet-wide")
            .font(.metaText)
            .foregroundStyle(Color.rupuMute)
            .padding(.horizontal, 12)
            .padding(.top, 8)
    }

    private var footer: some View {
        HStack(spacing: 8) {
            Button("Open rupu") {
                openMainWindow()
            }
            .buttonStyle(RupuButtonStyle.outline)
            .controlSize(.small)

            Button("New run") {
                // Same destination `ShellToolbar`'s "+ New run" leaves the
                // app in — land on Activity, then present the launcher
                // sheet via `AppModel.showLauncher`, the one presentation
                // seam `RootView.sheet(isPresented: $model.showLauncher)`
                // reads (see that type's doc comment) — front (or, per
                // `AppDelegate.frontMainWindow`'s windowless fallback, open)
                // the window FIRST so the sheet doesn't try to present over
                // a still-backgrounded or nonexistent window.
                //
                // **Known gap** (review fix, round 1 — honesty note, not a
                // fix): `ShellToolbar.newRunButton` also calls
                // `palette?.close()` before opening the launcher, because a
                // launcher sheet opening over an already-open command
                // palette would strand the palette's own Esc handling
                // behind the sheet. `RootView`'s `palette` (`PaletteStore`)
                // is `private` to that module — this module has no handle
                // on it and no way to close it. In practice this is a
                // narrow window (the palette must already be open AND the
                // operator triggers "New run" from the menu bar in the same
                // moment) rather than a routine occurrence, but it is a real
                // gap this module cannot close without `RootView` exposing
                // that seam.
                model.navigate(to: .activity(.all))
                openMainWindow()
                model.showLauncher = true
            }
            .buttonStyle(RupuButtonStyle.primary)
            .controlSize(.small)

            Spacer(minLength: 0)

            SettingsLink {
                Icon(.settings, size: 12)
                    .foregroundStyle(Color.rupuDim)
            }
            .buttonStyle(.plain)
        }
        .padding(12)
    }
}

/// One compact needs-you row: tone tag, subject (tap to deep-link), meta
/// breadcrumb, and — for a gate row — inline Approve/Reject.
private struct MenuBarNeedsYouRow: View {
    let item: NeedsYouItem
    let store: MenuBarStore
    let backend: BackendController
    /// `false` while the backend is unhealthy — see `MenuBarView.
    /// isBackendHealthy` (final-review fix, M4).
    let gateActionsEnabled: Bool
    let onOpen: (Route) -> Void

    private var tone: StatusTone {
        item.kind == .gate ? .awaiting : .failed
    }

    private var kindLabel: String {
        item.kind == .gate ? "Gate" : "Failed"
    }

    private var breadcrumb: String {
        [item.row.host, item.row.project].compactMap { $0 }.joined(separator: " · ")
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Rectangle()
                    .fill(Color.status(tone))
                    .frame(width: 2, height: 10)
                Eyebrow(kindLabel)
                Spacer(minLength: 0)
            }
            subjectButton
            if item.kind == .gate, case .run(let runID, let host) = item.row.navigation {
                MenuBarGateActions(
                    runID: runID,
                    host: host,
                    store: store,
                    backend: backend,
                    enabled: gateActionsEnabled
                )
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }

    /// `route == nil` (an autoflow-event row with no run materialized yet)
    /// has nothing to navigate to — same "no dead control" rule
    /// `NeedsYouRow.actions` already follows — so the subject renders as
    /// plain text instead of a button in that case.
    @ViewBuilder
    private var subjectButton: some View {
        if let route = item.row.navigation.route {
            Button {
                onOpen(route)
            } label: {
                subjectText
            }
            .buttonStyle(.plain)
        } else {
            subjectText
        }
    }

    private var subjectText: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(item.row.subject)
                .font(.uiText)
                .foregroundStyle(Color.rupuInk)
                .lineLimit(1)
                .truncationMode(.tail)
            if !breadcrumb.isEmpty {
                Text(breadcrumb)
                    .font(.metaText)
                    .foregroundStyle(Color.rupuDim)
                    .lineLimit(1)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// Compact Approve/Reject for one gate row, standalone (no `ActivityStore`
/// to route through — `MenuBarStore` doesn't own one). Resolves the sole
/// awaiting gate via `ActivityStore.resolveSoleAwaitingGate(client:runID:
/// host:)` (the shared static helper `NeedsYouCard`'s own gate actions use
/// — see that method's doc comment for why it's static and shared), then
/// posts through `BackendController.client()` directly and tracks the
/// mutation on `backend.pendingActions` — the SAME app-wide ledger
/// `ActivityStore`/`RunDetailStore` share (see `BackendController.
/// pendingActions`'s doc comment), so a gate approved from here reads as
/// pending/confirmed consistently if the operator also has Activity or Run
/// Detail open.
///
/// **Confirmation latency** (review fix, round 1): unlike `ActivityStore`'s
/// own approve/reject (confirmed via that store's live SSE `.statusPatch`
/// reduction), `MenuBarStore` has no live stream — it only polls REST. A
/// successful POST now calls `store.refreshNow()` (mirroring
/// `ActivityStore`'s own post-mutation `scheduleDebouncedRefresh()`), which
/// shrinks the wait from up to a full `pollInterval` (60s in production)
/// down to roughly one request round trip — the row disappearing from
/// `needsYou` once the server no longer reports the run as `.awaiting` is
/// still the signal this view relies on (same "row vanishing IS the
/// confirmation" contract `ActionVerb.remove` already uses elsewhere), it
/// just now arrives promptly instead of on the next scheduled tick. The
/// underlying `backend.pendingActions` key itself is still only ever
/// resolved by `PendingActions.resolve(runID:observedStatus:)` if SOME
/// live-patching store (Activity/Run Detail) is active and observes the
/// transition — `refreshNow()` narrows how long the row visibly sits
/// pending here, it doesn't change how the shared ledger entry itself gets
/// marked `.confirmed`.
private struct MenuBarGateActions: View {
    let runID: String
    let host: String?
    let store: MenuBarStore
    let backend: BackendController
    /// Final-review fix (M4): `false` while the backend is unhealthy. Both
    /// buttons disable — an approve/reject posted against a backend that
    /// isn't answering can't succeed, and the row it's acting on is
    /// last-known data whose "still awaiting" premise is precisely what
    /// can't be re-checked right now.
    let enabled: Bool

    @State private var isBusy = false
    @State private var resolvedGate: String?

    private var approveKey: ActionKey? {
        resolvedGate.map { ActionKey.gate(runID: runID, stepID: $0, verb: .approve) }
    }

    private var rejectKey: ActionKey? {
        resolvedGate.map { ActionKey.gate(runID: runID, stepID: $0, verb: .reject) }
    }

    private func isPending(_ key: ActionKey?) -> Bool {
        guard let key, case .pending = backend.pendingActions.state(key) else { return false }
        return true
    }

    private var anyPending: Bool {
        isPending(approveKey) || isPending(rejectKey)
    }

    /// Review fix (round 1): ported from `NeedsYouGateActions.isStale`
    /// (`RupuOverview/NeedsYou.swift:391-393`) — the menu bar is, if
    /// anything, MORE likely to sit on a stuck pending mutation than that
    /// card (no SSE confirmation here at all — see this type's doc comment
    /// on confirmation latency), so it needs the same "this may be stuck"
    /// affordance at least as much.
    private var isStale: Bool {
        [approveKey, rejectKey].compactMap { $0 }.contains { backend.pendingActions.isStale($0) }
    }

    /// Review fix (round 1): the mutating POST's own failure message was
    /// being written into `backend.pendingActions` via `.fail(_:_:)` and
    /// never displayed anywhere — a silently-swallowed failed approve/
    /// reject. Surfaces whichever of the two keys is currently `.failed`
    /// (both can never be simultaneously in flight — `anyPending`/`isBusy`
    /// disable the other button while one is running).
    private var failureMessage: String? {
        for key in [approveKey, rejectKey].compactMap({ $0 }) {
            if case .failed(let message) = backend.pendingActions.state(key) {
                return message
            }
        }
        return nil
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Button {
                    Task { await resolveAndApprove() }
                } label: {
                    HStack(spacing: 4) {
                        if isBusy || isPending(approveKey) {
                            ProgressView().controlSize(.mini)
                        }
                        Text("Approve")
                    }
                }
                .buttonStyle(RupuButtonStyle.primaryOk)
                .controlSize(.small)
                .disabled(!enabled || isBusy || anyPending)

                Button {
                    Task { await resolveAndReject() }
                } label: {
                    HStack(spacing: 4) {
                        if isBusy || isPending(rejectKey) {
                            ProgressView().controlSize(.mini)
                        }
                        Text("Reject")
                    }
                }
                .buttonStyle(RupuButtonStyle.dangerOutline)
                .controlSize(.small)
                .disabled(!enabled || isBusy || anyPending)
            }
            if isStale {
                Text("Still pending — this may be stuck")
                    .font(.noteText)
                    .foregroundStyle(Color.rupuMute)
            }
            if let failureMessage {
                Text(failureMessage)
                    .font(.noteText)
                    .foregroundStyle(Color.rupuErr)
                    .lineLimit(2)
            }
        }
    }

    private func resolveAndApprove() async {
        isBusy = true
        defer { isBusy = false }
        guard let client = backend.client(),
              let gate = await ActivityStore.resolveSoleAwaitingGate(client: client, runID: runID, host: host)
        else { return }
        resolvedGate = gate
        let key = ActionKey.gate(runID: runID, stepID: gate, verb: .approve)
        backend.pendingActions.begin(key)
        do {
            _ = try await client.approveRun(id: runID, host: host, gate: gate)
            await store.refreshNow()
        } catch {
            backend.pendingActions.fail(key, Self.mutationErrorMessage(error))
        }
    }

    private func resolveAndReject() async {
        isBusy = true
        defer { isBusy = false }
        guard let client = backend.client(),
              let gate = await ActivityStore.resolveSoleAwaitingGate(client: client, runID: runID, host: host)
        else { return }
        resolvedGate = gate
        let key = ActionKey.gate(runID: runID, stepID: gate, verb: .reject)
        backend.pendingActions.begin(key)
        do {
            _ = try await client.rejectRun(id: runID, host: host, gate: gate)
            await store.refreshNow()
        } catch {
            backend.pendingActions.fail(key, Self.mutationErrorMessage(error))
        }
    }

    /// Small, deliberate duplicate of `RupuStore`'s internal (non-`public`)
    /// `mutationErrorMessage(_:)` — this module has no other reason to reach
    /// into `RupuStore`'s private mutation-error mapping for the sake of one
    /// shared free function, and the mapping itself (one 501 special case,
    /// else `String(describing:)`) is small enough that duplicating it here
    /// is cheaper than widening that function's access across module
    /// boundaries for a single caller.
    private static func mutationErrorMessage(_ error: Error) -> String {
        if let cpError = error as? CPError, case .http(let status, _) = cpError, status == 501 {
            return "server lacks launch runtime — start with `rupu cp serve`"
        }
        return String(describing: error)
    }
}
