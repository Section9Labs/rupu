import SwiftUI
import RupuStore
import RupuBackend
import RupuDesign

/// The app's primary navigation surface: fixed 216pt column, `.sidebar`
/// vibrancy material, section list bound directly to
/// `model.selectedSidebarItem`, and a host-status footer row. No Settings
/// row — the Settings scene owns ⌘,.
struct Sidebar: View {
    @Bindable var model: AppModel

    var body: some View {
        VStack(spacing: 0) {
            List(selection: $model.selectedSidebarItem) {
                Label("Overview", systemImage: "square.grid.2x2")
                    .tag(SidebarItem.overview)

                Section("Runs") {
                    Label("All runs", systemImage: "circle.grid.2x2")
                        .tag(SidebarItem.runs)
                    Label("Agent runs", systemImage: "person.crop.circle")
                        .tag(SidebarItem.runsLeaf(.agents))
                    Label("Workflows", systemImage: "arrow.triangle.branch")
                        .tag(SidebarItem.runsLeaf(.workflows))
                    Label("Autoflows", systemImage: "bolt.circle")
                        .tag(SidebarItem.runsLeaf(.autoflows))
                    Label("Sessions", systemImage: "terminal")
                        .tag(SidebarItem.runsLeaf(.sessions))
                }

                Section("Subjects") {
                    Label("Projects", systemImage: "folder")
                        .tag(SidebarItem.projects)
                }

                Section {
                    Label("Security", systemImage: "checkmark.shield")
                        .tag(SidebarItem.security)
                    Label("Library", systemImage: "books.vertical")
                        .tag(SidebarItem.library)
                    Label("Fleet", systemImage: "server.rack")
                        .tag(SidebarItem.fleet)
                    Label("Usage", systemImage: "chart.bar")
                        .tag(SidebarItem.usage)
                }
            }
            .listStyle(.sidebar)
            .scrollContentBackground(.hidden)

            Divider()
            footer
        }
        .frame(width: 216)
        .background(VisualEffectView())
    }

    private var footer: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(healthDotColor)
                .frame(width: 6, height: 6)
            MicroLabel(healthLabel)
                .foregroundStyle(Color.rupuDim)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
    }

    private var healthDotColor: Color {
        switch model.backendHealth {
        case .healthy: .status(StatusTone.done)
        case .degraded: .status(.waiting)
        case .down, .incompatible: .status(.fail)
        case .starting: .status(.pause)
        }
    }

    private var healthLabel: String {
        switch model.backendHealth {
        case .starting: "Starting"
        case .healthy(let version): "Connected \u{2013} \(version)"
        case .degraded: "Degraded"
        case .down: "Offline"
        case .incompatible: "Incompatible"
        }
    }
}
