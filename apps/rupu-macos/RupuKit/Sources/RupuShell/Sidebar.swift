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
                sidebarLabel("Overview", icon: .layoutDashboard)
                    .tag(SidebarItem.overview)

                Section("Runs") {
                    sidebarLabel("All runs", icon: .activity)
                        .tag(SidebarItem.runs)
                    sidebarLabel("Agent runs", icon: .sparkles)
                        .tag(SidebarItem.runsLeaf(.agents))
                    sidebarLabel("Workflows", icon: .workflow)
                        .tag(SidebarItem.runsLeaf(.workflows))
                    sidebarLabel("Autoflows", icon: .repeatIcon)
                        .tag(SidebarItem.runsLeaf(.autoflows))
                    sidebarLabel("Sessions", icon: .messageSquare)
                        .tag(SidebarItem.runsLeaf(.sessions))
                }

                Section("Subjects") {
                    sidebarLabel("Projects", icon: .folderGit2)
                        .tag(SidebarItem.projects)
                }

                Section {
                    sidebarLabel("Security", icon: .shieldCheck)
                        .tag(SidebarItem.security)
                    sidebarLabel("Library", icon: .bookMarked)
                        .tag(SidebarItem.library)
                    sidebarLabel("Fleet", icon: .server)
                        .tag(SidebarItem.fleet)
                    sidebarLabel("Usage", icon: .dollarSign)
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

    /// A sidebar row's label — `Icon`, not `systemImage:`, per the v2 nav
    /// icon contract (`Icon.swift`'s `LucideIcon` table).
    private func sidebarLabel(_ title: String, icon: LucideIcon) -> some View {
        Label {
            Text(title)
        } icon: {
            Icon(icon)
        }
    }

    private var footer: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(healthDotColor)
                .frame(width: 6, height: 6)
            Text(healthLabel)
                .font(.metaText)
                .foregroundStyle(Color.rupuDim)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
    }

    private var healthDotColor: Color {
        switch model.backendHealth {
        case .healthy: .status(.done)
        case .degraded: .status(.awaiting)
        case .down, .incompatible: .status(.failed)
        case .starting: .status(.paused)
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
