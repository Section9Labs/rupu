import SwiftUI
import RupuDesign
import RupuFlowKit

// The inspector rail's Blocks tab (macOS Workflow Builder, Task 12): a
// filterable card grid over `RupuFlowKit.blockCatalog` (spec §4, web's
// `NodePalette.tsx`), split into WORK/ORCHESTRATION sections by
// `kindVisual(_:).family`, a detail card for the selected kind ("Add to
// canvas"), and a custom pointer-tracked drag-to-canvas gesture — SwiftUI's
// `NSItemProvider` drag-and-drop isn't needed for an in-window drag, so this
// reports raw pointer points up to `WorkflowBuilderScreen` instead, which
// owns the shared `paletteDrag` ghost state and the drop-target conversion.

// MARK: - Pure helpers (tested directly, no SwiftUI render pass)

/// Case-insensitive substring match against a catalog entry's label, kind
/// tagline (`kindVisual(_:).tagline` — RupuFlowKit stays the single source
/// for that string, see `KindVisuals.swift`), or what-blurb. A
/// whitespace-only (or empty) query matches every entry, in `catalog`'s own
/// order — never reshuffled by relevance.
func filteredCatalog(query: String, catalog: [BlockCatalogEntry] = blockCatalog) -> [BlockCatalogEntry] {
    let q = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    guard !q.isEmpty else { return catalog }
    return catalog.filter { entry in
        entry.label.lowercased().contains(q)
            || kindVisual(entry.kind).tagline.lowercased().contains(q)
            || entry.what.lowercased().contains(q)
    }
}

/// Splits `catalog` into the rail's two sections — membership DERIVED from
/// `kindVisual(_:).family`, never a locally hardcoded kind list, so the
/// palette tracks `KindVisuals.swift` automatically if a kind's family ever
/// moves. Each section preserves `catalog`'s own relative order.
func catalogSections(_ catalog: [BlockCatalogEntry]) -> (work: [BlockCatalogEntry], orchestration: [BlockCatalogEntry]) {
    let work = catalog.filter { kindVisual($0.kind).family == .work }
    let orchestration = catalog.filter { kindVisual($0.kind).family == .orchestration }
    return (work, orchestration)
}

/// The detail card's "Add to canvas" drop point. True viewport-center needs
/// scroll-offset plumbing `CanvasView`'s plain `ScrollView` doesn't expose
/// (see `canvasPoint(fromBuilderPoint:canvasFrame:scrollOffset:)`'s doc
/// comment below) — this phase uses a fixed point instead: the rightmost
/// existing node's x plus clearance for one node width and a gap, so
/// repeated adds lay out left-to-right rather than stacking exactly on top
/// of each other; `(200, 200)` — comfortably inside the default viewport —
/// when the graph is empty.
func addToCanvasDropPoint(nodes: [GraphNode]) -> CGPoint {
    guard let maxX = nodes.map(\.position.x).max() else {
        return CGPoint(x: 200, y: 200)
    }
    return CGPoint(x: maxX + 260, y: 200)
}

/// Converts a point in the screen-wide `"builder"` named coordinate space
/// (where the drag gesture on a palette card reports its pointer, and where
/// the canvas's own frame is captured via `.onGeometryChange`) into canvas
/// CONTENT coordinates — `nil` when the point falls outside the canvas's
/// frame (a drop that never reached the canvas).
///
/// `scrollOffset` is the canvas `ScrollView`'s content offset if the caller
/// has it; `CanvasView` currently exposes none (SwiftUI's `ScrollView`
/// doesn't surface its offset without extra plumbing this task doesn't
/// add), so every real call site passes `.zero` — a drop therefore lands in
/// VIEWPORT coordinates (correct only when the canvas hasn't been scrolled).
/// The result is clamped to `>= 0` on both axes either way, matching
/// `CanvasView.handleMoveEnded`'s own "never negative" invariant for a node
/// position.
func canvasPoint(fromBuilderPoint point: CGPoint, canvasFrame: CGRect, scrollOffset: CGPoint) -> CGPoint? {
    guard canvasFrame.contains(point) else { return nil }
    let local = CGPoint(x: point.x - canvasFrame.minX + scrollOffset.x, y: point.y - canvasFrame.minY + scrollOffset.y)
    return CGPoint(x: max(0, local.x), y: max(0, local.y))
}

// MARK: - PaletteTab

/// The rail's "Blocks" tab body. `store` is only needed for `addNode` (the
/// detail card's button) and to read `graph.nodes` for
/// `addToCanvasDropPoint`; the drag-to-canvas gesture itself reports raw
/// points up through `onDragChanged`/`onDragEnded` rather than mutating
/// `store` directly, so `WorkflowBuilderScreen` stays the one place that
/// decides whether a drop landed on the canvas.
struct PaletteTab: View {
    @Bindable var store: BuilderStore
    let onDragChanged: (StepKind, CGPoint) -> Void
    let onDragEnded: (StepKind, CGPoint) -> Void

    @State private var query = ""
    @State private var selectedKind: StepKind?

    private var sections: (work: [BlockCatalogEntry], orchestration: [BlockCatalogEntry]) {
        catalogSections(filteredCatalog(query: query))
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                filterField

                if !sections.work.isEmpty {
                    section(title: "WORK", entries: sections.work)
                }
                if !sections.orchestration.isEmpty {
                    section(title: "ORCHESTRATION", entries: sections.orchestration)
                }
                if sections.work.isEmpty && sections.orchestration.isEmpty {
                    Text("No blocks match “\(query)”.")
                        .font(.noteText)
                        .foregroundStyle(Color.rupuMute)
                }

                if let selectedKind, let entry = blockCatalog.first(where: { $0.kind == selectedKind }) {
                    detailCard(entry)
                }
            }
            .padding(12)
        }
    }

    // MARK: - Filter field

    private var filterField: some View {
        HStack(spacing: 6) {
            Icon(.search, size: 12).foregroundStyle(Color.rupuDim)
            TextField("Filter blocks…", text: $query)
                .textFieldStyle(.plain)
                .font(.uiText)
                .foregroundStyle(Color.rupuInk)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
        .background(Color.rupuBg)
        .overlay(RoundedRectangle(cornerRadius: 5).stroke(Color.rupuBorder, lineWidth: 1))
    }

    // MARK: - Section + grid

    private func section(title: String, entries: [BlockCatalogEntry]) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Eyebrow(title)
            LazyVGrid(columns: [GridItem(.flexible(), spacing: 8), GridItem(.flexible(), spacing: 8)], spacing: 8) {
                ForEach(entries, id: \.kind) { entry in
                    PaletteCard(
                        entry: entry,
                        selected: selectedKind == entry.kind,
                        onSelect: { selectedKind = entry.kind },
                        onDragChanged: { point in onDragChanged(entry.kind, point) },
                        onDragEnded: { point in onDragEnded(entry.kind, point) }
                    )
                }
            }
        }
    }

    // MARK: - Detail card

    private func detailCard(_ entry: BlockCatalogEntry) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Eyebrow(entry.label)

            Text(entry.what)
                .font(.uiText)
                .foregroundStyle(Color.rupuDim)
                .fixedSize(horizontal: false, vertical: true)

            if !entry.requiredFields.isEmpty {
                requiredFieldChips(entry.requiredFields)
            }

            Text(entry.example)
                .font(.dataMono(10))
                .foregroundStyle(Color.rupuInk)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(8)
                .background(Color.rupuBg)
                .clipShape(RoundedRectangle(cornerRadius: 5))

            Button {
                store.addNode(kind: entry.kind, at: addToCanvasDropPoint(nodes: store.graph.nodes))
            } label: {
                Text("Add to canvas")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(RupuButtonStyle.primary)
        }
        .padding(12)
        .panelStyle(.innerCard)
    }

    private func requiredFieldChips(_ fields: [String]) -> some View {
        // Required fields top out at 3 across every `blockCatalog` entry
        // (see `KindVisuals.swift`) — a plain wrapping `HStack` never
        // overflows the 320pt rail at that count.
        HStack(spacing: 6) {
            ForEach(fields, id: \.self) { field in
                Text("\(field)*")
                    .font(.dataMono(10))
                    .foregroundStyle(Color.rupuInk)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 3)
                    .background(Color.rupuSurface)
                    .clipShape(RoundedRectangle(cornerRadius: 4))
            }
        }
    }
}

/// One palette card: mini silhouette preview, label, tagline. Its own
/// `View` (not a helper function on `PaletteTab`) so `@State private var
/// hovering` has a stable identity to attach to across re-renders — `ForEach
/// (entries, id: \.kind)` keeps this struct's identity keyed on `kind`.
private struct PaletteCard: View {
    let entry: BlockCatalogEntry
    let selected: Bool
    let onSelect: () -> Void
    let onDragChanged: (CGPoint) -> Void
    let onDragEnded: (CGPoint) -> Void

    @State private var hovering = false

    private var visual: KindVisual { kindVisual(entry.kind) }
    // `RupuBuilder.` prefix disambiguates from `View`'s own deprecated
    // `accentColor(_:)` method — see `ShapePaths.swift`'s doc comment.
    private var accent: Color { RupuBuilder.accentColor(visual.accent) }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            SilhouetteShape(name: visual.shape)
                .stroke(accent, lineWidth: 1.2)
                .frame(width: 34, height: 20)
            Text(entry.label)
                .font(.dataMono(11))
                .foregroundStyle(accent)
            Text(visual.tagline)
                .font(.noteText)
                .foregroundStyle(Color.rupuDim)
        }
        .padding(8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(hovering ? Color.rupuSurface : Color.clear)
        .clipShape(RoundedRectangle(cornerRadius: 6))
        .overlay(RoundedRectangle(cornerRadius: 6).stroke(selected ? accent : Color.clear, lineWidth: 1))
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .onTapGesture { onSelect() }
        // `"builder"` is the screen-wide named coordinate space
        // `WorkflowBuilderScreen` establishes on its root — so `value.
        // location` here is directly comparable to the canvas frame it
        // captures via `.onGeometryChange(... in: .named("builder"))`,
        // with no local->screen conversion needed on either side.
        .gesture(
            DragGesture(minimumDistance: 4, coordinateSpace: .named("builder"))
                .onChanged { value in onDragChanged(value.location) }
                .onEnded { value in onDragEnded(value.location) }
        )
    }
}

/// The drag-to-canvas ghost `WorkflowBuilderScreen` overlays at the pointer
/// while a palette card drag is in flight — same mini-silhouette treatment
/// as `PaletteCard`'s own preview, a little larger and never hit-testable
/// (it only ever tracks the pointer, never receives one).
struct PaletteDragGhost: View {
    let kind: RupuFlowKit.StepKind

    private var visual: KindVisual { kindVisual(kind) }
    // `RupuBuilder.` prefix — see `PaletteCard.accent`'s comment above.
    private var accent: Color { RupuBuilder.accentColor(visual.accent) }

    var body: some View {
        VStack(spacing: 3) {
            SilhouetteShape(name: visual.shape)
                .stroke(accent, lineWidth: 1.4)
                .frame(width: 48, height: 28)
            Text(kind.rawValue)
                .font(.dataMono(9))
                .foregroundStyle(accent)
        }
        .padding(6)
        .background(Color.rupuPanel.opacity(0.9))
        .clipShape(RoundedRectangle(cornerRadius: 6))
        .opacity(0.85)
        .allowsHitTesting(false)
    }
}
