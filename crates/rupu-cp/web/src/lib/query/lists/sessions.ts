// The session list's query fields (`GET /api/sessions?q=`,
// `GET /api/projects/:ws_id/sessions?q=`). The server evaluates them.
import { Archive, Bot, CircleDot, Cpu, FolderGit2, Plug } from 'lucide-react';
import { withDefault } from '../control';
import { serverField, type ServerField } from '../rows';
import { CODENAME_FIELD, CUSTOMER_FIELD, HOST_FIELD, ID_FIELD, timeFields } from './shared';

export const SESSION_FIELDS: ServerField[] = [
  serverField({ key: 'status', values: ['idle', 'running', 'failed', 'stopped'], match: 'exact', facet: true, label: 'Status', description: 'status:running · -status:stopped', icon: CircleDot }),
  serverField({ key: 'agent', match: 'contains', text: true, facet: true, label: 'Agent', description: 'agent:recon (contains)', icon: Bot }),
  serverField({ key: 'model', match: 'contains', facet: true, label: 'Model', description: 'model:opus (contains)', icon: Cpu }),
  serverField({ key: 'provider', match: 'exact', facet: true, label: 'Provider', description: 'provider:anthropic', icon: Plug }),
  CODENAME_FIELD,
  ID_FIELD,
  HOST_FIELD,
  CUSTOMER_FIELD,
  serverField({ key: 'project', aliases: ['workspace'], match: 'exact', facet: true, label: 'Project', description: 'project:<workspace id>', icon: FolderGit2 }),
  serverField({ key: 'is', values: ['active', 'archived'], match: 'exact', facet: true, label: 'Is', description: 'is:archived · is:active,archived (all; active when absent)', icon: Archive }),
  ...timeFields('Created'),
];

/** The query the server gets: active sessions when it names no `is:`. */
export function sessionsQuery(q: string): string {
  return withDefault(q, SESSION_FIELDS, 'is', 'is:active');
}
