import Testing
import Foundation
@testable import RupuShell
import RupuStore
import RupuBackend
import RupuAPI

// MARK: - Test infra (generation-token-isolated stub — see
// `ConfigStoreTests.ConfigStubURLProtocol`'s doc comment, in
// `RupuStoreTests`, for the full rationale; duplicated here because it's
// `internal`/file-scoped to that sibling target, same as every other
// store-test file's own copy of this rig).

final class ConfigTabStubURLProtocol: URLProtocol {
    private static let generationHeader = "X-ConfigTab-Stub-Generation"
    private static let lock = NSLock()
    nonisolated(unsafe) private static var handler: (@Sendable (URLRequest) -> (status: Int, body: Data))?
    nonisolated(unsafe) private static var currentGeneration = 0

    static func session() -> URLSession {
        let generation = lock.withLock { currentGeneration }
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [ConfigTabStubURLProtocol.self]
        config.httpAdditionalHeaders = [generationHeader: String(generation)]
        return URLSession(configuration: config)
    }

    static func reset(handler: @escaping @Sendable (URLRequest) -> (status: Int, body: Data)) {
        lock.withLock {
            currentGeneration += 1
            self.handler = handler
        }
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        let requestGeneration = request.value(forHTTPHeaderField: Self.generationHeader).flatMap(Int.init) ?? -1
        let (isCurrent, activeHandler): (Bool, (@Sendable (URLRequest) -> (status: Int, body: Data))?) = Self.lock.withLock {
            (requestGeneration == Self.currentGeneration, Self.handler)
        }
        guard isCurrent else {
            client?.urlProtocol(self, didFailWithError: URLError(.cancelled))
            return
        }
        guard let handler = activeHandler else {
            client?.urlProtocol(self, didFailWithError: URLError(.badURL))
            return
        }
        let (status, body) = handler(request)
        let response = HTTPURLResponse(
            url: request.url!, statusCode: status, httpVersion: "HTTP/1.1",
            headerFields: ["Content-Type": "application/json"]
        )!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: body)
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}
}

/// In-memory `TokenStoring` fake — same rationale as `SettingsViewTests`'s
/// own copy (file-scoped there, so re-declared here): Keychain access in a
/// CI/test sandbox is flaky, and `PolicyLockEditor`'s constructor needs a
/// `BackendController` even though these tests never actually call
/// `backend.client()`.
private final class FakeTokenStore: TokenStoring, @unchecked Sendable {
    func save(token: String, account: String) throws {}
    func load(account: String) -> String? { nil }
    func delete(account: String) throws {}
}

@MainActor
private func makeBackend() -> BackendController {
    let defaults = UserDefaults(suiteName: "test-\(UUID())")!
    return BackendController(
        defaults: defaults,
        tokenStore: FakeTokenStore(),
        discoverBinary: { _ in nil },
        embeddedProbe: { _ in false }
    )
}

@Suite(.serialized)
@MainActor
struct ConfigTabTests {
    private func makeClient(respond: @escaping @Sendable (URLRequest) -> (status: Int, body: Data)) -> CPClient {
        ConfigTabStubURLProtocol.reset(handler: respond)
        return CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: ConfigTabStubURLProtocol.session()
        )
    }

    /// Loads a `ConfigStore` from the real `config_view.json` fixture (the
    /// same fixture `RupuAPITests/ConfigModelsTests.swift` decodes directly)
    /// via a stub `CPClient`, mirroring `ConfigStoreTests`'s own rig.
    private func loadedStore() async throws -> ConfigStore {
        let fixture = try Fixtures.data("config_view.json")
        let client = makeClient { req in
            guard req.url?.path == "/api/config" else { return (404, Data()) }
            return (200, fixture)
        }
        let store = ConfigStore()
        await store.load(client: client, project: nil)
        return store
    }

    // MARK: - Step 1: Effective grouping

    /// The fixture's `providers."GLM-5.2-FP8".model` provenance key must
    /// group under the `providers` SECTION — not get torn apart at the
    /// quoted segment's embedded `.` — and its display remainder must still
    /// carry the model id, per `DottedKey.split`'s quote-aware contract.
    @Test func effectiveGroupingPutsQuotedProviderKeyUnderProvidersSectionNotSplitAtQuotedDot() async throws {
        let store = try await loadedStore()
        let view = try #require(store.view.value)

        let rows = EffectiveConfigGrouping.rows(for: view)
        let quotedRow = try #require(rows.first { $0.id == "providers.\"GLM-5.2-FP8\".model" })

        #expect(quotedRow.section == "providers")
        #expect(quotedRow.remainderDisplay.contains("GLM-5.2-FP8"))
        #expect(quotedRow.source == .project)
        #expect(!quotedRow.locked)
        // The resolved value walks `effective.providers."GLM-5.2-FP8".model`
        // successfully (a real string, not the `—` unresolved fallback).
        #expect(quotedRow.valueDisplay == "GLM-5.2-FP8")

        let groups = EffectiveConfigGrouping.grouped(rows)
        let providersGroup = try #require(groups.first { $0.id == "providers" })
        #expect(providersGroup.rows.contains { $0.id == quotedRow.id })
    }

    /// Review-round fix (Q4): `rows(for:)` walks LEAVES of `effective`, not
    /// `provenance`'s keys — a key at its default (no `provenance` entry at
    /// all) must still render as a row, with a `default` chip. The
    /// fixture's `effective.cp.bind` is exactly this case: it's a real,
    /// resolvable value, but `provenance` only has an entry for
    /// `cp.max_workspace_bytes`, never `cp.bind`. Under the OLD
    /// provenance-keys-only enumeration this row was invisible entirely.
    @Test func effectiveGroupingSurfacesALeafWithNoProvenanceEntryUnderTheDefaultChip() async throws {
        let store = try await loadedStore()
        let view = try #require(store.view.value)

        let rows = EffectiveConfigGrouping.rows(for: view)
        let bindRow = try #require(rows.first { $0.id == "cp.bind" })

        #expect(bindRow.section == "cp")
        #expect(bindRow.valueDisplay == "127.0.0.1:7420")
        #expect(bindRow.source == .default)
        #expect(!bindRow.locked)
    }

    /// Review-round fix (Q1): `effective.policy.lock` (now present in the
    /// fixture per the same fix) resolves cleanly through the leaf walk —
    /// it is no longer the "unresolvable key" example this test used to be
    /// (that example was itself a symptom of Q1's bug: the fixture didn't
    /// carry `effective.policy.lock` at all). `resolveValue` itself still
    /// returns `nil` for a genuinely bogus path, asserted directly below
    /// without needing a fixture case for it.
    @Test func resolveValueReturnsNilForAPathNotPresentInTheTree() async throws {
        let store = try await loadedStore()
        let view = try #require(store.view.value)

        let resolved = EffectiveConfigGrouping.resolveValue(view.effective, segments: ["not", "a", "real", "path"][...])
        #expect(resolved == nil)
        #expect(EffectiveConfigGrouping.render(resolved) == "—")
    }

    /// Review-round fix (Q8): every single-segment top-level key
    /// (`default_model`) groups under the shared `topLevelSectionName`
    /// section rather than getting its own one-row section whose header
    /// just repeats the row underneath it.
    @Test func singleSegmentTopLevelKeysGroupUnderTheSharedTopLevelSection() async throws {
        let store = try await loadedStore()
        let view = try #require(store.view.value)

        let rows = EffectiveConfigGrouping.rows(for: view)
        let defaultModelRow = try #require(rows.first { $0.id == "default_model" })

        #expect(defaultModelRow.section == EffectiveConfigGrouping.topLevelSectionName)
        // The row's own label must still show the real key, not the
        // shared section name.
        #expect(defaultModelRow.remainderDisplay == "default_model")

        let groups = EffectiveConfigGrouping.grouped(rows)
        let topLevelGroup = try #require(groups.first { $0.id == EffectiveConfigGrouping.topLevelSectionName })
        #expect(topLevelGroup.rows.contains { $0.id == "default_model" })
    }

    // MARK: - Step 1: Policy editor renders the fixture's lock entries

    /// Review-round fix (Q1 — the data-loss bug): the fixture's
    /// `effective.policy.lock` is `["policy.lock", "permission_mode"]`, but
    /// `permission_mode` has NO `provenance` entry at all (no layer ever
    /// sets it — see `resolve.rs`'s "only insert provenance when a layer
    /// supplies a value" rule). The editor's seed list must still include
    /// it — deriving from `provenance`'s locked keys instead (the
    /// pre-fix behavior) would silently drop it, and the next policy save
    /// would silently delete it from the real lock list.
    @Test func policyEditorSeedsCurrentLockListFromEffectivePolicyLockIncludingAKeyWithNoProvenanceEntry() async throws {
        let store = try await loadedStore()
        let backend = makeBackend()
        let editor = PolicyLockEditor(store: store, backend: backend, lockedKeys: .constant([]))

        #expect(editor.currentLockKeys == ["permission_mode", "policy.lock"])
        // Confirm the silent-delete premise directly: `permission_mode`
        // really has no `provenance` entry in this fixture.
        #expect(store.view.value?.provenance["permission_mode"] == nil)
    }

    /// Same helper `ConfigTab`'s own dirty check uses — pinned here so a
    /// future change to `PolicyLockEditor.currentLockKeys`'s source can't
    /// drift from `EffectiveConfigGrouping.policyLockKeys(for:)` without a
    /// test noticing.
    @Test func policyLockKeysHelperMatchesTheEditorsCurrentLockKeys() async throws {
        let store = try await loadedStore()
        let backend = makeBackend()
        let editor = PolicyLockEditor(store: store, backend: backend, lockedKeys: .constant([]))
        let view = try #require(store.view.value)

        #expect(EffectiveConfigGrouping.policyLockKeys(for: view) == editor.currentLockKeys)
    }

    // MARK: - Step 1: readOnly disables Save with a reason string

    /// Review-round fix (Q7): asserts `store.saveError` against the SHARED
    /// `ConfigStore.readOnlyMessage` constant (the actual thing a real 501
    /// sets), not a gate-vs-gate tautology (`ConfigSaveGate.reason(...) ==
    /// ConfigSaveGate.readOnlyReason` proves nothing — both sides read the
    /// same constant by construction).
    @Test func readOnlyDisablesSaveWithAReasonString() async throws {
        let store = try await loadedStore()

        // Force `readOnly` the same way the real app would — a 501 from any
        // write route (`ConfigStore.handleSaveError`'s doc comment).
        let writeClient = makeClient { req in
            if req.url?.path == "/api/config" {
                return (200, (try? Fixtures.data("config_view.json")) ?? Data())
            }
            if req.url?.path == "/api/config/global" {
                return (501, Data(#"{"error":"editing config requires `rupu cp serve`"}"#.utf8))
            }
            return (404, Data())
        }
        _ = await store.saveGlobalRaw("default_model = \"x\"\n", client: writeClient)
        #expect(store.readOnly)
        #expect(store.saveError == ConfigStore.readOnlyMessage)

        let reason = ConfigSaveGate.reason(readOnly: store.readOnly, isDirty: true)
        #expect(reason == ConfigStore.readOnlyMessage)
        #expect(reason?.contains("rupu cp serve") == true)
    }

    /// A non-read-only store with no edit yet must disable Save for a
    /// DIFFERENT, honest reason ("no changes to save") — never silently
    /// disabled with no explanation, and never conflated with the
    /// read-only case.
    @Test func notDirtyDisablesSaveWithADifferentReasonThanReadOnly() {
        let reason = ConfigSaveGate.reason(readOnly: false, isDirty: false)
        #expect(reason != nil)
        #expect(reason != ConfigSaveGate.readOnlyReason)

        #expect(ConfigSaveGate.reason(readOnly: false, isDirty: true) == nil)
    }

    // MARK: - Final-review fix (C1): draft adoption rule

    /// The pure adoption table. An unseeded draft (`nil`) always adopts;
    /// a draft still equal to the value this sync last handed it adopts;
    /// a diverged draft never does.
    @Test func draftAdoptionTableAdoptsWhenUnseededOrUntouchedAndKeepsADivergedDraft() {
        // Unseeded — the first content load must always seed.
        #expect(ConfigDrafts.shouldAdoptNewValue(draft: nil, previous: nil))
        #expect(ConfigDrafts.shouldAdoptNewValue(draft: nil, previous: "anything"))

        // Seeded, untouched (draft == the PREVIOUS store value).
        #expect(ConfigDrafts.shouldAdoptNewValue(draft: "old", previous: "old"))
        // A `nil` previous (the `.loading` bounce) reads as "".
        #expect(ConfigDrafts.shouldAdoptNewValue(draft: "", previous: nil))

        // Diverged — an in-progress edit must survive a background refresh.
        #expect(!ConfigDrafts.shouldAdoptNewValue(draft: "operator edit", previous: "old"))
        // Including a deliberately EMPTIED draft, which is exactly what the
        // optional-vs-"" distinction buys: `Optional("")` is a real edit,
        // unlike `nil`.
        #expect(!ConfigDrafts.shouldAdoptNewValue(draft: "", previous: "old"))

        // Lock-list analogue, order-insensitive.
        #expect(ConfigDrafts.shouldAdoptNewLockList(draft: nil, previous: ["a"]))
        #expect(ConfigDrafts.shouldAdoptNewLockList(draft: ["b", "a"], previous: ["a", "b"]))
        #expect(!ConfigDrafts.shouldAdoptNewLockList(draft: ["a"], previous: ["a", "b"]))
        #expect(!ConfigDrafts.shouldAdoptNewLockList(draft: [], previous: ["a"]))
    }

    /// **The regression this whole fix exists for.** The pre-fix closures
    /// compared the draft against `store.view`'s CURRENT (already-new)
    /// value, so adoption only ran when the draft ALREADY equaled what was
    /// about to be adopted — a no-op. Cold-opening Raw/Policy therefore
    /// rendered empty drafts with Save enabled, and saving PUT empty raw
    /// TOML (wiping `config.toml`) or an empty lock list (deleting every
    /// policy lock).
    ///
    /// This drives `ConfigDrafts` through the exact `onChange(...,
    /// initial: true)` call `ConfigTab.content(store:)` makes on first
    /// mount over real fixture content (SwiftUI passes `oldValue ==
    /// newValue` for the initial invocation) and asserts every draft comes
    /// out seeded with the FIXTURE's non-empty content.
    @Test @MainActor func initialContentLoadSeedsEveryDraftFromTheFixtureNeverEmpty() async throws {
        let store = try await loadedStore()
        let view = try #require(store.view.value)

        var drafts = ConfigDrafts()
        // `initial: true` — SwiftUI hands the same value as both old and new.
        drafts.syncGlobal(previous: view.rawGlobal, new: view.rawGlobal)
        drafts.syncProject(previous: view.rawProject, new: view.rawProject)
        let lockKeys = EffectiveConfigGrouping.policyLockKeys(for: view)
        drafts.syncLock(previous: lockKeys, new: lockKeys)

        #expect(drafts.global == view.rawGlobal)
        #expect(drafts.global?.isEmpty == false, "an empty global draft is the data-loss bug: Save would PUT empty TOML")
        #expect(drafts.project == view.rawProject)
        #expect(drafts.lock == ["permission_mode", "policy.lock"])
        #expect(drafts.lock?.isEmpty == false, "an empty lock draft is the data-loss bug: Save would delete every lock")

        // And with everything freshly seeded from the server, nothing reads
        // as dirty — so Save is correctly DISABLED rather than armed over
        // an empty draft.
        #expect(!drafts.hasUnsavedEdits(against: view))
    }

    /// The other half of the cold-open path: `content(store:)` mounts while
    /// `store.view` is still `.loading` (no content at all), so the very
    /// first `initial: true` invocation carries `nil` on both sides and the
    /// real content arrives as a LATER change. Both legs must end with the
    /// drafts holding the server's content.
    @Test @MainActor func draftsSeedThroughTheLoadingBounceThatPrecedesFirstContent() async throws {
        let store = try await loadedStore()
        let view = try #require(store.view.value)

        var drafts = ConfigDrafts()
        // Leg 1 — mounted against `.loading`: no content yet.
        drafts.syncGlobal(previous: nil, new: nil)
        drafts.syncLock(previous: nil, new: nil)
        #expect(drafts.global == "")

        // Leg 2 — content lands. The draft still equals what leg 1 gave it,
        // so it adopts.
        drafts.syncGlobal(previous: nil, new: view.rawGlobal)
        drafts.syncLock(previous: nil, new: EffectiveConfigGrouping.policyLockKeys(for: view))
        #expect(drafts.global == view.rawGlobal)
        #expect(drafts.lock == ["permission_mode", "policy.lock"])
    }

    /// A background reload (`.content` → `.loading` → `.content`) must never
    /// swallow an in-progress edit — neither leg of the bounce may adopt.
    @Test @MainActor func aDirtyDraftSurvivesABackgroundReloadBounce() async throws {
        let store = try await loadedStore()
        let view = try #require(store.view.value)

        var drafts = ConfigDrafts()
        drafts.syncGlobal(previous: view.rawGlobal, new: view.rawGlobal)
        drafts.global = "# operator's in-progress edit\n"

        drafts.syncGlobal(previous: view.rawGlobal, new: nil)   // → .loading
        #expect(drafts.global == "# operator's in-progress edit\n")
        drafts.syncGlobal(previous: nil, new: "server side changed\n") // → .content
        #expect(drafts.global == "# operator's in-progress edit\n")
        #expect(drafts.hasUnsavedEdits(against: view))
    }

    /// An all-`nil` (never-seeded) draft set has nothing to lose, so a
    /// scope switch must not raise the "discard unsaved edits?" dialog.
    @Test @MainActor func unseededDraftsAreNeverConsideredDirty() async throws {
        let store = try await loadedStore()
        let view = try #require(store.view.value)

        #expect(!ConfigDrafts().hasUnsavedEdits(against: view))
        #expect(!ConfigDrafts().hasUnsavedEdits(against: nil))
    }

    /// `adoptAll` is the confirmed-scope-switch bypass: it overwrites every
    /// draft regardless of dirtiness (see `ConfigTab.performScopeSwitch(to:
    /// store:)`'s doc comment for why that's correct there).
    @Test @MainActor func adoptAllForcesEveryDraftToTheNewScopesContent() async throws {
        let store = try await loadedStore()
        let view = try #require(store.view.value)

        var drafts = ConfigDrafts(global: "dirty", project: "dirty", lock: ["dirty"])
        drafts.adoptAll(from: view)

        #expect(drafts.global == view.rawGlobal)
        #expect(drafts.project == view.rawProject)
        #expect(drafts.lock == ["permission_mode", "policy.lock"])
        #expect(!drafts.hasUnsavedEdits(against: view))
    }
}
