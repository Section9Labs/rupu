import SwiftUI
import RupuAPI
import RupuStore
import RupuDesign

private enum CodeTabLayout {
    static let treeColumnWidth: CGFloat = 260
}

/// The Project Detail screen's Code tab (Phase 6B, Task 4) — a
/// workspace-relative directory tree on the left, a read-only file viewer
/// on the right, and a top filter field that swaps the tree column for a
/// flat, client-side-filtered file list while the operator is typing.
///
/// **Viewer only — no editing affordances.** Every control this tab exposes
/// is navigational (open a directory, open a file, filter by name); there
/// is no save/write/rename control anywhere here, matching `CodeStore`'s
/// own read-only contract (see that type's doc comment).
///
/// **Selection is screen-local** — unlike the Runs/Sessions/Definitions
/// tabs, nothing here pushes an `AppModel`/`Route` navigation; which
/// directory is browsed and which file is open live entirely in
/// `CodeStore`'s own `currentPath`/`selectedPath`, scoped to this tab.
///
/// **Contained scrolling** — both the tree/filter column and the viewer's
/// line list sit inside their own bounded `ScrollView` (`LazyVStack`
/// inside), never an eagerly-rendered `VStack` inside an unbounded
/// container: the Phase 5B "eager container" lesson (a real source file can
/// run thousands of lines).
public struct CodeTab: View {
    let store: CodeStore

    @State private var filterQuery: String = ""

    public init(store: CodeStore) {
        self.store = store
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            filterField
            Divider()
            HStack(spacing: 0) {
                leftColumn
                    .frame(width: CodeTabLayout.treeColumnWidth)
                    .frame(maxHeight: .infinity)
                Divider()
                viewerColumn
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        // Fires on every keystroke; `CodeStore.loadFilter()`'s own
        // `filter == nil` guard makes every call after the very first
        // keystroke a no-op fetch-wise (see that method's doc comment) —
        // an `onChange` rather than a debounced `.task(id:)` because there
        // is no per-keystroke network cost to protect against here, only a
        // client-side re-filter of whatever `filter.value?.files` already
        // holds.
        .onChange(of: filterQuery) { _, newValue in
            guard !newValue.isEmpty else { return }
            Task { await store.loadFilter() }
        }
    }

    // MARK: - Filter field

    private var filterField: some View {
        HStack(spacing: 8) {
            Icon(.search, size: 13).foregroundStyle(Color.rupuDim)
            TextField("Filter files…", text: $filterQuery)
                .textFieldStyle(.plain)
                .font(.uiText)
                .foregroundStyle(Color.rupuInk)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }

    // MARK: - Left column: tree browser OR filter results

    @ViewBuilder
    private var leftColumn: some View {
        if filterQuery.isEmpty {
            treeBrowser
        } else {
            filterResults
        }
    }

    @ViewBuilder
    private var treeBrowser: some View {
        switch store.tree {
        case .loading:
            centeredLabel("Loading…")
        case .failed(let message):
            failedColumn(message: message) {
                Task { await store.navigate(path: store.currentPath) }
            }
        case .empty:
            // Structurally unreachable — `CodeStore.navigate` never
            // produces `.empty` (see that type's `tree` doc comment) — but
            // `BlockState` is a shared generic every switch must exhaust.
            centeredLabel("No entries")
        case .content(let tree):
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    if Self.showsParentRow(parent: tree.parent) {
                        parentRow(target: tree.parent ?? "")
                    }
                    let entries = Self.sortedEntries(tree.entries)
                    if entries.isEmpty && !Self.showsParentRow(parent: tree.parent) {
                        Text("Empty directory")
                            .font(.noteText)
                            .foregroundStyle(Color.rupuMute)
                            .padding(12)
                    } else {
                        ForEach(entries, id: \.path) { entry in
                            entryRow(entry)
                        }
                    }
                }
            }
        }
    }

    private func parentRow(target: String) -> some View {
        Button {
            Task { await store.navigate(path: target) }
        } label: {
            HStack(spacing: 8) {
                Icon(.arrowLeft, size: 13).foregroundStyle(Color.rupuDim)
                Text("..")
                    .font(.dataMono(12))
                    .foregroundStyle(Color.rupuDim)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 6)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func entryRow(_ entry: APITreeEntry) -> some View {
        let isDir = entry.kind == "dir"
        let isSelected = !isDir && store.selectedPath == entry.path
        return Button {
            Task {
                if isDir {
                    await store.navigate(path: entry.path)
                } else {
                    await store.open(path: entry.path)
                }
            }
        } label: {
            HStack(spacing: 8) {
                Icon(isDir ? .folder : .fileText, size: 13)
                    .foregroundStyle(isDir ? Color.rupuDim : Color.rupuMute)
                Text(entry.name)
                    .font(.dataMono(12))
                    .foregroundStyle(Color.rupuInk)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 6)
            .background(isSelected ? Color.rupuSurfaceActive : Color.clear)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    // MARK: - Left column: filter results

    @ViewBuilder
    private var filterResults: some View {
        switch store.filter {
        case nil, .loading:
            centeredLabel("Loading…")
        case .failed(let message):
            failedColumn(message: message) {
                Task { await store.reloadFilter() }
            }
        case .empty:
            centeredLabel("No files")
        case .content(let list):
            let matches = Self.filteredFiles(list.files, query: filterQuery)
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    if matches.isEmpty {
                        Text("No matches")
                            .font(.noteText)
                            .foregroundStyle(Color.rupuMute)
                            .padding(12)
                    } else {
                        ForEach(matches, id: \.self) { path in
                            filterResultRow(path)
                        }
                    }
                    if let footer = Self.truncationFooterText(truncated: list.truncated) {
                        Text(footer)
                            .font(.metaText)
                            .foregroundStyle(Color.rupuMute)
                            .padding(.horizontal, 12)
                            .padding(.vertical, 6)
                    }
                }
            }
        }
    }

    private func filterResultRow(_ path: String) -> some View {
        let isSelected = store.selectedPath == path
        return Button {
            Task { await store.open(path: path) }
        } label: {
            HStack(spacing: 8) {
                Icon(.fileText, size: 13).foregroundStyle(Color.rupuMute)
                Text(path)
                    .font(.dataMono(12))
                    .foregroundStyle(Color.rupuInk)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 6)
            .background(isSelected ? Color.rupuSurfaceActive : Color.clear)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    // MARK: - Right column: viewer

    @ViewBuilder
    private var viewerColumn: some View {
        switch store.file {
        case nil:
            centeredLabel("Select a file")
        case .loading:
            centeredLabel("Loading…")
        case .failed(let message):
            failedColumn(message: message) {
                guard let path = store.selectedPath else { return }
                Task { await store.open(path: path) }
            }
        case .empty:
            // Structurally unreachable — `CodeStore.open` only ever
            // produces `.content`/`.failed` (an unavailable file is still a
            // `200` decoded into `.content`, see `CodeStore.open`'s doc
            // comment) — exhaustive switch over the shared `BlockState`.
            centeredLabel("No content")
        case .content(let file):
            if file.available {
                availableFileViewer(file)
            } else {
                centeredLabel(Self.unavailableMessage(file))
            }
        }
    }

    private func availableFileViewer(_ file: APIFileContent) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            viewerHeader(file)
            Divider()
            ScrollView([.vertical, .horizontal]) {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(file.lines ?? [], id: \.n) { line in
                        sourceLineRow(line)
                    }
                }
            }
        }
    }

    private func viewerHeader(_ file: APIFileContent) -> some View {
        HStack(spacing: 8) {
            Text(file.path ?? store.selectedPath ?? "")
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuInk)
                .lineLimit(1)
                .truncationMode(.middle)
            if let language = file.language {
                Badge(language)
            }
            Spacer(minLength: 0)
            if let totalLines = file.totalLines {
                Text("\(totalLines) lines")
                    .font(.metaText)
                    .foregroundStyle(Color.rupuMute)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }

    private func sourceLineRow(_ line: APISourceLine) -> some View {
        HStack(spacing: 12) {
            Text("\(line.n)")
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuMute)
                .frame(width: 40, alignment: .trailing)
            Text(line.text)
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuInk)
                .lineLimit(1)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 1)
    }

    // MARK: - Shared

    private func centeredLabel(_ label: String) -> some View {
        VStack {
            Spacer(minLength: 0)
            Text(label).font(.noteText).foregroundStyle(Color.rupuMute)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func failedColumn(message: String, retry: @escaping () -> Void) -> some View {
        VStack(spacing: 10) {
            Spacer(minLength: 0)
            Text("Failed to load")
                .font(.noteText)
                .foregroundStyle(Color.status(.failed))
            Text(message)
                .font(.noteText)
                .foregroundStyle(Color.rupuDim)
                .lineLimit(3)
                .multilineTextAlignment(.center)
                .padding(.horizontal, 12)
            Button("Retry", action: retry)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    // MARK: - Pure static seams (testable via `@testable import
    // RupuProjects` from `CodeTabTests`, same "view-member pure logic gets
    // its own testable static func" idiom `ClaimTableRow`/`RunDetailScreen.
    // unrecognizedStatusRaw` already establish, since a SwiftUI `body`
    // itself can't be meaningfully unit-rendered).

    /// Dirs first, then files; alphabetical (case-insensitive) within each
    /// group — the brief's own "dirs first" ordering, with a stable
    /// tie-break so re-navigating the same directory never visibly reorders.
    static func sortedEntries(_ entries: [APITreeEntry]) -> [APITreeEntry] {
        entries.sorted { lhs, rhs in
            let lhsIsDir = lhs.kind == "dir"
            let rhsIsDir = rhs.kind == "dir"
            if lhsIsDir != rhsIsDir { return lhsIsDir }
            return lhs.name.localizedCaseInsensitiveCompare(rhs.name) == .orderedAscending
        }
    }

    /// The `..` row shows whenever `parent` is non-`nil` — **including when
    /// `parent == ""`** (the workspace root, per `APITreeResult`'s own doc
    /// comment: navigating "up" from a direct child of the root lands on
    /// `""`, a valid, navigable target, not "no parent"). Only a `nil`
    /// parent — meaning `tree` already IS the root's own listing — hides the
    /// row. Never nil-coalesce `parent` away before this check.
    static func showsParentRow(parent: String?) -> Bool {
        parent != nil
    }

    /// Client-side contains-match, case-insensitive — `CodeStore.filter`'s
    /// fetched-once file list, narrowed locally on every keystroke rather
    /// than re-fetched.
    static func filteredFiles(_ files: [String], query: String) -> [String] {
        guard !query.isEmpty else { return files }
        return files.filter { $0.localizedCaseInsensitiveContains(query) }
    }

    /// Truthful truncation disclosure: `nil` (no footer) when the
    /// server-reported file list was NOT truncated; otherwise the exact
    /// wording the brief specifies, verbatim — this describes the
    /// underlying `files` list's own 20,000-cap truncation (`APIFileList.
    /// truncated`), not how many of THOSE files match the current filter
    /// text.
    static func truncationFooterText(truncated: Bool) -> String? {
        truncated ? "+ more (list truncated)" : nil
    }

    /// `available == false`'s honest message — the server's own `reason`
    /// verbatim ("binary file" / a size-cap description / etc.) when
    /// present; a generic-but-still-honest fallback only for the
    /// structurally-unexpected case where `reason` itself is absent (never
    /// a blank pane).
    static func unavailableMessage(_ file: APIFileContent) -> String {
        file.reason ?? "This file is unavailable."
    }
}
