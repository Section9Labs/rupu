# rupu.app Design Alignment — Plan 3: Run-Graph Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the app's status-only capsule strip with the web run graph's full visual system — per-kind accents/icons (two-channel rule), gate/action/parallel/fan-out/panel node treatments, kind-colored animated edges, live unit/round/pause patching, and node selection driving the tabbed panel — with a real selected-node highlight the web doesn't have.

**Architecture:** The pure `layoutGraph` grows into a v2 view model (`GraphNodeVM` gains kind/action/gate/sub-steps/fan-out/round + transcript path) fed by `RunDetailStore`'s extended live maps; a `KindBridge` mirrors the web's `kindBridge.ts`/`kindVisuals.ts` palette; node views implement the web's per-kind treatments (`components/graph/*`); edges and motion port the `rg-*` keyframes as SwiftUI animations. Selection taps route through the already-built `RunDetailStore.focusStep`.

**Tech Stack:** SwiftUI (macOS 14+), Swift 6, Swift Testing. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-24-rupu-macos-design-alignment-design.md` §4 · **Contract:** `docs/macOS_design/V2-CONTRACT.md` ("Run graph" section) · **Web sources of truth:** `crates/rupu-cp/web/src/lib/runGraphModel.ts`, `components/graph/{StepNode,GateNode,ActionNode,ParallelNode,FanoutNode,PanelLoopNode}.tsx`, `components/workflow-editor/kindVisuals.ts`, `components/graph/stepStyle.ts`, `web/src/styles.css` lines 200–248 (`rg-*` keyframes).

## Global Constraints

- Plans 1 and 2 have landed: v2 tokens (`StatusTone`, `Color.status(_:)`, `Color.severity(_:)`), lucide `Icon` view, chrome kit, and the Plan 2 run-detail recomposition (header → graph → tabbed panel) all exist. If Plan 2's tabbed panel is NOT yet merged when this plan executes, STOP and say so — Task 5's selection wiring targets it.
- **Two-channel rule (never violate):** kind color only on the 3px top accent bar + kind pill; state color only on the glyph badge + state label.
- Kind accents (from `kindVisuals.ts:17-48`): step→`Color.status(.running)` · for_each→`.rupuBrand` · parallel→`Color.severity(.crit)` · panel→`Color.status(.awaiting)` · gate→`Color.status(.paused)` · action→`Color.severity(.info)` · run→`Color.severity(.med)`.
- Merge precedence in the model: live SSE > step results > `.pending`. Never infer `.running` from position.
- All motion guarded by `accessibilityReduceMotion` (follow `StepGraphView.swift`'s existing `updatePulse` pattern — including its `onChange(of: state)` restart fix).
- Store/contract discipline: `make macos-test` + `make macos-build` green after every task; only files this plan names change. Never bare `git stash pop`.
- Null discipline unchanged (`—` never 0).

## File structure

```
RupuKit/Sources/RupuRunDetail/KindBridge.swift        # NEW: kind → accent/icon/label
RupuKit/Sources/RupuStore/NodeState.swift             # +paused; UnitLiveState; PanelRoundState
RupuKit/Sources/RupuRunDetail/GraphLayout.swift       # Rewrite: GraphNodeVM v2 + layoutGraph v2
RupuKit/Sources/RupuStore/RunDetailStore.swift        # apply(): unit/round/pause patching; selection cursor
RupuKit/Sources/RupuRunDetail/Graph/StepNodeCard.swift     # NEW (split from StepGraphView)
RupuKit/Sources/RupuRunDetail/Graph/ContainerNodes.swift   # NEW: parallel + panel + fan-out
RupuKit/Sources/RupuRunDetail/Graph/GraphEdge.swift        # NEW: connector w/ ants
RupuKit/Sources/RupuRunDetail/StepGraphView.swift          # Rewrite: canvas, selection, seed
apps/rupu-macos/scripts/extract-lucide.mjs            # +5 icons; regenerate
RupuKit/Tests/RupuRunDetailTests/{KindBridgeTests,GraphLayoutTests}.swift
```

---

### Task 1: KindBridge + five new lucide icons

**Files:**
- Modify: `apps/rupu-macos/scripts/extract-lucide.mjs` (add kebab names: `bot`, `columns-3`, `user-check`, `zap`, `terminal`)
- Modify (regenerated): `RupuKit/Sources/RupuDesign/Icons/LucideIconData.swift`, `Icons/svg/*.svg`
- Modify: `RupuKit/Sources/RupuDesign/Icons/Icon.swift` scope only if enum lives there — add cases to `LucideIcon`: `bot, columns3, userCheck, zap, terminal`
- Create: `RupuKit/Sources/RupuRunDetail/KindBridge.swift`
- Test: `RupuKit/Tests/RupuRunDetailTests/KindBridgeTests.swift`

**Interfaces:**
- Produces:
  ```swift
  public enum StepKind: String, CaseIterable, Sendable {
      case step, forEach = "for_each", parallel, panel, gate, action, run
      public init(raw: String)   // unknown raw → .step (web bridge does the same)
  }
  public extension StepKind {
      var accent: Color   // per Global Constraints table
      var icon: LucideIcon  // .bot,.repeatIcon,.columns3,.shieldCheck,.userCheck,.zap,.terminal
      var label: String     // "step","for each","parallel","panel","gate","action","run"
  }
  ```
- Consumes: Plan 1's `LucideIcon` enum + `LucideIconData.paths(for:)`.

- [ ] **Step 1: Failing tests** — table-driven: every `StepKind` raw string round-trips; unknown raw → `.step`; each kind's `icon` case exists in `LucideIconData` (≥1 parseable path — reuse the LucideIconDataTests helper); label table exact.
- [ ] **Step 2:** Add the five kebab names to the extractor list, run `node apps/rupu-macos/scripts/extract-lucide.mjs` (`npm ci` in `crates/rupu-cp/web` first if node_modules absent), add the five `LucideIcon` cases, write `KindBridge.swift`. GREEN via `make macos-test`.
- [ ] **Step 3: Commit** — `feat(macos-graph): KindBridge (kind→accent/icon/label) + bot/columns3/userCheck/zap/terminal icons`

### Task 2: Graph model v2 (pure)

**Files:**
- Modify: `RupuKit/Sources/RupuStore/NodeState.swift`
- Rewrite: `RupuKit/Sources/RupuRunDetail/GraphLayout.swift`
- Test: `RupuKit/Tests/RupuRunDetailTests/GraphLayoutTests.swift` (extend the existing suite)

**Interfaces:**
- Produces (in `RupuStore/NodeState.swift`):
  ```swift
  public enum NodeState: Equatable, Sendable {
      case done(success: Bool), running, gatePending, paused, pending, skipped  // + paused
  }
  public struct UnitLiveState: Equatable, Sendable {   // live overlay for one unit
      public let key: String?            // unit_key from CPEvent (REST rows have none)
      public let transcriptPath: String?
      public let success: Bool?          // nil while in flight
  }
  public struct PanelRoundState: Equatable, Sendable {
      public let round: UInt32; public let maxIterations: UInt32
  }
  ```
- Produces (in `GraphLayout.swift`):
  ```swift
  public struct SubStepVM: Identifiable, Equatable, Sendable { public let id: String; public let agent: String; public let state: NodeState }
  public struct UnitVM: Identifiable, Equatable, Sendable { public let id: Int /* index */; public let key: String; public let state: NodeState; public let transcriptPath: String? }
  public struct FanoutVM: Equatable, Sendable { public let units: [UnitVM]; public let done: Int; public let failed: Int; public let running: Int; public let total: Int }
  public struct GraphNodeVM: Identifiable, Equatable, Sendable {
      public let id: String
      public let kind: StepKind
      public let agentLabel: String?
      public let state: NodeState
      public let actionName: String?          // APIStepNode.action
      public let gateAuto: Bool               // approvalGate?.autoApprove ?? false
      public let gateHasOnReject: Bool
      public let subSteps: [SubStepVM]        // parallel/panel lanes; [] otherwise
      public let fanout: FanoutVM?            // for_each only
      public let panelRound: PanelRoundState? // panel only, live
      public let panelGate: APIPanelGate?     // panel gate condition line (Task 3 renders it)
      public let transcriptPath: String?      // step's own transcript when known live
  }
  public func layoutGraph(
      nodes: [APIStepNode], results: [APIStepResult], units: [APIUnitRow],
      liveStates: [String: NodeState],
      liveUnits: [String: [Int: UnitLiveState]],
      panelRounds: [String: PanelRoundState],
      stepTranscripts: [String: String]
  ) -> [GraphNodeVM]
  ```
- Semantics (mirror `runGraphModel.ts` phases 4–5):
  - Node state: `liveStates[id]` > result (`skipped`→`.skipped` else `.done(success:)`) > `.pending`. NEW: a `for_each` parent that is `.pending` but has any in-flight unit promotes to `.running` (web L296-301).
  - `fanout`: REST `units` rows (state: `success == nil ? .running : .done(success:)`) overlaid by `liveUnits[stepID]` per index (live key/transcript/success win); `key` falls back to `"#\(index)"` when no live key exists. Counts derived, not stored.
  - `subSteps`: from `node.parallel` (id+agent) or `node.panelists` (id=agent=name); state = `liveStates[subID]` > result-by-subID > `.pending`.
  - `laneCount`/`kindLabel`/`unitProgress` from the old VM are deleted — the new fields subsume them. Fix the two call sites (`RunDetailScreen`, tests) in this task.
- Consumes: Task 1's `StepKind`.

- [ ] **Step 1: Failing tests** — extend GraphLayoutTests table-driven: precedence triple (live beats result beats pending); paused; for_each promotion from in-flight unit; fanout overlay (live key/success wins over REST row, `#index` fallback); subSteps from parallel and panelists; gate flags; action name; panelRound passthrough; unknown kind → `.step`.
- [ ] **Step 2:** RED → rewrite `GraphLayout.swift` + extend `NodeState.swift` → GREEN (`make macos-test`; patch the `RunDetailScreen` call site minimally — it is properly rewired in Task 5).
- [ ] **Step 3: Commit** — `feat(macos-graph): graph model v2 — paused state, fan-out/sub-step/round VMs, live-overlay merge`

### Task 3: Node views

**Files:**
- Create: `RupuKit/Sources/RupuRunDetail/Graph/StepNodeCard.swift` (step + gate + action variants)
- Create: `RupuKit/Sources/RupuRunDetail/Graph/ContainerNodes.swift` (`ParallelNodeCard`, `PanelNodeCard`, `FanoutNodeView`)
- Test: none new (pure-view; logic already covered by Task 2; visuals verified Task 6)

**Interfaces:**
- Produces: `StepNodeCard(node: GraphNodeVM, isSelected: Bool)`, `ParallelNodeCard(node:isSelected:)`, `PanelNodeCard(node:isSelected:)`, `FanoutNodeView(node: GraphNodeVM, isSelected: Bool, onUnitTap: (Int) -> Void)`. All render inside the Task 4 canvas; none owns tap handling except `onUnitTap`.
- Anatomy (per `StepNode.tsx` / contract):
  - Card: `panel` fill, 1px border (gate: dashed `StrokeStyle(dash: [4,3])`), radius 7, 3px `kind.accent` top bar, min width 170. Pending at 75% opacity.
  - Row 1: 15pt square badge (state color fill, white glyph via `StatusPill`'s descriptor icons at 9pt) · step id (`uiText`, truncating) · state label right (`metaText`, state color).
  - Row 2: kind pill — `Icon(kind.icon, size: 10)` + `kind.label` (`metaText`) on `kind.accent.opacity(0.14)`, radius 4 · agent chip (`metaText`, `rupuDim`) when present.
  - Gate extras: `auto` Badge when `gateAuto`; `↳ on reject` caption (`metaText`, `rupuMute`) when `gateHasOnReject`.
  - Action: headline is `actionName` in `dataMono(12)`, step id drops to caption; `connector` Badge.
  - Parallel: container tinted `kind.accent.opacity(0.08)`, header `parallel · id` + `done/total ✓`; one chip per `subSteps` entry with a 12pt state glyph square; "no sub-steps" empty line.
  - Panel: header + `round r/max` (`dataMono`), spinning `↻` (`Icon(.repeatIcon)` rotating 2.4s linear, reduced-motion → static) while `state == .running`, gate line `gate ≥ untilSeverity · max maxIterations` from `GraphNodeVM.panelGate` (Task 2 threads it), panelist chips call nothing this plan (transcript focus for panelists is Plan 4 territory; render non-interactive).
  - Fan-out three-way (per `FanoutNode.tsx`): `total == 0` → state-aware placeholder text ("starting units…" running / "no units — nothing to fan out" done / "failed before fan-out" / "skipped" / "awaiting units…" pending); `total ≤ 12` → grid of 15pt unit squares (state fill, tap → `onUnitTap(index)`, `help("\(key) · \(state)")`); `> 12` → collapsed card: `done` large `dataMono` over `/ total units`, percent, 9pt progress bar filled `Color.status(.running)`→`.done` gradient, counts row (+ `failed` in `rupuErr` when > 0), 60-cell density grid of 9pt squares, `▸ expand all N` ghost button toggling the full grid.
  - Selected: 2px `rupuBrand` ring outside the border (`isSelected`).
  - Ring-pulse for `.running`/`.gatePending`: expanding stroked circle behind the badge, scale 1→~1.5 + fade, 1.7s ease-in-out repeat (port of `rg-pulse-run`/`rg-pulse-await`; awaiting uses `.awaiting` color) — reuse/extract the existing `updatePulse` restart discipline from `StepGraphView.swift:113-134`.

- [ ] **Step 1:** Implement all views. `make macos-build` green (views compile; nothing routes to them yet).
- [ ] **Step 2: Commit** — `feat(macos-graph): per-kind node cards — step/gate/action, parallel/panel containers, three-mode fan-out`

### Task 4: Edges + canvas rewrite

**Files:**
- Create: `RupuKit/Sources/RupuRunDetail/Graph/GraphEdge.swift`
- Rewrite: `RupuKit/Sources/RupuRunDetail/StepGraphView.swift`

**Interfaces:**
- Produces: `StepGraphView(nodes: [GraphNodeVM], selectedStepID: String?, onSelect: (String) -> Void, onSelectUnit: (String, Int) -> Void)`.
- `GraphEdge(source: GraphNodeVM, target: GraphNodeVM)` — 2pt horizontal line + arrowhead, 40pt long, vertically centered on the cards:
  - color: `Color.status(.awaiting)` when `target.state == .gatePending`; else `source.kind.accent`, at full alpha when target is `.running`/`.done`, `opacity(0.35)` otherwise (ghosted untraversed).
  - marching ants when target is `.running`/`.gatePending`: dash `[7,7]`, `strokeDashPhase` animated −28 over 0.7s linear repeat; reduced-motion → static dashes.
- Canvas: horizontal `ScrollView` of node views joined by edges (topology stays the linear chain, same as the web); container height moves from 140 → 420 in `RunDetailScreen` (that one-line height change happens here). Node tap → `onSelect(node.id)` — except fan-out unit squares, which call `onSelectUnit(stepID, index)` (mirror the web's body-click skip for containers: container background tap still selects the node).

- [ ] **Step 1:** Implement; wire `RunDetailScreen` to pass the new arguments (selection state arrives Task 5 — pass `selectedStepID: nil` + no-op closures for now). `make macos-build` + `make macos-test` green.
- [ ] **Step 2: Commit** — `feat(macos-graph): kind-colored animated edges + canvas rewrite with tap routing`

### Task 5: Live patching + selection wiring

**Files:**
- Modify: `RupuKit/Sources/RupuStore/RunDetailStore.swift`
- Modify: `RupuKit/Sources/RupuRunDetail/RunDetailScreen.swift`
- Test: extend `RupuKit/Tests/RupuStoreTests/RunDetailStoreTests.swift`

**Interfaces:**
- Produces on `RunDetailStore`:
  ```swift
  public private(set) var liveUnits: [String: [Int: UnitLiveState]] = [:]
  public private(set) var panelRounds: [String: PanelRoundState] = [:]
  public private(set) var stepTranscripts: [String: String] = [:]   // step_working adoption
  public private(set) var selection: GraphSelection?                // struct {stepID: String; unitIndex: Int?}
  public func select(stepID: String) async      // sets selection, calls focusStep(stepID)
  public func select(stepID: String, unitIndex: Int) async  // focuses the unit's transcriptPath
  ```
- `apply(_ event:)` extensions (`RunDetailStore.swift:616`): `unitStarted` → `liveUnits[stepID][index] = UnitLiveState(key:transcriptPath:success:nil)`; `unitCompleted` → same with `success`; `panelRound` → `panelRounds[stepID]`; `stepPaused` → `liveStates[stepID] = .paused`; `stepResumed` → `.running`; `stepWorking` with `transcriptPath` → `stepTranscripts[stepID]`.
- Selection seeding: on `activate()`, seed ONCE — prefer a `.running` node with a transcript, else the last node with a result/transcript (this generalizes the existing auto-focus at `RunDetailStore.swift:280`); live rebuilds must never overwrite a non-nil user `selection` (guard flag, mirroring the web's `seededSelRef`).
- `RunDetailScreen` threads `selection?.stepID` and the two closures into `StepGraphView`; the graph call site now passes the full v2 `layoutGraph` argument list.

- [ ] **Step 1: Failing store tests** — unit event overlay lands in `liveUnits`; panel round; paused→resumed round-trip; seed-once (a second `activate`-driven rebuild does not move an explicit selection); `select(stepID:unitIndex:)` focuses the unit transcript path.
- [ ] **Step 2:** RED → implement → GREEN (`make macos-test`).
- [ ] **Step 3: Commit** — `feat(macos-graph): live unit/round/pause patching + selection cursor wired to focusStep`

### Task 6: Gates + checkpoint

**Files:**
- Modify: `CLAUDE.md` (RupuRunDetail module line: mention kind-colored live graph)
- Test: full gates

- [ ] **Step 1:** `make macos-test && make macos-build && cargo test -p rupu-cp`. Grep the four re-skinned modules for leftover `RunTone`-era graph code (`kindLabel(for:`, `laneCount`, `unitProgress`) → 0.
- [ ] **Step 2:** `make macos-run` — screenshot a live workflow run (both themes) beside the web run graph for matt's checkpoint; annotate any delta that belongs to Plan 4.
- [ ] **Step 3: Commit** — `feat(macos-graph): run-graph parity — checkpoint evidence + docs`

---

## Self-review notes

- Spec §4 coverage: two-channel + accents/icons→T1/T3; node treatments→T3; edges→T4; state model + live patching→T2/T5; selection + seed + brand-ring highlight→T3/T4/T5; motion→T3/T4; 420pt height→T4.
- Type consistency: `StepKind`/`GraphNodeVM`/`UnitLiveState`/`PanelRoundState`/`GraphSelection` names used identically across tasks; `layoutGraph` v2 signature in T2 matches the T5 call site.
- Known deltas vs web, accepted: no minimap/zoom (native scroll suffices, YAGNI); panelist chips non-interactive until Plan 4; REST unit rows lack `unit_key` so `#index` is the fallback label.
