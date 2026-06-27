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

export type StepKind = 'step' | 'for_each' | 'parallel' | 'panel';

export interface SubStep {
  id: string;
  agent: string;
  prompt: string;
}

export interface PanelGate {
  until_severity?: string;
  max_iterations?: number;
  fixer?: string;
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
  approvalRequired?: boolean;
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
    const us = asString(gateRaw.until_severity);
    if (us !== undefined) gate.until_severity = us;
    const mi = asNumber(gateRaw.max_iterations);
    if (mi !== undefined) gate.max_iterations = mi;
    const fx = asString(gateRaw.fixer);
    if (fx !== undefined) gate.fixer = fx;
    cfg.gate = gate;
  }
  return cfg;
}

function parseStepData(raw: unknown, i: number): StepNodeData {
  const o = asRecord(raw) ?? {};
  const id = asString(o.id) ?? `step-${i}`;

  // Kind precedence: panel > parallel > for_each > step. A step matching none
  // cleanly still becomes a plain `step` node carrying whatever it has.
  const panelRaw = asRecord(o.panel);
  const parallelRaw = asArray(o.parallel);
  const forEach = asString(o.for_each);
  let kind: StepKind = 'step';
  if (panelRaw) kind = 'panel';
  else if (parallelRaw) kind = 'parallel';
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
  const approval = asRecord(o.approval);
  if (approval && approval.required === true) data.approvalRequired = true;

  return data;
}

// ── extractStepRefs ───────────────────────────────────────────────────────────

const STEP_REF = /steps\.([A-Za-z0-9_-]+)/g;

/** Scan every template string carried by a node (prompt, for_each, when, each
 *  sub-step prompt, panel subject/prompt) for `steps.<id>` references and return
 *  the unique referenced ids. */
export function extractStepRefs(data: StepNodeData): string[] {
  const buckets: (string | undefined)[] = [data.prompt, data.for_each, data.when];
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
  const addEdge = (source: string, target: string): void => {
    const key = `${source}->${target}`;
    if (source === target || seen.has(key)) return;
    seen.add(key);
    edges.push({ id: key, source, target });
  };

  // (a) base chain edges for ordering, then (b) data-ref edges X->Y whenever Y
  // references steps.X and X exists. Dedupe collapses (b) onto (a).
  for (let i = 0; i < nodes.length - 1; i++) addEdge(nodes[i].id, nodes[i + 1].id);
  for (const n of nodes) {
    for (const ref of extractStepRefs(n.data)) {
      if (ids.has(ref)) addEdge(ref, n.id);
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
    return o;
  }

  if (d.kind === 'panel') {
    const p = d.panel ?? { panelists: [], subject: '' };
    const po: Record<string, unknown> = { panelists: p.panelists, subject: p.subject };
    if (p.prompt) po.prompt = p.prompt;
    if (p.max_parallel !== undefined) po.max_parallel = p.max_parallel;
    if (p.gate) {
      const go: Record<string, unknown> = {};
      if (p.gate.until_severity !== undefined) go.until_severity = p.gate.until_severity;
      if (p.gate.max_iterations !== undefined) go.max_iterations = p.gate.max_iterations;
      if (p.gate.fixer !== undefined) go.fixer = p.gate.fixer;
      po.gate = go;
    }
    o.panel = po;
    return o;
  }

  // step / for_each
  if (d.agent) o.agent = d.agent;
  if (d.prompt) o.prompt = d.prompt;
  if (d.when) o.when = d.when;
  if (d.continue_on_error === true) o.continue_on_error = true;
  if (d.actions && d.actions.length > 0) o.actions = d.actions;
  if (d.for_each) o.for_each = d.for_each;
  if (d.max_parallel !== undefined) o.max_parallel = d.max_parallel;
  if (d.approvalRequired) o.approval = { required: true };
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
    }
    if ((counts.get(n.id) ?? 0) > 1) add(n.id, 'duplicate step id');
  }

  // Forward references: a node references steps.X where X runs AFTER it.
  const sorted = topoSort(g.nodes, g.edges);
  if ('order' in sorted) {
    const pos = new Map<string, number>();
    sorted.order.forEach((n, i) => pos.set(n.id, i));
    for (const n of g.nodes) {
      const here = pos.get(n.id);
      if (here === undefined) continue;
      for (const ref of extractStepRefs(n.data)) {
        const there = pos.get(ref);
        if (there !== undefined && there > here) {
          add(n.id, `references steps.${ref} which runs later`);
        }
      }
    }
  }

  return out;
}
