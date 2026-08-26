# rupu.app Perf Arc — Plan 6: Graph Interactivity & Live-Effect Correctness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the run graph the web's direct manipulation — wheel/pinch zoom, drag-pan, double-click zoom, fit-on-open, zoom controls, minimap — plus smoothstep-curved kind-colored edges and animation-stable live effects; and fix the fan-out transcript experience by porting the web's per-unit transcript browser (matt 2026-08-26: graphs need to be dramatically better; for_each unit transcripts "not working").

**Architecture:** The graph canvas moves into an `NSScrollView`-backed representable with `allowsMagnification` (native trackpad pinch + ⌘-wheel zoom, momentum pan) hosting the existing SwiftUI node/edge content via `NSHostingView`; zoom bounds mirror React Flow defaults (0.5–2.0) with fit-on-open ≤1.0. Animation stability rides Plan 5's coalesced store-side `graphVM`: node/edge views keyed by stable step ids so live rebuilds diff instead of remount, keeping pulse/ants animations alive.

**Tech Stack:** SwiftUI + AppKit (NSViewRepresentable), Swift 6, Swift Testing. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-25-rupu-macos-perf-interaction-design.md` §2 · Web reference: `crates/rupu-cp/web/src/components/RunGraph.tsx` (React Flow defaults: zoomOnScroll, pinch, panOnDrag, zoomOnDoubleClick, fitView `{padding:0.2, maxZoom:1.0}`, Controls zoom-in/out/fit, minZoom 0.5 / maxZoom 2 defaults; MiniMap DEFERRED per spec).

## Global Constraints

- Plan 5 is merged first (store-side coalesced `graphVM`, `LazyHStack` canvas, static caches). If not, STOP.
- Two-channel color rule and all Plan 3 node/edge visuals unchanged — this plan changes the CANVAS and identity/animation stability, not node anatomy.
- Live effects stay STATE-derived (web parity; never connection-derived). Reduced-motion guards intact.
- Test hygiene rules as Plan 5. Gates per task: `make macos-test` + `make macos-build`. NEVER bare `git stash pop`.

## File structure

```
RupuKit/Sources/RupuRunDetail/Graph/ZoomableGraphCanvas.swift  # NEW: NSScrollView representable
RupuKit/Sources/RupuRunDetail/StepGraphView.swift              # hosts content in the canvas + controls overlay
RupuKit/Sources/RupuRunDetail/Graph/{StepNodeCard,ContainerNodes,GraphEdge}.swift  # identity/animation stability only
RupuKit/Sources/RupuRunDetail/RunDetailScreen.swift            # non-content states stop reserving 420pt
```

---

### Task 1: ZoomableGraphCanvas

**Files:**
- Create: `Graph/ZoomableGraphCanvas.swift`
- Test: `RupuRunDetailTests/ZoomableGraphCanvasTests.swift` (pure logic only: zoom clamping, fit-scale computation)

**Interfaces:**
- `ZoomableGraphCanvas<Content: View>: NSViewRepresentable` — wraps an `NSScrollView`: `allowsMagnification = true`, `minMagnification = 0.5`, `maxMagnification = 2.0`, document = `NSHostingView(rootView: content)`; drag-pan on empty canvas (scroll view's natural pan; content must not swallow drags), pinch + scroll-wheel zoom (`scrollWheel` with magnification default behavior — verify trackpad pinch works via `allowsMagnification` alone; add a `magnify(with:)` passthrough if the hosting view intercepts), double-click → zoom-in step (1.25×, clamped) via a click gesture recognizer on the scroll view (NOT on nodes — node taps must still select; double-click on a NODE is a node concern, ignore it).
- `func fitToContent(maxScale: CGFloat = 1.0, padding: CGFloat = 0.2)` — computes magnification to fit the document with 20% padding, capped at 1.0; called once on first content-size availability (fit-on-open) and from the Fit control. Pure helper `fitScale(container:content:padding:maxScale:) -> CGFloat` is unit-tested.
- Exposes `magnification: Binding<CGFloat>`-ish control surface (a small `ZoomController` observable: `zoomIn()`, `zoomOut()` (1.25× steps, clamped), `fit()`, `current`).
- [ ] **Step 1:** TDD `fitScale` + clamping math → implement the representable → `make macos-build` green.
- [ ] **Step 2: Commit** — `feat(macos-graph): ZoomableGraphCanvas — pinch/wheel zoom, drag pan, fit math`

### Task 2: Canvas integration + controls

**Files:**
- Modify: `StepGraphView.swift` (content moves inside `ZoomableGraphCanvas`; horizontal `ScrollView` removed — the scroll view IS the pan surface; selection taps verified to still land)
- Modify: `RunDetailScreen.swift` (graph section keeps 420pt for content; loading/empty/failed states render compact (~120pt) — the parked Plan 3 minor, promoted by the spec)
- Controls overlay: bottom-left vertical pill group — zoom-in / zoom-out / fit (`Icon` glyphs, ghost buttons, panel chrome) mirroring the web's three-button Controls (`showInteractive=false` equivalence).
- Test: existing suites stay green; controls logic (clamp sequence) covered via ZoomController tests.

- [ ] **Step 1:** Integrate; verify node tap → selection and fan-out unit taps still work through the hosting view (manual-logic check: gestures attach to content, canvas pans on empty space).
- [ ] **Step 2:** Full suite + build. **Commit** — `feat(macos-graph): zoomable canvas integration + zoom controls, compact non-content states`

### Task 3: Animation stability + open-on-running verification

**Files:**
- Modify: `Graph/{StepNodeCard,ContainerNodes,GraphEdge}.swift`, `StepGraphView.swift`
- Test: `RupuRunDetailTests` — identity stability (rebuilding graphVM with one state change produces same ids in same order); store-level test that opening a store against an already-running run's replayed events yields `.running` liveStates BEFORE any new event arrives (the byte-0 replay path)

**The problem:** per-event VM array rebuilds can remount node/edge views (SwiftUI identity loss), restarting or killing repeatForever animations — likely why matt saw no live effects. With Plan 5's coalesced `graphVM`:
- `ForEach` over nodes/edges keyed by stable ids (`node.id`; edge id = `"\(source.id)->\(target.id)"` — make `GraphEdge`'s inputs identity-stable, not positional).
- Node cards and `GraphEdge` become `Equatable` (all-value-type fields) so unchanged nodes skip body re-evaluation entirely; animation state (`@State isPulsing`, ants phase) survives because the VIEW identity survives.
- The ants/pulse restart discipline (onChange(of: state)) still handles genuine state transitions.
- Verify + test the replay path: `RunDetailStore.activate()` on a run whose stream replays historical `stepStarted` events must show `.running` in liveStates promptly (this already works by design — the test pins it so it can't regress; if it does NOT work, that's the root cause of matt's "no live effects" and gets fixed here).
- [ ] **Step 1:** TDD identity/replay tests → implement Equatable + stable keys → GREEN.
- [ ] **Step 2:** Full suite + build. **Commit** — `fix(macos-graph): stable node/edge identity — live pulse/ants survive live updates`

### Task 4: Curved edges, minimap, layout polish (added 2026-08-26 — the "1000x better" pass)

**Files:** `Graph/GraphEdge.swift`, `StepGraphView.swift`, new `Graph/GraphMinimap.swift`; test: pure path-geometry tests for the smoothstep curve.

- **Smoothstep edges:** replace the straight 40pt stick with a web-parity smoothstep bezier connecting the SOURCE node's trailing anchor to the TARGET node's leading anchor at each node's true vertical midline (nodes have different heights — containers vs leaf cards — so edges currently miss centers). Color/ghost/ants/awaiting rules unchanged; ants march along the curved path (`strokeDashPhase` on the bezier). Pure helper `smoothstepPath(from:to:) -> Path` unit-tested (endpoints, monotonic x, curvature bounds).
- **Minimap:** a small (≈160×64) inset overview bottom-right inside the canvas — scaled node rectangles in kind accents + a viewport rectangle; click-to-jump pans the scroll view. Un-deferred from the spec per matt's feedback; spec note updated at close-out.
- **Layout polish:** consistent vertical centering of the chain on the canvas midline; node spacing from the 4px grid (24pt gaps); container cards get matching vertical rhythm; canvas background gets the web's subtle dot grid (Canvas-drawn, cheap, reduced-motion-irrelevant).
- [ ] **Step 1:** TDD the path helper → implement edges + minimap + polish → gates green. **Commit** — `feat(macos-graph): smoothstep edges, minimap, layout polish`

### Task 5: Fan-out unit transcript browser (added 2026-08-26 — matt: for_each transcripts "not working")

**Files:** new `RupuRunDetail/StepTranscriptBrowser.swift`; modify `RunDetailTabs.swift` (Transcript tab three-way), `RupuStore/RunDetailStore.swift` (unit-list projection if needed); tests in `RupuRunDetailTests` + store tests.

- **Root-cause first:** trace why unit transcripts don't load today — instrument `select(stepID:unitIndex:)` → `focusPath` with a failing-path test against fixture unit rows (candidate causes: unit `transcriptPath` resolution/relative-path handling, remote-host guard, the tab panel not switching to the focused unit transcript, or fan-out unit taps not registering through the container gesture). Fix the actual defect with a regression test; document the found cause in the report.
- **Port the web's `StepTranscriptBrowser`** (`components/run/StepTranscriptBrowser.tsx`): when the SELECTED node has a fanout, the Transcript tab renders a two-pane browser — left rail (≈34% width): step id header, unit count, `all/running/done/failed` filter pills WITH counts, per-unit rows (state glyph square + mono key + uppercase state label), row cap 300 + "+N more"; selected row `rupuBrand`-tinted; right pane: the existing TranscriptFeed for the selected unit's path, live-tailing while that unit is running (this closes the parked "units never tail" gap — route the unit path through the same tail machinery focusStep uses, gated on unit state == running, local runs only as today). Selecting a unit in the rail = the same `select(stepID:unitIndex:)` cursor the graph's unit squares drive — one selection model, two entry points.
- [ ] **Step 1:** failing root-cause test → fix → browser TDD (rail filter counts, cap, selection sync) → gates green. **Commit** — `feat(macos-rundetail): fan-out unit transcript browser + unit transcript loading fix`

### Task 6: Gates + checkpoint

- [ ] **Step 1:** Full gates (`make macos-test && make macos-build && cargo test -p rupu-cp`); CLAUDE.md graph note (zoomable canvas).
- [ ] **Step 2:** matt's GUI checkpoint items recorded in the PR: pinch/wheel zoom, drag pan, double-click zoom, fit button, pulse/ants visible when opening an already-running run and across its updates. **Commit** — `feat(macos-graph): Plan 6 close-out`

---

## Self-review notes

- Spec §2 coverage: zoom/pan/fit/controls→T1/T2; compact non-content states→T2; animation stability + replay verification→T3; minimap deferred per spec.
- Order: T1→T2→T3 (canvas before identity work so animation tests run in the final container).
- Known risk: NSHostingView gesture interplay (pan vs node taps) — T2 explicitly verifies; if pan-vs-tap conflicts, restrict pan to empty-canvas hit-test (documented fallback).
