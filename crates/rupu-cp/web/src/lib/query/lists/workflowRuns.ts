// The workflow-run list's query fields (`GET /api/runs/workflows?q=`,
// `GET /api/projects/:ws_id/runs?q=`). The server evaluates them.
import { Archive, CircleDot, Workflow, Zap } from 'lucide-react';
import { parseToken, tokenize } from '../../findingQuery/grammar';
import { serverField, type ServerField } from '../rows';
import { CODENAME_FIELD, CUSTOMER_FIELD, HOST_FIELD, ID_FIELD, timeFields } from './shared';

export const RUN_STATUSES = ['pending', 'running', 'awaiting_approval', 'paused', 'completed', 'failed', 'rejected', 'cancelled'];

export const WORKFLOW_RUN_FIELDS: ServerField[] = [
  serverField({ key: 'status', values: RUN_STATUSES, match: 'exact', facet: true, label: 'Status', description: 'status:failed · status:completed,cancelled', icon: CircleDot }),
  serverField({ key: 'workflow', match: 'contains', text: true, facet: true, label: 'Workflow', description: 'workflow:nightly (contains)', icon: Workflow }),
  ID_FIELD,
  CODENAME_FIELD,
  serverField({ key: 'trigger', values: ['manual', 'cron', 'event'], match: 'exact', facet: true, label: 'Trigger', description: 'trigger:cron · -trigger:manual', icon: Zap }),
  HOST_FIELD,
  CUSTOMER_FIELD,
  ...timeFields('Started'),
  serverField({ key: 'is', values: ['archived'], match: 'exact', label: 'Is', description: 'is:archived (the archive, this host only)', icon: Archive }),
];

/** Whether a positive `is:archived` term asks for the archive. `fields` is
 *  the page's registry (a list that extends this one parses with its own). */
export function asksArchived(q: string, fields: readonly ServerField[] = WORKFLOW_RUN_FIELDS): boolean {
  const t = tokenize(q);
  return (
    t.ok &&
    t.tokens.some((x) => {
      const p = parseToken(x.text, fields);
      return p.ok && p.terms[0]?.key === 'is' && !p.terms[0].neg && p.terms[0].values.includes('archived');
    })
  );
}
