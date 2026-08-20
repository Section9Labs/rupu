import SwiftUI
import RupuDesign

/// Settings scene content (⌘,). General tab only this phase — appearance,
/// embedded-server port, and the rupu binary override path that Task 9's
/// `RupuDiscovery` wiring reads back. Connection/Providers/Notifications/
/// Dashboard tabs arrive in Phase 6.
public struct SettingsView: View {
    @AppStorage("appearance") private var appearance: String = "system"
    @AppStorage("embedded.port") private var embeddedPort: Int = 7420
    @AppStorage("rupu.binaryPath") private var binaryPathOverride: String = ""

    public init() {}

    public var body: some View {
        TabView {
            generalTab
                .tabItem { Label("General", systemImage: "gearshape") }
        }
        .frame(width: 420)
        .padding(20)
    }

    private var generalTab: some View {
        Form {
            Picker("Appearance", selection: $appearance) {
                Text("System").tag("system")
                Text("Light").tag("light")
                Text("Dark").tag("dark")
            }

            Section("Embedded server") {
                TextField(
                    "Port",
                    value: $embeddedPort,
                    format: .number.grouping(.never)
                )
                TextField(
                    "rupu binary override",
                    text: $binaryPathOverride,
                    prompt: Text("Auto-detected")
                )
            }
        }
        .padding(.top, 12)
    }
}
