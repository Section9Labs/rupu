import SwiftUI

/// The Overview Customize menu's reorder sheet (Task 3). `ShellToolbar`'s
/// `customizeMenu` presents this via `.sheet(isPresented:)`; it's a sheet
/// rather than a `List` embedded directly in the `Menu` because a `List`
/// inside an AppKit `NSMenu` fights the platform (drag interactions and
/// AppKit's own menu tracking loop don't compose — verified empirically),
/// and a sheet is the lightest thing that actually works: a plain SwiftUI
/// `List` + `ForEach(...).onMove(perform:)`, which on macOS shows drag
/// handles and reorders on drop with no `EditMode`/`EditButton` plumbing
/// (that machinery is iOS-only — macOS `List` rows are draggable whenever
/// `.onMove` is attached).
///
/// Reads/writes the same `overview.widgets` `Data` blob as the Customize
/// menu's visibility toggles and `OverviewScreen`'s block gating — one
/// persistence seam, per the brief, via a `Binding<Data>` the caller
/// supplies from its own `@AppStorage(OverviewWidgets.storageKey)`
/// declaration (same trick those two already use to stay in sync with no
/// custom observation code).
public struct OverviewOrderEditor: View {
    @Binding var widgetsData: Data
    @Environment(\.dismiss) private var dismiss

    public init(widgetsData: Binding<Data>) {
        self._widgetsData = widgetsData
    }

    private var widgets: OverviewWidgets {
        OverviewWidgets.decode(widgetsData)
    }

    public var body: some View {
        NavigationStack {
            List {
                ForEach(widgets.order, id: \.self) { id in
                    Label(OverviewWidgets.label(for: id), systemImage: "line.3.horizontal")
                        .labelStyle(.titleOnly)
                }
                .onMove(perform: move)
            }
            .navigationTitle("Reorder Widgets")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Reset Order", action: resetOrder)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        .frame(minWidth: 320, minHeight: 280)
    }

    private func move(from source: IndexSet, to destination: Int) {
        widgetsData = Self.moved(widgets, from: source, to: destination).encoded
    }

    private func resetOrder() {
        widgetsData = Self.reset(widgets).encoded
    }

    // MARK: - Pure seam (the tested half — a `View`-type member, so its
    // tests carry `@MainActor` per this phase's CI rule, but no SwiftUI
    // hierarchy needs to actually run to exercise the reorder logic itself).

    /// Applies one drag-reorder gesture's `IndexSet`/destination (the shape
    /// `.onMove` hands a `List`) to `widgets.order`, leaving every other
    /// field untouched.
    static func moved(_ widgets: OverviewWidgets, from source: IndexSet, to destination: Int) -> OverviewWidgets {
        var result = widgets
        result.order.move(fromOffsets: source, toOffset: destination)
        return result
    }

    /// "Reset Order" — restores `OverviewWidgets.defaultOrder`, leaving
    /// visibility untouched (mirrors HANDOFF's "reset layout" affordance,
    /// scoped to just the order this task actually implements — no add/
    /// remove/size, since those aren't part of this task).
    static func reset(_ widgets: OverviewWidgets) -> OverviewWidgets {
        var result = widgets
        result.order = OverviewWidgets.defaultOrder
        return result
    }
}
