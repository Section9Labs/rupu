import SwiftUI
import RupuAPI
import RupuStore
import RupuDesign

/// The Run Detail screen (Task 8, write path added Phase 3 Task 5; recomposed
/// to a single vertical stack in flows-composition Task 4): header (back
/// chevron, breadcrumb, status pill, facts row + a second identity meta
/// line, run-control buttons), an awaiting banner with live Approve/Reject
/// controls per parked gate, the live step graph — each node tappable,
/// driving `RunDetailStore.select(step:)` — and a selection-following tab
/// panel (`RunDetailTabPanel`: Transcript · Events · Findings · Netflow,
/// `RunDetailTabs.swift`) beneath it. The old fixed-width `RailColumn`
/// (`RailViews.swift`, deleted) is gone; `FactsCard`'s identity rows folded
/// into the header, `NetflowCard`/`FindingsCard` became tab content. Owns a
/// `RunDetailStore` lifecycle the same way `ActivityScreen` owns an
/// `ActivityStore` — built lazily on first appearance (once
/// `backend.client()` exists), rebuilt whenever `runID` changes (navigating
/// from one run's detail straight to another's), and `deactivate()`d
/// `.onDisappear`.
///
/// **Write path (Phase 3, Task 5)**: the header renders live Cancel/Pause/
/// Resume buttons plus an Archive/Restore overflow menu, driven strictly
/// from `RunDetailStore.availableVerbs` for the run's current status — no
/// dead controls. The awaiting banner renders one Approve/Reject control
/// pair per parked gate. Every mutating control's own tap is its retry (see
/// `mutationButton`'s doc comment) and a `.failed` outcome renders inline —
/// `runVerbFailureNotes` below the header controls for cancel/pause/resume/
/// archive/restore, `gateFailureNote` per gate for approve/reject. Every
/// block below (`detail`/`graph`/`netflow`/`findings`) still fails
/// independently: one `.failed` block renders its own failure box without
/// blanking the other three.
///
/// **Does NOT need `OverviewScreen`'s `.onChange(of: backend.health)`
/// cold-launch fix**: that fix exists because `.overview` is
/// `AppModel.route`'s default and never persisted (see `AppModel.swift`),
/// so it is *always* the very first screen a cold launch renders —
/// sometimes before `backend.client()` resolves, with nothing to
/// re-trigger `activate()` once it does. `RunDetailScreen` is only ever
/// reached by pushing onto `AppModel.routeStack` (an `ActivityRow`/
/// `NeedsYouRow` tap) — never a cold-launch route — so its `.task(id: runID)`
/// always runs for the first time well after the shell's own connection
/// attempt has already resolved.
public struct RunDetailScreen: View {
    @Bindable var model: AppModel
    let backend: BackendController
    let runID: String
    let host: String?

    @State private var store: RunDetailStore?
    @State private var storeRunID: String?
    /// Phase 6B, Task 5: the transcript tab's source/AST preview cache. A
    /// client-identity change rebuilds it fresh (a new `CPClient`); a
    /// run-only change instead reconfigures the SAME instance in place via
    /// `setRun(runID:host:)`, which flushes its cache — see `activate()`'s
    /// doc comment for why the split exists.
    @State private var sourcePreviewStore: SourcePreviewStore?

    /// Tracked alongside `storeRunID` so `activate()` rebuilds on a backend
    /// client swap (embedded/remote switch, reconnect, restart) too, not
    /// just a `runID` change — see `activate()`'s doc comment and
    /// `BackendController.clientIdentity()`.
    @State private var storeClientID: ObjectIdentifier?
    @State private var selectedTab: RunDetailTab = .transcript

    public init(model: AppModel, backend: BackendController, runID: String, host: String?) {
        self.model = model
        self.backend = backend
        self.runID = runID
        self.host = host
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
        .task(id: runID) {
            await activate()
        }
        .onDisappear {
            store?.deactivate()
        }
    }

    /// Builds (or rebuilds, on a `runID` change OR a backend client swap)
    /// the store and activates it. `storeRunID` — not the store's own
    /// identity — is what decides "rebuild vs. reuse" for a `runID` change:
    /// `RunDetailStore` doesn't expose its `runID` publicly (it's plumbing,
    /// not UI-relevant state), so this screen keeps its own record of which
    /// run the current `store` was built for. `storeClientID` is the same
    /// idea for the backend connection: an embedded/remote mode switch,
    /// reconnect, or restart swaps `backend.client()` to a brand-new
    /// `CPClient` (see `BackendController.clientIdentity()`'s doc comment)
    /// without ever going through `nil` in between, so a plain "do I
    /// already have a store" check would never notice and would keep
    /// running `store` against the abandoned connection.
    ///
    /// **`sourcePreviewStore` splits the two triggers** (review fix,
    /// finding 3): a client-identity change still rebuilds it fresh (a new
    /// `CPClient` means the old instance's captured client is dead weight —
    /// same reasoning `RunDetailStore` gets rebuilt fresh for too). A
    /// run-ONLY change instead calls `setRun(runID:host:)` on the EXISTING
    /// instance — this is what makes `SourcePreviewStore`'s own generation
    /// guard (see that type's doc comment) load-bearing rather than
    /// theoretical: without this, every run switch would already start from
    /// an empty cache via a fresh instance, and `setRun`'s flush would never
    /// actually run in production. `RunDetailStore` itself is still rebuilt
    /// fresh on every run change (it owns a live stream/tail lifecycle that
    /// genuinely needs a clean restart, unlike this store's plain fetch
    /// cache).
    private func activate() async {
        guard let client = backend.client() else { return }
        let clientID = backend.clientIdentity()
        if storeClientID != clientID {
            store?.deactivate()
            store = RunDetailStore(runID: runID, host: host, client: client, backend: backend)
            sourcePreviewStore = SourcePreviewStore(runID: runID, host: host, client: client)
            storeRunID = runID
            storeClientID = clientID
        } else if storeRunID != runID {
            store?.deactivate()
            store = RunDetailStore(runID: runID, host: host, client: client, backend: backend)
            if let sourcePreviewStore {
                sourcePreviewStore.setRun(runID: runID, host: host)
            } else {
                sourcePreviewStore = SourcePreviewStore(runID: runID, host: host, client: client)
            }
            storeRunID = runID
        }
        guard let store else { return }
        await store.activate()
    }

    // MARK: - Layout

    private func content(store: RunDetailStore) -> some View {
        VStack(alignment: .leading, spacing: 16) {
            header(store: store)
            if case .content(let detail) = store.detail {
                awaitingBanner(store: store, detail: detail)
            }
            stepGraphSection(store: store)
                .frame(height: 420)
            RunDetailTabPanel(store: store, tab: $selectedTab, runID: runID, host: host, sourcePreviewStore: sourcePreviewStore)
        }
        .padding(16)
    }

    // MARK: - Header

    private func header(store: RunDetailStore) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                Button {
                    model.navigateBack()
                } label: {
                    Icon(.arrowLeft)
                        .foregroundStyle(Color.rupuDim)
                }
                .buttonStyle(.plain)

                if case .content(let detail) = store.detail {
                    Text("Activity ▸ \(detail.run.workflowName)")
                        .font(.noteText)
                        .foregroundStyle(Color.rupuDim)
                    Spacer(minLength: 0)
                    statusPill(detail.run.status)
                    actionControls(store: store)
                } else {
                    Text("Activity ▸ \(runID)")
                        .font(.noteText)
                        .foregroundStyle(Color.rupuDim)
                    Spacer(minLength: 0)
                }
            }
            runVerbFailureNotes(store: store)
            if case .content(let detail) = store.detail {
                factsRow(detail: detail)
                identityMetaLine(detail: detail)
            }
        }
    }

    /// Flows-composition Task 4: absorbs the old rail-side `FactsCard`'s
    /// identifier rows (run id, workspace, permission mode) as a second mono
    /// meta line beneath the header's token/cost `factsRow` — a detail
    /// expansion of the header, not a duplicate of it. Middle-truncated
    /// (`.truncationMode(.middle)`) same as `FactsCard.factRow` used to be,
    /// since a run/workspace id can run long.
    private func identityMetaLine(detail: APIRunDetail) -> some View {
        Text("RUN \(detail.run.id)  ·  WS \(detail.run.workspaceID)  ·  \(detail.run.permissionMode ?? "—")")
            .font(.dataMono(10))
            .foregroundStyle(Color.rupuDim)
            .lineLimit(1)
            .truncationMode(.middle)
    }

    // MARK: - Header mutations (Phase 3, Task 5)

    /// Renders strictly from `store.availableVerbs` — no dead controls, per
    /// the brief. Cancel/Pause/Resume are plain buttons (their own tap is
    /// the retry — a failed one can just be tapped again); Archive/Restore
    /// live behind an overflow menu since at most one of them is ever
    /// available at once and neither is a frequent action.
    @ViewBuilder
    private func actionControls(store: RunDetailStore) -> some View {
        let verbs = store.availableVerbs
        HStack(spacing: 6) {
            if verbs.contains(.cancel) {
                mutationButton(store: store, title: "Cancel", verb: .cancel) { await store.cancel() }
            }
            if verbs.contains(.pause) {
                mutationButton(store: store, title: "Pause", verb: .pause) { await store.pause() }
            }
            if verbs.contains(.resume) {
                mutationButton(store: store, title: "Resume", verb: .resume) { await store.resume() }
            }
            if verbs.contains(.archive) || verbs.contains(.restore) {
                Menu {
                    if verbs.contains(.archive) {
                        Button("Archive") { Task { await store.archive() } }
                            .disabled(isPending(store.pendingActions.state(ActionKey(runID, .archive))))
                    }
                    if verbs.contains(.restore) {
                        Button("Restore") { Task { await store.restore() } }
                            .disabled(isPending(store.pendingActions.state(ActionKey(runID, .restore))))
                    }
                } label: {
                    Icon(.moreHorizontal)
                        .foregroundStyle(Color.rupuDim)
                }
                .menuStyle(.borderlessButton)
                .frame(width: 20)
            }
        }
    }

    /// One header mutation button: `verb`'s own key drives the
    /// spinner-while-pending + disabled state (shared across every control
    /// keyed to it — an Activity-row tap for the same run would disable
    /// this one too, via the shared `pendingActions` ledger).
    private func mutationButton(store: RunDetailStore, title: String, verb: ActionVerb, action: @escaping () async -> Void) -> some View {
        let key = ActionKey(runID, verb)
        let state = store.pendingActions.state(key)
        let pending = isPending(state)
        return Button {
            Task { await action() }
        } label: {
            HStack(spacing: 4) {
                if pending {
                    ProgressView().controlSize(.mini)
                }
                Text(title)
            }
        }
        .buttonStyle(RupuButtonStyle.outline)
        .disabled(pending)
    }

    private func isPending(_ state: ActionState) -> Bool {
        if case .pending = state { return true }
        return false
    }

    /// Final-review fix: a run-level mutation's failure used to be
    /// invisible — `mutationButton`'s own tap is the retry, so nothing ever
    /// rendered `.failed` and the button just silently re-enabled with no
    /// on-screen sign anything went wrong. This renders one compact note
    /// area below the header controls, listing every one of the five
    /// run-scoped verbs (cancel/pause/resume/archive/restore) currently
    /// `.failed`, each with its own message and an explicit Retry — the
    /// same message+Retry convention `gateFailureNote` already established
    /// for the awaiting banner's approve/reject controls. Checked
    /// unconditionally against `pendingActions` (not filtered through
    /// `store.availableVerbs`) so a note stays visible even if the run's
    /// status moved the verb out of `availableVerbs` after the failure —
    /// the operator still needs to see what went wrong and retry it.
    @ViewBuilder
    private func runVerbFailureNotes(store: RunDetailStore) -> some View {
        let entries: [(title: String, key: ActionKey, retry: () async -> Void)] = [
            ("Cancel", ActionKey(runID, .cancel), { await store.cancel() }),
            ("Pause", ActionKey(runID, .pause), { await store.pause() }),
            ("Resume", ActionKey(runID, .resume), { await store.resume() }),
            ("Archive", ActionKey(runID, .archive), { await store.archive() }),
            ("Restore", ActionKey(runID, .restore), { await store.restore() }),
        ]
        let failures = entries.compactMap { entry -> (title: String, message: String, retry: () async -> Void)? in
            guard case .failed(let message) = store.pendingActions.state(entry.key) else { return nil }
            return (entry.title, message, entry.retry)
        }
        if !failures.isEmpty {
            VStack(alignment: .leading, spacing: 4) {
                ForEach(failures, id: \.title) { failure in
                    HStack(spacing: 6) {
                        Text("\(failure.title) failed:")
                            .font(.noteText.weight(.semibold))
                            .foregroundStyle(Color.status(.failed))
                        Text(failure.message)
                            .font(.noteText)
                            .foregroundStyle(Color.status(.failed))
                            .lineLimit(2)
                        Button("Retry") {
                            Task { await failure.retry() }
                        }
                        .buttonStyle(RupuButtonStyle.outline)
                    }
                }
            }
        }
    }

    /// Web-parity status pill (Step 1 sweep): the header's status readout is
    /// exactly the "run-detail pill" the v2 chrome sweep replaces with the
    /// shared `StatusPill` — label vocabulary now comes from
    /// `StatusDescriptor` (Task 4) instead of this screen's own copy.
    ///
    /// Fix round 1: `StatusPill` only ever knows how to render the 9
    /// `StatusTone` cases — for a raw server status it doesn't recognize,
    /// `ActivityStatus.normalize` still maps the *tone* to `.pending` (per
    /// its own doc comment) but carries the original string in
    /// `.unknown(raw)`. Rendering straight through `StatusPill` would
    /// silently swap that diagnostic string for the generic "Pending"
    /// label — the same raw-string fallback `ActivityStatus.displayLabel`
    /// (now `public` in `RupuStore`, flows-composition Task 3) preserves
    /// for `ActivityTable`/`FilterBar`. This screen doesn't reuse
    /// `displayLabel` directly: it needs the *decision* of whether the
    /// status is unrecognized at all (to choose between `StatusPill` and
    /// `unknownStatusPill`), not just a label string — `unrecognizedStatusRaw`
    /// is that pure seam, unit-tested in `RunDetailScreenStatusTests`.
    @ViewBuilder
    private func statusPill(_ rawStatus: String) -> some View {
        if let raw = Self.unrecognizedStatusRaw(rawStatus) {
            unknownStatusPill(raw)
        } else {
            StatusPill(ActivityStatus.normalize(rawStatus).tone)
        }
    }

    /// `nil` for any raw status `ActivityStatus.normalize` recognizes
    /// (render via `StatusPill`); the original raw string for one it
    /// doesn't (`.unknown(raw)`) — the pure decision `statusPill` renders
    /// from. Free of `self`/view state so it's directly testable.
    static func unrecognizedStatusRaw(_ rawStatus: String) -> String? {
        if case .unknown(let raw) = ActivityStatus.normalize(rawStatus) { return raw }
        return nil
    }

    /// Fallback pill for a status `StatusDescriptor` has no vocabulary
    /// for — same chrome as `StatusPill` (`ChromeShape.pill`, flat/neutral
    /// tint since `.pending` is one of the three flat tones — see
    /// `StatusTone.isFlatPill`) but with the raw server string as the label
    /// instead of a synthesized one.
    private func unknownStatusPill(_ raw: String) -> some View {
        HStack(spacing: 4) {
            Icon(StatusDescriptor.descriptor(for: .pending).icon, size: 11)
                .foregroundStyle(Color.statusPillInk(.pending))
            Text(raw)
                .font(.dataMono(10))
                .foregroundStyle(Color.statusPillInk(.pending))
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 3)
        .background(Color.statusPillBackground(.pending))
        .clipShape(ChromeShape.pill)
        .overlay(ChromeShape.pill.stroke(Color.statusPillRing(.pending), lineWidth: 1))
    }

    /// IN/OUT/CACHED are not redundant with TOKENS — that item is the
    /// aggregate `totalTokens`; these three are the per-kind breakdown the
    /// retired facts rail used to carry.
    private func factsRow(detail: APIRunDetail) -> some View {
        HStack(spacing: 20) {
            factItem("STARTED", relativeLabel(detail.run.startedAt))
            factItem("DURATION", durationLabel(detail.run))
            factItem("TOKENS", Fmt.count(Int(detail.usage.totalTokens)))
            factItem("IN", Fmt.count(Int(detail.usage.inputTokens)))
            factItem("OUT", Fmt.count(Int(detail.usage.outputTokens)))
            factItem("CACHED", Fmt.count(Int(detail.usage.cachedTokens)))
            factItem("COST", Fmt.cost(detail.usage.costUSD))
            Spacer(minLength: 0)
        }
    }

    private func factItem(_ label: String, _ value: String) -> some View {
        HStack(spacing: 6) {
            Eyebrow(label)
            Text(value)
                .font(.dataMono(11.5))
                .foregroundStyle(Color.rupuInk)
        }
    }

    private func relativeLabel(_ iso: String) -> String {
        guard let date = ActivityRow.parseISO(iso) else { return "—" }
        return Self.relativeFormatter.localizedString(for: date, relativeTo: Date())
    }

    /// `nil` (undefined) while the run hasn't finished yet — a live "time
    /// elapsed so far" counter isn't part of this phase's design.
    private func durationLabel(_ run: APIRunRecord) -> String {
        guard let finishedAt = run.finishedAt,
              let started = ActivityRow.parseISO(run.startedAt),
              let finished = ActivityRow.parseISO(finishedAt)
        else { return "—" }
        let ms = max(0, finished.timeIntervalSince(started) * 1000)
        return Fmt.duration(ms: UInt64(ms))
    }

    private static let relativeFormatter: RelativeDateTimeFormatter = {
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter
    }()

    // MARK: - Awaiting banner (Phase 3, Task 5)

    /// One row per parked gate — `gate.stepID` from `detail.run.awaiting` is
    /// always what's sent as the mutation's `gate:` (gate targeting is
    /// always explicit; never omitted even when there's only one).
    private func awaitingBanner(store: RunDetailStore, detail: APIRunDetail) -> some View {
        Group {
            if !detail.run.awaiting.isEmpty {
                TintBanner(tone: Color.status(.awaiting), toneBg: Color.status(.awaiting).opacity(0.08)) {
                    VStack(alignment: .leading, spacing: 10) {
                        ForEach(detail.run.awaiting, id: \.stepID) { gate in
                            gateRow(store: store, gate: gate)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
        }
    }

    private func gateRow(store: RunDetailStore, gate: APIAwaitingGate) -> some View {
        // Fix round 1: gate-scoped keys, not plain run-scoped ones — a run
        // showing more than one parked gate at once (this `ForEach`) needs
        // independent pending state per gate. A plain `ActionKey(runID,
        // .approve)` collided across gates: approving gate A
        // spinnered/disabled gate B's controls too, then silently
        // un-disabled them the moment gate A's own confirmation landed,
        // even though gate B's own mutation was never fired. See
        // `ActionKey.gate`'s doc comment.
        let approveKey = ActionKey.gate(runID: runID, stepID: gate.stepID, verb: .approve)
        let rejectKey = ActionKey.gate(runID: runID, stepID: gate.stepID, verb: .reject)
        let approveState = store.pendingActions.state(approveKey)
        let rejectState = store.pendingActions.state(rejectKey)
        let approvePending = isPending(approveState)
        let rejectPending = isPending(rejectState)
        let anyPending = approvePending || rejectPending

        return VStack(alignment: .leading, spacing: 6) {
            Text("Awaiting — \(gate.stepID)")
                .font(.metaText)
                .foregroundStyle(Color.status(.awaiting))
            Text(gate.prompt ?? "Approval requested")
                .font(.leadText)
                .foregroundStyle(Color.rupuInk)

            HStack(spacing: 8) {
                Button {
                    Task { await store.approve(gate: gate.stepID) }
                } label: {
                    HStack(spacing: 5) {
                        if approvePending {
                            ProgressView().controlSize(.mini)
                        }
                        Text("Approve")
                    }
                }
                .buttonStyle(RupuButtonStyle.primaryOk)
                .disabled(anyPending)

                Button {
                    Task { await store.reject(gate: gate.stepID) }
                } label: {
                    HStack(spacing: 5) {
                        if rejectPending {
                            ProgressView().controlSize(.mini)
                        }
                        Text("Reject")
                    }
                }
                .buttonStyle(RupuButtonStyle.dangerOutline)
                .disabled(anyPending)
            }

            gateFailureNote(state: approveState) { await store.approve(gate: gate.stepID) }
            gateFailureNote(state: rejectState) { await store.reject(gate: gate.stepID) }

            if store.pendingActions.isStale(approveKey) || store.pendingActions.isStale(rejectKey) {
                Text("Still pending — this may be stuck")
                    .font(.noteText)
                    .foregroundStyle(Color.rupuMute)
            }
        }
    }

    /// Inline failure text + retry for one gate control — only renders when
    /// `state` is `.failed`, so a fresh/pending/confirmed key shows nothing.
    @ViewBuilder
    private func gateFailureNote(state: ActionState, retry: @escaping () async -> Void) -> some View {
        if case .failed(let message) = state {
            HStack(spacing: 6) {
                Text(message)
                    .font(.noteText)
                    .foregroundStyle(Color.status(.failed))
                    .lineLimit(2)
                Button("Retry") {
                    Task { await retry() }
                }
                .buttonStyle(RupuButtonStyle.outline)
            }
        }
    }

    // MARK: - Step graph

    @ViewBuilder
    private func stepGraphSection(store: RunDetailStore) -> some View {
        switch store.graph {
        case .loading:
            blockShell { ProgressView().controlSize(.small) }
        case .failed(let message):
            blockShell {
                FailedBlock(subject: "workflow steps", message: message, retry: { await store.loadGraph() })
                    .padding(12)
            }
        case .empty:
            blockShell { Text("No workflow steps").font(.noteText).foregroundStyle(Color.rupuMute) }
        case .content:
            // Perf & interaction arc, Plan 5 Task 3: `layoutGraph` (and its
            // `effectiveLiveStates` gate overlay) moved into
            // `RunDetailStore` as the coalesced derived `graphVM` — this
            // body no longer recomputes the graph inline on every render.
            StepGraphView(
                nodes: store.graphVM,
                selectedID: store.selectedStepID,
                onSelect: { stepID in Task { await store.select(step: stepID) } },
                onSelectUnit: { stepID, index in Task { await store.select(stepID: stepID, unitIndex: index) } }
            )
            .panelStyle(.panel)
        }
    }

    // MARK: - Shared shells

    private func blockShell<Content: View>(@ViewBuilder content: () -> Content) -> some View {
        VStack {
            Spacer(minLength: 0)
            content()
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
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
}
