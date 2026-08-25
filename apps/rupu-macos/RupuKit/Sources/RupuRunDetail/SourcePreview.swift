import SwiftUI
import RupuAPI
import RupuStore
import RupuDesign

/// Run-scoped source-file slice for the transcript's "view source"
/// disclosure (Phase 6B, Task 5) — an `ast_grep` match row's own file:line
/// reference expands into one of these. Sibling of `AstTreeView`'s "view
/// AST" disclosure; both fetch through the same `SourcePreviewStore`.
///
/// **Lazy on mount, not on init** — this view's own `.task(id:)` is what
/// triggers `SourcePreviewStore.loadSourceIfNeeded`, so a caller controls
/// "fetch on expand only" simply by not creating this view until the row's
/// disclosure is toggled open (same convention the web `SourcePreview`
/// establishes via its own mount-triggered `useEffect`).
///
/// States: `nil`/`.loading` → "Loading source…"; `.failed` → the fetch
/// error plus a compact Retry (this store is new — unlike the older
/// `CodeStore`-backed viewer, which defers a retry affordance, see
/// `CodeTab.failedColumn`); `.content` with `available == false` → the
/// server's own `reason` verbatim (never invented — a remote run always
/// reads exactly `"Source preview is not available for remote-host runs
/// yet."`); `.content` with `available == true` → a numbered, contained-
/// scroll slice with the `targetLine` row highlighted.
struct SourcePreview: View {
    let store: SourcePreviewStore
    let path: String
    let line: Int

    init(store: SourcePreviewStore, path: String, line: Int) {
        self.store = store
        self.path = path
        self.line = line
    }

    var body: some View {
        content
            .task(id: "\(path):\(line)") {
                await store.loadSourceIfNeeded(path: path, line: line)
            }
    }

    @ViewBuilder
    private var content: some View {
        switch store.sourceState(path: path, line: line) {
        case nil, .loading:
            Text("Loading source…")
                .font(.noteText)
                .foregroundStyle(Color.rupuMute)
        case .failed(let message):
            HStack(spacing: 6) {
                Text("Could not load source: \(message)")
                    .font(.noteText)
                    .foregroundStyle(Color.status(.failed))
                    .lineLimit(2)
                Button("Retry") {
                    Task { await store.reloadSource(path: path, line: line) }
                }
                .buttonStyle(RupuButtonStyle.outline)
            }
        case .empty:
            // Structurally unreachable — the store never produces `.empty`
            // for this key (see `SourcePreviewStore.sourceState`'s doc
            // comment); `BlockState` is a shared generic every switch must
            // exhaust.
            EmptyView()
        case .content(let slice):
            if slice.available {
                availableSlice(slice)
            } else {
                Text(slice.reason ?? "Source not available.")
                    .font(.noteText)
                    .foregroundStyle(Color.rupuMute)
            }
        }
    }

    private func availableSlice(_ slice: APISourceSlice) -> some View {
        let gutterWidth = Self.gutterWidth(totalLines: slice.totalLines)
        return VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                Text(slice.path ?? path)
                    .font(.dataMono(10))
                    .foregroundStyle(Color.rupuDim)
                    .lineLimit(1)
                    .truncationMode(.middle)
                if let language = slice.language {
                    Badge(language)
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 8)
            .padding(.top, 6)
            .padding(.bottom, 4)

            // Contained scrolling (the 5B "eager container" lesson) — a
            // source slice sits inside its own bounded `ScrollView`, never
            // an eagerly-rendered list inside an unbounded parent.
            ScrollView([.vertical, .horizontal]) {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(slice.lines ?? [], id: \.n) { sourceLine in
                        lineRow(sourceLine, target: slice.targetLine, gutterWidth: gutterWidth)
                    }
                }
            }
            .frame(maxHeight: 220)
        }
        .panelStyle(.innerCard)
    }

    private func lineRow(_ sourceLine: APISourceLine, target: Int?, gutterWidth: CGFloat) -> some View {
        let isTarget = sourceLine.n == target
        return HStack(spacing: 10) {
            Text("\(sourceLine.n)")
                .font(.dataMono(10.5))
                .foregroundStyle(Color.rupuMute)
                .frame(width: gutterWidth, alignment: .trailing)
            Text(sourceLine.text)
                .font(.dataMono(10.5))
                .foregroundStyle(Color.rupuInk)
                .lineLimit(1)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 1)
        .background(isTarget ? Color.rupuWarnBg : Color.clear)
    }

    // MARK: - Pure seam (testable via `@testable import RupuRunDetail`, the
    // same "view-member pure logic gets its own testable static func" idiom
    // `CodeTab.lineNumberGutterWidth`/`RunDetailScreen.
    // unrecognizedStatusRaw` already establish — reimplemented locally
    // rather than reused since `CodeTab`'s own copy is internal to
    // `RupuProjects`, a different module).

    /// The line-number gutter's width, sized from `totalLines`'s own digit
    /// count so a large file's line numbers are never clipped — same
    /// `8pt`/digit-plus-floor shape `CodeTab.lineNumberGutterWidth` uses,
    /// tuned to this view's slightly smaller `dataMono(10.5)` gutter text.
    static func gutterWidth(totalLines: Int?) -> CGFloat {
        let digits = String(totalLines ?? 1).count
        return max(28, CGFloat(digits) * 7 + 12)
    }
}
