// WorkflowEditor — the unified, lazy-loaded visual workflow editor shell.
//
// One screen (no tabs): an editable @xyflow/react canvas (WorkflowEditorGraph) on
// top, the live YAML editor (CodeEditor) below — separated by a resizable
// SplitPane — and an inspector rail on the right that switches between the
// workflow Settings form and the selected step's form.
//
// The whole thing round-trips through YAML:
//   draftYaml → yamlToGraph → (edit) → graphToWorkflowObject → yaml.dump
// `draftYaml` is owned by the PAGE (single source of truth); graph edits emit
// regenerated YAML back via `onYamlChange`. YAML→graph reseeds only on a FOREIGN
// change (parent swapped the document) — guarded by `lastSeenYaml`. Live per-
// keystroke YAML→graph reconcile is a LATER phase; behavior here matches today.
//
// The PAGE lazy-loads this module so the @xyflow/react dependency (pulled in
// transitively via the canvas) stays out of the main bundle. We may therefore
// import the canvas (and CodeEditor) statically here.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import yaml from 'js-yaml';
import { api, type AgentSummary, type ToolSpec } from '../../lib/api';
import {
  graphToWorkflowObject,
  topoSort,
  validateGraph,
  yamlToGraph,
  type StepKind,
  type StepNodeData,
  type WorkflowGraph,
  type WorkflowMeta,
} from '../../lib/workflowGraph';
import { autoLayout, reconcileFromYaml } from '../../lib/workflowLayout';
import type { WorkflowEditorUi } from '../../hooks/useWorkflowEditorUi';
import CodeEditor from '../CodeEditor';
import WorkflowEditorGraph from './WorkflowEditorGraph';
import StepForm from './StepForm';
import WorkflowSettingsForm from './WorkflowSettingsForm';
import SplitPane from './SplitPane';
import ExpressionReference from './ExpressionReference';

interface WorkflowEditorProps {
  /** The shared editable YAML draft (owned by the page — single source of truth). */
  draftYaml: string;
  /** Emit serialized YAML whenever the graph/meta changes. */
  onYamlChange: (yaml: string) => void;
  agents: AgentSummary[];
  /** Live-validate result from the page (server-side parse check). */
  validity: { ok: boolean; error?: string } | null;
  /** Workflow-editor-UI flag — threaded down to NodePalette (gates the branch
   *  palette card) and StepForm (gates the "Branch (if)" kind option).
   *  Defaults to 'classic' for callers that don't thread it. */
  workflowEditorUi?: WorkflowEditorUi;
}

/** Parse `draftYaml` into a laid-out graph. A non-object document (or a parse
 *  error) degrades to an empty workflow rather than throwing. */
function seedGraph(draftYaml: string): WorkflowGraph {
  let obj: Record<string, unknown> = {};
  try {
    const loaded = yaml.load(draftYaml);
    if (typeof loaded === 'object' && loaded !== null && !Array.isArray(loaded)) {
      obj = loaded as Record<string, unknown>;
    }
  } catch {
    obj = {};
  }
  const g = yamlToGraph(obj);
  g.nodes = autoLayout(g.nodes, g.edges);
  return g;
}

type PanelTab = 'step' | 'settings' | 'reference';

/** localStorage flag: the canonical-rewrite notice has been shown once. */
const REFORMAT_NOTICE_KEY = 'rupu.editor.reformatNoticeSeen';

/** localStorage flag: whether the YAML source pane is open (`next` UI only).
 *  Missing / garbage / '1' → open; only an explicit '0' collapses it. */
const SOURCE_OPEN_KEY = 'rupu.editor.sourceOpen';

function readSourceOpen(): boolean {
  try {
    if (typeof localStorage === 'undefined') return true;
    return localStorage.getItem(SOURCE_OPEN_KEY) !== '0';
  } catch {
    return true;
  }
}

/** id shared between the source-toggle button's `aria-controls` and the YAML
 *  editor's wrapping container, so assistive tech can locate the pane it
 *  expands/collapses. Only mounted while the pane is open. */
const SOURCE_PANE_ID = 'wf-source-editor';

// ── inspector rail width (Task 5, `next` UI only, lg+ screens) ──────────────

/** localStorage key for the persisted inspector-rail width (px, int). */
const RAIL_WIDTH_KEY = 'rupu.editor.railWidth';
const RAIL_WIDTH_DEFAULT = 320;
const RAIL_WIDTH_MIN = 280;
const RAIL_WIDTH_MAX = 640;

function clampRailWidth(n: number): number {
  return Math.min(RAIL_WIDTH_MAX, Math.max(RAIL_WIDTH_MIN, n));
}

/** Read the persisted rail width. Missing / garbage / out-of-range → the
 *  default (280–640 clamp handles out-of-range; a non-finite parse falls back
 *  to the default outright). */
function readRailWidth(): number {
  try {
    if (typeof localStorage === 'undefined') return RAIL_WIDTH_DEFAULT;
    const raw = localStorage.getItem(RAIL_WIDTH_KEY);
    if (raw === null) return RAIL_WIDTH_DEFAULT;
    const n = parseInt(raw, 10);
    return Number.isFinite(n) ? clampRailWidth(n) : RAIL_WIDTH_DEFAULT;
  } catch {
    return RAIL_WIDTH_DEFAULT;
  }
}

function persistRailWidth(n: number): void {
  try {
    localStorage.setItem(RAIL_WIDTH_KEY, String(Math.round(n)));
  } catch {
    /* localStorage unavailable (private mode / SSR) — skip persistence */
  }
}

/** True if `text` contains a YAML comment — a line whose first non-space char is
 *  `#`, or an inline ` #`. A heuristic (may flag `#` inside a quoted scalar);
 *  used only to gate a one-time, dismissible notice, so over-warning is benign. */
function hasYamlComments(text: string): boolean {
  for (const line of text.split('\n')) {
    if (line.trimStart().startsWith('#')) return true;
    if (/\s#/.test(line)) return true;
  }
  return false;
}

export default function WorkflowEditor({
  draftYaml,
  onYamlChange,
  agents,
  validity,
  workflowEditorUi = 'classic',
}: WorkflowEditorProps) {
  const [graph, setGraph] = useState<WorkflowGraph>(() => seedGraph(draftYaml));
  // MCP tool catalog for the connector ACTION cards + the action-body tool
  // <select> (Task 5). Best-effort: a fetch failure just leaves no connector
  // cards (the palette degrades to kind cards only).
  const [tools, setTools] = useState<ToolSpec[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [panelTab, setPanelTab] = useState<PanelTab>('settings');
  const [connError, setConnError] = useState<string | null>(null);
  // Transient notice (e.g. selection lost on rename), auto-dismissed.
  const [notice, setNotice] = useState<string | null>(null);
  // True while the YAML is unparseable: the graph is kept but frozen + dimmed.
  const [paused, setPaused] = useState(false);
  // One-time banner: a graph edit on a commented document reformats the YAML
  // (canonical rewrite) and drops comments. Shown at most once per browser.
  const [reformatNotice, setReformatNotice] = useState(false);

  // Inspector-rail palette slot (Task 1, `next` only): captured via a ref
  // callback (not useRef) so its FIRST paint is a state update — the graph
  // needs the actual mounted element to portal the palette into, and a plain
  // ref wouldn't trigger the re-render that hands it down.
  const [paletteSlot, setPaletteSlot] = useState<HTMLElement | null>(null);
  const paletteSlotRef = useCallback((el: HTMLElement | null) => setPaletteSlot(el), []);

  // YAML source pane visibility (`next` UI only, Task 2). Classic always shows
  // the SplitPane and never reads/writes this state.
  const [sourceOpen, setSourceOpen] = useState<boolean>(() => readSourceOpen());
  const toggleSourceOpen = useCallback(() => {
    setSourceOpen((prev) => {
      const next = !prev;
      try {
        localStorage.setItem(SOURCE_OPEN_KEY, next ? '1' : '0');
      } catch {
        /* localStorage unavailable (private mode / SSR) — skip persistence */
      }
      return next;
    });
  }, []);

  // Inspector-rail width (`next` UI only, lg+ screens, Task 5). Classic keeps
  // the literal `lg:w-80` markup and never reads this state. A ref mirrors the
  // latest width so the pointer-drag "up" handler (registered once per drag,
  // outside React state) can persist the FINAL value without re-subscribing
  // on every pointermove.
  const [railWidth, setRailWidth] = useState<number>(() => readRailWidth());
  const railWidthRef = useRef(railWidth);
  railWidthRef.current = railWidth;
  const asideRef = useRef<HTMLElement>(null);

  const setRailWidthFromClientX = useCallback((clientX: number) => {
    const el = asideRef.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    // The aside's right edge is fixed while dragging (only its left edge —
    // i.e. its width — moves), so `rect.right - clientX` is the live width;
    // dragging left (smaller clientX) widens the rail.
    setRailWidth(clampRailWidth(rect.right - clientX));
  }, []);

  const onRailPointerDown = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      e.preventDefault();
      const handle = e.currentTarget;
      handle.setPointerCapture(e.pointerId);
      const onMove = (ev: PointerEvent) => setRailWidthFromClientX(ev.clientX);
      const onUp = (ev: PointerEvent) => {
        handle.releasePointerCapture(ev.pointerId);
        handle.removeEventListener('pointermove', onMove);
        handle.removeEventListener('pointerup', onUp);
        persistRailWidth(railWidthRef.current); // persist once, at drag end.
      };
      handle.addEventListener('pointermove', onMove);
      handle.addEventListener('pointerup', onUp);
    },
    [setRailWidthFromClientX],
  );

  const onRailKeyDown = useCallback((e: React.KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'ArrowLeft') {
      e.preventDefault();
      setRailWidth((w) => {
        const next = clampRailWidth(w + 16); // left widens, mirrors the drag direction.
        persistRailWidth(next);
        return next;
      });
    } else if (e.key === 'ArrowRight') {
      e.preventDefault();
      setRailWidth((w) => {
        const next = clampRailWidth(w - 16);
        persistRailWidth(next);
        return next;
      });
    }
  }, []);

  // Keep the latest selectedId + graph readable inside the debounce timeout
  // WITHOUT adding them to the effect deps (which would re-arm the timer on every
  // select / graph edit). Reading via refs lets every state write below stay a
  // top-level, pure call (no setState inside a setGraph updater).
  const selectedIdRef = useRef(selectedId);
  selectedIdRef.current = selectedId;
  const graphRef = useRef(graph);
  graphRef.current = graph;

  // Live YAML→graph reconcile, debounced 250ms after the last edit.
  //
  // `lastSeenYaml` is the echo guard: when WE emit YAML via onYamlChange the
  // parent feeds it straight back as the next `draftYaml`, and `commit` records
  // that emission here first — so the effect skips our own echo and never fights
  // the user / clobbers positions. A genuine user edit reconciles BY NODE ID
  // (reconcileFromYaml → reconcileGraph): survivors keep their on-screen
  // position, new ids get a dagre slot, removed ids drop. Unparseable YAML pauses
  // (keeps the last good graph) instead of nuking the canvas.
  const lastSeenYaml = useRef(draftYaml);
  // Latest draftYaml readable inside `commit` (which intentionally excludes
  // draftYaml from its deps) so we can sniff the pre-rewrite text for comments.
  const draftYamlRef = useRef(draftYaml);
  draftYamlRef.current = draftYaml;
  useEffect(() => {
    if (lastSeenYaml.current === draftYaml) return; // our own echo — ignore.
    const handle = setTimeout(() => {
      lastSeenYaml.current = draftYaml;
      const res = reconcileFromYaml(graphRef.current, draftYaml);
      if (res.paused || !res.graph) {
        setPaused(true); // keep the last good graph on screen.
        return;
      }
      setPaused(false);
      // Selection preservation: drop + notify if the selected id vanished.
      const sel = selectedIdRef.current;
      if (sel !== null && !res.graph.nodes.some((n) => n.id === sel)) {
        setSelectedId(null);
        setNotice('Selected step changed in YAML.');
      }
      setGraph(res.graph);
    }, 250);
    return () => clearTimeout(handle);
  }, [draftYaml]);

  // Fetch the MCP tool catalog once on mount (connector cards + action body).
  useEffect(() => {
    let alive = true;
    api
      .getTools()
      .then((t) => {
        if (alive) setTools(t);
      })
      .catch(() => {
        /* no catalog → palette degrades to kind cards only */
      });
    return () => {
      alive = false;
    };
  }, []);

  // Auto-dismiss the transient notice.
  useEffect(() => {
    if (notice === null) return;
    const t = setTimeout(() => setNotice(null), 4000);
    return () => clearTimeout(t);
  }, [notice]);

  const problemsById = useMemo(() => validateGraph(graph), [graph]);

  // Commit a new graph: store it, then serialize back to YAML and emit. A cycle
  // (graphToWorkflowObject → {error}) keeps the graph state but skips the emit —
  // the canvas already prevents cycles, so this is only a safety net.
  const commit = useCallback(
    (next: WorkflowGraph): void => {
      setGraph(next);
      const res = graphToWorkflowObject(next);
      if ('obj' in res) {
        // First graph edit on a commented document: warn (once per browser) that
        // editing the graph reformats the YAML canonically and removes comments.
        try {
          if (
            typeof localStorage !== 'undefined' &&
            !localStorage.getItem(REFORMAT_NOTICE_KEY) &&
            hasYamlComments(draftYamlRef.current)
          ) {
            localStorage.setItem(REFORMAT_NOTICE_KEY, '1');
            setReformatNotice(true);
          }
        } catch {
          /* localStorage unavailable (private mode / SSR) — skip the notice */
        }
        const dumped = yaml.dump(res.obj);
        lastSeenYaml.current = dumped;
        onYamlChange(dumped);
      }
    },
    [onYamlChange],
  );

  const handleSelect = useCallback((id: string | null) => {
    setSelectedId(id);
    if (id !== null) setPanelTab('step');
  }, []);

  const onStepChange = useCallback(
    (data: StepNodeData): void => {
      if (selectedId === null) return;
      const next: WorkflowGraph = {
        ...graph,
        nodes: graph.nodes.map((n) => (n.id === selectedId ? { ...n, id: data.id, data } : n)),
      };
      commit(next);
      // The step id may have been renamed — keep the selection pointing at it.
      setSelectedId(data.id);
    },
    [commit, graph, selectedId],
  );

  const onMetaChange = useCallback(
    (meta: WorkflowMeta): void => {
      commit({ ...graph, meta });
    },
    [commit, graph],
  );

  const selectedNode = selectedId ? graph.nodes.find((n) => n.id === selectedId) ?? null : null;

  // Expression-editor vocabulary for the selected node: declared input names +
  // the steps that topologically PRECEDE it (so completions never offer a step
  // that runs later). Recomputed on graph / selection change.
  const exprContext = useMemo<{
    nodeKind: StepKind;
    inputNames: string[];
    priorSteps: { id: string; kind: StepKind }[];
  }>(() => {
    const inputsRaw = graph.meta.rest.inputs;
    const inputNames =
      typeof inputsRaw === 'object' && inputsRaw !== null && !Array.isArray(inputsRaw)
        ? Object.keys(inputsRaw as Record<string, unknown>)
        : [];

    let nodeKind: StepKind = selectedNode?.data.kind ?? 'step';
    let priorSteps: { id: string; kind: StepKind }[] = [];
    if (selectedId) {
      const sorted = topoSort(graph.nodes, graph.edges);
      if ('order' in sorted) {
        const idx = sorted.order.findIndex((n) => n.id === selectedId);
        if (idx >= 0) {
          nodeKind = sorted.order[idx].data.kind;
          priorSteps = sorted.order.slice(0, idx).map((n) => ({ id: n.id, kind: n.data.kind }));
        }
      }
    }
    return { nodeKind, inputNames, priorSteps };
  }, [graph, selectedId, selectedNode]);

  return (
    <div className="relative flex h-[52rem] min-h-[40rem] flex-col overflow-hidden rounded-xl border border-border bg-panel lg:flex-row">
      {connError && (
        <div
          role="alert"
          className="absolute left-3 right-3 top-3 z-20 flex items-start gap-2 rounded-md border border-warn/30 bg-warn-bg px-3 py-2 text-ui text-warn shadow-card lg:right-[21rem]"
        >
          <span className="flex-1">{connError}</span>
          <button
            type="button"
            onClick={() => setConnError(null)}
            aria-label="Dismiss"
            className="shrink-0 font-semibold text-warn hover:text-warn"
          >
            ✕
          </button>
        </div>
      )}

      {notice && (
        <div
          role="status"
          className="absolute left-1/2 top-3 z-30 -translate-x-1/2 rounded-md border border-slate-300 bg-slate-800 px-3 py-1.5 text-ui font-medium text-white shadow-card"
        >
          {notice}
        </div>
      )}

      {reformatNotice && (
        <div
          role="status"
          className="absolute left-3 right-3 top-3 z-20 flex items-start gap-2 rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-ui text-amber-800 shadow-card lg:right-[21rem]"
        >
          <span className="flex-1">
            Editing the graph reformats the YAML and removes comments. Edit the YAML directly to keep
            comments intact.
          </span>
          <button
            type="button"
            onClick={() => setReformatNotice(false)}
            aria-label="Dismiss"
            className="shrink-0 font-semibold text-amber-700 hover:text-amber-900"
          >
            ✕
          </button>
        </div>
      )}

      {/* ── LEFT / MAIN: graph over YAML, resizable ───────────────────────── */}
      <div className="min-h-0 min-w-0 flex-1">
        {workflowEditorUi === 'next' && !sourceOpen ? (
          <div className="flex h-full min-h-0 flex-col">
            <div className="min-h-0 flex-1 overflow-hidden p-3">
              <WorkflowEditorGraph
                graph={graph}
                onChange={commit}
                selectedId={selectedId}
                onSelect={handleSelect}
                problemsById={problemsById}
                onInvalidConnection={setConnError}
                paused={paused}
                workflowEditorUi={workflowEditorUi}
                paletteContainer={paletteSlot}
                tools={tools}
              />
            </div>
            <div className="flex items-center gap-2 border-t border-border bg-panel px-3 py-1.5">
              <span className="text-note text-ink-mute">⟳ synced from graph</span>
              <button
                type="button"
                onClick={toggleSourceOpen}
                aria-expanded={false}
                aria-controls={SOURCE_PANE_ID}
                className="rounded px-1.5 py-0.5 text-note font-medium text-ink-dim hover:bg-surface-hover hover:text-ink"
              >
                Show source
              </button>
              <span className="ml-auto">
                <ValidityBadge validity={validity} />
              </span>
            </div>
          </div>
        ) : (
          <SplitPane
            top={
              <div className="h-full overflow-hidden p-3">
                <WorkflowEditorGraph
                  graph={graph}
                  onChange={commit}
                  selectedId={selectedId}
                  onSelect={handleSelect}
                  problemsById={problemsById}
                  onInvalidConnection={setConnError}
                  paused={paused}
                  workflowEditorUi={workflowEditorUi}
                  paletteContainer={workflowEditorUi === 'next' ? paletteSlot : undefined}
                  tools={tools}
                />
              </div>
            }
            bottom={
              <div className="flex h-full min-h-0 flex-col">
                <div
                  className="min-h-0 flex-1 overflow-auto p-3"
                  {...(workflowEditorUi === 'next' ? { id: SOURCE_PANE_ID } : {})}
                >
                  <CodeEditor
                    value={draftYaml}
                    onChange={onYamlChange}
                    language="yaml"
                    ariaLabel="Workflow YAML editor"
                  />
                </div>
                <div className="flex items-center gap-2 border-t border-border bg-panel px-3 py-1.5">
                  <span className="text-note text-ink-mute">⟳ synced from graph</span>
                  {workflowEditorUi === 'next' && (
                    <button
                      type="button"
                      onClick={toggleSourceOpen}
                      aria-expanded={true}
                      aria-controls={SOURCE_PANE_ID}
                      className="rounded px-1.5 py-0.5 text-note font-medium text-ink-dim hover:bg-surface-hover hover:text-ink"
                    >
                      Hide source
                    </button>
                  )}
                  <span className="ml-auto">
                    <ValidityBadge validity={validity} />
                  </span>
                </div>
              </div>
            }
          />
        )}
      </div>

      {/* ── RIGHT: inspector rail ─────────────────────────────────────────── */}
      <aside
        ref={workflowEditorUi === 'next' ? asideRef : undefined}
        className={
          workflowEditorUi === 'next'
            ? 'relative flex w-full shrink-0 flex-col border-t border-border bg-panel wfx-rail-sized lg:border-l lg:border-t-0'
            : 'flex w-full shrink-0 flex-col border-t border-border bg-panel lg:w-80 lg:border-l lg:border-t-0'
        }
        style={workflowEditorUi === 'next' ? ({ '--wfx-rail-w': `${railWidth}px` } as React.CSSProperties) : undefined}
      >
        {workflowEditorUi === 'next' && (
          <div ref={paletteSlotRef} className="wfx-rail-palette-slot border-b border-border" />
        )}
        {workflowEditorUi === 'next' && (
          <div
            role="separator"
            aria-orientation="vertical"
            aria-label="Resize inspector"
            aria-valuenow={railWidth}
            aria-valuemin={RAIL_WIDTH_MIN}
            aria-valuemax={RAIL_WIDTH_MAX}
            tabIndex={0}
            onPointerDown={onRailPointerDown}
            onKeyDown={onRailKeyDown}
            className="wfx-rail-handle hidden lg:block"
          />
        )}
        <div className="border-b border-border p-3">
          <div role="tablist" aria-label="Inspector" className="inline-flex rounded-lg border border-border bg-panel p-0.5">
            <PanelTabButton
              active={panelTab === 'settings'}
              onClick={() => setPanelTab('settings')}
              tabId="inspector-tab-settings"
              controls="inspector-settings"
            >
              Settings
            </PanelTabButton>
            <PanelTabButton
              active={panelTab === 'step'}
              onClick={() => setPanelTab('step')}
              tabId="inspector-tab-step"
              controls="inspector-step"
            >
              Step
            </PanelTabButton>
            <PanelTabButton
              active={panelTab === 'reference'}
              onClick={() => setPanelTab('reference')}
              tabId="inspector-tab-reference"
              controls="inspector-reference"
            >
              Reference
            </PanelTabButton>
          </div>
        </div>

        <div className="min-h-0 flex-1 overflow-y-auto p-4">
          {panelTab === 'step' && (
            <div role="tabpanel" id="inspector-step" aria-labelledby="inspector-tab-step">
              {selectedNode ? (
                <StepForm
                  node={selectedNode}
                  agents={agents}
                  onChange={onStepChange}
                  problems={problemsById[selectedNode.id] ?? []}
                  exprContext={exprContext}
                  allNodeIds={graph.nodes.map((n) => n.id)}
                  workflowEditorUi={workflowEditorUi}
                  tools={tools}
                />
              ) : (
                <p className="text-lead text-ink-dim">Select a node to edit its step.</p>
              )}
            </div>
          )}
          {panelTab === 'settings' && (
            <div role="tabpanel" id="inspector-settings" aria-labelledby="inspector-tab-settings">
              <WorkflowSettingsForm meta={graph.meta} onChange={onMetaChange} workflowEditorUi={workflowEditorUi} />
            </div>
          )}
          {panelTab === 'reference' && (
            <div
              role="tabpanel"
              id="inspector-reference"
              aria-labelledby="inspector-tab-reference"
              className="h-full"
            >
              <ExpressionReference />
            </div>
          )}
        </div>
      </aside>
    </div>
  );
}

function ValidityBadge({ validity }: { validity: { ok: boolean; error?: string } | null }) {
  if (!validity) return null;
  if (validity.ok) {
    return (
      <span className="inline-flex items-center rounded px-1.5 py-0.5 text-note font-medium ring-1 bg-ok-bg text-ok ring-ok/30">
        ✓ valid
      </span>
    );
  }
  return (
    <span
      className="inline-flex max-w-[16rem] items-center truncate rounded px-1.5 py-0.5 text-note font-medium ring-1 bg-err-bg text-err ring-err/30"
      title={validity.error}
    >
      ✕ {validity.error ?? 'invalid'}
    </span>
  );
}

function PanelTabButton({
  active,
  onClick,
  tabId,
  controls,
  children,
}: {
  active: boolean;
  onClick: () => void;
  tabId: string;
  controls: string;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      role="tab"
      id={tabId}
      aria-selected={active}
      aria-controls={controls}
      onClick={onClick}
      className={
        active
          ? 'rounded-md bg-brand-600 px-3 py-1 text-ui font-medium text-white'
          : 'rounded-md px-3 py-1 text-ui font-medium text-ink-dim hover:text-ink'
      }
    >
      {children}
    </button>
  );
}
