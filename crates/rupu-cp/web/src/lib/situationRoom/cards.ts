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
import { memberLabel, parseCodename } from '../codename';
import { findingCodename } from '../findingIdentity';

/** Which filter chip a card answers to. `activity` is the catch-all for agent
 *  work / step + run lifecycle / panel rounds. `warning` is a non-failing
 *  operator notice (`step_warning`) — deliberately NOT `error`: the step did
 *  not fail. */
export type CardGroup = 'finding' | 'await' | 'error' | 'warning' | 'activity';

/** The editorial form the card renders as. `finding` gets the rich
 *  severity + evidence + code treatment; `await` gets inline approve/reject. */
export type CardForm =
  | 'activity'
  | 'panel'
  | 'lifecycle'
  | 'complete'
  | 'finding'
  | 'await'
  | 'error'
  | 'warning';

/** Left-stripe / badge color key — a severity for findings, otherwise a
 *  semantic role. Maps 1:1 to a `sr-s-*` CSS class. */
export type CardAccent = FindingSeverity | 'brand' | 'await' | 'error' | 'warn';

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
  /** Workflow name — resolved by the page from run_id → run.json (like
   *  projectName). Absent until resolved / for a bare-agent run. */
  workflow?: string;
  stepId?: string;
  agent?: string;
  /** Server-minted agent codename (e.g. `jade-reef/heron#4`) when the event /
   *  finding carries one. Display-only — never computed client-side. */
  codename?: string;
  /** True when the server derived `codename` for a pre-codename run (render
   *  it muted). */
  codenameDerived?: boolean;
  /** Crew word of the run (`jade-reef`) — from the event's codename, or the
   *  caller's run_id → run codename lookup for run-level cards. Drives the
   *  tint stripe + the run link label. */
  crew?: string;
  /** Provider / model the agent ran on (agent_started carries them). */
  provider?: string;
  model?: string;
  /** Step kind (e.g. `for_each`, `panel`) when the event carries it. */
  stepKind?: string;
  /** Fan-out / panel unit key — WHICH target this event is about. */
  unitKey?: string;
  /** step_completed wall-clock, in ms. */
  durationMs?: number;
  /** unit_completed token counts. */
  tokensIn?: number;
  tokensOut?: number;
  /** panel_round progress. */
  round?: { n: number; max: number };
  /** Deep-link to the step / unit transcript when the event carries one. */
  transcriptPath?: string;
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
/** Cross-event context the caller (which holds the whole event list) can
 *  supply; `cardFromEvent` itself stays a pure per-event mapper. */
export interface CardContext {
  /** run_id → run codename (the page resolves it via getRun). Gives cards
   *  with no codename of their own (run_started / awaiting / completed …)
   *  their run's crew word, so every card of a run shares the tint. */
  crewByRun?: ReadonlyMap<string, string>;
  /** (run, step, unit index) → unit_key, from {@link unitKeyIndex}. Lets an
   *  agent_started card show the fan-out target its (suppressed, named)
   *  unit_started carried. */
  unitKeys?: ReadonlyMap<string, string>;
  /** (run, step, unit index) of every unit-scoped agent_started, from
   *  {@link agentUnitIndex}. A pre-codename run whose unit has BOTH a
   *  unit_started and an agent_started (both named on read) renders only the
   *  agent_started card — the same one-card-per-unit rule as a named run. */
  agentUnits?: ReadonlySet<string>;
}

function unitKeyId(runId: string, stepId: string, index: number): string {
  return `${runId}\u0000${stepId}\u0000${index}`;
}

/** Index every `unit_started`'s unit_key by (run, step, index). */
export function unitKeyIndex(events: Iterable<RunEvent>): Map<string, string> {
  const out = new Map<string, string>();
  for (const ev of events) {
    if (!isKnownRunEvent(ev) || ev.type !== 'unit_started') continue;
    out.set(unitKeyId(ev.run_id, ev.step_id, ev.index), ev.unit_key);
  }
  return out;
}

/** Keys (see {@link CardContext.agentUnits}) of every unit-scoped
 *  agent_started in `events`. */
export function agentUnitIndex(events: Iterable<RunEvent>): Set<string> {
  const out = new Set<string>();
  for (const ev of events) {
    if (!isKnownRunEvent(ev) || ev.type !== 'agent_started' || ev.unit_index == null) continue;
    out.add(unitKeyId(ev.run_id, ev.step_id, ev.unit_index));
  }
  return out;
}

export function cardFromEvent(
  ev: RunEvent,
  ts: number,
  key: string,
  ctx?: CardContext,
): StreamCard | null {
  if (
    ctx?.agentUnits &&
    isKnownRunEvent(ev) &&
    ev.type === 'unit_started' &&
    ev.codename_derived &&
    ctx.agentUnits.has(unitKeyId(ev.run_id, ev.step_id, ev.index))
  ) {
    return null;
  }
  let card = cardFromEventInner(ev, ts, key);
  if (!card || !ctx) return card;
  if (ctx.unitKeys && isKnownRunEvent(ev) && ev.type === 'agent_started' && ev.unit_index != null) {
    const unitKey = ctx.unitKeys.get(unitKeyId(ev.run_id, ev.step_id, ev.unit_index));
    if (unitKey) card = { ...card, unitKey };
  }
  if (ctx.unitKeys && isKnownRunEvent(ev) && ev.type === 'step_warning' && typeof ev.index === 'number') {
    const unitKey = ctx.unitKeys.get(unitKeyId(ev.run_id, ev.step_id, ev.index));
    if (unitKey) card = { ...card, unitKey };
  }
  if (!card.crew && card.runId && ctx.crewByRun) {
    const runName = ctx.crewByRun.get(card.runId);
    if (runName) card = { ...card, crew: parseCodename(runName).crew };
  }
  return card;
}

/** Codename + its crew word (+ the derived flag), spread onto a card when
 *  the event carries one. */
function named(
  codename: string | undefined,
  derived?: boolean,
): { codename?: string; crew?: string; codenameDerived?: boolean } {
  if (!codename) return {};
  const out: { codename?: string; crew?: string; codenameDerived?: boolean } = {
    codename,
    crew: parseCodename(codename).crew,
  };
  if (derived === true) out.codenameDerived = true;
  return out;
}

function cardFromEventInner(ev: RunEvent, ts: number, key: string): StreamCard | null {
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
        stepKind: k.kind, ...named(k.codename, k.codename_derived),
        // Agent is rendered as its own field; the title is just the step so it
        // isn't repeated ("agent agent · step").
        title: stepLabel(k.step_id),
        detail: k.agent ? undefined : k.kind };
    case 'step_working': {
      const note = k.note?.trim();
      if (!note) return null; // note-less heartbeat — step_started already covered it
      return { ...base, form: 'activity', group: 'activity', accent: 'brand',
        badge: 'Working', stepId: k.step_id, title: stepLabel(k.step_id), detail: note,
        transcriptPath: k.transcript_path ?? undefined };
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
        durationMs: k.duration_ms,
        title: stepLabel(k.step_id),
        detail: `${k.success ? 'ok' : 'failed'} · ${Math.round(k.duration_ms / 100) / 10}s` };
    case 'step_failed':
      return { ...base, form: 'error', group: 'error', accent: 'error',
        badge: 'Error', stepId: k.step_id, title: `${stepLabel(k.step_id)} failed`, detail: k.error };
    case 'step_warning':
      // A warning never fails the step — its own group/accent (amber), never
      // the error group. A unit-scoped one names the unit in the headline
      // (the page's unit_key index adds the target as a meta chip).
      return { ...base, form: 'warning', group: 'warning', accent: 'warn',
        badge: 'Warning', stepId: k.step_id,
        title: typeof k.index === 'number'
          ? `${stepLabel(k.step_id)} · unit ${k.index} warning`
          : `${stepLabel(k.step_id)} warning`,
        detail: k.message };
    case 'step_skipped':
      return { ...base, form: 'activity', group: 'activity', accent: 'brand',
        badge: 'Skipped', stepId: k.step_id, title: `${stepLabel(k.step_id)} skipped`, detail: k.reason };
    case 'unit_started':
      // New-era runs always follow a named unit_started with an agent_started
      // (which carries provider/model too) — render only that one, so a wide
      // fan-out doesn't double its cards. Legacy units (no agent_started)
      // keep theirs — including ones whose name the server derived on read.
      if (k.codename && !k.codename_derived) return null;
      return { ...base, form: 'activity', group: 'activity', accent: 'brand',
        badge: 'Fan-out', stepId: k.step_id, agent: k.agent ?? undefined,
        ...named(k.codename, k.codename_derived),
        unitKey: k.unit_key, transcriptPath: k.transcript_path,
        // Agent + unit render as their own fields; keep the title the step.
        title: stepLabel(k.step_id) };
    case 'agent_started':
      return { ...base, form: 'activity', group: 'activity', accent: 'brand',
        badge: 'Agent', stepId: k.step_id, agent: k.agent,
        provider: k.provider, model: k.model, ...named(k.codename, k.codename_derived),
        transcriptPath: k.transcript_path,
        // The member label IS the headline: who launched, on what.
        title: memberLabel(k.codename, k.agent, k.provider, k.model),
        // The codename (`heron#4`, 1-based) already identifies the unit; a
        // 0-based `unit 3` beside it only contradicted it. The fan-out target
        // (unitKey, from the page's unit_key index) is the useful context.
        detail: stepLabel(k.step_id) };
    case 'unit_completed':
      return { ...base, form: 'complete', group: 'activity',
        accent: k.success ? 'brand' : 'error', badge: k.success ? 'Unit done' : 'Unit failed',
        stepId: k.step_id, unitKey: k.unit_key, tokensIn: k.tokens_in, tokensOut: k.tokens_out,
        // Step is the title; unit + tokens render as their own meta fields.
        title: stepLabel(k.step_id) };
    case 'panel_round':
      return { ...base, form: 'panel', group: 'activity', accent: 'brand',
        badge: 'Panel round', stepId: k.step_id, round: { n: k.round, max: k.max_iterations },
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
  const who = findingCodename(f);
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
    ...named(who?.codename, who?.derived),
    agent: who?.agent,
    provider: who?.provider,
    model: who?.model,
  };
}
