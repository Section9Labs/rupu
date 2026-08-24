import SwiftUI
import RupuStore
import RupuDesign

/// The ⌘K command palette overlay (flows-composition Task 3): a 40% black
/// scrim (tap to close, per the V2-CONTRACT dialog rule) behind a
/// top-centered card — `TextField` + a ranked results list — driven
/// entirely by `PaletteStore`. `RootView` presents this in an `.overlay`
/// while `store.isOpen`, and owns the hidden ⌘K button that calls
/// `store.open()` (same pattern as its existing hidden ⌘N button).
public struct CommandPaletteView: View {
    @Bindable var store: PaletteStore

    @FocusState private var searchFocused: Bool

    public init(store: PaletteStore) {
        self.store = store
    }

    public var body: some View {
        ZStack {
            // Dialog scrim, per V2-CONTRACT: 40% black, no shadows anywhere
            // in this view — chrome comes entirely from `.panelStyle`.
            Color.black.opacity(0.4)
                .ignoresSafeArea()
                .onTapGesture { store.close() }

            card
                .frame(maxWidth: 640)
                .padding(.top, 96)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .onKeyPress(.upArrow) { moveActive(by: -1); return .handled }
        .onKeyPress(.downArrow) { moveActive(by: 1); return .handled }
        .onKeyPress(.return) { executeActive(); return .handled }
        // Escape is deliberately NOT wired via `.onKeyPress(.escape)` — live
        // GUI validation found it dead: the `TextField` above is always
        // focused the instant this view appears (`card`'s `.onAppear` sets
        // `searchFocused = true`, and there is no other path to seeing this
        // view at all), and on macOS a focused `NSTextField`'s field editor
        // consumes the Escape *key event* for its own text-editing purposes
        // before SwiftUI's `onKeyPress` (which reads raw key events) ever
        // sees it. ↑/↓/Return above are unaffected because AppKit's field
        // editor only intercepts Escape, not arrow/Return.
        //
        // `.onExitCommand` sidesteps this because it isn't a raw-key-event
        // handler at all — it binds to AppKit's `cancelOperation:`
        // responder *action message*, which `NSTextView`/`NSTextField`
        // explicitly forward up the responder chain once they've decided
        // Escape isn't theirs to consume as text editing (same mechanism
        // that makes a dialog's Cancel button respond to Escape regardless
        // of which control has focus). Attached here, at the view's root,
        // rather than on the `TextField` itself, so it also covers a future
        // focus target inside `results` without needing to move.
        .onExitCommand { store.close() }
    }

    private var card: some View {
        VStack(alignment: .leading, spacing: 0) {
            TextField("Search…", text: $store.query)
                .textFieldStyle(.plain)
                .font(.uiText)
                .foregroundStyle(Color.rupuInk)
                .focused($searchFocused)
                .padding(12)

            Divider()

            results
        }
        .panelStyle(.panel)
        // `TextField` needs the palette to already be in the view tree
        // before it can become first responder — `.onAppear` fires once
        // `RootView`'s `.overlay { if store.isOpen { ... } }` actually
        // inserts this view, which only happens after `store.open()` has
        // already flipped `isOpen`, so there's no race with the fetch.
        .onAppear { searchFocused = true }
    }

    @ViewBuilder
    private var results: some View {
        if store.results.isEmpty {
            Text("No results")
                .font(.noteText)
                .foregroundStyle(Color.rupuMute)
                .padding(16)
                .frame(maxWidth: .infinity, alignment: .leading)
        } else {
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(store.results.enumerated()), id: \.element.id) { index, item in
                        row(item, active: index == store.activeIndex)
                            .onTapGesture {
                                store.activeIndex = index
                                Task { await store.execute(item) }
                            }
                    }
                }
            }
            .frame(maxHeight: 360)
        }
    }

    /// One result row: kind `Eyebrow` + title `.uiText` + optional
    /// `.rupuMute` subtitle. Active row gets `rupuSurface` fill plus a 2px
    /// `rupuBrand` leading inset — the same active-state treatment the
    /// sidebar rail uses (`Sidebar.railRow`), so the palette's keyboard
    /// selection reads as the same "this is the current pick" affordance
    /// everywhere else in the app.
    private func row(_ item: PaletteItem, active: Bool) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Eyebrow(item.kind.rawValue)
                .frame(width: 60, alignment: .leading)
            VStack(alignment: .leading, spacing: 2) {
                Text(item.title)
                    .font(.uiText)
                    .foregroundStyle(Color.rupuInk)
                    .lineLimit(1)
                if let subtitle = item.subtitle {
                    Text(subtitle)
                        .font(.noteText)
                        .foregroundStyle(Color.rupuMute)
                        .lineLimit(1)
                }
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(active ? Color.rupuSurface : .clear)
        .overlay(alignment: .leading) {
            if active { Color.rupuBrand.frame(width: 2) }
        }
        .contentShape(Rectangle())
    }

    private func moveActive(by delta: Int) {
        let count = store.results.count
        guard count > 0 else { return }
        store.activeIndex = ((store.activeIndex + delta) % count + count) % count
    }

    private func executeActive() {
        guard store.results.indices.contains(store.activeIndex) else { return }
        let item = store.results[store.activeIndex]
        Task { await store.execute(item) }
    }
}
