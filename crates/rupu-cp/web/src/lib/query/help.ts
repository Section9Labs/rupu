// The query bar's "?" fields help: which server list a registry filters
// (`GET /api/query/:list/fields` is then the source of truth, so the help
// can't advertise a field the server doesn't take), and the rows the popover
// shows — key, aliases, kind, values and one example each.

import type { QueryField } from '../findingQuery/fields';
import { FINDING_FIELDS } from '../findingQuery/fields';
import type { ServerQueryField, ServerQueryList } from '../api3b/library';
import { AGENT_RUN_FIELDS } from './lists/agentRuns';
import { AUTOFLOW_RUN_FIELDS } from './lists/autoflowRuns';
import { EVENT_FIELDS } from './lists/events';
import { SESSION_FIELDS } from './lists/sessions';
import { WORKFLOW_RUN_FIELDS } from './lists/workflowRuns';

/** The registries the server lockstep pins, by identity. */
const SERVER_LISTS = new Map<readonly QueryField[], ServerQueryList>([
  [FINDING_FIELDS, 'findings'],
  [WORKFLOW_RUN_FIELDS, 'workflow_runs'],
  [AGENT_RUN_FIELDS, 'agent_runs'],
  [SESSION_FIELDS, 'sessions'],
  [AUTOFLOW_RUN_FIELDS, 'autoflow_runs'],
  [EVENT_FIELDS, 'events'],
]);

/** The server list whose registry `fields` is, or `null` for a list the
 *  browser filters with a registry of its own. */
export function serverListOf(fields: readonly QueryField[]): ServerQueryList | null {
  return SERVER_LISTS.get(fields) ?? null;
}

export interface HelpRow {
  key: string;
  aliases: string[];
  /** The kind, in words. */
  kind: string;
  values: string[];
  example: string;
}

function kindText(kind: string): string {
  switch (kind) {
    case 'severity':
      return 'ordered · supports >= <=';
    case 'enum':
      return 'one of';
    case 'time':
      return 'time · 30m, 12h, 7d, 2w, a date or RFC 3339';
    default:
      return kind;
  }
}

function madeUpExample(f: ServerQueryField): string {
  if (f.kind === 'time') return `${f.key}:7d`;
  if (f.values.length > 0) return `${f.key}:${f.values[0]}`;
  return `${f.key}:…`;
}

/** The popover's rows: the server's fields when it answered (in its order),
 *  each with the local field's example where the local registry has one;
 *  else the local registry itself. */
export function helpRows(local: readonly QueryField[], server: readonly ServerQueryField[] | null): HelpRow[] {
  const fields: readonly ServerQueryField[] = server ?? local;
  return fields.map((f) => {
    const mine = local.find((l) => l.key === f.key);
    return {
      key: f.key,
      aliases: [...f.aliases],
      kind: kindText(f.kind),
      values: [...f.values],
      example: mine?.description ?? madeUpExample(f),
    };
  });
}
