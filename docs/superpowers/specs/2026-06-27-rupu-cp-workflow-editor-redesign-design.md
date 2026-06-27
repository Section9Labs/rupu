# rupu-cp — Workflow Editor Redesign (unified graph + YAML, live bidirectional) — Design

**Date:** 2026-06-27
**Surfaces:** `rupu-cp/web` (the bulk — the editor surface on `WorkflowDetail`), no new backend (reuses `POST /api/workflows/validate` + the existing validated `saveWorkflow` path)
**Status:** proposed (redesign of the Phase 3c tabbed editor — see `2026-06-27-rupu-cp-phase3c-visual-workflow-editor-design.md`)
**Supersedes (UI only):** the Graph|YAML **tab** layout in `WorkflowDetail.tsx`. Keeps Phase 3c's pure core (`workflowGraph.ts`), its honest DAG semantics, its connection rules, and its validate endpoint **unchanged**.

---

## 0. What's wrong today (the redesign's reason to exist)

The Phase 3c editor split the definition into two **mutually exclusive tabs** — *Graph* and *YAML* — under a read-only "Steps" spine. Three concrete problems:

1. **Tabs hide one half of the truth.** A user editing the graph can't see the YAML it produces, and a user hand-editing YAML loses the graph. There's no "I see what I'm building" moment. The current code even comments that the Graph tab and YAML tab "edit the same `draftYaml`" — but you can only ever see one.
2. **Two visual languages for the same thing.** The editor canvas (`EditableStepNode`, top→bottom `rankdir: 'TB'`) looks *different* from the Runs graph (`StepNode`/`ParallelNode`/`FanoutNode`/`PanelLoopNode`, left→right `rankdir: 'LR'`). A user who learned the run graph has to re-learn the editor.
3. **Plain-text palette + no expression help.** The palette is four colored text buttons (`Step` / `For-each` / `Parallel` / `Panel`). The expression fields (`prompt`, `when`, `for_each`, panel `subject`, …) are bare `<input>`/`<textarea>` — no highlighting, no hint that `{{ steps.x.output }}` is even a thing. The whole supported template vocabulary is invisible.

This redesign collapses the tabs into **one screen** — **editable graph on top, live YAML below** — makes the editor canvas *look like the Runs graph*, replaces the text palette with **draggable node previews**, and gives every expression field **syntax highlighting + context-aware autocomplete** backed by a **discoverable expression reference**.

**Hard constraints carried forward (do not regress):**
- **YAML is the source of truth.** The graph round-trips through `workflowGraph.ts` (`yamlToGraph` / `graphToWorkflowObject`). Positions are cosmetic and never serialized.
- **Writes go through the existing validated `saveWorkflow`.** No new write path; server-side `Workflow::parse` still gates every save.
- **`@xyflow/react` and `@codemirror/*` stay lazy** (out of `index-*.js`). The whole editor remains a `React.lazy` chunk.
- **Pure Tailwind** with the existing tokens (`ink`, `panel`, `brand`, `border`, `bg`, `sev`). No `any` when this becomes code.
- **Honest semantics.** Edge A→B = "B runs after A" (ordering + lets B reference A's output), never a promise of parallelism. Real concurrency = a container node (`parallel`/`for_each`/`panel`). Save = topo-sort → linear `steps:`.

---

## 1. Layout & information architecture

### 1.1 The single screen

The whole `WorkflowDetail` page becomes (top → bottom):

```
┌───────────────────────────────────────────────────────────────────────────┐
│  ‹ Workflows                                                                 │  ← BackLink (unchanged)
│  nightly-review   [project]  [Autoflow]            [✓ valid]  [Delete] [Run] │  ← Header (unchanged actions; validity badge promoted here)
│  Short description line. cron: 0 3 * * *                                     │
├───────────────────────────────────────────────────────────────────────────┤
│  EDITOR SHELL  (full-bleed, one card, ~calc(100vh − headers))               │
│  ┌─────────────────────────────────────────────────────────┬─────────────┐ │
│  │  GRAPH PANE  (top)                                        │ INSPECTOR   │ │
│  │  ┌───────────────────────────────────────────────────┐   │ (right rail)│ │
│  │  │ toolbar: [Fit] [Re-layout] [+ Add ▾]   ⌕   ƒx help │   │ Settings /  │ │
│  │  ├───────────────────────────────────────────────────┤   │ Step form / │ │
│  │  │                                                     │   │ Reference   │ │
│  │  │     ●──▶[ build ]──▶[ ⫶ parallel ]──▶[ panel ↻ ]   │   │             │ │
│  │  │            left → right flow (LR)                   │   │             │ │
│  │  │                                                     │   │             │ │
│  │  │  ┌── palette dock (bottom-left, draggable cards) ─┐ │   │             │ │
│  │  │  │ [step] [for_each] [parallel] [panel]           │ │   │             │ │
│  │  │  └────────────────────────────────────────────────┘ │   │             │ │
│  │  └───────────────────────────────────────────────────┘   │             │ │
│  │ ═══════════════ resizable splitter (drag ↕) ════════════ │             │ │
│  │  YAML PANE  (bottom)                                      │             │ │
│  │  ┌───────────────────────────────────────────────────┐   │             │ │
│  │  │ 1  name: nightly-review                             │   │             │ │
│  │  │ 2  steps:                                           │   │             │ │
│  │  │ 3    - id: build   ...                              │   │             │ │
│  │  │ [⟳ synced from graph]            [Revert] [Save]    │   │             │ │
│  │  └───────────────────────────────────────────────────┘   │             │ │
│  └─────────────────────────────────────────────────────────┴─────────────┘ │
└───────────────────────────────────────────────────────────────────────────┘
```

**Three regions inside one editor shell:**

- **Graph pane (top-left).** The editable LR canvas. Owns its own floating toolbar (Fit / Re-layout / Add menu / search / ƒx help) and a **palette dock** (graphical, draggable node cards) anchored bottom-left over the canvas.
- **YAML pane (bottom-left).** The CodeMirror editor (lazy), always visible, always live. Carries the dirty/sync indicator + Revert/Save. **No more "Edit" toggle** — it's directly editable at all times.
- **Inspector (right rail).** A single right column, full height of the editor shell, with three modes: **Settings** (workflow meta), **Step** (the selected node's form), **Reference** (the expression cheatsheet). Mode follows selection: click a node → Step; click empty canvas → Settings; click the ƒx help → Reference (as an overlay, see §6).

### 1.2 Proportions & resizing

- **Editor shell** fills the viewport below the header: `height: calc(100vh − [header])`, min `640px`. The page stops being `max-w-5xl`; the editor goes **full-bleed** (it needs the room). The header stays constrained for readability.
- **Vertical split (graph ↔ YAML):** a draggable horizontal splitter. Default **62% graph / 38% YAML**. Clamp each pane to a `min-height` (graph `18rem`, YAML `8rem`). The split ratio persists to `localStorage` (`rupu.cp.wfeditor.split`). Two quick affordances on the splitter: **double-click to reset to default**, and a **collapse chevron** to fully collapse the YAML pane (graph-only) or expand it (a "focus YAML" peek). Collapsing YAML does **not** stop sync — it's purely visual.
- **Horizontal split (canvas ↔ inspector):** inspector is a fixed `w-96` (`24rem`) right rail, matching the current `lg:w-96` form panel. On narrow viewports (`< lg`) the inspector drops **below** the editor shell (stacked) rather than overlapping, same responsive instinct as today's `flex-col lg:flex-row`.
- **The read-only "Steps" spine is removed.** It duplicated information the live graph now shows. (If we want a quick textual index, it returns as a collapsed "Outline" disclosure inside the inspector Settings tab — optional, §11.)

### 1.3 Coexistence with the existing header

The header keeps **everything it has** — BackLink, name, `ScopeChip`, `Autoflow` badge, description, trigger line, **Delete**, **Run** (`LauncherSheet`). Two changes:
- The **validity badge** (`✓ valid` / `✕ <reason>`) moves from beside the old tab bar up into the header action row (left of Delete), so it's always visible regardless of which pane has focus.
- **Save / Revert** move *out* of the header area and live on the **YAML pane footer** (they act on the shared draft). One Save button, one path, for both graph and YAML edits — eliminating today's duplicated Save blocks (one in the graph branch, one in the YAML branch).

---

## 2. Wireframes

### (a) Full editor at rest (a node selected is the common case; here nothing selected → Settings)

```
‹ Workflows
nightly-review   [project ▸ acme/api]   [Autoflow]        [✓ valid]   [ Delete ]  [ Run ]
Reviews the nightly build and files findings.   cron: 0 3 * * *
────────────────────────────────────────────────────────────────────────────────────────
┌─ GRAPH ───────────────────────────────────────────────────────────┬─ INSPECTOR ──────┐
│ [⤢ Fit] [�ﮌ Re-layout] [ + Add ▾ ]            ⌕ find step    [ ƒx ] │ [Settings][Step] │
│                                                                     │  [Reference]     │
│                                                                     │ ───────────────  │
│   ●─▶┌───────────┐    ┌────────────────┐    ┌──────────────────┐   │ NAME             │
│      │▎step       │──▶ │▎⫶ parallel  2/2│──▶ │▎panel ↻   gate≥med│   │ [nightly-review] │
│      │ build      │    │ ▸ lint         │    │ 3 panelists      │   │ DESCRIPTION      │
│      │ 🤖 builder │    │ ▸ unit         │    │ subject: PR diff │   │ [ ............ ] │
│      └───────────┘    └────────────────┘    └──────────────────┘   │ PRESERVED KEYS   │
│                                                                     │ trigger  inputs  │
│  ┌ PALETTE ─────────────────────────────────────┐                  │ autoflow         │
│  │ drag onto canvas →                            │                  │ (edit in YAML)   │
│  │ ┌──────┐ ┌────────┐ ┌─────────┐ ┌────────┐    │                  │                  │
│  │ │▎step │ │▎for_each│ │▎parallel│ │▎panel  │    │                  │                  │
│  │ │ 🤖   │ │  ↺ item │ │  ⫶ subs │ │  ↻ gate│    │                  │                  │
│  │ └──────┘ └────────┘ └─────────┘ └────────┘    │                  │                  │
│  └───────────────────────────────────────────────┘                 │                  │
│════════════════════ drag ↕ to resize ═════════════════════════════ │                  │
│─ YAML ─────────────────────────────────────────────────────────────│                  │
│  1  name: nightly-review                                            │                  │
│  2  description: Reviews the nightly build and files findings.      │                  │
│  3  trigger: { on: cron, cron: "0 3 * * *" }                        │                  │
│  4  steps:                                                          │                  │
│  5    - id: build                                                   │                  │
│  6      agent: builder                                              │                  │
│  7      prompt: Build the project and report failures.             │                  │
│  ⟳ synced from graph · last edit 2s ago             [ Revert ] [ Save ]                │
└────────────────────────────────────────────────────────────────────┴─────────────────┘
```

### (b) A node selected → Step form open in the inspector

```
┌─ GRAPH ───────────────────────────────────────────┬─ INSPECTOR ─────────────────────┐
│   ●─▶┌───────────┐    ┌════════════════┐  ← thick   │ [Settings][▣ Step][Reference]   │
│      │ build     │──▶ ║▎panel ↻  ⚠     ║   selected │ ─────────────────────────────── │
│      │ 🤖 builder│    ║ 0 panelists    ║   ring +   │ ⚠ panel needs ≥1 panelist        │
│      └───────────┘    ║ ⌖ subject:—    ║   shadow   │   panel needs a subject          │
│                       ╚════════════════╝            │ ─────────────────────────────── │
│                         ▲ red ⚠ validity badge      │ STEP ID   [ review        ]      │
│                                                     │ KIND      [ panel        ▾]      │
│                                                     │ PANELISTS  ☑ sec  ☐ perf  ☐ …   │
│                                                     │ SUBJECT  (ƒx)                    │
│                                                     │  ┌────────────────────────────┐ │
│                                                     │  │ Diff for {{ inputs.subject │ │
│                                                     │  │  ▏  ┌───────── popup ─────┐ │ │
│                                                     │  │     │ inputs.subject      │ │ │
│                                                     │  │     │ inputs.title        │ │ │
│                                                     │  │     └─────────────────────┘ │ │
│                                                     │  └────────────────────────────┘ │
│                                                     │ PROMPT (ƒx)  [ .............. ]  │
│                                                     │ MAX PARALLEL [ 3 ]               │
│                                                     │ ☑ Enable gate                    │
│                                                     │   until ≥ [ medium ] fix_with[..]│
│                                                     │   max_iterations [ 3 ]           │
│                                                     │ ─────────────────────────────── │
│                                                     │ [ Duplicate ]      [ Delete step]│
└────────────────────────────────────────────────────┴─────────────────────────────────┘
```

### (c) The graphical palette (drag source) — hover/drag states

```
┌ PALETTE  (docked bottom-left over canvas, collapsible ▾) ───────────────────────────┐
│  Drag a card onto the canvas, or click to drop at center.                            │
│  ┌─────────────┐  ┌───────────────┐  ┌────────────────┐  ┌───────────────┐          │
│  │▎▔▔▔▔▔▔▔▔▔   │  │▎▔▔▔▔▔▔▔▔▔▔▔   │  │▎▔▔▔▔▔▔▔▔▔▔▔▔   │  │▎▔▔▔▔▔▔▔▔▔▔▔   │          │
│  │ step        │  │ for_each      │  │ parallel       │  │ panel         │          │
│  │ 🤖 one agent│  │ ↺ over a list │  │ ⫶ N at once    │  │ ↻ review+gate │          │
│  └─────────────┘  └───────────────┘  └────────────────┘  └───────────────┘          │
│     blue bar         violet bar          purple bar          amber bar               │
│                                                                                      │
│  drag ghost  →  ┌─ ─ ─ ─ ─┐   on valid canvas: ⊕ cursor, snap preview               │
│                 ┊▎for_each ┊   over an existing edge: edge highlights = "insert here"│
│                 └─ ─ ─ ─ ─┘                                                          │
└──────────────────────────────────────────────────────────────────────────────────┘
```

Each palette card is a **shrunk, non-interactive instance of the real node card** (same colored top-bar, same glyph/chip), so "what you drag is what you get." Colors mirror `EditableStepNode.KIND_STYLE`: step=blue `#1860f2`, for_each=violet `#8b5cf6`, parallel=purple `#9333ea`, panel=amber `#f59e0b`.

### (d) Expression autocomplete popup in a field (CodeMirror micro-editor)

```
 PROMPT  (ƒx)                                                       [ insert ref ▾ ]
 ┌──────────────────────────────────────────────────────────────────────────────┐
 │ Summarize the build log:                                                       │
 │ {{ steps.bu│ }}                                                                 │
 │           └─┬─────────────────────────────────────────────┐                    │
 │             │ steps.build.output      step output (text)   │  ← highlighted     │
 │             │ steps.build.success     bool — did it pass    │                   │
 │             │ steps.build.skipped     bool — was it skipped │                   │
 │             ├───────────────────────────────────────────────┤                  │
 │             │ ƒ default(…)   filter · fallback value        │  (dim, grouped)   │
 │             └───────────────────────────────────────────────┘                  │
 └──────────────────────────────────────────────────────────────────────────────┘
   ↑↓ navigate · ⏎ insert · esc close · only steps that run BEFORE this one shown
```

Inside `{{ … }}` the text is **token-highlighted** (path segments in brand, filters in green, strings in slate, unknown identifiers underlined amber). The popup is CodeMirror's native `autocompletion` panel themed to match the app.

### (e) Empty state (new / stepless workflow)

```
┌─ GRAPH ───────────────────────────────────────────┬─ INSPECTOR ─────────────────────┐
│ [⤢ Fit] [ﮌ Re-layout] [ + Add ▾ ]      ⌕   [ ƒx ]  │ [▣ Settings][Step][Reference]   │
│                                                    │ NAME  [ my-workflow        ]    │
│                  ╭───────────────────────╮         │ DESCRIPTION [ ............. ]    │
│                  │   ⛶  No steps yet      │         │                                  │
│                  │                       │         │  A workflow needs a name and    │
│                  │  Drag a node from the │         │  at least one step to run.      │
│                  │  palette below, or    │         │                                  │
│                  │  [ + Add first step ] │         │                                  │
│                  ╰───────────────────────╯         │                                  │
│  ┌ PALETTE ─────────────────────────────────────┐ │                                  │
│  │ ┌─────┐ ┌────────┐ ┌─────────┐ ┌──────┐       │ │                                  │
│  │ │step │ │for_each│ │parallel │ │panel │  drag→│ │                                  │
│  │ └─────┘ └────────┘ └─────────┘ └──────┘       │ │                                  │
│  └───────────────────────────────────────────────┘│                                  │
│════════════════════════════════════════════════════│                                  │
│─ YAML ─────────────────────────────────────────────│                                  │
│  1  name: my-workflow                              │                                  │
│  2  steps: []                                      │                                  │
│  (valid skeleton — Save enabled once a step is added & complete)                       │
└────────────────────────────────────────────────────┴─────────────────────────────────┘
```

---

## 3. The editable graph (matches the Runs graph)

### 3.1 Reuse the run-graph LR visual language

**Switch the editor layout to LR** to match Runs. In `workflowLayout.ts`, change `rankdir: 'TB'` → `rankdir: 'LR'` and adopt the run-graph spacing (`nodesep: 36, ranksep: 72` from `graphLayout.ts`). Handles move from Top/Bottom to **Left (target) / Right (source)** in the editable node — exactly like `StepNode`/`ParallelNode`/etc.

**Node card anatomy** — converge the editable node visuals onto the run-graph cards. Rather than the current minimal `EditableStepNode` (kind chip + id + one summary line), each editable node renders the **same body** as its read-only run-graph twin, minus run state:

| Editor kind | Mirrors | Card content (edit mode) |
|---|---|---|
| `step` | `StepNode` | colored top-bar (blue) · id · `step` chip · agent chip |
| `for_each` | `FanoutNode` (pending/slim form) | violet bar · `for_each · {id}` · the `for_each:` expr · `max_parallel` chip |
| `parallel` | `ParallelNode` | purple bordered container · `parallel · {id}` · stacked **sub-step rows** (id list) |
| `panel` | `PanelLoopNode` | amber/violet container · `panel · {id}` · gate block (`gate ≥ sev · max N`) · panelist count |

Because the run nodes color by *run state* (`STATE_STYLE`) and the editor has no run state, the editor uses a neutral **"design state"** palette (the `KIND_STYLE` bars already in `EditableStepNode`) for the top-bar/border, and shows **content** (agent, expr, sub-steps, gate) where the run card would show progress. This keeps them visually sibling — same silhouette, same color identity per kind — without faking run status. *(Implementation note: factor the run cards' presentational shell into a shared `nodes/cards/*` so both Runs and the editor import the same JSX skeleton; the editor passes `editable` + content props, Runs passes run state. Lower-effort fallback: keep the current `EditableStepNode` but restyle it to LR handles + the richer per-kind bodies above.)*

### 3.2 Node states in edit mode

- **Default:** `border-border`, `shadow-card`, white bg (containers tinted per `ParallelNode`/`PanelLoopNode`).
- **Hover:** subtle `ring-1 ring-brand-100` + reveal the inline **"+ add next"** affordance on the right handle (see 3.4).
- **Selected:** `ring-2 ring-brand-500` + elevated `shadow` (the run graph uses `selected`; we make it prominent because selection drives the inspector).
- **Validity badge:** reuse the existing red dot from `EditableStepNode` (`problemsById[id]` non-empty → `bg-red-500` dot with the joined problems as `title`), upgraded to a small **⚠ pill** in the top-bar corner so it's legible at a glance and announces via `aria-label`. Clean nodes show nothing.
- **Drag:** node follows cursor; positions are cosmetic (React state only), per Phase 3c.

### 3.3 Connection UX (drag from handle)

Unchanged engine (`canConnect` + React Flow `isValidConnection` + `applyConnect`), better feedback:
- **Drag from a node's right (source) handle.** Valid drop targets' left handles **light up green** (`#2ac769`) as the drag approaches; the connection line is brand-colored.
- **Invalid target** → the line turns **red**, the target handle shows a **no-drop** cursor, and on release a transient inline reason banner appears (reusing the existing `connError` amber banner, but anchored near the drop with the specific `canConnect` reason: "This would create a cycle — steps must form a DAG." / "These steps are already connected." / "A step can't depend on itself."). The banner auto-dismisses after ~4s and is also dismissible.
- **Delete an edge:** select it → Backspace/Delete (existing `onEdgesChange`), or hover an edge → a small **✕** midpoint button.

### 3.4 Inline add (the discoverable build path)

Two new ways to grow the graph beyond the palette:
- **"+ next" on a node:** hovering a node reveals a `⊕` button on its right edge. Click → a tiny kind-picker popover (the four palette cards, small) → drops a new node to the right and **auto-connects** the current node → new node. The new node is selected (inspector → Step). This is the primary fast path for linear authoring.
- **Drop-on-edge to insert:** dragging a palette card over an existing edge highlights that edge ("insert here"); dropping splits A→B into A→new→B (rewires both edges; validated through `canConnect` — always legal since it's a linear insert). 

### 3.5 Palette → canvas drag interaction

- The palette dock holds four **mini node cards** (drag sources). HTML5 drag (or pointer-based DnD) carries the `StepKind`. On drop over the canvas, project the drop point to flow coords (`screenToFlowPosition`) and call a new `applyAddNodeAt(graph, kind, pos)` (generalizes the existing `applyAddNode`, which currently hard-codes a stacked position). New node is selected.
- **Click-to-add** still works: clicking a palette card (no drag) drops at canvas center — accessible fallback and the keyboard path.
- The existing toolbar **"+ Add ▾"** menu lists the same four kinds for users who don't find the dock.

### 3.6 Re-layout

Keep the **Re-layout** button (re-runs dagre LR, tidies positions). Auto-layout still runs once on load and on a *foreign* YAML reseed (see §4). Manual drags are kept during the session; not persisted across reloads (Phase 3c decision, unchanged). **Fit view** button re-frames (`fitView`).

---

## 4. Live bidirectional sync (the heart of the redesign)

### 4.1 The model: YAML draft is the single shared state; graph is a projection

One piece of truth lives on the page: **`draftYaml: string`** (already exists in `WorkflowDetail`). Everything is derived from / writes back to it.

```
                       ┌──────────── draftYaml (string) ────────────┐
                       │   (page-level state; the shared draft)      │
                       └───────▲──────────────────────────▲─────────┘
       graph edit ───────┐     │                          │     ┌─────── YAML edit
   commit(graph) →       │     │ (parse on change,        │     │  CodeMirror onChange →
   graphToWorkflowObject │     │  debounced 250ms)        │     │  setDraftYaml(text)
   → yaml.dump  ─────────┘     │                          └─────┘
                               ▼
                        yaml.load(draftYaml) → yamlToGraph → autoLayout(once)
                               │
                               ▼  graph projection (nodes/edges) fed to the canvas
```

Two directions, each with a guard against the echo:

**Graph → YAML.** A graph mutation calls `commit(nextGraph)` (already implemented in `WorkflowEditor`): `graphToWorkflowObject` → `yaml.dump` → `setDraftYaml`. This already exists and works. The YAML pane (CodeMirror) is a controlled view of `draftYaml`, so it updates instantly. Cursor preservation in CM is handled by its existing external-value diff (`CodeEditorImpl` only dispatches a change when `value !== current`).

**YAML → Graph.** New. When the user types in the YAML pane, `setDraftYaml(text)` fires. A **debounced (250ms) parser** turns `text` into a graph: `yaml.load` → `yamlToGraph`. The current `WorkflowEditor` only reseeds the graph when `initialYaml` *identity* changes from a "foreign" source (its `lastSeenYaml` ref trick). We extend that: the page distinguishes **the source of a `draftYaml` change**:
  - change originated from the **graph** (we just dumped it) → do **not** reseed the canvas (would clobber positions / fight the user). Recognized via the existing `lastSeenYaml.current === incoming` echo check.
  - change originated from the **YAML editor** → **reconcile** the canvas (see 4.3).

### 4.2 Debounce, dirty, validity

- **Parse debounce:** 250ms after the last YAML keystroke (snappy but not per-char). The existing **validity** call is already debounced 400ms server-side; keep it.
- **Dirty:** `draftYaml !== detail.yaml` (unchanged). Shown on the YAML pane footer (`⟳ synced from graph` when graph-driven; `● unsaved changes` when dirty).
- **Save gating:** `saving || !dirty || validity?.ok === false` (unchanged logic, now in one place).

### 4.3 Reconciliation that doesn't nuke the graph

The danger: a user mid-edit in YAML produces **transiently invalid** text ("steps:\n  - id: bu" while typing). We must **not** blow the canvas away to an empty graph on every intermediate keystroke. Rules:

1. **Parse failure (yaml.load throws or returns non-object):** keep the **last good graph** on screen, dim it slightly (`opacity-60`), and show a non-blocking **"YAML not parseable — graph paused"** chip on the canvas. The YAML pane shows the parser/validity error inline. Nothing is destroyed; the moment the YAML parses again, the graph un-dims and reconciles.
2. **Parse success:** diff the new graph against the on-screen graph by **node id**:
   - ids unchanged, only field values changed → **patch node data in place**, keep existing positions. No relayout. (The common case: editing a prompt in YAML updates the node's summary without the canvas jumping.)
   - node ids added/removed, or edges changed → reconcile structure: keep positions for surviving ids, assign new nodes a dagre position (or place near their topo neighbor), drop removed ones. **Do not full-relayout** unless the user hits Re-layout (avoids the canvas "jumping" while typing). 
3. **Selection preservation:** if the selected node's id still exists after reconcile, keep it selected (the inspector stays on that node's form). If it was renamed *in YAML*, we can't track identity reliably → deselect and toast "selected step changed in YAML." (Renames done *in the Step form* already re-point selection — existing `onStepChange` handles this.)
4. **Cursor preservation:** the YAML editor never gets its value reset while the user is the source of truth (the echo guard ensures graph→YAML round-trips don't re-dispatch identical text). When graph edits *do* rewrite YAML, CM's minimal-change dispatch keeps the cursor as stable as possible; we accept that a structural rewrite (canonical re-dump) may move the cursor — this only happens on graph edits, when the user's attention is on the canvas, not the text.

### 4.4 Conflict handling (graph + YAML "simultaneously")

True simultaneity is impossible (one keyboard, one pointer); the real case is **rapid alternation**. The echo guard + debounce make this deterministic: the **last committed source wins**, and because both directions funnel through the same `draftYaml`, there's no divergent state to merge. The one explicit guard: while a YAML parse is **pending/failed**, graph interactions on the (paused, dimmed) canvas are **disabled** (`nodesDraggable={false}`, palette disabled) — you can't drag a node built from stale text. Re-enabled the instant YAML parses.

### 4.5 The canonical-rewrite caveat (carried forward, surfaced better)

Saving from the graph (or any reconcile that re-dumps) rewrites YAML canonically — **comments and custom formatting are lost**. Today this is a paragraph of fine print. In the redesign:
- The YAML pane shows a small **`⟳ synced from graph`** badge whenever the current text was machine-generated, vs **`✎ hand-edited`** when the user has typed since the last graph commit.
- A first-time **"editing the graph will reformat your YAML (comments removed)"** confirm, remembered in `localStorage`, fired only the first time a user with comment-bearing YAML touches the graph. Honest, once.

---

## 5. Node settings + expression editing

### 5.1 Keep the forms, upgrade the expression fields

The per-kind forms (`StepForm` + sub-forms, `WorkflowSettingsForm`) are good and **stay structurally** — same fields, same `patch()` immutability, same `raw_passthrough` preservation, same validity `problems` block. The redesign changes **which control renders the expression-bearing fields**.

**Expression fields** (get the rich editor): `step/for_each` **prompt**, **when**, **for_each** expression; **parallel sub-step** prompt; **panel** subject and prompt; panel gate **fix_with** is an agent select (no expression). Plain fields (ids, numbers, checkboxes, agent selects, panelist checkboxes) stay as today.

**New control: `<ExpressionField>`** — a tiny single-purpose CodeMirror instance (lazy, reusing the `@codemirror/*` chunk already loaded for the YAML pane), themed to look like the existing `fieldCls` input (`rounded-md border border-border … focus:border-brand-500`). It provides:
- **Syntax highlighting** of minijinja inside the value: `{{ … }}` / `{% … %}` regions highlighted; inside them, `inputs`/`steps`/`item`/`loop`/`event`/`issue` roots in **brand**, sub-paths in `ink`, filters after `|` in **green**, string literals in slate, **unknown identifiers underlined amber**.
- **Autocomplete** (CodeMirror `autocompletion` with a custom source) — see 5.2.
- A small **ƒx** affordance in the field's top-right corner that opens the **Reference** overlay (§6) anchored to that field, and an **"insert ref ▾"** quick menu of the most common refs for the current context.
- A **live validity hint** under the field for the most common mistake — a `steps.x` reference to a step that runs later or doesn't exist (we already compute this in `validateGraph` / `extractStepRefs`); render it inline as amber text, not just as a node dot.

`ExpressionField` is a drop-in for the current `<textarea>`/`<input>` (same `value` / `onChange(string|undefined)` contract), so `StepForm`'s wiring barely changes.

### 5.2 Context-aware autocomplete (the vocabulary, exactly)

A pure function **`completionsFor(ctx) → Completion[]`** (unit-tested, no CM dependency in the pure part) produces suggestions from the **verified vocabulary**, scoped by where the field lives. `ctx` carries: the field kind (`prompt`/`when`/`for_each`/`subject`/`sub_prompt`), the **owning node** (its kind + id), the **declared workflow inputs** (from `meta.rest.inputs` keys), the **trigger kind** (cron/event/issue, from `meta.rest.trigger`), and the **set of step ids that topo-sort BEFORE this node** (from `topoSort` + the node's position).

| Group | Offered when | Items |
|---|---|---|
| **Inputs** | always (if any declared) | `inputs.<name>` for each declared input |
| **Earlier steps** | always | for each step id **before** this node (topo order): `steps.<id>.output`, `.success`, `.skipped` |
| **for_each results** | the earlier step is a `for_each`/`parallel` | `steps.<id>.results[*]`; parallel adds `steps.<id>.sub_results.<sub_id>.output` / `.success` |
| **panel findings** | the earlier step is a `panel` | `steps.<id>.findings[*]` (`{source,severity,title,body}`), `.max_severity`, `.iterations`, `.resolved` |
| **Loop locals** | **only** in a `for_each` node's **prompt** | `item`, `loop.index`, `loop.index0`, `loop.length`, `loop.first`, `loop.last` |
| **Panel subject** | **only** in a panel's prompt/subject | `inputs.subject` surfaced first (plus all inputs) |
| **Event / Issue** | trigger is event/issue | `event.*`; `issue.number/title/body/labels/author/state` |
| **Functions** | always (dim, grouped last) | `read_file('path')` |
| **Filters** | after a `|` | `length, join, default, tojson, map, select, first, last, upper, lower, trim, sort, reverse` |
| **when truthiness hint** | `when` field only | inline note: "falsy = false / 0 / '' / no / off" |

Each completion carries a **label**, a **one-line doc** (shown in the CM info panel), and an **insert template** (e.g. `default(${1:value})` with a tab-stop; `findings[*]` inserts the bracket). The dropdown groups with subtle section headers (CM `section`), most-relevant first (Earlier steps and Inputs above functions/filters).

**Crucially context-aware (per the prompt):** `item`/`loop.*` appear *only* in a for_each prompt; `steps.<id>` only offers ids that run **before** the current node (so you can't autocomplete a forward reference — matching `validateGraph`'s forward-ref rule); panel `inputs.subject` only in panel prompts.

### 5.3 Live validity badge (per field + per node + global)

Three tiers, all already computable:
- **Field:** the `ExpressionField` inline hint (unknown/forward `steps.x`).
- **Node:** the `problemsById[id]` ⚠ pill on the card + the red alert block atop the Step form (both exist).
- **Global:** the header `✓ valid / ✕` from the server validate endpoint (exists).

---

## 6. Expression reference design

A **discoverable, searchable reference** of the whole vocabulary, surfaced two ways:
1. **Inline (autocomplete):** the popup *is* the reference in-flow (label + doc + grouped).
2. **The Reference panel:** a third inspector tab **"Reference"** (and the **ƒx** button in any expression field opens it as a focused overlay anchored to the field). Layout:

```
┌ REFERENCE ─────────────────────────────────┐
│ ⌕ [ find an expression…            ]        │
│ ─ Inputs ─────────────────────────────────  │
│   inputs.<name>            value of an input │  ← click = insert at cursor
│ ─ Steps (earlier only) ───────────────────   │
│   steps.<id>.output        step's text output│
│   steps.<id>.success       did it pass (bool)│
│   steps.<id>.skipped       was it skipped     │
│ ─ For-each / parallel ────────────────────    │
│   steps.<id>.results[*]    per-item results   │
│   item, loop.index, loop.first, …  (for_each) │
│ ─ Panel ─────────────────────────────────     │
│   steps.<id>.findings[*]   {source,severity…} │
│   .max_severity .iterations .resolved         │
│ ─ Event / Issue ─────────────────────────     │
│   event.*    issue.number / .title / .labels  │
│ ─ Functions & filters ───────────────────     │
│   read_file('path')                           │
│   | default · join · length · map · …         │
│ ─ when truthiness ───────────────────────     │
│   falsy = false / 0 / "" / no / off           │
└─────────────────────────────────────────────┘
```

- **Grouped** by the §5.2 groups; **searchable** (filters the list as you type).
- **Click-to-insert** at the active expression field's cursor (the overlay remembers which field opened it). Greyed groups (e.g. loop locals when not in a for_each) show with a tooltip "available in for_each prompts" rather than vanishing — teaches the model.
- Backed by **one data module** `lib/workflowExpressions.ts` (the vocabulary as typed data: `{ group, label, doc, insert, availableWhen }[]`) — the *single source* feeding both the autocomplete source and this panel, so they never drift.

---

## 7. Interaction flows

### (a) Build a new workflow from scratch
1. User opens a new/empty workflow → **empty state** (§2e); inspector on **Settings**; YAML shows `name: …\nsteps: []`.
2. Types a **name** + description in Settings → graph→YAML keeps `steps: []`; validity badge `✓ valid` (a stepless workflow parses).
3. **Drags a `step` card** from the palette onto the canvas → node `step-1` appears, auto-selected → inspector flips to **Step**.
4. Picks an **agent** (dropdown), types a **prompt** with autocomplete (`{{ inputs.` → suggests declared inputs). Node's red ⚠ clears as agent+prompt fill in.
5. Hovers `step-1`, clicks **⊕ next**, picks **panel** → `step-2` (panel) drops to the right, auto-connected `step-1 → step-2`. Adds panelists, a subject (autocomplete offers `inputs.subject`, and `steps.step-1.output`).
6. YAML pane has been **live-updating** the whole time; user glances down, sees correct YAML, header shows `✓ valid`, footer Save enables → **Save** → `saveWorkflow`.

### (b) Add a parallel review panel to an existing workflow
1. Open an existing linear workflow (`build → deploy`). Graph shows two LR cards; YAML below.
2. **Drag a `parallel` card onto the edge** between `build` and `deploy` → edge highlights "insert here" → drop splits into `build → review → deploy` (auto-rewired, all valid). `review` selected → Step form.
3. In the Step form, **Add sub-step** twice → `lint` / `security`, each with an agent + prompt. The parallel card on canvas now shows two stacked sub-rows (mirrors `ParallelNode`).
4. In `deploy`'s prompt, type `{{ steps.rev` → autocomplete offers `steps.review.success` (review now runs before deploy). Insert it; the field highlights the path.
5. YAML updates live (`parallel:` block inserted in order); `✓ valid` → **Save**.

### (c) Wire a step's prompt to an earlier step's output via autocomplete
1. Select a downstream step → Step form → focus the **prompt** `ExpressionField`.
2. Type `{{ ` → popup opens grouped: **Inputs**, **Earlier steps** (only ids topo-before this node), Functions, Filters.
3. Type `steps.bu` → narrows to `steps.build.output | .success | .skipped` with docs.
4. ⏎ on `steps.build.output` → inserts `{{ steps.build.output }}`, path highlighted brand. An **edge `build → <this>`** appears automatically (the data-ref edge — `yamlToGraph` already adds `steps.X → Y` edges; here the round-trip re-derives it, so the wire shows up after the next reconcile). Node ⚠ stays clear (reference is valid + backward).
5. If the user instead referenced a step that runs *later*, the field shows the amber inline hint "references steps.x which runs later" and the node gets the ⚠ pill — before Save, before the server ever sees it.

---

## 8. States & edge cases

- **Loading:** the editor shell shows a skeleton (graph area shimmer + 6 YAML line placeholders) while `getWorkflow` resolves; the lazy editor chunk shows "Loading editor…" (existing `Suspense` fallback). Agents load best-effort (existing).
- **Invalid YAML (parse error):** graph **pauses** (dimmed, last-good kept), canvas chip "YAML not parseable — graph paused", inline error in the YAML pane, Save disabled. No data loss. (§4.3)
- **Empty graph:** the §2e empty state in the canvas; `steps: []` is valid YAML so the validity badge is green but Save stays disabled until there's a complete step (a stepless workflow can't usefully run — gate Save on "≥1 step" too, with a hint).
- **A step shape the editor can't model:** Phase 3c already preserves unmodeled step keys via `raw_passthrough` and unmodeled top-level keys via `meta.rest`. The redesign **surfaces** them: such a node renders with a small **"⊕ extra config"** marker and the Step form shows a read-only "Unmodeled keys (edit in YAML): contract, …" chip row (like `WorkflowSettingsForm`'s preserved-keys block). Never dropped; never silently editable-into-loss.
- **Very large workflows:** the canvas already has MiniMap + Controls + Fit. Add the toolbar **⌕ find step** (filter/zoom-to a node by id) for big graphs. The YAML pane is virtualized by CodeMirror. Reconcile diffs by id (no full relayout on type) so big graphs don't thrash. dagre handles dozens of nodes fine; beyond that the MiniMap + find carry navigation.
- **Save / dirty / validity gating:** Save enabled iff `dirty && validity.ok && !saving && hasAtLeastOneStep`. Revert restores `detail.yaml` (existing). Navigating away while dirty → a `beforeunload`/route guard confirm (new, small).
- **Rename collisions:** duplicate step ids already flagged by `validateGraph` ("duplicate step id") → node ⚠ + Save blocked. No change needed beyond surfacing.
- **Server reseed after Save:** on successful save the server returns canonical YAML; `setDraftYaml(updated.yaml)` reseeds — the graph reconciles (ids stable → positions kept).

---

## 9. Visual design notes

- **Tokens (no new colors):** surfaces `bg`/`panel`/`border`; text `ink`/`ink-dim`/`ink-mute`; primary `brand-600` (Save, selection ring, active tab); kind accents reuse the established per-kind hues (step blue `#1860f2`, for_each violet `#8b5cf6`, parallel purple `#9333ea`, panel amber `#f59e0b` — already in `EditableStepNode.KIND_STYLE`); validity green `#2ac769` / red `#fb4e4e` / amber `#f59e0b` from `stepStyle.ts`. Severity scale `sev.*` only where panel severities show.
- **Spacing/typography:** match existing — node ids `text-[12px] font-semibold`, chips `text-[10px]`, form labels `text-[12px] font-semibold uppercase tracking-wide text-ink-dim`, fields `text-[13px]`. Cards `rounded-[10px]`/containers `rounded-[12px]`, `shadow-card`, `border-border`. The shell card `rounded-xl border border-border`.
- **Canvas:** Background dots `gap={16} color="#e2e8f0"`, `style={{ background: '#fafafa' }}` — identical to RunGraph so the editor *reads as the same surface*. Reuse the run-graph edge marker (`MarkerType.ArrowClosed`) and `smoothstep` edges (the editor currently uses default edges; switch to `smoothstep` to match Runs).
- **Motion/feedback:** connection-line color transitions (green valid / red invalid); palette drag ghost at `opacity-60`; node select ring animates in (`transition`); reconcile patches are instant (no layout animation, to avoid jumpiness while typing); the "graph paused" dim is a 150ms fade. Reuse `rg-pulse-*` only in Runs — the editor stays still (no run state to pulse).
- **Accessibility:**
  - **Canvas keyboard:** Tab cycles nodes (React Flow focusable nodes); Enter selects (opens Step form); Delete removes; `⊕ next` reachable via a per-node "Add next step" button with `aria-label`. The **palette cards are buttons** (click-to-add) so the whole build flow works without drag. A documented note that drag is an enhancement, click is the baseline.
  - **Autocomplete:** CodeMirror's autocomplete is ARIA-complete (listbox + `aria-activedescendant`); ↑↓/⏎/esc as in (d). The `ExpressionField` gets an `aria-label` from its form label.
  - **Splitter:** `role="separator"` `aria-orientation="horizontal"` with arrow-key resize and `aria-valuenow` (the %).
  - **Inspector tabs:** `role="tablist"`/`tab`/`tabpanel`, arrow-key navigation (the current PanelTabButton becomes a proper tablist).
  - **Validity:** node ⚠ and field hints use `aria-label`/`role="alert"` (the form's problem block already does `role="alert"`).
  - **Color is never the only signal:** kind shown by chip *text* too; validity by ⚠ glyph + text, not just red.

---

## 10. Component plan

### 10.1 Reused unchanged (the load-bearing core — do not touch behavior)
- `lib/workflowGraph.ts` — `yamlToGraph`, `graphToWorkflowObject`, `topoSort`, `canConnect`, `validateGraph`, `extractStepRefs`, all types. **Untouched.**
- `lib/api.ts` — `getWorkflow`, `getAgents`, `saveWorkflow`, `validateWorkflow`. **Untouched.**
- `components/CodeEditor*` — the lazy CodeMirror wrapper. Reused for the YAML pane; its `@codemirror/*` chunk is also reused by `ExpressionField` (no new heavy dep).
- Run-graph node shells (`graph/StepNode` etc.) — referenced for visual parity; ideally factored into a shared presentational skeleton (see 3.1), else mirrored.

### 10.2 Modified
- **`pages/WorkflowDetail.tsx`** — **biggest change.** Remove the Graph|YAML **tabs**, the read-only **Steps spine**, the `view` state, and the **duplicated Save blocks / Edit toggle**. New body = the **`<WorkflowEditorShell>`** (the unified layout). Header keeps name/scope/autoflow/Delete/Run; validity badge moves into the header row; Save/Revert move into the shell's YAML footer. The page still owns `draftYaml`, `detail`, `agents`, `validity`, `save`, `remove`.
- **`components/workflow-editor/WorkflowEditor.tsx`** → evolve into **`WorkflowEditorShell.tsx`**: hosts the **resizable vertical split** (graph pane / YAML pane) + the **right inspector**. Replaces the current `flex-col lg:flex-row` (canvas + aside) with the three-region layout. Keeps `commit`, `seedGraph`, the echo-guard (`lastSeenYaml`), `problemsById`. **Adds** the YAML→graph reconcile (§4.3) and owns the split ratio.
- **`components/workflow-editor/WorkflowEditorGraph.tsx`** — switch to **LR** (handles + edges `smoothstep`), add the **graphical palette dock** (drag sources), **drop-on-canvas** (`screenToFlowPosition` + `applyAddNodeAt`), **drop-on-edge insert**, **inline ⊕-next**, connection green/red feedback, **find-step**. Keeps `applyConnect`/`applyDelete`/`applyAddNode` (generalize `applyAddNode` → `applyAddNodeAt`).
- **`components/workflow-editor/nodes/EditableStepNode.tsx`** — LR handles (Left target / Right source), richer per-kind bodies (sub-step rows for parallel, gate block for panel, for_each expr) to match the run cards; ⚠ pill upgrade; selected ring; hover ⊕.
- **`lib/workflowLayout.ts`** — `rankdir: 'TB'` → **`'LR'`**, spacing to match `graphLayout.ts`.
- **`components/workflow-editor/StepForm.tsx`** — swap expression `<input>/<textarea>` for **`<ExpressionField>`** (prompt/when/for_each/subject/sub-prompt); surface unmodeled-keys chip row. Plain fields unchanged.

### 10.3 New
- **`components/workflow-editor/ExpressionField.tsx`** — the small CodeMirror expression input (highlight + autocomplete + ƒx + inline validity). Lazy, reuses the CM chunk.
- **`lib/workflowExpressions.ts`** — the **typed vocabulary** (§6) + **`completionsFor(ctx)`** pure function (§5.2). Heavily unit-tested.
- **`components/workflow-editor/ExpressionReference.tsx`** — the searchable, grouped, click-to-insert Reference panel/overlay.
- **`components/workflow-editor/Palette.tsx`** — the graphical, draggable node-card dock (+ click-to-add fallback).
- **`components/workflow-editor/SplitPane.tsx`** — the accessible resizable horizontal splitter (or a tiny dependency-free implementation).
- **`components/workflow-editor/InspectorTabs.tsx`** — the `role="tablist"` Settings / Step / Reference switcher.
- (optional) **`components/graph/cards/*`** — shared presentational node skeletons used by both Runs and the editor (the parity refactor).

### 10.4 Backend
- **None.** `POST /api/workflows/validate` and the `saveWorkflow` path already exist (Phase 3c). The redesign is entirely `web/`.

### 10.5 What changes from the current tab-based implementation (call-outs)
1. **Tabs removed** → one screen, graph+YAML co-visible.
2. **YAML is always editable** (no Edit toggle); **Save/Revert unified** to one place (was duplicated per tab).
3. **YAML→graph is now live** (was: graph reseeds only on foreign identity change; now reconciles as you type, §4.3).
4. **Editor canvas re-skinned to the Runs graph** (LR, run-style cards, smoothstep edges) — was a distinct TB look.
5. **Palette is graphical drag-source cards** (was text buttons); **inline ⊕-next** and **drop-on-edge** added.
6. **Expression fields get highlighting + context-aware autocomplete + a Reference** (was bare inputs).
7. **Read-only Steps spine removed** (the live graph replaces it).

### 10.6 Phased build order (→ becomes the implementation plan)
- **Phase 1 — Unify the shell (no new capability).** `WorkflowEditorShell` with the resizable graph/YAML split + inspector; remove tabs/spine/Edit-toggle from `WorkflowDetail`; unified Save/Revert; validity in header. *Graph→YAML already works; this just co-locates it.* Ship-able on its own.
- **Phase 2 — Live YAML→graph reconcile (§4.3).** Debounced parse, id-diff patch, pause-on-invalid, selection preservation. The "bidirectional" promise.
- **Phase 3 — Runs-graph parity.** LR layout, run-style editable cards, smoothstep edges, selected/hover/⚠ states. (Optional shared `cards/*` refactor here.)
- **Phase 4 — Graphical palette + richer canvas authoring.** Draggable palette cards, drop-on-canvas/-edge, inline ⊕-next, connection green/red feedback, find-step.
- **Phase 5 — Expression intelligence.** `workflowExpressions.ts` + `completionsFor`, `ExpressionField` (highlight + autocomplete), wire into `StepForm`.
- **Phase 6 — Reference & polish.** `ExpressionReference` panel/overlay + ƒx, unmodeled-keys surfacing, a11y pass (tablist/separator/keyboard), the first-time reformat confirm, dirty-navigation guard.

Each phase is independently mergeable and visually validatable (matt runs the binary — `make cp-web` before any release per repo memory).

---

## 11. Open questions / tradeoffs

> **Decisions (matt, 2026-06-27):** (1) **Top/bottom locked** for v1 (graph top, YAML below), resizable splitter. (2) **Accept canonical YAML rewrite** + `⟳ synced` badge (comment loss carried from 3c). (3) Never auto-relayout while typing; relayout only on explicit Re-layout. (4) **Mirror the node visuals** in the editor as separate components with shared style tokens — do NOT refactor the Runs cards (zero Runs-page risk). (5) **CodeMirror `ExpressionField`**, mounted lazily for the focused field. (6) Autocomplete offers input **names** only in v1. (7) **Gate Save on ≥1 complete step**. (8) **No position persistence** — auto-layout on load, session-only positions. **Build cadence: 6 phased PRs, validate the binary between each.**

1. **Default split ratio & graph-on-top vs. side-by-side.** The brief says graph-top/YAML-below. An alternative for wide monitors is **graph-left / YAML-right**. Proposal: ship top/bottom (matches the brief + reading order), keep the split orientation as a later toggle. *Decide:* lock top/bottom for v1?
2. **Canonical YAML rewrite (comment loss).** Inherent to a structural editor. Options: (a) accept + the `⟳ synced` badge + one-time confirm (proposed); (b) attempt comment-preserving emit (much harder, `js-yaml` can't). *Decide:* is (a) acceptable for v1? (Phase 3c already accepted it.)
3. **Reconcile aggressiveness on YAML typing.** Patch-in-place vs. occasional relayout. Proposal: never auto-relayout while typing; only on explicit Re-layout. *Decide:* OK, or should adding a node via YAML auto-place it via dagre immediately?
4. **Shared node-card refactor (10.1/3.1).** Factor run cards into a shared skeleton (clean, but touches Runs — needs matt's GUI re-validation of the *Runs* page) vs. mirror the visuals in the editor (no Runs risk, mild duplication). *Decide:* refactor or mirror for v1?
5. **`ExpressionField` = CodeMirror vs. a lighter highlighter.** CM gives real autocomplete/highlight but a CM-per-field has a mount cost on big panel forms. Alternative: a contenteditable + regex highlighter + a custom popup (lighter, more code to own). Proposal: CM (chunk already loaded), mount lazily only for the *focused* field, render others as styled static text until focus. *Decide:* acceptable?
6. **Scope of autocomplete inputs.** Declared `inputs` live in `meta.rest.inputs` (untyped on the wire). We can offer names reliably; offering input *types* (to filter filters) is more work. Proposal: names only in v1.
7. **Empty/stepless Save.** Gate Save on ≥1 step (proposed) vs. allow saving a skeleton. *Decide.*
8. **Persist node positions?** Phase 3c said no (auto-layout on load). Live YAML editing strengthens the case for *session-only* positions (already proposed) but still no cross-reload persistence. *Decide:* keep no-persistence for v1?

---

## Appendix — verified expression vocabulary (the autocomplete/Reference data source)

```
inputs.<name>
steps.<id>.output | .success | .skipped
for_each / parallel: steps.<id>.results[*]
parallel:           steps.<id>.sub_results.<sub_id>.output | .success
panel:              steps.<id>.findings[*] {source,severity,title,body}
                    steps.<id>.max_severity | .iterations | .resolved
for_each PROMPT only: item · loop.index · loop.index0 · loop.length · loop.first · loop.last
event-triggered:    event.*
issue-target:       issue.number | .title | .body | .labels | .author | .state
function:           read_file('path')
filters:            length join default tojson map select first last upper lower trim sort reverse
when truthiness:    falsy = false | 0 | "" | no | off
context rules:      item/loop.* ONLY in for_each prompt · steps.<id> ONLY for steps topo-before this node
                    inputs.subject surfaced first in panel prompts
```
This table is encoded once in `lib/workflowExpressions.ts` and feeds both the in-field autocomplete and the Reference panel — single source, no drift.
```
