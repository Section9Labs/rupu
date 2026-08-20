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
