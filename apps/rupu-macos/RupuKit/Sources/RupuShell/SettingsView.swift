import SwiftUI
import RupuDesign
import RupuStore

/// Settings scene content (⌘,). Four tabs: **General** (appearance only),
/// **Connection** (embedded-server port, `rupu` binary override, keep-running
/// — moved out of General so General stays about presentation only — plus a
/// read-only current-connection line sourced from `backend.mode`/`.origin`),
/// **Config** (`ConfigTab`, defined in `ConfigTab.swift` — a `ConfigStore`-
/// backed editor for `[cp]` and provider sections; Providers folds into this
/// tab rather than getting its own), and **Notifications** (`NotificationsTab`,
/// defined in `NotificationsTab.swift` — a `RunNotifier`-backed preferences
/// editor; Task 7 replaced the earlier placeholder shell).
/// There is no separate Dashboard tab: that disposition is covered by
/// Overview's own Customize visibility menu, not Settings.
public struct SettingsView: View {
    @AppStorage("appearance") private var appearance: String = "system"
    @AppStorage("embedded.port") private var embeddedPort: Int = 7420
    @AppStorage("rupu.binaryPath") private var binaryPathOverride: String = ""
    @AppStorage("keepServerRunning") private var keepServerRunning: Bool = false

    private let model: AppModel
    private let backend: BackendController
    private let notifier: RunNotifier

    public init(model: AppModel, backend: BackendController, notifier: RunNotifier) {
        self.model = model
        self.backend = backend
        self.notifier = notifier
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
        // Redesign-pass fix (spec §4, "Settings scene tone"): the native
        // `Settings` window paints its own chrome-gray material behind
        // whatever this `TabView` renders, which is what the 6A validation
        // note and the redesign audit (A8) both flagged as reading
        // out-of-family with the app's v2 token palette. An explicit
        // `Color.rupuBg` fill behind the tab content — the same background
        // token every other screen sits on — brings the tone into line
        // while the `Settings { }` scene itself (window chrome, traffic
        // lights, tab strip) stays exactly the native macOS frame the spec
        // calls for keeping.
        .background(Color.rupuBg)
    }

    var generalTab: some View {
        VStack(alignment: .leading, spacing: 12) {
            settingsCard(title: "Appearance") {
                labeledRow(label: "Appearance") {
                    Picker("", selection: $appearance) {
                        Text("System").tag("system")
                        Text("Light").tag("light")
                        Text("Dark").tag("dark")
                    }
                    .labelsHidden()
                    .frame(width: 160)
                }
            }
            Spacer(minLength: 0)
        }
        .padding(.top, 12)
        // General is a single picker; keep it from collapsing to a sliver
        // now the shared tab width grew to 560 to fit the Config editors.
        // `maxHeight: .infinity` + the background below is what actually
        // repaints the native tab well (see `body`'s doc comment) rather
        // than leaving a chrome-gray margin around an intrinsically-sized
        // card.
        .frame(maxWidth: .infinity, minHeight: 200, maxHeight: .infinity, alignment: .top)
        .background(Color.rupuBg)
    }

    var connectionTab: some View {
        VStack(alignment: .leading, spacing: 12) {
            settingsCard(title: "Embedded server") {
                VStack(alignment: .leading, spacing: 10) {
                    labeledRow(label: "Port") {
                        TextField(
                            "",
                            value: $embeddedPort,
                            format: .number.grouping(.never)
                        )
                        .textFieldStyle(.plain)
                        .padding(6)
                        .panelStyle(.innerCard)
                        .frame(width: 100)
                    }
                    labeledRow(label: "rupu binary override") {
                        TextField(
                            "",
                            text: $binaryPathOverride,
                            prompt: Text("Auto-detected").foregroundStyle(Color.rupuMute)
                        )
                        .textFieldStyle(.plain)
                        .padding(6)
                        .panelStyle(.innerCard)
                    }
                    Toggle("Keep server running when app quits", isOn: $keepServerRunning)
                        .font(.uiText)
                        .foregroundStyle(Color.rupuInk)
                }
            }

            settingsCard(title: "Current connection") {
                Text(connectionSummary)
                    .font(.uiText)
                    .foregroundStyle(Color.rupuDim)
            }

            Spacer(minLength: 0)
        }
        .padding(.top, 12)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(Color.rupuBg)
    }

    /// Shared card chrome for General/Connection: an `Eyebrow` header over a
    /// `Color.rupuPanel` card — the same panel/eyebrow idiom `ConfigTab`'s
    /// `sectionCard`/`EffectiveConfigList` already use, so all four Settings
    /// tabs read as one token-styled family rather than General/Connection
    /// (formerly native `Form`/`Section`) looking like a different app from
    /// Config/Notifications.
    private func settingsCard(title: String, @ViewBuilder content: () -> some View) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Eyebrow(title)
            content()
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .panelStyle(.panel)
    }

    /// A left label / right control row — replaces the label column a
    /// native `Form` drew for free, styled on tokens instead of the system
    /// control-label color.
    private func labeledRow(label: String, @ViewBuilder control: () -> some View) -> some View {
        HStack {
            Text(label)
                .font(.uiText)
                .foregroundStyle(Color.rupuInk)
            Spacer(minLength: 12)
            control()
        }
    }

    var configTab: some View {
        ConfigTab(model: model, backend: backend)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
            .background(Color.rupuBg)
    }

    var notificationsTab: some View {
        NotificationsTab(notifier: notifier)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
            .background(Color.rupuBg)
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
