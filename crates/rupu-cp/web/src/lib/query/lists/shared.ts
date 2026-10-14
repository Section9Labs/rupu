// Fields the server-filtered lists (workflow runs, agent runs, sessions)
// share, as `crates/rupu-cp/src/query/mod.rs`' `shared` defines them.
import { Building2, CalendarClock, Fingerprint, Hash, History, Server } from 'lucide-react';
import { serverField, type ServerField } from '../rows';

export const HOST_FIELD: ServerField = serverField({ key: 'host', match: 'exact', facet: true, label: 'Host', description: 'host:local · host:sutajio · -host:mini', icon: Server });

export const CUSTOMER_FIELD: ServerField = serverField({ key: 'customer', match: 'exact', facet: true, label: 'Customer', description: 'customer:acme · customer:none', icon: Building2 });

export const CODENAME_FIELD: ServerField = serverField({ key: 'codename', match: 'contains', text: true, label: 'Codename', description: 'codename:cobalt-harbor (contains)', icon: Fingerprint });

export const ID_FIELD: ServerField = serverField({ key: 'id', match: 'prefix', text: true, label: 'Id', description: 'id:run_01J (prefix)', icon: Hash });

/** `since:` / `until:` on the row's start. */
export function timeFields(noun: string): ServerField[] {
  return [
    serverField({ key: 'since', match: 'since', label: `${noun} since`, description: 'since:7d · since:2026-10-01 (a UTC day)', icon: History }),
    serverField({ key: 'until', match: 'until', label: `${noun} until`, description: 'until:7d (older than) · until:2026-10-01', icon: CalendarClock }),
  ];
}
