import SwiftUI
import AppKit
import RupuAPI
import RupuStore
import RupuDesign

/// Sortable keys for the Projects list (Phase 5A, Task 5) — `ListSort`'s
/// generic `Key` (`RupuDesign/ListSort.swift`), a screen-owned parallel to
/// `RupuStore.ActivitySort.Key` per that generic type's own doc comment ("a
/// deliberate parallel implementation of the same contract for any other
/// sortable list ... not a shared base type the two funnel through").
public enum ProjectsSortKey: Hashable, CaseIterable, Sendable {
    case name, runs, lastRun, spend
}

private enum ProjectsListLayout {
    static let runs: CGFloat = 64
    static let lastRun: CGFloat = 104
    static let spend: CGFloat = 72
}

/// The Projects list screen (Phase 5A, Task 5), replacing the `.projects`
/// placeholder: a single sortable table of every registered workspace —
/// name, run count, last-run timestamp, spend — tapping a row pushes
/// `.projectDetail(wsID:)`. Owns a `ProjectsStore` lifecycle the same way
/// `RunDetailScreen` owns a `RunDetailStore`: built lazily once
/// `backend.client()` exists, rebuilt on a backend client swap, activated on
/// appear.
///
/// **Does NOT need `OverviewScreen`'s cold-launch `.onChange(of: backend.
/// health)` fix**: `.overview` is `AppModel.route`'s default and the only
/// route a cold launch can render before `backend.client()` resolves (see
/// that fix's own doc comment on `RunDetailScreen`) — `.projects` is only
/// ever reached by a sidebar click, well after the shell's own connection
/// attempt has already resolved, same reasoning `RunDetailScreen` documents
/// for itself.
public struct ProjectsScreen: View {
    @Bindable var model: AppModel
    let backend: BackendController

    @State private var store: ProjectsStore?
    @State private var storeClientID: ObjectIdentifier?
    @State private var sort = ListSort<ProjectsSortKey>(key: .lastRun, ascending: false)

    public init(model: AppModel, backend: BackendController) {
        self.model = model
        self.backend = backend
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
        .task {
            await activate()
        }
    }

    /// Builds (or rebuilds, on a backend client swap — embedded/remote
    /// switch, reconnect, restart) the store and activates it. Same
    /// `storeClientID` recipe `RunDetailScreen`/`OverviewScreen` already
    /// use — see `BackendController.clientIdentity()`'s doc comment.
    private func activate() async {
        guard let client = backend.client() else { return }
        let clientID = backend.clientIdentity()
        if store == nil || storeClientID != clientID {
            store = ProjectsStore(client: client)
            storeClientID = clientID
        }
        guard let store else { return }
        await store.activate()
    }

    // MARK: - Layout

    private func content(store: ProjectsStore) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            switch store.rows {
            case .loading:
                loadingBlock
            case .failed(let message):
                FailedBlock(subject: "projects", message: message, retry: { await store.activate() })
            case .empty:
                emptyBlock
            case .content(let rows):
                table(rows: rows)
            }
        }
        .padding(16)
    }

    private func table(rows: [APIProjectRow]) -> some View {
        let sorted = sortRows(rows, sort: sort, value: Self.sortValue)
        return VStack(alignment: .leading, spacing: 0) {
            SortableHeaderRow(columns: columns, sort: $sort)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
            Divider()
            List(sorted, id: \.wsID) { row in
                ProjectListRow(row: row)
                    .listRowInsets(EdgeInsets(top: 0, leading: 0, bottom: 0, trailing: 0))
                    .listRowSeparator(.visible)
                    .contentShape(Rectangle())
                    .onTapGesture { model.navigate(to: .projectDetail(wsID: row.wsID)) }
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
        }
        .panelStyle(.panel)
    }

    private var columns: [SortableColumn<ProjectsSortKey>] {
        [
            SortableColumn(key: .name, label: "Name", width: nil),
            SortableColumn(key: .runs, label: "Runs", width: ProjectsListLayout.runs, alignment: .trailing),
            SortableColumn(key: .lastRun, label: "Last run", width: ProjectsListLayout.lastRun, alignment: .trailing),
            SortableColumn(key: .spend, label: "Spend", width: ProjectsListLayout.spend, alignment: .trailing),
        ]
    }

    /// `.text`/`.number`/`.date` per `sortRows`'s null-discipline contract —
    /// a project with no runs yet (`runCount == nil`, `lastRunAt == nil`,
    /// unpriced `costUSD == nil`) always sorts last on that column,
    /// regardless of direction.
    private static func sortValue(_ row: APIProjectRow, _ key: ProjectsSortKey) -> ListSortValue {
        switch key {
        case .name: .text(row.name)
        case .runs: .number(row.runCount.map(Double.init))
        case .lastRun: .date(ActivityRow.parseISO(row.lastRunAt))
        case .spend: .number(row.usage.costUSD)
        }
    }

    private var loadingBlock: some View {
        HStack {
            Spacer(minLength: 0)
            ProgressView().controlSize(.small)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, minHeight: 120)
        .panelStyle(.panel)
    }

    private var emptyBlock: some View {
        HStack {
            Spacer(minLength: 0)
            Text("No projects registered").font(.noteText).foregroundStyle(Color.rupuMute)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, minHeight: 120)
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

/// One project row: name (with a folder glyph — same `.folderGit2` icon
/// `Sidebar` uses for the Projects rail item), run count, last-run relative
/// timestamp, spend.
private struct ProjectListRow: View {
    let row: APIProjectRow

    var body: some View {
        HStack(spacing: 0) {
            HStack(spacing: 8) {
                Icon(.folderGit2, size: 13).foregroundStyle(Color.rupuDim)
                Text(row.name)
                    .foregroundStyle(Color.rupuInk)
                    .lineLimit(1)
                    .truncationMode(.tail)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.trailing, 8)

            Text(Fmt.count(row.runCount))
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuInk)
                .frame(width: ProjectsListLayout.runs, alignment: .trailing)
                .padding(.trailing, 8)

            Text(lastRunLabel)
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuDim)
                .frame(width: ProjectsListLayout.lastRun, alignment: .trailing)
                .padding(.trailing, 8)

            Text(Fmt.cost(row.usage.costUSD))
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuInk)
                .frame(width: ProjectsListLayout.spend, alignment: .trailing)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .onHover { hovering in
            if hovering {
                NSCursor.pointingHand.push()
            } else {
                NSCursor.pop()
            }
        }
    }

    private var lastRunLabel: String {
        guard let date = ActivityRow.parseISO(row.lastRunAt) else { return "—" }
        return Self.relativeFormatter.localizedString(for: date, relativeTo: Date())
    }

    private static let relativeFormatter: RelativeDateTimeFormatter = {
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter
    }()
}
