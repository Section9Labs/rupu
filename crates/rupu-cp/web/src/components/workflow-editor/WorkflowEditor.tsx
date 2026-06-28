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
import type { AgentSummary } from '../../lib/api';
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

export default function WorkflowEditor({ draftYaml, onYamlChange, agents, validity }: WorkflowEditorProps) {
  const [graph, setGraph] = useState<WorkflowGraph>(() => seedGraph(draftYaml));
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
              />
            </div>
          }
          bottom={
            <div className="flex h-full min-h-0 flex-col">
              <div className="min-h-0 flex-1 overflow-auto p-3">
                <CodeEditor
                  value={draftYaml}
                  onChange={onYamlChange}
                  language="yaml"
                  ariaLabel="Workflow YAML editor"
                />
              </div>
              <div className="flex items-center gap-2 border-t border-border bg-panel px-3 py-1.5">
                <span className="text-note text-ink-mute">⟳ synced from graph</span>
                <span className="ml-auto">
                  <ValidityBadge validity={validity} />
                </span>
              </div>
            </div>
          }
        />
      </div>

      {/* ── RIGHT: inspector rail ─────────────────────────────────────────── */}
      <aside className="flex w-full shrink-0 flex-col border-t border-border bg-panel lg:w-80 lg:border-l lg:border-t-0">
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
                />
              ) : (
                <p className="text-lead text-ink-dim">Select a node to edit its step.</p>
              )}
            </div>
          )}
          {panelTab === 'settings' && (
            <div role="tabpanel" id="inspector-settings" aria-labelledby="inspector-tab-settings">
              <WorkflowSettingsForm meta={graph.meta} onChange={onMetaChange} />
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
