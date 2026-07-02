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
  state: StepState;
  /** Path to this step's agent transcript JSONL, when one was recorded. */
  transcriptPath?: string;
  fanout?: FanoutState;
  parallel?: { id: string; state: StepState }[];
  /** For panel/gate steps — current iteration / max. Task 9 populates `current`. */
  round?: { current: number; max: number };
  gate?: StepNodeDto['gate'];
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

    // Parallel sub-steps: initialise each to pending.
    if (dto.kind === 'parallel' && dto.parallel != null) {
      node.parallel = dto.parallel.map((sub) => ({ id: sub.id, state: 'pending' as StepState }));
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
    units.set(cp.index, unit);
  }

  // ------------------------------------------------------------------
  // Phase 4: Overlay live events (highest precedence).
  //
  // Events are processed in array order; later events overwrite earlier
  // ones for the same step/unit (last-event-wins within the events slice).
  // ------------------------------------------------------------------
  for (const ev of events) {
    if (!isKnownRunEvent(ev)) continue;

    switch (ev.type) {
      case 'step_started':
      case 'step_working': {
        const node = nodeMap.get(ev.step_id);
        if (node) {
          node.state = 'running';
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
        } else {
          units.set(ev.index, {
            index: ev.index,
            key: ev.unit_key,
            state: 'running',
            transcriptPath: ev.transcript_path,
          });
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
      case 'run_started':
      case 'run_completed':
      case 'run_failed':
      case 'run_paused':
      case 'run_resumed':
        // Run-level events — no per-step state change needed here (the
        // in-flight step's own `step_paused`/`step_resumed` event, above,
        // carries the per-node transition).
        break;
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
  // Phase 6: Build edges — linear chain.
  // ------------------------------------------------------------------
  const nodes = g.workflow.steps
    .map((dto) => nodeMap.get(dto.id))
    .filter((n): n is GraphNode => n !== undefined);

  const edges: GraphEdge[] = [];
  for (let i = 0; i < nodes.length - 1; i++) {
    edges.push({ from: nodes[i].id, to: nodes[i + 1].id });
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
