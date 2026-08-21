import SwiftUI
import RupuAPI
import RupuStore
import RupuDesign

/// The Run Detail screen (Task 8): header (back chevron, breadcrumb, status
/// pill, facts row), a strict-read-only awaiting banner, the live step
/// graph, the transcript feed for whichever step is currently focused, and
/// the netflow/findings rails. Owns a `RunDetailStore` lifecycle the same
/// way `ActivityScreen` owns an `ActivityStore` — built lazily on first
/// appearance (once `backend.client()` exists), rebuilt whenever `runID`
/// changes (navigating from one run's detail straight to another's), and
/// `deactivate()`d `.onDisappear`.
///
/// **Strict read-only**: the awaiting banner renders the gate's prompt and
/// an honest "actions arrive in Phase 3" label — no approve/reject buttons,
/// no dead controls. Every block below (`detail`/`graph`/`netflow`/
/// `findings`) fails independently: one `.failed` block renders its own
/// failure box without blanking the other three.
public struct RunDetailScreen: View {
    @Bindable var model: AppModel
    let backend: BackendController
    let runID: String
    let host: String?

    @State private var store: RunDetailStore?
    @State private var storeRunID: String?

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
                centeredLabel("BACKEND NOT CONNECTED")
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

    /// Builds (or rebuilds, on a `runID` change) the store and activates it.
    /// `storeRunID` — not the store's own identity — is what decides
    /// "rebuild vs. reuse": `RunDetailStore` doesn't expose its `runID`
    /// publicly (it's plumbing, not UI-relevant state), so this screen keeps
    /// its own record of which run the current `store` was built for.
    private func activate() async {
        guard let client = backend.client() else { return }
        if storeRunID != runID {
            store?.deactivate()
            let newStore = RunDetailStore(runID: runID, host: host, client: client, backend: backend)
            store = newStore
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
                .frame(height: 140)
            HStack(alignment: .top, spacing: 12) {
                transcriptColumn(store: store)
                ScrollView {
                    RailColumn(detail: store.detail, netflow: store.netflow, findings: store.findings)
                }
                .frame(width: RailColumn.width)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
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
                    Image(systemName: "chevron.left")
                        .foregroundStyle(Color.rupuDim)
                }
                .buttonStyle(.plain)

                if case .content(let detail) = store.detail {
                    MicroLabel("Activity ▸ \(detail.run.workflowName)")
                        .foregroundStyle(Color.rupuDim)
                    Spacer(minLength: 0)
                    statusPill(detail.run.status)
                    actionControls(store: store)
                } else {
                    MicroLabel("Activity ▸ \(runID)")
                        .foregroundStyle(Color.rupuDim)
                    Spacer(minLength: 0)
                }
            }
            if case .content(let detail) = store.detail {
                factsRow(detail: detail)
            }
        }
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
                    Image(systemName: "ellipsis.circle")
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
        .disabled(pending)
    }

    private func isPending(_ state: ActionState) -> Bool {
        if case .pending = state { return true }
        return false
    }

    private func statusPill(_ rawStatus: String) -> some View {
        let status = ActivityStatus.normalize(rawStatus)
        return HStack(spacing: 6) {
            Circle()
                .fill(Color.status(status.tone))
                .frame(width: 6, height: 6)
            MicroLabel(statusLabel(status))
                .foregroundStyle(Color.rupuDim)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
        .background(Color.status(status.tone).opacity(0.12))
        .clipShape(Capsule())
    }

    /// Local vocabulary rather than reusing `RupuActivity`'s
    /// `ActivityStatus.displayLabel` — that extension lives in a module this
    /// target has no dependency edge to (`RupuRunDetail` depends on
    /// `RupuAPI`/`RupuStore`/`RupuDesign` only, per `Package.swift`), and one
    /// small switch isn't worth adding one.
    private func statusLabel(_ status: ActivityStatus) -> String {
        switch status {
        case .pending: "Pending"
        case .running: "Running"
        case .completed: "Completed"
        case .failed: "Failed"
        case .awaiting: "Awaiting"
        case .rejected: "Rejected"
        case .cancelled: "Cancelled"
        case .paused: "Paused"
        case .unknown(let raw): raw
        }
    }

    private func factsRow(detail: APIRunDetail) -> some View {
        HStack(spacing: 20) {
            factItem("STARTED", relativeLabel(detail.run.startedAt))
            factItem("DURATION", durationLabel(detail.run))
            factItem("TOKENS", Fmt.count(Int(detail.usage.totalTokens)))
            factItem("COST", Fmt.cost(detail.usage.costUSD))
            Spacer(minLength: 0)
        }
    }

    private func factItem(_ label: String, _ value: String) -> some View {
        HStack(spacing: 6) {
            MicroLabel(label).foregroundStyle(Color.rupuMute)
            Text(value)
                .font(.numeral(size: 11.5))
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
                VStack(alignment: .leading, spacing: 10) {
                    ForEach(detail.run.awaiting, id: \.stepID) { gate in
                        gateRow(store: store, gate: gate)
                    }
                }
                .padding(12)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.status(.waiting).opacity(0.08))
                .clipShape(RoundedRectangle(cornerRadius: 8))
                .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color.status(.waiting).opacity(0.3), lineWidth: 1))
            }
        }
    }

    private func gateRow(store: RunDetailStore, gate: APIAwaitingGate) -> some View {
        let approveKey = ActionKey(runID, .approve)
        let rejectKey = ActionKey(runID, .reject)
        let approveState = store.pendingActions.state(approveKey)
        let rejectState = store.pendingActions.state(rejectKey)
        let approvePending = isPending(approveState)
        let rejectPending = isPending(rejectState)
        let anyPending = approvePending || rejectPending

        return VStack(alignment: .leading, spacing: 6) {
            MicroLabel("Awaiting — \(gate.stepID)")
                .foregroundStyle(Color.status(.waiting))
            Text(gate.prompt ?? "Approval requested")
                .font(.system(size: 12.5))
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
                .buttonStyle(.borderedProminent)
                .tint(Color.rupuBrand)
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
                .disabled(anyPending)
            }

            gateFailureNote(state: approveState) { await store.approve(gate: gate.stepID) }
            gateFailureNote(state: rejectState) { await store.reject(gate: gate.stepID) }

            if store.pendingActions.isStale(approveKey) || store.pendingActions.isStale(rejectKey) {
                MicroLabel("STILL PENDING — THIS MAY BE STUCK")
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
                    .font(.system(size: 11))
                    .foregroundStyle(Color.status(.fail))
                    .lineLimit(2)
                Button("Retry") {
                    Task { await retry() }
                }
                .buttonStyle(.plain)
                .foregroundStyle(Color.rupuBrandHi)
                .font(.system(size: 11, weight: .semibold))
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
            blockShell { failedContent(message) }
        case .empty:
            blockShell { MicroLabel("NO WORKFLOW STEPS").foregroundStyle(Color.rupuMute) }
        case .content(let g):
            StepGraphView(nodes: layoutGraph(
                nodes: g.workflow.steps,
                results: g.stepResults,
                units: g.units,
                liveStates: effectiveLiveStates(store: store)
            ))
            .panelStyle(.panel)
        }
    }

    /// `store.liveStates` (event-driven) wins outright per step; a gate the
    /// run is *currently* parked on (`RunRecord.awaiting`) fills in
    /// `.gatePending` for any step that has no live entry at all — the
    /// remote case (no stream, so `liveStates` never gets anything) per the
    /// brief, and also a local run whose gate was already parked before this
    /// screen was ever opened (no live `stepAwaitingApproval` event to
    /// replay). Never overrides an existing live entry, so a later
    /// live-confirmed transition for that same step always wins.
    private func effectiveLiveStates(store: RunDetailStore) -> [String: NodeState] {
        var states = store.liveStates
        guard case .content(let detail) = store.detail else { return states }
        for gate in detail.run.awaiting where states[gate.stepID] == nil {
            states[gate.stepID] = .gatePending
        }
        return states
    }

    // MARK: - Transcript

    private func transcriptColumn(store: RunDetailStore) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                MicroLabel("TRANSCRIPT").foregroundStyle(Color.rupuMute)
                Spacer(minLength: 0)
                transcriptLiveIndicator(store: store)
            }
            TranscriptFeed(events: store.transcript)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .panelStyle(.panel)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    @ViewBuilder
    private func transcriptLiveIndicator(store: RunDetailStore) -> some View {
        if store.isRemote {
            MicroLabel("REMOTE STREAMING LANDS WITH FLEET (PHASE 5)")
                .foregroundStyle(Color.rupuMute)
        } else if store.transcriptTailActive {
            HStack(spacing: 6) {
                Circle().fill(Color.status(.run)).frame(width: 6, height: 6)
                MicroLabel("LIVE").foregroundStyle(Color.status(.run))
            }
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

    private func failedContent(_ message: String) -> some View {
        VStack(spacing: 4) {
            MicroLabel("FAILED TO LOAD").foregroundStyle(Color.status(.fail))
            Text(message)
                .font(.system(size: 11))
                .foregroundStyle(Color.rupuDim)
                .multilineTextAlignment(.center)
                .lineLimit(2)
        }
    }

    private func centeredLabel(_ label: String) -> some View {
        VStack {
            Spacer(minLength: 0)
            MicroLabel(label).foregroundStyle(Color.rupuMute)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
