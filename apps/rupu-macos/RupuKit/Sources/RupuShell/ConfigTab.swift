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
/// - **Effective** (`EffectiveConfigList`): the resolved config, grouped by
///   top-level section, with a provenance chip per key.
/// - **Raw** (`RawConfigEditor`): the raw TOML text for the Global layer (and
///   the Project layer, once a project is scoped) with dirty tracking and a
///   confirmation gate before an unsaved edit is discarded by a layer switch.
/// - **Policy** (`PolicyLockEditor`): the GLOBAL `[policy].lock` enforced-key
///   list.
///
/// **Project scope is one control for the whole tab**, not per-segment: the
/// picker above the segmented control drives `ConfigStore.load(client:
/// project:)`, and the Raw tab's "Project" layer is simply "whatever project
/// this picker currently has selected" rather than a second, independent
/// project chooser — `GET /api/config` only ever resolves ONE project layer
/// at a time, so there is nothing for a second picker to select that this
/// one doesn't already cover.
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

    /// Writing this binding both updates the picker's own selection AND
    /// re-`load`s the shared store against the new scope — the Raw tab's
    /// "Project" layer and the Policy tab's provenance both need the fresh
    /// scope's data, not just this row's own display.
    private func scopeBinding(store: ConfigStore) -> Binding<String?> {
        Binding(
            get: { selectedProjectID },
            set: { newValue in
                selectedProjectID = newValue
                guard let client = backend.client() else { return }
                Task { await store.load(client: client, project: newValue) }
            }
        )
    }

    @ViewBuilder
    private func body(store: ConfigStore) -> some View {
        switch store.view {
        case .loading:
            centeredLabel("Loading…")
        case .failed(let message):
            FailedNote(message: message)
        case .empty:
            EmptyView()
        case .content:
            switch segment {
            case .effective:
                EffectiveConfigList(store: store)
            case .raw:
                RawConfigEditor(store: store, backend: backend)
            case .policy:
                PolicyLockEditor(store: store, backend: backend)
            }
        }
    }

    /// Builds `store` (once) and activates it — same `storeClientID` recipe
    /// `ProjectDetailScreen`/`FleetScreen` use, see their `activate()` doc
    /// comments and `BackendController.clientIdentity()`. Also loads
    /// `projects` for the scope picker exactly once per client swap; a
    /// failed fetch leaves it at `[]` (an honest "Global only" picker,
    /// matching `ShellToolbar.loadProjects()`'s own fallback), never an
    /// error surface of its own.
    private func activate() async {
        guard let client = backend.client() else { return }
        let clientID = backend.clientIdentity()
        guard storeClientID != clientID else { return }
        storeClientID = clientID
        let newStore = ConfigStore()
        store = newStore
        async let projectsFetch: [APIProjectRow]? = try? client.projects()
        await newStore.load(client: client, project: selectedProjectID)
        projects = (await projectsFetch) ?? []
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
    /// Matches `ConfigStore.handleSaveError`'s fixed 501 message verbatim —
    /// see that method's doc comment. Shown proactively here (before any
    /// save is even attempted) so a read-only deployment never has to let an
    /// operator discover it by clicking Save first.
    static let readOnlyReason = "editing config requires `rupu cp serve`"

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
    /// One row per `view.provenance` entry (provenance's keys are already
    /// the wire's canonical dotted-key encoding — see `DottedKey`'s doc
    /// comment — so this is the full set of resolvable fields; `effective`
    /// itself is never walked top-down to discover keys, only to resolve
    /// each provenance key's value).
    static func rows(for view: APIConfigView) -> [EffectiveConfigRow] {
        view.provenance.keys.sorted().map { key in
            let segments = DottedKey.split(key)
            let section = segments.first ?? key
            let remainderSegments = Array(segments.dropFirst())
            let remainderDisplay = DottedKey.join(remainderSegments)
            let resolved = resolveValue(view.effective, segments: segments[...])
            let provenance = view.provenance[key]
            return EffectiveConfigRow(
                id: key,
                section: section,
                remainderDisplay: remainderDisplay.isEmpty ? section : remainderDisplay,
                valueDisplay: render(resolved),
                source: provenance?.source ?? .default,
                locked: provenance?.locked ?? false
            )
        }
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
    /// throws.
    private static func resolveValue(_ value: JSONValue, segments: ArraySlice<String>) -> JSONValue? {
        guard let first = segments.first else { return value }
        guard case .object(let dict) = value, let next = dict[first] else { return nil }
        return resolveValue(next, segments: segments.dropFirst())
    }

    /// Scalars render inline; arrays join their (already-rendered) elements
    /// with `", "`; a bare object (a provenance key that resolves to a
    /// whole table rather than a leaf — not expected given provenance is
    /// itself leaf-keyed, but handled rather than left to crash) renders a
    /// key count. `nil`/`.null` both render `—`.
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
/// glyph when `locked`, and a provenance chip.
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
                .frame(minWidth: 140, alignment: .leading)
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
/// Project layer). Tracks local edits per layer independently so switching
/// layers never loses either one's in-progress edit; a layer switch while
/// the CURRENT layer is dirty prompts via `confirmationDialog` rather than
/// silently discarding.
struct RawConfigEditor: View {
    let store: ConfigStore
    let backend: BackendController

    @State private var layer: RawLayer = .global
    @State private var globalText = ""
    @State private var projectText = ""
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
        // `initial: true` seeds local text from whatever `store.view`
        // already holds the first time this view appears (not just on a
        // later change) — a plain `.onChange` only fires on a CHANGE from
        // here on, which would leave the editor blank if the store had
        // already finished loading before this view mounted. Only synced
        // when NOT locally dirty, so a reload triggered by something else
        // (e.g. a project-scope switch) never clobbers an in-progress edit.
        .onChange(of: store.view.value?.rawGlobal, initial: true) { _, newValue in
            if !isGlobalDirty { globalText = newValue ?? "" }
        }
        .onChange(of: store.view.value?.rawProject, initial: true) { _, newValue in
            if !isProjectDirty { projectText = newValue ?? "" }
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

/// Editor over the GLOBAL `[policy].lock` enforced-key list. There is no
/// dedicated "current lock list" field on the wire — `policy.lock`'s own
/// resolved value lives wherever `effective.policy.lock` would be, which
/// this app never needs to reach into directly: every key the CURRENT lock
/// list enforces already shows up as `provenance[key].locked == true`
/// (that's exactly what "locked" means), so the list this editor seeds from
/// is `provenance`'s locked keys, sorted — no separate lookup needed.
struct PolicyLockEditor: View {
    let store: ConfigStore
    let backend: BackendController

    @State private var lockedKeys: [String] = []
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
        // Same `initial: true` + not-dirty-guard rationale as
        // `RawConfigEditor`'s own `onChange` pair.
        .onChange(of: store.view.value?.provenance, initial: true) { _, _ in
            if !isDirty { lockedKeys = currentLockKeys }
        }
    }

    /// Not `private` — `ConfigTabTests` reads this directly (via
    /// `@testable import`) to assert the Policy tab seeds its editable list
    /// from the fixture's locked provenance entries, without constructing a
    /// SwiftUI render pass.
    var currentLockKeys: [String] {
        guard let provenance = store.view.value?.provenance else { return [] }
        return provenance.filter(\.value.locked).map(\.key).sorted()
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
