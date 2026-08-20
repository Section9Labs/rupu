import SwiftUI
import RupuStore
import RupuDesign

/// The app's window content: fixed sidebar + detail pane that switches on
/// `model.route`. Phase 2+ swaps each `PlaceholderScreen` branch for a real
/// screen one route at a time; the switch shape is deliberate so those
/// swaps stay one-line diffs.
public struct RootView: View {
    @Bindable var model: AppModel

    public init(model: AppModel) {
        self.model = model
    }

    public var body: some View {
        NavigationSplitView {
            Sidebar(model: model)
                .navigationSplitViewColumnWidth(216)
        } detail: {
            detail
                .toolbar { ShellToolbar(model: model) }
        }
        .background(Color.rupuBg)
    }

    @ViewBuilder
    private var detail: some View {
        switch model.route {
        case .overview:
            PlaceholderScreen(title: model.route.screenTitle, phase: model.route.placeholderPhase)
        case .activity:
            PlaceholderScreen(title: model.route.screenTitle, phase: model.route.placeholderPhase)
        case .projects:
            PlaceholderScreen(title: model.route.screenTitle, phase: model.route.placeholderPhase)
        case .security:
            PlaceholderScreen(title: model.route.screenTitle, phase: model.route.placeholderPhase)
        case .library:
            PlaceholderScreen(title: model.route.screenTitle, phase: model.route.placeholderPhase)
        case .fleet:
            PlaceholderScreen(title: model.route.screenTitle, phase: model.route.placeholderPhase)
        case .usage:
            PlaceholderScreen(title: model.route.screenTitle, phase: model.route.placeholderPhase)
        }
    }
}
