# rupu Slice D — Native macOS App Design

**Date:** 2026-05-11
**Status:** Draft
**Companion docs:** [Slice A design](./2026-05-01-rupu-slice-a-design.md), [Slice B-1 multi-provider](./2026-05-02-rupu-slice-b1-multi-provider-design.md), [Slice B-2 SCM](./2026-05-03-rupu-slice-b2-scm-design.md), [Slice C TUI](./2026-05-05-rupu-slice-c-tui-design.md), [Autoflow v1](./2026-05-08-rupu-autoflow-design.md), [Autoflow Observability](./2026-05-11-rupu-autoflow-observability-design.md)

---

## 1. What this is

A native macOS desktop app — `rupu.app` — that becomes the developer's primary surface for orchestrating agents, workflows, and autoflows. Built in pure Rust on [GPUI](https://www.gpui.rs/) (the framework Zed uses), Metal-accelerated, single-process.

The app does not wrap the CLI. It links the same library crates the CLI links (`rupu-orchestrator`, `rupu-agent`, `rupu-scm`, `rupu-mcp`, `rupu-tools`, `rupu-transcript`) and runs workflows in-process. The CLI keeps working unchanged because the libs don't know about it; it shifts to a headless-executor + MCP-server role for CI / servers / scripted contexts.

Three product personas in the same shell:
- **IDE for agentic work.** Open a workspace, see your workflows / agents / autoflows / runs / repos / issues in one place, edit any of them in YAML or via a visual canvas, run them, watch them stream.
- **Production console.** Watch live + historical runs across one or many workspaces, approve / reject gates from the desktop, drill into per-step transcripts.
- **Authoring surface.** Drag-and-drop canvas to compose workflows and autoflows; agent file editor with frontmatter UI; autoflow contracts derived from the node graph.

---

## 2. Why now

Slice C shipped a TUI canvas that proves the visual language — vertical git-graph layout, status glyphs, per-node drill-down, approval prompts — is the right way to read a workflow run. The CLI is now the right tool for headless contexts (CI, autoflow workers) but the wrong tool for interactive day-to-day work:

- The TUI canvas is constrained by terminal width; real workflows have 8-30 nodes with multi-level fan-out and run for tens of minutes.
- The transcript-on-stdout pattern doesn't survive context switches (open another terminal, scroll back, lose position).
- Multi-window observability (watching workspace A while editing workspace B) has no story in the CLI.
- Editing workflows means hand-writing YAML; there's no visual aid for fan-out shape, panel composition, or autoflow contracts.
- Repos / issues / agents are CLI-table views; they want a richer surface.

We have a complete library stack (orchestrator, agent runner, SCM, MCP, transcript) and a proven visual language. The missing piece is the surface that puts them together. **A native Rust + Metal app is the right answer**: same process, microsecond event latency, native macOS feel, no Electron tax, and an architecture that maps cleanly onto the library layout already in place.

---

## 3. Design principles

1. **Library-shared, not CLI-shelled.** The app calls the same `run_workflow()` the CLI does. No subprocess, no JSONL-over-stdio, no IPC fence between UI and orchestrator. CLI and app are peers, not parent/child.

2. **YAML is canonical.** The workflow / agent / autoflow YAML is the only source. The canvas derives layout from topology; no sidecar layout file, no inline position comments, no canvas-only state. Anything you can do in the canvas must round-trip through the YAML cleanly.

3. **One window per workspace.** A workspace = a directory + a per-user manifest. Workspaces are independent; cross-workspace awareness happens via a menubar badge, not a global sidebar.

4. **View, don't lock.** Each pane has a view-picker (Graph / Canvas / Transcript / YAML). Splits hold different views of the same artifact simultaneously. The user never has to choose between "see the graph" and "see the source" — they can have both at once.

5. **Animation as signal.** Motion (pulse, ring, stream-dash, fresh-stripe) is used only where it carries information: live nodes, active edges, newly-arrived events. No decorative animation.

6. **Light footprint on the project tree.** Workspace metadata lives in `~/Library/Application Support/rupu.app/`; the project directory only carries the existing `<dir>/.rupu/` conventions. Adding rupu.app to a project should require zero file changes inside the repo.

7. **macOS first, Mac only for v1.** GPUI runs on macOS, Linux, Windows; we target macOS exclusively for v1 so we don't pay the cross-platform tax before we know the product is good. Other platforms are explicitly out of scope.

---

## 4. Architecture

### 4.1 Orchestrator coupling

v1 ships a single `WorkflowExecutor` implementation: `InProcessExecutor`. It owns a `tokio::runtime::Runtime`, calls `rupu_orchestrator::run_workflow()` directly, and emits events through the `EventSink` trait.

```rust
// rupu-orchestrator: new traits
pub trait WorkflowExecutor: Send + Sync {
    async fn start(&self, opts: WorkflowRunOpts) -> Result<RunHandle, ExecutorError>;
    fn list_runs(&self, filter: RunFilter) -> Vec<RunRecord>;
    fn tail(&self, run_id: &str) -> EventStream;
    async fn approve(&self, run_id: &str, approver: &str) -> Result<(), ExecutorError>;
    async fn reject(&self, run_id: &str, reason: &str) -> Result<(), ExecutorError>;
    async fn cancel(&self, run_id: &str) -> Result<(), ExecutorError>;
}

pub trait EventSink: Send + Sync {
    fn emit(&self, run_id: &str, ev: &Event);
}
```

The current JSONL writer becomes one `EventSink` impl (`JsonlSink`) and stays as the persistence layer. The app subscribes to an `InMemorySink` (a `tokio::broadcast` channel) for live UI updates with microsecond latency. Both sinks run in parallel for every run; persistence and live-stream are independent concerns.

v2+: a `RemoteExecutor` impl speaks MCP to a `rupu mcp serve` host on another machine. Same trait, different backend. The app talks to a `Box<dyn WorkflowExecutor>` and doesn't care which it is. Out of scope for v1 except: the trait shape must accommodate it.

Autoflow's worker layer (the thing that picks up an autoflow claim and runs the corresponding workflow) becomes a consumer of `WorkflowExecutor` — same execution path as the desktop app, same observability hooks. The CLI's `cmd/run.rs` and `cmd/workflow.rs` port to it too over time so there's a single execution surface across all three callers.

### 4.2 Crate layout

New crates:
- **`rupu-app`** — the GPUI desktop binary. Depends on rupu-orchestrator, rupu-agent, rupu-scm, rupu-mcp, rupu-tools, rupu-transcript, rupu-config, rupu-auth. Owns the window shell, menubar, command palette, workspace persistence.
- **`rupu-app-canvas`** — pure-Rust view widgets (Graph layout, Canvas layout, Transcript filtering, YAML schema-aware highlighting). Independent of GPUI: takes a `&CanvasModel`, returns a render description. Snapshot-testable without booting the UI.

Existing crates touched:
- **`rupu-orchestrator`** — add `WorkflowExecutor` + `EventSink` traits, `InProcessExecutor` impl, `JsonlSink` (refactor of current writer), `InMemorySink` (broadcast).
- **`rupu-cli`** — refactor `cmd/run.rs` and `cmd/workflow.rs` to consume `WorkflowExecutor` so CLI and app share the same execution surface.
- **`rupu-agent`** — no changes expected; the runner already takes an event writer.

Workspace deps added at root `Cargo.toml`: `gpui` (workspace fork or git pin while pre-1.0).

### 4.3 Process / threading

The app runs on a single GPUI main thread + a tokio runtime for orchestrator work. GPUI's actor model handles cross-pane state updates without locks; the executor's broadcast channel feeds the UI's event loop via a small adapter (`UiEventBridge`) that converts `Event` into pane-targeted UI updates.

No web view. No subprocess. No native panels embedded.

---

## 5. Workspace model

### 5.1 Workspace = directory + manifest

A workspace binds a project directory to per-user state:

- **Directory** — the project's filesystem root. `<dir>/.rupu/agents/*.md`, `<dir>/.rupu/workflows/*.yaml`, `<dir>/.rupu/autoflows/*.yaml` are auto-discovered (same convention `rupu` CLI uses today).
- **Manifest** — `~/Library/Application Support/rupu.app/workspaces/<workspace-id>.toml`. Holds name, color, attached repo refs, attached rupu hosts, UI state (last-open tabs, collapsed sections), recent-runs cache.

Workspace IDs are ULIDs prefixed with `ws_`, matching the existing `run_*` ID convention so logs / cross-references read uniformly.

```toml
# ~/Library/Application Support/rupu.app/workspaces/ws_01H8X.toml
id = "ws_01H8X..."
name = "rupu"
color = "purple"
path = "/Users/matt/Code/Oracle/rupu"
opened_at = "2026-05-11T15:00:00Z"

[[repos]]
ref = "github:Section9Labs/rupu"

[[attached_hosts]]
kind = "local"
# v2+:
# kind = "mcp"
# url = "mcp://server.internal:8080"
# auth_key = "mcp-host-internal"

[ui]
last_open_tabs = ["workflow:review", "file:src/lib.rs"]
sidebar_collapsed_sections = ["issues"]
active_view_per_tab = { "workflow:review" = "Graph" }
```

The split is deliberate: the project directory stays clean and shareable (no `.rupu.app/` metadata to gitignore), and per-user state persists across `git clean -fdx`.

### 5.2 Asset discovery

On workspace open, the app populates the sidebar from three sources:

| Section | Project (per workspace dir) | Global (per user) |
|---|---|---|
| Workflows | `<dir>/.rupu/workflows/*.yaml` | `~/.rupu/workflows/*.yaml` |
| Agents | `<dir>/.rupu/agents/*.md` | `~/.rupu/agents/*.md` |
| Autoflows | `<dir>/.rupu/autoflows/*.yaml` | `~/.rupu/autoflows/*.yaml` |
| Repos | manifest `[[repos]]` (rupu-scm connectors) | n/a |
| Issues | per-repo via `IssueConnector` | n/a |
| Runs | `<dir>/.rupu/runs/*` + `~/.rupu/runs/*` | unified, newest-first |

Each section header shows `project ▾ N` / `global ▾ M` subsection counts when both are non-empty.

### 5.3 Workspace lifecycle

- **Create:** `File > New Workspace…` opens a modal: pick a directory (create if needed), optionally attach repos via `RepoRef`, optionally pick a starter template (lifts from `rupu init --with-samples`).
- **Open:** `File > Open Workspace…` browses for a directory; if no matching manifest exists, creates an empty one on the fly.
- **Recent:** `File > Open Recent` shows last 10 workspaces with their color chip + last-opened timestamp.
- **Close:** `Cmd+W` closes the workspace window; if runs are in-flight, modal asks confirm. In-flight runs continue (they're in-process, not subprocess) — closing the window detaches the UI but the run keeps writing to disk.

---

## 6. UI architecture

### 6.1 Window

One window per workspace. Workspace windows are independent; closing one doesn't affect others. Three regions:

```
┌──────────────────────────────────────────────────────────────────────┐
│ titlebar — workspace color chip + name + this-window in-flight count │
├───────┬──────────────────────────────────────────────────────────────┤
│       │ ── tab strip per pane ─────────────────────────────────────  │
│       │                                                              │
│ side  │  pane                                                        │
│ bar   │  ──────────────────────────────────────────────              │
│ (180) │  view picker · live indicator · split chevron                │
│       │  ──────────────────────────────────────────────              │
│       │  active view (Graph / Canvas / Transcript / YAML)            │
│       │  ──────────────────────────────────────────────              │
│       │  drill-down (per-pane, collapsible)                          │
└───────┴──────────────────────────────────────────────────────────────┘
```

The titlebar count is **this workspace only**. The system menubar (section 8.7) carries the cross-workspace count. So an operator with one workspace open sees the same number in both places; with multiple workspaces open the titlebar shows the local count and the menubar aggregates.

Menubar item (system-wide, not per-window): cross-workspace runs badge. Icon shows total in-flight runs across all open workspaces. Click → dropdown listing each in-flight run with its workspace name + active step; clicking a row focuses that workspace window. The badge is the "I can be deep in workspace A and notice workspace B hit an approval gate" affordance.

### 6.2 Sidebar (180px, minimal accordion)

Single sidebar, never swaps modes. Sections are always all reachable; collapsible by clicking the header. Minimal visual chrome:

- Tiny uppercase labels (10px, letter-spacing 0.14em, slate-500 color)
- No section backgrounds, no borders, no boxed headers
- Counts appear only when a section is collapsed (faint number next to caret)
- Active items are the only highlight: 2px purple left-border + subtle bg tint
- 180px width (canvas gets the rest)

Sections (in fixed order): `workflows` · `runs` · `repos` · `agents` · `issues`. Order matches the workflow-IDE mental model: what to run → what ran → what's it touching → who runs it → what triggers it.

Bottom of sidebar: workspace switcher dropdown (current workspace name + color, clicking opens recent workspaces + "New Workspace…"), command palette icon (⌘K), settings.

### 6.3 Tabs

Each tab represents one viewable artifact:

| Tab kind | What's in it | Default view |
|---|---|---|
| Workflow | a `.yaml` workflow file | Graph |
| Run | one execution of a workflow (live or historical) | Graph |
| File | a source file from a repo | (code editor) |
| Issue | one issue from an issue tracker | (issue layout) |
| Agent | a `.md` agent file | (frontmatter form + body editor) |
| Repo | repo dashboard | (repo summary) |
| Autoflow | a `.yaml` autoflow | Canvas |

Tabs persist across sidebar interaction. Tab right-click menu: close, close-others, close-all, split-right, split-down, pin, open-in-new-window. Drag a tab to the edge of a pane → split. Drag a tab out of the window → new window.

### 6.4 Panes

Standard Zed-style pane tree. A workspace window starts with one pane occupying the entire main area; the user splits via `⌘\` (vertical) / `⌘k ⌘\` (horizontal), drag-to-edge, or context menu. Splits are recursive — splits can be split.

Each pane owns: its own tab strip, the active tab's view, the view picker (when applicable), and a per-pane drill-down at the bottom. Drilling into a node in pane A doesn't affect pane B.

### 6.5 View picker

Inside a pane (workflow / run / autoflow tab), a 4-button segmented control above the canvas:

**Graph | Canvas | Transcript | YAML**

Behavior:
- Pane remembers the picked view per tab (set on the active tab when switched).
- Different panes can show different views of the same artifact (Graph in the left pane, Transcript in the right pane is the canonical "watching a long run" layout).
- View picker is not shown for File / Issue / Agent / Repo tabs.

### 6.6 Drill-down pane

Bottom of each main-area pane. Per-pane. Collapsible (default expanded for Run tabs, collapsed for Workflow/Agent edit tabs). Inner tabs: `tools` · `transcript` · `findings` · `agent`. The live thinking spinner anchors at the top of the drill-down when a node is in flight.

Collapsed state: a one-line strip showing the focused node's name + event count, with a chevron to expand.

---

## 7. Views

Four views, all available v1.5; **Graph ships in v1** (the others are sub-slices). The view-picker UI ships in v1 so the inactive buttons are visible but greyed out — users get a roadmap, not a missing-feature surprise.

### 7.1 Graph view (v1 default)

The Slice C TUI canvas, lifted into GPUI. Vertical git-graph spine, ASCII-style glyphs (●, │, ├, ╭, ╰, ◄, ◐, ✓, ✗, ⏸, ⊘) rendered in a code-editor monospace typeface. Status colors from the existing palette.

Layout: same auto-layout algorithm as Slice C (`derive_edges` + `Canvas auto-layout`), ported to GPUI's coordinate model. Topology determines positions; no manual placement.

Live behavior:
- Status glyphs update as `Event::*` arrives via the broadcast sink.
- Active node's glyph pulses (`dot-pulse` animation).
- Awaiting-approval nodes show the `⏸` glyph with `fresh-stripe` highlight on transition.

Why Graph is v1: it lifts proven Slice C code, gives an immediate visual on day-one, and rhymes typographically with the YAML view (same monospace). Canvas (graphical) requires significantly more layout work and lands as v1.5.

### 7.2 Canvas view (v1.5)

Horizontal left-to-right flow, Okesu-shaped (see reference screenshot at `okesu.to/screenshots/orchestration-editor.webp`). Card nodes with:
- Agent icon (colored circle, color = step status)
- Step ID (bold, small uppercase, letter-spaced)
- Status glyph (right side)
- Prompt preview (3 lines max, truncated with ellipsis, dim gray)
- Meta line (agent · duration · tokens, smaller, dimmer)
- 3px colored left-border = status

**Fan-out (panel / parallel / for_each) renders as one grouped card**, NOT parallel branches:
- Group label uppercase: `PANEL · review_panel · 3 panelists`
- Each unit is a chip inside the card: status glyph + name + meta
- Active chip has subtle blue border + glow
- Skipped chips fade to 60% opacity

Edges between top-level nodes:
- Solid 1.5px line, slate gray for idle
- Stream-dash animation when feeding the active node
- Dashed 2px for pending edges (not yet reached)

Annotations:
- `GATED` badge (small pill, amber) on approval-gate nodes
- Live indicator pill in top-right of canvas when run is in-flight (pulsing dot + "live · Xs elapsed")

Editing affordances (when the tab is in edit mode — Canvas tab for a workflow, not a Run tab):
- `+ Agent` button in canvas header opens a slide-out right panel: agent palette (drag source rows, one per available agent — both project and global)
- Drag agent → drop on canvas → inserts new step into YAML at the topology-appropriate position based on drop coords
- Click a node → focuses; drill-down loads with that node's events (in Run tabs) or its prompt + meta (in Workflow tabs)
- Right-click a node → context menu: reorder, change-kind (linear ↔ panel ↔ parallel ↔ for_each), delete, edit-prompt
- Drag a node by its handle → reorder its YAML position (drop above/below another step)
- No free-floating drag of node coordinates: positions are always derived from YAML topology

Floating overlays:
- Bottom-left: zoom controls (+ / − / fit-to-screen / lock)
- Bottom-right: minimap (rectangle per node, colored by status, viewport rectangle outline)

### 7.3 Transcript view (v1.5)

Chronological event stream, lifted from Okesu's `EventTimeline`. Each row:
- Timestamp (mono, dim)
- Source step / panelist (small, dim)
- Event kind (assistant / tool / finding / error / gate / etc.)
- Content (one line, expandable on click)

Filter chips above the stream: `all · assistant · tools · findings · errors · gates`. Multi-select.

Animations:
- `timeline-enter` (380ms cubic-bezier spring) on freshly-arrived rows
- `fresh-stripe` (purple left-edge fading over 2.4s) on rows arrived in the last few seconds
- `dot-pulse` on the live-indicator dot for the currently-streaming step

The Transcript view's filter state is per-pane. Two panes side-by-side, Graph on left + Transcript on right with `tools + errors` filter, is the canonical "babysit a long autoflow" layout.

### 7.4 YAML view

Schema-aware text editor:
- tree-sitter YAML grammar for syntax highlighting
- Validator powered by `rupu-orchestrator::workflow::Workflow::parse` — errors render as inline diagnostics with the same message the CLI would print
- Completion driven by the workflow / agent schema (step kinds, known agent names from the workspace, tool names)
- Hover over a step → preview the corresponding node in the Graph/Canvas (via a small popover)

Save behavior: writes back on `⌘S` or on tab blur (matches Zed). The file watcher in the parent crate picks up the disk change and the canvas re-lays out automatically; the YAML pane and Graph/Canvas panes stay in sync without an explicit reload step.

YAML view is what `rupu init`-created sample files look like when opened. It's the "view source" mode the Okesu canvas reference promotes via its Visual/YAML toggle — in rupu.app it's first-class as one of the four views.

---

## 8. Surfaces

### 8.1 Launcher (workflow run)

Triggered by `⌘R` or right-click → Run on a workflow in the sidebar. Floating sheet:
- Workflow name + description (from YAML)
- Inputs form: one row per declared `inputs:` field with type-appropriate widget (text, select for `enum`, checkbox for bool, number for int). Required fields highlighted; defaults pre-filled.
- Mode picker: `Ask · Bypass · Read-only`
- Target picker: current workspace directory (default), pick another directory, or paste a `RepoRef` (clones to temp dir under the hood, mirrors `rupu run --tmp`)
- Run button → opens a new Run tab, switches to Graph view, drill-down auto-focuses the first running node

### 8.2 Agent editor

Tab view for a `.md` agent file. Split vertically:
- **Top: frontmatter form** — name (text), provider (select), model (select, populated from the chosen provider's known models), tools allowlist (checkboxes per known tool), system_prompt mode (select: ask / bypass / readonly defaults).
- **Bottom: markdown body editor** — the system prompt, rendered with syntax highlighting for the markdown fences inside.

YAML toggle in the header reveals raw frontmatter as YAML; round-trips to the form.

### 8.3 Autoflow editor

Extends the workflow editor with an **autoflow contracts panel** below the Canvas. Each node can declare structured outputs (JSON Schema). Schemas are auto-generated from:
- The node graph topology (which fields downstream nodes consume from upstream outputs)
- The agent's declared output shape (if any, from agent frontmatter)

Manual override: user can edit the schema directly in the contracts panel; warnings if the override contradicts what downstream nodes need.

### 8.4 Repos panel (sidebar mode)

When `repos` is the active sidebar section: file tree of the active repo with git status decorations (M / A / D / U). Sub-sections:
- File tree (full directory listing, scrollable)
- Git status (modified files, list)
- Open PRs (lifted from `rupu-scm::IssueConnector::list_prs` once that exists; v1 falls back to "open in browser" link)

Click a file → opens in a File tab in the active pane.

### 8.5 Issues panel (sidebar mode)

When `issues` is the active sidebar section: list view (default) with filter chips by state, label, assignee. Filter chips colored by GitHub's hex (already supported in `rupu-cli::output::tables::label_chips_with_colors`).

Click an issue → opens an Issue tab with the issue body, comments, and a "Linked runs" sub-section (runs filtered by `--issue <ref>` from the CLI's existing flag).

"Run workflow against this issue" button on the issue header → opens the Launcher pre-populated with `--input issue=<ref>` plus the workflow's standard inputs.

### 8.6 Workspace creation flow

`File > New Workspace…` modal:
1. Pick a directory (create if it doesn't exist)
2. Optionally attach repo refs (one or more, validated against `rupu-scm` connectors)
3. Optionally select a starter workflow template (the same templates `rupu init --with-samples` emits)
4. Workspace opens, ready to use

If matt opens a directory that already has `.rupu/` content, the workspace gets created with that content auto-discovered and no template choice offered.

### 8.7 Cross-workspace menubar badge

Always-on macOS menubar item. Icon shows total in-flight runs across all open workspaces (`0` shows as a quiet dot; `>0` shows the count with the brand purple). Click → dropdown listing each in-flight run:
- `[workspace color chip] workspace-name · workflow · active step · elapsed`
- Click a row → focuses the workspace window + brings the relevant Run tab to front
- Section below: recent completions (last 5, dim)

The badge is the cross-workspace observability piece — it's what makes multi-window-per-workspace tolerable for "I'm working on A but want to know when B's autoflow finishes."

---

## 9. Animation vocabulary

Lifted directly from Okesu's `web/src/styles.css`. Five primitives, named the same as in Okesu for cross-reference:

| Name | Duration | Curve | Where |
|---|---|---|---|
| `dot-pulse` | 1.4s loop | ease-in-out, scale 1.0 → 1.18 → 1.0 | Live indicators (status dots on active nodes, top-right run badge) |
| `ring-expand` | 1.1s loop | ease-out, scale 1 → 3.2, opacity 0.55 → 0 | Under live dots — radar-ping feel |
| `stream-dash` | 800ms linear infinite | `stroke-dashoffset` 0 → -16 | Edges feeding an active node (Canvas view) |
| `fresh-stripe` | 2.4s | ease-out, opacity 1 → 0 | 2px purple left-edge on freshly-arrived transcript rows |
| `timeline-enter` | 380ms | cubic-bezier(0.16, 1, 0.3, 1) spring | Row slide-in for new transcript events |

All animations are GPU-accelerated via GPUI's Metal-backed painting. None are decorative — each carries information (this node is live; this edge is carrying I/O; this row just landed). No fade-ins on static content, no hover bounces, no loading shimmer.

---

## 10. Sub-slice ordering

The work decomposes into ten sub-slices. Each is its own plan doc and ships independently. Order is dependency-driven; the user-visible "this is a real product" line is crossed at D-4.

| # | Slice | Ships |
|---|---|---|
| **D-1** | Workspace shell | One window per workspace, sidebar accordion, menubar badge stub, workspace manifest + persistence. No tab content. |
| **D-2** | Graph view widget | Lifts Slice C canvas auto-layout into `rupu-app-canvas` and renders in a GPUI pane. No live data — reads a static `WorkflowSpec` and lays it out. |
| **D-3** | Run viewer | `WorkflowExecutor` + `EventSink` traits land in `rupu-orchestrator`. `InProcessExecutor` + `InMemorySink` + `JsonlSink` impls. App subscribes to runs; Graph view comes alive. Drill-down pane works. Approve / reject from desktop. |
| **D-4** | Launcher | Workflow inputs form, run button, new tab opens streaming. **Operator-complete: rupu.app is now self-sufficient for "open workspace → run workflow → watch + approve".** |
| **D-5** | YAML view + source mode | Schema-aware YAML editor; side-by-side splits between Graph and YAML; live re-layout on save. |
| **D-6** | Canvas view + editor | Okesu-shaped horizontal canvas; drag-from-palette inserts YAML; reorder / change-kind / delete via right-click; floating zoom + minimap. |
| **D-7** | Agent editor | Frontmatter form + markdown body editor for `.md` agent files. |
| **D-8** | Transcript view | Chronological event stream with filter chips; `timeline-enter` / `fresh-stripe` / `dot-pulse` animations. |
| **D-9** | Repos + Issues panels | Connector-backed file tree, git status, open PRs (Repos); list + filter + linked-runs view (Issues). |
| **D-10** | Autoflow editor + remote executor + polish | Autoflow contracts panel below Canvas (auto-generated from node graph); `RemoteExecutor` over MCP; preferences / theme toggle; app signing + notarization for distribution. |

Each slice is shippable on its own. D-1 through D-4 = Operator-grade v1 (~3-4 months). D-5 through D-8 = Editor v1.5 (~3 months). D-9, D-10 = polish + remote (~2-3 months). Rough total: 8-10 months for one engineer focused, less for two.

---

## 11. Testing strategy

- **Unit tests in `rupu-app-canvas`** — view layout and rendering logic, no GPUI dep. `insta` snapshots for Graph layout, Canvas card placement, Transcript row formatting.
- **Integration tests in `rupu-orchestrator`** — `InProcessExecutor` against mock providers (existing pattern from the orchestrator's lib tests); `InMemorySink` broadcast subscription.
- **GPUI smoke tests** — headless boot of the app via GPUI's test harness; open a fixture workspace; assert no panics in the first 5 seconds; assert tab + sidebar render. Modeled after Slice C's `tui-smoke` make target.
- **Manual / pre-release** — matt drives UX validation. No substitute for actually using it.
- **Visual regression** (post-v1) — once D-6 lands the graphical Canvas, snapshot the rendered canvas to PNG and diff against golden frames. Defer to v1.5 polish slice.

---

## 12. Error handling

- **Workspace open fails** (missing directory, permissions): modal asks to relocate or remove from recents.
- **Missing connector credential** (`github` / `gitlab` / etc. not logged in): sidebar shows the section but rows fail-soft with "no credential — `rupu auth login --provider github`" hint that becomes a clickable link launching the OAuth flow.
- **Workflow validation error** (caught by `Workflow::parse` in YAML view): inline diagnostic at the failing line; toast at the bottom of the YAML pane with the same message.
- **Run failures**: surfaced on the failed node (red border + ✗ glyph) + drill-down auto-jumps to the failing step's transcript.
- **MCP host unreachable** (v2): attached host shows a red dot in the sidebar; runs against it queue or refuse with a clear hint.
- **Pane renderer panic**: isolated to the pane. GPUI catches and shows an "error" tile in that pane; other panes / the rest of the window stays alive. Crash report offered to the user via a dialog.

---

## 13. Out of scope (v1)

- **Remote executor** — `WorkflowExecutor` trait designed for it; impl lands in D-10 as opt-in; production-grade MCP transport is a follow-on slice.
- **Cross-workspace search** — search inside a workspace exists from D-1 (sidebar filter); across workspaces is post-v1.
- **Theme customization** — dark theme is default; light theme is a v1 toggle but no user-customizable theme.
- **iOS / iPad** — out of scope.
- **Windows / Linux** — out of scope for v1. GPUI runs there but the workspace persistence path, signing pipeline, menubar pattern, and dock interactions are all macOS-specific. Lift later when the product is good.
- **Plugin / extension API** — agents and workflows are the extension model; no separate plugin surface.
- **In-app billing / licensing** — open source; if a hosted-MCP product layer exists in the future, it's separate.
- **Real-time multi-user collaboration on a canvas** — single-user; YAML round-trip + git is the collaboration model.

---

## 14. Open questions

These are calls we'll revisit at plan time, not blockers for the spec:

- **GPUI versioning.** GPUI is pre-1.0; do we git-pin a Zed commit or wait for a published crate? Likely git-pin for D-1, revisit at D-10.
- **File watcher choice.** `notify` (used in Slice C) vs GPUI's built-in. Likely `notify` for consistency.
- **Menubar implementation.** GPUI doesn't yet have a documented menubar API; may need a small `objc2` shim. Confirm at D-1 plan time.
- **Schema-aware YAML completion.** Driven by which crate (tree-sitter, lsp-types, custom)? D-5 plan picks.
- **Insta snapshot strategy for GPUI scenes.** May need a custom helper; D-2 plan establishes the pattern.

---

## 15. Reference material

- **Slice C TUI design** — visual language precursor, status palette, canvas auto-layout algorithm.
- **Okesu** — `~/Code/Oracle/okesu` (matt's Go sibling). Source of the visual vocabulary for Canvas (horizontal flow + grouped fan-out cards + drag-source agent library) and Transcript (event-row animations). See `okesu/web/src/components/investigations/CaseGraph.tsx` and `okesu/web/src/styles.css`.
- **Zed** — `github.com/zed-industries/zed`. GPUI examples, menubar patterns, split-pane tree model.
- **Brainstorm artifacts** — `.superpowers/brainstorm/35768-1778550913/content/*.html` (workspace shell v1/v2, section switchers, splits, canvas variants, view picker). Persists for future-self reference; should be added to `.gitignore` if not already.
