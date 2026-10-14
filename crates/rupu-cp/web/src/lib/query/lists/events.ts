// The Live Events stream's query fields — the twin of the server's
// `crates/rupu-cp/src/query/events.rs`, which filters the event history
// (`GET /api/events?q=`); the page evaluates the same registry over every
// card it holds (history, live frames and findings). Pinned to the server
// by `tests/fixtures/list_query/events.fields.json`, the group table in
// `events.groups.json` and the rows each query keeps in
// `events.matches.json`.
import { Activity, Bot, CalendarClock, Cpu, Fingerprint, FolderGit2, History, Layers, MessageSquare, Play, Server, ShieldAlert, Tag, Workflow } from 'lucide-react';
import type { RunEvent } from '../../api';
import type { StreamCard } from '../../situationRoom/cards';
import { rowField, type RowField } from '../rows';

/** The keys the query reads, named as `GET /api/events` names them on a
 *  history row: the event's own and its run's attribution. */
export interface QueryRow {
  type?: string;
  run_id?: string;
  project?: string;
  workflow?: string;
  run_codename?: string;
  agent?: string;
  step_id?: string;
  codename?: string;
  provider?: string;
  model?: string;
  severity?: string;
  error?: string;
  reason?: string;
  message?: string;
  note?: string;
}

/** A stream card with the row the query reads. An event's is the event's
 *  own keys, never the card's rendering of them, so the page and the server
 *  answer a query from the same values. */
export interface EventRow {
  card: StreamCard;
  row: QueryRow;
}

/** The keys `GET /api/events` adds to a history row beyond the event. */
export const HISTORY_ATTRIBUTION_KEYS = ['workflow', 'project', 'run_codename'] as const;

const EVENT_KEYS = ['type', 'run_id', 'agent', 'step_id', 'codename', 'provider', 'model', 'severity', 'error', 'reason', 'message', 'note'] as const;

function str(v: unknown): string | undefined {
  return typeof v === 'string' && v !== '' ? v : undefined;
}

/** An event card's row: the event's keys and its history attribution, else
 *  the page's lookups (`lookup`) for the run. */
export function eventRow(
  card: StreamCard,
  ev: RunEvent,
  lookup: { project?: string; workflow?: string; runCodename?: string },
): EventRow {
  const raw = ev as Record<string, unknown>;
  const row: QueryRow = {};
  for (const k of EVENT_KEYS) row[k] = str(raw[k]);
  row.project = str(raw.project) ?? card.projectName ?? lookup.project;
  row.workflow = str(raw.workflow) ?? card.workflow ?? lookup.workflow;
  row.run_codename = str(raw.run_codename) ?? lookup.runCodename;
  return { card, row };
}

/** A finding card's row: its card's values; its summary and rationale are
 *  its words. */
export function findingRow(card: StreamCard): EventRow {
  return {
    card,
    row: {
      run_id: card.runId,
      project: card.projectName,
      agent: card.agent,
      step_id: card.stepId,
      codename: card.codename,
      provider: card.provider,
      model: card.model,
      severity: card.severity,
      message: card.title,
      note: card.detail,
    },
  };
}

const iso = (ms: number) => new Date(ms).toISOString();

export const EVENT_FIELDS: RowField<EventRow>[] = [
  rowField({ key: 'group', values: ['finding', 'activity', 'await', 'warning', 'error'], match: 'exact', get: (r) => r.card.group, facet: true, label: 'Group', description: 'group:error · group:await · group:finding', icon: Layers }),
  rowField({ key: 'type', match: 'exact', get: (r) => r.row.type, facet: true, label: 'Type', description: 'type:step_failed (the event type)', icon: Tag }),
  rowField({ key: 'run', match: 'prefix', get: (r) => r.row.run_id, text: true, label: 'Run', description: 'run:run_01J (prefix)', icon: Play }),
  rowField({ key: 'project', match: 'contains', get: (r) => r.row.project, text: true, facet: true, label: 'Project', description: 'project:shop-web', icon: FolderGit2 }),
  rowField({ key: 'workflow', match: 'contains', get: (r) => r.row.workflow, text: true, facet: true, label: 'Workflow', description: 'workflow:nightly (contains)', icon: Workflow }),
  rowField({ key: 'agent', match: 'contains', get: (r) => r.row.agent, text: true, facet: true, label: 'Agent', description: 'agent:recon (contains)', icon: Bot }),
  rowField({ key: 'step', match: 'contains', get: (r) => r.row.step_id, text: true, facet: true, label: 'Step', description: 'step:scan (contains)', icon: Activity }),
  rowField({ key: 'codename', match: 'contains', get: (r) => [r.row.codename, r.row.run_codename], text: true, label: 'Codename', description: 'codename:heron · codename:cobalt-harbor (the run)', icon: Fingerprint }),
  rowField({ key: 'provider', match: 'exact', get: (r) => r.row.provider, facet: true, label: 'Provider', description: 'provider:anthropic', icon: Server }),
  rowField({ key: 'model', match: 'exact', get: (r) => r.row.model, facet: true, label: 'Model', description: 'model:claude-opus-4-1', icon: Cpu }),
  rowField({ key: 'severity', aliases: ['sev'], values: ['info', 'low', 'medium', 'high', 'critical'], match: 'ordered', get: (r) => r.row.severity, facet: true, label: 'Severity', description: 'severity>=high (findings)', icon: ShieldAlert }),
  rowField({ key: 'message', match: 'contains', get: (r) => [r.row.error, r.row.reason, r.row.message, r.row.note], text: true, label: 'Message', description: 'message:timeout (an error, reason, warning or note)', icon: MessageSquare }),
  rowField({ key: 'since', match: 'since', get: (r) => iso(r.card.ts), label: 'Since', description: 'since:30m · since:2026-10-01', icon: History }),
  rowField({ key: 'until', match: 'until', get: (r) => iso(r.card.ts), label: 'Until', description: 'until:1h (older than) · until:2026-10-01', icon: CalendarClock }),
];

/** Shown under the bar while a query narrows the stream. */
export const EVENT_HISTORY_NOTE =
  "Event history holds the most recent runs the query's run:, project: and workflow: terms keep; the run lists hold older ones.";
