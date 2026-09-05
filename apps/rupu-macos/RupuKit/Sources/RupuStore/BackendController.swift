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

    /// The app-wide pending-mutation ledger (Phase 3, Task 5): every screen
    /// that can fire a run-control mutation (`RunDetailStore`'s
    /// approve/reject/cancel/pause/resume/archive/restore,
    /// `ActivityStore`'s awaiting-row approve/reject) shares this ONE
    /// instance rather than owning a private one — a key `ActivityStore`
    /// begins (e.g. an Activity-row Approve tap) must read as `.pending` to
    /// `RunDetailStore` too if the operator navigates to that same run's
    /// detail screen mid-flight, and vice versa. Both stores' designated
    /// inits take a `pendingActions: PendingActions` parameter defaulted to
    /// a fresh private instance (keeps every existing unit test — which
    /// never cares about cross-screen sharing — unchanged); production call
    /// sites (`RunDetailScreen`/`ActivityScreen`) pass this instance
    /// explicitly, same DI seam already used for `client()`/
    /// `makeFirehoseStream()`.
    public let pendingActions = PendingActions()

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

    /// Bumped exactly once per `startHealthMonitor(config:)` call — i.e.
    /// the instant `activeClient`/`activeEventStream` are swapped in to a
    /// freshly usable pair, independent of whether `health` has reached
    /// `.healthy` yet (perf & interaction arc, Plan 5 Task 2). `client()`
    /// is usable — and the SSE endpoint reachable — the moment
    /// `startHealthMonitor` runs, well before `HealthMonitor`'s own first
    /// probe round-trip resolves `health` to `.healthy`; a screen that
    /// waits for `health == .healthy` before activating (the old
    /// `health -> onChange -> onChange` relay `RootView`/`OverviewScreen`
    /// used to depend on) pays that extra, unnecessary round trip on every
    /// cold start for no reason. Screens should key a `.task(id:
    /// backend.clientGeneration)` off this directly instead — see
    /// `RootView`/`OverviewScreen` for the converted call sites. `health`
    /// keeps its own independent semantics for STATUS DISPLAY (the sidebar
    /// footer dot, the toolbar pill) — this counter is purely an
    /// activation signal, never surfaced in any UI itself.
    public private(set) var clientGeneration = 0

    private static let modeKey = "backend.mode"
    private static let binaryPathOverrideKey = "rupu.binaryPath"
    /// Cache key for the last successfully-started `rupu` binary path
    /// (perf & interaction arc, Plan 5 Task 2) — see `configureEmbedded(port:)`'s
    /// doc comment on the fast cold-start path this enables.
    private static let cachedBinaryPathKey = "backend.binaryPath.cached"

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

    /// Discovers `rupu` (Settings override → cached path → `which` →
    /// standard paths), attaches to (or spawns) `cp serve` on `port`, and
    /// starts polling its health. A missing binary never touches
    /// `EmbeddedServer` — it sets `.down` directly with an install hint and
    /// leaves `mode`/`origin` nil so the caller knows nothing is
    /// configured.
    ///
    /// `discoverBinary` is synchronous and, in production, shells out to
    /// the user's login shell (`RupuDiscovery.loginShellWhich`) — a slow
    /// shell profile can take 0.5-2s+. This class is `@MainActor`, so
    /// calling it inline would hang the UI for that long; `Task.detached`
    /// hops it off the main actor while keeping the injected closure
    /// itself unchanged (tests still pass a fast synchronous fake).
    ///
    /// **Cached-path fast start** (perf & interaction arc, Plan 5 Task 2):
    /// a Settings override always wins, same as before. Absent that, the
    /// path that successfully started `cp serve` LAST time (persisted under
    /// `Self.cachedBinaryPathKey`) is used OPTIMISTICALLY — skipping the
    /// full `discoverBinary` shell-out entirely — since the installed
    /// binary's location essentially never changes between launches. If
    /// `EmbeddedServer.start()` then fails against that cached path (moved,
    /// uninstalled, or otherwise no longer valid — `invalidCachedPath`
    /// tracks which case this is), the cache is invalidated and this method
    /// retries EXACTLY ONCE against a fresh full `discoverBinary` call —
    /// self-healing rather than wedging the app against a stale path
    /// forever, and `allowCacheRetry: false` on that retry prevents any
    /// possibility of looping. A cold start with no cache yet (first ever
    /// launch) always falls through to the full discovery path, same as
    /// before this task.
    public func configureEmbedded(port: Int) async {
        await attemptConfigureEmbedded(port: port, allowCacheRetry: true)
    }

    private func attemptConfigureEmbedded(port: Int, allowCacheRetry: Bool) async {
        await tearDownEmbeddedServer(keepRunning: false)
        healthMonitor?.stop()
        healthMonitor = nil

        let override = defaults.string(forKey: Self.binaryPathOverrideKey).flatMap { $0.isEmpty ? nil : $0 }
        let cachedPath = override == nil
            ? defaults.string(forKey: Self.cachedBinaryPathKey).flatMap { $0.isEmpty ? nil : $0 }
            : nil

        let binaryPath: String
        let usedCachedPath: Bool
        if let cachedPath {
            binaryPath = cachedPath
            usedCachedPath = true
        } else {
            let discoverBinary = discoverBinary
            guard let discovered = await Task.detached(priority: .userInitiated, operation: {
                discoverBinary(override)
            }).value else {
                mode = nil
                origin = nil
                health = .down("rupu not found — install rupu or set the path in Settings")
                return
            }
            binaryPath = discovered
            usedCachedPath = false
        }

        let server = EmbeddedServer(binaryPath: binaryPath, port: port, probe: embeddedProbe)
        embeddedServer = server
        do {
            origin = try await server.start()
        } catch {
            embeddedServer = nil
            // The cached path may be stale (binary moved/uninstalled since
            // it was last cached) — invalidate it and retry once against a
            // fresh full discovery before reporting failure. Never retries
            // a second time (`allowCacheRetry: false`): if the freshly
            // rediscovered path ALSO fails to start, that's a real failure
            // worth surfacing, not something a further retry would fix.
            if usedCachedPath, allowCacheRetry {
                defaults.removeObject(forKey: Self.cachedBinaryPathKey)
                await attemptConfigureEmbedded(port: port, allowCacheRetry: false)
                return
            }
            health = .down("failed to start rupu cp serve: \(error)")
            return
        }

        // A successful start (cached or freshly discovered) is exactly
        // what's worth caching for next launch — an override is deliberate
        // operator configuration and is never cached over.
        if override == nil {
            defaults.set(binaryPath, forKey: Self.cachedBinaryPathKey)
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

    /// Identity of whatever `client()` currently returns — `nil` iff
    /// `client()` itself is `nil`. `CPClient` is an `actor` (a reference
    /// type), so wrapping it in `ObjectIdentifier` is a legitimate, honest
    /// identity check: `startHealthMonitor(config:)` builds a brand-new
    /// `CPClient` and swaps `activeClient` directly (never through `nil` in
    /// between) on every `configureEmbedded`/`connectRemote` call — an
    /// embedded/remote mode switch, a manual reconnect, or a restart all go
    /// through there. A store-owning screen (`OverviewScreen`/
    /// `ActivityScreen`/`RunDetailScreen`) that only checks "do I already
    /// have a store" without also checking this would keep running its
    /// store against an abandoned, disconnected `CPClient` forever — this
    /// is the seam those screens compare against `client()`'s result to
    /// detect that swap and rebuild.
    public func clientIdentity() -> ObjectIdentifier? {
        activeClient.map(ObjectIdentifier.init)
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

    /// Same rationale as `makeFirehoseStream` above, but scoped server-side
    /// to one run's events (`/api/events/stream?run=<id>`) — `RunDetailStore`
    /// (Task 8)'s own independent connection for the step graph's live
    /// pulses, wired to its own `onConnectionChange`, never sharing
    /// `eventStream()`'s single already-claimed callback slot.
    public func makeRunEventStream(runID: String, host: String? = nil, onConnectionChange: (@Sendable (Bool) -> Void)? = nil) -> JSONEventStream<CPEvent>? {
        guard let config = activeConfig,
              var components = URLComponents(url: config.baseURL.appendingPathComponent("api/events/stream"), resolvingAgainstBaseURL: false)
        else { return nil }
        var items = [URLQueryItem(name: "run", value: runID)]
        if let host, host != "local" { items.append(URLQueryItem(name: "host", value: host)) }
        components.queryItems = items
        guard let url = components.url else { return nil }
        return JSONEventStream<CPEvent>(url: url, token: config.token, onConnectionChange: onConnectionChange)
    }

    /// Same shape as `makeRunEventStream` above, for the transcript tail
    /// (`/api/transcript/stream?path=<file>`) — `RunDetailStore.focusStep`
    /// tears this down and rebuilds it against a new `path` every time focus
    /// switches to a different step. `host`/`run` route a remote run's tail
    /// through the coordinator's lazy SSH mirror.
    public func makeTranscriptStream(path: String, host: String? = nil, run: String? = nil, onConnectionChange: (@Sendable (Bool) -> Void)? = nil) -> JSONEventStream<TranscriptEvent>? {
        guard let config = activeConfig,
              var components = URLComponents(url: config.baseURL.appendingPathComponent("api/transcript/stream"), resolvingAgainstBaseURL: false)
        else { return nil }
        var items = [URLQueryItem(name: "path", value: path)]
        if let host, host != "local" { items.append(URLQueryItem(name: "host", value: host)) }
        if let run { items.append(URLQueryItem(name: "run", value: run)) }
        components.queryItems = items
        guard let url = components.url else { return nil }
        return JSONEventStream<TranscriptEvent>(url: url, token: config.token, onConnectionChange: onConnectionChange)
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

        // `client()`/`eventStream()` are usable THIS INSTANT — bumped after
        // both are already assigned above, so any observer reacting to the
        // change sees a fully-wired pair, never a half-updated one. See
        // `clientGeneration`'s own doc comment.
        clientGeneration += 1
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
