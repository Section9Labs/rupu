/**
 * runGraphModel.ts — Pure merge function for the run-graph view.
 *
 * Merge precedence (highest → lowest) per step:
 *   live SSE events  >  checkpoints / step_results  >  skeleton (pending)
 *
 * Unit matching: by `index` (integer).  `item: unknown` is coerced to a
 * display string via `coerceItem`.
 */

import type {
  RunGraphResponse,
  StepNodeDto,
  RunEvent,
} from './api';
import { isKnownRunEvent } from './api';

// ---------------------------------------------------------------------------
// Exported types
// ---------------------------------------------------------------------------

export type StepState =
  | 'pending'
  | 'running'
  | 'awaiting_approval'
  | 'paused'
  | 'done'
  | 'failed'
  | 'skipped';

export interface UnitView {
  index: number;
  key: string;
  state: StepState;
  transcriptPath?: string;
  /** The agent this unit runs, from its unit_started / agent_started event
   *  (panel units each run their own panelist / fixer agent). */
  agent?: string;
  /** Server-minted agent codename for this unit (checkpoint / unit_started / agent_started). */
  codename?: string;
  /** True when `codename` was derived on read for a pre-codename run (render muted). */
  codenameDerived?: boolean;
  /** Provider + model from the unit's `agent_started` (absent for placed units). */
  provider?: string;
  model?: string;
  /** `step_warning` messages about this unit, in arrival order. Never affects
   *  `state` — a warning is not a status. */
  warnings?: string[];
}

/** One `step_warning`, as kept on its step. `index` is set when the warning is
 *  about one fan-out unit (the unit's own `warnings` carries the message too). */
export interface StepWarningView {
  index?: number;
  message: string;
}

/** A warning flattened for run-level display (see {@link collectWarnings}). */
export interface RunWarning extends StepWarningView {
  stepId: string;
}

/** One `parallel:` sub-step. `agent` comes from the DAG; codename from the
 *  step_result's per-sub item; codename/provider/model from `agent_started`
 *  whose `unit_index` is the sub-step's declared position. */
export interface ParallelSubView {
  id: string;
  state: StepState;
  agent?: string;
  codename?: string;
  codenameDerived?: boolean;
  provider?: string;
  model?: string;
}

export interface FanoutState {
  total: number;
  byState: Record<StepState, number>;
  units: UnitView[];
}

export interface GraphNode {
  id: string;
  kind: StepNodeDto['kind'];
  agent?: string;
  /** Server-minted codename for a linear step's agent. Fan-out / panel /
   *  parallel steps carry none — their instances live on `fanout.units`. */
  codename?: string;
  /** True when `codename` was derived on read for a pre-codename run. */
  codenameDerived?: boolean;
  /** Provider + model from the step's `agent_started` event. */
  provider?: string;
  model?: string;
  /** Every `step_warning` for this step (step-level and per-unit), in arrival
   *  order. Informational only — never changes `state`. */
  warnings?: StepWarningView[];
  state: StepState;
  /** Path to this step's agent transcript JSONL, when one was recorded. */
  transcriptPath?: string;
  fanout?: FanoutState;
  parallel?: ParallelSubView[];
  /** For panel/gate steps — current iteration / max. Task 9 populates `current`. */
  round?: { current: number; max: number };
  gate?: StepNodeDto['gate'];
  /** Connector action tool name — populated only for `kind === 'action'`. */
  action?: StepNodeDto['action'];
  /** Standalone approval-gate configuration — populated only for
   *  `kind === 'gate'`. Distinct from `gate` above, which is the panel step's
   *  iteration-loop gate. */
  approval_gate?: StepNodeDto['approval_gate'];
}

export interface GraphEdge {
  from: string;
  to: string;
}

export interface RunGraphModel {
  nodes: GraphNode[];
  edges: GraphEdge[];
  nodeById(id: string): GraphNode | undefined;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Coerce `item: unknown` from a UnitCheckpoint to a display string. */
function coerceItem(item: unknown): string {
  // `JSON.stringify(undefined) === undefined`, so guard the total path:
  // strings pass through; everything else stringifies, falling back to
  // `String(item)` when stringify yields undefined (e.g. `item === undefined`).
  return typeof item === 'string' ? item : (JSON.stringify(item) ?? String(item));
}

interface AgentIdentity {
  agent?: string;
  codename?: string;
  codename_derived?: boolean;
  provider?: string;
  model?: string;
}

/** Layer `later` over `base` field-by-field (later wins where it has a value). */
function mergeIdentity(base: AgentIdentity | undefined, later: AgentIdentity): AgentIdentity {
  const out: AgentIdentity = { ...base };
  if (later.agent) out.agent = later.agent;
  if (later.codename) {
    out.codename = later.codename;
    out.codename_derived = later.codename_derived;
  }
  if (later.provider) out.provider = later.provider;
  if (later.model) out.model = later.model;
  return out;
}

interface Named {
  codename?: string;
  codenameDerived?: boolean;
}

/** Set a codename together with its derived flag (a stored name clears a
 *  previously derived one). An absent codename leaves the target as is. */
function setCodename(target: Named, codename: unknown, derived: unknown): void {
  if (typeof codename !== 'string' || !codename) return;
  target.codename = codename;
  if (derived === true) target.codenameDerived = true;
  else delete target.codenameDerived;
}

/** Overlay an agent_started identity; absent fields leave existing values. */
function applyIdentity(
  target: Named & { provider?: string; model?: string },
  id: AgentIdentity,
): void {
  setCodename(target, id.codename, id.codename_derived);
  if (id.provider) target.provider = id.provider;
  if (id.model) target.model = id.model;
}

/** Every warning in the model, flattened in graph (step) order — what a
 *  run-level "this run has warnings" banner lists. */
export function collectWarnings(model: RunGraphModel): RunWarning[] {
  const out: RunWarning[] = [];
  for (const node of model.nodes) {
    for (const w of node.warnings ?? []) out.push({ stepId: node.id, ...w });
  }
  return out;
}

/** Zero-fill a byState counter object. */
function emptyByState(): Record<StepState, number> {
  return {
    pending: 0,
    running: 0,
    awaiting_approval: 0,
    paused: 0,
    done: 0,
    failed: 0,
    skipped: 0,
  };
}

// ---------------------------------------------------------------------------
// Core builder
// ---------------------------------------------------------------------------

export function buildRunGraphModel(
  g: RunGraphResponse,
  events: RunEvent[],
): RunGraphModel {
  // ------------------------------------------------------------------
  // Phase 1: Build skeleton from workflow.steps — all pending.
  // ------------------------------------------------------------------
  const nodeMap = new Map<string, GraphNode>();

  for (const dto of g.workflow.steps) {
    const node: GraphNode = {
      id: dto.id,
      kind: dto.kind,
      state: 'pending',
    };

    if (dto.agent != null) node.agent = dto.agent;
    if (dto.gate != null) node.gate = dto.gate;
    if (dto.action != null) node.action = dto.action;
    if (dto.approval_gate != null) node.approval_gate = dto.approval_gate;

    // Parallel sub-steps: initialise each to pending.
    if (dto.kind === 'parallel' && dto.parallel != null) {
      node.parallel = dto.parallel.map((sub) => {
        const view: ParallelSubView = { id: sub.id, state: 'pending' };
        if (sub.agent) view.agent = sub.agent;
        return view;
      });
    }

    nodeMap.set(dto.id, node);
  }

  // ------------------------------------------------------------------
  // Phase 2: Overlay step_results (lower precedence than events).
  // ------------------------------------------------------------------
  for (const result of g.step_results) {
    const node = nodeMap.get(result.step_id);
    if (!node) continue;

    if (result.transcript_path != null) node.transcriptPath = result.transcript_path;
    setCodename(node, result.codename, result.codename_derived);
    // Parallel steps persist one item per sub-step (`sub_id`) carrying the
    // instance's codename.
    if (node.parallel && Array.isArray(result.items)) {
      for (const item of result.items) {
        if (item == null || typeof item !== 'object') continue;
        const rec = item as Record<string, unknown>;
        if (typeof rec.sub_id !== 'string' || typeof rec.codename !== 'string' || !rec.codename) continue;
        const sub = node.parallel.find((s) => s.id === rec.sub_id);
        if (sub) setCodename(sub, rec.codename, rec.codename_derived);
      }
    }

    if (result.skipped === true) {
      node.state = 'skipped';
    } else if (result.success === true) {
      node.state = 'done';
    } else if (result.success === false) {
      node.state = 'failed';
    }
    // If both success and skipped are absent/undefined, leave as pending.
  }

  // ------------------------------------------------------------------
  // Phase 3: Build per-step unit map from checkpoints (terminal).
  // ------------------------------------------------------------------
  // unitsByStep: step_id → map of index → UnitView
  const unitsByStep = new Map<string, Map<number, UnitView>>();

  for (const cp of g.units) {
    let units = unitsByStep.get(cp.step_id);
    if (!units) {
      units = new Map<number, UnitView>();
      unitsByStep.set(cp.step_id, units);
    }
    const unitState: StepState = cp.success === true ? 'done' : cp.success === false ? 'failed' : 'running';
    const unit: UnitView = {
      index: cp.index,
      key: coerceItem(cp.item),
      state: unitState,
      transcriptPath: cp.transcript_path,
    };
    setCodename(unit, cp.codename, cp.codename_derived);
    if (cp.agent) unit.agent = cp.agent;
    if (cp.provider) unit.provider = cp.provider;
    if (cp.model) unit.model = cp.model;
    units.set(cp.index, unit);
  }

  // ------------------------------------------------------------------
  // Phase 4: Overlay live events (highest precedence).
  //
  // Events are processed in array order; later events overwrite earlier
  // ones for the same step/unit (last-event-wins within the events slice).
  // ------------------------------------------------------------------
  // `agent_started` identities are applied after the loop: a unit's
  // agent_started can precede (or race) its unit_started, so deferring lets
  // it land on the unit whichever arrives first. `unit_index` absent ⇒ the
  // step itself; present ⇒ that unit. Last event wins per target.
  //
  // Seeded from the graph response's server-side fold (the WHOLE
  // events.jsonl), so identities survive the capped live-event window; the
  // live events below layer on top, field by field (live wins).
  // `step_warning`s are likewise applied after the loop (a unit warning can
  // beat its unit_started) and never touch a status: they only annotate.
  const warningEvents: Array<{ stepId: string; index?: number; message: string }> = [];
  const stepIdentities = new Map<string, AgentIdentity>(Object.entries(g.step_identities ?? {}));
  const unitIdentities = new Map<string, Map<number, AgentIdentity>>();
  for (const [stepId, byIndex] of Object.entries(g.unit_identities ?? {})) {
    const m = new Map<number, AgentIdentity>();
    for (const [idx, id] of Object.entries(byIndex)) {
      const n = Number(idx);
      if (Number.isInteger(n)) m.set(n, id);
    }
    unitIdentities.set(stepId, m);
  }

  // What a superseded attempt's in-flight work falls back to: the durable
  // state from Phases 2-3 (a unit's checkpoint / a step's result), else
  // pending. A runner that ended — resumed over, paused, or failed — leaves
  // `*_started` events with no completion; without this they read as
  // "running" forever and a resume that re-runs a done unit drops it from
  // the done count.
  const nodeBaseline = new Map<string, StepState>();
  for (const node of nodeMap.values()) nodeBaseline.set(node.id, node.state);
  const unitBaseline = new Map<string, Map<number, StepState>>();
  for (const [stepId, units] of unitsByStep) {
    unitBaseline.set(stepId, new Map(Array.from(units.values(), (u) => [u.index, u.state])));
  }
  const settled = (s: StepState | undefined): StepState => (s && s !== 'running' ? s : 'pending');
  function settleInFlight() {
    for (const node of nodeMap.values()) {
      if (node.state === 'running') node.state = settled(nodeBaseline.get(node.id));
    }
    for (const [stepId, units] of unitsByStep) {
      for (const unit of units.values()) {
        if (unit.state === 'running') unit.state = settled(unitBaseline.get(stepId)?.get(unit.index));
      }
    }
  }

  for (const ev of events) {
    if (!isKnownRunEvent(ev)) continue;

    switch (ev.type) {
      case 'step_started':
      case 'step_working': {
        const node = nodeMap.get(ev.step_id);
        if (node) {
          node.state = 'running';
          if (ev.type === 'step_started') setCodename(node, ev.codename, ev.codename_derived);
          // A running linear step has no persisted step_result yet, so its
          // transcript path arrives live on step_working — adopt it so the
          // panel can select and tail the file in real time.
          if (ev.type === 'step_working' && ev.transcript_path) {
            node.transcriptPath = ev.transcript_path;
          }
        }
        break;
      }
      case 'step_awaiting_approval': {
        const node = nodeMap.get(ev.step_id);
        if (node) node.state = 'awaiting_approval';
        break;
      }
      case 'step_completed': {
        const node = nodeMap.get(ev.step_id);
        if (node) node.state = ev.success ? 'done' : 'failed';
        break;
      }
      case 'step_failed': {
        const node = nodeMap.get(ev.step_id);
        if (node) node.state = 'failed';
        break;
      }
      case 'step_skipped': {
        const node = nodeMap.get(ev.step_id);
        if (node) node.state = 'skipped';
        break;
      }
      case 'step_warning': {
        // Deliberately no `state` change — see the deferred application below.
        warningEvents.push({
          stepId: ev.step_id,
          index: typeof ev.index === 'number' ? ev.index : undefined,
          message: ev.message,
        });
        break;
      }
      case 'unit_started': {
        // Ensure this unit exists in the map; if a checkpoint already placed it,
        // the live event wins — set to 'running'.
        let units = unitsByStep.get(ev.step_id);
        if (!units) {
          units = new Map<number, UnitView>();
          unitsByStep.set(ev.step_id, units);
        }
        const existing = units.get(ev.index);
        if (existing) {
          existing.state = 'running';
          setCodename(existing, ev.codename, ev.codename_derived);
          if (ev.agent) existing.agent = ev.agent;
        } else {
          const unit: UnitView = {
            index: ev.index,
            key: ev.unit_key,
            state: 'running',
            transcriptPath: ev.transcript_path,
          };
          setCodename(unit, ev.codename, ev.codename_derived);
          if (ev.agent) unit.agent = ev.agent;
          units.set(ev.index, unit);
        }
        break;
      }
      case 'agent_started': {
        const id: AgentIdentity = {
          agent: ev.agent,
          codename: ev.codename,
          codename_derived: ev.codename_derived,
          provider: ev.provider,
          model: ev.model,
        };
        if (ev.unit_index == null) {
          stepIdentities.set(ev.step_id, mergeIdentity(stepIdentities.get(ev.step_id), id));
        } else {
          let m = unitIdentities.get(ev.step_id);
          if (!m) {
            m = new Map<number, AgentIdentity>();
            unitIdentities.set(ev.step_id, m);
          }
          m.set(ev.unit_index, mergeIdentity(m.get(ev.unit_index), id));
        }
        break;
      }
      case 'unit_completed': {
        const units = unitsByStep.get(ev.step_id);
        if (units) {
          const unit = units.get(ev.index);
          if (unit) {
            unit.state = ev.success ? 'done' : 'failed';
          }
        }
        break;
      }
      case 'panel_round': {
        const n = nodeMap.get(ev.step_id);
        if (n) n.round = { current: ev.round, max: ev.max_iterations };
        break;
      }
      case 'step_paused': {
        const node = nodeMap.get(ev.step_id);
        if (node) node.state = 'paused';
        break;
      }
      case 'step_resumed': {
        const node = nodeMap.get(ev.step_id);
        if (node) node.state = 'running';
        break;
      }
      case 'run_started': // a resume: the previous attempt's runner is gone
      case 'run_paused':
      case 'run_failed':
        settleInFlight();
        break;
      case 'run_completed':
      case 'run_resumed':
        // No per-step change (the in-flight step's own
        // `step_paused`/`step_resumed` event, above, carries the per-node
        // transition; a completed run is reconciled in Phase 5b).
        break;
    }
  }

  // Apply deferred agent_started identities. A unit key matches the unit
  // list's `index` (== the unit_started index; panel/parallel units use
  // their own view/declared index). Never fabricates a node or unit.
  for (const [stepId, id] of stepIdentities) {
    const node = nodeMap.get(stepId);
    if (node) applyIdentity(node, id);
  }
  for (const [stepId, byIndex] of unitIdentities) {
    const units = unitsByStep.get(stepId);
    const subs = nodeMap.get(stepId)?.parallel;
    for (const [idx, id] of byIndex) {
      const unit = units?.get(idx);
      if (unit) {
        applyIdentity(unit, id);
        if (id.agent) unit.agent = id.agent;
      }
      // Parallel sub-steps: unit_index is the sub-step's declared position.
      const sub = subs?.[idx];
      if (sub) applyIdentity(sub, id);
    }
  }

  // ------------------------------------------------------------------
  // Phase 5: Fold unit maps into fanout; flip parent state if in-flight.
  // ------------------------------------------------------------------
  for (const [stepId, units] of unitsByStep.entries()) {
    const node = nodeMap.get(stepId);
    if (!node) continue;

    const sorted = Array.from(units.values()).sort((a, b) => a.index - b.index);

    const byState = emptyByState();
    for (const u of sorted) {
      byState[u.state] += 1;
    }

    node.fanout = {
      total: sorted.length,
      byState,
      units: sorted,
    };

    // If any unit is running/awaiting and the step's own state is still
    // pending (i.e. no step-level event fired yet), promote to running.
    const hasInFlight = byState.running > 0 || byState.awaiting_approval > 0;
    if (hasInFlight && node.state === 'pending') {
      node.state = 'running';
    }
  }

  // Apply step warnings (annotation only — never a status). Each lands on its
  // step, and on its unit when `index` names one that exists. A warning for a
  // step the skeleton lacks is dropped (never fabricates a node); an exact
  // repeat (same unit + message) is shown once.
  for (const w of warningEvents) {
    const node = nodeMap.get(w.stepId);
    if (!node) continue;
    const list = (node.warnings ??= []);
    if (list.some((x) => x.index === w.index && x.message === w.message)) continue;
    list.push(w.index === undefined ? { message: w.message } : { index: w.index, message: w.message });
    if (w.index !== undefined) {
      const unit = unitsByStep.get(w.stepId)?.get(w.index);
      if (unit) (unit.warnings ??= []).push(w.message);
    }
  }

  // ------------------------------------------------------------------
  // Phase 5b: Reconcile lingering in-flight state against a terminally
  // successful run.
  //
  // A unit checkpoint with `success: null` (no terminal checkpoint / an
  // unmatched `unit_completed`) folds to a non-terminal state in Phases 3-5,
  // which surfaces as "awaiting" in the graph. On a run that has already
  // completed successfully, nothing else reconciles those leftovers — so a
  // finished run can still display in-flight units. Promote them to 'done'.
  //
  // This ONLY runs for a successfully completed run (`status === 'completed'`).
  // Failed / rejected / still-running / pending runs are left untouched so
  // genuine failures and genuine in-flight work keep rendering truthfully.
  if (g.run.status === 'completed') {
    for (const node of nodeMap.values()) {
      if (
        node.state === 'pending' ||
        node.state === 'running' ||
        node.state === 'awaiting_approval'
      ) {
        node.state = 'done';
      }

      if (node.fanout) {
        let changed = false;
        for (const unit of node.fanout.units) {
          if (unit.state === 'running' || unit.state === 'awaiting_approval') {
            unit.state = 'done';
            changed = true;
          }
        }
        // Recompute byState from the (possibly promoted) units so the
        // fan-out badges stay consistent with the unit list.
        if (changed) {
          const byState = emptyByState();
          for (const u of node.fanout.units) {
            byState[u.state] += 1;
          }
          node.fanout.byState = byState;
        }
      }
    }
  }

  // ------------------------------------------------------------------
  // Phase 6: Build edges — the workflow's REAL DAG.
  //
  // The backend sends `workflow.edges` derived from the workflow's actual
  // topology (split / join / branch / next / depends_on, or the legacy
  // consecutive-pair chain for an edge-free workflow). Using them makes the
  // run graph FORK where the workflow forks instead of collapsing every step
  // into one inline line. Fall back to a linear chain only when the field is
  // absent — an older backend, or a synthesized bare-agent / unpersisted DAG
  // that carries no edges — so pre-`edges` responses still render.
  // ------------------------------------------------------------------
  const nodes = g.workflow.steps
    .map((dto) => nodeMap.get(dto.id))
    .filter((n): n is GraphNode => n !== undefined);

  const edges: GraphEdge[] = [];
  const wfEdges = g.workflow.edges;
  if (wfEdges !== undefined) {
    for (const e of wfEdges) {
      // Guard against an edge naming a step the skeleton doesn't include.
      if (nodeMap.has(e.from) && nodeMap.has(e.to)) {
        edges.push({ from: e.from, to: e.to });
      }
    }
  } else {
    for (let i = 0; i < nodes.length - 1; i++) {
      edges.push({ from: nodes[i].id, to: nodes[i + 1].id });
    }
  }

  // ------------------------------------------------------------------
  // Build the model — include nodeById lookup.
  // ------------------------------------------------------------------
  return {
    nodes,
    edges,
    nodeById(id: string): GraphNode | undefined {
      return nodeMap.get(id);
    },
  };
}
