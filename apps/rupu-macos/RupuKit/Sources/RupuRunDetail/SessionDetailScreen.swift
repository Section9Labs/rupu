import SwiftUI
import RupuAPI
import RupuStore
import RupuDesign

/// The Session Detail screen (Task 9): header (back chevron, breadcrumb,
/// read-only banner), a metadata facts row, the session's ordered runs
/// (each row navigates to that run's own `RunDetailScreen`), and a
/// transcript feed for whichever run is currently focused — the newest run
/// by default, per `SessionDetailStore.activate()`. Owns a
/// `SessionDetailStore` lifecycle the same way `RunDetailScreen` owns a
/// `RunDetailStore` — built lazily on first appearance (once
/// `backend.client()` exists), rebuilt whenever `sessionID` changes.
///
/// **Strict read-only**: the header carries an explicit "actions arrive in
/// Phase 3" label — no send box, no dead input affordance. Every block
/// below (`session`/`runs`) fails independently: one `.failed` block
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
                centeredLabel("BACKEND NOT CONNECTED")
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
            let newStore = SessionDetailStore(sessionID: sessionID, client: client)
            store = newStore
            storeSessionID = sessionID
        }
        guard let store else { return }
        await store.activate()
    }

    // MARK: - Layout

    private func content(store: SessionDetailStore) -> some View {
        VStack(alignment: .leading, spacing: 16) {
            header(store: store)
            HStack(alignment: .top, spacing: 12) {
                runsColumn(store: store)
                    .frame(width: Self.runsColumnWidth)
                transcriptColumn(store: store)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .padding(16)
    }

    private static let runsColumnWidth: CGFloat = 280

    // MARK: - Header

    private func header(store: SessionDetailStore) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                Button {
                    model.navigateBack()
                } label: {
                    Image(systemName: "chevron.left")
                        .foregroundStyle(Color.rupuDim)
                }
                .buttonStyle(.plain)

                if case .content(let session) = store.session {
                    MicroLabel("Activity ▸ Session ▸ \(session.agentName)")
                        .foregroundStyle(Color.rupuDim)
                } else {
                    MicroLabel("Activity ▸ Session ▸ \(sessionID)")
                        .foregroundStyle(Color.rupuDim)
                }
                Spacer(minLength: 0)
                MicroLabel("READ-ONLY — SEND ARRIVES IN PHASE 3")
                    .foregroundStyle(Color.rupuMute)
            }
            switch store.session {
            case .loading:
                ProgressView().controlSize(.small)
            case .failed(let message):
                failedContent(message)
            case .empty:
                EmptyView()
            case .content(let session):
                factsRow(session: session)
            }
        }
    }

    private func factsRow(session: APISessionRow) -> some View {
        HStack(spacing: 20) {
            factItem("AGENT", session.agentName)
            factItem("MODEL", session.model)
            factItem("PROVIDER", session.providerName)
            factItem("TURNS", Fmt.count(Int(session.totalTurns)))
            factItem("TOKENS", Fmt.count(Int(session.totalTokensIn + session.totalTokensOut)))
            factItem("COST", Fmt.cost(session.usage?.costUSD))
            Spacer(minLength: 0)
        }
    }

    private func factItem(_ label: String, _ value: String) -> some View {
        HStack(spacing: 6) {
            MicroLabel(label).foregroundStyle(Color.rupuMute)
            Text(value)
                .font(.numeral(size: 11.5))
                .foregroundStyle(Color.rupuInk)
                .lineLimit(1)
                .truncationMode(.middle)
        }
    }

    // MARK: - Runs column

    @ViewBuilder
    private func runsColumn(store: SessionDetailStore) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            MicroLabel("RUNS").foregroundStyle(Color.rupuMute)
            switch store.runs {
            case .loading:
                blockShell { ProgressView().controlSize(.small) }
            case .failed(let message):
                blockShell { failedContent(message) }
            case .empty:
                blockShell { MicroLabel("NO RUNS YET").foregroundStyle(Color.rupuMute) }
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
                        .font(.system(size: 12))
                        .foregroundStyle(Color.rupuInk)
                        .lineLimit(1)
                        .truncationMode(.tail)
                }
                HStack(spacing: 10) {
                    MicroLabel(relativeLabel(run.startedAt))
                        .foregroundStyle(Color.rupuDim)
                    MicroLabel(Fmt.duration(ms: run.durationMS))
                        .foregroundStyle(Color.rupuDim)
                    MicroLabel("\(Fmt.count(Int(run.tokensIn + run.tokensOut))) tok")
                        .foregroundStyle(Color.rupuDim)
                }
                if let error = run.error {
                    Text(error)
                        .font(.system(size: 10.5))
                        .foregroundStyle(Color.status(.fail))
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
                MicroLabel(transcriptLabel(store: store)).foregroundStyle(Color.rupuMute)
                Spacer(minLength: 0)
            }
            TranscriptFeed(events: store.transcript)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .panelStyle(.panel)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func transcriptLabel(store: SessionDetailStore) -> String {
        guard let focusedRunID = store.focusedRunID else { return "TRANSCRIPT" }
        return "TRANSCRIPT — \(focusedRunID)"
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
        VStack(alignment: .leading, spacing: 4) {
            MicroLabel("FAILED TO LOAD").foregroundStyle(Color.status(.fail))
            Text(message)
                .font(.system(size: 11))
                .foregroundStyle(Color.rupuDim)
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
