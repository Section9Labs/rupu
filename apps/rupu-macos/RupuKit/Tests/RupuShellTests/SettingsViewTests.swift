import Testing
import Foundation
@testable import RupuShell
import RupuStore
import RupuBackend

/// Minimal in-memory `TokenStoring` fake so these tests never touch the
/// real Keychain (same rationale as `RootViewTests`'s `FakeTokenStore` —
/// Keychain access in CI/test sandboxes is flaky).
private final class FakeTokenStore: TokenStoring, @unchecked Sendable {
    func save(token: String, account: String) throws {}
    func load(account: String) -> String? { nil }
    func delete(account: String) throws {}
}

@MainActor
private func makeSettingsView() -> (view: SettingsView, model: AppModel, backend: BackendController) {
    let defaults = UserDefaults(suiteName: "test-\(UUID())")!
    let model = AppModel(defaults: defaults)
    let backend = BackendController(
        defaults: defaults,
        tokenStore: FakeTokenStore(),
        discoverBinary: { _ in nil },
        embeddedProbe: { _ in false }
    )
    return (SettingsView(model: model, backend: backend), model, backend)
}

/// The four tabs (General/Connection/Config/Notifications) are each a
/// distinct `some View`-returning member accessor on `SettingsView`.
/// Accessing every one exercises its body-construction code path (the
/// `Form`s, the `backend.mode`/`.origin` read in `connectionTab`, the
/// `ConfigTab`/`NotificationsTab` placeholders) without crashing, and
/// comparing the concrete opaque types confirms none of the four tabs
/// silently collapsed onto a shared implementation.
@MainActor @Test func settingsViewTabsAreDistinctViews() {
    let (view, _, _) = makeSettingsView()

    let tabTypes: Set<String> = [
        String(describing: type(of: view.generalTab)),
        String(describing: type(of: view.connectionTab)),
        String(describing: type(of: view.configTab)),
        String(describing: type(of: view.notificationsTab)),
    ]

    #expect(tabTypes.count == 4, "each Settings tab must be backed by a distinct view")
}

/// `connectionTab` reads `backend.mode`/`.origin` for its read-only
/// current-connection line; with no mode configured yet (fresh
/// `BackendController`, as at first launch before `RootView` calls
/// `configureEmbedded`/`configureRemote`), that read must not trap.
@MainActor @Test func settingsViewConnectionTabRendersWithNoBackendModeConfigured() {
    let (view, _, backend) = makeSettingsView()

    #expect(backend.mode == nil)
    _ = view.connectionTab
}
