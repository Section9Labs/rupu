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

export type StepKind =
  | 'step'
  | 'for_each'
  | 'parallel'
  | 'panel'
  | 'branch'
  | 'approval_gate'
  | 'action'
  | 'split'
  | 'join';

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
  // Any panel-level keys we don't model, captured verbatim on load and spread
  // back on emit — mirrors the step-level `raw_passthrough` pattern one level
  // deeper so an unmodeled key nested directly under `panel:` isn't dropped.
  _rest?: Record<string, unknown>;
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
  // Standalone approval-GATE fields (workflow.rs Approval, only meaningful on a
  // gate NODE — a step with `approval:` and no agent/prompt/for_each/parallel/
  // panel/branch/action). `notify` / `on_reject` are preserved verbatim as raw
  // step objects so their full shape (action steps, extra keys) round-trips even
  // though the form edits only a subset.
  approvalAutoApprove?: string;
  approvalOnTimeout?: 'approve' | 'reject' | 'fail';
  approvalNotify?: Record<string, unknown>[];
  approvalOnReject?: Record<string, unknown>[];
  // Any keys nested directly under `branch:` / `approval:` we don't model,
  // captured verbatim on load and spread back on emit — mirrors the
  // step-level `raw_passthrough` pattern one level deeper.
  branchRest?: Record<string, unknown>;
  approvalRest?: Record<string, unknown>;
  // Connector ACTION-step fields (workflow.rs Step.action / Step.with). An action
  // node carries no agent/prompt — it invokes an SCM/issue/CI tool with params.
  action?: string;
  with?: Record<string, unknown>;
  // Non-linear orchestration fields (workflow.rs `Step.next`/`Step.split`/
  // `Step.join`, Phase 1 language). `next` is a general field any step kind
  // may carry (its explicit successor edges); `split`/`join` are mutually
  // exclusive with agent/action/for_each/parallel/panel/branch/approval and
  // each imply their own `kind` (see parseStepData). `joinWait` mirrors Rust's
  // `JoinWait`: the bare keyword `'all'`/`'any'`, or the `{ count }` form.
  next?: string[];
  split?: string[];
  joinWait?: 'all' | 'any' | { count: number };
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
  const rest: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(o)) {
    if (!PANEL_KEYS.has(k)) rest[k] = v;
  }
  if (Object.keys(rest).length > 0) cfg._rest = rest;
  return cfg;
}

// Keys this module models on a `panel:` block. Everything else is captured
// into `PanelCfg._rest` on load and re-emitted on save.
const PANEL_KEYS = new Set<string>(['panelists', 'subject', 'prompt', 'max_parallel', 'gate']);

/** Parse a `join.wait` value into the `StepNodeData.joinWait` shape. Mirrors
 *  workflow.rs `JoinWait`'s `#[serde(untagged)]`: try the bare keyword
 *  (`"all"`/`"any"`) first, then the `{ count }` map form. Returns `undefined`
 *  for anything else (including absent — Rust defaults to `all` at execution
 *  time, but the editor preserves "not specified" rather than synthesizing a
 *  value that wasn't in the source, so a bare `join: {}` round-trips as-is). */
function parseJoinWait(v: unknown): 'all' | 'any' | { count: number } | undefined {
  const kw = asString(v);
  if (kw === 'all' || kw === 'any') return kw;
  const rec = asRecord(v);
  if (rec) {
    const count = asNumber(rec.count);
    if (count !== undefined) return { count };
  }
  return undefined;
}

function parseStepData(raw: unknown, i: number): StepNodeData {
  const o = asRecord(raw) ?? {};
  const id = asString(o.id) ?? `step-${i}`;

  // Kind precedence (most-specific first): panel > parallel > branch > split >
  // join > action > for_each > approval_gate > step. A step matching none
  // cleanly still becomes a plain `step` node carrying whatever it has. A
  // branch/split/join/gate/action step has no agent/prompt of its own. `split`/
  // `join` mirror workflow.rs `validate_graph`'s `is_orch` check (a step
  // declaring `split:`/`join:` carries no agent/action/for_each/parallel/
  // panel/branch/approval work of its own) — presence of the key, even an
  // empty `split: []` or bare `join: {}`, is what makes the node that kind,
  // same as the `parallel`/`panel`/`branch` arms above it. The gate arm
  // mirrors workflow.rs `is_approval_gate`: an `approval:` block AND no
  // agent/prompt/for_each/parallel/panel/branch/action (the earlier arms
  // already peeled those shapes off, so here we only re-check agent/prompt).
  const panelRaw = asRecord(o.panel);
  const parallelRaw = asArray(o.parallel);
  const branchRaw = asRecord(o.branch);
  const splitRaw = asStringArray(o.split);
  const joinRaw = asRecord(o.join);
  const actionName = asString(o.action);
  const forEach = asString(o.for_each);
  const approvalRaw = asRecord(o.approval);
  const agentName = asString(o.agent);
  const promptText = asString(o.prompt);
  let kind: StepKind = 'step';
  if (panelRaw) kind = 'panel';
  else if (parallelRaw) kind = 'parallel';
  else if (branchRaw) kind = 'branch';
  else if (splitRaw !== undefined) kind = 'split';
  else if (joinRaw) kind = 'join';
  else if (actionName !== undefined) kind = 'action';
  else if (forEach !== undefined) kind = 'for_each';
  else if (approvalRaw && agentName === undefined && promptText === undefined) kind = 'approval_gate';

  const data: StepNodeData = { id, kind };

  if (splitRaw !== undefined) data.split = splitRaw;
  if (joinRaw) {
    const wait = parseJoinWait(joinRaw.wait);
    if (wait !== undefined) data.joinWait = wait;
  }
  const nextArr = asStringArray(o.next);
  if (nextArr && nextArr.length > 0) data.next = nextArr;

  if (agentName !== undefined) data.agent = agentName;
  if (promptText !== undefined) data.prompt = promptText;
  if (actionName !== undefined) data.action = actionName;
  const withRaw = asRecord(o.with);
  if (withRaw !== undefined) data.with = withRaw;
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
    const branchRest: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(branchRaw)) {
      if (!BRANCH_KEYS.has(k)) branchRest[k] = v;
    }
    if (Object.keys(branchRest).length > 0) data.branchRest = branchRest;
  }

  const approval = approvalRaw;
  if (approval) {
    if (approval.required === true) data.approvalRequired = true;
    const ap = asString(approval.prompt);
    if (ap !== undefined) data.approvalPrompt = ap;
    const ats = asNumber(approval.timeout_seconds);
    if (ats !== undefined) data.approvalTimeoutSeconds = ats;
    const aa = asString(approval.auto_approve);
    if (aa !== undefined) data.approvalAutoApprove = aa;
    const ot = asString(approval.on_timeout);
    if (ot === 'approve' || ot === 'reject' || ot === 'fail') data.approvalOnTimeout = ot;
    const notify = asArray(approval.notify);
    if (notify) data.approvalNotify = notify.map((n) => asRecord(n) ?? {});
    const onReject = asArray(approval.on_reject);
    if (onReject) data.approvalOnReject = onReject.map((s) => asRecord(s) ?? {});
    const approvalRest: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(approval)) {
      if (!APPROVAL_KEYS.has(k)) approvalRest[k] = v;
    }
    if (Object.keys(approvalRest).length > 0) data.approvalRest = approvalRest;
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
  'action',
  'with',
  'next',
  'split',
  'join',
]);

// Keys this module models on a `branch:` block. Everything else is captured
// into `StepNodeData.branchRest` on load and re-emitted on save.
const BRANCH_KEYS = new Set<string>(['condition', 'then', 'else']);

// Keys this module models on an `approval:` block. Everything else is
// captured into `StepNodeData.approvalRest` on load and re-emitted on save.
const APPROVAL_KEYS = new Set<string>([
  'required',
  'prompt',
  'timeout_seconds',
  'auto_approve',
  'on_timeout',
  'notify',
  'on_reject',
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

// ── deriveEdges ───────────────────────────────────────────────────────────────

/** A workflow "has explicit edges" (is a graph workflow rather than a legacy
 *  linear one) when any node declares `next:`, `split:`, or `join:` — mirrors
 *  workflow.rs `workflow_has_explicit_edges` exactly (non-empty `next`,
 *  `split` present (any array, even empty), `join` present). Legacy edge-free
 *  workflows keep displaying as the pre-existing linear chain (see
 *  `deriveEdges`). Exported so callers (Tasks 5-7: the editor canvas, node
 *  forms, add/connect affordances) can branch on the same distinction. */
export function hasExplicitEdges(nodes: GraphNode[]): boolean {
  return nodes.some((n) => (n.data.next && n.data.next.length > 0) || n.data.kind === 'split' || n.data.kind === 'join');
}

/** The legacy->graph migration primitive (spec 3d): a no-op when `nodes`
 *  already has explicit edges (`hasExplicitEdges`); otherwise returns a copy
 *  where every node's `next` is set to exactly what `deriveEdges`'s legacy
 *  chain loop would already have drawn for it — `[the next node in list
 *  order]`, and `[]` for the last node — so the derived edge SET is identical
 *  before and after, just now explicit instead of implicit.
 *
 *  A `branch` node is left untouched (no `next` written, even `[]`): its
 *  routing is its `then`/`else` targets, and `next` is not part of that
 *  node's identity the way it is for every other kind.
 *
 *  Exists to fix the Critical bug where authoring the FIRST explicit edge on
 *  a legacy multi-step workflow (a single `applyConnect`/
 *  `applyAddConnectedNext` draw) flips `hasExplicitEdges` for the WHOLE
 *  graph, and every OTHER node — which still has no explicit `next` of its
 *  own — silently loses the implicit chain edge it used to derive from list
 *  order. Callers run the current nodes through this BEFORE mutating the
 *  node that's about to gain the new edge, so the pre-existing connections
 *  survive as explicit `next` entries and the new edge applies on top of
 *  that materialized base rather than a graph that just lost the rest of its
 *  chain. */
export function materializeLegacyChain(nodes: GraphNode[]): GraphNode[] {
  if (hasExplicitEdges(nodes)) return nodes;
  return nodes.map((n, i) => {
    if (n.data.kind === 'branch') return n;
    const next = i < nodes.length - 1 ? [nodes[i + 1].id] : [];
    return { ...n, data: { ...n.data, next } };
  });
}

/** The ONE producer of canvas edges. Pure function of the ordered node list,
 *  branching on `hasExplicitEdges`:
 *
 *  **Graph mode** (any node has `next`/`split`/`join`): edges come ONLY from
 *  explicit connections — (a) each node's `next` targets, (b) a `split`
 *  node's fan-out targets, (c) a `branch` node's then/else arms — UNION (d)
 *  inferred data-ref edges (X->Y whenever Y references steps.X). List order
 *  contributes NOTHING; there is no consecutive-pair chain.
 *
 *  **Legacy mode** (no node has any of those): today's pre-existing
 *  behavior, unchanged — (a) a chain edge between each consecutive pair
 *  (declared order), (b) the same data-ref inference, (c) the same
 *  branch-arm edges. This is the compat guarantee: an edge-free workflow
 *  authored before non-linear orchestration existed still displays as the
 *  linear chain it always did.
 *
 *  graph.edges is ALWAYS this — never stored independently. */
export function deriveEdges(nodes: GraphNode[]): GraphEdge[] {
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

  const graphMode = hasExplicitEdges(nodes);

  if (graphMode) {
    // (a) explicit `next` edges, (b) `split` fan-out edges.
    for (const n of nodes) {
      for (const t of n.data.next ?? []) {
        if (ids.has(t)) addEdge(n.id, t);
      }
      if (n.data.kind === 'split') {
        for (const t of n.data.split ?? []) {
          if (ids.has(t)) addEdge(n.id, t);
        }
      }
    }
  } else {
    // Legacy: a chain edge between each consecutive pair (declared order).
    for (let i = 0; i < nodes.length - 1; i++) addEdge(nodes[i].id, nodes[i + 1].id);
  }

  // Data-ref edges X->Y whenever Y references steps.X and X exists — inferred
  // in BOTH modes. Dedupe collapses onto any chain/next/split edge that
  // already connects the same pair.
  for (const n of nodes) {
    for (const ref of extractStepRefs(n.data)) {
      if (ids.has(ref)) addEdge(ref, n.id);
    }
  }

  // Branch-arm edges: a `branch` node points at each of its then/else targets
  // with a label so the renderer can draw true/false arms distinctly — in
  // BOTH modes (a branch is an explicit connection either way).
  for (const n of nodes) {
    if (n.data.kind !== 'branch') continue;
    for (const t of n.data.thenTargets ?? []) {
      if (ids.has(t)) addEdge(n.id, t, { label: 'true', branch: 'then' });
    }
    for (const t of n.data.elseTargets ?? []) {
      if (ids.has(t)) addEdge(n.id, t, { label: 'false', branch: 'else' });
    }
  }

  return edges;
}

/** Build a WorkflowGraph whose edges are derived from its nodes — the only
 *  correct way to construct/return a graph. */
export function withDerivedEdges(meta: WorkflowMeta, nodes: GraphNode[]): WorkflowGraph {
  return { meta, nodes, edges: deriveEdges(nodes) };
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

  return { nodes, edges: deriveEdges(nodes), meta };
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
    if (p._rest) {
      for (const [k, v] of Object.entries(p._rest)) {
        if (!(k in po)) po[k] = v;
      }
    }
    o.panel = po;
    if (d.when) o.when = d.when;
    if (d.continue_on_error === true) o.continue_on_error = true;
    if (d.actions && d.actions.length > 0) o.actions = d.actions;
  } else if (d.kind === 'branch') {
    const bo: Record<string, unknown> = {};
    if (d.condition !== undefined) bo.condition = d.condition;
    if (d.thenTargets && d.thenTargets.length > 0) bo.then = d.thenTargets;
    if (d.elseTargets && d.elseTargets.length > 0) bo.else = d.elseTargets;
    if (d.branchRest) {
      for (const [k, v] of Object.entries(d.branchRest)) {
        if (!(k in bo)) bo[k] = v;
      }
    }
    o.branch = bo;
  } else if (d.kind === 'action') {
    // action step — `action:` (tool name) + optional `with:` params, plus the
    // shared when/continue_on_error a linear step also carries.
    if (d.action) o.action = d.action;
    if (d.with !== undefined) o.with = d.with;
    if (d.when) o.when = d.when;
    if (d.continue_on_error === true) o.continue_on_error = true;
    if (d.actions && d.actions.length > 0) o.actions = d.actions;
  } else if (d.kind === 'split') {
    // `split` orchestration node — its whole identity is the `split:` array
    // (fan-out targets); it carries no agent/prompt/action of its own.
    o.split = d.split ?? [];
    if (d.when) o.when = d.when;
    if (d.continue_on_error === true) o.continue_on_error = true;
    if (d.actions && d.actions.length > 0) o.actions = d.actions;
  } else if (d.kind === 'join') {
    // `join` (barrier) orchestration node — its whole identity is the
    // `join:` block; it carries no agent/prompt/action of its own. `wait` is
    // omitted when not set on load, so a bare `join: {}` round-trips as-is
    // rather than gaining a synthesized default (see `parseJoinWait`).
    const jo: Record<string, unknown> = {};
    if (d.joinWait !== undefined) jo.wait = d.joinWait;
    o.join = jo;
    if (d.when) o.when = d.when;
    if (d.continue_on_error === true) o.continue_on_error = true;
    if (d.actions && d.actions.length > 0) o.actions = d.actions;
  } else if (d.kind === 'approval_gate') {
    // standalone gate NODE — its whole identity is the `approval:` block emitted
    // below; it carries no agent/prompt. `when` is still valid on a gate, so
    // preserve it (it lives in MODELLED_STEP_KEYS, i.e. never in raw_passthrough).
    if (d.when) o.when = d.when;
    if (d.continue_on_error === true) o.continue_on_error = true;
    if (d.actions && d.actions.length > 0) o.actions = d.actions;
  } else {
    // step / for_each
    if (d.agent) o.agent = d.agent;
    if (d.prompt) o.prompt = d.prompt;
    if (d.when) o.when = d.when;
    if (d.continue_on_error === true) o.continue_on_error = true;
    if (d.actions && d.actions.length > 0) o.actions = d.actions;
    if (d.for_each) o.for_each = d.for_each;
    if (d.max_parallel !== undefined) o.max_parallel = d.max_parallel;
    // `with` is normally only meaningful alongside `action:` (see the action
    // arm above), but it's in MODELLED_STEP_KEYS (so never falls into
    // raw_passthrough either) — emit it here too so a step/for_each node that
    // somehow carries one (e.g. hand-authored YAML with a stray `with:`)
    // still round-trips instead of silently vanishing. Schema-invalid on save
    // either way (workflow.rs rejects `with:` without `action:`), but dropping
    // data silently is worse than a validation error the user can act on.
    if (d.with !== undefined) o.with = d.with;
  }

  // `next` (explicit successor edges) applies to any step kind — omitted
  // when empty so a legacy node (no `next:` on load) round-trips clean.
  if (d.next && d.next.length > 0) o.next = d.next;

  // Approval applies to any step kind. A gate NODE ALWAYS emits an `approval:`
  // block (it is the node's identity); other kinds emit only when there's an
  // inline approval to say. The gate-only fields (auto_approve / on_timeout /
  // notify / on_reject) round-trip verbatim.
  const hasGateExtras =
    d.approvalAutoApprove !== undefined ||
    d.approvalOnTimeout !== undefined ||
    (d.approvalNotify !== undefined && d.approvalNotify.length > 0) ||
    (d.approvalOnReject !== undefined && d.approvalOnReject.length > 0);
  const hasApprovalRest = d.approvalRest !== undefined && Object.keys(d.approvalRest).length > 0;
  if (
    d.kind === 'approval_gate' ||
    d.approvalRequired ||
    d.approvalPrompt !== undefined ||
    d.approvalTimeoutSeconds !== undefined ||
    hasGateExtras ||
    hasApprovalRest
  ) {
    const ap: Record<string, unknown> = {};
    if (d.approvalRequired) ap.required = true;
    if (d.approvalPrompt !== undefined) ap.prompt = d.approvalPrompt;
    if (d.approvalTimeoutSeconds !== undefined) ap.timeout_seconds = d.approvalTimeoutSeconds;
    if (d.approvalAutoApprove !== undefined) ap.auto_approve = d.approvalAutoApprove;
    if (d.approvalOnTimeout !== undefined) ap.on_timeout = d.approvalOnTimeout;
    if (d.approvalNotify !== undefined && d.approvalNotify.length > 0) ap.notify = d.approvalNotify;
    if (d.approvalOnReject !== undefined && d.approvalOnReject.length > 0) ap.on_reject = d.approvalOnReject;
    if (d.approvalRest) {
      for (const [k, v] of Object.entries(d.approvalRest)) {
        if (!(k in ap)) ap[k] = v;
      }
    }
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
  const sorted = topoSort(g.nodes, deriveEdges(g.nodes));
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
 *  close a cycle.
 *
 *  `arm` distinguishes WHICH logical edge is being drawn: a plain connect
 *  (`arm` undefined) duplicate-checks only against other plain edges; a
 *  branch-arm connect (`arm: 'then' | 'else'`) duplicate-checks only against
 *  an existing edge tagged with that SAME arm. Without this, a branch node
 *  that sits array-adjacent to its intended then/else target would always
 *  find the auto-derived chain edge between them and reject the arm connect
 *  as "already connected" — under the derived-edges model every consecutive
 *  node pair carries a chain edge, so a branch arm and a chain edge to the
 *  same target are two DISTINCT logical edges, not a duplicate. */
export function canConnect(
  source: string,
  target: string,
  g: { edges: GraphEdge[] },
  arm?: 'then' | 'else',
): { ok: true } | { ok: false; reason: string; kind: 'self' | 'duplicate' | 'cycle' } {
  if (source === target) return { ok: false, reason: "A step can't depend on itself.", kind: 'self' };
  if (g.edges.some((e) => e.source === source && e.target === target && (e.branch ?? undefined) === arm)) {
    return { ok: false, reason: 'These steps are already connected.', kind: 'duplicate' };
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
      return { ok: false, reason: 'This would create a cycle — steps must form a DAG.', kind: 'cycle' };
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
      if (p?.gate) {
        const gate = p.gate;
        if (!gate.until_no_findings_at_severity_or_above || !gate.fix_with || gate.max_iterations === undefined) {
          add(n.id, 'gate needs a severity, a fix agent, and max iterations');
        }
      }
      if (p?.max_parallel !== undefined && p.max_parallel < 1) {
        add(n.id, 'panel `max_parallel` must be at least 1');
      }
    } else if (d.kind === 'branch') {
      if (!d.condition) add(n.id, 'branch needs a condition');
      for (const t of [...(d.thenTargets ?? []), ...(d.elseTargets ?? [])]) {
        if (!nodeIds.has(t)) add(n.id, `branch target ${t} is not a known step`);
      }
    }
    if (d.max_parallel !== undefined && d.max_parallel < 1) add(n.id, '`max_parallel` must be at least 1');
    if ((counts.get(n.id) ?? 0) > 1) add(n.id, 'duplicate step id');

    // A `notify` row with no `action` (backend `NotifyAction.action` is
    // required) 400s on save rather than failing validation up-front.
    if (d.approvalNotify) {
      d.approvalNotify.forEach((entry, i) => {
        if (!entry.action || entry.action === '') add(n.id, `notification ${i + 1} needs an action`);
      });
    }
    // An action-shaped `on_reject` row (identified by having an `action` key
    // at all, vs. an agent-shaped row) with an empty `action` is the same
    // required-field gap as above. An agent-shaped row (agent/prompt) is
    // validated elsewhere / pre-existing and is left alone here.
    if (d.approvalOnReject) {
      d.approvalOnReject.forEach((entry, i) => {
        if ('action' in entry && (!entry.action || entry.action === '')) {
          add(n.id, `on_reject entry ${i + 1} needs an action`);
        }
      });
    }
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

  // Graph-mode checks (Phase 1 non-linear orchestration): only meaningful once
  // a workflow has explicit edges at all — a legacy edge-free workflow can't
  // author a cycle/dangling-target/degenerate-split-join through this
  // vocabulary, so these mirror the backend's `validate_graph` gate exactly
  // and never fire on a legacy workflow (spec §2/§4 compat).
  if (hasExplicitEdges(g.nodes)) {
    // Cycle: mirrors the backend's `WorkflowCycle` check. `sorted` (above)
    // already ran Kahn's algorithm over `g.edges` (== `deriveEdges(g.nodes)`
    // by the graph's own invariant); any node topoSort couldn't place still
    // has unresolved in-degree, i.e. is part of a cycle. A reconverging
    // diamond (a→b, a→c, b→d, c→d) is NOT a cycle — Kahn's tracks per-node
    // in-degree, not pairwise adjacency, so it drains cleanly.
    if ('cycle' in sorted) {
      for (const id of sorted.cycle) add(id, 'part of a cycle — steps must form a DAG');
    }

    // Unknown edge target: a `next`/`split` id that isn't a known node.
    for (const n of g.nodes) {
      for (const t of n.data.next ?? []) {
        if (!nodeIds.has(t)) add(n.id, `edge target \`${t}\` is not a known step`);
      }
      if (n.data.kind === 'split') {
        for (const t of n.data.split ?? []) {
          if (!nodeIds.has(t)) add(n.id, `edge target \`${t}\` is not a known step`);
        }
      }
    }

    // Degenerate split/join: fanning out to (or in from) fewer than 2 steps
    // isn't doing real orchestration work — a plain `next` chain would do the
    // same job with less ceremony. Low-severity (not a save-blocking error
    // like the checks above), so distinct wording.
    for (const n of g.nodes) {
      if (n.data.kind === 'split') {
        if ((n.data.split ?? []).length < 2) add(n.id, 'a split should fan out to 2+ steps');
      } else if (n.data.kind === 'join') {
        const inbound = g.edges.filter((e) => e.target === n.id).length;
        if (inbound < 2) add(n.id, 'a join should have 2+ inbound paths');
      }
    }
  }

  return out;
}

// ── convertInlineApprovalToGate ──────────────────────────────────────────────

/** True for a "legacy inline approval": an agent-bearing step/for_each node
 *  whose `approval.required` is set directly on it (workflow.rs `Approval` on
 *  `Step`), rather than expressed as a standalone `approval_gate` node. This is
 *  the shape the dashed-gate badge and the "Convert to gate node" affordance
 *  both target. */
export function hasInlineApproval(d: StepNodeData): boolean {
  return (d.kind === 'step' || d.kind === 'for_each') && d.approvalRequired === true;
}

/** Horizontal shift applied to the agent step when a gate node is inserted
 *  before it, so the two never overlap: the gate's own box width (`GATE_W`,
 *  214) + the `applyAddConnectedNext` gap (64) in workflowLayout /
 *  WorkflowEditorGraph — duplicated as a literal rather than imported so this
 *  module stays framework/layout-free (see the file-header comment). The gate
 *  is a trapezoid and is WIDER than a plain step (210), which is why this is
 *  not `NODE_W + gap`. */
const CONVERT_GATE_X_OFFSET = 278;

/** Rewrite `stepId`'s inline `approval:` into a NEW standalone `approval_gate`
 *  node inserted immediately before it: every edge that targeted `stepId` is
 *  re-targeted at the new gate, and a gate→`stepId` edge is added, so the gate
 *  always runs first regardless of node array order or layout position. The
 *  agent step's `approval*` fields are cleared (moved onto the gate) — every
 *  other field (agent/prompt/when/raw_passthrough/etc.) is untouched.
 *
 *  A no-op (returns `g` unchanged) when `stepId` doesn't name a node, or names
 *  a node without an inline approval (see `hasInlineApproval`) — callers don't
 *  need to guard before invoking this.
 *
 *  Pure graph transform, like `canConnect`/`applyConnect` — callers re-serialize
 *  the result via `graphToWorkflowObject` + `yaml.dump` same as any other graph
 *  edit. Powers the StepForm "Convert to gate node" button (Slice D Plan 3
 *  Task 6); full auto-synthesis of a gate on EVERY legacy inline approval is
 *  deferred (this is opt-in, one node at a time, from the editor). */
export function convertInlineApprovalToGate(g: WorkflowGraph, stepId: string): WorkflowGraph {
  const idx = g.nodes.findIndex((n) => n.id === stepId);
  if (idx === -1) return g;
  const node = g.nodes[idx];
  if (!hasInlineApproval(node.data)) return g;

  // Smallest-available `<stepId>-gate[-N]` id, so converting the same step
  // twice (or a workflow that already has `<id>-gate`) never collides.
  const existingIds = new Set(g.nodes.map((n) => n.id));
  let gateId = `${stepId}-gate`;
  for (let n = 1; existingIds.has(gateId); n++) gateId = `${stepId}-gate-${n}`;

  const gateData: StepNodeData = { id: gateId, kind: 'approval_gate', approvalRequired: true };
  if (node.data.approvalPrompt !== undefined) gateData.approvalPrompt = node.data.approvalPrompt;
  if (node.data.approvalTimeoutSeconds !== undefined) {
    gateData.approvalTimeoutSeconds = node.data.approvalTimeoutSeconds;
  }
  if (node.data.approvalAutoApprove !== undefined) {
    gateData.approvalAutoApprove = node.data.approvalAutoApprove;
  }
  if (node.data.approvalOnTimeout !== undefined) {
    gateData.approvalOnTimeout = node.data.approvalOnTimeout;
  }
  if (node.data.approvalNotify !== undefined) {
    gateData.approvalNotify = node.data.approvalNotify;
  }
  if (node.data.approvalOnReject !== undefined) {
    gateData.approvalOnReject = node.data.approvalOnReject;
  }
  if (node.data.approvalRest !== undefined) {
    gateData.approvalRest = node.data.approvalRest;
  }

  const strippedData: StepNodeData = { ...node.data };
  delete strippedData.approvalRequired;
  delete strippedData.approvalPrompt;
  delete strippedData.approvalTimeoutSeconds;
  delete strippedData.approvalAutoApprove;
  delete strippedData.approvalOnTimeout;
  delete strippedData.approvalNotify;
  delete strippedData.approvalOnReject;
  delete strippedData.approvalRest;

  const gateNode: GraphNode = {
    id: gateId,
    data: gateData,
    position: { x: node.position.x, y: node.position.y },
  };
  const strippedNode: GraphNode = {
    ...node,
    data: strippedData,
    position: { x: node.position.x + CONVERT_GATE_X_OFFSET, y: node.position.y },
  };

  const nodes = [...g.nodes];
  nodes[idx] = strippedNode;
  nodes.splice(idx, 0, gateNode);

  // gate -> stepId re-derives as a plain chain edge because gateNode is
  // spliced immediately before strippedNode; any node that previously chained
  // straight into stepId now chains into gateNode first for the same reason
  // (consecutive array order shifted, nothing else to rewire). Branch-arm
  // then/else targets naming stepId are NOT retargeted here — they live on
  // the source branch node's own data, untouched by this transform, so a
  // branch that routed to stepId still routes there (running after the new
  // gate in the chain, same as before the branch existed) rather than
  // silently retargeting someone else's routing decision.
  return withDerivedEdges(g.meta, nodes);
}
