// The agent-run list's query fields (`GET /api/runs/agents?q=`). The server
// evaluates them.
import { Bot, CircleDot, Cpu, Layers, MessageSquare, Plug, Zap } from 'lucide-react';
import { withDefault } from '../control';
import { serverField, type ServerField } from '../rows';
import { CODENAME_FIELD, CUSTOMER_FIELD, HOST_FIELD, ID_FIELD, timeFields } from './shared';

export const AGENT_RUN_FIELDS: ServerField[] = [
  serverField({ key: 'status', match: 'exact', facet: true, label: 'Status', description: 'status:error · status:ok', icon: CircleDot }),
  serverField({ key: 'agent', match: 'contains', text: true, facet: true, label: 'Agent', description: 'agent:recon (contains)', icon: Bot }),
  serverField({ key: 'provider', match: 'exact', facet: true, label: 'Provider', description: 'provider:anthropic', icon: Plug }),
  serverField({ key: 'model', match: 'contains', facet: true, label: 'Model', description: 'model:opus (contains)', icon: Cpu }),
  serverField({ key: 'source', values: ['standalone', 'session'], match: 'exact', facet: true, label: 'Source', description: 'source:session · source:standalone,session (standalone when absent)', icon: Layers }),
  serverField({ key: 'session', match: 'prefix', label: 'Session', description: 'session:ses_01J (prefix)', icon: MessageSquare }),
  serverField({ key: 'trigger', match: 'exact', facet: true, label: 'Trigger', description: 'trigger:cli', icon: Zap }),
  CODENAME_FIELD,
  ID_FIELD,
  HOST_FIELD,
  CUSTOMER_FIELD,
  ...timeFields('Started'),
];

/** The query the server gets: standalone runs when it names no `source:`. */
export function agentRunsQuery(q: string): string {
  return withDefault(q, AGENT_RUN_FIELDS, 'source', 'source:standalone');
}
