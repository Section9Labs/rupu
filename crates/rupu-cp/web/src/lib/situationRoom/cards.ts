// Situation Room — pure mapping from raw wire objects to StreamCard view
// models. The Live Events "stream" is a merge of two REAL sources:
//   1. the SSE / history event firehose (`RunEvent`) — agent activity, step
//      lifecycle, approvals, errors, panel rounds;
//   2. the REST findings list (`FindingOut`) — the high-value results, which
//      are NOT on the event wire today (findings are REST-only). We merge them
//      in by `declared_at` so a landed finding appears in the timeline.
//
// Everything here is a pure function so it unit-tests without a DOM. The React
// layer (`EventCard`) only renders the StreamCard this produces — no data
// decisions live in the component.

import {
  isKnownRunEvent,
  normFindingSeverity,
  type FindingOut,
  type FindingSeverity,
  type KnownRunEvent,
  type RunEvent,
} from '../api';

/** Which filter chip a card answers to. `activity` is the catch-all for agent
 *  work / step + run lifecycle / panel rounds. */
export type CardGroup = 'finding' | 'await' | 'error' | 'activity';

/** The editorial form the card renders as. `finding` gets the rich
 *  severity + evidence + code treatment; `await` gets inline approve/reject. */
export type CardForm =
  | 'activity'
  | 'panel'
  | 'lifecycle'
  | 'complete'
  | 'finding'
  | 'await'
  | 'error';

/** Left-stripe / badge color key — a severity for findings, otherwise a
 *  semantic role. Maps 1:1 to a `sr-s-*` CSS class. */
export type CardAccent = FindingSeverity | 'brand' | 'await' | 'error';

export interface StreamCard {
  /** Stable identity — dedup + React key. Findings use their id; events use
   *  run+pos or a content hash stamped by the caller. */
  key: string;
  /** Ordering key (ms since epoch), newest-first in the stream. */
  ts: number;
  form: CardForm;
  group: CardGroup;
  accent: CardAccent;
  /** Uppercase pill text, e.g. "SCANNING", "HIGH", "AWAITING YOU". */
  badge: string;
  /** Headline. */
  title: string;
  /** Secondary line (note / error / rationale). Optional. */
  detail?: string;
  runId?: string;
  /** Project/workspace label. Findings carry it directly; event cards get it
   *  resolved by the page from run_id → workspace. */
  projectName?: string;
  stepId?: string;
  agent?: string;
  /** Findings only: normalized severity, a `file:line` ref, and a real code
   *  excerpt from the finding's evidence (never fabricated). */
  severity?: FindingSeverity;
  fileRef?: string;
  code?: string;
  /** Findings only: source location + provenance, so the card can deep-link to
   *  the project Code viewer and (when present) the SCM permalink. */
  wsId?: string;
  filePath?: string;
  fileLine?: number;
  permalink?: string;
  /** Present on `await` cards — the run + reason an approval can act on. */
  approvable?: { runId: string; stepId?: string; reason: string };
}

const SEV_BADGE: Record<FindingSeverity, string> = {
  critical: 'Critical',
  high: 'High',
  medium: 'Medium',
  low: 'Low',
  info: 'Info',
};

/** Short, human step label — strips noise, keeps the id readable. */
function stepLabel(stepId: string | undefined): string {
  return (stepId ?? '').trim();
}

/**
 * Map one raw event to a StreamCard, or `null` when the event carries nothing
 * worth a row on its own (e.g. a note-less `step_working` heartbeat — the
 * `step_started` already announced the step).
 *
 * `ts` is supplied by the caller: history rows carry their own `ts`; live SSE
 * frames are stamped with arrival time (mirroring the existing Events page).
 * `key` is likewise caller-owned so history↔live dedup stays in one place.
 */
export function cardFromEvent(ev: RunEvent, ts: number, key: string): StreamCard | null {
  if (!isKnownRunEvent(ev)) {
    // Unknown/forward-compat event — still surface it rather than drop it, so
    // a new backend event type is visible instead of silently missing.
    return {
      key, ts, form: 'activity', group: 'activity', accent: 'brand',
      badge: ev.type.replace(/_/g, ' '),
      title: typeof (ev as { step_id?: unknown }).step_id === 'string'
        ? String((ev as { step_id?: unknown }).step_id)
        : ev.type,
      runId: ev.run_id,
    };
  }
  const k: KnownRunEvent = ev;
  const base = { key, ts, runId: k.run_id } as const;

  switch (k.type) {
    case 'run_started':
      return { ...base, form: 'lifecycle', group: 'activity', accent: 'brand',
        badge: 'Run started', title: 'Workflow run started', detail: k.workflow_path };
    case 'run_completed':
      return { ...base, form: 'complete', group: 'activity',
        accent: k.status === 'completed' ? 'brand' : k.status === 'failed' ? 'error' : 'brand',
        badge: 'Run ' + k.status, title: `Run ${k.status}` };
    case 'run_failed':
      return { ...base, form: 'error', group: 'error', accent: 'error',
        badge: 'Run failed', title: 'Workflow run failed', detail: k.error };
    case 'step_started':
      return { ...base, form: 'activity', group: 'activity', accent: 'brand',
        badge: k.agent ? 'Scanning' : 'Step', stepId: k.step_id, agent: k.agent ?? undefined,
        title: k.agent ? `${k.agent} · ${stepLabel(k.step_id)}` : stepLabel(k.step_id),
        detail: k.agent ? undefined : k.kind };
    case 'step_working': {
      const note = k.note?.trim();
      if (!note) return null; // note-less heartbeat — step_started already covered it
      return { ...base, form: 'activity', group: 'activity', accent: 'brand',
        badge: 'Working', stepId: k.step_id, title: stepLabel(k.step_id), detail: note };
    }
    case 'step_awaiting_approval':
      return { ...base, form: 'await', group: 'await', accent: 'await',
        badge: 'Awaiting you', stepId: k.step_id,
        title: `Approval needed · ${stepLabel(k.step_id)}`, detail: k.reason,
        approvable: { runId: k.run_id, stepId: k.step_id, reason: k.reason } };
    case 'step_completed':
      return { ...base, form: 'complete', group: 'activity',
        accent: k.success ? 'brand' : 'error',
        badge: k.success ? 'Step done' : 'Step failed', stepId: k.step_id,
        title: stepLabel(k.step_id),
        detail: `${k.success ? 'ok' : 'failed'} · ${Math.round(k.duration_ms / 100) / 10}s` };
    case 'step_failed':
      return { ...base, form: 'error', group: 'error', accent: 'error',
        badge: 'Error', stepId: k.step_id, title: `${stepLabel(k.step_id)} failed`, detail: k.error };
    case 'step_skipped':
      return { ...base, form: 'activity', group: 'activity', accent: 'brand',
        badge: 'Skipped', stepId: k.step_id, title: `${stepLabel(k.step_id)} skipped`, detail: k.reason };
    case 'unit_started':
      return { ...base, form: 'activity', group: 'activity', accent: 'brand',
        badge: 'Fan-out', stepId: k.step_id, agent: k.agent ?? undefined,
        title: `${stepLabel(k.step_id)} · ${k.unit_key}`, detail: k.agent ? `agent ${k.agent}` : undefined };
    case 'unit_completed':
      return { ...base, form: 'complete', group: 'activity',
        accent: k.success ? 'brand' : 'error', badge: k.success ? 'Unit done' : 'Unit failed',
        stepId: k.step_id, title: `${stepLabel(k.step_id)} · ${k.unit_key}`,
        detail: `${k.success ? 'ok' : 'failed'} · ${k.tokens_in}→${k.tokens_out} tok` };
    case 'panel_round':
      return { ...base, form: 'panel', group: 'activity', accent: 'brand',
        badge: 'Panel round', stepId: k.step_id,
        title: `${stepLabel(k.step_id)} · round ${k.round}/${k.max_iterations}`,
        detail: k.max_severity_remaining ? `max severity remaining: ${k.max_severity_remaining}` : undefined };
    case 'run_paused':
      return { ...base, form: 'lifecycle', group: 'activity', accent: 'await', badge: 'Paused', title: 'Run paused' };
    case 'run_resumed':
      return { ...base, form: 'lifecycle', group: 'activity', accent: 'brand', badge: 'Resumed', title: 'Run resumed' };
    case 'step_paused':
      return { ...base, form: 'lifecycle', group: 'activity', accent: 'await', badge: 'Paused', stepId: k.step_id, title: `${stepLabel(k.step_id)} paused` };
    case 'step_resumed':
      return { ...base, form: 'lifecycle', group: 'activity', accent: 'brand', badge: 'Resumed', stepId: k.step_id, title: `${stepLabel(k.step_id)} resumed` };
    default:
      return null;
  }
}

/** Map one REST finding to a StreamCard. Findings are the richest cards —
 *  severity accent, a `file:line` reference, the evidence rationale as the
 *  detail, and the real `code_excerpt` (when the finding carries one). */
export function cardFromFinding(f: FindingOut): StreamCard {
  const sev = normFindingSeverity(f.severity);
  const ts = Date.parse(f.declared_at);
  const fileRef = f.file_path
    ? f.line_range
      ? `${f.file_path}:${f.line_range[0]}-${f.line_range[1]}`
      : f.file_path
    : undefined;
  return {
    key: `finding:${f.id}`,
    ts: Number.isNaN(ts) ? 0 : ts,
    form: 'finding',
    group: 'finding',
    accent: sev,
    severity: sev,
    badge: SEV_BADGE[sev],
    title: f.summary,
    detail: f.evidence?.rationale,
    fileRef,
    code: f.evidence?.code_excerpt ?? undefined,
    runId: undefined,
    projectName: f.project,
    wsId: f.ws_id,
    filePath: f.file_path ?? undefined,
    fileLine: f.line_range?.[0],
    permalink: f.permalink ?? undefined,
  };
}
