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

    /// Local key monitor that answers Escape while the palette is up.
    /// Chosen over `.onKeyPress(.escape)` / `.onExitCommand` because a
    /// focused `NSTextField` field editor can claim modifier-less Escape
    /// (completion trigger) before either SwiftUI mechanism sees it, and a
    /// local `NSEvent` monitor runs before window dispatch — immune to that
    /// ordering by construction. (Automated validation can't exercise this
    /// key: synthetic Escape events are not delivered to the app, verified
    /// by this very monitor logging arrows but never Escape — so the belt
    /// chosen here is the one that cannot lose the race, pending a
    /// real-keyboard check.) Umbrella spec sanctions AppKit interop where
    /// needed. Installed `.onAppear`/removed `.onDisappear`, so it exists
    /// only while the palette is on screen.
    @State private var escMonitor: Any?

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
        .onAppear {
            // See `escMonitor`'s doc comment for why this is an NSEvent
            // monitor and not a SwiftUI key handler.
            escMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
                guard event.keyCode == 53 else { return event }  // 53 = Escape
                store.close()
                return nil
            }
        }
        .onDisappear {
            if let escMonitor {
                NSEvent.removeMonitor(escMonitor)
                self.escMonitor = nil
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .onKeyPress(.upArrow) { moveActive(by: -1); return .handled }
        .onKeyPress(.downArrow) { moveActive(by: 1); return .handled }
        .onKeyPress(.return) { executeActive(); return .handled }
        // Escape is handled by the NSEvent monitor installed `.onAppear`
        // below — see `escMonitor`'s doc comment. ↑/↓/Return stay as plain
        // key handlers: the field editor doesn't contest arrows or Return
        // (verified live — they move the selection and execute).
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
