import Foundation
import RupuAPI
import RupuDesign
import RupuStore
import SwiftUI

// MARK: - Pure seam (the tested derivation)
//
// `NeedsYouItem`/`deriveNeedsYou` moved to `RupuStore/NeedsYou.swift` (perf &
// interaction arc, Plan 5 Task 4) — `RupuActivity`'s new `.all`-kind stats
// surface needs the identical derivation for its own compact
// needs-attention list, and the module graph has no edge from
// `RupuActivity` to this module (nor should it grow one just for a data
// helper). Every type the derivation touches was already `RupuStore`-native,
// so this module (which already depends on `RupuStore`) keeps working with
// zero call-site changes below — only the definitions moved, resolved
// through the existing `import RupuStore` instead of locally.

// MARK: - View

/// The Overview screen's needs-you queue (spec §5.1-ish: the operator's
/// glance-first "what needs me" panel) — v2 card rows over
/// `deriveNeedsYou`'s output, with inline gate approve/reject wired through
/// `ActivityStore`'s gate-scoped mutations.
public struct NeedsYouCard: View {
    /// **Fleet-wide by construction, not by caller discipline** (perf &
    /// interaction arc, Plan 5 Task 2 fix-round-1): this reads
    /// `store.unscopedRows`, never `store.scopeFilter`-or-`statusFilter`-
    /// narrowed `store.rows` — see `ActivityStore.unscopedRows`'s own doc
    /// comment. The needs-you queue has to see every gate/failure
    /// regardless of the v2 top bar's project-scope selection AND
    /// regardless of whatever `scopeFilter`/`statusFilter` `ActivityScreen`
    /// (which shares this exact `ActivityStore` instance — see `RootView.
    /// activityStore`'s doc comment) may have last left the store in.
    /// Originally this was an invariant the CALLER had to uphold ("never
    /// pass a scoped store in here") — that broke the moment the store
    /// became shared, since nothing reset `scopeFilter` when the operator
    /// navigated from Activity back to Overview. Reading `unscopedRows`
    /// instead makes the guarantee structural: no store state any other
    /// screen sets can make this view hide anything, because this view
    /// never reads the narrowed projection at all.
    private let store: ActivityStore
    private let backend: BackendController
    private let range: TimeRange
    private let onNavigate: (Route) -> Void

    public init(store: ActivityStore, backend: BackendController, range: TimeRange, onNavigate: @escaping (Route) -> Void) {
        self.store = store
        self.backend = backend
        self.range = range
        self.onNavigate = onNavigate
    }

    /// Controller ruling (Phase 4, Task 5 fix round 1 — the brief's row
    /// anatomy left this underspecified): when there is nothing to show,
    /// the whole panel collapses to the single 36pt empty row — no
    /// "Needs you" header, no divider under it. Per the v2 redesign's §1a
    /// intent, "never render an empty card": a header + divider framing
    /// nothing would still read as a card with content, just an
    /// unusually terse one.
    public var body: some View {
        let result = deriveNeedsYou(rows: store.unscopedRows, range: range, now: Date())

        if result.items.isEmpty {
            emptyRow
                .panelStyle(.panel)
        } else {
            let oldestGateID = result.items.first(where: { $0.kind == .gate })?.id
            VStack(alignment: .leading, spacing: 0) {
                header
                Divider()
                VStack(spacing: 0) {
                    ForEach(result.items) { item in
                        NeedsYouRow(
                            item: item,
                            isOldestGate: item.id == oldestGateID,
                            store: store,
                            backend: backend,
                            onOpen: onNavigate
                        )
                        if item.id != result.items.last?.id {
                            Divider()
                        }
                    }
                }
                if result.overflow > 0 {
                    footer(overflow: result.overflow)
                }
            }
            .panelStyle(.panel)
        }
    }

    /// Final-review fix (Task 2): spec §3's "Needs-you data (disposition)"
    /// promises this coverage limitation is "stated in-app" — no UI carried
    /// it until now. A `.help(...)` tooltip rather than always-visible copy:
    /// the header itself already reads as "the operator's attention queue",
    /// and a permanent disclaimer line would fight that at a glance for
    /// something only worth knowing on demand.
    private var header: some View {
        Eyebrow("Needs you")
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .help("Covers runs the fleet-wide list APIs return — the same coverage as Activity — not a server-defined attention set.")
    }

    /// Single 36pt row, `.rupuMute` — nothing needs the operator right now.
    /// The *entire* card when empty (see the controller ruling on `body`
    /// above) — not a row nested inside a header+divider shell.
    private var emptyRow: some View {
        Text("Nothing needs you")
            .font(.uiText)
            .foregroundStyle(Color.rupuMute)
            .frame(maxWidth: .infinity, minHeight: 36, alignment: .leading)
            .padding(.horizontal, 12)
    }

    /// "+N more" always routes to the unfiltered Activity screen — Activity's
    /// own awaiting chip is one click away from there, and `Route.activity`
    /// only ever carries a `RunKindFilter` (all/agents/workflows/autoflows/
    /// sessions), never a status preset. Verified against `RupuStore/
    /// Route.swift` at implementation time: there is no status-scoped route
    /// to navigate to instead, so this is `.activity(.all)`, not a
    /// synthesized "awaiting" route that doesn't exist.
    private func footer(overflow: Int) -> some View {
        Button {
            onNavigate(.activity(.all))
        } label: {
            Text("+\(overflow) more")
                .font(.metaText)
                .foregroundStyle(Color.rupuDim)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .buttonStyle(.plain)
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .overlay(alignment: .top) {
            Rectangle().fill(Color.rupuBorder).frame(height: 1)
        }
    }
}

/// One needs-you row: a 2px leading tone edge, kind tag, subject +
/// breadcrumb, right-aligned age, and per-kind actions.
private struct NeedsYouRow: View {
    let item: NeedsYouItem
    let isOldestGate: Bool
    let store: ActivityStore
    let backend: BackendController
    let onOpen: (Route) -> Void

    private var tone: StatusTone {
        item.kind == .gate ? .awaiting : .failed
    }

    private var kindLabel: String {
        item.kind == .gate ? "Gate" : "Failed"
    }

    /// "host · project", dropping the project segment when the row carries
    /// none (most kinds — only `.session` rows populate it).
    private var breadcrumb: String {
        [item.row.host, item.row.project].compactMap { $0 }.joined(separator: " · ")
    }

    private var ageLabel: String {
        guard let date = item.row.startedAt else { return "—" }
        return Self.relativeFormatter.localizedString(for: date, relativeTo: Date())
    }

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Rectangle()
                .fill(Color.status(tone))
                .frame(width: 2)
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 6) {
                    Eyebrow(kindLabel)
                    Spacer(minLength: 8)
                    Text(ageLabel)
                        .font(.dataMono(11))
                        .foregroundStyle(isOldestGate ? Color.status(.awaiting) : Color.rupuDim)
                }
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
                actions
            }
            .padding(.trailing, 12)
        }
        .padding(.vertical, 8)
    }

    @ViewBuilder
    private var actions: some View {
        switch item.kind {
        case .gate:
            if case .run(let runID, let host) = item.row.navigation {
                NeedsYouGateActions(runID: runID, host: host, store: store, backend: backend)
            }
        case .failedRun:
            if let route = item.row.navigation.route {
                Button {
                    onOpen(route)
                } label: {
                    Text("Open")
                }
                .buttonStyle(RupuButtonStyle.outline)
                .controlSize(.small)
            }
            // `route == nil` (an autoflow-event row with no run
            // materialized yet — `ActivityRow.Navigation.none`) has nothing
            // to navigate to: no dead control, same "no navigation, no tap
            // affordance" rule `ActivityTable`'s `isClickable` already
            // follows.
        }
    }

    private static let relativeFormatter: RelativeDateTimeFormatter = {
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter
    }()
}

/// Compact Approve/Reject for one gate row — reuses `ActivityTable`'s
/// inline-action shape (`RupuActivity/ActivityTable.swift`'s
/// `awaitingActions`): this feed (`ActivityStore.rows`, same federated
/// source `ActivityTable` renders) carries no per-row `awaiting[]` detail,
/// only `GET /api/runs/:id` does — so a tap resolves the sole parked gate
/// before posting, same "gate targeting always explicit" contract every
/// write route in this phase follows. The resolution call itself
/// (`ActivityStore.resolveSoleAwaitingGate(client:runID:host:)`) is shared,
/// not duplicated — Phase 4, Task 5 fix round 1 lifted it out of both this
/// type and `ActivityTable` into `RupuStore`.
///
/// The button *chrome* stays duplicated, deliberately: the two call sites
/// render different UI (`ActivityTable`'s icon-only pair inside a
/// fixed-width table column vs. this card's full `RupuButtonStyle.primaryOk`/
/// `.dangerOutline` labeled buttons with a busy/stale-state readout below),
/// so lifting it would mean parameterizing the chrome itself for two call
/// sites that don't actually want to look alike — reviewed and agreed as
/// legitimately view-local, not a missed seam.
private struct NeedsYouGateActions: View {
    let runID: String
    let host: String?
    let store: ActivityStore
    let backend: BackendController

    /// Covers the resolve-then-post round trip's own UI feedback (spinner +
    /// disable, double-tap guard) — same scope `ActivityTable`'s `isBusy`
    /// covers, and for the same reason: this view doesn't know which gate
    /// it's targeting until `resolveSoleAwaitingGate` answers, so there's no
    /// `ActionKey` to read pending state from until then.
    @State private var isBusy = false

    /// Set once `resolveSoleAwaitingGate` answers — from that point on, the
    /// row can read the mutation's actual pending/stale state out of the
    /// shared `ActivityStore.pendingActions` ledger (the same one
    /// `RunDetailStore`'s gate banner reads), rather than only ever showing
    /// the local busy spinner.
    @State private var resolvedGate: String?

    private var approveKey: ActionKey? {
        resolvedGate.map { ActionKey.gate(runID: runID, stepID: $0, verb: .approve) }
    }

    private var rejectKey: ActionKey? {
        resolvedGate.map { ActionKey.gate(runID: runID, stepID: $0, verb: .reject) }
    }

    private func isPending(_ key: ActionKey?) -> Bool {
        guard let key, case .pending = store.pendingActions.state(key) else { return false }
        return true
    }

    private var anyPending: Bool {
        isPending(approveKey) || isPending(rejectKey)
    }

    private var isStale: Bool {
        [approveKey, rejectKey].compactMap { $0 }.contains { store.pendingActions.isStale($0) }
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
            if isStale {
                Text("Still pending — this may be stuck")
                    .font(.noteText)
                    .foregroundStyle(Color.rupuMute)
            }
        }
    }

    private func resolveAndApprove() async {
        isBusy = true
        defer { isBusy = false }
        guard let gate = await resolveSoleAwaitingGate() else { return }
        resolvedGate = gate
        await store.approve(runID: runID, gate: gate, host: host)
    }

    private func resolveAndReject() async {
        isBusy = true
        defer { isBusy = false }
        guard let gate = await resolveSoleAwaitingGate() else { return }
        resolvedGate = gate
        await store.reject(runID: runID, gate: gate, host: host)
    }

    /// Delegates to `ActivityStore.resolveSoleAwaitingGate(client:runID:
    /// host:)` — Phase 4, Task 5 fix round 1 lifted the actual resolution
    /// (this method used to duplicate `ActivityTable`'s byte-for-byte) into
    /// `RupuStore` as the one shared home; see that method's doc comment
    /// for the full "why here, why static" rationale. A run with no
    /// resolvable single gate is a silent no-op here too — the row's next
    /// live-patch or refresh corrects its status either way.
    private func resolveSoleAwaitingGate() async -> String? {
        guard let client = backend.client() else { return nil }
        return await ActivityStore.resolveSoleAwaitingGate(client: client, runID: runID, host: host)
    }
}
