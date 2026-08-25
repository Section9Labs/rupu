import SwiftUI
import RupuAPI
import RupuStore
import RupuDesign

/// The Settings **Config** tab (Phase 6A, Task 6) — a `ConfigStore`-backed
/// editor over `[cp]`/provider config, replacing `SettingsView`'s
/// placeholder shell. Three segments share ONE `ConfigStore` (built lazily
/// once `backend.client()` exists, rebuilt on a client swap — same
/// `storeClientID` recipe every other screen in this module uses, see
/// `BackendController.clientIdentity()`'s doc comment):
///
/// - **Effective** (`EffectiveConfigList`): every LEAF of the resolved
///   `effective` JSON tree, grouped by top-level section, with a provenance
///   chip per key (falling back to a `default` chip when the key has no
///   `provenance` entry at all — see `EffectiveConfigGrouping.rows(for:)`).
/// - **Raw** (`RawConfigEditor`): the raw TOML text for the Global layer (and
///   the Project layer, once a project is scoped) with dirty tracking and a
///   confirmation gate before an unsaved edit is discarded by a layer switch.
/// - **Policy** (`PolicyLockEditor`): the GLOBAL `[policy].lock` enforced-key
///   list, seeded from `effective.policy.lock` — see that type's doc comment
///   for why `provenance` is the wrong source.
///
/// **Project scope is one control for the whole tab**, not per-segment: the
/// picker above the segmented control drives `ConfigStore.load(client:
/// project:)`, and the Raw tab's "Project" layer is simply "whatever project
/// this picker currently has selected" rather than a second, independent
/// project chooser — `GET /api/config` only ever resolves ONE project layer
/// at a time, so there is nothing for a second picker to select that this
/// one doesn't already cover.
///
/// **Drafts live here, not in the sub-views (review-round fix).** The Raw
/// layer selection and its two draft texts, and the Policy lock draft, are
/// all `ConfigTab`-level `@State`, passed down to `RawConfigEditor`/
/// `PolicyLockEditor` as `@Binding`s. This is deliberate, not incidental:
/// `body(store:)` below switches on BOTH `store.view` (loading/failed/
/// content) AND `segment` (effective/raw/policy) to decide what to render,
/// and EVERY branch of a `switch` is a distinct position in the view tree —
/// SwiftUI tears down and rebuilds a child view's OWN `@State` whenever the
/// branch that constructs it changes, even transiently (a segment switch, OR
/// a reload that bounces `store.view` through `.loading` and back to
/// `.content`, e.g. a scope-picker switch or the reload every successful
/// save triggers). If the drafts lived as `@State` on `RawConfigEditor`/
/// `PolicyLockEditor` themselves (as an earlier version of this file did),
/// every one of those transitions would silently reset — or in the
/// scope-picker's case, silently repoint at the WRONG project's raw text —
/// an in-progress edit. Hoisting them into `content(store:)`'s own state (a
/// view that stays mounted across every one of those transitions; only the
/// `body(store:)` switch inside it changes what renders) fixes all three at
/// once: a segment switch simply changes which sub-view is REPRESENTING the
/// same underlying data, and a `.loading` bounce never touches state that
/// lives one level up from the thing being torn down.
///
/// **Does NOT need `OverviewScreen`'s cold-launch `.onChange(of: backend.
/// health)` fix**: like `ActivityScreen`/`FleetScreen`, this tab is only
/// ever reached by an explicit ⌘, after the shell (and therefore the
/// backend connection attempt) has already been running for a while — never
/// the very first thing a cold launch renders.
public struct ConfigTab: View {
    enum Segment: String, CaseIterable, Identifiable {
        case effective = "Effective"
        case raw = "Raw"
        case policy = "Policy"
        var id: String { rawValue }
    }

    let model: AppModel
    let backend: BackendController

    @State private var store: ConfigStore?
    @State private var storeClientID: ObjectIdentifier?
    @State private var projects: [APIProjectRow] = []
    @State private var selectedProjectID: String?
    @State private var segment: Segment = .effective

    // MARK: Hoisted drafts (see the type doc comment's "Drafts live here"
    // section). `rawLayer` is hoisted alongside the two raw texts so a
    // post-save reload can't reset the Raw tab back to Global either.
    @State private var rawLayer: RawLayer = .global
    @State private var globalDraft = ""
    @State private var projectDraft = ""
    @State private var lockDraft: [String] = []

    /// Non-`nil` while the "discard unsaved edits?" dialog for a SCOPE
    /// switch is up — see `requestScopeSwitch(to:store:)`.
    @State private var pendingScopeSwitch: ScopeSwitchTarget?

    public init(model: AppModel, backend: BackendController) {
        self.model = model
        self.backend = backend
    }

    public var body: some View {
        Group {
            if let store {
                content(store: store)
            } else {
                centeredLabel("Backend not connected")
            }
        }
        .frame(maxWidth: .infinity, minHeight: 340, alignment: .top)
        .padding(.top, 12)
        .task {
            await activate()
        }
    }

    private func content(store: ConfigStore) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            scopeRow(store: store)

            Picker("Section", selection: $segment) {
                ForEach(Segment.allCases) { s in
                    Text(s.rawValue).tag(s)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()

            body(store: store)
        }
        // Background-refresh sync: adopt whatever `store.view` currently
        // holds INTO the draft, but only while the draft isn't already
        // dirty relative to the LAST value this synced from — an
        // in-progress edit is never silently overwritten by a reload that
        // wasn't the operator's own save or a confirmed scope switch (both
        // of which bypass this guard deliberately — see
        // `performScopeSwitch(to:store:)` and this type's doc comment).
        // `initial: true` additionally seeds the draft from whatever
        // `store.view` already holds the first time `content(store:)`
        // mounts, not just on a later change.
        .onChange(of: store.view.value?.rawGlobal, initial: true) { _, newValue in
            if !isGlobalDirty(store: store) { globalDraft = newValue ?? "" }
        }
        .onChange(of: store.view.value?.rawProject, initial: true) { _, newValue in
            if !isProjectDirty(store: store) { projectDraft = newValue ?? "" }
        }
        .onChange(of: store.view.value.map { EffectiveConfigGrouping.policyLockKeys(for: $0) }, initial: true) { _, newValue in
            if !isPolicyDirty(store: store) { lockDraft = newValue ?? [] }
        }
        .confirmationDialog(
            "Discard unsaved edits?",
            isPresented: pendingScopeSwitchBinding,
            presenting: pendingScopeSwitch
        ) { target in
            Button("Discard and Switch", role: .destructive) {
                pendingScopeSwitch = nil
                Task { await performScopeSwitch(to: target.projectID, store: store) }
            }
            Button("Cancel", role: .cancel) { pendingScopeSwitch = nil }
        } message: { _ in
            Text("Switching project scope discards your unsaved Raw/Policy edits.")
        }
    }

    private func scopeRow(store: ConfigStore) -> some View {
        HStack(spacing: 8) {
            Text("Project scope")
                .font(.noteText)
                .foregroundStyle(Color.rupuDim)
            Picker("Project scope", selection: scopeBinding(store: store)) {
                Text("Global only").tag(nil as String?)
                ForEach(projects, id: \.wsID) { project in
                    Text(project.name).tag(project.wsID as String?)
                }
            }
            .labelsHidden()
            .frame(width: 200)
            Spacer(minLength: 0)
            if store.saving {
                ProgressView().controlSize(.small)
            }
        }
    }

    /// Routes every scope change through `requestScopeSwitch(to:store:)`
    /// rather than switching + `load`ing directly — see that method's doc
    /// comment. The picker's displayed selection still reads live off
    /// `selectedProjectID`, so a switch that ends up needing confirmation
    /// visually snaps back to the prior selection until the operator either
    /// confirms or cancels (same "reject and re-render the old state" idiom
    /// `FleetScreen`'s host-removal confirmation uses).
    private func scopeBinding(store: ConfigStore) -> Binding<String?> {
        Binding(
            get: { selectedProjectID },
            set: { newValue in requestScopeSwitch(to: newValue, store: store) }
        )
    }

    /// A no-op if `target` is already the current scope. Otherwise: with
    /// nothing unsaved, switches immediately; with an unsaved Raw or Policy
    /// draft, parks `target` in `pendingScopeSwitch` for the
    /// `confirmationDialog` in `content(store:)` to resolve — the SAME
    /// "never silently discard" contract `RawConfigEditor`'s in-editor
    /// layer button already enforced before this fix, now also covering the
    /// scope picker (review-round fix — previously this called `store.load`
    /// directly with no dirty check at all).
    private func requestScopeSwitch(to target: String?, store: ConfigStore) {
        guard target != selectedProjectID else { return }
        if hasUnsavedEdits(store: store) {
            pendingScopeSwitch = ScopeSwitchTarget(projectID: target)
        } else {
            Task { await performScopeSwitch(to: target, store: store) }
        }
    }

    /// Actually switches `selectedProjectID` and re-`load`s, then — once the
    /// new scope's content lands — force-adopts it into every draft,
    /// UNCONDITIONALLY (not the dirty-guarded sync `content(store:)`'s
    /// `onChange` triple uses for a background refresh). That bypass is
    /// safe specifically because getting here at all already means one of
    /// two things happened: `requestScopeSwitch` found nothing dirty to
    /// lose, or the operator just explicitly confirmed discarding it in the
    /// dialog — either way, the new scope's server content is exactly what
    /// every draft should show next.
    private func performScopeSwitch(to target: String?, store: ConfigStore) async {
        selectedProjectID = target
        guard let client = backend.client() else { return }
        await store.load(client: client, project: target)
        globalDraft = store.view.value?.rawGlobal ?? ""
        projectDraft = store.view.value?.rawProject ?? ""
        lockDraft = store.view.value.map { EffectiveConfigGrouping.policyLockKeys(for: $0) } ?? []
    }

    private var pendingScopeSwitchBinding: Binding<Bool> {
        Binding(get: { pendingScopeSwitch != nil }, set: { if !$0 { pendingScopeSwitch = nil } })
    }

    private func isGlobalDirty(store: ConfigStore) -> Bool {
        globalDraft != (store.view.value?.rawGlobal ?? "")
    }

    private func isProjectDirty(store: ConfigStore) -> Bool {
        projectDraft != (store.view.value?.rawProject ?? "")
    }

    private func isPolicyDirty(store: ConfigStore) -> Bool {
        let current = store.view.value.map { EffectiveConfigGrouping.policyLockKeys(for: $0) } ?? []
        return lockDraft.sorted() != current.sorted()
    }

    private func hasUnsavedEdits(store: ConfigStore) -> Bool {
        isGlobalDirty(store: store) || isProjectDirty(store: store) || isPolicyDirty(store: store)
    }

    @ViewBuilder
    private func body(store: ConfigStore) -> some View {
        switch store.view {
        case .loading:
            centeredLabel("Loading…")
        case .failed(let message):
            VStack(alignment: .leading, spacing: 10) {
                FailedNote(message: message)
                Button("Retry") {
                    Task { await retryLoad(store: store) }
                }
            }
        case .empty:
            // Structurally unreachable today — `ConfigStore.load` only ever
            // produces `.content`/`.failed` — but `BlockState` is a shared
            // generic type every switch over it must exhaust regardless.
            // An honest placeholder rather than a silent `EmptyView()`, in
            // case a future `ConfigStore` change ever does reach this.
            centeredLabel("No config data")
        case .content:
            switch segment {
            case .effective:
                EffectiveConfigList(store: store)
            case .raw:
                RawConfigEditor(
                    store: store,
                    backend: backend,
                    layer: $rawLayer,
                    globalText: $globalDraft,
                    projectText: $projectDraft
                )
            case .policy:
                PolicyLockEditor(store: store, backend: backend, lockedKeys: $lockDraft)
            }
        }
    }

    /// Builds `store` (once) and activates it — same `storeClientID` recipe
    /// `ProjectDetailScreen`/`FleetScreen` use, see their `activate()` doc
    /// comments and `BackendController.clientIdentity()`. Matches
    /// `ProjectDetailScreen.activate()`'s actual shape (review-round fix):
    /// the rebuild guard covers ONLY "do I need a NEW store" — `store.load`
    /// itself runs on every call, outside that guard, so a transient load
    /// failure (network blip, brief 5xx) doesn't strand the tab forever
    /// with `storeClientID` already set and no further `.task` trigger to
    /// retry it; the `FailedNote` path's Retry button (`retryLoad`) reaches
    /// this same unconditional `store.load` call directly. `projects` for
    /// the scope picker stays gated to "once per client swap" — an
    /// unrelated, cheap, idempotent fetch that a retry doesn't need to
    /// repeat — and a failed fetch leaves it at `[]` (an honest "Global
    /// only" picker, matching `ShellToolbar.loadProjects()`'s own
    /// fallback), never an error surface of its own.
    private func activate() async {
        guard let client = backend.client() else { return }
        let clientID = backend.clientIdentity()
        if storeClientID != clientID {
            storeClientID = clientID
            store = ConfigStore()
            Task { projects = (try? await client.projects()) ?? [] }
        }
        guard let store else { return }
        await store.load(client: client, project: selectedProjectID)
    }

    /// The `FailedNote` path's Retry button — re-runs the SAME `store.load`
    /// call `activate()` makes, without touching `storeClientID`/rebuilding
    /// `store`, since the client itself never became invalid (a load
    /// failure is not a client-identity change).
    private func retryLoad(store: ConfigStore) async {
        guard let client = backend.client() else { return }
        await store.load(client: client, project: selectedProjectID)
    }

    private func centeredLabel(_ label: String) -> some View {
        VStack {
            Spacer(minLength: 0)
            Text(label).font(.noteText).foregroundStyle(Color.rupuMute)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

/// Identifiable wrapper around a candidate scope-switch target — `String?`
/// itself can't drive `confirmationDialog(presenting:)` (which needs a
/// non-optional `Identifiable` to detect "is one currently pending", and a
/// bare `Optional<String>` can't distinguish "no pending switch" from "a
/// pending switch TO nil/Global"). `id` is a fresh `UUID` per instance
/// (rather than `projectID` itself) so requesting the SAME target twice in a
/// row — cancel, then pick it again — still re-triggers the dialog.
private struct ScopeSwitchTarget: Identifiable {
    let id = UUID()
    let projectID: String?
}

/// Local copy of the "Failed to load" note every other screen module's own
/// file already carries its own copy of (`ProjectDetailScreen`/
/// `CoverageDetailScreen`/`AgentDetailScreen`/`WorkflowDetailScreen`) —
/// `private` to this file, same as those.
private struct FailedNote: View {
    let message: String

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("Failed to load")
                .font(.noteText)
                .foregroundStyle(Color.status(.failed))
            Text(message)
                .font(.noteText)
                .foregroundStyle(Color.rupuDim)
                .lineLimit(3)
        }
    }
}

// MARK: - Shared save-gate reason

/// `nil` = the associated Save button is enabled. Shared by `RawConfigEditor`
/// and `PolicyLockEditor` so both explain a disabled Save with the same
/// wording, and `readOnly` always wins over "no changes" — a read-only
/// deployment's Save reads as read-only from the moment the tab opens, never
/// flickering to "no changes to save" before the operator has typed anything.
enum ConfigSaveGate {
    /// `ConfigStore.readOnlyMessage` itself — the SAME constant `saveError`
    /// gets set to on a real 501, not a re-typed copy of its text. Shown
    /// proactively here (before any save is even attempted) so a read-only
    /// deployment never has to let an operator discover it by clicking Save
    /// first.
    static let readOnlyReason = ConfigStore.readOnlyMessage

    static func reason(readOnly: Bool, isDirty: Bool) -> String? {
        if readOnly { return readOnlyReason }
        if !isDirty { return "no changes to save" }
        return nil
    }
}

// MARK: - Provenance chip

/// Tone-coded provenance badge — global/project/default — matching the web's
/// `SOURCE_CLASS` mapping (`crates/rupu-cp/web/src/components/settings/
/// ConfigField.tsx`): global = info (blue), project = ok (green), default =
/// neutral/mute. `Badge` is this design's existing tone-tinted chip
/// component; no new chrome primitive needed.
struct ProvenanceChip: View {
    let source: APIKeySource

    var body: some View {
        Badge(source.rawValue, tone: tone)
    }

    private var tone: Color {
        switch source {
        case .global: Color.rupuInfo
        case .project: Color.rupuOk
        case .default: Color.rupuMute
        }
    }
}

// MARK: - Effective tab

/// One resolved config key, ready to render: `id`/the full dotted key,
/// which top-level `section` it groups under, the remaining key segments
/// re-joined for display (`remainderDisplay`), its rendered value, and its
/// provenance. Built by `EffectiveConfigGrouping.rows(for:)` — kept as a
/// plain `Equatable` value type (not a `View`) so the grouping/filtering
/// logic is testable without constructing any SwiftUI view.
struct EffectiveConfigRow: Identifiable, Equatable {
    let id: String
    let section: String
    let remainderDisplay: String
    let valueDisplay: String
    let source: APIKeySource
    let locked: Bool
}

/// One top-level section's rows, in the order `EffectiveConfigGrouping.
/// grouped(_:)` produces them (alphabetical by section).
struct EffectiveConfigGroup: Identifiable, Equatable {
    let id: String
    let rows: [EffectiveConfigRow]
}

/// Pure data layer behind the Effective tab — deliberately free of any View
/// dependency so `ConfigTabTests` can assert the grouping/value-resolution
/// contract directly against a fixture-loaded `APIConfigView`, with no
/// SwiftUI rendering pass involved.
enum EffectiveConfigGrouping {
    /// Section name every single-segment top-level key (`default_model`,
    /// `log_level`, …) groups under, instead of each getting its own
    /// one-row section whose header just repeats the row below it
    /// (review-round fix — Q8).
    static let topLevelSectionName = "top level"

    /// One row per LEAF of `view.effective`'s JSON tree (review-round fix —
    /// Q4). Previously this enumerated `view.provenance`'s keys instead,
    /// which is WRONG: `provenance` only covers keys some layer explicitly
    /// SET (`rupu_config::resolve` only inserts an entry when a layer
    /// supplies a value), while `effective` is the FULL typed `Config`,
    /// defaults included — a key that's only ever at its default (no
    /// `provenance` entry at all, e.g. the fixture's `cp.bind`) is still a
    /// real, resolvable value and was previously invisible here. A leaf
    /// with no `provenance` entry falls back to a `default` chip
    /// (`provenance?.source ?? .default`), which is exactly correct: no
    /// entry means no layer overrode it, i.e. it IS at its default.
    ///
    /// A leaf is anything in the tree that isn't itself a JSON object —
    /// `leafPaths` stops descending at a scalar/array/null, matching
    /// `render`'s "arrays join inline, never expand into further rows"
    /// treatment.
    static func rows(for view: APIConfigView) -> [EffectiveConfigRow] {
        leafPaths(view.effective, prefix: []).map { segments in
            let key = DottedKey.join(segments)
            let isTopLevel = segments.count <= 1
            let section = isTopLevel ? topLevelSectionName : (segments.first ?? key)
            let remainderSegments = isTopLevel ? segments : Array(segments.dropFirst())
            let remainderDisplay = DottedKey.join(remainderSegments)
            let resolved = resolveValue(view.effective, segments: segments[...])
            let provenance = view.provenance[key]
            return EffectiveConfigRow(
                id: key,
                section: section,
                remainderDisplay: remainderDisplay.isEmpty ? key : remainderDisplay,
                valueDisplay: render(resolved),
                source: provenance?.source ?? .default,
                locked: provenance?.locked ?? false
            )
        }.sorted { $0.id < $1.id }
    }

    /// Recursively collects every LEAF path (segment list from root to a
    /// non-object value) under `value`. A genuinely empty object (`{}`,
    /// including a nested one like a hypothetical `providers = {}`) is
    /// itself treated as a leaf — there's nothing further to descend into,
    /// but it's still a real, renderable value (`render` shows it as
    /// `{}`) — EXCEPT at the true root with an empty `prefix`, where an
    /// empty `effective` object correctly produces zero rows rather than
    /// one row with no key at all.
    private static func leafPaths(_ value: JSONValue, prefix: [String]) -> [[String]] {
        guard case .object(let dict) = value else {
            return prefix.isEmpty ? [] : [prefix]
        }
        if dict.isEmpty {
            return prefix.isEmpty ? [] : [prefix]
        }
        var out: [[String]] = []
        for (key, child) in dict.sorted(by: { $0.key < $1.key }) {
            out.append(contentsOf: leafPaths(child, prefix: prefix + [key]))
        }
        return out
    }

    /// Groups already-built rows by `section`, sections alphabetical,
    /// each section's rows in whatever order `rows` handed them in
    /// (`rows(for:)` already sorts by full key, so this stays stable).
    static func grouped(_ rows: [EffectiveConfigRow]) -> [EffectiveConfigGroup] {
        var order: [String] = []
        var buckets: [String: [EffectiveConfigRow]] = [:]
        for row in rows {
            if buckets[row.section] == nil { order.append(row.section) }
            buckets[row.section, default: []].append(row)
        }
        return order.sorted().map { EffectiveConfigGroup(id: $0, rows: buckets[$0] ?? []) }
    }

    /// Case-insensitive substring match against the row's full key or its
    /// rendered value. Empty `search` is a no-op (every row passes).
    static func filtered(_ rows: [EffectiveConfigRow], search: String) -> [EffectiveConfigRow] {
        let needle = search.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !needle.isEmpty else { return rows }
        return rows.filter {
            $0.id.lowercased().contains(needle) || $0.valueDisplay.lowercased().contains(needle)
        }
    }

    /// Walks `value` by `segments`, descending one JSON object key per
    /// segment. Returns `nil` the moment a segment doesn't resolve (missing
    /// key, or the current node isn't an object at all) — the lenient-read
    /// contract: a key that fails to resolve renders `—`, never crashes or
    /// throws. Every path `rows(for:)` itself feeds this always resolves
    /// (it was discovered BY walking the same tree), so this only ever
    /// returns `nil` for a genuinely bogus caller-supplied path — e.g.
    /// `PolicyLockEditor.currentLockKeys`'s `["policy", "lock"]` lookup on a
    /// config that happens not to have a `policy` table at all.
    ///
    /// Not `private` — `PolicyLockEditor.currentLockKeys`/`policyLockKeys(
    /// for:)` reuse this SAME walker to read `effective.policy.lock` (see
    /// `PolicyLockEditor`'s doc comment for why `provenance` alone is the
    /// wrong source for the current lock list), rather than a second
    /// hand-rolled descent.
    static func resolveValue(_ value: JSONValue, segments: ArraySlice<String>) -> JSONValue? {
        guard let first = segments.first else { return value }
        guard case .object(let dict) = value, let next = dict[first] else { return nil }
        return resolveValue(next, segments: segments.dropFirst())
    }

    /// `effective.policy.lock` as a sorted `[String]`, or `[]` if that path
    /// doesn't resolve to a string array at all (a config with no `policy`
    /// table — shouldn't happen given `PolicyConfig` has no
    /// `skip_serializing_if`, but handled rather than crashing on a
    /// malformed/future payload). Shared by `PolicyLockEditor.
    /// currentLockKeys` and `ConfigTab`'s own dirty check, so there is
    /// exactly one place that knows how to read the enforced-key list off
    /// the wire.
    static func policyLockKeys(for view: APIConfigView) -> [String] {
        guard case .array(let items)? = resolveValue(view.effective, segments: ["policy", "lock"][...]) else {
            return []
        }
        return items.compactMap { item -> String? in
            guard case .string(let s) = item else { return nil }
            return s
        }.sorted()
    }

    /// Scalars render inline; arrays join their (already-rendered) elements
    /// with `", "`; a bare object (an empty table — see `leafPaths`'s doc
    /// comment) renders a key count. `nil`/`.null` both render `—`.
    static func render(_ value: JSONValue?) -> String {
        guard let value else { return "—" }
        switch value {
        case .null: return "—"
        case .string(let s): return s
        case .number(let n): return formatNumber(n)
        case .bool(let b): return b ? "true" : "false"
        case .array(let items):
            let rendered = items.map(renderScalar)
            return rendered.isEmpty ? "[]" : rendered.joined(separator: ", ")
        case .object(let dict):
            return dict.isEmpty ? "{}" : "{\(dict.count) key\(dict.count == 1 ? "" : "s")}"
        }
    }

    private static func renderScalar(_ value: JSONValue) -> String {
        switch value {
        case .null: return "—"
        case .string(let s): return s
        case .number(let n): return formatNumber(n)
        case .bool(let b): return b ? "true" : "false"
        case .array, .object: return "…"
        }
    }

    /// Integral doubles (the common case — byte counts, ports, timeouts)
    /// render without a trailing `.0`; anything else falls back to Swift's
    /// default `Double` description.
    private static func formatNumber(_ n: Double) -> String {
        if n.isFinite, n == n.rounded(), abs(n) < 1e15 {
            return String(Int64(n))
        }
        return String(n)
    }
}

/// Searchable list of every resolved config key, grouped by top-level
/// section, each row showing its remainder key, rendered value, a lock
/// glyph when `locked`, and a provenance chip. The list scrolls within a
/// bounded height (review-round fix — Q5) rather than growing the Settings
/// window to fit an arbitrarily large real config.
struct EffectiveConfigList: View {
    let store: ConfigStore

    @State private var searchText = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            searchField
            if let view = store.view.value {
                let rows = EffectiveConfigGrouping.filtered(EffectiveConfigGrouping.rows(for: view), search: searchText)
                let groups = EffectiveConfigGrouping.grouped(rows)
                if groups.isEmpty {
                    Text("No matching keys")
                        .font(.noteText)
                        .foregroundStyle(Color.rupuMute)
                        .frame(maxWidth: .infinity, alignment: .center)
                        .padding(.top, 24)
                } else {
                    ScrollView {
                        VStack(alignment: .leading, spacing: 12) {
                            ForEach(groups) { group in
                                sectionCard(group)
                            }
                        }
                    }
                    .frame(maxHeight: 380)
                }
            }
        }
    }

    private var searchField: some View {
        HStack(spacing: 6) {
            Icon(.search, size: 12)
                .foregroundStyle(Color.rupuMute)
            TextField("Search keys…", text: $searchText)
                .textFieldStyle(.plain)
                .font(.uiText)
        }
        .padding(8)
        .panelStyle(.innerCard)
    }

    private func sectionCard(_ group: EffectiveConfigGroup) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Eyebrow(group.id)
            ForEach(group.rows) { row in
                effectiveRow(row)
            }
        }
        .padding(10)
        .panelStyle(.innerCard)
    }

    private func effectiveRow(_ row: EffectiveConfigRow) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Text(row.remainderDisplay)
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuDim)
                .lineLimit(1)
                .truncationMode(.tail)
                .frame(minWidth: 140, alignment: .leading)
                .help(row.id)
            Text(row.valueDisplay)
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuInk)
                .lineLimit(2)
                .truncationMode(.tail)
                .frame(maxWidth: .infinity, alignment: .leading)
            if row.locked {
                Icon(.lock, size: 10)
                    .foregroundStyle(Color.rupuWarn)
            }
            ProvenanceChip(source: row.source)
        }
    }
}

// MARK: - Raw tab

/// Which raw-TOML layer `RawConfigEditor` is currently showing/editing.
/// `.project` is only selectable while `ConfigStore.selectedProject != nil`
/// — see `ConfigTab`'s "one project scope for the whole tab" doc comment.
enum RawLayer: String, CaseIterable, Identifiable {
    case global, project
    var id: String { rawValue }
    var label: String { self == .global ? "Global" : "Project" }
}

/// Raw TOML editor for the Global layer (and, once a project is scoped, the
/// Project layer). `layer`/`globalText`/`projectText` are `@Binding`s into
/// `ConfigTab`'s own `@State` (review-round fix — see that type's "Drafts
/// live here" doc comment) rather than owned locally, so switching Effective/
/// Raw/Policy segments — which tears down and rebuilds THIS view — never
/// loses either draft or which layer was selected. A layer switch while the
/// CURRENT layer is dirty still prompts via `confirmationDialog` rather than
/// silently discarding — that part is unchanged and stays local (`
/// pendingLayerSwitch` is transient UI state, not user data).
struct RawConfigEditor: View {
    let store: ConfigStore
    let backend: BackendController
    @Binding var layer: RawLayer
    @Binding var globalText: String
    @Binding var projectText: String

    @State private var pendingLayerSwitch: RawLayer?

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            layerPicker
            if let error = store.saveError {
                Text(error)
                    .font(.noteText)
                    .foregroundStyle(Color.status(.failed))
                    .lineLimit(4)
            }
            if !store.lastSaveRestartKeys.isEmpty {
                restartBanner
            }
            editor
            actions
        }
        .confirmationDialog(
            "Discard unsaved edits?",
            isPresented: pendingSwitchBinding,
            presenting: pendingLayerSwitch
        ) { target in
            Button("Discard and Switch", role: .destructive) {
                layer = target
                pendingLayerSwitch = nil
            }
            Button("Cancel", role: .cancel) { pendingLayerSwitch = nil }
        } message: { _ in
            Text("Switching layers discards your unsaved changes to the \(layer.label) layer.")
        }
    }

    private var isGlobalDirty: Bool { globalText != (store.view.value?.rawGlobal ?? "") }
    private var isProjectDirty: Bool { projectText != (store.view.value?.rawProject ?? "") }
    private var isDirty: Bool { layer == .global ? isGlobalDirty : isProjectDirty }

    private var pendingSwitchBinding: Binding<Bool> {
        Binding(get: { pendingLayerSwitch != nil }, set: { if !$0 { pendingLayerSwitch = nil } })
    }

    private var layerPicker: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                layerButton(.global)
                layerButton(.project)
            }
            if store.selectedProject == nil {
                Text("Select a project above to edit its layer.")
                    .font(.noteText)
                    .foregroundStyle(Color.rupuMute)
            }
        }
    }

    private func layerButton(_ target: RawLayer) -> some View {
        let isSelected = layer == target
        let disabled = target == .project && store.selectedProject == nil
        return Button {
            requestLayerSwitch(to: target)
        } label: {
            Text(target.label)
                .font(.uiText)
                .foregroundStyle(isSelected ? Color.rupuInk : Color.rupuDim)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 6)
                .background(isSelected ? Color.rupuSurfaceActive : Color.rupuSurface)
                .clipShape(RoundedRectangle(cornerRadius: 6))
        }
        .buttonStyle(.plain)
        .disabled(disabled)
    }

    private func requestLayerSwitch(to target: RawLayer) {
        guard target != layer else { return }
        if isDirty {
            pendingLayerSwitch = target
        } else {
            layer = target
        }
    }

    private var editor: some View {
        TextEditor(text: layer == .global ? $globalText : $projectText)
            .font(.dataMono(11))
            .scrollContentBackground(.hidden)
            .frame(minHeight: 220)
            .padding(8)
            .panelStyle(.innerCard)
            .disabled(store.readOnly)
    }

    private var restartBanner: some View {
        HStack(spacing: 6) {
            Icon(.clock, size: 11)
                .foregroundStyle(Color.rupuWarn)
            Text("May require a `cp serve` restart to take effect: \(store.lastSaveRestartKeys.joined(separator: ", "))")
                .font(.noteText)
                .foregroundStyle(Color.rupuWarn)
        }
    }

    private var actions: some View {
        HStack(spacing: 8) {
            if isDirty {
                Button("Discard") { discardCurrentLayer() }
                    .buttonStyle(.plain)
                    .foregroundStyle(Color.rupuDim)
            }
            Spacer(minLength: 0)
            if let reason = ConfigSaveGate.reason(readOnly: store.readOnly, isDirty: isDirty) {
                Text(reason)
                    .font(.noteText)
                    .foregroundStyle(Color.rupuMute)
            }
            Button(store.saving ? "Saving…" : "Save") {
                Task { await save() }
            }
            .disabled(store.readOnly || !isDirty || store.saving)
        }
    }

    private func discardCurrentLayer() {
        switch layer {
        case .global: globalText = store.view.value?.rawGlobal ?? ""
        case .project: projectText = store.view.value?.rawProject ?? ""
        }
    }

    private func save() async {
        guard let client = backend.client() else { return }
        switch layer {
        case .global:
            _ = await store.saveGlobalRaw(globalText, client: client)
        case .project:
            _ = await store.saveProjectRaw(projectText, client: client)
        }
    }
}

// MARK: - Policy tab

/// Editor over the GLOBAL `[policy].lock` enforced-key list, seeded from
/// `effective.policy.lock` — NOT from `provenance`'s locked keys (a prior
/// version of this type made that mistake; see the correction below, kept
/// as a warning against repeating it). `lockedKeys` is a `@Binding` into
/// `ConfigTab`'s own `@State` (review-round fix — see that type's "Drafts
/// live here" doc comment), not owned locally, so an Effective/Raw/Policy
/// segment switch never loses an in-progress lock-list edit.
///
/// **Why `provenance` is the WRONG source.** `rupu_config::resolve` only
/// inserts a `provenance` entry for a key when some layer actually SETS a
/// value for it (`resolve.rs`: `if let Some(v) = val { ... provenance.
/// insert(...) }`). A lock list can name a key that no layer ever sets —
/// perfectly valid TOML (`[policy]\nlock = ["permission_mode"]` with no
/// `permission_mode = ...` anywhere) — and that key then has NO `provenance`
/// entry at all, locked or otherwise. Seeding this editor from `provenance`'s
/// `locked == true` keys would silently drop that entry from the visible
/// list, and since `savePolicy` PUTs the COMPLETE list back, the next save
/// would silently delete it from the real lock list too — a data-loss bug,
/// not just a display gap.
///
/// `effective.policy.lock` has no such gap: `resolve()` unconditionally pins
/// `config.policy.lock` to the full GLOBAL-derived lock array regardless of
/// whether any of its entries resolve to anything (`resolve.rs`'s `config.
/// policy.lock = lock.clone()`), and `PolicyConfig::lock` has no
/// `skip_serializing_if`, so it's always present on the wire — this is
/// exactly what the web reads too (`pages/Settings.tsx`). Walked via
/// `EffectiveConfigGrouping.policyLockKeys(for:)`, the same
/// `resolveValue` walker the Effective tab uses.
struct PolicyLockEditor: View {
    let store: ConfigStore
    let backend: BackendController
    @Binding var lockedKeys: [String]

    @State private var newKeyText = ""
    @State private var pickerSelection = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            currentLockList
            addRow
            if let error = store.saveError {
                Text(error)
                    .font(.noteText)
                    .foregroundStyle(Color.status(.failed))
                    .lineLimit(4)
            }
            actions
        }
    }

    /// Not `private` — `ConfigTabTests` reads this directly (via
    /// `@testable import`) to assert the Policy tab's Discard button (and
    /// its dirty check) reads `effective.policy.lock`, WITHOUT constructing
    /// a SwiftUI render pass — including the case of a lock entry with no
    /// `provenance` entry at all (see this type's doc comment). Delegates
    /// to `EffectiveConfigGrouping.policyLockKeys(for:)`, the same helper
    /// `ConfigTab`'s own dirty check uses, so there is exactly one
    /// implementation of "how do we read the enforced-key list."
    var currentLockKeys: [String] {
        guard let view = store.view.value else { return [] }
        return EffectiveConfigGrouping.policyLockKeys(for: view)
    }

    private var isDirty: Bool { lockedKeys.sorted() != currentLockKeys }

    private var currentLockList: some View {
        VStack(alignment: .leading, spacing: 6) {
            Eyebrow("Locked keys")
            if lockedKeys.isEmpty {
                Text("No keys locked")
                    .font(.noteText)
                    .foregroundStyle(Color.rupuMute)
            } else {
                ForEach(lockedKeys, id: \.self) { key in
                    HStack(spacing: 8) {
                        Icon(.lock, size: 11)
                            .foregroundStyle(Color.rupuWarn)
                        Text(key)
                            .font(.dataMono(11))
                            .foregroundStyle(Color.rupuInk)
                        Spacer(minLength: 0)
                        Button {
                            lockedKeys.removeAll { $0 == key }
                        } label: {
                            Icon(.trash2, size: 11)
                                .foregroundStyle(Color.rupuMute)
                        }
                        .buttonStyle(.plain)
                        .disabled(store.readOnly)
                    }
                    .padding(.vertical, 2)
                }
            }
        }
    }

    private var addRow: some View {
        VStack(alignment: .leading, spacing: 6) {
            Eyebrow("Lock a key")
            HStack(spacing: 8) {
                Picker("Lock a resolved key", selection: $pickerSelection) {
                    Text("Choose a resolved key…").tag("")
                    ForEach(lockablePickerKeys, id: \.self) { key in
                        Text(key).tag(key)
                    }
                }
                .labelsHidden()
                .onChange(of: pickerSelection) { _, newValue in
                    guard !newValue.isEmpty else { return }
                    addLock(newValue)
                    pickerSelection = ""
                }

                TextField("or type a dotted key…", text: $newKeyText)
                    .textFieldStyle(.plain)
                    .font(.dataMono(11))
                    .onSubmit { submitTypedKey() }

                Button("Add") { submitTypedKey() }
                    .disabled(newKeyText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }
        .disabled(store.readOnly)
    }

    /// Populated from `provenance`'s keys verbatim (the canonical dotted-key
    /// encoding straight off the wire — no re-derivation), already-locked
    /// keys excluded so the picker only ever offers something new to add.
    /// (Unlike `currentLockKeys`, this is deliberately still `provenance`-
    /// sourced — it's a convenience list of keys KNOWN to resolve to
    /// something, for the "lock one of these" shortcut; the free-text field
    /// right next to it covers any other key, e.g. one with no `provenance`
    /// entry, same as the "permission_mode" case `currentLockKeys` itself
    /// must handle.)
    private var lockablePickerKeys: [String] {
        guard let provenance = store.view.value?.provenance else { return [] }
        return provenance.keys.filter { !lockedKeys.contains($0) }.sorted()
    }

    private func submitTypedKey() {
        addLock(newKeyText)
        newKeyText = ""
    }

    /// Free text, sent verbatim — this editor does no client-side validation
    /// of the key's shape; the server validates on `PUT /api/config/policy`
    /// and a bad key surfaces there as a 400, same as any other save
    /// failure.
    private func addLock(_ key: String) {
        let trimmed = key.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, !lockedKeys.contains(trimmed) else { return }
        lockedKeys.append(trimmed)
    }

    private var actions: some View {
        HStack(spacing: 8) {
            if isDirty {
                Button("Discard") { lockedKeys = currentLockKeys }
                    .buttonStyle(.plain)
                    .foregroundStyle(Color.rupuDim)
            }
            Spacer(minLength: 0)
            if let reason = ConfigSaveGate.reason(readOnly: store.readOnly, isDirty: isDirty) {
                Text(reason)
                    .font(.noteText)
                    .foregroundStyle(Color.rupuMute)
            }
            Button(store.saving ? "Saving…" : "Save") {
                Task { await save() }
            }
            .disabled(store.readOnly || !isDirty || store.saving)
        }
    }

    private func save() async {
        guard let client = backend.client() else { return }
        _ = await store.savePolicy(lock: lockedKeys.sorted(), client: client)
    }
}
