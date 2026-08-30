# rupu.app Workflow Builder Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A visual, drag-and-drop Workflow Builder in rupu.app (`apps/rupu-macos`), replacing the Workflow detail screen's body — the native port of the CP web editor at `crates/rupu-cp/web/src/components/workflow-editor/`.

**Architecture:** Two new RupuKit modules following the `RupuUsageKit`/`RupuUsage` precedent: **`RupuFlowKit`** (pure — a YAML subset parser/emitter, the `workflowGraph.ts` port, `nodeShapes.ts` geometry, kind visuals, auto-layout; no SwiftUI) and **`RupuBuilder`** (the screen: canvas, palette, step form, run overlay, `BuilderStore`). The draft graph round-trips graph → workflow object → canonical YAML on every mutation, exactly like `WorkflowEditor.tsx`; the YAML pane is read-only. Persistence is **explicit Save (⌘S) via `PUT /api/workflows/:name`; Launch saves first** (decided with matt 2026-08-28). Run overlay states come from `RunDetailStore` — no new endpoints.

**Tech Stack:** Swift 6 / SwiftUI, Swift Testing (no XCTest), no third-party Swift dependencies. Behavioral contract: `crates/rupu-cp/web/src/lib/workflowGraph.ts`, `.../workflow-editor/{kindVisuals,nodeShapes}.ts`, `NodePalette.tsx`, `WorkflowEditor.tsx`, `StepForm.tsx`; schema truth: `crates/rupu-orchestrator/src/workflow.rs`. Approved design: `docs/workflow_design/` (mockup + screenshots + `IMPLEMENTATION_PROMPT.md`).

**Spec:** `docs/workflow_design/IMPLEMENTATION_PROMPT.md` (the user brief of 2026-08-28 is the same text; the mockup `docs/workflow_design/Workflow Builder.dc.html` and `docs/workflow_design/screenshots/*.png` are the visual contract).

## Global Constraints

- **No third-party Swift dependencies.** Swift Testing (not XCTest) for all tests (CLAUDE.md rupu.app rule 4).
- **XcodeGen owns the project file** — never edit `.xcodeproj`; `make macos-gen` after `project.yml` changes (rule 1). New library code goes in the RupuKit Swift package, so `project.yml` is untouched by this plan.
- **RupuDesign tokens only** — no ad-hoc colors/fonts. Everything from `RupuKit/Sources/RupuDesign/Tokens.swift` + `Typography.swift` (`rupuBg/rupuPanel/rupuSurface/rupuBorder/rupuBorderStrong/rupuInk/rupuDim/rupuMute`, `Color.status(_:)`, `Color.severity(_:)`, `Color.rupuBrand/rupuBrand600/rupuBrand700`, `ChromeShape.pill`, `Eyebrow`, `panelStyle(.innerCard)`, `Font.dataMono(_:)`).
- **Mutations are pending-state, not optimistic** (rule 9) — Save/Launch use `PendingActions` keys, never assume success from a 200 alone (Save is the exception documented in Task 12: a `PUT` workflow write IS synchronous on the server — 200 means written — so `confirm` on response is honest there, matching ConfigStore's precedent).
- **GUI validation rule** (rule 7): `make macos-test` + `make macos-build` green ≠ rendering green; matt runs the app before merge.
- **Verification:** every task ends with `cd apps/rupu-macos/RupuKit && swift test --filter <module>Tests` (or `make macos-test` where stated) actually run and green before commit.
- **Branch/PR:** all work on a feature branch (e.g. `feat/macos-workflow-builder`), merged via PR — never direct to main.
- **Numeric constants from the spec are law:** node box 176×68; shape constants I=2, R=12, SHEAR=20, TAPER=26, POINT=22, Q=22, RAIL=11, LAYER=9, FAN=30, edge-clamp fraction 0.3, layer-clamp fraction 0.5; header row 46pt; YAML pane 192pt; rail 320pt; dot grid 22pt pitch, 1px dots; node stroke 1.4pt (2pt selected); edge stroke 1.5pt; palette preview 34×20.

## Key decisions (locked before writing tasks)

1. **YAML engine is native.** `GET /api/workflows/:name` returns raw `yaml`; the builder parses it with a new pure Swift YAML **subset** parser and emits canonical YAML with a matching emitter (`RupuFlowKit/YAML*`). Rationale: the server's parsed `workflow` JSON serializes defaults (`trigger:`, `defaults:`, `notifyIssue: false`, nulls) that would pollute the emitted YAML; parsing the raw source is the only way to keep `meta.rest` source-faithful, which is the web contract (`yamlToGraph` keeps top-level rest verbatim). Unsupported YAML (anchors/aliases/tags) fails the load with a visible error block — never a silent degrade.
2. **`run:` steps are modeled first-class.** The web's `workflowGraph.ts` does NOT detect `run:` (falls into `raw_passthrough` as kind `step`), but `kindVisuals.ts` styles the kind and the approved mockup renders RUN nodes with a mono `$ cmd` sub-line. The native port adds `run` to the kind precedence (after `action`) and a COMMAND form field editing `run.cmd` (everything else under `run:` round-trips verbatim). Documented extension, not drift.
3. **Loops round-trip; loop editing is out of scope.** `loops:` parses into `WorkflowLoop` values, is re-emitted on save (name-sorted, field order `nodes/until/max_iterations/on_max`, `on_max` always emitted), and the `outerCycleEdges` exemption is ported so refine-style loop workflows stay serializable. No loop grouping UI, no `validateLoops` matrix this plan (web parity for editing loops is a follow-up).
4. **Save model:** header Save button + ⌘S → `PUT /api/workflows/:name?scope_kind=&scope_id=` with `{raw: <canonical yaml>}`; dirty dot on the button; Launch auto-saves first, then opens the prefilled Launcher sheet (`model.presentLauncher`). The sheet keeps its existing navigate-to-run behavior; the builder's Run mode independently follows the workflow's most recent run — a stated deviation from the mockup's "Launch flips to Run in place," accepted because the LauncherSheet owns post-launch navigation (`LauncherSheet.swift:314`).
5. **The old `WorkflowDetailScreen` is deleted**, its route (`RootView.swift:235`) now renders `WorkflowBuilderScreen`. The autoflow enable/disable toggle and scope badge move into the builder's Settings tab (same `LibraryStore` mutation).
6. **`nodeStyle` build flag:** the "cards" variant (rounded rect + 3.5pt accent top bar) ships as a `BuilderNodeStyle` enum defaulting to `.silhouette`, switchable in one place (`NodeView`), not a user-facing setting.

## File Structure

```
apps/rupu-macos/RupuKit/
  Package.swift                                  (modify: add RupuFlowKit, RupuBuilder + test targets; RupuShell gains RupuBuilder dep)
  Sources/RupuFlowKit/
    YAMLValue.swift            — ordered-map YAML value tree + equality + subscripts
    YAMLParser.swift           — block-YAML subset parser (returns YAMLValue or throws YAMLError)
    YAMLEmitter.swift          — canonical dump (2-space indent, literal blocks, minimal quoting)
    GraphModel.swift           — StepKind, StepNodeData, GraphNode, GraphEdge, WorkflowMeta, WorkflowLoop, WorkflowGraph
    GraphParse.swift           — yamlToGraph / parseStepData / parsePanel / parseLoops
    GraphEdges.swift           — extractStepRefs, hasExplicitEdges, materializeLegacyChain, deriveEdges, withDerivedEdges, canConnect, outerCycleEdges, loopControlEdges, cyclicNodes
    GraphSerialize.swift       — topoSort, nodeToStepObject, graphToWorkflowObject
    GraphValidate.swift        — validateGraph (per-kind checks, refs, graph-mode checks)
    GraphMutations.swift       — applyAdd/applyConnect/applyDelete/applyRename (pure graph edits)
    KindVisuals.swift          — accent/icon/shape/family/tagline/catalog per kind
    NodeShapes.swift           — ShapeName, SafeRect, NodeShapeGeometry, shapeFor(_:w:h:) port
    AutoLayout.swift           — layered DAG auto-layout (longest-path columns)
  Sources/RupuBuilder/
    BuilderStore.swift         — @Observable: load/parse, commit round-trip, selection, dirty, save, validate, mode, run-follow
    WorkflowBuilderScreen.swift— screen shell: header row, canvas+YAML split, inspector rail
    BuilderHeader.swift        — breadcrumb, StatusPill, valid dot, Design/Run segmented, source toggle, Save, Launch
    CanvasView.swift           — dot grid, scroll/pan, node layer, edge layer, drag/connect gestures, keyboard
    NodeView.swift             — silhouette Shape + content (chip/icon/id/sub-line), selection, ports, run-overlay glyphs
    ShapePaths.swift           — SwiftUI Shape adapters over NodeShapeGeometry
    EdgeLayer.swift            — cubic beziers, arrowheads, then/else labels, run-overlay edge states
    PaletteTab.swift           — filter, WORK/ORCHESTRATION sections, kind cards, detail card, add + drag source
    StepFormTab.swift          — per-kind fields, rename, derived rows, remove
    SettingsTab.swift          — name/description, trigger card, inputs card, autoflow toggle, scope badge
    RunOverlayModel.swift      — RunDetailStore → [String: NodeState] + edge state mapping
  Sources/RupuAPI/CPClient.swift        (modify: add writeWorkflow PUT)
  Sources/RupuAPI/WriteModels.swift     (modify: add WorkflowWriteBody)
  Sources/RupuShell/RootView.swift      (modify: route .workflowDefinition → WorkflowBuilderScreen)
  Sources/RupuLibrary/WorkflowDetailScreen.swift  (delete)
  Sources/RupuDesign/Icons/…            (modify: 7 new lucide icons via scripts/extract-lucide.mjs)
  Tests/RupuFlowKitTests/…              — YAML round-trip, graph parse/serialize/edges/validate, shapes, layout
  Tests/RupuBuilderTests/…              — BuilderStore behavior, run-overlay mapping, mutation helpers
```

Task order: pure engine first (1–7, each independently testable), API + store (8–9), then UI (10–14). UI tasks keep logic in `BuilderStore`/`RupuFlowKit` so the view layer stays thin and the interesting behavior stays unit-tested.

---

### Task 1: RupuFlowKit module + YAMLValue

**Files:**
- Modify: `apps/rupu-macos/RupuKit/Package.swift`
- Create: `apps/rupu-macos/RupuKit/Sources/RupuFlowKit/YAMLValue.swift`
- Test: `apps/rupu-macos/RupuKit/Tests/RupuFlowKitTests/YAMLValueTests.swift`

**Interfaces:**
- Produces: `public enum YAMLValue: Equatable, Sendable` with cases `.null`, `.bool(Bool)`, `.int(Int)`, `.double(Double)`, `.string(String)`, `.sequence([YAMLValue])`, `.mapping([(key: String, value: YAMLValue)])` — mapping is an **ordered** key/value array (js-yaml preserves insertion order; Swift `Dictionary` does not, and `meta.rest`/passthrough re-emission depends on order). Helpers: `subscript(key: String) -> YAMLValue?`, `var stringValue: String?`, `var intValue: Int?` (also unwraps an exact-integral `.double`), `var boolValue: Bool?`, `var sequenceValue: [YAMLValue]?`, `var mappingValue: [(key: String, value: YAMLValue)]?`, `func mapping(settingKey:to:)` (replace-or-append preserving position), `static func == ` synthesized via manual conformance (tuple arrays don't synthesize).

- [ ] **Step 1: Add both new targets to Package.swift** — `.target(name: "RupuFlowKit")` (no deps), `.target(name: "RupuBuilder", dependencies: ["RupuAPI", "RupuStore", "RupuDesign", "RupuFlowKit"])`, `.testTarget(name: "RupuFlowKitTests", dependencies: ["RupuFlowKit"])`, `.testTarget(name: "RupuBuilderTests", dependencies: ["RupuBuilder", "RupuFlowKit", "RupuAPI", "RupuStore", "RupuDesign"])`, and add `"RupuBuilder"` to `RupuShell`'s dependency list and to the umbrella `RupuKit` product's target list (mirror how `RupuLibrary` appears in both). Create `Sources/RupuBuilder/Placeholder.swift` containing only `// populated in Task 10` + one public marker enum so the target compiles.
- [ ] **Step 2: Write failing tests** — `YAMLValueTests.swift` using Swift Testing (`import Testing`, `@Test func …` + `#expect`):

```swift
import Testing
@testable import RupuFlowKit

@Test func orderedMappingPreservesInsertionOrder() {
    let v = YAMLValue.mapping([("b", .int(1)), ("a", .int(2))])
    #expect(v.mappingValue?.map(\.key) == ["b", "a"])
}

@Test func subscriptFindsKey() {
    let v = YAMLValue.mapping([("name", .string("x"))])
    #expect(v["name"]?.stringValue == "x")
    #expect(v["missing"] == nil)
}

@Test func equalityIsOrderSensitiveForMappings() {
    #expect(YAMLValue.mapping([("a", .int(1)), ("b", .int(2))]) != YAMLValue.mapping([("b", .int(2)), ("a", .int(1))]))
}

@Test func intValueUnwrapsIntegralDouble() {
    #expect(YAMLValue.double(4).intValue == 4)
    #expect(YAMLValue.double(4.5).intValue == nil)
}

@Test func settingKeyReplacesInPlaceOrAppends() {
    let v = YAMLValue.mapping([("a", .int(1)), ("b", .int(2))])
    #expect(v.mapping(settingKey: "a", to: .int(9)).mappingValue?.map(\.key) == ["a", "b"])
    #expect(v.mapping(settingKey: "c", to: .int(3)).mappingValue?.map(\.key) == ["a", "b", "c"])
}
```

- [ ] **Step 3: Run to verify failure** — `cd apps/rupu-macos/RupuKit && swift test --filter RupuFlowKitTests` → compile error (type missing).
- [ ] **Step 4: Implement `YAMLValue.swift`** per the Produces block. Manual `==`: switch over case pairs; mappings compare zipped `(key, value)` pairs plus count.
- [ ] **Step 5: Run tests green, commit** — `git add -A && git commit -m "feat(macos): RupuFlowKit module + ordered YAMLValue tree"`.

---

### Task 2: YAML subset parser

**Files:**
- Create: `apps/rupu-macos/RupuKit/Sources/RupuFlowKit/YAMLParser.swift`
- Test: `apps/rupu-macos/RupuKit/Tests/RupuFlowKitTests/YAMLParserTests.swift`

**Interfaces:**
- Produces: `public enum YAMLError: Error, Equatable { case unsupported(String, line: Int), malformed(String, line: Int) }`; `public enum YAMLParser { public static func parse(_ text: String) throws -> YAMLValue }`. Empty/whitespace-only input parses to `.null`.

**Supported subset** (everything `Workflow::parse`-able workflows in this repo use — verified against `.rupu/workflows/*.yaml`): block mappings + block sequences at any nesting; inline (flow) sequences `[a, b]` and flow mappings `{a: 1}` (one level of nesting inside flow is enough: flow values may themselves be flow collections or scalars); plain scalars with type resolution (null/`~`/empty → `.null`; `true/false` → bool; ints incl. negative; floats; everything else string — **only** lowercase `true/false/null` keywords, matching YAML 1.2 core-lite, plus bare `~`); single- and double-quoted scalars (double-quoted handles `\n \t \" \\` escapes); literal `|`/`|-`/`|+` and folded `>`/`>-` block scalars with proper indentation stripping and chomping; full-line and trailing ` #` comments (a `#` inside a quoted scalar is content); `---` document start ignored. **Rejected with `.unsupported`:** anchors `&`, aliases `*`, tags `!`, multiple documents, `? ` complex keys, duplicate keys in one mapping.

- [ ] **Step 1: Write failing tests.** Table-driven where possible:

```swift
@Test func parsesScalars() throws {
    let y = """
    name: review
    count: 3
    ratio: 0.5
    on: true
    off: false
    nothing: null
    tilde: ~
    quoted: "a: b"
    single: 'it''s'
    """
    let v = try YAMLParser.parse(y)
    #expect(v["name"] == .string("review"))
    #expect(v["count"] == .int(3))
    #expect(v["ratio"] == .double(0.5))
    #expect(v["on"] == .bool(true))
    #expect(v["nothing"] == .null)
    #expect(v["tilde"] == .null)
    #expect(v["quoted"] == .string("a: b"))
    #expect(v["single"] == .string("it's"))
}

@Test func parsesNestedBlockStructures() throws {
    let y = """
    steps:
      - id: triage
        agent: triager
        prompt: |
          Review the diff.
          Be thorough.
      - id: route
        branch:
          condition: "{{ steps.triage.output }}"
          then: [ship]
          else: [fix]
    """
    let v = try YAMLParser.parse(y)
    let steps = v["steps"]?.sequenceValue
    #expect(steps?.count == 2)
    #expect(steps?[0]["prompt"] == .string("Review the diff.\nBe thorough.\n"))
    #expect(steps?[1]["branch"]?["then"] == .sequence([.string("ship")]))
}

@Test func literalChompingVariants() throws {
    #expect(try YAMLParser.parse("a: |-\n  x\n  y\n")["a"] == .string("x\ny"))
    #expect(try YAMLParser.parse("a: |\n  x\n")["a"] == .string("x\n"))
}

@Test func commentsAreSkipped() throws {
    let v = try YAMLParser.parse("# top\na: 1 # trailing\nb: \"#not\"\n")
    #expect(v["a"] == .int(1))
    #expect(v["b"] == .string("#not"))
}

@Test func rejectsAnchorsAndTags() {
    #expect(throws: YAMLError.self) { try YAMLParser.parse("a: &x 1\nb: *x\n") }
    #expect(throws: YAMLError.self) { try YAMLParser.parse("a: !!str 1\n") }
}

@Test func duplicateKeyIsMalformed() {
    #expect(throws: YAMLError.self) { try YAMLParser.parse("a: 1\na: 2\n") }
}

@Test func parsesEveryRepoWorkflowSample() throws {
    // Golden corpus committed in Task 2 Step 3 from .rupu/workflows/*.yaml.
    for (name, text) in YAMLGolden.samples {
        let v = try YAMLParser.parse(text)
        #expect(v["name"] != nil, "sample \(name) lost its name key")
        #expect(v["steps"]?.sequenceValue?.isEmpty == false, "sample \(name) lost steps")
    }
}
```

- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Copy the golden corpus** — create `Tests/RupuFlowKitTests/YAMLGolden.swift` embedding (as Swift multiline string constants, `enum YAMLGolden { static let samples: [(String, String)] }`) the current content of every file in the repo's `.rupu/workflows/*.yaml` (skip `.bak`/`.lock`). These are committed snapshots, not filesystem reads, so tests stay hermetic.
- [ ] **Step 4: Implement the parser.** Recommended structure: pre-pass into `[(indent: Int, content: String, line: Int)]` with comments stripped (respecting quotes) and blanks kept only inside block scalars; then a recursive-descent over the line array: `parseNode(lines, from:, indent:) -> (YAMLValue, nextIndex)` dispatching on whether the first line at this indent starts with `- ` (sequence) or contains a top-level `: ` outside quotes/flow (mapping); scalar parsing shared by both; flow collections parsed with a small in-line tokenizer. Keep it one file; every `throw` carries the 1-based source line.
- [ ] **Step 5: Run tests green, commit.**

---

### Task 3: Canonical YAML emitter

**Files:**
- Create: `apps/rupu-macos/RupuKit/Sources/RupuFlowKit/YAMLEmitter.swift`
- Test: `apps/rupu-macos/RupuKit/Tests/RupuFlowKitTests/YAMLEmitterTests.swift`

**Interfaces:**
- Produces: `public enum YAMLEmitter { public static func dump(_ value: YAMLValue) -> String }` — canonical block-style dump: 2-space indent; mapping keys in stored order; multiline strings as literal blocks (`|` when the string ends with exactly one `\n`, `|-` when it has no trailing newline, `|+`-free — normalize instead); strings quoted (double) only when needed (empty, leading/trailing space, starts with a YAML-significant char ``- ? : # & * ! | > ' " % @ ` [ ] { },``, contains `: ` or ` #`, or would parse as bool/int/float/null); sequences as `- ` items with nested block content indented under the dash js-yaml-style (`- id: x` inline first key); empty mapping → `{}`, empty sequence → `[]` inline.

- [ ] **Step 1: Write failing tests:**

```swift
@Test func dumpsCanonicalBlockStyle() {
    let v = YAMLValue.mapping([
        ("name", .string("nightly")),
        ("steps", .sequence([.mapping([
            ("id", .string("scan")),
            ("prompt", .string("line1\nline2\n")),
            ("when", .string("true")),          // must quote: parses as bool
        ])])),
    ])
    let out = YAMLEmitter.dump(v)
    #expect(out == """
    name: nightly
    steps:
      - id: scan
        prompt: |
          line1
          line2
        when: "true"

    """.dropLast().description + "\n")
}

@Test func quotesOnlyWhenNeeded() {
    #expect(YAMLEmitter.dump(.mapping([("a", .string("plain text"))])) == "a: plain text\n")
    #expect(YAMLEmitter.dump(.mapping([("a", .string("- not a list"))])) == "a: \"- not a list\"\n")
    #expect(YAMLEmitter.dump(.mapping([("a", .string(""))])) == "a: \"\"\n")
    #expect(YAMLEmitter.dump(.mapping([("a", .string("3"))])) == "a: \"3\"\n")
}

@Test func emptyCollectionsInline() {
    #expect(YAMLEmitter.dump(.mapping([("j", .mapping([]))])) == "j: {}\n")
    #expect(YAMLEmitter.dump(.mapping([("s", .sequence([]))])) == "s: []\n")
}

@Test func roundTripsGoldenCorpusStructurally() throws {
    for (name, text) in YAMLGolden.samples {
        let once = try YAMLParser.parse(text)
        let dumped = YAMLEmitter.dump(once)
        let twice = try YAMLParser.parse(dumped)
        #expect(once == twice, "structural round-trip failed for \(name)")
        // Canonical form is a fixed point: dump(parse(dump(x))) == dump(x).
        #expect(YAMLEmitter.dump(twice) == dumped, "canonical dump not stable for \(name)")
    }
}
```

- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement the emitter** per the Produces contract. The needs-quoting test doubles as truth: a plain emit must re-parse to `.string(same)` — implement `needsQuoting(_ s: String) -> Bool` and add an internal debug assertion (in tests only) that `YAMLParser.parse("k: \(scalar)\n")` round-trips every plain-emitted scalar in the golden corpus.
- [ ] **Step 4: Run tests green, commit.**

---

### Task 4: Graph model + yamlToGraph port

**Files:**
- Create: `apps/rupu-macos/RupuKit/Sources/RupuFlowKit/GraphModel.swift`, `GraphParse.swift`
- Test: `apps/rupu-macos/RupuKit/Tests/RupuFlowKitTests/GraphParseTests.swift`

**Interfaces:**
- Produces (`GraphModel.swift`, port of `workflowGraph.ts:16-155` with the decision-2 `run` extension):

```swift
public enum StepKind: String, CaseIterable, Sendable, Equatable {
    case step, forEach = "for_each", parallel, panel, branch,
         approvalGate = "approval_gate", action, run, split, join
}
public struct SubStep: Equatable, Sendable { public var id, agent, prompt: String }
public struct PanelGate: Equatable, Sendable {
    public var untilNoFindingsAtSeverityOrAbove: String?
    public var fixWith: String?
    public var maxIterations: Int?
}
public struct PanelCfg: Equatable, Sendable {
    public var panelists: [String]; public var subject: String
    public var prompt: String?; public var maxParallel: Int?
    public var gate: PanelGate?; public var rest: [(key: String, value: YAMLValue)] // manual ==
}
public enum JoinWait: Equatable, Sendable { case all, any, count(Int) }
public struct StepNodeData: Equatable, Sendable {
    public var id: String; public var kind: StepKind
    public var agent, prompt, when: String?
    public var continueOnError: Bool?
    public var actions: [String]?
    public var forEach: String?; public var maxParallel: Int?
    public var parallel: [SubStep]?
    public var panel: PanelCfg?
    public var condition: String?; public var thenTargets, elseTargets: [String]?
    public var approvalRequired: Bool?; public var approvalPrompt: String?
    public var approvalTimeoutSeconds: Int?; public var approvalAutoApprove: String?
    public var approvalOnTimeout: String?   // "approve" | "reject" | "fail"
    public var approvalNotify, approvalOnReject: [YAMLValue]?
    public var branchRest, approvalRest: [(key: String, value: YAMLValue)]? // manual ==
    public var action: String?; public var with: YAMLValue?
    public var runBlock: YAMLValue?          // the whole `run:` mapping, verbatim (decision 2)
    public var next, dependsOn, split: [String]?
    public var joinWait: JoinWait?
    public var hasJoin: Bool                 // `join:` present at all (even bare {})
    public var rawPassthrough: [(key: String, value: YAMLValue)]?
    public init(id: String, kind: StepKind)  // all optionals nil, hasJoin false
}
public struct GraphNode: Equatable, Sendable, Identifiable {
    public var id: String; public var data: StepNodeData
    public var position: CGPoint
}
public struct GraphEdge: Equatable, Sendable, Identifiable {
    public var id, source, target: String
    public var label: String?; public var branchArm: String? // "then"|"else"
}
public struct WorkflowMeta: Equatable, Sendable {
    public var name: String; public var description: String?
    public var rest: [(key: String, value: YAMLValue)]     // manual ==
}
public struct WorkflowLoop: Equatable, Sendable {
    public var name: String; public var nodes: [String]
    public var until: String; public var maxIterations: Int
    public var onMax: String // "fail" | "proceed"
}
public struct WorkflowGraph: Equatable, Sendable {
    public var nodes: [GraphNode]; public var edges: [GraphEdge]
    public var meta: WorkflowMeta; public var loops: [WorkflowLoop]
}
```

- Produces (`GraphParse.swift`): `public func yamlToGraph(_ obj: YAMLValue) -> WorkflowGraph` and internal `parseStepData(_ raw: YAMLValue, index: Int) -> StepNodeData`, `parsePanel`, `parseLoops`, plus the modelled-key sets `MODELLED_STEP_KEYS` (the TS set **plus `"run"`**), `BRANCH_KEYS`, `APPROVAL_KEYS`, `PANEL_KEYS` — port `workflowGraph.ts:236-421` line-for-line semantics. Kind precedence with the run extension: `panel > parallel > branch > split > join > action > run > for_each > approval_gate > step` (run slots after action, before for_each: `run:` is exclusive with agent/for_each per workflow.rs validation, and this order can never misclassify an existing web-editable workflow because `run:` was previously unreachable as a kind). `hasJoin` mirrors "joinRaw present"; `joinWait` parses `"all"`/`"any"`/`{count: n}` (`parseJoinWait`, TS `:243-253`). Positions seed at `.zero` (layout in Task 7).

- [ ] **Step 1: Write failing tests** — direct ports of the semantics, driven through YAML text for realism:

```swift
private func graph(_ yaml: String) throws -> WorkflowGraph {
    try yamlToGraph(YAMLParser.parse(yaml))
}

@Test func kindPrecedence() throws {
    let g = try graph("""
    name: t
    steps:
      - id: a
        agent: x
        prompt: p
      - id: b
        agent: x
        prompt: p
        for_each: "{{ inputs.files }}"
      - id: c
        parallel:
          - id: s1
            agent: x
            prompt: p
      - id: d
        panel:
          panelists: [r1]
          subject: s
      - id: e
        branch:
          condition: c
          then: [a]
      - id: f
        approval:
          prompt: ok?
      - id: g
        action: github.comment
        with: {body: hi}
      - id: h
        run:
          cmd: semgrep
          args: [scan]
      - id: i
        split: [a, b]
      - id: j
        join: {}
    """)
    #expect(g.nodes.map(\.data.kind) == [.step, .forEach, .parallel, .panel, .branch, .approvalGate, .action, .run, .split, .join])
    #expect(g.nodes[7].data.runBlock?["cmd"] == .string("semgrep"))
    #expect(g.nodes[9].data.hasJoin && g.nodes[9].data.joinWait == nil)
}

@Test func agentWithApprovalIsNotAGate() throws {
    let g = try graph("name: t\nsteps:\n  - id: a\n    agent: x\n    prompt: p\n    approval:\n      required: true\n")
    #expect(g.nodes[0].data.kind == .step)
    #expect(g.nodes[0].data.approvalRequired == true)
}

@Test func passthroughCapturesUnmodelledKeys() throws {
    let g = try graph("name: t\nsteps:\n  - id: a\n    agent: x\n    prompt: p\n    contract:\n      format: json\n")
    #expect(g.nodes[0].data.rawPassthrough?.first?.key == "contract")
}

@Test func metaRestKeepsTopLevelOrderVerbatim() throws {
    let g = try graph("name: t\ndescription: d\ntrigger:\n  kind: cron\n  cron: \"0 3 * * *\"\ninputs:\n  files:\n    type: list\nsteps:\n  - id: a\n    agent: x\n    prompt: p\n")
    #expect(g.meta.rest.map(\.key) == ["trigger", "inputs"])
}

@Test func loopsParseSorted() throws {
    let g = try graph("name: t\nloops:\n  zeta:\n    nodes: [a, b]\n    until: done\n    max_iterations: 3\n  alpha:\n    nodes: [c, d]\n    until: ok\n    max_iterations: 2\n    on_max: proceed\nsteps:\n  - id: a\n    agent: x\n    prompt: p\n")
    #expect(g.loops.map(\.name) == ["alpha", "zeta"])
    #expect(g.loops[0].onMax == "proceed")
    #expect(g.loops[1].onMax == "fail")
}
```

- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement `GraphModel.swift` + `GraphParse.swift`.** Port defensively-narrowing helpers as `YAMLValue` accessors (`stringValue` etc. from Task 1 stand in for `asString`/`asNumber`). Every TS `if (x !== undefined)` becomes `if let`. `edges` are filled by `deriveEdges` (Task 5) — until then `yamlToGraph` sets `edges: []` with a `// Task 5` note and the tests above don't assert edges.
- [ ] **Step 4: Run tests green, commit.**

---

### Task 5: Edges + topoSort + serialization

**Files:**
- Create: `apps/rupu-macos/RupuKit/Sources/RupuFlowKit/GraphEdges.swift`, `GraphSerialize.swift`
- Modify: `GraphParse.swift` (yamlToGraph now derives edges)
- Test: `Tests/RupuFlowKitTests/GraphEdgesTests.swift`, `GraphSerializeTests.swift`

**Interfaces:**
- Produces (`GraphEdges.swift`, ports of `workflowGraph.ts:761-905, 1206-1256` + loop helpers `:566-668`):
  - `public func extractStepRefs(_ d: StepNodeData) -> [String]` — regex `steps\.([A-Za-z0-9_-]+)` over prompt/forEach/when/condition/sub-step prompts/panel subject+prompt.
  - `public func hasExplicitEdges(_ nodes: [GraphNode]) -> Bool`
  - `public func materializeLegacyChain(_ nodes: [GraphNode]) -> [GraphNode]`
  - `public func deriveEdges(_ nodes: [GraphNode]) -> [GraphEdge]` — graph mode: next/split/depends_on + data-refs + branch arms (labels `"true"`/`"false"`, `branchArm` `"then"`/`"else"`); legacy mode: consecutive chain + data-refs + branch arms. Dedupe key `"\(source)->\(target)::\(label ?? "")"`; self-edges dropped.
  - `public func withDerivedEdges(meta: WorkflowMeta, nodes: [GraphNode], loops: [WorkflowLoop]) -> WorkflowGraph`
  - `public func canConnect(source: String, target: String, edges: [GraphEdge], arm: String?) -> ConnectVerdict` where `public enum ConnectVerdict: Equatable { case ok; case selfLoop(String); case duplicate(String); case cycle(String) }` (reason strings verbatim from TS: "A step can't depend on itself.", "These steps are already connected.", "This would create a cycle — steps must form a DAG.")
  - Internal: `cyclicNodes`, `loopControlEdges`, `outerCycleEdges` (ports of `:566-585, :601-668`).
- Produces (`GraphSerialize.swift`, ports of `:958-1010, 1080-1199` + `loopsToObject :481-497`):
  - `public func topoSort(nodes: [GraphNode], edges: [GraphEdge]) -> TopoResult` with `public enum TopoResult: Equatable { case order([GraphNode]); case cycle([String]) }` — Kahn with the y/x/id tiebreak.
  - `internal func nodeToStepObject(_ d: StepNodeData) -> YAMLValue` — per-kind emit arms exactly as TS, with the run extension: the `.run` arm emits `o["run"] = runBlock` plus shared when/continueOnError/actions; gate arm always emits `approval:`; `next`/`depends_on` omitted when empty; approval block assembled per `:1122-1152`; passthrough spread last never clobbering (`:1155-1160`).
  - `public func graphToWorkflowObject(_ g: WorkflowGraph) -> SerializeResult` with `public enum SerializeResult { case object(YAMLValue); case failure(String) }` — topo over `outerCycleEdges(nodes, deriveEdges(nodes), loops)`; key order `name`, `description?`, meta.rest verbatim, `loops` (only when non-empty, name-sorted, `on_max` always emitted), `steps` last.

- [ ] **Step 1: Write failing edge tests** (`GraphEdgesTests.swift`):

```swift
@Test func legacyModeChainsConsecutivePairs() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p}\n  - {id: b, agent: x, prompt: p}\n  - {id: c, agent: x, prompt: p}\n")
    #expect(g.edges.map { "\($0.source)->\($0.target)" } == ["a->b", "b->c"])
}

@Test func graphModeIgnoresListOrder() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p, next: [c]}\n  - {id: b, agent: x, prompt: p}\n  - {id: c, agent: x, prompt: p}\n")
    #expect(g.edges.map { "\($0.source)->\($0.target)" } == ["a->c"])
}

@Test func dataRefEdgesInferredInBothModes() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p}\n  - {id: b, agent: x, prompt: \"use {{ steps.a.output }}\"}\n")
    #expect(g.edges.count == 1) // chain edge a->b dedupes with the data-ref
}

@Test func branchArmsAreLabelled() throws {
    let g = try graph("name: t\nsteps:\n  - id: r\n    branch: {condition: c, then: [a], else: [b]}\n  - {id: a, agent: x, prompt: p}\n  - {id: b, agent: x, prompt: p}\n")
    let arms = g.edges.filter { $0.branchArm != nil }
    #expect(arms.map(\.label) == ["true", "false"])
}

@Test func materializeLegacyChainWritesExplicitNext() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p}\n  - {id: b, agent: x, prompt: p}\n")
    let m = materializeLegacyChain(g.nodes)
    #expect(m[0].data.next == ["b"])
    #expect(m[1].data.next == [])
}

@Test func canConnectRejectsCycleDuplicateSelf() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p, next: [b]}\n  - {id: b, agent: x, prompt: p}\n")
    #expect(canConnect(source: "a", target: "a", edges: g.edges, arm: nil) == .selfLoop("A step can't depend on itself."))
    #expect(canConnect(source: "a", target: "b", edges: g.edges, arm: nil) == .duplicate("These steps are already connected."))
    #expect(canConnect(source: "b", target: "a", edges: g.edges, arm: nil) == .cycle("This would create a cycle — steps must form a DAG."))
    #expect(canConnect(source: "a", target: "b", edges: g.edges, arm: "then") == .ok) // arm-distinct duplicate check
}
```

- [ ] **Step 2: Write failing serialize tests** (`GraphSerializeTests.swift`):

```swift
@Test func fullRoundTripIsCanonicalFixedPoint() throws {
    for (name, text) in YAMLGolden.samples {
        let g1 = try graph(text)
        guard case .object(let obj) = graphToWorkflowObject(g1) else {
            Issue.record("serialize failed for \(name)"); continue
        }
        let dumped = YAMLEmitter.dump(obj)
        let g2 = try graph(dumped)
        // Same steps, same kinds, same edge set, same meta.
        #expect(g1.nodes.map(\.data) == g2.nodes.map(\.data), "step drift in \(name)")
        #expect(Set(g1.edges.map(\.id)) == Set(g2.edges.map(\.id)), "edge drift in \(name)")
        #expect(g1.meta == g2.meta, "meta drift in \(name)")
        // Fixed point: serializing again emits identical YAML.
        guard case .object(let obj2) = graphToWorkflowObject(g2) else { Issue.record("re-serialize failed for \(name)"); continue }
        #expect(YAMLEmitter.dump(obj2) == dumped, "canonical YAML not stable for \(name)")
    }
}

@Test func topoOrderTiebreaksOnPositionThenID() {
    var nodes = [GraphNode(id: "b", data: .init(id: "b", kind: .step), position: .init(x: 0, y: 100)),
                 GraphNode(id: "a", data: .init(id: "a", kind: .step), position: .init(x: 0, y: 0))]
    if case .order(let o) = topoSort(nodes: nodes, edges: []) {
        #expect(o.map(\.id) == ["a", "b"])
    } else { Issue.record("unexpected cycle") }
    nodes[0].position = .zero
    if case .order(let o) = topoSort(nodes: nodes, edges: []) {
        #expect(o.map(\.id) == ["a", "b"]) // id tiebreak
    } else { Issue.record("unexpected cycle") }
}

@Test func cycleBlocksSerialization() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p, next: [b]}\n  - {id: b, agent: x, prompt: p, next: [a]}\n")
    guard case .failure(let msg) = graphToWorkflowObject(g) else { return Issue.record("expected failure") }
    #expect(msg.hasPrefix("Cannot serialize: cycle through "))
}

@Test func gateAlwaysEmitsApprovalBlock() throws {
    let g = try graph("name: t\nsteps:\n  - id: gate\n    approval: {}\n")
    guard case .object(let obj) = graphToWorkflowObject(g) else { return Issue.record("serialize failed") }
    #expect(obj["steps"]?.sequenceValue?[0]["approval"] == .mapping([]))
}

@Test func loopFeedbackRefDoesNotBlockSerialize() throws {
    let y = """
    name: t
    loops:
      refine:
        nodes: [gen, critique]
        until: ok
        max_iterations: 3
    steps:
      - {id: gen, agent: x, prompt: "use {{ steps.critique.output }}", next: [critique]}
      - {id: critique, agent: x, prompt: p}
    """
    guard case .object = graphToWorkflowObject(try graph(y)) else { return Issue.record("refine loop must serialize") }
}
```

- [ ] **Step 3: Run to verify failures.**
- [ ] **Step 4: Implement** both files per the TS sources cited in Interfaces; flip `yamlToGraph` to call `deriveEdges`. Port comments where they carry semantics (e.g. why label is in the dedupe key).
- [ ] **Step 5: Run all RupuFlowKitTests green, commit.**

---

### Task 6: validateGraph port

**Files:**
- Create: `apps/rupu-macos/RupuKit/Sources/RupuFlowKit/GraphValidate.swift`
- Test: `Tests/RupuFlowKitTests/GraphValidateTests.swift`

**Interfaces:**
- Produces: `public func validateGraph(_ g: WorkflowGraph) -> [String: [String]]` — port of `workflowGraph.ts:1262-1428` **minus** `validateLoops` (decision 3) but **including** `outerCycleEdges` in the order/cycle computation. Message strings verbatim from TS. Two run-extension additions: a `.run` node with no `runBlock["cmd"]` string → `"run needs a cmd"`; an `.action` node with empty/missing `action` → `"action needs a tool"` (the web validates action tool presence in StepForm; here it belongs in validate so the valid-dot is honest).

- [ ] **Step 1: Write failing tests:**

```swift
@Test func stepNeedsAgentAndPrompt() throws {
    let g = try graph("name: t\nsteps:\n  - id: a\n")
    #expect(validateGraph(g)["a"]?.sorted() == ["needs a prompt", "needs an agent"])
}

@Test func cleanGraphIsEmpty() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p}\n")
    #expect(validateGraph(g).isEmpty)
}

@Test func forwardRefFlagged() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: \"{{ steps.b.output }}\"}\n  - {id: b, agent: x, prompt: p}\n")
    #expect(validateGraph(g)["a"] == ["references steps.b which runs later"])
}

@Test func unknownRefFlagged() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: \"{{ steps.ghost.output }}\"}\n")
    #expect(validateGraph(g)["a"] == ["references unknown step ghost"])
}

@Test func graphModeChecks() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p, next: [ghost, a]}\n  - id: s\n    split: [a]\n  - id: j\n    join: {}\n")
    let p = validateGraph(g)
    #expect(p["a"]?.contains("edge target `ghost` is not a known step") == true)
    #expect(p["a"]?.contains("an edge cannot target its own step") == true)
    #expect(p["s"]?.contains("a split should fan out to 2+ steps") == true)
    #expect(p["j"]?.contains("a join should have 2+ inbound paths") == true)
}

@Test func duplicateIDFlagged() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p}\n  - {id: a, agent: x, prompt: q}\n")
    #expect(validateGraph(g)["a"]?.contains("duplicate step id") == true)
}

@Test func panelBranchParallelChecks() throws {
    let g = try graph("name: t\nsteps:\n  - id: p\n    panel: {panelists: [], subject: \"\"}\n  - id: b\n    branch: {then: [ghost]}\n  - id: par\n    parallel: []\n")
    let out = validateGraph(g)
    #expect(out["p"]?.contains("panel needs at least one panelist") == true)
    #expect(out["b"]?.contains("branch needs a condition") == true)
    #expect(out["b"]?.contains("branch target ghost is not a known step") == true)
    #expect(out["par"]?.contains("needs at least one parallel sub-step") == true)
}

@Test func runAndActionExtensions() throws {
    let g = try graph("name: t\nsteps:\n  - id: r\n    run: {}\n  - id: c\n    action: \"\"\n")
    let out = validateGraph(g)
    #expect(out["r"] == ["run needs a cmd"])
    #expect(out["c"] == ["action needs a tool"])
}
```

- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement** — a direct arm-by-arm port; keep the notify/on_reject row checks (`:1301-1316`).
- [ ] **Step 4: Run tests green, commit.**

---

### Task 7: Kind visuals, node shapes, auto-layout, icons

**Files:**
- Create: `apps/rupu-macos/RupuKit/Sources/RupuFlowKit/KindVisuals.swift`, `NodeShapes.swift`, `AutoLayout.swift`
- Modify: `apps/rupu-macos/scripts/extract-lucide.mjs` (7 new icons), `apps/rupu-macos/RupuKit/Sources/RupuDesign/Icons/Icon.swift`
- Test: `Tests/RupuFlowKitTests/NodeShapesTests.swift`, `AutoLayoutTests.swift`, extend `Tests/RupuDesignTests` icon-data test list (it's `CaseIterable`-driven — verify it needs no edit, only regenerated data)

**Interfaces:**
- Produces (`KindVisuals.swift`) — pure descriptors, no SwiftUI types beyond nothing (colors are token *names* resolved by RupuBuilder so RupuFlowKit stays UI-free):

```swift
public enum KindAccent: Sendable { case statusRunning, brand500, sevCritical, statusAwaiting,
    statusDone, statusPaused, sevInfo, sevMedium, brand600, brand700 }
public enum KindFamily: String, Sendable { case work, orchestration }
public struct KindVisual: Sendable {
    public let accent: KindAccent
    public let iconName: String       // LucideIcon rawValue-compatible name
    public let shape: ShapeName
    public let family: KindFamily
    public let tagline: String
}
public func kindVisual(_ k: StepKind) -> KindVisual
```

Table verbatim from spec §3 + `kindVisuals.ts`: step/(statusRunning, "bot", .rect, work, "one agent"); for_each/(brand500, "repeat", .hexagon, work, "over a list"); parallel/(sevCritical, "columns3", .subroutine, work, "N at once"); panel/(statusAwaiting, "shieldCheck", .stacked, work, "review+gate"); action/(sevInfo, "zap", .parallelogram, work, "connector call"); run/(sevMedium, "terminal", .rect, work, "command"); branch/(statusDone, "gitBranch", .vhex, orchestration, "if / then / else"); split/(brand600, "split", .fanout, orchestration, "fan out"); join/(brand700, "merge", .fanin, orchestration, "barrier"); approval_gate/(statusPaused, "userCheck", .trapezoid, orchestration, "human approval").

Also `public struct BlockCatalogEntry { kind, label, what, requiredFields: [String], example: String }` + `public let blockCatalog: [BlockCatalogEntry]` — the 8 web entries ported verbatim from `NodePalette.tsx:74-140` **plus** `action` (`what: "Invokes one SCM/issue/CI connector tool with parameters — no agent runs on this node."`, required `["action"]`, example `"- id: comment\n  action: github.issue_comment\n  with:\n    body: \"Done.\""`) and `run` (`what: "Runs one deterministic command; stdout binds to steps.<id>.output."`, required `["cmd"]`, example `"- id: scan\n  run:\n    cmd: semgrep\n    args: [scan, --config, auto]"`).

- Produces (`NodeShapes.swift`) — port of `nodeShapes.ts` geometry, CoreGraphics only:

```swift
public enum ShapeName: String, Sendable { case rect, vhex, parallelogram, trapezoid,
    hexagon, subroutine, stacked, fanout, fanin }
public struct SafeRect: Equatable, Sendable { public var x, y, w, h: CGFloat }
public enum HandleSide: String, Sendable { case left, right, bottom }
public struct HandleAnchor: Equatable, Sendable { public var side: HandleSide; public var fraction: CGFloat; public var inset: CGFloat }
public struct SourceAnchor: Equatable, Sendable { public var arm: String?; public var anchor: HandleAnchor }
public struct NodeShapeGeometry: Sendable {
    public var points: [CGPoint]
    public var path: CGPath          // silhouette (rounded for .rect)
    public var extra: [CGPath]       // rails / stack layers, stroke-only
    public var safe: SafeRect
    public var centered: Bool        // align
    public var target: HandleAnchor
    public var sources: [SourceAnchor]
}
public func shapeFor(_ shape: ShapeName, w: CGFloat, h: CGFloat) -> NodeShapeGeometry
```

Constants and clamps exactly as the TS file: I=2, R=12, SHEAR=20, TAPER=26, POINT=22, Q=22, RAIL=11, LAYER=9, FAN=30, `EDGE_CLAMP_FRACTION`=0.3, `LAYER_CLAMP_FRACTION`=0.5; `clampInset(value, dim, fraction) = min(value, (dim - 2*I) * fraction)`; every safe-rect/inset formula per shape arm ported verbatim (the file is exhaustively commented — keep the semantic comments). `fraction` in `HandleAnchor` replaces the CSS `'50%'` offset (always 0.5 today).

- Produces (`AutoLayout.swift`): `public func autoLayout(nodes: [GraphNode], edges: [GraphEdge]) -> [GraphNode]` — longest-path layering: column(n) = longest edge-path length from any root; x = 60 + column * (NODE_W 176 + GAP_X 84); rows within a column ordered by mean predecessor y then id, y = 60 + row * (NODE_H 68 + GAP_Y 46). Nodes already positioned (non-zero) keep their position (used on YAML reload reconcile); only `.zero`-positioned nodes get slots. Cycle-safe: on `topoSort` cycle, fall back to array order columns.

- [ ] **Step 1: Add the 7 icons** to `extract-lucide.mjs` ICONS table: `{ enumCase: "bot", asset: "bot", source: "bot" }`, `{ enumCase: "columns3", asset: "columns-3", source: "columns-3" }`, `{ enumCase: "zap", asset: "zap", source: "zap" }`, `{ enumCase: "terminal", asset: "terminal", source: "terminal" }`, `{ enumCase: "split", asset: "split", source: "split" }`, `{ enumCase: "merge", asset: "merge", source: "merge" }`, `{ enumCase: "userCheck", asset: "user-check", source: "user-check" }`. Add matching cases to `LucideIcon` (`case bot`, `case columns3`, `case zap`, `case terminal`, `case split`, `case merge`, `case userCheck`). Run the script per its header instructions (check `head -50 scripts/extract-lucide.mjs` for the invocation; it needs the lucide npm package — if the vendored source isn't present, run `npm ci` in the CP web dir first per the script's comments) and regenerate `LucideIconData`. Run `swift test --filter RupuDesignTests` — the `CaseIterable` data test must pass for the new cases.
- [ ] **Step 2: Write failing shape/layout tests:**

```swift
@Test func allShapesAreClosedPolygonsInsideBox() {
    for name in ShapeName.allCases {
        let s = shapeFor(name, w: 176, h: 68)
        for p in s.points {
            #expect(p.x >= 0 && p.x <= 176 && p.y >= 0 && p.y <= 68, "\(name) point escapes box")
        }
        #expect(s.safe.x >= 0 && s.safe.x + s.safe.w <= 176 && s.safe.y >= 0 && s.safe.y + s.safe.h <= 68, "\(name) safe rect escapes box")
    }
}

@Test func hexagonInsetsClampAtPaletteSize() {
    // 34x20 palette chip: POINT (22) must clamp to (34-4)*0.3 = 9.
    let s = shapeFor(.hexagon, w: 34, h: 20)
    #expect(abs(s.points[0].x - 9) < 0.001)
}

@Test func vhexHasTwoLabelledSources() {
    let s = shapeFor(.vhex, w: 176, h: 68)
    #expect(s.sources.map(\.arm) == ["then", "else"])
    #expect(s.sources[1].anchor.side == .bottom)
    #expect(s.centered)
}

@Test func parallelogramHandleInsetIsSlantMidpoint() {
    let s = shapeFor(.parallelogram, w: 176, h: 68)
    #expect(abs(s.sources[0].anchor.inset - 11) < 0.001) // (SHEAR 20 + I 2)/2
}

@Test func stackedSourceInsetClearsLayers() {
    let s = shapeFor(.stacked, w: 176, h: 68)
    #expect(abs(s.sources[0].anchor.inset - 11) < 0.001) // LAYER 9 + I 2
}

@Test func subroutineHasTwoRails() {
    #expect(shapeFor(.subroutine, w: 176, h: 68).extra.count == 2)
}

@Test func autoLayoutColumnsFollowLongestPath() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p, next: [b, c]}\n  - {id: b, agent: x, prompt: p, next: [d]}\n  - {id: c, agent: x, prompt: p}\n  - {id: d, agent: x, prompt: p}\n")
    let laid = autoLayout(nodes: g.nodes, edges: g.edges)
    let x = Dictionary(uniqueKeysWithValues: laid.map { ($0.id, $0.position.x) })
    #expect(x["a"]! < x["b"]!)
    #expect(x["b"]! == x["c"]!)
    #expect(x["b"]! < x["d"]!)
}

@Test func autoLayoutKeepsExistingPositions() throws {
    var g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p}\n  - {id: b, agent: x, prompt: p}\n")
    g.nodes[0].position = CGPoint(x: 500, y: 500)
    let laid = autoLayout(nodes: g.nodes, edges: g.edges)
    #expect(laid[0].position == CGPoint(x: 500, y: 500))
}
```

- [ ] **Step 3: Run to verify failure; implement all three files; run green.**
- [ ] **Step 4: Commit** (icons regen + FlowKit visuals/shapes/layout together — one reviewable unit).

---

### Task 8: Graph mutations (add / connect / delete / rename / field edit)

**Files:**
- Create: `apps/rupu-macos/RupuKit/Sources/RupuFlowKit/GraphMutations.swift`
- Test: `Tests/RupuFlowKitTests/GraphMutationsTests.swift`

**Interfaces:**
- Produces — all pure, all returning a NEW `WorkflowGraph` built via `withDerivedEdges` (callers then run `graphToWorkflowObject` to commit; a serialize failure rejects the edit, Task 9):
  - `public func applyAdd(_ g: WorkflowGraph, kind: StepKind, at: CGPoint) -> (WorkflowGraph, newID: String)` — id = smallest-available `"\(kind.rawValue)-N"` (N from 1); seeds per-kind identity fields so the node round-trips immediately: `.approvalGate` → `approvalRequired = true` is NOT set (gate emits bare `approval: {}` — matches web `applyAddNode` semantics: identity is block presence), `.split` → `split = []`, `.join` → `hasJoin = true`, `.parallel` → `parallel = []`, `.panel` → `panel = PanelCfg(panelists: [], subject: "", …)`, `.branch` → no seeds, `.run` → `runBlock = .mapping([("cmd", .string(""))])`.
  - `public func applyConnect(_ g: WorkflowGraph, source: String, target: String, arm: String?) -> ConnectOutcome` with `public enum ConnectOutcome { case connected(WorkflowGraph); case rejected(String) }` — validates via `canConnect` (over the derived edges); on ok: `materializeLegacyChain` FIRST (the Critical-bug guard from TS `:807-830`), then writes `thenTargets`/`elseTargets` append for a branch arm, else appends to source's `next`.
  - `public func applyDelete(_ g: WorkflowGraph, id: String) -> WorkflowGraph` — drop the node; scrub `id` from every other node's `next`/`split`/`dependsOn`/`thenTargets`/`elseTargets`; `scrubStepFromLoops` port (drop loop members, drop loops shrunk below 2).
  - `public func applyRename(_ g: WorkflowGraph, from: String, to rawNew: String) -> RenameOutcome` with `public enum RenameOutcome { case renamed(WorkflowGraph, id: String); case rejected(String) }` — slugify (`lowercased`, spaces/invalid → `-`, collapse repeats, trim `-`, allowed `[a-z0-9_-]`); reject empty or duplicate id; rewrite every reference: other nodes' `next`/`split`/`dependsOn`/`thenTargets`/`elseTargets` and loop memberships. (Prompt-text `steps.old` references are NOT rewritten — matches web behavior; the validate layer flags danglers.)
  - `public func applyUpdate(_ g: WorkflowGraph, id: String, data: StepNodeData) -> WorkflowGraph` — replace one node's data (form edits; branch target edits included since routing is data-borne).

- [ ] **Step 1: Write failing tests:**

```swift
@Test func addAllocatesSmallestFreeID() throws {
    let g0 = try graph("name: t\nsteps:\n  - {id: step-1, agent: x, prompt: p}\n")
    let (g1, id) = applyAdd(g0, kind: .step, at: .init(x: 10, y: 10))
    #expect(id == "step-2")
    #expect(g1.nodes.count == 2)
}

@Test func addedGateRoundTripsImmediately() throws {
    let (g, _) = applyAdd(try graph("name: t\nsteps: []\n"), kind: .approvalGate, at: .zero)
    guard case .object(let obj) = graphToWorkflowObject(g) else { return Issue.record("gate must serialize") }
    #expect(obj["steps"]?.sequenceValue?[0]["approval"] != nil)
}

@Test func connectMaterializesLegacyChainFirst() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p}\n  - {id: b, agent: x, prompt: p}\n  - {id: c, agent: x, prompt: p}\n")
    guard case .connected(let g2) = applyConnect(g, source: "a", target: "c", arm: nil) else { return Issue.record("connect refused") }
    // The pre-existing implicit a->b and b->c chain must survive as explicit next.
    let ids = Set(g2.edges.map { "\($0.source)->\($0.target)" })
    #expect(ids == ["a->b", "b->c", "a->c"])
}

@Test func connectRejectsCycle() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p, next: [b]}\n  - {id: b, agent: x, prompt: p}\n")
    guard case .rejected(let why) = applyConnect(g, source: "b", target: "a", arm: nil) else { return Issue.record("expected reject") }
    #expect(why == "This would create a cycle — steps must form a DAG.")
}

@Test func branchArmConnectWritesTargets() throws {
    let g = try graph("name: t\nsteps:\n  - id: r\n    branch: {condition: c}\n  - {id: a, agent: x, prompt: p}\n")
    guard case .connected(let g2) = applyConnect(g, source: "r", target: "a", arm: "then") else { return Issue.record("refused") }
    #expect(g2.nodes[0].data.thenTargets == ["a"])
}

@Test func deleteScrubsEveryReference() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p, next: [b]}\n  - {id: b, agent: x, prompt: p}\n  - id: r\n    branch: {condition: c, then: [b], else: [a]}\n  - id: s\n    split: [b, a]\n")
    let g2 = applyDelete(g, id: "b")
    #expect(!g2.nodes.contains { $0.id == "b" })
    #expect(g2.nodes.first { $0.id == "a" }?.data.next == [])
    #expect(g2.nodes.first { $0.id == "r" }?.data.thenTargets == nil || g2.nodes.first { $0.id == "r" }!.data.thenTargets! == [])
    #expect(g2.nodes.first { $0.id == "s" }?.data.split == ["a"])
}

@Test func renameSlugifiesAndRewritesEdges() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p, next: [b]}\n  - {id: b, agent: x, prompt: p}\n")
    guard case .renamed(let g2, let id) = applyRename(g, from: "b", to: "My Step!!") else { return Issue.record("refused") }
    #expect(id == "my-step")
    #expect(g2.nodes.first { $0.id == "a" }?.data.next == ["my-step"])
}

@Test func renameRejectsDuplicate() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p}\n  - {id: b, agent: x, prompt: p}\n")
    guard case .rejected = applyRename(g, from: "b", to: "a") else { return Issue.record("expected reject") }
}
```

- [ ] **Step 2: Run to verify failure; implement; run green; commit.**

---

### Task 9: CPClient write endpoint + BuilderStore core

**Files:**
- Modify: `apps/rupu-macos/RupuKit/Sources/RupuAPI/WriteModels.swift` (add `WorkflowWriteBody`), `apps/rupu-macos/RupuKit/Sources/RupuAPI/CPClient.swift` (add `writeWorkflow`)
- Create: `apps/rupu-macos/RupuKit/Sources/RupuBuilder/BuilderStore.swift` (replaces Placeholder.swift)
- Test: `Tests/RupuAPITests/WorkflowWriteTests.swift` (follow the existing CPClient test harness pattern in that target — find the mock-transport helper the config PUT tests use and mirror it), `Tests/RupuBuilderTests/BuilderStoreTests.swift`

**Interfaces:**
- Produces (RupuAPI): `public struct WorkflowWriteBody: Encodable, Equatable, Sendable { public let raw: String; public init(raw: String) }`; `public func writeWorkflow(name: String, body: WorkflowWriteBody, scopeKind: String?, scopeID: String?) async throws` — `PUT api/workflows/\(name)` with `scope_kind`/`scope_id` query items when non-nil (mirror how the existing scoped DELETE/PUT calls build query strings; reuse the internal `put(_:body:)` helper at `CPClient.swift:755`).
- Produces (RupuBuilder):

```swift
@MainActor @Observable
public final class BuilderStore {
    public enum Phase: Equatable { case loading, failed(String), unsupported(String), ready }
    public enum Mode: Equatable { case design, run }
    public private(set) var phase: Phase
    public private(set) var graph: WorkflowGraph
    public private(set) var canonicalYAML: String
    public private(set) var problems: [String: [String]]
    public private(set) var selectedID: String?
    public private(set) var dirty: Bool
    public private(set) var serverValid: Bool?     // nil = not yet checked
    public private(set) var commitError: String?   // rejected edit (cycle), transient
    public private(set) var saveError: String?
    public var mode: Mode
    public private(set) var agents: [AgentDefinition] // picker catalog (client.agentDefinitions())
    public private(set) var tools: [ToolSpec]       // action tool catalog

    public init(name: String, scopeKind: String?, scopeID: String?,
                client: CPClient, pendingActions: PendingActions)
    public func activate() async        // GET detail → parse → layout; fetch agents + tools best-effort
    public func select(_ id: String?)
    public func commit(_ next: WorkflowGraph)   // serialize-first, all-or-nothing (WorkflowEditor.tsx commit port)
    public func addNode(kind: StepKind, at: CGPoint)
    public func connect(source: String, target: String, arm: String?)
    public func moveNode(id: String, to: CGPoint)   // position-only: NO YAML re-emit, but topo tiebreak uses it — re-serialize canonicalYAML without marking dirty? NO: position changes CAN reorder topo ties, so moves DO re-emit + mark dirty (honest round-trip).
    public func deleteSelection()
    public func rename(id: String, to: String) -> Bool
    public func updateStep(_ data: StepNodeData)
    public func updateMeta(name: String?, description: String?)
    public func save() async -> Bool    // PUT; true on success; clears dirty
    public func revalidate() async      // POST /api/workflows/validate {raw: canonicalYAML} → serverValid
}
```

`activate()`: `client.workflowDetail(name:)` → `YAMLParser.parse` (a `YAMLError` → `.unsupported(message)` phase — the builder never silently degrades, per matt's no-mock-features rule) → `yamlToGraph` → `autoLayout` → `canonicalYAML = dump(graphToWorkflowObject(...))`, `dirty = false` (the canonical rewrite of an untouched load is NOT dirty — dirtiness means user edits, not reformatting). `commit(_:)`: run `graphToWorkflowObject`; `.failure` → set `commitError`, discard; `.object` → set graph/canonicalYAML/problems, `dirty = true`, kick a 400ms debounced `revalidate()` task (cancel the prior). `save()`: `pendingActions.begin(ActionKey("workflow-editor", .save))`-style key (check `ActionKey`/verb vocabulary in `RupuStore/PendingActions.swift` and add a `.save` verb if absent — mirror how ConfigStore's raw-TOML save names its key), PUT, on success `dirty = false` + `confirm`; on error set `saveError` + `fail`.

- [ ] **Step 1: Write failing CPClient test** (mirror the existing pattern in `Tests/RupuAPITests` for a PUT — asserting method, path `api/workflows/nightly`, query `scope_kind=project&scope_id=ws1`, and body `{"raw":"name: nightly\n"}`).
- [ ] **Step 2: Write failing BuilderStore tests.** `CPClient` in tests: check how existing store tests fake the client (`Tests/RupuStoreTests` — they build a `CPClient` over a stubbed `URLProtocol` or a protocol seam; reuse the same harness; if `CPClient` is concrete-only, add the same seam style used there, do not invent a new one). Core cases:

```swift
@Test @MainActor func activateParsesAndIsNotDirty() async { /* stub GET detail returns two-step YAML; expect phase == .ready, graph.nodes.count == 2, dirty == false, canonicalYAML fixed-point */ }
@Test @MainActor func unsupportedYAMLSurfacesHonestly() async { /* stub YAML with an anchor; expect phase == .unsupported and no graph mutation affordances */ }
@Test @MainActor func commitRejectsCycleWithoutMutating() async { /* build cycle graph, commit; expect commitError set, graph unchanged, dirty unchanged */ }
@Test @MainActor func editFlowMarksDirtyAndRegeneratesYAML() async { /* addNode then expect dirty, canonicalYAML contains the new id */ }
@Test @MainActor func saveClearsDirtyOnSuccess() async { /* stub PUT 200; expect dirty == false */ }
@Test @MainActor func saveFailureKeepsDirtyAndSurfacesError() async { /* stub PUT 400 {error}; expect dirty == true, saveError != nil */ }
@Test @MainActor func renameKeepsSelectionOnNewID() async { /* select b, rename b→my-step, expect selectedID == "my-step" */ }
@Test @MainActor func deleteClearsSelection() async { /* select then deleteSelection; expect selectedID == nil, node gone */ }
```

- [ ] **Step 3: Run to verify failure; implement WriteModels/CPClient/BuilderStore; run green.**
- [ ] **Step 4: Commit** — `feat(macos): workflow write endpoint + BuilderStore round-trip core`.

---

### Task 10: Screen shell — header, canvas chrome, YAML pane, rail tabs, routing

**Files:**
- Create: `Sources/RupuBuilder/WorkflowBuilderScreen.swift`, `BuilderHeader.swift`
- Modify: `Sources/RupuShell/RootView.swift:235` (render `WorkflowBuilderScreen(model:backend:name:scopeKind:scopeID:)`), `Sources/RupuShell/RouteDisplay.swift` (label stays "Workflow")
- Delete: `Sources/RupuLibrary/WorkflowDetailScreen.swift` (+ its tests in `Tests/RupuLibraryTests` — port any still-relevant assertions to RupuBuilderTests; the autoflow-toggle behavior moves to Task 13's Settings tab)
- Test: `Tests/RupuBuilderTests/BuilderScreenWiringTests.swift` (store-level assertions; SwiftUI bodies aren't render-tested in this package — same convention as every other screen module)

**Layout contract (spec §1):**
- Header row, fixed 46pt: back chevron (`Icon(.arrowLeft)`, `model.navigateBack()` — check AppModel for the exact back API used by `AgentDetailScreen`'s chevron and reuse it), breadcrumb `Library ▸ <name>` in `.leadText` (name `.semibold`, "Library" `.rupuDim`), `StatusPill` (existing RupuDesign chrome) showing `running` only while Run mode has a live run; `Spacer()`; valid dot (8pt circle: `Color.status(.done)` when `serverValid == true && problems.isEmpty`, `Color.status(.failed)` when invalid, `rupuMute` while nil) + label `valid`/`invalid` in `.noteText`; Design/Run segmented control (SwiftUI `Picker` `.segmented` or the app's segmented idiom — check how Activity's Runs/Claims sub-toggle is built and reuse that control style); YAML source toggle button (`Icon(.code…)` — lucide `code` is NOT yet in the set: reuse `chevronUp`/`chevronDown` alternative is wrong; add `{ enumCase: "code", asset: "code", source: "code" }` to the icon table in this task, same regen flow as Task 7); Save button (`RupuButtonStyle` secondary; shows a 6pt brand dot when `dirty`; `.keyboardShortcut("s", modifiers: .command)`); Launch button (`RupuButtonStyle.primary` — verify it renders `rupuBrand600` bg + white text radius 5; if primary uses a different radius/token, add a `.builderLaunch` style rather than restyling the shared one).
- Below: `HSplit`-free fixed layout — left column (canvas over YAML pane), right inspector rail fixed 320pt with top tab strip `Blocks | Step | Settings` (`.uiText`, active tab `rupuInk` + 2px `rupuBrand` underline, inactive `rupuDim`), content per tab (Tasks 11/13).
- YAML pane: 192pt tall, collapsible via the header toggle; `@AppStorage("rupu.builder.sourceOpen") var sourceOpen = true`; header row: `Eyebrow("WORKFLOW.YAML")` + `canonical · round-trips with the canvas` in `.noteText` `rupuMute`; body: `ScrollView` with `Text(store.canonicalYAML).font(.dataMono(11)).textSelection(.enabled)`, `rupuBg` background, top border 1px `rupuBorder`.
- Persistence: `@AppStorage("rupu.builder.railTab")` (String: blocks/step/settings), `@AppStorage("rupu.builder.sourceOpen")`. (Rail width is fixed 320 this phase — spec gives a fixed width; the web's resizable rail is not in the mockup. The acceptance item "rail width/tab persist" is satisfied by tab persistence + fixed width; note this in the PR.)
- Selection behavior: `store.select(id)` flips rail to Step tab (set the AppStorage-backed tab state); Esc in canvas clears selection.

- [ ] **Step 1: Write the wiring tests** (behavioral, store-level): selecting flips tab state helper; valid-dot state function `validDotTone(serverValid:problems:) -> StatusTone?` extracted as a pure `internal` function with tests (nil→mute case returns nil, true+empty→`.done`, else `.failed`).
- [ ] **Step 2: Implement screen + header per contract.** `WorkflowBuilderScreen` owns `@State private var store: BuilderStore?` built in `.task(id: name)` with the same client-identity guard recipe `WorkflowDetailScreen.activateStore()` used (copy that doc-commented pattern). Canvas placeholder: `Color.rupuBg` + dot grid (Task 11 fills interactions) — implement the dot grid NOW as `Canvas { context, size in … }` drawing 1px `rupuBorder` circles at 22pt pitch.
- [ ] **Step 3: Swap the route** in `RootView.swift:235` to `WorkflowBuilderScreen`, delete `WorkflowDetailScreen.swift`, fix `Tests/RupuLibraryTests` compile (delete the dead tests; the LibraryScreen list tests stay).
- [ ] **Step 4: `make macos-test` + `make macos-build` green** (full app — the routing change crosses modules), commit.

---

### Task 11: Canvas — nodes, edges, drag, connect, keyboard

**Files:**
- Create: `Sources/RupuBuilder/CanvasView.swift`, `NodeView.swift`, `ShapePaths.swift`, `EdgeLayer.swift`
- Test: `Tests/RupuBuilderTests/CanvasGeometryTests.swift`

**Interfaces:**
- `ShapePaths.swift`: `struct SilhouetteShape: Shape { let name: ShapeName; func path(in rect: CGRect) -> Path }` (wraps `shapeFor(name, w: rect.width, h: rect.height).path`, offset into rect) + `struct SilhouetteExtras: Shape` (the `extra` rails/layers). `func accentColor(_ a: KindAccent) -> Color` mapping: `.statusRunning→Color.status(.running)`, `.brand500→.rupuBrand`, `.sevCritical→Color.severity(.crit)`, `.statusAwaiting→Color.status(.awaiting)`, `.statusDone→Color.status(.done)`, `.statusPaused→Color.status(.paused)`, `.sevInfo→Color.severity(.info)`, `.sevMedium→Color.severity(.med)`, `.brand600→.rupuBrand600`, `.brand700→.rupuBrand700`.
- `NodeView.swift`: 176×68 node; fill `rupuPanel`, stroke 1.4pt accent (2pt `rupuBrand` + `.shadow(color: Color.rupuBrand.opacity(0.35), radius: 8)` when selected). Content inside `safe` rect (respect `centered`): row 1 = `Icon(named kind icon, size: 11)` + kind chip (`Text(kind.rawValue.uppercased()).font(.dataMono(9)).kerning(0.8)` in accent); row 2 = step id `.system(size: 12.5, weight: .semibold)` `rupuInk`, lineLimit 1; row 3 = sub-line `.system(size: 10.5)` `rupuDim` (mono `dataMono(10.5)` for run/action), content per kind: step/for_each → agent name; for_each also `over <for_each>` when agent empty; parallel → `\(n) sub-steps`; panel → `\(n) panelists`; branch → `if <condition>` truncated; split → `→ \(n) targets`; join → `wait: \(policy)`; gate → approval prompt or `human approval`; action → tool name; run → `$ <cmd> <args…>`. Ports: source dot(s) 8pt circle `rupuBorderStrong` fill + accent stroke at each `SourceAnchor` (branch: two, labeled `THEN`/`ELSE` in `dataMono(8)` accent near the dots), target dot on the left at hover-time only. `BuilderNodeStyle` enum gates the "cards" variant (rounded rect 6pt radius + 3.5pt accent top bar) — `static let current: BuilderNodeStyle = .silhouette`.
- `CanvasView.swift`: `ScrollView([.horizontal, .vertical])` containing a `ZStack` sized to `contentSize` (computed: max node x+176+120, max y+68+120, min 1200×800): `DotGrid` background, `EdgeLayer`, nodes at `position`. Node drag: `DragGesture` on the node body updates a transient offset, on end `store.moveNode(id:to:)` clamped ≥ 0. Click selects. Port drag: `DragGesture` starting on a port dot draws a dashed `rupuBrand` bezier in an overlay from the port to the current point; on end, hit-test node frames — inside a node → `store.connect(source:target:arm:)` (rejections surface via `store.commitError` banner, reusing the reason strings); else discard. Keyboard: the canvas wrapper is `.focusable()`; `.onKeyPress(.escape)` clears selection; `.onKeyPress(.delete)` calls `store.deleteSelection()` — deletion NEVER fires while a text field has focus (text fields live only in the rail, which holds its own focus; document this invariant in a comment).
- `EdgeLayer.swift`: for each `GraphEdge`, resolve source anchor (branch arm → matching `SourceAnchor`, else the single source) and target anchor from the two nodes' geometry; cubic bezier with horizontal control offsets `min(120, max(40, dx*0.5))`; stroke 1.5pt `rupuBorderStrong`; arrowhead: 7pt chevron at target rotated to the end tangent; branch labels `THEN`/`ELSE` (`dataMono(8)` uppercase, accent `Color.status(.done)`) at 24pt past the source anchor. Pure geometry helpers (`bezierPoints(from:to:) -> (c1: CGPoint, c2: CGPoint)`, `edgeAnchors(edge:nodes:) -> (CGPoint, CGPoint)?`) are `internal` functions.

- [ ] **Step 1: Write failing geometry tests** — `edgeAnchors` picks the right side/arm anchors (branch else exits bottom); `bezierPoints` control clamps; content-size computation grows with node positions; connect hit-test helper `nodeHit(at:nodes:) -> String?` finds the node whose frame contains a point (and nil outside).
- [ ] **Step 2: Run to verify failure; implement the four files.**
- [ ] **Step 3: `make macos-test` green; commit.**

---

### Task 12: Palette tab (Blocks) — cards, detail, add + drag-to-canvas

**Files:**
- Create: `Sources/RupuBuilder/PaletteTab.swift`
- Modify: `Sources/RupuBuilder/CanvasView.swift` (drop-target overlay), `WorkflowBuilderScreen.swift` (shared drag state)
- Test: `Tests/RupuBuilderTests/PaletteTests.swift`

**Contract (spec §4, `NodePalette.tsx`):**
- Filter `TextField` (`rupuBg` bg, 1px `rupuBorder`, radius 5) matching kind label/tagline/what, case-insensitive.
- Sections: `Eyebrow("WORK")`, `Eyebrow("ORCHESTRATION")` — kinds via `kindVisual(_:).family`, catalog order = spec table order.
- Card per kind: mini silhouette preview 34×20 (`SilhouetteShape` stroked 1.2pt accent — the small-box clamps from Task 7 make this legible), label `dataMono(11)` in accent, tagline `.noteText` `rupuDim`; card bg `rupuSurface` on hover, selected card 1px accent border.
- Detail card below (selected kind): what-blurb `.uiText` `rupuDim`; required fields as `dataMono(10)` chips each suffixed `*`; example in `dataMono(10)` block on `rupuBg`; **Add to canvas** button (`RupuButtonStyle` primary, full width) → `store.addNode(kind:at:)` at the visible canvas center.
- Drag-to-canvas: custom pointer-tracked ghost (spec allows either): a `DragGesture(coordinateSpace: .named("builder"))` on the card sets `@State var paletteDrag: (kind: StepKind, point: CGPoint)?` on the screen; the screen overlays a ghost mini-silhouette at the point; on end, if the point is inside the canvas frame (`GeometryReader`-captured), convert to canvas coordinates (+ scroll offset) and `store.addNode(kind:at: converted)`. (No `NSItemProvider` — in-window drag needs no pasteboard.)

- [ ] **Step 1: Write failing tests** for the pure parts: `filteredCatalog(query:) ` matching; section split by family covers all 10 kinds exactly once; canvas-point conversion helper `canvasPoint(fromWindowPoint:canvasOrigin:scrollOffset:)`.
- [ ] **Step 2: Run to verify failure; implement; run green; commit.**

---

### Task 13: Step form + Settings tab

**Files:**
- Create: `Sources/RupuBuilder/StepFormTab.swift`, `Sources/RupuBuilder/SettingsTab.swift`
- Test: `Tests/RupuBuilderTests/StepFormModelTests.swift`

**Contract (spec §7, `StepForm.tsx` field vocabulary, `workflow.rs` names):**
- All inputs: `Eyebrow` labels, `rupuBg` field bg, 1px `rupuBorder`, radius 5, brand focus ring (`.overlay(RoundedRectangle(cornerRadius: 5).stroke(focused ? Color.rupuBrand : Color.rupuBorder, lineWidth: focused ? 1.5 : 1))` via `@FocusState`).
- Header: 44×26 silhouette preview + id + kind chip.
- Fields per kind (each edit builds a new `StepNodeData` → `store.updateStep(_:)`; commits debounce 300ms per field so typing doesn't spam round-trips — hold local `@State` text, push on debounce AND on focus loss):
  - all kinds: STEP ID (`TextField`, commit via `store.rename(id:to:)` on submit/focus-loss; rejection shows inline `.noteText` error in `rupuErr`).
  - step: AGENT (Picker over `store.agents` names + current value if unknown), PROMPT (`TextEditor` 5 lines, mono NO — prompts are prose, `.uiText`).
  - for_each: AGENT, PROMPT, FOR EACH (`dataMono(11)` TextField), MAX PARALLEL (numeric TextField, empty → nil).
  - parallel: SUB-STEP list editor — one row per `SubStep` (ID + AGENT picker + PROMPT), add/remove row buttons; MAX PARALLEL.
  - panel: PANELISTS (multi-row of agent pickers, add/remove), SUBJECT (`dataMono(11)`), PROMPT (optional), MAX PARALLEL.
  - branch: CONDITION (`dataMono(11)`), derived rows below.
  - split: derived `targets →` row only (targets are edge-derived; SUB-STEP IDS not hand-typed — the web edits split targets by drawing edges; `split:` array IS the storage: the derived row lists `data.split`).
  - join: WAIT POLICY (Picker all/any/count + count numeric field when `.count`).
  - approval_gate: APPROVAL PROMPT, TIMEOUT (seconds numeric), ON TIMEOUT (Picker approve/reject/fail/—).
  - action: TOOL (Picker over `store.tools` names + free-text fallback when catalog empty), WITH (raw YAML sub-editor: `TextEditor` `dataMono(11)` whose text parses via `YAMLParser` into `data.with` on debounce; parse error shown inline, value not committed).
  - run: COMMAND (`dataMono(11)` TextField bound to `runBlock["cmd"]`, preserving all sibling keys via `mapping(settingKey:to:)`).
- Derived read-only rows (from edges/data): `then → a, b`, `else → …`, `targets → …` (split), `waits on ← ` inbound edge sources (join) — `dataMono(10)` `rupuDim`.
- **Remove step** destructive button (`rupuErr` text, confirm-free — ⌫ parity) → `store.deleteSelection()`.
- Empty state (no selection): `Select a step on the canvas` `.noteText` `rupuMute` centered.
- Settings tab: NAME (TextField → `updateMeta(name:…)` — note: renaming a workflow changes the PUT target contract (`parsed name must equal :name`); on name-change, Save PUTs to the NEW name path — implement `save()` to use `graph.meta.name` as the path segment and document that the old file stays until a delete, matching the web's create-on-write behavior), DESCRIPTION (TextEditor), TRIGGER card (read-only render of `meta.rest["trigger"]`: kind pill via `Color.trigger(_:)` + cron expression `dataMono(11)`; no trigger → `manual` pill), INPUTS card (read-only rows: name + type + required marker from `meta.rest["inputs"]`), AUTOFLOW card (the toggle ported from the deleted `WorkflowDetailScreen.autoflowToggleRow` — same `LibraryStore.setAutoflowEnabled` call, only rendered when the definition row carries `autoflowEnabled`), scope `Badge` (from the `LibraryStore` definition row, as before).
- StepFormModelTests: pure helpers — `derivedRows(for: node, graph:) -> [(label: String, value: String)]` covering branch/split/join; the run COMMAND setter preserves sibling `run:` keys; the `with:` sub-editor parse-or-hold behavior.

- [ ] **Step 1: Write failing tests for the pure helpers; run to verify failure.**
- [ ] **Step 2: Implement both tabs; wire into the rail; run green.**
- [ ] **Step 3: Commit.**

---

### Task 14: Run overlay + Launch integration + acceptance sweep

**Files:**
- Create: `Sources/RupuBuilder/RunOverlayModel.swift`
- Modify: `Sources/RupuBuilder/BuilderStore.swift` (run-follow), `NodeView.swift`/`EdgeLayer.swift` (overlay rendering), `BuilderHeader.swift` (Launch behavior, running pill)
- Test: `Tests/RupuBuilderTests/RunOverlayTests.swift`
- Modify: `CLAUDE.md` (module map: add `RupuFlowKit`/`RupuBuilder` one-liners to rule 3's module list; note the WorkflowDetailScreen replacement)

**Contract (spec §8, `StepGraphView.swift` glyph language, `RunDetailStore`):**
- Entering Run mode: `BuilderStore.enterRunMode(client:backend:)` finds the run to follow — the run launched from this screen this session if recorded, else the newest run for this workflow from `client` runs listing (`api/runs/workflows`, filter rows by workflow name, first row), else Run mode shows an empty-state banner (`No runs yet — Launch one`) with no overlay. Following = owning a `RunDetailStore(runID:host:client:backend:)` + `activate()`; leaving Run mode deactivates it (mirror RunDetailScreen's lifecycle calls).
- `RunOverlayModel.swift`: `public struct RunOverlay: Equatable { public let states: [String: NodeState]; public let unitProgress: [String: (done: Int, total: Int)] }` + `public func runOverlay(detail: APIRunDetail?, graph: APIRunGraph?, liveStates: [String: NodeState]) -> RunOverlay` — the `layoutGraph` priority port (live wins → result settles skipped/done → pending), keyed by the run's step ids which are the same YAML step ids the canvas draws. Manual `==` for the tuple map.
- Node overlay (per `StepGraphView` semantics): done → 14pt green check badge (`Icon(.checkCircle2, size: 14)` in `Color.status(.done)`) at top-right corner + stroke `Color.status(.done)`; running → pulsing 8pt `Color.status(.running)` dot at top-right (scale 1→1.3, opacity 1→0.5, `.repeatForever`, fully disabled under `accessibilityReduceMotion` — static dot instead) + stroke `.running`; gatePending → same pulse in `Color.status(.awaiting)`; pending → dashed ring (`StrokeStyle(lineWidth: 1.4, dash: [3, 2])` in `rupuBorder`) + content opacity 0.6; skipped → `rupuMute` ring + node opacity 0.45; for_each running/done shows `n/m items` (from unitProgress) replacing the sub-line. Design-mode rendering is untouched when `mode == .design`.
- Edge overlay: both endpoints done → `Color.status(.done)` stroke; target running → `Color.status(.running)` + dashed `[6, 4]` with an animated phase offset (reduce-motion: static dashes); source or target skipped → dashed `rupuBorder` at 0.5 opacity; else design default.
- Header: `StatusPill` `running` (existing chrome component, `StatusTone.running`) shown while the followed run's status is running/pending; Launch button behavior: `await store.save()` first — on failure surface `saveError` and do NOT launch; on success `model.presentLauncher(kind: .workflow, name: graph.meta.name, scopeKind:, scopeID:)` and set `mode = .run` so returning to the builder shows the overlay of the newest run.
- RunOverlayTests: the priority merge (live beats result beats pending); skipped mapping; unit progress counts (`success != nil` = done); newest-run pick helper `latestRunID(rows:workflowName:)`.

- [ ] **Step 1: Write failing overlay-model tests; run; implement model; green.**
- [ ] **Step 2: Implement the rendering + Launch flow.**
- [ ] **Step 3: Acceptance sweep against the spec checklist** — walk each item; for the two GUI-only items (light/dark correctness, glyph rendering) run `make macos-run`, exercise: place all 10 kinds via Add AND via drag; connect incl. branch arms; rename with edge rewrite visible in YAML pane; delete; field edits; toggle appearance (System Settings or `defaults`), toggle Reduce Motion; screenshot for the PR. Record results in the PR description.
- [ ] **Step 4: Update CLAUDE.md module map; `make macos-test` + `make macos-build` green; commit; open PR** (branch → push → PR per repo convention). PR body includes: the decision log (YAML engine, run-kind extension, loops round-trip-only, Launch deviation), screenshots both appearances, and the note that matt must run the app before merge (rule 7).

---

## Self-review notes (already applied)

- **Spec coverage:** §1 layout → Task 10; §2 tokens → Global Constraints + per-task token callouts; §3 kinds/shapes → Task 7 (+ Task 11 rendering); §4 palette → Task 12; §5 canvas interactions → Task 11; §6 round-trip → Tasks 2–5, 8, 9 (commit-serialize-first); §7 step form/settings → Task 13; §8 run overlay → Task 14; §9 checklist → Task 14 Step 3 sweep. Save model (matt's decision) → Tasks 9/10/14.
- **Acceptance-item deltas, stated:** "rail width persists" — width is fixed 320pt per spec layout; only the tab + YAML-pane state persist (flagged in Task 10 for the PR). "Launch flips to Run" — Launch saves → launcher sheet → run mode on return (Key decision 4).
- **Type consistency:** `StepNodeData` field names in Tasks 4/8/13 match; `ConnectVerdict`/`ConnectOutcome`/`RenameOutcome`/`SerializeResult`/`TopoResult` names used consistently across Tasks 5/8/9; `NodeShapeGeometry.sources[].arm` matches `GraphEdge.branchArm` vocabulary (`"then"`/`"else"`).
- **Known simplifications, all explicit:** no loop-editing UI (decision 3); no YAML text editing in-pane (read-only per spec); no dock palette placement (spec: "ship rail first"); `nodeStyle` cards variant behind a code-level flag (decision 6).
