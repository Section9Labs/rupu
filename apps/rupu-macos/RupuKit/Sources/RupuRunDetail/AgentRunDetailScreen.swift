import SwiftUI
import RupuAPI
import RupuStore
import RupuDesign

/// The Agent Run Detail screen (hotfix root cause C): a lightweight,
/// transcript-only view for a standalone agent run — one that
/// `RunDetailScreen` can never correctly show, because it isn't an
/// orchestrator run and `GET /api/runs/:id` 404s for it. Header (back
/// chevron, breadcrumb with the run id) plus a single `TranscriptFeed`, fed
/// by one REST snapshot (`AgentRunDetailStore`). No step graph, no
/// netflow/findings rails — this row's data doesn't support them.
///
/// **Strict read-only**, same as every other Phase 2 detail screen this
/// phase.
///
/// **`transcriptPath == nil`**: some agent runs never recorded a
/// transcript at all. Rendered as an honest "NO TRANSCRIPT RECORDED" label
/// rather than a spinner that would never resolve — `AgentRunDetailStore`
/// never calls the network in that case either.
public struct AgentRunDetailScreen: View {
    @Bindable var model: AppModel
    let backend: BackendController
    let runID: String
    let transcriptPath: String?
    let host: String?

    @State private var store: AgentRunDetailStore?
    @State private var storeRunID: String?

    public init(model: AppModel, backend: BackendController, runID: String, transcriptPath: String?, host: String?) {
        self.model = model
        self.backend = backend
        self.runID = runID
        self.transcriptPath = transcriptPath
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
    }

    /// Builds (or rebuilds, on a `runID` change) the store and activates
    /// it. `storeRunID` — not the store's own identity — decides "rebuild
    /// vs. reuse", the same pattern `RunDetailScreen.activate`/
    /// `SessionDetailScreen.activate` use.
    private func activate() async {
        guard let client = backend.client() else { return }
        if storeRunID != runID {
            let newStore = AgentRunDetailStore(transcriptPath: transcriptPath, host: host, client: client)
            store = newStore
            storeRunID = runID
        }
        guard let store else { return }
        await store.activate()
    }

    // MARK: - Layout

    private func content(store: AgentRunDetailStore) -> some View {
        VStack(alignment: .leading, spacing: 16) {
            header
            transcriptColumn(store: store)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .padding(16)
    }

    // MARK: - Header

    private var header: some View {
        HStack(spacing: 10) {
            Button {
                model.navigateBack()
            } label: {
                Image(systemName: "chevron.left")
                    .foregroundStyle(Color.rupuDim)
            }
            .buttonStyle(.plain)

            MicroLabel("Activity ▸ Agent run ▸ \(runID)")
                .foregroundStyle(Color.rupuDim)
            Spacer(minLength: 0)
            MicroLabel("READ-ONLY")
                .foregroundStyle(Color.rupuMute)
        }
    }

    // MARK: - Transcript

    @ViewBuilder
    private func transcriptColumn(store: AgentRunDetailStore) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            MicroLabel("TRANSCRIPT").foregroundStyle(Color.rupuMute)
            switch store.transcript {
            case .loading:
                blockShell { ProgressView().controlSize(.small) }
            case .failed(let message):
                blockShell { failedContent(message) }
            case .empty where transcriptPath == nil:
                blockShell { MicroLabel("NO TRANSCRIPT RECORDED").foregroundStyle(Color.rupuMute) }
            case .empty:
                TranscriptFeed(events: [])
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .panelStyle(.panel)
            case .content(let events):
                TranscriptFeed(events: events)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .panelStyle(.panel)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
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
