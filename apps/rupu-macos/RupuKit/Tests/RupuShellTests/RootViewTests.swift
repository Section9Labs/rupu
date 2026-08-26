import Testing
import Foundation
@testable import RupuShell
import RupuStore
import RupuBackend

/// Minimal in-memory `TokenStoring` fake so these tests never touch the
/// real Keychain (same rationale as `RupuStoreTests`'s `InMemoryTokenStore`
/// — Keychain access in CI/test sandboxes is flaky).
private final class FakeTokenStore: TokenStoring, @unchecked Sendable {
    func save(token: String, account: String) throws {}
    func load(account: String) -> String? { nil }
    func delete(account: String) throws {}
}

@MainActor
private func makeRootView() -> (root: RootView, model: AppModel, backend: BackendController) {
    let defaults = UserDefaults(suiteName: "test-\(UUID())")!
    let model = AppModel(defaults: defaults)
    let backend = BackendController(
        defaults: defaults,
        tokenStore: FakeTokenStore(),
        discoverBinary: { _ in nil },
        embeddedProbe: { _ in false }
    )
    return (RootView(model: model, backend: backend), model, backend)
}

/// Regression test for the Esc/Cmd-. onboarding dead-end: an interactive
/// sheet dismissal writes `false` back through `onboardingSheetBinding`
/// (exactly what SwiftUI does on Esc/Cmd-.), and that write must never
/// persist `onboardingComplete = true` — the only legitimate completion
/// path is `OnboardingView`'s own health-driven `onChange`, which sets the
/// model flag directly. Before this fix, the binding's setter treated
/// `isPresented == false` as "user dismissed, mark it done," permanently
/// stranding the app pre-onboarding with no reconnect path.
@MainActor @Test func onboardingSheetBindingIgnoresInteractiveDismissal() {
    let (root, model, _) = makeRootView()

    #expect(model.onboardingComplete == false)
    #expect(root.onboardingSheetBinding.wrappedValue == true, "sheet presented while onboarding incomplete")

    root.onboardingSheetBinding.wrappedValue = false
    #expect(model.onboardingComplete == false, "interactive dismissal must not mark onboarding complete")
    #expect(root.onboardingSheetBinding.wrappedValue == true, "sheet must stay presented")

    // The only legitimate completion path: the model flag flips directly,
    // the same way OnboardingView's `.onChange(of: backend.health)` does.
    model.onboardingComplete = true
    #expect(root.onboardingSheetBinding.wrappedValue == false)
}

/// Regression test for the live-pill flicker: `.degraded` (a single failed
/// health poll) must NOT force `liveConnected` false — the SSE stream may
/// still be connected, and `onLiveConnectionChange` already owns that
/// signal. Only a hard `.down`/`.incompatible` should force it false as a
/// backstop.
@MainActor @Test func handleHealthChangeBackstopOnlyForcesFalseOnDownOrIncompatible() {
    let (root, model, _) = makeRootView()

    model.liveConnected = true
    root.handleHealthChange(.degraded("one failed poll"))
    #expect(model.liveConnected == true, ".degraded must not force liveConnected false")

    root.handleHealthChange(.starting)
    #expect(model.liveConnected == true, ".starting must not force liveConnected false")

    root.handleHealthChange(.down("connection refused"))
    #expect(model.liveConnected == false, ".down must force liveConnected false")

    model.liveConnected = true
    root.handleHealthChange(.incompatible(serverVersion: "0.1.0"))
    #expect(model.liveConnected == false, ".incompatible must force liveConnected false")
}

/// `activateOnClientAvailable()` (perf & interaction arc, Plan 5 Task 2 —
/// the `.task(id: backend.clientGeneration)`-driven replacement for the old
/// health-gated activation) is a safe no-op before any client exists, and
/// safe to call repeatedly (idempotent) once one does — mirroring every
/// other `activate`-style method's "safe to call more than once" contract
/// in this codebase. `hostsFooter`/`palette` are private `@State`, so this
/// only asserts the observable half: no client yet means nothing crashes
/// and `backend.client()` stays `nil`; a real client (via `configureEmbedded`)
/// lets repeated calls run without ever throwing/crashing.
@MainActor @Test func activateOnClientAvailableIsANoOpWithoutAClientAndIdempotentOnceOneExists() async {
    let (root, _, backend) = makeRootView()

    // No client yet (discoverBinary returns nil in `makeRootView()`) — must
    // not crash.
    root.activateOnClientAvailable()
    #expect(backend.client() == nil)

    // Swap in a controller that DOES resolve a client, then call it twice.
    let defaults = UserDefaults(suiteName: "test-\(UUID())")!
    let model = AppModel(defaults: defaults)
    let connectedBackend = BackendController(
        defaults: defaults,
        tokenStore: FakeTokenStore(),
        discoverBinary: { _ in "/fake/path/to/rupu" },
        embeddedProbe: { _ in true }
    )
    await connectedBackend.configureEmbedded(port: 7420)
    #expect(connectedBackend.client() != nil)

    let connectedRoot = RootView(model: model, backend: connectedBackend)
    connectedRoot.activateOnClientAvailable()
    connectedRoot.activateOnClientAvailable() // idempotent — must not crash or duplicate
}
