import type { RunListRow } from './api';

/** The detail link for a run, with `?host=` when the run's own `host_id` isn't
 *  local (an absent `host_id` — an older server — is local). */
export function runHref(r: Pick<RunListRow, 'id' | 'host_id'>): string {
  const hid = r.host_id;
  if (hid && hid !== 'local') {
    return `/runs/${encodeURIComponent(r.id)}?host=${encodeURIComponent(hid)}`;
  }
  return `/runs/${encodeURIComponent(r.id)}`;
}
