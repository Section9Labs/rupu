import SwiftUI
import RupuAPI
import RupuStore
import RupuDesign

/// Run-scoped tree-sitter CST viewer for the transcript's "view AST"
/// disclosure (Phase 6B, Task 5) — sibling of `SourcePreview`'s "view
/// source" disclosure; both fetch through the same `SourcePreviewStore`.
/// Recursive `AstNodeRow`, matched-node highlight, and an ancestor-chain
/// auto-expand on load mirror the web `AstTree` component (`crates/rupu-cp/
/// web/src/components/transcript/AstTree.tsx`).
///
/// **Lazy on mount** — same convention as `SourcePreview`: this view's own
/// `.task(id:)` triggers the fetch, so the caller controls "fetch on expand
/// only" by not creating this view until toggled open.
///
/// States: `nil`/`.loading` → "Loading AST…"; `.failed` → the fetch error
/// plus a compact Retry; `.content` with `available == false` (or no
/// `root`) → the server's own `reason` verbatim; `.content` with a `root` →
/// the recursive tree, contained-scroll, the matched node highlighted and
/// its ancestor chain expanded by default, a truthful footer when
/// `truncated == true`.
struct AstTreeView: View {
    let store: SourcePreviewStore
    let path: String
    let line: Int
    let col: Int

    @State private var expanded: Set<String> = []

    init(store: SourcePreviewStore, path: String, line: Int, col: Int) {
        self.store = store
        self.path = path
        self.line = line
        self.col = col
    }

    var body: some View {
        content
            .task(id: "\(path):\(line):\(col)") {
                await store.loadAstIfNeeded(path: path, line: line, col: col)
                seedExpansionIfPossible()
            }
    }

    /// Re-derives the initially-expanded path set from whatever's currently
    /// cached for this key — runs after the `.task`'s fetch (a no-op if
    /// already cached) so this fires whether the response just arrived or
    /// was already sitting in the store from a prior expand.
    private func seedExpansionIfPossible() {
        guard case .content(let response) = store.astState(path: path, line: line, col: col),
              let root = response.root
        else { return }
        expanded = Set(Self.matchedAncestorPaths(root: root, path: "0") ?? ["0"])
    }

    @ViewBuilder
    private var content: some View {
        switch store.astState(path: path, line: line, col: col) {
        case nil, .loading:
            Text("Loading AST…")
                .font(.noteText)
                .foregroundStyle(Color.rupuMute)
        case .failed(let message):
            HStack(spacing: 6) {
                Text("Could not load AST: \(message)")
                    .font(.noteText)
                    .foregroundStyle(Color.status(.failed))
                    .lineLimit(2)
                Button("Retry") {
                    Task {
                        await store.reloadAst(path: path, line: line, col: col)
                        seedExpansionIfPossible()
                    }
                }
                .buttonStyle(RupuButtonStyle.outline)
            }
        case .empty:
            // Structurally unreachable — see `SourcePreview`'s identical
            // note; `BlockState` is a shared generic every switch must
            // exhaust.
            EmptyView()
        case .content(let response):
            if response.available, let root = response.root {
                VStack(alignment: .leading, spacing: 4) {
                    if response.truncated == true {
                        Text("tree truncated (large file)")
                            .font(.metaText)
                            .foregroundStyle(Color.rupuMute)
                    }
                    // Contained scrolling — a CST tree can run deep/wide; it
                    // sits inside its own bounded `ScrollView`, never an
                    // eagerly-rendered tree inside an unbounded parent (the
                    // 5B "eager container" lesson).
                    ScrollView([.vertical, .horizontal]) {
                        VStack(alignment: .leading, spacing: 1) {
                            AstNodeRow(node: root, path: "0", depth: 0, expanded: $expanded)
                        }
                    }
                    .frame(maxHeight: 220)
                }
                .padding(6)
                .panelStyle(.innerCard)
            } else {
                Text(response.reason ?? "AST not available.")
                    .font(.noteText)
                    .foregroundStyle(Color.rupuMute)
            }
        }
    }

    // MARK: - Pure seam (testable via `@testable import RupuRunDetail`, same
    // idiom as `SourcePreview.gutterWidth`)

    /// Depth-first search for the `matched` node, returning the path keys of
    /// every node from the root down to (but not including) the matched
    /// node itself — i.e. the set that must be expanded for the matched
    /// node to be visible. `nil` when no descendant is matched. Direct port
    /// of the web `AstTree`'s `findMatchedAncestorPaths`.
    static func matchedAncestorPaths(root: APIAstNode, path: String) -> [String]? {
        if root.matched { return [] }
        for (index, child) in root.children.enumerated() {
            if let found = matchedAncestorPaths(root: child, path: "\(path).\(index)") {
                return [path] + found
            }
        }
        return nil
    }
}

/// One recursive CST row: a chevron toggle (when the node has children), the
/// node's field name (if any) + kind + `startLine:startCol-endLine:endCol`
/// range, `Color.rupuWarnBg` highlight when `node.matched`, indented by
/// `depth`. Named-vs-anonymous distinction reads as dimmed text rather than
/// a separate toggle (unlike the web viewer's "show anonymous" checkbox —
/// this phase keeps it simple: anonymous nodes are always shown, just
/// visually de-emphasized).
private struct AstNodeRow: View {
    let node: APIAstNode
    let path: String
    let depth: Int
    @Binding var expanded: Set<String>

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 4) {
                if node.children.isEmpty {
                    Color.clear.frame(width: 12, height: 12)
                } else {
                    Button {
                        toggle()
                    } label: {
                        Icon(.chevronDown, size: 10)
                            .foregroundStyle(Color.rupuMute)
                            .rotationEffect(.degrees(isExpanded ? 0 : -90))
                    }
                    .buttonStyle(.plain)
                }
                if let field = node.field {
                    Text("\(field):")
                        .font(.dataMono(10))
                        .foregroundStyle(Color.rupuMute)
                }
                Text(node.kind)
                    .font(.dataMono(10.5))
                    .foregroundStyle(node.named ? Color.rupuInk : Color.rupuMute)
                Text("\(node.startLine):\(node.startCol)-\(node.endLine):\(node.endCol)")
                    .font(.dataMono(9))
                    .foregroundStyle(Color.rupuMute)
            }
            .padding(.leading, CGFloat(depth) * 14)
            .padding(.vertical, 1)
            .background(node.matched ? Color.rupuWarnBg : Color.clear)

            if isExpanded {
                ForEach(Array(node.children.enumerated()), id: \.offset) { index, child in
                    AstNodeRow(node: child, path: "\(path).\(index)", depth: depth + 1, expanded: $expanded)
                }
            }
        }
    }

    private var isExpanded: Bool { expanded.contains(path) }

    private func toggle() {
        if expanded.contains(path) {
            expanded.remove(path)
        } else {
            expanded.insert(path)
        }
    }
}
