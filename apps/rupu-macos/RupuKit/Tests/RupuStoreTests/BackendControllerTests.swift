import Testing
import Foundation
@testable import RupuStore
import RupuBackend
import RupuAPI

/// In-memory `TokenStoring` fake so these tests never touch the real
/// Keychain (Keychain access in CI/test sandboxes is flaky, per Task 8's
/// note on `KeychainTokenStore`).
final class InMemoryTokenStore: TokenStoring, @unchecked Sendable {
    private let lock = NSLock()
    private(set) var saved: [String: String] = [:]

    func save(token: String, account: String) throws {
        lock.withLock { saved[account] = token }
    }

    func load(account: String) -> String? {
        lock.withLock { saved[account] }
    }

    func delete(account: String) throws {
        lock.withLock { saved[account] = nil }
    }
}

/// A `BackendController` wired for tests: `discoverBinary` returns a fixed
/// fake path (so embedded configuration never shells out to `which`), and
/// `embeddedProbe` always reports "already answering" so `EmbeddedServer`
/// takes the `.attached` path and never actually spawns a process or hits
/// the real network.
@MainActor
private func makeController(defaults: UserDefaults, tokenStore: TokenStoring, binaryFound: Bool = true) -> BackendController {
    BackendController(
        defaults: defaults,
        tokenStore: tokenStore,
        discoverBinary: { _ in binaryFound ? "/fake/path/to/rupu" : nil },
        embeddedProbe: { _ in true }
    )
}

@MainActor @Test func modeRoundTripsThroughUserDefaults() async {
    let suite = UserDefaults(suiteName: "test-\(UUID())")!
    let tokenStore = InMemoryTokenStore()

    let first = makeController(defaults: suite, tokenStore: tokenStore)
    #expect(first.mode == nil)
    await first.configureEmbedded(port: 7420)
    #expect(first.mode == .embedded(port: 7420))

    // A fresh instance backed by the same UserDefaults suite restores the
    // persisted mode from its `init` alone — no reconnect call needed.
    let second = makeController(defaults: suite, tokenStore: tokenStore)
    #expect(second.mode == .embedded(port: 7420))
}

@MainActor @Test func remoteConfigureStoresTokenInFakeStoreNeverInUserDefaults() async {
    let suite = UserDefaults(suiteName: "test-\(UUID())")!
    let tokenStore = InMemoryTokenStore()
    let controller = makeController(defaults: suite, tokenStore: tokenStore)

    let url = URL(string: "https://build-01.internal:7420")!
    await controller.configureRemote(url: url, token: "s3cr3t-token")

    #expect(controller.mode == .remote(url: url))
    #expect(tokenStore.load(account: url.absoluteString) == "s3cr3t-token")

    // The persisted `backend.mode` blob is just the URL — the token never
    // rides along into UserDefaults in any form.
    guard let data = suite.data(forKey: "backend.mode"),
          let raw = String(data: data, encoding: .utf8) else {
        Issue.record("expected backend.mode to be persisted")
        return
    }
    #expect(!raw.contains("s3cr3t-token"))

    // Nor does it leak into any other UserDefaults key.
    for (key, value) in suite.dictionaryRepresentation() where key.hasPrefix("backend") || key.hasPrefix("embedded") || key.hasPrefix("rupu") {
        #expect(!"\(value)".contains("s3cr3t-token"), "token leaked into UserDefaults key \(key)")
    }

    let second = makeController(defaults: suite, tokenStore: tokenStore)
    #expect(second.mode == .remote(url: url))
}

@MainActor @Test func missingBinarySetsDownHealthAndClearsMode() async {
    let suite = UserDefaults(suiteName: "test-\(UUID())")!
    let tokenStore = InMemoryTokenStore()
    let controller = makeController(defaults: suite, tokenStore: tokenStore, binaryFound: false)

    await controller.configureEmbedded(port: 7420)

    #expect(controller.mode == nil)
    #expect(controller.health == .down("rupu not found — install rupu or set the path in Settings"))
    #expect(controller.client() == nil)
    #expect(controller.eventStream() == nil)
}

// MARK: - Cached binary path fast start (perf & interaction arc, Plan 5 Task 2)

/// Thread-safe call counter — same rationale as every other store test
/// file's own copy (`@Sendable` fetch/probe closures can't capture a plain
/// mutable `var`).
private final class CallCounter: @unchecked Sendable {
    private let lock = NSLock()
    private var v = 0
    @discardableResult
    func increment() -> Int { lock.withLock { v += 1; return v } }
    var value: Int { lock.withLock { v } }
}

/// A cached binary path from a prior successful launch skips
/// `discoverBinary` (the slow login-shell `which`) entirely — `discoverBinary`
/// here fails the test outright if it's ever called, so any invocation at
/// all is a hard failure, not just a wrong-path assertion.
@MainActor @Test func cachedBinaryPathSkipsDiscoverBinaryEntirely() async {
    let suite = UserDefaults(suiteName: "test-\(UUID())")!
    suite.set("/cached/path/to/rupu", forKey: "backend.binaryPath.cached")
    let tokenStore = InMemoryTokenStore()
    let discoverCalls = CallCounter()

    let controller = BackendController(
        defaults: suite,
        tokenStore: tokenStore,
        discoverBinary: { _ in
            discoverCalls.increment()
            Issue.record("discoverBinary must not be called when a cached path exists")
            return "/should-never-be-used/rupu"
        },
        embeddedProbe: { _ in true } // "already attached" — no real spawn needed.
    )

    await controller.configureEmbedded(port: 7420)

    #expect(discoverCalls.value == 0)
    #expect(controller.mode == .embedded(port: 7420), "the cached path must still be used to start successfully")
    #expect(controller.client() != nil)
}

/// A cached path that no longer resolves (binary moved/uninstalled since
/// last launch) is invalidated and this method falls back to a full
/// `discoverBinary` rediscovery — exactly once, never looping — recovering
/// successfully against the freshly-discovered path.
@MainActor @Test func staleCachedPathFallsBackToDiscoverBinaryOnSpawnFailureAndRecovers() async {
    let suite = UserDefaults(suiteName: "test-\(UUID())")!
    suite.set("/stale/uninstalled/rupu", forKey: "backend.binaryPath.cached")
    let tokenStore = InMemoryTokenStore()
    let discoverCalls = CallCounter()
    let probeCalls = CallCounter()

    let controller = BackendController(
        defaults: suite,
        tokenStore: tokenStore,
        discoverBinary: { _ in
            discoverCalls.increment()
            return "/freshly/discovered/rupu"
        },
        // First call (against the STALE cached path) reports "not already
        // running" — forcing `EmbeddedServer` to attempt an actual spawn,
        // which fails immediately (`/stale/uninstalled/rupu` doesn't exist
        // as an executable). Every subsequent call (the retry, against the
        // freshly-discovered path) reports "already attached" — recovering
        // without needing a real spawn either.
        embeddedProbe: { _ in probeCalls.increment() > 1 }
    )

    await controller.configureEmbedded(port: 7420)

    #expect(discoverCalls.value == 1, "exactly one fallback rediscovery, never a loop")
    #expect(suite.string(forKey: "backend.binaryPath.cached") == "/freshly/discovered/rupu", "the stale cache entry is replaced by the recovered path")
    #expect(controller.mode == .embedded(port: 7420), "must recover, not stay down")
    #expect(controller.client() != nil)
}

/// A Settings override always wins over any cached path, and a successful
/// start under an override is never itself written to the cache (an
/// override is deliberate operator configuration, not something to
/// silently promote to the optimistic default).
@MainActor @Test func settingsOverrideWinsOverCachedPathAndIsNeverCached() async {
    let suite = UserDefaults(suiteName: "test-\(UUID())")!
    suite.set("/cached/path/to/rupu", forKey: "backend.binaryPath.cached")
    suite.set("/override/path/to/rupu", forKey: "rupu.binaryPath")
    let tokenStore = InMemoryTokenStore()
    let discoverCalls = CallCounter()

    let controller = BackendController(
        defaults: suite,
        tokenStore: tokenStore,
        discoverBinary: { override in
            discoverCalls.increment()
            return override
        },
        embeddedProbe: { _ in true }
    )

    await controller.configureEmbedded(port: 7420)

    #expect(discoverCalls.value == 1, "an override still goes through discoverBinary (it resolves/validates the override), just never the cache")
    #expect(controller.mode == .embedded(port: 7420))
    #expect(suite.string(forKey: "backend.binaryPath.cached") == "/cached/path/to/rupu", "unchanged — an override's own success is never cached over")
}

// MARK: - clientGeneration (perf & interaction arc, Plan 5 Task 2)

/// `clientGeneration` bumps the instant a client/event-stream pair is
/// wired up — independent of `health` ever reaching `.healthy` (this
/// controller's fake `HealthMonitor` probe never runs in these tests, so
/// `health` stays `.starting` throughout, yet the client is already usable
/// and the generation already bumped).
@MainActor @Test func clientGenerationBumpsOnceClientIsUsableIndependentOfHealth() async {
    let suite = UserDefaults(suiteName: "test-\(UUID())")!
    let tokenStore = InMemoryTokenStore()
    let controller = makeController(defaults: suite, tokenStore: tokenStore)

    #expect(controller.clientGeneration == 0)

    await controller.configureEmbedded(port: 7420)

    #expect(controller.clientGeneration == 1)
    #expect(controller.client() != nil)
}

/// A second `configureEmbedded`/reconnect bumps it again — the exact signal
/// a `.task(id: backend.clientGeneration)`-driven screen activation needs
/// to notice a client swap (embedded/remote switch, manual reconnect,
/// restart) and rebuild against the new one.
@MainActor @Test func clientGenerationBumpsAgainOnASecondConfigureEmbeddedCall() async {
    let suite = UserDefaults(suiteName: "test-\(UUID())")!
    let tokenStore = InMemoryTokenStore()
    let controller = makeController(defaults: suite, tokenStore: tokenStore)

    await controller.configureEmbedded(port: 7420)
    #expect(controller.clientGeneration == 1)

    await controller.configureEmbedded(port: 7420)
    #expect(controller.clientGeneration == 2)
}

@MainActor @Test func reconnectIfNeededIsNoOpWithoutPersistedMode() async {
    let suite = UserDefaults(suiteName: "test-\(UUID())")!
    let tokenStore = InMemoryTokenStore()
    let controller = makeController(defaults: suite, tokenStore: tokenStore)

    await controller.reconnectIfNeeded()

    #expect(controller.mode == nil)
    #expect(controller.health == .starting)
    #expect(controller.client() == nil)
}

/// Regression test: `reconnectIfNeeded`'s `.remote` branch used to forward
/// straight into `configureRemote`, which unconditionally re-saves
/// whatever token it was handed — including `""` when `tokenStore.load`
/// returned `nil`. That silently clobbered a real stored token on any
/// transient Keychain read failure, with no error surfaced and no path
/// back to onboarding. When a token IS present, reconnecting must restore
/// the connection and leave the store exactly as it was (no re-save).
@MainActor @Test func reconnectIfNeededRemoteWithTokenPresentDoesNotRewriteStore() async {
    let suite = UserDefaults(suiteName: "test-\(UUID())")!
    let tokenStore = InMemoryTokenStore()
    let url = URL(string: "http://127.0.0.1:65535")!

    let first = makeController(defaults: suite, tokenStore: tokenStore)
    await first.configureRemote(url: url, token: "original-token")
    #expect(tokenStore.load(account: url.absoluteString) == "original-token")

    // A fresh instance, simulating a relaunch: `init` restores `mode` from
    // UserDefaults; `reconnectIfNeeded` is what actually reconnects.
    let second = makeController(defaults: suite, tokenStore: tokenStore)
    #expect(second.mode == .remote(url: url))

    await second.reconnectIfNeeded()

    #expect(second.mode == .remote(url: url))
    #expect(second.client() != nil)
    // The store must be untouched by the reconnect — same token, and never
    // rewritten (asserted via `saved` directly, not just `load`, so a
    // delete-then-rewrite round trip couldn't hide behind an equal value).
    #expect(tokenStore.saved == [url.absoluteString: "original-token"])
}

/// The other half of the same regression: when `tokenStore.load` returns
/// `nil` (Keychain unavailable), reconnecting must surface `.down` and
/// must NOT write anything to the store — proving the old
/// `?? ""`-then-save behavior is gone, not just that it happens to produce
/// the right value in the happy path.
@MainActor @Test func reconnectIfNeededRemoteWithMissingTokenSetsDownAndNeverWritesStore() async {
    let suite = UserDefaults(suiteName: "test-\(UUID())")!
    let tokenStore = InMemoryTokenStore()
    let url = URL(string: "http://127.0.0.1:65535")!

    // Persist a `.remote` mode the same way `configureRemote` would, but
    // without ever having stored a token for it — simulating a token
    // that's gone missing from Keychain since the mode was last persisted.
    let modeData = try! JSONEncoder().encode(BackendMode.remote(url: url))
    suite.set(modeData, forKey: "backend.mode")

    let controller = makeController(defaults: suite, tokenStore: tokenStore)
    #expect(controller.mode == .remote(url: url))

    await controller.reconnectIfNeeded()

    #expect(controller.mode == .remote(url: url))
    #expect(controller.health == .down("keychain unavailable — token could not be read"))
    #expect(controller.client() == nil)
    #expect(tokenStore.saved.isEmpty)
}
