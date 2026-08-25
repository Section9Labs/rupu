import SwiftUI
import RupuAPI
import RupuStore
import RupuDesign

/// The Agent detail screen (Phase 5A, Task 7), pushed from a Library
/// agents-tab row tap (`.agentDefinition(name:)`): back chevron + breadcrumb,
/// meta chips (scope/model/provider/effort/max tokens/permission tone),
/// description, the raw `.md` source in a mono scroll block, and a page
/// Launch button. Read-only otherwise, per the spec's Phase 5A disposition
/// for Library detail views.
///
/// **A plain one-shot fetch, not a dedicated `Store` class**: unlike
/// `ProjectDetailStore`/`RunDetailStore`, there is nothing here beyond "GET
/// one detail resource and render it" — no mutation, no streaming, no
/// multi-block independence to coordinate. `LibraryStore` already owns the
/// one mutation this phase's Library offers (`setAutoflowEnabled`, which
/// this screen has no need of — agents have no autoflow toggle), so a
/// second store class here would exist purely to wrap a single `await
/// client.agentDetail(name:)` call, adding indirection without adding
/// testable behavior. Mirrors how `ProjectDetailScreen`'s own simpler tabs
/// (`.overview`/`.coverage`) don't get separate store types either.
///
/// **Does NOT need `OverviewScreen`'s cold-launch fix** — same reasoning
/// `ProjectDetailScreen` documents for itself: only ever reached by pushing
/// from `.library`, never a cold-launch route.
public struct AgentDetailScreen: View {
    @Bindable var model: AppModel
    let backend: BackendController
    let name: String
    /// The tapped row's own scope (final-review fix wave, threaded from
    /// `Route.agentDefinition`). `GET /api/agents/:name` (unlike the
    /// Workflow-side `WorkflowDetail`, which decodes NO scope fields at all)
    /// returns a FULL, self-describing `AgentDetail` including its own
    /// `scope`/`scopeKind`/`scopeID` — but that response has no scope query
    /// param to pin it to, so when the same agent name exists at two
    /// scopes, `detail.scope`/`detail.scopeKind`/`detail.scopeID` reflect
    /// whichever record the server's own resolution happened to pick, which
    /// may differ from the row that was actually tapped (a shared,
    /// server-side web gap — same one `WorkflowDetailScreen`'s doc comment
    /// documents). The residual honestly: the meta chips + raw markdown
    /// body below are the server's precedence pick for this name; only
    /// Launch pins the tapped row (`scopeKind`/`scopeID` here, not
    /// `detail`'s).
    let scopeKind: String?
    let scopeID: String?

    @State private var detail: BlockState<AgentDetail> = .loading

    public init(model: AppModel, backend: BackendController, name: String, scopeKind: String? = nil, scopeID: String? = nil) {
        self.model = model
        self.backend = backend
        self.name = name
        self.scopeKind = scopeKind
        self.scopeID = scopeID
    }

    public var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                header
                switch detail {
                case .loading:
                    ProgressView().controlSize(.small)
                case .failed(let message):
                    FailedNote(message: message)
                case .empty:
                    Text("Not found").font(.noteText).foregroundStyle(Color.rupuMute)
                case .content(let value):
                    body(for: value)
                }
            }
            .padding(16)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(Color.rupuBg)
        // `.task(id: name)` — a `navigate(to: .agentDefinition(name:))` to a
        // DIFFERENT name while this screen is already showing (e.g. a
        // future "related agent" link) must refetch, not keep stale
        // content; keyed on `name` alone since there is no backend-client
        // swap concern worth guarding here — a client swap mid-visit to a
        // pushed detail screen is an edge case none of this module's other
        // detail screens guard against either without a `storeClientID`
        // (this screen has no store to rebuild in the first place).
        .task(id: name) {
            await load()
        }
    }

    private func load() async {
        guard let client = backend.client() else { return }
        detail = .loading
        do {
            detail = .content(try await client.agentDetail(name: name))
        } catch {
            guard !isCancellation(error) else { return }
            detail = .failed(String(describing: error))
        }
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

            Text("Library ▸ \(name)")
                .font(.leadText)
                .foregroundStyle(Color.rupuInk)
            Spacer(minLength: 0)
        }
    }

    // MARK: - Content

    private func body(for detail: AgentDetail) -> some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 10) {
                metaChips(detail)
                Spacer(minLength: 0)
                Button("Launch") {
                    // Pinned to the tapped row's scope (`self.scopeKind`/
                    // `scopeID`), NOT `detail.scopeKind`/`scopeID` — see this
                    // screen's own doc comment on why the server's GET
                    // response can't be trusted to be the same-scoped record
                    // the operator actually tapped.
                    model.presentLauncher(kind: .agentRun, name: detail.name, scopeKind: scopeKind, scopeID: scopeID)
                }
                .buttonStyle(RupuButtonStyle.primary)
            }

            if let description = detail.description, !description.isEmpty {
                Text(description)
                    .font(.uiText)
                    .foregroundStyle(Color.rupuDim)
            }

            rawBlock(detail.raw)
        }
    }

    private func metaChips(_ detail: AgentDetail) -> some View {
        HStack(spacing: 6) {
            Badge(detail.scope)
            if let provider = detail.provider {
                Badge(provider)
            }
            if let model = detail.model {
                Badge(model)
            }
            if let effort = detail.effort {
                Badge(effort)
            }
            if let maxTokens = detail.maxTokens {
                Badge("\(maxTokens) tok")
            }
            PermissionBadge(mode: detail.mode)
            Badge(detail.tools.isEmpty ? "unrestricted" : "\(detail.tools.count) tools")
        }
    }

    /// Same "mono block" idiom `TranscriptFeed.jsonBlock` (`RupuRunDetail`)
    /// establishes for the transcript's tool JSON — re-derived locally
    /// rather than shared, that type is private to its own module.
    private func rawBlock(_ raw: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Eyebrow("Source")
            Text(raw)
                .font(.dataMono(11.5))
                .foregroundStyle(Color.rupuInk)
                .textSelection(.enabled)
                .padding(8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.rupuSurface)
                .clipShape(RoundedRectangle(cornerRadius: 4))
        }
    }
}

private struct FailedNote: View {
    let message: String

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("Failed to load")
                .font(.noteText)
                .foregroundStyle(Color.status(.failed))
            Text(message)
                .font(.noteText)
                .foregroundStyle(Color.rupuDim)
                .lineLimit(3)
        }
    }
}
