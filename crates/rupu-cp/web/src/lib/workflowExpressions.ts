// workflowExpressions — the PURE vocabulary model for minijinja template
// expressions used inside workflow step fields (prompt / when / for_each /
// panel subject+prompt / parallel sub-step prompts).
//
// This module is the single source of truth for the REAL supported template
// vocabulary (verified against crates/rupu-orchestrator's render context). It is
// framework-free (no React, no DOM): it just maps an editing context to the set
// of completion entries valid there, and exposes the full grouped reference.
//
// Context rules (design §5.2), enforced by `completionsFor`:
//   • `item` / `loop.*`        — only inside a for_each step's PROMPT field.
//   • `inputs.subject`         — only inside a panel step's subject/prompt.
//   • `steps.<id>.…`           — only ids that run BEFORE this node (topo order);
//                                the sub-paths offered depend on that step's kind.
//   • `inputs.<name>`          — the workflow's declared input names.
//   • event.* / issue.* / read_file / filters — always offered.

import type { StepKind } from './workflowGraph';

// ── Types ───────────────────────────────────────────────────────────────────

export type ExprKind = 'path' | 'filter' | 'function' | 'loop' | 'keyword';

export interface ExprEntry {
  /** Text inserted on accept (e.g. `steps.build.output`). */
  insert: string;
  /** Display label in the completion list / reference panel. */
  label: string;
  /** Short human description shown as the completion detail. */
  detail: string;
  kind: ExprKind;
}

export interface ExprContext {
  /** Kind of the node whose field is being edited. */
  nodeKind: StepKind;
  /** True only for a for_each node's PROMPT field (gates `item` / `loop.*`). */
  isForEachPrompt: boolean;
  /** True only for a panel node's subject/prompt (gates `inputs.subject`). */
  isPanelField: boolean;
  /** Declared workflow input names (from the workflow meta `inputs:` block). */
  inputNames: string[];
  /** Steps that topologically precede this node, with their kind. */
  priorSteps: { id: string; kind: StepKind }[];
}

// ── Static vocabulary fragments ───────────────────────────────────────────────

/** Loop helpers — valid only inside a for_each prompt. */
function loopEntries(): ExprEntry[] {
  return [
    { insert: 'item', label: 'item', detail: 'current for_each item', kind: 'loop' },
    { insert: 'loop.index', label: 'loop.index', detail: 'iteration number (1-based)', kind: 'loop' },
    { insert: 'loop.index0', label: 'loop.index0', detail: 'iteration number (0-based)', kind: 'loop' },
    { insert: 'loop.length', label: 'loop.length', detail: 'total number of items', kind: 'loop' },
    { insert: 'loop.first', label: 'loop.first', detail: 'true on the first iteration', kind: 'loop' },
    { insert: 'loop.last', label: 'loop.last', detail: 'true on the last iteration', kind: 'loop' },
  ];
}

/** Event payload (event-triggered runs only). */
function eventEntries(): ExprEntry[] {
  return [
    { insert: 'event.action', label: 'event.action', detail: 'event action (event runs only)', kind: 'path' },
    { insert: 'event.repository', label: 'event.repository', detail: 'event repository (event runs only)', kind: 'path' },
    {
      insert: 'event.pull_request',
      label: 'event.pull_request',
      detail: 'event pull request (event runs only)',
      kind: 'path',
    },
    { insert: 'event.issue', label: 'event.issue', detail: 'event issue (event runs only)', kind: 'path' },
  ];
}

/** Issue payload (issue-target runs only). */
function issueEntries(): ExprEntry[] {
  return [
    { insert: 'issue.number', label: 'issue.number', detail: 'issue number (issue runs only)', kind: 'path' },
    { insert: 'issue.title', label: 'issue.title', detail: 'issue title (issue runs only)', kind: 'path' },
    { insert: 'issue.body', label: 'issue.body', detail: 'issue body (issue runs only)', kind: 'path' },
    { insert: 'issue.labels', label: 'issue.labels', detail: 'issue labels (array)', kind: 'path' },
    { insert: 'issue.author', label: 'issue.author', detail: 'issue author (issue runs only)', kind: 'path' },
    { insert: 'issue.state', label: 'issue.state', detail: 'issue state (issue runs only)', kind: 'path' },
  ];
}

/** Custom function(s). */
function functionEntries(): ExprEntry[] {
  return [
    { insert: "read_file('path')", label: "read_file('path')", detail: 'read a file into the prompt', kind: 'function' },
  ];
}

const FILTER_NAMES: { name: string; detail: string }[] = [
  { name: 'length', detail: 'length of a string/array' },
  { name: 'join', detail: 'join an array with a separator' },
  { name: 'default', detail: 'fallback when value is undefined' },
  { name: 'tojson', detail: 'serialize value as JSON' },
  { name: 'map', detail: 'map an attribute/filter over a sequence' },
  { name: 'select', detail: 'keep items matching a test' },
  { name: 'first', detail: 'first element' },
  { name: 'last', detail: 'last element' },
  { name: 'upper', detail: 'uppercase a string' },
  { name: 'lower', detail: 'lowercase a string' },
  { name: 'trim', detail: 'strip surrounding whitespace' },
  { name: 'sort', detail: 'sort a sequence' },
  { name: 'reverse', detail: 'reverse a sequence' },
];

/** Standard minijinja filters — offered as `| name`. */
function filterEntries(): ExprEntry[] {
  return FILTER_NAMES.map((f) => ({
    insert: f.name,
    label: `| ${f.name}`,
    detail: `filter — ${f.detail}`,
    kind: 'filter' as const,
  }));
}

/** The sub-paths exposed by a referenced step, by its kind. */
function stepSubPaths(id: string, kind: StepKind): ExprEntry[] {
  const out: ExprEntry[] = [
    { insert: `steps.${id}.output`, label: `steps.${id}.output`, detail: 'step output (text)', kind: 'path' },
    { insert: `steps.${id}.success`, label: `steps.${id}.success`, detail: 'step succeeded (bool)', kind: 'path' },
    { insert: `steps.${id}.skipped`, label: `steps.${id}.skipped`, detail: 'step was skipped (bool)', kind: 'path' },
  ];
  if (kind === 'for_each' || kind === 'parallel') {
    out.push({
      insert: `steps.${id}.results`,
      label: `steps.${id}.results`,
      detail: 'per-item / per-branch results (array)',
      kind: 'path',
    });
  }
  if (kind === 'parallel') {
    out.push({
      insert: `steps.${id}.sub_results`,
      label: `steps.${id}.sub_results`,
      detail: 'results by sub-step id (.<sub_id>.output / .success)',
      kind: 'path',
    });
  }
  if (kind === 'panel') {
    out.push(
      {
        insert: `steps.${id}.findings`,
        label: `steps.${id}.findings`,
        detail: 'panel findings (array of {source,severity,title,body})',
        kind: 'path',
      },
      {
        insert: `steps.${id}.max_severity`,
        label: `steps.${id}.max_severity`,
        detail: 'highest finding severity',
        kind: 'path',
      },
      { insert: `steps.${id}.iterations`, label: `steps.${id}.iterations`, detail: 'gate iterations run', kind: 'path' },
      {
        insert: `steps.${id}.resolved`,
        label: `steps.${id}.resolved`,
        detail: 'gate cleared all findings (bool)',
        kind: 'path',
      },
    );
  }
  return out;
}

// ── completionsFor ─────────────────────────────────────────────────────────────

/** The completion entries valid in the given editing context. */
export function completionsFor(ctx: ExprContext): ExprEntry[] {
  const out: ExprEntry[] = [];

  // inputs.<name>
  for (const name of ctx.inputNames) {
    out.push({ insert: `inputs.${name}`, label: `inputs.${name}`, detail: 'workflow input', kind: 'path' });
  }
  // inputs.subject — panel fields only.
  if (ctx.isPanelField) {
    out.push({ insert: 'inputs.subject', label: 'inputs.subject', detail: 'the panel subject', kind: 'path' });
  }

  // steps.<id>.… — only prior steps, sub-paths by kind.
  for (const s of ctx.priorSteps) {
    out.push(...stepSubPaths(s.id, s.kind));
  }

  // item / loop.* — for_each prompt only.
  if (ctx.isForEachPrompt) {
    out.push(...loopEntries());
  }

  out.push(...eventEntries());
  out.push(...issueEntries());
  out.push(...functionEntries());
  out.push(...filterEntries());

  return out;
}

// ── expressionReference ────────────────────────────────────────────────────────

/** The FULL grouped vocabulary, using `<id>` / `<name>` placeholders — for the
 *  reference panel (P6). Independent of any single editing context. */
export function expressionReference(): { group: string; entries: ExprEntry[] }[] {
  return [
    {
      group: 'Inputs',
      entries: [
        { insert: 'inputs.<name>', label: 'inputs.<name>', detail: 'a declared workflow input', kind: 'path' },
        { insert: 'inputs.subject', label: 'inputs.subject', detail: 'the panel subject (panel steps)', kind: 'path' },
      ],
    },
    {
      group: 'Steps',
      entries: [
        { insert: 'steps.<id>.output', label: 'steps.<id>.output', detail: 'step output (text)', kind: 'path' },
        { insert: 'steps.<id>.success', label: 'steps.<id>.success', detail: 'step succeeded (bool)', kind: 'path' },
        { insert: 'steps.<id>.skipped', label: 'steps.<id>.skipped', detail: 'step was skipped (bool)', kind: 'path' },
        {
          insert: 'steps.<id>.results',
          label: 'steps.<id>.results',
          detail: 'for_each / parallel results (array)',
          kind: 'path',
        },
        {
          insert: 'steps.<id>.sub_results.<sub_id>.output',
          label: 'steps.<id>.sub_results.<sub_id>.output',
          detail: 'a parallel sub-step output',
          kind: 'path',
        },
        {
          insert: 'steps.<id>.findings',
          label: 'steps.<id>.findings',
          detail: 'panel findings (array of {source,severity,title,body})',
          kind: 'path',
        },
        {
          insert: 'steps.<id>.max_severity',
          label: 'steps.<id>.max_severity',
          detail: 'highest panel finding severity',
          kind: 'path',
        },
        {
          insert: 'steps.<id>.iterations',
          label: 'steps.<id>.iterations',
          detail: 'panel gate iterations run',
          kind: 'path',
        },
        {
          insert: 'steps.<id>.resolved',
          label: 'steps.<id>.resolved',
          detail: 'panel gate cleared all findings (bool)',
          kind: 'path',
        },
      ],
    },
    { group: 'Loop (for_each)', entries: loopEntries() },
    { group: 'Event', entries: eventEntries() },
    { group: 'Issue', entries: issueEntries() },
    { group: 'Functions', entries: functionEntries() },
    { group: 'Filters', entries: filterEntries() },
  ];
}
