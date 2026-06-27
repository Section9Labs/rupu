// WorkflowEditor — the lazy-loaded visual workflow editor.
//
// Composes the editable @xyflow/react canvas (WorkflowEditorGraph) with the
// per-step form (StepForm) and the workflow-settings form (WorkflowSettingsForm)
// built in earlier tasks, round-tripping the whole thing through YAML:
//   initialYaml → yamlToGraph → (edit) → graphToWorkflowObject → yaml.dump
//
// The PAGE lazy-loads this module so the @xyflow/react dependency (pulled in
// transitively via the canvas) stays out of the main bundle. We may therefore
// import the canvas statically here.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import yaml from 'js-yaml';
import type { AgentSummary } from '../../lib/api';
import {
  graphToWorkflowObject,
  validateGraph,
  yamlToGraph,
  type StepNodeData,
  type WorkflowGraph,
  type WorkflowMeta,
} from '../../lib/workflowGraph';
import { autoLayout } from '../../lib/workflowLayout';
import WorkflowEditorGraph from './WorkflowEditorGraph';
import StepForm from './StepForm';
import WorkflowSettingsForm from './WorkflowSettingsForm';

interface WorkflowEditorProps {
  initialYaml: string;
  agents: AgentSummary[];
  /** Emit serialized YAML whenever the graph/meta changes. */
  onYamlChange: (yaml: string) => void;
}

/** Parse `initialYaml` into a laid-out graph. A non-object document (or a parse
 *  error) degrades to an empty workflow rather than throwing. */
function seedGraph(initialYaml: string): WorkflowGraph {
  let obj: Record<string, unknown> = {};
  try {
    const loaded = yaml.load(initialYaml);
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

type PanelTab = 'step' | 'settings';

export default function WorkflowEditor({ initialYaml, agents, onYamlChange }: WorkflowEditorProps) {
  const [graph, setGraph] = useState<WorkflowGraph>(() => seedGraph(initialYaml));
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [panelTab, setPanelTab] = useState<PanelTab>('settings');
  const [connError, setConnError] = useState<string | null>(null);

  // We re-seed only when `initialYaml` IDENTITY changes from a *foreign* source
  // (the parent swapped the document). When WE emit YAML via onYamlChange the
  // parent feeds it straight back as the next `initialYaml`; recording our own
  // emission here lets the effect skip that echo and avoid clobbering edits.
  const lastSeenYaml = useRef(initialYaml);
  useEffect(() => {
    if (lastSeenYaml.current === initialYaml) return;
    lastSeenYaml.current = initialYaml;
    setGraph(seedGraph(initialYaml));
    setSelectedId(null);
  }, [initialYaml]);

  const problemsById = useMemo(() => validateGraph(graph), [graph]);

  // Commit a new graph: store it, then serialize back to YAML and emit. A cycle
  // (graphToWorkflowObject → {error}) keeps the graph state but skips the emit —
  // the canvas already prevents cycles, so this is only a safety net.
  const commit = useCallback(
    (next: WorkflowGraph): void => {
      setGraph(next);
      const res = graphToWorkflowObject(next);
      if ('obj' in res) {
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

  return (
    <div className="space-y-3">
      {connError && (
        <div
          role="alert"
          className="flex items-start gap-2 rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-[12px] text-amber-800"
        >
          <span className="flex-1">{connError}</span>
          <button
            type="button"
            onClick={() => setConnError(null)}
            aria-label="Dismiss"
            className="shrink-0 font-semibold text-amber-700 hover:text-amber-900"
          >
            ✕
          </button>
        </div>
      )}

      <div className="flex flex-col gap-4 lg:flex-row">
        <div className="min-w-0 flex-1">
          <WorkflowEditorGraph
            graph={graph}
            onChange={commit}
            selectedId={selectedId}
            onSelect={handleSelect}
            problemsById={problemsById}
            onInvalidConnection={setConnError}
          />
        </div>

        <aside className="w-full shrink-0 rounded-xl border border-border bg-panel p-4 lg:w-96">
          <div className="mb-3 inline-flex rounded-lg border border-border bg-white p-0.5">
            <PanelTabButton active={panelTab === 'step'} onClick={() => setPanelTab('step')}>
              Step
            </PanelTabButton>
            <PanelTabButton active={panelTab === 'settings'} onClick={() => setPanelTab('settings')}>
              Settings
            </PanelTabButton>
          </div>

          {panelTab === 'step' ? (
            selectedNode ? (
              <StepForm
                node={selectedNode}
                agents={agents}
                onChange={onStepChange}
                problems={problemsById[selectedNode.id] ?? []}
              />
            ) : (
              <p className="text-[13px] text-ink-dim">
                Select a step in the canvas to edit it, or add one from the palette.
              </p>
            )
          ) : (
            <WorkflowSettingsForm meta={graph.meta} onChange={onMetaChange} />
          )}
        </aside>
      </div>
    </div>
  );
}

function PanelTabButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={
        active
          ? 'rounded-md bg-brand-600 px-3 py-1 text-[12px] font-medium text-white'
          : 'rounded-md px-3 py-1 text-[12px] font-medium text-ink-dim hover:text-ink'
      }
    >
      {children}
    </button>
  );
}
