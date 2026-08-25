import SwiftUI
import RupuAPI
import RupuStore
import RupuDesign

/// The Agent Run Detail screen (hotfix root cause C): a lightweight,
/// transcript-only view for a standalone agent run — one that
/// `RunDetailScreen` can never correctly show, because it isn't an
/// orchestrator run and `GET /api/runs/:id` 404s for it. Header (back
/// chevron, breadcrumb with the run id) plus a single `TranscriptFeed`, fed
/// by one REST snapshot (`AgentRunDetailStore`). No step graph and none of
/// `RunDetailScreen`'s tabbed panel (`RunDetailTabPanel`: Transcript ·
/// Events · Findings · Netflow) either — this row's data doesn't support
/// them.
///
/// **Strict read-only**, same as every other Phase 2 detail screen this
/// phase.
///
/// **`transcriptPath == nil`**: some agent runs never recorded a
/// transcript at all — and a just-launched run (Phase 3 final-review fix)
/// may simply not have had a path handed to this screen yet, since
/// `LauncherStore`'s `.agentRun` launch route only ever returns a `run_id`.
/// `AgentRunDetailStore` makes one best-effort resolution attempt for the
/// latter case (see that type's doc comment); this screen reads
/// `store.resolvedPath`, not its own `transcriptPath` argument, to decide
/// between the two — "genuinely no transcript" renders "NO TRANSCRIPT
/// RECORDED" only once resolution has actually come up empty, never a
/// spinner that would never resolve.
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
                centeredLabel("Backend not connected")
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
            let newStore = AgentRunDetailStore(runID: runID, transcriptPath: transcriptPath, host: host, client: client)
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
                Icon(.arrowLeft)
                    .foregroundStyle(Color.rupuDim)
            }
            .buttonStyle(.plain)

            Text("Activity ▸ Agent run ▸ \(runID)")
                .font(.noteText)
                .foregroundStyle(Color.rupuDim)
            Spacer(minLength: 0)
            Eyebrow("Read-only")
        }
    }

    // MARK: - Transcript

    @ViewBuilder
    private func transcriptColumn(store: AgentRunDetailStore) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Eyebrow("Transcript")
            switch store.transcript {
            case .loading:
                blockShell { ProgressView().controlSize(.small) }
            case .failed(let message):
                blockShell {
                    FailedBlock(subject: "transcript", message: message, retry: { await store.activate() })
                        .padding(12)
                }
            case .empty where store.resolvedPath == nil:
                blockShell { Text("No transcript recorded").font(.noteText).foregroundStyle(Color.rupuMute) }
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

    private func centeredLabel(_ label: String) -> some View {
        VStack {
            Spacer(minLength: 0)
            Text(label).font(.noteText).foregroundStyle(Color.rupuMute)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
