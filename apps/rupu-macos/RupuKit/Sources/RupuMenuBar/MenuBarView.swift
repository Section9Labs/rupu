import SwiftUI
import RupuAPI
import RupuStore
import RupuDesign
import RupuOverview

/// The `MenuBarExtra` scene's own label content. There is no dedicated
/// app-icon/asset catalog yet (`apps/rupu-macos/App` has none — icon work is
/// deferred to Phase 7's signing/notarization pass), so this reuses the
/// exact wordmark `Sidebar.brandHeader` already renders (`Text("rupu")`,
/// `leadText`/semibold) rather than inventing a separate glyph asset, with a
/// small dot overlay — the `.awaiting` status tone, matching the gate rows
/// it's warning about — when at least one run is awaiting approval
/// fleet-wide. `hasAttention` is read from `MenuBarStore.counts` by the
/// caller (`RupuApp`'s `MenuBarExtra` label closure); this view has no
/// store dependency of its own so it stays a plain, easily-previewed leaf.
public struct MenuBarStatusLabel: View {
    let hasAttention: Bool

    public init(hasAttention: Bool) {
        self.hasAttention = hasAttention
    }

    public var body: some View {
        ZStack(alignment: .topTrailing) {
            Text("rupu")
                .font(.system(size: 12, weight: .semibold))
            if hasAttention {
                Circle()
                    .fill(Color.status(.awaiting))
                    .frame(width: 6, height: 6)
                    .offset(x: 5, y: -3)
            }
        }
    }
}

/// The `MenuBarExtra`'s popover content (`.menuBarExtraStyle(.window)`):
/// four live stat tiles, a top-5 needs-you triage list with inline gate
/// actions, and a footer (Open rupu / New run / Settings…).
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

    public var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            statTiles
            Divider()
            needsYouSection
            Divider()
            footer
        }
        .frame(width: 320)
        .background(Color.rupuBg)
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
    /// comment) renders `"—"` rather than a misleading `0`.
    private func statTile(label: String, value: Int?, tone: StatusTone) -> some View {
        VStack(spacing: 3) {
            Text(value.map(String.init) ?? "—")
                .font(.dataMono(15))
                .foregroundStyle(Color.status(tone))
            Eyebrow(label)
        }
        .frame(maxWidth: .infinity)
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
                    MenuBarNeedsYouRow(item: item, backend: backend, onOpen: openRoute)
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
                // reads (see that type's doc comment) — front the window
                // FIRST so the sheet doesn't try to present over a
                // still-backgrounded window.
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
    let backend: BackendController
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
                MenuBarGateActions(runID: runID, host: host, backend: backend)
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
/// **Confirmation caveat**: unlike `ActivityStore`'s own approve/reject
/// (confirmed via that store's live SSE `.statusPatch` reduction),
/// `MenuBarStore` has no live stream — it only polls REST every 60s. A key
/// begun here is only ever resolved by `PendingActions.resolve(runID:
/// observedStatus:)` if SOME other live-patching store (Activity/Run
/// Detail) happens to be active and observes the same run's status
/// transition; otherwise it just sits `.pending` until the ledger entry is
/// cleared some other way. The row disappearing from `needsYou` on the
/// NEXT poll (once the server no longer reports the run as `.awaiting`) is
/// the honest signal this view itself relies on — the same "row vanishing
/// IS the confirmation" contract `ActionVerb.remove` already uses elsewhere.
private struct MenuBarGateActions: View {
    let runID: String
    let host: String?
    let backend: BackendController

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

    var body: some View {
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
            .disabled(isBusy || anyPending)

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
            .disabled(isBusy || anyPending)
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
