// Graph parsing — yamlToGraph and friends. Line-for-line port of
// `crates/rupu-cp/web/src/lib/workflowGraph.ts:236-421` (parseStepData /
// parsePanel / parseLoops / yamlToGraph and the modelled-key sets), plus the
// `run:` extension documented in `GraphModel.swift`'s header comment. Every
// TS `if (x !== undefined)` becomes `if let`; the TS `as*` narrowing helpers
// are `YAMLValue` accessors from Task 1 (`stringValue`/`intValue`/
// `boolValue`/`sequenceValue`/`mappingValue`/`subscript(key:)`) plus the
// small local helpers below for the array/record shapes those accessors
// don't directly cover.

// ── Narrowing helpers ───────────────────────────────────────────────────────
// Small helpers over `YAMLValue?` mirroring the TS `as*` guards.

/// `Array.isArray` — any sequence, elements unfiltered.
private func asArray(_ v: YAMLValue?) -> [YAMLValue]? {
    v?.sequenceValue
}

/// A sequence filtered down to its string elements — mirrors the TS
/// `asStringArray`: it does NOT fail on a mixed array, it filters. Returns
/// `nil` only when `v` is absent or not a sequence at all (so `split: []`
/// still yields `[]`, distinct from "key absent").
private func asStringArray(_ v: YAMLValue?) -> [String]? {
    guard let seq = v?.sequenceValue else { return nil }
    return seq.compactMap(\.stringValue)
}

/// `typeof v === 'object' && v !== null && !Array.isArray(v)` — returns the
/// mapping itself (so callers can both subscript it and iterate its entries)
/// or `nil` when `v` isn't a mapping.
private func asRecord(_ v: YAMLValue?) -> YAMLValue? {
    guard let v, v.mappingValue != nil else { return nil }
    return v
}

/// Sort-and-append a mapping's entries whose key isn't in `modelled` into a
/// `rest`-style tuple array, preserving source order.
private func restEntries(of mapping: YAMLValue, excluding modelled: Set<String>) -> [(key: String, value: YAMLValue)] {
    (mapping.mappingValue ?? []).filter { !modelled.contains($0.key) }
}

// ── Modelled-key sets ────────────────────────────────────────────────────────

/// Step-level keys this module models explicitly. Everything else (e.g.
/// `contract:`) is captured into `rawPassthrough` on load. The TS set plus
/// `"run"` (macOS decision 2).
let MODELLED_STEP_KEYS: Set<String> = [
    "id", "agent", "prompt", "when", "continue_on_error", "actions", "for_each", "max_parallel", "parallel",
    "panel", "branch", "approval", "action", "with", "next", "depends_on", "split", "join", "run",
]

/// Keys this module models on a `branch:` block. Everything else is captured
/// into `StepNodeData.branchRest`.
let BRANCH_KEYS: Set<String> = ["condition", "then", "else"]

/// Keys this module models on an `approval:` block. Everything else is
/// captured into `StepNodeData.approvalRest`.
let APPROVAL_KEYS: Set<String> = ["required", "prompt", "timeout_seconds", "auto_approve", "on_timeout", "notify", "on_reject"]

/// Keys this module models on a `panel:` block. Everything else is captured
/// into `PanelCfg.rest`.
let PANEL_KEYS: Set<String> = ["panelists", "subject", "prompt", "max_parallel", "gate"]

// ── Parsing a single step ────────────────────────────────────────────────────

private func parseSubStep(_ raw: YAMLValue, _ j: Int) -> SubStep {
    SubStep(
        id: raw["id"]?.stringValue ?? "sub-\(j)",
        agent: raw["agent"]?.stringValue ?? "",
        prompt: raw["prompt"]?.stringValue ?? ""
    )
}

func parsePanel(_ o: YAMLValue) -> PanelCfg {
    var cfg = PanelCfg(panelists: asStringArray(o["panelists"]) ?? [], subject: o["subject"]?.stringValue ?? "")
    cfg.prompt = o["prompt"]?.stringValue
    cfg.maxParallel = o["max_parallel"]?.intValue
    if let gateRaw = asRecord(o["gate"]) {
        cfg.gate = PanelGate(
            untilNoFindingsAtSeverityOrAbove: gateRaw["until_no_findings_at_severity_or_above"]?.stringValue,
            fixWith: gateRaw["fix_with"]?.stringValue,
            maxIterations: gateRaw["max_iterations"]?.intValue
        )
    }
    cfg.rest = restEntries(of: o, excluding: PANEL_KEYS)
    return cfg
}

/// Parse a `join.wait` value into the `StepNodeData.joinWait` shape. Mirrors
/// workflow.rs `JoinWait`'s `#[serde(untagged)]`: try the bare keyword
/// (`"all"`/`"any"`) first, then the `{ count }` map form. Returns `nil` for
/// anything else (including absent).
private func parseJoinWait(_ v: YAMLValue?) -> JoinWait? {
    switch v?.stringValue {
    case "all": return .all
    case "any": return .any
    default: break
    }
    if let rec = asRecord(v), let count = rec["count"]?.intValue {
        return .count(count)
    }
    return nil
}

/// Kind precedence (most-specific first): panel > parallel > branch > split >
/// join > action > run > for_each > approval_gate > step. A step matching
/// none cleanly still becomes a plain `step` node carrying whatever it has.
/// `run` slots after `action` and before `for_each` (macOS decision 2 — see
/// `GraphModel.swift`'s header comment: `run:` is exclusive with agent/
/// for_each per workflow.rs validation, so this ordering can never
/// misclassify an existing web-editable workflow, because `run` was
/// previously unreachable as a kind at all).
func parseStepData(_ raw: YAMLValue, index: Int) -> StepNodeData {
    let id = raw["id"]?.stringValue ?? "step-\(index)"

    let panelRaw = asRecord(raw["panel"])
    let parallelRaw = asArray(raw["parallel"])
    let branchRaw = asRecord(raw["branch"])
    let splitRaw = asStringArray(raw["split"])
    let joinRaw = asRecord(raw["join"])
    let actionName = raw["action"]?.stringValue
    let forEach = raw["for_each"]?.stringValue
    let approvalRaw = asRecord(raw["approval"])
    let runRaw = asRecord(raw["run"])
    let agentName = raw["agent"]?.stringValue
    let promptText = raw["prompt"]?.stringValue

    var kind: StepKind = .step
    if panelRaw != nil {
        kind = .panel
    } else if parallelRaw != nil {
        kind = .parallel
    } else if branchRaw != nil {
        kind = .branch
    } else if splitRaw != nil {
        kind = .split
    } else if joinRaw != nil {
        kind = .join
    } else if actionName != nil {
        kind = .action
    } else if runRaw != nil {
        kind = .run
    } else if forEach != nil {
        kind = .forEach
    } else if approvalRaw != nil, agentName == nil, promptText == nil {
        kind = .approvalGate
    }

    var data = StepNodeData(id: id, kind: kind)

    if let splitRaw { data.split = splitRaw }
    if let joinRaw {
        data.hasJoin = true
        if let wait = parseJoinWait(joinRaw["wait"]) { data.joinWait = wait }
    }
    if let nextArr = asStringArray(raw["next"]), !nextArr.isEmpty { data.next = nextArr }
    if let dependsOnArr = asStringArray(raw["depends_on"]), !dependsOnArr.isEmpty { data.dependsOn = dependsOnArr }

    data.agent = agentName
    data.prompt = promptText
    data.action = actionName
    if let withRaw = asRecord(raw["with"]) { data.with = withRaw }
    data.when = raw["when"]?.stringValue
    data.continueOnError = raw["continue_on_error"]?.boolValue
    if let actions = asStringArray(raw["actions"]), !actions.isEmpty { data.actions = actions }
    data.forEach = forEach
    data.maxParallel = raw["max_parallel"]?.intValue
    if let parallelRaw {
        data.parallel = parallelRaw.enumerated().map { j, s in parseSubStep(s, j) }
    }
    if let panelRaw { data.panel = parsePanel(panelRaw) }
    if let runRaw { data.runBlock = runRaw }
    if let branchRaw {
        data.condition = branchRaw["condition"]?.stringValue
        if let thenTargets = asStringArray(branchRaw["then"]), !thenTargets.isEmpty { data.thenTargets = thenTargets }
        if let elseTargets = asStringArray(branchRaw["else"]), !elseTargets.isEmpty { data.elseTargets = elseTargets }
        let branchRest = restEntries(of: branchRaw, excluding: BRANCH_KEYS)
        if !branchRest.isEmpty { data.branchRest = branchRest }
    }

    if let approvalRaw {
        if approvalRaw["required"]?.boolValue == true { data.approvalRequired = true }
        data.approvalPrompt = approvalRaw["prompt"]?.stringValue
        data.approvalTimeoutSeconds = approvalRaw["timeout_seconds"]?.intValue
        data.approvalAutoApprove = approvalRaw["auto_approve"]?.stringValue
        if let ot = approvalRaw["on_timeout"]?.stringValue, ot == "approve" || ot == "reject" || ot == "fail" {
            data.approvalOnTimeout = ot
        }
        if let notify = asArray(approvalRaw["notify"]) {
            data.approvalNotify = notify.map { n in n.mappingValue != nil ? n : .mapping([]) }
        }
        if let onReject = asArray(approvalRaw["on_reject"]) {
            data.approvalOnReject = onReject.map { n in n.mappingValue != nil ? n : .mapping([]) }
        }
        let approvalRest = restEntries(of: approvalRaw, excluding: APPROVAL_KEYS)
        if !approvalRest.isEmpty { data.approvalRest = approvalRest }
    }

    // Capture any step-level keys we don't model so they survive round-trips.
    let passthrough = restEntries(of: raw, excluding: MODELLED_STEP_KEYS)
    if !passthrough.isEmpty { data.rawPassthrough = passthrough }

    return data
}

// ── loops: parse ─────────────────────────────────────────────────────────────

/// Parse the top-level `loops:` map into a name-sorted `[WorkflowLoop]`
/// (mirrors the Rust side's `BTreeMap<String, LoopDef>` iteration order). A
/// malformed entry (not a mapping at all) is skipped rather than dropping the
/// whole workflow — same defensive posture as every other narrowing helper in
/// this file.
///
/// Only `on_max` has a real default on the Rust side, so `onMax` defaults to
/// `"fail"` here to match; `nodes`/`until` absent parse to `[]`/`""`.
/// `max_iterations` parses to `nil` when absent/non-numeric rather than a
/// synthesized placeholder — see `WorkflowLoop.maxIterations`'s doc comment
/// in `GraphModel.swift` for why (this is the one deliberate deviation from
/// the TS port's `NaN` sentinel, since Swift's `Int` has none).
func parseLoops(_ raw: YAMLValue?) -> [WorkflowLoop] {
    guard let entries = raw?.mappingValue else { return [] }
    var out: [WorkflowLoop] = []
    for (name, defRaw) in entries {
        guard defRaw.mappingValue != nil else { continue }
        let nodes = asStringArray(defRaw["nodes"]) ?? []
        let until = defRaw["until"]?.stringValue ?? ""
        let maxIterations = defRaw["max_iterations"]?.intValue
        let onMax = defRaw["on_max"]?.stringValue == "proceed" ? "proceed" : "fail"
        out.append(WorkflowLoop(name: name, nodes: nodes, until: until, maxIterations: maxIterations, onMax: onMax))
    }
    out.sort { $0.name < $1.name }
    return out
}

// ── yamlToGraph ───────────────────────────────────────────────────────────────

/// Convert a parsed workflow document into the graph model. Non-throwing:
/// non-mapping input degrades to an empty graph with an empty meta name,
/// mirroring the TS `seedGraph` degrade path.
public func yamlToGraph(_ obj: YAMLValue) -> WorkflowGraph {
    guard obj.mappingValue != nil else {
        return WorkflowGraph(nodes: [], edges: [], meta: WorkflowMeta(name: ""), loops: [])
    }

    // meta: name + description are surfaced; everything else top-level
    // survives in `rest` so a round-trip leaves trigger/inputs/defaults/
    // autoflow/etc untouched.
    let rest = restEntries(of: obj, excluding: ["name", "description", "steps", "loops"])
    var meta = WorkflowMeta(name: obj["name"]?.stringValue ?? "", rest: rest)
    meta.description = obj["description"]?.stringValue

    let stepsRaw = obj["steps"]?.sequenceValue ?? []
    let nodes: [GraphNode] = stepsRaw.enumerated().map { i, s in
        let data = parseStepData(s, index: i)
        return GraphNode(id: data.id, data: data, position: .zero)
    }

    let loops = parseLoops(obj["loops"])

    return WorkflowGraph(nodes: nodes, edges: deriveEdges(nodes), meta: meta, loops: loops)
}
