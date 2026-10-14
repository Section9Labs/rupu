// The Hosts list's query fields.
import { Activity, CalendarClock, Cable, CircleCheck, Cpu, History, Link, Server, Tag } from 'lucide-react';
import type { HostView } from '../../api';
import { rowField, type RowField } from '../rows';

export const HOST_FIELDS: RowField<HostView>[] = [
  rowField({ key: 'name', match: 'contains', get: (h) => [h.name, h.id], text: true, label: 'Name', description: 'name:sutajio (name or id, contains)', icon: Server }),
  rowField({ key: 'transport', values: ['local', 'http_cp', 'tunnel', 'ssh', 'bucket'], match: 'exact', get: (h) => h.transport_kind, facet: true, label: 'Transport', description: 'transport:ssh · -transport:local', icon: Cable }),
  rowField({ key: 'status', values: ['online', 'offline', 'stale'], match: 'exact', get: (h) => h.status, facet: true, label: 'Status', description: 'status:offline', icon: Activity }),
  rowField({ key: 'version', match: 'prefix', get: (h) => h.version, facet: true, label: 'Version', description: 'version:0.80 (prefix)', icon: Tag }),
  rowField({ key: 'url', match: 'contains', get: (h) => h.base_url, text: true, label: 'URL', description: 'url:10.9. (contains)', icon: Link }),
  rowField({ key: 'backend', match: 'exact', get: (h) => h.capabilities?.backends, facet: true, label: 'Backend', description: 'backend:anthropic', icon: Cpu }),
  rowField({ key: 'has', values: ['active_runs'], match: 'exact', get: (h) => (h.active_run_count > 0 ? ['active_runs'] : []), facet: true, label: 'Has', description: 'has:active_runs', icon: CircleCheck }),
  rowField({ key: 'since', match: 'since', get: (h) => h.last_seen_at, label: 'Seen since', description: 'since:10m (last seen)', icon: History }),
  rowField({ key: 'until', match: 'until', get: (h) => h.last_seen_at, label: 'Seen until', description: 'until:1d (last seen before)', icon: CalendarClock }),
];
