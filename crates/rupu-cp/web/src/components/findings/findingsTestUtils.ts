import type { api, FindingOut, FindingsResponse } from '../../lib/api';

const SEVS = ['critical', 'high', 'medium', 'low', 'info'];

/** A fake `api.getFindings` over `rows` (the scope): it honours a lone
 *  `severity:<x>` token in `q`, and its severity facets count the scope
 *  before `q`, as the real server's do. */
export function findingsServer(rows: FindingOut[]): typeof api.getFindings {
  return async (opts) => {
    const sev = /severity:(\w+)/.exec(opts?.q ?? '')?.[1];
    const findings = sev ? rows.filter((r) => r.severity === sev) : rows;
    const count = (s: string) => rows.filter((r) => r.severity === s).length;
    const resp: FindingsResponse = {
      findings,
      summary: {
        total: rows.length,
        critical: count('critical'),
        high: count('high'),
        medium: count('medium'),
        low: count('low'),
        info: count('info'),
      },
      facets: { severity: SEVS.map((value) => ({ value, count: count(value) })) },
      tags_unavailable: [],
    };
    return resp;
  };
}
