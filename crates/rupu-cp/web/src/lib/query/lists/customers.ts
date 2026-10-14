// The Customers list's query fields. `range:` is not a row filter: it is the
// window the server rolls each customer's cost, runs and findings over
// (`GET /api/customers?range=`, which takes 7d, 30d or all), so the page
// reads it with `customersRange` and strips it before filtering.
import { Building2, CalendarRange, CircleCheck, Hash, KeyRound, Archive } from 'lucide-react';
import type { CustomerRow, DashboardRange } from '../../api';
import { controlTerm, withDefault } from '../control';
import { rowField, type RowField } from '../rows';

export const CUSTOMER_RANGES: DashboardRange[] = ['7d', '30d', 'all'];
/** The rollup window when the query names none. */
export const DEFAULT_CUSTOMER_RANGE: DashboardRange = '30d';

function customerFlags(r: CustomerRow): string[] {
  const out: string[] = [];
  if (r.rollup.findings_open > 0) out.push('findings');
  if (r.rollup.run_count > 0) out.push('runs');
  if (r.rollup.projects > 0) out.push('projects');
  if (r.layer_error) out.push('layer_error');
  return out;
}

export const CUSTOMER_FIELDS: RowField<CustomerRow>[] = [
  rowField({ key: 'name', match: 'contains', get: (r) => [r.name, r.slug], text: true, label: 'Name', description: 'name:acme (name or slug, contains)', icon: Building2 }),
  rowField({ key: 'slug', match: 'exact', get: (r) => r.slug, label: 'Slug', description: 'slug:acme', icon: Hash }),
  rowField({ key: 'is', values: ['active', 'archived'], match: 'exact', get: (r) => (r.archived ? 'archived' : 'active'), facet: true, label: 'Is', description: 'is:archived · is:active,archived (all; active when absent)', icon: Archive }),
  rowField({ key: 'range', values: [...CUSTOMER_RANGES], match: 'exact', get: () => undefined, label: 'Range', description: 'range:7d · range:all (the rollup window; 30d when absent)', icon: CalendarRange }),
  rowField({ key: 'account', match: 'exact', get: (r) => r.default_account?.account, facet: true, label: 'Default account', description: 'account:acme-prod', icon: KeyRound }),
  rowField({ key: 'has', values: ['findings', 'runs', 'projects', 'layer_error'], match: 'exact', get: customerFlags, facet: true, label: 'Has', description: 'has:findings · has:layer_error · -has:runs', icon: CircleCheck }),
];

/** The rollup range a query names, the query without it, and why its
 *  `range:` can't be used. */
export function customersRange(q: string): { range: DashboardRange; rest: string; error: string | null } {
  const t = controlTerm(q, CUSTOMER_FIELDS, 'range', 'range: names one window (7d, 30d or all)');
  return { range: (t.value as DashboardRange | null) ?? DEFAULT_CUSTOMER_RANGE, rest: t.rest, error: t.error };
}

/** The query as it filters rows: without `range:`, and with `is:active` when
 *  it names no `is:` (archived customers stay out unless asked for, even
 *  while another token is broken). */
export function customersRowQuery(rest: string): string {
  return withDefault(rest, CUSTOMER_FIELDS, 'is', 'is:active');
}
