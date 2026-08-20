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
