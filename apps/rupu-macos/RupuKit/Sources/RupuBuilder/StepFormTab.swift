import SwiftUI
import RupuAPI
import RupuDesign
import RupuFlowKit

// The inspector rail's Step tab (macOS Workflow Builder, Task 13): every
// per-kind field vocabulary from spec §7 / the web's `StepForm.tsx`, bound
// to `BuilderStore.updateStep(_:)` — each edit copies the SELECTED node's
// `StepNodeData`, mutates exactly one field, and re-submits the whole
// struct. Every field holds its own local `@State` text so typing is never
// interrupted by a re-render off the store; edits commit on a 300ms
// debounce AND on focus loss/submit (never per keystroke onto the store),
// except STEP ID (routes through `store.rename(id:to:)`, submit/blur only —
// a rename is a structural edit, not a text edit) and Step ID's sibling
// NAME field on the Settings tab (same rationale, see `SettingsTab.swift`).

// MARK: - Pure helpers (tested directly in StepFormModelTests, no SwiftUI render pass)

/// Read-only rows derived from graph structure/data — shown below a branch/
/// split/join node's editable fields. Empty for every other kind. A row's
/// `value` is the plain comma-joined list (no arrow decoration); the view
/// layer prepends the kind-appropriate arrow (`→` for branch/split, `←` for
/// join's inbound-edge "waits on" row).
func derivedRows(for node: GraphNode, graph: WorkflowGraph) -> [(label: String, value: String)] {
    switch node.data.kind {
    case .branch:
        let then = node.data.thenTargets ?? []
        let elseTargets = node.data.elseTargets ?? []
        return [
            (label: "then", value: then.isEmpty ? "—" : then.joined(separator: ", ")),
            (label: "else", value: elseTargets.isEmpty ? "—" : elseTargets.joined(separator: ", ")),
        ]
    case .split:
        let targets = node.data.split ?? []
        return [(label: "targets", value: targets.isEmpty ? "—" : targets.joined(separator: ", "))]
    case .join:
        // `data.split`/`thenTargets` aren't how a join's incoming edges are
        // stored — a join is a pure barrier, its predecessors point AT it
        // via their own `next:` (or `split:`/`depends_on:`). The graph's
        // already-derived edges are the one correct source: every edge
        // whose `target` is this node.
        let sources = graph.edges.filter { $0.target == node.id }.map(\.source)
        return [(label: "waits on", value: sources.isEmpty ? "—" : sources.joined(separator: ", "))]
    default:
        return []
    }
}

/// Sets `run:`'s `cmd:` key on `data.runBlock`, preserving every sibling key
/// (`args:`, etc. — anything else a `run:` block carries) via `YAMLValue.
/// mapping(settingKey:to:)`. `runBlock == nil` (a freshly-added `run` node)
/// seeds a bare one-key mapping rather than requiring a prior `with:`-style
/// scaffold.
func settingRunCommand(_ data: StepNodeData, to command: String) -> StepNodeData {
    var copy = data
    let base = data.runBlock ?? .mapping([])
    copy.runBlock = base.mapping(settingKey: "cmd", to: .string(command))
    return copy
}

/// Parse-or-hold result for the action `with:` sub-editor's raw-YAML text.
/// Empty text is a valid "clear the with: block" edit (`.success(nil)`); a
/// parse error is surfaced inline without ever touching `data.with` — the
/// last successfully-parsed value stays committed until the text parses
/// again.
enum WithParseResult: Equatable {
    case success(YAMLValue?)
    case failure(String)
}

func parseWithText(_ text: String) -> WithParseResult {
    let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty else { return .success(nil) }
    do {
        return .success(try YAMLParser.parse(text))
    } catch let error as YAMLError {
        return .failure(withParseErrorMessage(error))
    } catch {
        return .failure(String(describing: error))
    }
}

private func withParseErrorMessage(_ error: YAMLError) -> String {
    switch error {
    case .unsupported(let message, let line):
        return "\(message) (line \(line))"
    case .malformed(let message, let line):
        return "\(message) (line \(line))"
    }
}

/// The AGENT picker's option list: every catalog name, plus the currently
/// selected value appended if it isn't in the catalog — an agent renamed or
/// deleted out from under a saved step must still display (and round-trip
/// unchanged if the user never touches this field) rather than silently
/// reset to the first catalog entry.
func agentPickerOptions(current: String?, agents: [AgentDefinition]) -> [String] {
    var names = agents.map(\.name)
    if let current, !current.isEmpty, !names.contains(current) {
        names.append(current)
    }
    return names
}

// MARK: - StepFormTab

/// The rail's "Step" tab body — the empty state when nothing is selected,
/// otherwise `StepFormBody` keyed on the selected node's id so every field's
/// local `@State` resets cleanly on selection change (and stays put across
/// re-renders the SAME node's own edits cause).
struct StepFormTab: View {
    @Bindable var store: BuilderStore

    var body: some View {
        if let id = store.selectedID, let node = store.graph.nodes.first(where: { $0.id == id }) {
            StepFormBody(store: store, node: node)
                .id(node.id)
        } else {
            Text("Select a step on the canvas")
                .font(.noteText)
                .foregroundStyle(Color.rupuMute)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .center)
        }
    }
}

/// One selected node's editable form. `node` is a snapshot from the current
/// render — every field-commit closure below reads `node.data` FRESH at the
/// moment it's called (captured by reference through `self`, not copied at
/// init), so chaining several field edits in the same node always mutates
/// off the latest committed data, never a stale snapshot from first mount.
private struct StepFormBody: View {
    @Bindable var store: BuilderStore
    let node: GraphNode

    @State private var idText: String
    @State private var idError: String?

    init(store: BuilderStore, node: GraphNode) {
        self.store = store
        self.node = node
        _idText = State(initialValue: node.id)
    }

    private var visual: KindVisual { kindVisual(node.data.kind) }
    private var accent: Color { RupuBuilder.accentColor(visual.accent) }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                header
                idField
                kindFields
                derivedRowsSection
                removeButton
            }
            .padding(12)
        }
    }

    // MARK: - Header

    private var header: some View {
        HStack(spacing: 8) {
            SilhouetteShape(name: visual.shape)
                .stroke(accent, lineWidth: 1.2)
                .frame(width: 44, height: 26)
            VStack(alignment: .leading, spacing: 2) {
                Text(node.id)
                    .font(.dataMono(12))
                    .foregroundStyle(Color.rupuInk)
                    .lineLimit(1)
                Badge(node.data.kind.rawValue, tone: accent)
            }
            Spacer(minLength: 0)
        }
    }

    // MARK: - Step ID (every kind — routes through rename, not updateStep)

    private var idField: some View {
        VStack(alignment: .leading, spacing: 4) {
            CommitOnBlurField(text: $idText, font: .dataMono(11)) { newID in
                guard newID != node.id else { return }
                if !store.rename(id: node.id, to: newID) {
                    idError = store.commitError
                    idText = node.id
                } else {
                    idError = nil
                }
            } label: { Eyebrow("STEP ID") }
            if let idError {
                Text(idError)
                    .font(.noteText)
                    .foregroundStyle(Color.rupuErr)
            }
        }
    }

    // MARK: - Mutation helper (every per-kind field below routes through this)

    /// Forwards straight to `store.updateSelectedStep(_:)` — NEVER reads
    /// `self.node.data` here (review fix, Finding 1: a debounce `Task`
    /// captures its enclosing `self` — a value-type snapshot — back when
    /// the keystroke that scheduled it fired, so reading `node.data` at
    /// FIRE time would silently apply a stale, possibly-already-superseded
    /// copy of every OTHER field, or target an id a mid-debounce rename
    /// already moved past). `store` is a class reference, so even a stale
    /// captured `self` still calls through to the LIVE store, which
    /// resolves the current selection and its current data itself.
    private func commit(_ mutate: (inout StepNodeData) -> Void) {
        store.updateSelectedStep(mutate)
    }

    // MARK: - Per-kind fields

    @ViewBuilder
    private var kindFields: some View {
        switch node.data.kind {
        case .step:
            agentField
            promptField()
        case .forEach:
            agentField
            promptField()
            forEachField
            maxParallelField
        case .parallel:
            parallelField
            maxParallelField
        case .panel:
            panelistsField
            subjectField
            promptField(optional: true)
            maxParallelField
        case .branch:
            conditionField
        case .split:
            EmptyView()
        case .join:
            joinWaitField
        case .approvalGate:
            approvalPromptField
            timeoutField
            onTimeoutField
        case .action:
            toolField
            withField
        case .run:
            commandField
        }
    }

    // MARK: - step / for_each shared fields

    private var agentField: some View {
        VStack(alignment: .leading, spacing: 4) {
            Eyebrow("AGENT")
            Picker(
                "",
                selection: Binding(
                    get: { node.data.agent ?? "" },
                    set: { newValue in commit { $0.agent = newValue } }
                )
            ) {
                ForEach(agentPickerOptions(current: node.data.agent, agents: store.agents), id: \.self) { name in
                    Text(name).tag(name)
                }
            }
            .labelsHidden()
            .pickerStyle(.menu)
        }
    }

    private func promptField(optional: Bool = false) -> some View {
        DebouncedTextEditor(
            label: optional ? "PROMPT (OPTIONAL)" : "PROMPT",
            initial: node.data.prompt ?? "",
            font: .uiText,
            minHeight: 88
        ) { newValue in
            commit { $0.prompt = newValue.isEmpty ? nil : newValue }
        }
    }

    private var forEachField: some View {
        DebouncedField(label: "FOR EACH", initial: node.data.forEach ?? "", font: .dataMono(11)) { newValue in
            commit { $0.forEach = newValue.isEmpty ? nil : newValue }
        }
    }

    private var maxParallelField: some View {
        DebouncedNumericField(label: "MAX PARALLEL", initial: node.data.maxParallel.map(String.init) ?? "") { newValue in
            commit { $0.maxParallel = newValue }
        }
    }

    // MARK: - parallel

    private var parallelField: some View {
        VStack(alignment: .leading, spacing: 6) {
            Eyebrow("SUB-STEPS")
            let subSteps = node.data.parallel ?? []
            // Keyed on "<index>-<id>" (review fix, Finding 2), not the
            // bare `.offset` this used to use — a plain offset key means
            // removing row N leaves every row AFTER it sitting at the SAME
            // ForEach identity it always had, so SwiftUI treats it as "the
            // same row" and never re-seeds `SubStepRow`'s local `@State`
            // text: the id/prompt fields kept showing the REMOVED row's
            // stale text bound to what is now a DIFFERENT sub-step
            // underneath. `id` is folded into the key (not just `index`)
            // so a genuine content change (add/remove/reorder) always
            // mints a fresh identity; two rows sharing an id — a user
            // hasn't renamed a freshly-added one yet — still get distinct
            // keys because `index` differs.
            ForEach(Array(subSteps.enumerated()), id: \.offset) { index, subStep in
                SubStepRow(
                    subStep: subStep,
                    agents: store.agents,
                    onChangeID: { newID in commitSubStep(at: index) { $0.id = newID } },
                    onChangeAgent: { newAgent in commitSubStep(at: index) { $0.agent = newAgent } },
                    onChangePrompt: { newPrompt in commitSubStep(at: index) { $0.prompt = newPrompt } },
                    onRemove: { removeSubStep(at: index) }
                )
                .id("\(index)-\(subStep.id)")
            }
            Button {
                addSubStep()
            } label: {
                Text("+ Add sub-step")
                    .font(.noteText)
                    .foregroundStyle(Color.rupuBrand)
            }
            .buttonStyle(.plain)
        }
    }

    private func commitSubStep(at index: Int, mutate: (inout SubStep) -> Void) {
        commit { data in
            var list = data.parallel ?? []
            guard index < list.count else { return }
            mutate(&list[index])
            data.parallel = list
        }
    }

    private func addSubStep() {
        commit { data in
            var list = data.parallel ?? []
            list.append(SubStep(id: freshRowID(existing: list.map(\.id), prefix: "sub"), agent: "", prompt: ""))
            data.parallel = list
        }
    }

    private func removeSubStep(at index: Int) {
        commit { data in
            var list = data.parallel ?? []
            guard index < list.count else { return }
            list.remove(at: index)
            data.parallel = list
        }
    }

    // MARK: - panel

    private var panelistsField: some View {
        VStack(alignment: .leading, spacing: 6) {
            Eyebrow("PANELISTS")
            let panelists = node.data.panel?.panelists ?? []
            // Same composite-identity fix as `parallelField`'s sub-step
            // rows above (review fix, Finding 2) — panelists are bare
            // strings (duplicates entirely possible, e.g. two rows still
            // defaulted to the same agent), so the key folds `index` in
            // ahead of the value specifically so two same-valued rows
            // never collide.
            ForEach(Array(panelists.enumerated()), id: \.offset) { index, panelist in
                HStack(spacing: 6) {
                    Picker(
                        "",
                        selection: Binding(
                            get: { panelist },
                            set: { newValue in commitPanelist(at: index, to: newValue) }
                        )
                    ) {
                        ForEach(agentPickerOptions(current: panelist, agents: store.agents), id: \.self) { name in
                            Text(name).tag(name)
                        }
                    }
                    .labelsHidden()
                    .pickerStyle(.menu)
                    Button {
                        removePanelist(at: index)
                    } label: {
                        Text("×")
                            .font(.uiText)
                            .foregroundStyle(Color.rupuDim)
                    }
                    .buttonStyle(.plain)
                }
                .id("\(index)-\(panelist)")
            }
            Button {
                addPanelist()
            } label: {
                Text("+ Add panelist")
                    .font(.noteText)
                    .foregroundStyle(Color.rupuBrand)
            }
            .buttonStyle(.plain)
        }
    }

    private func commitPanelist(at index: Int, to newValue: String) {
        commit { data in
            var panel = data.panel ?? PanelCfg(panelists: [], subject: "")
            guard index < panel.panelists.count else { return }
            panel.panelists[index] = newValue
            data.panel = panel
        }
    }

    private func addPanelist() {
        commit { data in
            var panel = data.panel ?? PanelCfg(panelists: [], subject: "")
            let fallback = store.agents.first?.name ?? ""
            panel.panelists.append(fallback)
            data.panel = panel
        }
    }

    private func removePanelist(at index: Int) {
        commit { data in
            guard var panel = data.panel, index < panel.panelists.count else { return }
            panel.panelists.remove(at: index)
            data.panel = panel
        }
    }

    private var subjectField: some View {
        DebouncedField(label: "SUBJECT", initial: node.data.panel?.subject ?? "", font: .dataMono(11)) { newValue in
            commit { data in
                var panel = data.panel ?? PanelCfg(panelists: [], subject: "")
                panel.subject = newValue
                data.panel = panel
            }
        }
    }

    // MARK: - branch

    private var conditionField: some View {
        DebouncedField(label: "CONDITION", initial: node.data.condition ?? "", font: .dataMono(11)) { newValue in
            commit { $0.condition = newValue.isEmpty ? nil : newValue }
        }
    }

    // MARK: - join

    private var joinWaitField: some View {
        VStack(alignment: .leading, spacing: 8) {
            VStack(alignment: .leading, spacing: 4) {
                Eyebrow("WAIT POLICY")
                Picker(
                    "",
                    selection: Binding(
                        get: { joinWaitKind(node.data.joinWait) },
                        set: { newKind in commit { $0.joinWait = joinWait(forKind: newKind, count: joinWaitCount(node.data.joinWait)) } }
                    )
                ) {
                    Text("all").tag("all")
                    Text("any").tag("any")
                    Text("count").tag("count")
                }
                .labelsHidden()
                .pickerStyle(.segmented)
            }
            if joinWaitKind(node.data.joinWait) == "count" {
                DebouncedNumericField(label: "COUNT", initial: joinWaitCount(node.data.joinWait).map(String.init) ?? "") { newValue in
                    commit { $0.joinWait = .count(newValue ?? 0) }
                }
            }
        }
    }

    // MARK: - approval_gate

    private var approvalPromptField: some View {
        DebouncedTextEditor(
            label: "APPROVAL PROMPT", initial: node.data.approvalPrompt ?? "", font: .uiText, minHeight: 60
        ) { newValue in
            commit { $0.approvalPrompt = newValue.isEmpty ? nil : newValue }
        }
    }

    private var timeoutField: some View {
        DebouncedNumericField(
            label: "TIMEOUT (SECONDS)", initial: node.data.approvalTimeoutSeconds.map(String.init) ?? ""
        ) { newValue in
            commit { $0.approvalTimeoutSeconds = newValue }
        }
    }

    private var onTimeoutField: some View {
        VStack(alignment: .leading, spacing: 4) {
            Eyebrow("ON TIMEOUT")
            Picker(
                "",
                selection: Binding(
                    get: { node.data.approvalOnTimeout ?? "—" },
                    set: { newValue in commit { $0.approvalOnTimeout = newValue == "—" ? nil : newValue } }
                )
            ) {
                ForEach(["—", "approve", "reject", "fail"], id: \.self) { Text($0).tag($0) }
            }
            .labelsHidden()
            .pickerStyle(.menu)
        }
    }

    // MARK: - action

    private var toolField: some View {
        VStack(alignment: .leading, spacing: 4) {
            Eyebrow("TOOL")
            if store.tools.isEmpty {
                DebouncedField(label: "", initial: node.data.action ?? "", font: .dataMono(11)) { newValue in
                    commit { $0.action = newValue.isEmpty ? nil : newValue }
                }
            } else {
                Picker(
                    "",
                    selection: Binding(
                        get: { node.data.action ?? "" },
                        set: { newValue in commit { $0.action = newValue } }
                    )
                ) {
                    ForEach(toolPickerOptions(current: node.data.action, tools: store.tools), id: \.self) { name in
                        Text(name).tag(name)
                    }
                }
                .labelsHidden()
                .pickerStyle(.menu)
            }
        }
    }

    private var withField: some View {
        WithEditor(initial: node.data.with) { newValue in
            commit { $0.with = newValue }
        }
    }

    // MARK: - run

    private var commandField: some View {
        DebouncedField(
            label: "COMMAND", initial: node.data.runBlock?["cmd"]?.stringValue ?? "", font: .dataMono(11)
        ) { newValue in
            commit { $0 = settingRunCommand($0, to: newValue) }
        }
    }

    // MARK: - Derived rows

    @ViewBuilder
    private var derivedRowsSection: some View {
        let rows = derivedRows(for: node, graph: store.graph)
        if !rows.isEmpty {
            VStack(alignment: .leading, spacing: 4) {
                ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                    Text("\(row.label) \(node.data.kind == .join ? "←" : "→") \(row.value)")
                        .font(.dataMono(10))
                        .foregroundStyle(Color.rupuDim)
                }
            }
        }
    }

    // MARK: - Remove step

    private var removeButton: some View {
        Button {
            store.deleteSelection()
        } label: {
            Text("Remove step")
                .frame(maxWidth: .infinity)
        }
        .buttonStyle(RupuButtonStyle.dangerOutline)
        .padding(.top, 6)
    }
}

// MARK: - Shared local field/parsing helpers

private func joinWaitKind(_ wait: JoinWait?) -> String {
    switch wait {
    case .all, .none: "all"
    case .any: "any"
    case .count: "count"
    }
}

private func joinWaitCount(_ wait: JoinWait?) -> Int? {
    if case .count(let n) = wait { return n }
    return nil
}

private func joinWait(forKind kind: String, count: Int?) -> JoinWait {
    switch kind {
    case "any": .any
    case "count": .count(count ?? 0)
    default: .all
    }
}

/// The action TOOL picker's option list — same "keep the unknown current
/// value visible" contract as `agentPickerOptions`.
func toolPickerOptions(current: String?, tools: [ToolSpec]) -> [String] {
    var names = tools.map(\.name)
    if let current, !current.isEmpty, !names.contains(current) {
        names.append(current)
    }
    return names
}

/// Generates a fresh row id not already present in `existing` — `"<prefix>-
/// 1"`, `"<prefix>-2"`, ... — for a freshly-added `parallel:` sub-step or
/// (unused today, kept for symmetry) any other add-a-row control.
private func freshRowID(existing: [String], prefix: String) -> String {
    var n = 1
    var candidate = "\(prefix)-\(n)"
    while existing.contains(candidate) {
        n += 1
        candidate = "\(prefix)-\(n)"
    }
    return candidate
}

// MARK: - SubStepRow

/// One `parallel:` sub-step's row — id, agent picker, prompt. Local `@State`
/// so id/prompt text edits aren't interrupted by the store's own re-render;
/// each field commits independently through its own closure.
private struct SubStepRow: View {
    let subStep: SubStep
    let agents: [AgentDefinition]
    let onChangeID: (String) -> Void
    let onChangeAgent: (String) -> Void
    let onChangePrompt: (String) -> Void
    let onRemove: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                DebouncedField(label: "ID", initial: subStep.id, font: .dataMono(11), onCommit: onChangeID)
                Button {
                    onRemove()
                } label: {
                    Text("×")
                        .font(.uiText)
                        .foregroundStyle(Color.rupuDim)
                }
                .buttonStyle(.plain)
            }
            Picker(
                "",
                selection: Binding(get: { subStep.agent }, set: { newValue in onChangeAgent(newValue) })
            ) {
                ForEach(agentPickerOptions(current: subStep.agent, agents: agents), id: \.self) { name in
                    Text(name).tag(name)
                }
            }
            .labelsHidden()
            .pickerStyle(.menu)
            DebouncedField(label: "PROMPT", initial: subStep.prompt, font: .uiText, onCommit: onChangePrompt)
        }
        .padding(8)
        .panelStyle(.innerCard)
    }
}

// MARK: - WithEditor

/// The action `with:` raw-YAML sub-editor — parses on the same debounce/blur
/// contract as every other field, but a parse FAILURE never commits: the
/// text stays in the box, an inline error shows, and `data.with` keeps its
/// last successfully-parsed value.
private struct WithEditor: View {
    let initial: YAMLValue?
    let onCommit: (YAMLValue?) -> Void

    @State private var text: String
    @State private var error: String?
    @FocusState private var focused: Bool
    @State private var debounceTask: Task<Void, Never>?

    init(initial: YAMLValue?, onCommit: @escaping (YAMLValue?) -> Void) {
        self.initial = initial
        self.onCommit = onCommit
        _text = State(initialValue: initial.map { YAMLEmitter.dump($0) } ?? "")
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Eyebrow("WITH")
            TextEditor(text: $text)
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuInk)
                .scrollContentBackground(.hidden)
                .frame(minHeight: 60)
                .modifier(FieldChrome(focused: focused))
                .focused($focused)
                .onChange(of: text) { _, _ in scheduleParse() }
                .onChange(of: focused) { _, isFocused in
                    if !isFocused { parseNow() }
                }
            if let error {
                Text(error)
                    .font(.noteText)
                    .foregroundStyle(Color.rupuErr)
            }
        }
    }

    private func scheduleParse() {
        debounceTask?.cancel()
        debounceTask = Task {
            try? await Task.sleep(for: .milliseconds(300))
            guard !Task.isCancelled else { return }
            parseNow()
        }
    }

    private func parseNow() {
        debounceTask?.cancel()
        debounceTask = nil
        switch parseWithText(text) {
        case .success(let value):
            error = nil
            onCommit(value)
        case .failure(let message):
            error = message
        }
    }
}

// MARK: - Shared field chrome (also used by SettingsTab.swift)

/// The single-line/multi-line field frame the whole form uses: `rupuBg`
/// fill, 1px `rupuBorder`, radius 5, a 1.5pt `rupuBrand` ring while focused
/// — spec §7's exact contract, factored once so every field below (and
/// `SettingsTab`'s NAME/DESCRIPTION) shares the identical chrome.
struct FieldChrome: ViewModifier {
    let focused: Bool

    func body(content: Content) -> some View {
        content
            .padding(.horizontal, 8)
            .padding(.vertical, 6)
            .background(Color.rupuBg)
            .overlay(RoundedRectangle(cornerRadius: 5).stroke(focused ? Color.rupuBrand : Color.rupuBorder, lineWidth: focused ? 1.5 : 1))
    }
}

/// A single-line field that commits ONLY on submit (return) or focus loss —
/// never on a debounce timer. Used where a per-keystroke commit would be
/// wrong (STEP ID/NAME both change what a save's PUT target is, so a
/// half-typed id must never round-trip to the store).
struct CommitOnBlurField<Label: View>: View {
    @Binding var text: String
    var font: Font = .uiText
    let onCommit: (String) -> Void
    @ViewBuilder let label: () -> Label

    @FocusState private var focused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            label()
            TextField("", text: $text)
                .textFieldStyle(.plain)
                .font(font)
                .foregroundStyle(Color.rupuInk)
                .focused($focused)
                .modifier(FieldChrome(focused: focused))
                .onSubmit { onCommit(text) }
                .onChange(of: focused) { _, isFocused in
                    if !isFocused { onCommit(text) }
                }
        }
    }
}

/// A single-line field that commits on submit/focus-loss AND a 300ms
/// debounce after the text stops changing — the default contract for most
/// per-kind fields (FOR EACH, CONDITION, SUBJECT, numeric fields, ...).
/// Owns its own local `@State` text (seeded once from `initial` at init),
/// so a caller never needs to thread a binding down.
struct DebouncedField: View {
    let label: String
    var font: Font = .uiText
    let onCommit: (String) -> Void

    @State private var text: String
    @FocusState private var focused: Bool
    @State private var debounceTask: Task<Void, Never>?

    init(label: String, initial: String, font: Font = .uiText, onCommit: @escaping (String) -> Void) {
        self.label = label
        self.font = font
        self.onCommit = onCommit
        _text = State(initialValue: initial)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if !label.isEmpty { Eyebrow(label) }
            TextField("", text: $text)
                .textFieldStyle(.plain)
                .font(font)
                .foregroundStyle(Color.rupuInk)
                .focused($focused)
                .modifier(FieldChrome(focused: focused))
                .onSubmit { commitNow() }
                .onChange(of: text) { _, _ in scheduleDebounce() }
                .onChange(of: focused) { _, isFocused in
                    if !isFocused { commitNow() }
                }
        }
    }

    private func scheduleDebounce() {
        debounceTask?.cancel()
        debounceTask = Task {
            try? await Task.sleep(for: .milliseconds(300))
            guard !Task.isCancelled else { return }
            commitNow()
        }
    }

    private func commitNow() {
        debounceTask?.cancel()
        debounceTask = nil
        onCommit(text)
    }
}

/// The numeric counterpart of `DebouncedField` (review fix, Minor 2): same
/// debounce/blur commit contract, but empty text and unparseable non-empty
/// text are no longer conflated — `DebouncedField` used to hand a plain
/// `String` to a caller-side `Int(trimmed)` parse that silently coerced
/// BOTH "empty" and "not a number" to `nil`, so a typo (`"12x"`) meant to
/// set MAX PARALLEL/TIMEOUT vanished into "unlimited/absent" with no
/// feedback. This field keeps the WITH sub-editor's parse-or-hold contract
/// instead: empty commits `nil`; a valid integer commits it; anything else
/// shows an inline `.noteText` `rupuErr` error and holds — the field
/// keeps the invalid text on screen and does NOT call `onCommit` at all,
/// so the store's last-good value survives until the text is fixed.
struct DebouncedNumericField: View {
    let label: String
    let onCommit: (Int?) -> Void

    @State private var text: String
    @State private var error: String?
    @FocusState private var focused: Bool
    @State private var debounceTask: Task<Void, Never>?

    init(label: String, initial: String, onCommit: @escaping (Int?) -> Void) {
        self.label = label
        self.onCommit = onCommit
        _text = State(initialValue: initial)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Eyebrow(label)
            TextField("", text: $text)
                .textFieldStyle(.plain)
                .font(.dataMono(11))
                .foregroundStyle(Color.rupuInk)
                .focused($focused)
                .modifier(FieldChrome(focused: focused))
                .onSubmit { commitNow() }
                .onChange(of: text) { _, _ in scheduleDebounce() }
                .onChange(of: focused) { _, isFocused in
                    if !isFocused { commitNow() }
                }
            if let error {
                Text(error)
                    .font(.noteText)
                    .foregroundStyle(Color.rupuErr)
            }
        }
    }

    private func scheduleDebounce() {
        debounceTask?.cancel()
        debounceTask = Task {
            try? await Task.sleep(for: .milliseconds(300))
            guard !Task.isCancelled else { return }
            commitNow()
        }
    }

    private func commitNow() {
        debounceTask?.cancel()
        debounceTask = nil
        let trimmed = text.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else {
            error = nil
            onCommit(nil)
            return
        }
        guard let value = Int(trimmed) else {
            error = "Not a whole number."
            return
        }
        error = nil
        onCommit(value)
    }
}

/// The multi-line counterpart of `DebouncedField` — a `TextEditor` with the
/// same debounce/blur commit contract. Used for PROMPT/DESCRIPTION/APPROVAL
/// PROMPT (prose, `.uiText`); the action `with:` sub-editor has its own
/// `WithEditor` above since it also parses and can reject the text.
struct DebouncedTextEditor: View {
    let label: String
    var font: Font = .uiText
    var minHeight: CGFloat = 80
    let onCommit: (String) -> Void

    @State private var text: String
    @FocusState private var focused: Bool
    @State private var debounceTask: Task<Void, Never>?

    init(label: String, initial: String, font: Font = .uiText, minHeight: CGFloat = 80, onCommit: @escaping (String) -> Void) {
        self.label = label
        self.font = font
        self.minHeight = minHeight
        self.onCommit = onCommit
        _text = State(initialValue: initial)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Eyebrow(label)
            TextEditor(text: $text)
                .font(font)
                .foregroundStyle(Color.rupuInk)
                .scrollContentBackground(.hidden)
                .frame(minHeight: minHeight)
                .modifier(FieldChrome(focused: focused))
                .focused($focused)
                .onChange(of: text) { _, _ in scheduleDebounce() }
                .onChange(of: focused) { _, isFocused in
                    if !isFocused { commitNow() }
                }
        }
    }

    private func scheduleDebounce() {
        debounceTask?.cancel()
        debounceTask = Task {
            try? await Task.sleep(for: .milliseconds(300))
            guard !Task.isCancelled else { return }
            commitNow()
        }
    }

    private func commitNow() {
        debounceTask?.cancel()
        debounceTask = nil
        onCommit(text)
    }
}
