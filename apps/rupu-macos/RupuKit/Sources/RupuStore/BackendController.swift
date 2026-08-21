import Foundation
import Observation
import RupuAPI
import RupuBackend

/// Owns the app's one live connection to a `rupu cp serve` control plane:
/// which `BackendMode` is configured, the resulting `EmbeddedServer.Origin`
/// (embedded mode only), and the `HealthMonitor` polling it. `RootView`
/// wires this into `AppModel` (health → sidebar footer, `eventStream()` →
/// the live pill); the app target only owns an instance and hooks it into
/// termination — see `RupuApp.swift`.
///
/// `mode` persists to `UserDefaults` as JSON (`backend.mode`) so a relaunch
/// restores it in `init` without the onboarding sheet reappearing; a
/// configured remote token goes to Keychain via `TokenStoring`, never to
/// UserDefaults (`mode`'s `.remote` case carries only the URL, never a
/// token, so there's nothing token-shaped to accidentally serialize).
@MainActor
@Observable
public final class BackendController {
    public private(set) var mode: BackendMode?
    public private(set) var origin: EmbeddedServer.Origin?

    /// Forwarded from the active `HealthMonitor` while one is running;
    /// explicitly set on the failure paths below (missing binary, Keychain
    /// write failure) that never get as far as starting one.
    public private(set) var health: BackendHealth = .starting

    /// Fired (via each `EventStreamClient` this controller creates) on
    /// HTTP-2xx connect and on disconnect. `RootView` sets this once, at
    /// launch, before calling `reconnectIfNeeded`/`configureEmbedded`/
    /// `configureRemote`, to drive the toolbar's live pill from actual
    /// connection state rather than from decoded frames alone — a healthy
    /// but idle server (SSE keep-alives only, no events) never dispatches
    /// a frame, so a frame-only signal would show "offline" against a
    /// perfectly good stream. `client()`/`eventStream()`'s existing
    /// backward-compatible API is unaffected; this is purely additive.
    public var onLiveConnectionChange: (@Sendable (Bool) -> Void)?

    private let defaults: UserDefaults
    private let tokenStore: any TokenStoring
    private let discoverBinary: @Sendable (String?) -> String?
    private let embeddedProbe: @Sendable (URL) async -> Bool
    private let healthInterval: Duration

    private var embeddedServer: EmbeddedServer?
    private var healthMonitor: HealthMonitor?
    private var activeClient: CPClient?
    private var activeEventStream: EventStreamClient?
    private var activeConfig: CPConfig?

    private static let modeKey = "backend.mode"
    private static let binaryPathOverrideKey = "rupu.binaryPath"

    public init(
        defaults: UserDefaults = .standard,
        tokenStore: any TokenStoring = KeychainTokenStore(),
        discoverBinary: @escaping @Sendable (String?) -> String? = { RupuDiscovery.find(override: $0) },
        embeddedProbe: @escaping @Sendable (URL) async -> Bool = BackendController.defaultEmbeddedProbe,
        healthInterval: Duration = .seconds(5)
    ) {
        self.defaults = defaults
        self.tokenStore = tokenStore
        self.discoverBinary = discoverBinary
        self.embeddedProbe = embeddedProbe
        self.healthInterval = healthInterval
        self.mode = Self.loadPersistedMode(defaults)
    }

    /// Discovers `rupu` (Settings override → `which` → standard paths),
    /// attaches to (or spawns) `cp serve` on `port`, and starts polling its
    /// health. A missing binary never touches `EmbeddedServer` — it sets
    /// `.down` directly with an install hint and leaves `mode`/`origin` nil
    /// so the caller knows nothing is configured.
    ///
    /// `discoverBinary` is synchronous and, in production, shells out to
    /// the user's login shell (`RupuDiscovery.loginShellWhich`) — a slow
    /// shell profile can take 0.5-2s+. This class is `@MainActor`, so
    /// calling it inline would hang the UI for that long; `Task.detached`
    /// hops it off the main actor while keeping the injected closure
    /// itself unchanged (tests still pass a fast synchronous fake).
    public func configureEmbedded(port: Int) async {
        await tearDownEmbeddedServer(keepRunning: false)
        healthMonitor?.stop()
        healthMonitor = nil

        let override = defaults.string(forKey: Self.binaryPathOverrideKey).flatMap { $0.isEmpty ? nil : $0 }
        let discoverBinary = discoverBinary
        let discovered = await Task.detached(priority: .userInitiated) {
            discoverBinary(override)
        }.value
        guard let binaryPath = discovered else {
            mode = nil
            origin = nil
            health = .down("rupu not found — install rupu or set the path in Settings")
            return
        }

        let server = EmbeddedServer(binaryPath: binaryPath, port: port, probe: embeddedProbe)
        embeddedServer = server
        do {
            origin = try await server.start()
        } catch {
            embeddedServer = nil
            health = .down("failed to start rupu cp serve: \(error)")
            return
        }

        let newMode = BackendMode.embedded(port: port)
        mode = newMode
        persist(newMode)
        startHealthMonitor(config: CPConfig(baseURL: newMode.baseURL, token: nil))
    }

    /// Stores `token` in Keychain (via `tokenStore`), persists `.remote(url:)`
    /// (never the token) to UserDefaults, and starts polling health.
    public func configureRemote(url: URL, token: String) async {
        await connectRemote(url: url, token: token, persistToken: true)
    }

    /// Re-establishes the connection implied by a mode `init` restored from
    /// `UserDefaults` — `init` only decodes the persisted `BackendMode`
    /// enum value; this performs the actual attach/spawn (or remote client
    /// wiring) + health-monitor start so a relaunch with a persisted mode
    /// skips onboarding entirely. `RootView` calls this once at launch. A
    /// no-op if nothing is persisted or a connection is already live.
    ///
    /// The `.remote` branch deliberately never re-saves the token it reads:
    /// a Keychain read failure (locked keychain, a cross-process access
    /// prompt that can't be answered, ...) must never be allowed to
    /// overwrite a real stored token with an empty string, which is what
    /// unconditionally forwarding into `configureRemote` used to do. A read
    /// failure surfaces as `.down` instead and leaves both `mode` and the
    /// Keychain untouched, so the next launch attempt (or a manual retry)
    /// can still succeed.
    public func reconnectIfNeeded() async {
        guard healthMonitor == nil, let mode else { return }
        switch mode {
        case .embedded(let port):
            await configureEmbedded(port: port)
        case .remote(let url):
            guard let token = tokenStore.load(account: url.absoluteString) else {
                health = .down("keychain unavailable — token could not be read")
                return
            }
            await connectRemote(url: url, token: token, persistToken: false)
        }
    }

    /// Shared remote-connect path for `configureRemote` (explicit user
    /// action — always persists the token) and `reconnectIfNeeded`
    /// (restoring an already-persisted mode — never re-persists, since the
    /// token it read already came from Keychain and re-saving on every
    /// launch is both unnecessary and, on a `load` failure, unsafe).
    private func connectRemote(url: URL, token: String, persistToken: Bool) async {
        await tearDownEmbeddedServer(keepRunning: false)
        healthMonitor?.stop()
        healthMonitor = nil

        if persistToken {
            do {
                try tokenStore.save(token: token, account: url.absoluteString)
            } catch {
                health = .down("failed to store token in Keychain: \(error)")
                return
            }
        }

        let newMode = BackendMode.remote(url: url)
        mode = newMode
        persist(newMode)
        startHealthMonitor(config: CPConfig(baseURL: url, token: token))
    }

    public func client() -> CPClient? {
        activeClient
    }

    public func eventStream() -> EventStreamClient? {
        activeEventStream
    }

    /// Builds a brand-new `JSONEventStream<CPEvent>` against the same
    /// `/api/events/stream` endpoint and credentials as the shared
    /// `eventStream()` firehose — but as its own independent connection,
    /// wired to the caller's own `onConnectionChange`. `eventStream()`'s
    /// `EventStreamClient` has its `onConnectionChange` fixed at
    /// construction (already claimed by `RootView`, forwarded into
    /// `model.liveConnected`); there's no second slot on that instance for
    /// another consumer's own connection-state tracking. A caller (e.g.
    /// `ActivityStore`'s live tail) that needs its own honest
    /// connect/disconnect signal — not `eventStream()`'s frames blindly
    /// pumped through with someone else's connection state bolted on —
    /// gets a fresh, fully independent stream from here instead.
    ///
    /// `nil` when nothing is configured yet (mirrors `client()`/
    /// `eventStream()`'s own "nothing connected" behavior).
    public func makeFirehoseStream(onConnectionChange: (@Sendable (Bool) -> Void)? = nil) -> EventStreamClient? {
        guard let config = activeConfig else { return nil }
        return EventStreamClient(
            url: config.baseURL.appendingPathComponent("api/events/stream"),
            token: config.token,
            onConnectionChange: onConnectionChange
        )
    }

    /// Stops health polling and tears down a *spawned* embedded server
    /// (never an attached one) unless `keepRunning` is set. Idempotent:
    /// safe to call more than once (e.g. a raw `SIGTERM` reroute racing an
    /// AppKit-graceful quit that both reach this).
    public func shutdown(keepRunning: Bool) async {
        healthMonitor?.stop()
        healthMonitor = nil
        await tearDownEmbeddedServer(keepRunning: keepRunning)
    }

    private func tearDownEmbeddedServer(keepRunning: Bool) async {
        guard let server = embeddedServer else { return }
        await server.stop(keepRunning: keepRunning)
        embeddedServer = nil
    }

    private func startHealthMonitor(config: CPConfig) {
        let client = CPClient(config: config)
        activeClient = client
        activeConfig = config
        activeEventStream = EventStreamClient(
            url: config.baseURL.appendingPathComponent("api/events/stream"),
            token: config.token,
            onConnectionChange: onLiveConnectionChange
        )

        let monitor = HealthMonitor(probe: { try await client.hostInfo() }, interval: healthInterval)
        healthMonitor = monitor
        health = monitor.health
        monitor.start()
        observe(monitor)
    }

    /// Bridges the child `HealthMonitor`'s `@Observable` state into this
    /// object's own `health`: `withObservationTracking` fires `onChange`
    /// exactly once per change, so each firing copies the new value and
    /// re-subscribes. `healthMonitor === monitor` guards against a stale
    /// subscription outliving a `configureEmbedded`/`configureRemote` call
    /// that replaced the monitor out from under it.
    private func observe(_ monitor: HealthMonitor) {
        withObservationTracking {
            _ = monitor.health
        } onChange: { [weak self, weak monitor] in
            Task { @MainActor in
                guard let self, let monitor, self.healthMonitor === monitor else { return }
                self.health = monitor.health
                self.observe(monitor)
            }
        }
    }

    private func persist(_ mode: BackendMode) {
        guard let data = try? JSONEncoder().encode(mode) else { return }
        defaults.set(data, forKey: Self.modeKey)
    }

    private static func loadPersistedMode(_ defaults: UserDefaults) -> BackendMode? {
        guard let data = defaults.data(forKey: modeKey) else { return nil }
        return try? JSONDecoder().decode(BackendMode.self, from: data)
    }

    public static func defaultEmbeddedProbe(_ url: URL) async -> Bool {
        var request = URLRequest(url: url)
        request.timeoutInterval = 3
        do {
            let (_, response) = try await URLSession.shared.data(for: request)
            guard let http = response as? HTTPURLResponse else { return false }
            return (200..<300).contains(http.statusCode)
        } catch {
            return false
        }
    }
}
