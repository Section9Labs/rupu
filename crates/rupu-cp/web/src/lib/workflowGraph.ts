// workflowGraph — the PURE core of the visual workflow editor.
//
// Converts a parsed workflow object (a plain JS object as produced by js-yaml)
// to/from a graph model {nodes, edges, meta} and provides the graph algorithms
// the editor needs: topological sort (for deterministic serialization),
// connection validation (DAG enforcement), and step validation.
//
// This module is intentionally framework-free: no React, no @xyflow/react, no
// DOM. It operates on plain objects so it can be unit-tested in isolation and
// reused by any renderer. Workflow objects arrive from arbitrary YAML, so every
// field is narrowed defensively from `unknown` (see the helpers below) — we
// never assume a shape and never drop a step.

// ── Types ───────────────────────────────────────────────────────────────────

export type StepKind = 'step' | 'for_each' | 'parallel' | 'panel' | 'branch';

export interface SubStep {
  id: string;
  agent: string;
  prompt: string;
}

export interface PanelGate {
  // Field names mirror the real `PanelGate` schema in
  // crates/rupu-orchestrator/src/workflow.rs (serde deny_unknown_fields, no
  // aliases). Using any other names silently drops config on load AND is
  // rejected by Workflow::parse on save.
  until_no_findings_at_severity_or_above?: string;
  fix_with?: string;
  max_iterations?: number;
}

export interface PanelCfg {
  panelists: string[];
  subject: string;
  prompt?: string;
  max_parallel?: number;
  gate?: PanelGate;
}

export interface StepNodeData {
  id: string;
  kind: StepKind;
  agent?: string;
  prompt?: string;
  when?: string;
  continue_on_error?: boolean;
  actions?: string[];
  for_each?: string;
  max_parallel?: number;
  parallel?: SubStep[];
  panel?: PanelCfg;
  // Branch fields (workflow.rs `Branch`: condition / then / else). A branch
  // step carries no agent/prompt — routing is expressed entirely via these.
  condition?: string;
  thenTargets?: string[];
  elseTargets?: string[];
  approvalRequired?: boolean;
  // Approval.prompt / Approval.timeout_seconds (workflow.rs Approval). Preserved
  // so they survive a load→save round-trip; `approvalRequired` stays a distinct
  // boolean because it drives the form checkbox.
  approvalPrompt?: string;
  approvalTimeoutSeconds?: number;
  // Any step-level keys we don't model (e.g. `contract:`) are captured verbatim
  // here on load and spread back on emit, so unmodeled config is never dropped.
  raw_passthrough?: Record<string, unknown>;
}

export interface GraphNode {
  id: string;
  data: StepNodeData;
  position: { x: number; y: number };
}

export interface GraphEdge {
  id: string;
  source: string;
  target: string;
  // Set on branch-arm edges (yamlToGraph emits these from a `branch` node to
  // each of its then/else targets); absent on chain/data-ref edges.
  label?: string;
  branch?: 'then' | 'else';
}

export interface WorkflowMeta {
  name: string;
  description?: string;
  rest: Record<string, unknown>;
}

export interface WorkflowGraph {
  nodes: GraphNode[];
  edges: GraphEdge[];
  meta: WorkflowMeta;
}

// ── Narrowing helpers ─────────────────────────────────────────────────────────
// Small typed guards over `unknown`. Mirror the defensive style used in
// WorkflowDetail.tsx — never cast through `any`.

function asString(v: unknown): string | undefined {
  return typeof v === 'string' ? v : undefined;
}

function asNumber(v: unknown): number | undefined {
  return typeof v === 'number' && !Number.isNaN(v) ? v : undefined;
}

function asBool(v: unknown): boolean | undefined {
  return typeof v === 'boolean' ? v : undefined;
}

function asArray(v: unknown): unknown[] | undefined {
  return Array.isArray(v) ? v : undefined;
}

function asStringArray(v: unknown): string[] | undefined {
  const a = asArray(v);
  return a ? a.filter((x): x is string => typeof x === 'string') : undefined;
}

function asRecord(v: unknown): Record<string, unknown> | undefined {
  return typeof v === 'object' && v !== null && !Array.isArray(v) ? (v as Record<string, unknown>) : undefined;
}

// ── Parsing a single step (yaml object → StepNodeData) ────────────────────────

function parseSubStep(raw: unknown, j: number): SubStep {
  const o = asRecord(raw) ?? {};
  return {
    id: asString(o.id) ?? `sub-${j}`,
    agent: asString(o.agent) ?? '',
    prompt: asString(o.prompt) ?? '',
  };
}

function parsePanel(o: Record<string, unknown>): PanelCfg {
  const cfg: PanelCfg = {
    panelists: asStringArray(o.panelists) ?? [],
    subject: asString(o.subject) ?? '',
  };
  const prompt = asString(o.prompt);
  if (prompt !== undefined) cfg.prompt = prompt;
  const mp = asNumber(o.max_parallel);
  if (mp !== undefined) cfg.max_parallel = mp;
  const gateRaw = asRecord(o.gate);
  if (gateRaw) {
    const gate: PanelGate = {};
    const us = asString(gateRaw.until_no_findings_at_severity_or_above);
    if (us !== undefined) gate.until_no_findings_at_severity_or_above = us;
    const fx = asString(gateRaw.fix_with);
    if (fx !== undefined) gate.fix_with = fx;
    const mi = asNumber(gateRaw.max_iterations);
    if (mi !== undefined) gate.max_iterations = mi;
    cfg.gate = gate;
  }
  return cfg;
}

function parseStepData(raw: unknown, i: number): StepNodeData {
  const o = asRecord(raw) ?? {};
  const id = asString(o.id) ?? `step-${i}`;

  // Kind precedence: panel > parallel > branch > for_each > step. A step
  // matching none cleanly still becomes a plain `step` node carrying whatever
  // it has. A branch step has no agent/prompt of its own.
  const panelRaw = asRecord(o.panel);
  const parallelRaw = asArray(o.parallel);
  const branchRaw = asRecord(o.branch);
  const forEach = asString(o.for_each);
  let kind: StepKind = 'step';
  if (panelRaw) kind = 'panel';
  else if (parallelRaw) kind = 'parallel';
  else if (branchRaw) kind = 'branch';
  else if (forEach !== undefined) kind = 'for_each';

  const data: StepNodeData = { id, kind };

  const agent = asString(o.agent);
  if (agent !== undefined) data.agent = agent;
  const prompt = asString(o.prompt);
  if (prompt !== undefined) data.prompt = prompt;
  const when = asString(o.when);
  if (when !== undefined) data.when = when;
  const coe = asBool(o.continue_on_error);
  if (coe !== undefined) data.continue_on_error = coe;
  const actions = asStringArray(o.actions);
  if (actions && actions.length > 0) data.actions = actions;
  if (forEach !== undefined) data.for_each = forEach;
  const mp = asNumber(o.max_parallel);
  if (mp !== undefined) data.max_parallel = mp;
  if (parallelRaw) data.parallel = parallelRaw.map((s, j) => parseSubStep(s, j));
  if (panelRaw) data.panel = parsePanel(panelRaw);
  if (branchRaw) {
    const cond = asString(branchRaw.condition);
    if (cond !== undefined) data.condition = cond;
    const thenTargets = asStringArray(branchRaw.then);
    if (thenTargets && thenTargets.length > 0) data.thenTargets = thenTargets;
    const elseTargets = asStringArray(branchRaw.else);
    if (elseTargets && elseTargets.length > 0) data.elseTargets = elseTargets;
  }

  const approval = asRecord(o.approval);
  if (approval) {
    if (approval.required === true) data.approvalRequired = true;
    const ap = asString(approval.prompt);
    if (ap !== undefined) data.approvalPrompt = ap;
    const ats = asNumber(approval.timeout_seconds);
    if (ats !== undefined) data.approvalTimeoutSeconds = ats;
  }

  // Capture any step-level keys we don't model so they survive round-trips.
  const passthrough: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(o)) {
    if (!MODELLED_STEP_KEYS.has(k)) passthrough[k] = v;
  }
  if (Object.keys(passthrough).length > 0) data.raw_passthrough = passthrough;

  return data;
}

// Step-level keys this module models explicitly. Everything else (e.g.
// `contract:`) is captured into `raw_passthrough` on load and re-emitted on save.
const MODELLED_STEP_KEYS = new Set<string>([
  'id',
  'agent',
  'prompt',
  'when',
  'continue_on_error',
  'actions',
  'for_each',
  'max_parallel',
  'parallel',
  'panel',
  'branch',
  'approval',
]);

// ── extractStepRefs ───────────────────────────────────────────────────────────

const STEP_REF = /steps\.([A-Za-z0-9_-]+)/g;

/** Scan every template string carried by a node (prompt, for_each, when, each
 *  sub-step prompt, panel subject/prompt) for `steps.<id>` references and return
 *  the unique referenced ids. */
export function extractStepRefs(data: StepNodeData): string[] {
  const buckets: (string | undefined)[] = [data.prompt, data.for_each, data.when, data.condition];
  if (data.parallel) for (const s of data.parallel) buckets.push(s.prompt);
  if (data.panel) buckets.push(data.panel.subject, data.panel.prompt);

  const ids = new Set<string>();
  for (const t of buckets) {
    if (!t) continue;
    for (const m of t.matchAll(STEP_REF)) ids.add(m[1]);
  }
  return [...ids];
}

// ── yamlToGraph ───────────────────────────────────────────────────────────────

/** Convert a parsed workflow object into the graph model. */
export function yamlToGraph(obj: Record<string, unknown>): WorkflowGraph {
  // meta: name + description are surfaced; everything else top-level survives in
  // `rest` so a round-trip leaves trigger/inputs/defaults/autoflow/etc untouched.
  const rest: Record<string, unknown> = { ...obj };
  delete rest.name;
  delete rest.description;
  delete rest.steps;
  const meta: WorkflowMeta = { name: asString(obj.name) ?? '', rest };
  const desc = asString(obj.description);
  if (desc !== undefined) meta.description = desc;

  const stepsRaw = asArray(obj.steps) ?? [];
  const nodes: GraphNode[] = stepsRaw.map((s, i) => {
    const data = parseStepData(s, i);
    return { id: data.id, data, position: { x: 0, y: 0 } };
  });

  const ids = new Set(nodes.map((n) => n.id));
  const edges: GraphEdge[] = [];
  const seen = new Set<string>();
  const addEdge = (source: string, target: string, opts?: { label?: string; branch?: 'then' | 'else' }): void => {
    const label = opts?.label;
    // Label is part of the dedupe key so a labeled branch-arm edge never
    // collapses onto a plain chain/data-ref edge (or onto the other arm) that
    // happens to connect the same pair of nodes.
    const key = `${source}->${target}::${label ?? ''}`;
    if (source === target || seen.has(key)) return;
    seen.add(key);
    const id = opts?.branch ? `${source}->${target}:${opts.branch}` : `${source}->${target}`;
    const e: GraphEdge = { id, source, target };
    if (label !== undefined) e.label = label;
    if (opts?.branch !== undefined) e.branch = opts.branch;
    edges.push(e);
  };

  // (a) base chain edges for ordering, then (b) data-ref edges X->Y whenever Y
  // references steps.X and X exists. Dedupe collapses (b) onto (a).
  for (let i = 0; i < nodes.length - 1; i++) addEdge(nodes[i].id, nodes[i + 1].id);
  for (const n of nodes) {
    for (const ref of extractStepRefs(n.data)) {
      if (ids.has(ref)) addEdge(ref, n.id);
    }
  }

  // (c) branch-arm edges: a `branch` node points at each of its then/else
  // targets with a label so the renderer can draw true/false arms distinctly.
  for (const n of nodes) {
    if (n.data.kind !== 'branch') continue;
    for (const t of n.data.thenTargets ?? []) {
      if (ids.has(t)) addEdge(n.id, t, { label: 'true', branch: 'then' });
    }
    for (const t of n.data.elseTargets ?? []) {
      if (ids.has(t)) addEdge(n.id, t, { label: 'false', branch: 'else' });
    }
  }

  return { nodes, edges, meta };
}

// ── topoSort ──────────────────────────────────────────────────────────────────

/** Kahn's algorithm with a deterministic tiebreak. Among in-degree-0 "ready"
 *  nodes we always pick the one with the smallest position.y, then position.x,
 *  then id (lexicographic) — so the output is stable and layout-aware. Returns
 *  the remaining node ids as `{ cycle }` if the graph isn't a DAG. */
export function topoSort(
  nodes: GraphNode[],
  edges: GraphEdge[],
): { order: GraphNode[] } | { cycle: string[] } {
  const byId = new Map<string, GraphNode>();
  const indeg = new Map<string, number>();
  for (const n of nodes) {
    byId.set(n.id, n);
    indeg.set(n.id, 0);
  }
  const adj = new Map<string, string[]>();
  for (const e of edges) {
    if (!byId.has(e.source) || !byId.has(e.target)) continue;
    const list = adj.get(e.source);
    if (list) list.push(e.target);
    else adj.set(e.source, [e.target]);
    indeg.set(e.target, (indeg.get(e.target) ?? 0) + 1);
  }

  const cmp = (a: GraphNode, b: GraphNode): number =>
    a.position.y - b.position.y ||
    a.position.x - b.position.x ||
    (a.id < b.id ? -1 : a.id > b.id ? 1 : 0);

  const order: GraphNode[] = [];
  const ready: GraphNode[] = nodes.filter((n) => (indeg.get(n.id) ?? 0) === 0);
  while (ready.length > 0) {
    ready.sort(cmp);
    const n = ready.shift() as GraphNode;
    order.push(n);
    for (const t of adj.get(n.id) ?? []) {
      const d = (indeg.get(t) ?? 0) - 1;
      indeg.set(t, d);
      if (d === 0) {
        const tn = byId.get(t);
        if (tn) ready.push(tn);
      }
    }
  }

  if (order.length !== byId.size) {
    const done = new Set(order.map((n) => n.id));
    return { cycle: nodes.filter((n) => !done.has(n.id)).map((n) => n.id) };
  }
  return { order };
}

// ── graphToWorkflowObject ─────────────────────────────────────────────────────

/** Serialize one node back to a YAML-step object, including ONLY set fields
 *  (undefined / empty arrays / empty strings are omitted). */
function nodeToStepObject(d: StepNodeData): Record<string, unknown> {
  const o: Record<string, unknown> = { id: d.id };

  if (d.kind === 'parallel') {
    o.parallel = (d.parallel ?? []).map((s) => {
      const so: Record<string, unknown> = { id: s.id };
      if (s.agent) so.agent = s.agent;
      if (s.prompt) so.prompt = s.prompt;
      return so;
    });
    if (d.max_parallel !== undefined) o.max_parallel = d.max_parallel;
  } else if (d.kind === 'panel') {
    const p = d.panel ?? { panelists: [], subject: '' };
    const po: Record<string, unknown> = { panelists: p.panelists, subject: p.subject };
    if (p.prompt) po.prompt = p.prompt;
    if (p.max_parallel !== undefined) po.max_parallel = p.max_parallel;
    if (p.gate) {
      const go: Record<string, unknown> = {};
      if (p.gate.until_no_findings_at_severity_or_above !== undefined) {
        go.until_no_findings_at_severity_or_above = p.gate.until_no_findings_at_severity_or_above;
      }
      if (p.gate.fix_with !== undefined) go.fix_with = p.gate.fix_with;
      if (p.gate.max_iterations !== undefined) go.max_iterations = p.gate.max_iterations;
      po.gate = go;
    }
    o.panel = po;
  } else if (d.kind === 'branch') {
    const bo: Record<string, unknown> = {};
    if (d.condition !== undefined) bo.condition = d.condition;
    if (d.thenTargets && d.thenTargets.length > 0) bo.then = d.thenTargets;
    if (d.elseTargets && d.elseTargets.length > 0) bo.else = d.elseTargets;
    o.branch = bo;
  } else {
    // step / for_each
    if (d.agent) o.agent = d.agent;
    if (d.prompt) o.prompt = d.prompt;
    if (d.when) o.when = d.when;
    if (d.continue_on_error === true) o.continue_on_error = true;
    if (d.actions && d.actions.length > 0) o.actions = d.actions;
    if (d.for_each) o.for_each = d.for_each;
    if (d.max_parallel !== undefined) o.max_parallel = d.max_parallel;
  }

  // Approval applies to any step kind. Emit only when there's something to say,
  // preserving the optional prompt/timeout alongside `required`.
  if (d.approvalRequired || d.approvalPrompt !== undefined || d.approvalTimeoutSeconds !== undefined) {
    const ap: Record<string, unknown> = {};
    if (d.approvalRequired) ap.required = true;
    if (d.approvalPrompt !== undefined) ap.prompt = d.approvalPrompt;
    if (d.approvalTimeoutSeconds !== undefined) ap.timeout_seconds = d.approvalTimeoutSeconds;
    o.approval = ap;
  }

  // Spread unmodeled keys (e.g. `contract:`) back, never clobbering modeled ones.
  if (d.raw_passthrough) {
    for (const [k, v] of Object.entries(d.raw_passthrough)) {
      if (!(k in o)) o[k] = v;
    }
  }

  return o;
}

/** Serialize the graph back to a workflow object. Steps are emitted in topo
 *  order (so the YAML reads top-to-bottom in execution order). Key order of the
 *  result is the round-trip contract: `name` first, then `description` (if set),
 *  then all `meta.rest` keys verbatim, then `steps` last. */
export function graphToWorkflowObject(
  g: WorkflowGraph,
): { obj: Record<string, unknown> } | { error: string } {
  const sorted = topoSort(g.nodes, g.edges);
  if ('cycle' in sorted) {
    return { error: 'Cannot serialize: cycle through ' + sorted.cycle.join(', ') };
  }

  const steps = sorted.order.map((n) => nodeToStepObject(n.data));

  const obj: Record<string, unknown> = {};
  obj.name = g.meta.name;
  if (g.meta.description !== undefined) obj.description = g.meta.description;
  for (const [k, v] of Object.entries(g.meta.rest)) obj[k] = v;
  obj.steps = steps;
  return { obj };
}

// ── canConnect ────────────────────────────────────────────────────────────────

/** Whether dragging a new edge source→target is allowed: no self-loops, no
 *  duplicates, and the result must stay a DAG. We reject if `target` can already
 *  reach `source` (a DFS over existing edges) — adding source→target would then
 *  close a cycle. */
export function canConnect(
  source: string,
  target: string,
  g: { edges: GraphEdge[] },
): { ok: true } | { ok: false; reason: string } {
  if (source === target) return { ok: false, reason: "A step can't depend on itself." };
  if (g.edges.some((e) => e.source === source && e.target === target)) {
    return { ok: false, reason: 'These steps are already connected.' };
  }

  const adj = new Map<string, string[]>();
  for (const e of g.edges) {
    const list = adj.get(e.source);
    if (list) list.push(e.target);
    else adj.set(e.source, [e.target]);
  }

  const seen = new Set<string>();
  const stack = [target];
  while (stack.length > 0) {
    const cur = stack.pop() as string;
    if (cur === source) {
      return { ok: false, reason: 'This would create a cycle — steps must form a DAG.' };
    }
    if (seen.has(cur)) continue;
    seen.add(cur);
    for (const t of adj.get(cur) ?? []) stack.push(t);
  }
  return { ok: true };
}

// ── validateGraph ─────────────────────────────────────────────────────────────

/** Return a map of nodeId → human-readable problems. Only nodes that HAVE
 *  problems appear in the map (a clean graph → {}). */
export function validateGraph(g: WorkflowGraph): Record<string, string[]> {
  const out: Record<string, string[]> = {};
  const add = (id: string, msg: string): void => {
    const list = out[id];
    if (list) list.push(msg);
    else out[id] = [msg];
  };

  const counts = new Map<string, number>();
  for (const n of g.nodes) counts.set(n.id, (counts.get(n.id) ?? 0) + 1);
  const nodeIds = new Set(g.nodes.map((n) => n.id));

  for (const n of g.nodes) {
    const d = n.data;
    if (d.kind === 'step' || d.kind === 'for_each') {
      if (!d.agent) add(n.id, 'needs an agent');
      if (!d.prompt) add(n.id, 'needs a prompt');
    } else if (d.kind === 'parallel') {
      const subs = d.parallel ?? [];
      if (subs.length === 0) {
        add(n.id, 'needs at least one parallel sub-step');
      } else {
        subs.forEach((s, i) => {
          const label = s.id || `#${i}`;
          if (!s.agent) add(n.id, `parallel sub-step ${label} needs an agent`);
          if (!s.prompt) add(n.id, `parallel sub-step ${label} needs a prompt`);
        });
      }
    } else if (d.kind === 'panel') {
      const p = d.panel;
      if (!p || p.panelists.length === 0) add(n.id, 'panel needs at least one panelist');
      if (!p || !p.subject) add(n.id, 'panel needs a subject');
    } else if (d.kind === 'branch') {
      if (!d.condition) add(n.id, 'branch needs a condition');
      for (const t of [...(d.thenTargets ?? []), ...(d.elseTargets ?? [])]) {
        if (!nodeIds.has(t)) add(n.id, `branch target ${t} is not a known step`);
      }
    }
    if ((counts.get(n.id) ?? 0) > 1) add(n.id, 'duplicate step id');
  }

  // Reference checks: dangling refs (steps.X where X is not a node) and forward
  // refs (X runs AFTER the referencing node — only checkable when there's no
  // cycle, since order is otherwise undefined).
  const sorted = topoSort(g.nodes, g.edges);
  const pos = 'order' in sorted ? new Map(sorted.order.map((n, i) => [n.id, i])) : undefined;
  for (const n of g.nodes) {
    const here = pos?.get(n.id);
    for (const ref of extractStepRefs(n.data)) {
      if (!nodeIds.has(ref)) {
        add(n.id, `references unknown step ${ref}`);
        continue;
      }
      if (pos && here !== undefined) {
        const there = pos.get(ref);
        if (there !== undefined && there > here) {
          add(n.id, `references steps.${ref} which runs later`);
        }
      }
    }
  }

  return out;
}
