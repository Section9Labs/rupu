import SwiftUI
import RupuDesign
import RupuStore

/// Settings scene content (⌘,). Four tabs: **General** (appearance only),
/// **Connection** (embedded-server port, `rupu` binary override, keep-running
/// — moved out of General so General stays about presentation only — plus a
/// read-only current-connection line sourced from `backend.mode`/`.origin`),
/// **Config** (`ConfigTab`, a `ConfigStore`-backed editor for `[cp]` and
/// provider sections — Providers folds into this tab rather than getting its
/// own), and **Notifications** (`NotificationsTab`, a `RunNotifier`-backed
/// preferences editor). Both `ConfigTab` and `NotificationsTab` are
/// placeholder shells in this task, wired for real in Tasks 6/7 of the same
/// PR. There is no separate Dashboard tab: that disposition is covered by
/// Overview's own Customize visibility menu, not Settings.
public struct SettingsView: View {
    @AppStorage("appearance") private var appearance: String = "system"
    @AppStorage("embedded.port") private var embeddedPort: Int = 7420
    @AppStorage("rupu.binaryPath") private var binaryPathOverride: String = ""
    @AppStorage("keepServerRunning") private var keepServerRunning: Bool = false

    private let model: AppModel
    private let backend: BackendController

    public init(model: AppModel, backend: BackendController) {
        self.model = model
        self.backend = backend
    }

    public var body: some View {
        TabView {
            generalTab
                .tabItem {
                    Label {
                        Text("General")
                    } icon: {
                        Icon(.settings)
                    }
                }

            connectionTab
                .tabItem {
                    Label {
                        Text("Connection")
                    } icon: {
                        Icon(.network)
                    }
                }

            configTab
                .tabItem {
                    Label {
                        Text("Config")
                    } icon: {
                        Icon(.fileText)
                    }
                }

            notificationsTab
                .tabItem {
                    Label {
                        Text("Notifications")
                    } icon: {
                        Icon(.radio)
                    }
                }
        }
        .frame(width: 560)
        .padding(20)
    }

    var generalTab: some View {
        Form {
            Picker("Appearance", selection: $appearance) {
                Text("System").tag("system")
                Text("Light").tag("light")
                Text("Dark").tag("dark")
            }
        }
        .padding(.top, 12)
        // General is a single picker; keep it from collapsing to a sliver
        // now the shared tab width grew to 560 to fit the Config editors.
        .frame(minHeight: 200, alignment: .top)
    }

    var connectionTab: some View {
        Form {
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
                Toggle("Keep server running when app quits", isOn: $keepServerRunning)
            }

            Section("Current connection") {
                Text(connectionSummary)
                    .foregroundStyle(.secondary)
            }
        }
        .padding(.top, 12)
    }

    var configTab: some View {
        ConfigTab(model: model, backend: backend)
    }

    var notificationsTab: some View {
        NotificationsTab()
    }

    /// Read-only summary of `backend.mode`/`.origin` — the port/URL actually
    /// in effect right now, as distinct from the `embeddedPort`/
    /// `binaryPathOverride` fields above, which only take effect on the next
    /// (re)connect.
    private var connectionSummary: String {
        switch backend.mode {
        case .embedded(let port):
            let originText: String = switch backend.origin {
            case .attached: "attached to an existing server"
            case .spawned: "spawned by this app"
            case nil: "connecting"
            }
            return "Embedded on port \(port) — \(originText)"
        case .remote(let url):
            return "Remote — \(url.absoluteString)"
        case nil:
            return "Not connected"
        }
    }
}

/// Placeholder Config tab. Task 6 replaces this with a `ConfigStore`-backed
/// editor over `[cp]`/provider config sections; deliberately store-less here
/// since that wiring is next task's work, not this one's.
// Replaced in Task 6
struct ConfigTab: View {
    let model: AppModel
    let backend: BackendController

    var body: some View {
        Form {
            Text("Config")
                .foregroundStyle(.secondary)
        }
        .padding(.top, 12)
    }
}

/// Placeholder Notifications tab. Task 7 replaces this with a
/// `RunNotifier`-backed preferences editor.
// Replaced in Task 7
struct NotificationsTab: View {
    var body: some View {
        Form {
            Text("Notifications")
                .foregroundStyle(.secondary)
        }
        .padding(.top, 12)
    }
}
