// The agentiflow run list's query fields.
import { Activity, CalendarClock, CircleCheck, Hash, History, Layers, OctagonX, Users, Workflow } from 'lucide-react';
import type { AgentiflowRow } from '../../api';
import { rowField, type RowField } from '../rows';

/** A stop reason's kind: the word before any `:` detail (`budget_exhausted`
 *  of `budget_exhausted: tokens`). */
const stopKind = (r: AgentiflowRow) => r.stop_reason?.split(':')[0].trim();

function agentiflowFlags(r: AgentiflowRow): string[] {
  const out: string[] = [];
  if (r.goals_total > 0 && r.goals_met === r.goals_total) out.push('goals_met');
  if (r.status === 'running' && r.runner_alive === false) out.push('coordinator_gone');
  return out;
}

export const AGENTIFLOW_FIELDS: RowField<AgentiflowRow>[] = [
  rowField({ key: 'name', match: 'contains', get: (r) => r.name, text: true, label: 'Agentiflow', description: 'name:recon (contains)', icon: Workflow }),
  rowField({ key: 'codename', aliases: ['crew'], match: 'contains', get: (r) => r.codename, text: true, label: 'Codename', description: 'codename:cobalt-harbor', icon: Users }),
  rowField({ key: 'status', values: ['running', 'completed', 'failed'], match: 'exact', get: (r) => r.status, facet: true, label: 'Status', description: 'status:running · -status:completed', icon: Activity }),
  rowField({ key: 'stop', match: 'prefix', get: (r) => r.stop_reason, facet: stopKind, label: 'Stop reason', description: 'stop:budget_exhausted · stop:error (prefix)', icon: OctagonX }),
  rowField({ key: 'profile', match: 'exact', get: (r) => r.engagement_profiles, facet: true, label: 'Engagement profile', description: 'profile:network', icon: Layers }),
  rowField({ key: 'has', values: ['goals_met', 'coordinator_gone'], match: 'exact', get: agentiflowFlags, facet: true, label: 'Has', description: 'has:goals_met · has:coordinator_gone', icon: CircleCheck }),
  rowField({ key: 'since', match: 'since', get: (r) => r.started_at, label: 'Started since', description: 'since:24h · since:2026-10-01', icon: History }),
  rowField({ key: 'until', match: 'until', get: (r) => r.started_at, label: 'Started until', description: 'until:7d (started before)', icon: CalendarClock }),
  rowField({ key: 'id', match: 'exact', get: (r) => r.id, label: 'Id', description: 'id:af_…', icon: Hash }),
];
