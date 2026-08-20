import SwiftUI
import RupuStore
import RupuBackend
import RupuDesign

/// Sheet shown on any launch before `model.onboardingComplete` (artboard
/// 03): an Embedded card (rupu.app manages `cp serve` itself) and a Remote
/// card (attach to a host already running one). The moment `backend.health`
/// first reaches `.healthy`, this marks onboarding complete — `RootView`'s
/// `.sheet(isPresented:)` binding reads that flag, so the sheet dismisses
/// itself with no separate "dismiss" plumbing. `.incompatible` blocks with
/// a version-gate banner instead of letting the user proceed against a
/// server this app can't safely talk to (spec §5).
public struct OnboardingView: View {
    @Bindable var backend: BackendController
    @Bindable var model: AppModel

    @AppStorage("embedded.port") private var embeddedPort: Int = 7420
    @AppStorage("rupu.binaryPath") private var binaryPathOverride: String = ""

    @State private var discoveredPath: String?
    @State private var remoteURLText: String = ""
    @State private var remoteToken: String = ""
    @State private var isConnecting = false

    public init(backend: BackendController, model: AppModel) {
        self.backend = backend
        self.model = model
    }

    public var body: some View {
        VStack(spacing: 20) {
            header

            if case .incompatible(let serverVersion) = backend.health {
                incompatibleBanner(serverVersion: serverVersion)
            } else if case .down(let message) = backend.health {
                errorBanner(message)
            }

            HStack(alignment: .top, spacing: 14) {
                embeddedCard
                remoteCard
            }

            MicroLabel("Tokens are stored in the macOS Keychain")
                .foregroundStyle(Color.rupuMute)
        }
        .padding(28)
        .frame(width: 580)
        .task(id: binaryPathOverride) {
            discoveredPath = discoverBinary()
        }
        .onChange(of: backend.health) { _, newValue in
            if case .healthy = newValue {
                model.onboardingComplete = true
            }
        }
    }

    private var header: some View {
        VStack(spacing: 6) {
            Text("Where is your control plane?")
                .font(.system(size: 17, weight: .semibold))
                .foregroundStyle(Color.rupuInk)
            Text("rupu.app can run everything itself, or attach to a control plane you already run.")
                .font(.system(size: 12))
                .foregroundStyle(Color.rupuDim)
                .multilineTextAlignment(.center)
        }
    }

    private func incompatibleBanner(serverVersion: String) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(Color.status(.fail))
            Text("Server is rupu \(serverVersion); this app needs \(VersionGate.minimum) or newer. Run `rupu update` on the host, then try again.")
                .font(.system(size: 11.5))
                .foregroundStyle(Color.rupuInk)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(10)
        .panelStyle(.innerCard)
    }

    private func errorBanner(_ message: String) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: "exclamationmark.circle.fill")
                .foregroundStyle(Color.status(.waiting))
            Text(message)
                .font(.system(size: 11.5))
                .foregroundStyle(Color.rupuInk)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(10)
        .panelStyle(.innerCard)
    }

    private var embeddedCard: some View {
        VStack(alignment: .leading, spacing: 10) {
            MicroLabel("Recommended")
                .foregroundStyle(Color.rupuBrandHi)
            Text("Run it here")
                .font(.system(size: 14, weight: .semibold))
                .foregroundStyle(Color.rupuInk)
            Text("rupu.app embeds cp serve and manages it for you.")
                .font(.system(size: 11.5))
                .foregroundStyle(Color.rupuDim)

            Text(discoveryStatusLine)
                .font(.identifier)
                .foregroundStyle(discoveredPath == nil ? Color.status(.fail) : Color.rupuDim)
                .fixedSize(horizontal: false, vertical: true)

            TextField("Port", value: $embeddedPort, format: .number.grouping(.never))
                .textFieldStyle(.roundedBorder)

            Button("Start") {
                Task {
                    isConnecting = true
                    await backend.configureEmbedded(port: embeddedPort)
                    isConnecting = false
                }
            }
            .disabled(discoveredPath == nil || isConnecting)
            .buttonStyle(.borderedProminent)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(16)
        .panelStyle()
    }

    private var remoteCard: some View {
        VStack(alignment: .leading, spacing: 10) {
            MicroLabel("Remote")
                .foregroundStyle(Color.rupuMute)
            Text("Connect to one")
                .font(.system(size: 14, weight: .semibold))
                .foregroundStyle(Color.rupuInk)
            Text("Attach to a rupu cp serve already running on a build box or server.")
                .font(.system(size: 11.5))
                .foregroundStyle(Color.rupuDim)

            TextField("https://build-01.internal:7420", text: $remoteURLText)
                .textFieldStyle(.roundedBorder)
            SecureField("access token", text: $remoteToken)
                .textFieldStyle(.roundedBorder)

            Button("Connect") {
                guard let url = URL(string: remoteURLText) else { return }
                Task {
                    isConnecting = true
                    await backend.configureRemote(url: url, token: remoteToken)
                    isConnecting = false
                }
            }
            .disabled(URL(string: remoteURLText) == nil || isConnecting)
            .buttonStyle(.bordered)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(16)
        .panelStyle()
    }

    private func discoverBinary() -> String? {
        RupuDiscovery.find(override: binaryPathOverride.isEmpty ? nil : binaryPathOverride)
    }

    private var discoveryStatusLine: String {
        guard let discoveredPath else {
            return "rupu not found \u{2014} install it or set a path in Settings"
        }
        return "Found \(discoveredPath)"
    }
}
