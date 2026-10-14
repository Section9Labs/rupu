// The Live Events stream's query fields — the twin of the server's
// `crates/rupu-cp/src/query/events.rs`, which filters the event history
// (`GET /api/events?q=`); the page evaluates the same registry over every
// card it holds (history, live frames and findings). Pinned to the server
// by `tests/fixtures/list_query/events.fields.json` and the group table in
// `events.groups.json`.
import { Activity, Bot, CalendarClock, Cpu, Fingerprint, FolderGit2, History, Layers, MessageSquare, Play, Server, ShieldAlert, Tag, Workflow } from 'lucide-react';
import type { RunEvent } from '../../api';
import type { StreamCard } from '../../situationRoom/cards';
import { rowField, type RowField } from '../rows';

/** A stream card with what the query reads beyond it: the event's type and
 *  words, and its run's attribution (from the history row, else the page's
 *  run lookups). */
export interface EventRow {
  card: StreamCard;
  type?: string;
  project?: string;
  workflow?: string;
  runCodename?: string;
  message: string[];
}

/** The keys `GET /api/events` adds to a history row beyond the event. */
export const HISTORY_ATTRIBUTION_KEYS = ['workflow', 'project', 'run_codename'] as const;

function str(v: unknown): string | undefined {
  return typeof v === 'string' && v !== '' ? v : undefined;
}

/** An event card's row: the event's own words and its history attribution,
 *  else the page's lookups (`lookup`) for the run. */
export function eventRow(
  card: StreamCard,
  ev: RunEvent,
  lookup: { project?: string; workflow?: string; runCodename?: string },
): EventRow {
  const raw = ev as Record<string, unknown>;
  return {
    card,
    type: ev.type,
    project: str(raw.project) ?? card.projectName ?? lookup.project,
    workflow: str(raw.workflow) ?? card.workflow ?? lookup.workflow,
    runCodename: str(raw.run_codename) ?? lookup.runCodename,
    message: ['error', 'reason', 'message', 'note'].map((k) => str(raw[k])).filter((x): x is string => x !== undefined),
  };
}

/** A finding card's row: its summary and rationale are its words. */
export function findingRow(card: StreamCard): EventRow {
  return {
    card,
    project: card.projectName,
    message: [card.title, card.detail].filter((x): x is string => !!x),
  };
}

const iso = (ms: number) => new Date(ms).toISOString();

export const EVENT_FIELDS: RowField<EventRow>[] = [
  rowField({ key: 'group', values: ['finding', 'activity', 'await', 'warning', 'error'], match: 'exact', get: (r) => r.card.group, facet: true, label: 'Group', description: 'group:error · group:await · group:finding', icon: Layers }),
  rowField({ key: 'type', match: 'exact', get: (r) => r.type, facet: true, label: 'Type', description: 'type:step_failed (the event type)', icon: Tag }),
  rowField({ key: 'run', match: 'prefix', get: (r) => r.card.runId, text: true, label: 'Run', description: 'run:run_01J (prefix)', icon: Play }),
  rowField({ key: 'project', match: 'contains', get: (r) => r.project, text: true, facet: true, label: 'Project', description: 'project:shop-web', icon: FolderGit2 }),
  rowField({ key: 'workflow', match: 'contains', get: (r) => r.workflow, text: true, facet: true, label: 'Workflow', description: 'workflow:nightly (contains)', icon: Workflow }),
  rowField({ key: 'agent', match: 'contains', get: (r) => r.card.agent, text: true, facet: true, label: 'Agent', description: 'agent:recon (contains)', icon: Bot }),
  rowField({ key: 'step', match: 'contains', get: (r) => r.card.stepId, text: true, facet: true, label: 'Step', description: 'step:scan (contains)', icon: Activity }),
  rowField({ key: 'codename', match: 'contains', get: (r) => [r.card.codename, r.runCodename], text: true, label: 'Codename', description: 'codename:heron · codename:cobalt-harbor (the run)', icon: Fingerprint }),
  rowField({ key: 'provider', match: 'exact', get: (r) => r.card.provider, facet: true, label: 'Provider', description: 'provider:anthropic', icon: Server }),
  rowField({ key: 'model', match: 'exact', get: (r) => r.card.model, facet: true, label: 'Model', description: 'model:claude-opus-4-1', icon: Cpu }),
  rowField({ key: 'severity', aliases: ['sev'], values: ['info', 'low', 'medium', 'high', 'critical'], match: 'ordered', get: (r) => r.card.severity, facet: true, label: 'Severity', description: 'severity>=high (findings)', icon: ShieldAlert }),
  rowField({ key: 'message', match: 'contains', get: (r) => r.message, text: true, label: 'Message', description: 'message:timeout (an error, reason, warning or note)', icon: MessageSquare }),
  rowField({ key: 'since', match: 'since', get: (r) => iso(r.card.ts), label: 'Since', description: 'since:30m · since:2026-10-01', icon: History }),
  rowField({ key: 'until', match: 'until', get: (r) => iso(r.card.ts), label: 'Until', description: 'until:1h (older than) · until:2026-10-01', icon: CalendarClock }),
];

/** Shown under the bar while a query narrows the stream. */
export const EVENT_HISTORY_NOTE =
  "Event history holds the most recent runs the query's run:, project: and workflow: terms keep; the run lists hold older ones.";
