import SwiftUI
import RupuAPI
import RupuStore
import RupuDesign

/// The Session Detail screen (Task 9, write path added Task 6 of Phase 3;
/// recomposed to a single vertical stack in flows-composition Task 6,
/// mirroring `RunDetailScreen`'s Task 4 recomposition): header (back
/// chevron, breadcrumb, archive/restore overflow) + a mono identity meta
/// line, the session's ordered runs as a full-width stacked panel section
/// (each row navigates to that run's own `AgentRunDetailScreen`), then a
/// transcript feed for whichever run is currently focused — the newest run
/// by default, per `SessionDetailStore.activate()` — with a send box pinned
/// under it. The old fixed-width `runsColumn` (280pt beside the transcript
/// in an `HStack`) is gone; runs now sit above the transcript, full width,
/// same as the graph-then-tabs stack `RunDetailScreen.content` uses. Owns a
/// `SessionDetailStore` lifecycle the same way `RunDetailScreen` owns a
/// `RunDetailStore` — built lazily on first appearance (once
/// `backend.client()` exists), rebuilt whenever `sessionID` changes.
///
/// **Write path (Phase 3, Task 6)**: `sendBox` renders one of three states —
/// the normal TextField+Send box, `"SESSION STOPPED"` (`store.isStopped`),
/// or `"ARCHIVED"` (`store.isArchived`) — never more than one at a time, per
/// the type's own local-optimistic-state doc comment on those two flags.
/// Archive/Restore live behind the header's overflow menu, same convention
/// `RunDetailScreen.actionControls` already established. Every block below
/// (`session`/`runs`) still fails independently: one `.failed` block
/// renders its own failure box without blanking the other.
///
/// **No `deactivate()`**: unlike `RunDetailStore`, `SessionDetailStore`
/// never opens a stream (see that type's doc comment) — there is nothing to
/// tear down `.onDisappear`, so this screen doesn't call anything there.
public struct SessionDetailScreen: View {
    @Bindable var model: AppModel
    let backend: BackendController
    let sessionID: String

    @State private var store: SessionDetailStore?
    @State private var storeSessionID: String?
    @State private var draft: String = ""

    public init(model: AppModel, backend: BackendController, sessionID: String) {
        self.model = model
        self.backend = backend
        self.sessionID = sessionID
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
        .task(id: sessionID) {
            await activate()
        }
    }

    /// Builds (or rebuilds, on a `sessionID` change) the store and activates
    /// it. `storeSessionID` — not the store's own identity — decides
    /// "rebuild vs. reuse", the same pattern `RunDetailScreen.activate`
    /// uses for `storeRunID`.
    private func activate() async {
        guard let client = backend.client() else { return }
        if storeSessionID != sessionID {
            let newStore = SessionDetailStore(sessionID: sessionID, client: client, backend: backend)
            store = newStore
            storeSessionID = sessionID
            draft = ""
        }
        guard let store else { return }
        await store.activate()
    }

    // MARK: - Layout

    private func content(store: SessionDetailStore) -> some View {
        VStack(alignment: .leading, spacing: 16) {
            header(store: store)
            runsSection(store: store)
                .frame(height: 160)
            transcriptColumn(store: store)
                .frame(minHeight: 420)
        }
        .padding(16)
    }

    // MARK: - Header

    private func header(store: SessionDetailStore) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                Button {
                    model.navigateBack()
                } label: {
                    Icon(.arrowLeft)
                        .foregroundStyle(Color.rupuDim)
                }
                .buttonStyle(.plain)

                if case .content(let session) = store.session {
                    Text("Activity ▸ Session ▸ \(session.agentName)")
                        .font(.noteText)
                        .foregroundStyle(Color.rupuDim)
                } else {
                    Text("Activity ▸ Session ▸ \(sessionID)")
                        .font(.noteText)
                        .foregroundStyle(Color.rupuDim)
                }
                Spacer(minLength: 0)
                if store.isArchived {
                    Eyebrow("Archived")
                }
                overflowMenu(store: store)
            }
            overflowFailureNotes(store: store)
            switch store.session {
            case .loading:
                ProgressView().controlSize(.small)
            case .failed(let message):
                // `loadSession()`, NOT `activate()` — the full re-activate
                // would also refetch runs and refocus the newest run,
                // discarding whichever run the user had focused.
                FailedBlock(subject: "session", message: message, retry: { await store.loadSession() })
            case .empty:
                EmptyView()
            case .content(let session):
                identityMetaLine(session: session)
            }
        }
    }

    /// Flows-composition Task 6: the old per-item `factsRow` (an `Eyebrow`
    /// label beside a value, one `HStack` entry per field) is now a single
    /// mono meta line — `AGENT · MODEL · PROVIDER · TURNS · TOKENS · COST`
    /// — same idiom `RunDetailScreen.identityMetaLine` uses for its run/
    /// workspace/permission-mode line: one `Text`, `dataMono(10)`,
    /// `rupuDim`, single-line and truncating rather than wrapping or
    /// overflowing a fixed-width column now that this header is full width.
    private func identityMetaLine(session: APISessionRow) -> some View {
        Text([
            "AGENT \(session.agentName)",
            "MODEL \(session.model)",
            "PROVIDER \(session.providerName)",
            "TURNS \(Fmt.count(Int(session.totalTurns)))",
            "TOKENS \(Fmt.count(Int(session.totalTokensIn + session.totalTokensOut)))",
            "COST \(Fmt.cost(session.usage?.costUSD))",
        ].joined(separator: "  ·  "))
            .font(.dataMono(10))
            .foregroundStyle(Color.rupuDim)
            .lineLimit(1)
            .truncationMode(.tail)
    }

    // MARK: - Header mutations (Phase 3, Task 6)

    /// Archive/Restore behind the header's overflow menu — same convention
    /// `RunDetailScreen.actionControls` established: at most one of the two
    /// verbs is ever relevant at once (`store.isArchived` picks which), and
    /// neither is a frequent action.
    @ViewBuilder
    private func overflowMenu(store: SessionDetailStore) -> some View {
        Menu {
            if store.isArchived {
                Button("Restore") { Task { await store.restore() } }
                    .disabled(isPending(store.pendingActions.state(ActionKey(sessionID, .restore))))
            } else {
                Button("Archive") { Task { await store.archive() } }
                    .disabled(isPending(store.pendingActions.state(ActionKey(sessionID, .archive))))
            }
        } label: {
            Icon(.moreHorizontal)
                .foregroundStyle(Color.rupuDim)
        }
        .menuStyle(.borderlessButton)
        .frame(width: 20)
    }

    private func isPending(_ state: ActionState) -> Bool {
        if case .pending = state { return true }
        return false
    }

    /// Final-review fix: `archive()`/`restore()` failures used to be
    /// invisible the same way `RunDetailScreen`'s header mutations were —
    /// the overflow button's own re-tap is the retry, so a `.failed` state
    /// never rendered anything and the menu item just silently re-enabled.
    /// Mirrors `RunDetailScreen.runVerbFailureNotes`'s message+Retry
    /// convention (itself lifted from `gateFailureNote`), scoped to this
    /// screen's two overflow verbs. At most one of the two keys is ever
    /// relevant per `store.isArchived` (same "at most one at a time"
    /// convention `overflowMenu` already uses), but both are checked here
    /// unconditionally in case a failure lands just as `isArchived` flips.
    @ViewBuilder
    private func overflowFailureNotes(store: SessionDetailStore) -> some View {
        let entries: [(title: String, key: ActionKey, retry: () async -> Void)] = [
            ("Archive", ActionKey(sessionID, .archive), { await store.archive() }),
            ("Restore", ActionKey(sessionID, .restore), { await store.restore() }),
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

    // MARK: - Runs section

    @ViewBuilder
    private func runsSection(store: SessionDetailStore) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Eyebrow("Runs")
            switch store.runs {
            case .loading:
                blockShell { ProgressView().controlSize(.small) }
            case .failed(let message):
                blockShell {
                    FailedBlock(subject: "runs", message: message, retry: { await store.activate() })
                        .padding(12)
                }
            case .empty:
                blockShell { Text("No runs yet").font(.noteText).foregroundStyle(Color.rupuMute) }
            case .content(let rows):
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 6) {
                        ForEach(rows, id: \.runID) { run in
                            runRow(run, focused: run.runID == store.focusedRunID)
                        }
                    }
                }
                .panelStyle(.panel)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func runRow(_ run: APISessionRunRow, focused: Bool) -> some View {
        let status = ActivityStatus.normalize(run.status)
        return Button {
            // Hotfix root cause C (second path): a session's child runs are
            // session-turn agent runs, not orchestrator runs — `GET
            // /api/runs/:id` 404s for them, verified live for exactly this
            // row shape (`run_01M0H2AJA1J0T0JH0ZEAW1YAM7`, source
            // "session", 404 on both hosts). `.agentRunDetail` is the
            // destination that's actually addressable: one REST transcript
            // fetch, same as `ActivityRow`'s own session-turn agent rows.
            // `navigate(to:)`, not a direct `route =` assignment (Phase 3,
            // Task 4): pushes this session detail onto the stack so the
            // pushed screen's back-chevron returns here, not past it.
            model.navigate(to: .agentRunDetail(id: run.runID, transcriptPath: run.transcriptPath, host: nil))
        } label: {
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 6) {
                    Circle()
                        .fill(Color.status(status.tone))
                        .frame(width: 6, height: 6)
                    Text(run.prompt)
                        .font(.uiText)
                        .foregroundStyle(Color.rupuInk)
                        .lineLimit(1)
                        .truncationMode(.tail)
                }
                HStack(spacing: 10) {
                    Text(relativeLabel(run.startedAt))
                        .font(.dataMono(10))
                        .foregroundStyle(Color.rupuDim)
                    Text(Fmt.duration(ms: run.durationMS))
                        .font(.dataMono(10))
                        .foregroundStyle(Color.rupuDim)
                    Text("\(Fmt.count(Int(run.tokensIn + run.tokensOut))) tok")
                        .font(.dataMono(10))
                        .foregroundStyle(Color.rupuDim)
                }
                if let error = run.error {
                    Text(error)
                        .font(.noteText)
                        .foregroundStyle(Color.status(.failed))
                        .lineLimit(2)
                }
            }
            .padding(8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(focused ? Color.rupuInk.opacity(0.06) : Color.clear)
            .clipShape(RoundedRectangle(cornerRadius: 6))
        }
        .buttonStyle(.plain)
    }

    private func relativeLabel(_ iso: String?) -> String {
        guard let date = ActivityRow.parseISO(iso) else { return "—" }
        return Self.relativeFormatter.localizedString(for: date, relativeTo: Date())
    }

    private static let relativeFormatter: RelativeDateTimeFormatter = {
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter
    }()

    // MARK: - Transcript

    private func transcriptColumn(store: SessionDetailStore) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Eyebrow(transcriptLabel(store: store))
                Spacer(minLength: 0)
            }
            TranscriptFeed(events: store.transcript, runID: store.focusedRunID, host: nil)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .panelStyle(.panel)
            sendBox(store: store)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func transcriptLabel(store: SessionDetailStore) -> String {
        guard let focusedRunID = store.focusedRunID else { return "TRANSCRIPT" }
        return "TRANSCRIPT — \(focusedRunID)"
    }

    // MARK: - Send box (Phase 3, Task 6)

    /// Three mutually exclusive states, per the type doc comment's "Write
    /// path" section: a stopped session (`store.isStopped`) and an archived
    /// one (`store.isArchived`) each replace the input box outright with a
    /// plain status label rather than showing it disabled — there's nothing
    /// a disabled box would let the operator do that a label doesn't say
    /// more plainly. Stopped is checked first: a session found stopped
    /// while it also happens to be archived is a corner case neither state
    /// alone fully explains, but "why can't I send" is the more pressing
    /// question in that moment.
    @ViewBuilder
    private func sendBox(store: SessionDetailStore) -> some View {
        if store.isStopped {
            statusOnlyBox("Session stopped")
        } else if store.isArchived {
            statusOnlyBox("Archived")
        } else {
            sendInputBox(store: store)
        }
    }

    private func statusOnlyBox(_ label: String) -> some View {
        Text(label)
            .font(.noteText)
            .foregroundStyle(Color.rupuMute)
            .padding(10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .panelStyle(.panel)
    }

    /// The Send button's own tap is the retry for a failed send — same
    /// "no separate retry control" convention `RunDetailScreen.
    /// actionControls`'s doc comment establishes for Cancel/Pause/Resume —
    /// so a failed attempt just shows its message inline and leaves `draft`
    /// exactly as the operator left it, ready to edit and resend. `draft`
    /// only clears once the key actually reads `.confirmed`.
    ///
    /// Chrome (flows-composition Task 6): the field itself — not the whole
    /// row — carries the `rupuSurface`/1px-`rupuBorder`/radius-7 input
    /// chrome, distinct from the `rupuPanel` the transcript/runs panels
    /// above sit on; the Send button stands beside it, not inside a shared
    /// card, so its `RupuButtonStyle.primary` chrome reads as its own
    /// control.
    private func sendInputBox(store: SessionDetailStore) -> some View {
        let key = ActionKey(sessionID, .send)
        let state = store.pendingActions.state(key)
        let pending = isPending(state)
        let trimmedEmpty = draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty

        return VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .bottom, spacing: 8) {
                TextField("Send a message…", text: $draft, axis: .vertical)
                    .textFieldStyle(.plain)
                    .font(.uiText)
                    .lineLimit(1...4)
                    .disabled(pending)
                    .onSubmit { submitDraft(store: store) }
                    .padding(8)
                    .background(Color.rupuSurface)
                    .clipShape(RoundedRectangle(cornerRadius: 7))
                    .overlay(RoundedRectangle(cornerRadius: 7).stroke(Color.rupuBorder, lineWidth: 1))
                Button {
                    submitDraft(store: store)
                } label: {
                    HStack(spacing: 4) {
                        if pending {
                            ProgressView().controlSize(.mini)
                        }
                        Text("Send")
                    }
                }
                .buttonStyle(RupuButtonStyle.primary)
                .disabled(pending || trimmedEmpty)
            }

            if case .failed(let message) = state {
                Text(message)
                    .font(.noteText)
                    .foregroundStyle(Color.status(.failed))
                    .lineLimit(2)
            }
        }
        .onChange(of: state) { _, newValue in
            if newValue == .confirmed {
                draft = ""
            }
        }
    }

    private func submitDraft(store: SessionDetailStore) {
        Task { await store.send(draft) }
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
