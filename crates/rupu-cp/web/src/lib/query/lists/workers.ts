// The Workers list's query fields.
import { Activity, CalendarClock, CircleCheck, Cpu, GitBranch, History, Monitor, Shapes, ShieldCheck, Tag } from 'lucide-react';
import type { WorkerView } from '../../api';
import { rowField, type RowField } from '../rows';

/** A worker not seen for this long reads as stale. */
export const WORKER_STALE_MS = 5 * 60 * 1000;

export function isWorkerStale(lastSeen: string, now = Date.now()): boolean {
  const t = Date.parse(lastSeen);
  return !Number.isNaN(t) && now - t > WORKER_STALE_MS;
}

export const WORKER_FIELDS: RowField<WorkerView>[] = [
  rowField({ key: 'name', match: 'contains', get: (w) => [w.name, w.worker_id], text: true, label: 'Name', description: 'name:mbp (name or id, contains)', icon: Monitor }),
  rowField({ key: 'kind', match: 'exact', get: (w) => w.kind, facet: true, label: 'Kind', description: 'kind:cli · kind:autoflow_serve', icon: Shapes }),
  rowField({ key: 'host', match: 'contains', get: (w) => w.host, facet: true, text: true, label: 'Host', description: 'host:mbp.local (contains)', icon: Monitor }),
  rowField({ key: 'status', values: ['live', 'stale'], match: 'exact', get: (w) => (isWorkerStale(w.last_seen_at) ? 'stale' : 'live'), facet: true, label: 'Status', description: 'status:stale (not seen in 5 minutes)', icon: Activity }),
  rowField({ key: 'backend', match: 'exact', get: (w) => w.capabilities?.backends, facet: true, label: 'Backend', description: 'backend:anthropic', icon: Cpu }),
  rowField({ key: 'scm', match: 'exact', get: (w) => w.capabilities?.scm_hosts, facet: true, label: 'SCM host', description: 'scm:github.com', icon: GitBranch }),
  rowField({ key: 'mode', match: 'exact', get: (w) => w.capabilities?.permission_modes, facet: true, label: 'Permission mode', description: 'mode:bypass', icon: ShieldCheck }),
  rowField({ key: 'version', match: 'exact', get: (w) => w.version, facet: true, label: 'Version', description: 'version:1', icon: Tag }),
  rowField({ key: 'has', values: ['active_runs', 'runs'], match: 'exact', get: (w) => [...(w.active_run_count > 0 ? ['active_runs'] : []), ...(w.total_run_count > 0 ? ['runs'] : [])], facet: true, label: 'Has', description: 'has:active_runs · -has:runs', icon: CircleCheck }),
  rowField({ key: 'since', match: 'since', get: (w) => w.last_seen_at, label: 'Seen since', description: 'since:1h (last seen)', icon: History }),
  rowField({ key: 'until', match: 'until', get: (w) => w.last_seen_at, label: 'Seen until', description: 'until:1d (last seen before)', icon: CalendarClock }),
];
