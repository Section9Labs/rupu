// The warn banner a customer-scoped view shows for hosts it had to leave out
// (docs/cp-customers-api.md, "Remote hosts") — never counted as zero, so the
// view says which hosts are missing and why. Two causes, from three sources:
//
// - A host that can't tag every run with a customer (an older rupu, or a
//   mirror holding a worker's runs from before customers):
//     · a per-host slice `unavailable` with the single-host 501 reason
//       ("host <id> can't report a customer for every run"),
//     · the `X-Rupu-Hosts-Without-Customer` header of a scoped list / usage
//       request, and the `hosts_without_customer` arrays of `/api/usage`,
//       `/api/dashboard` and the customer rollups (`without`, host ids).
// - A remote host whose aggregate totals are summed on the host, so a
//   coordinator can't filter them (`/api/usage`, `/api/dashboard` under a
//   filter: "host <id> can't be filtered by customer: …").
//
// Unscoped requests never produce any of these, so the banner needs no scope
// check of its own: it renders whenever a host qualifies, nothing otherwise.

import { AlertTriangle } from 'lucide-react';
import { cn } from '../../lib/cn';

export interface BannerHost {
  /** The host id, used to name the ids `without` lists. */
  id?: string;
  name: string;
  state: string;
  reason: string | null | undefined;
}

const UNREPORTABLE = /can[’']t report (a customer|customers)/i;
const AGGREGATE = /can[’']t be filtered by customer/i;

function joinNames(names: string[]): string {
  if (names.length <= 1) return names.join('');
  return `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}`;
}

/** The hosts left out, by cause. Exported for the pages' own tests. */
export function hostsLeftOut(
  hosts: readonly BannerHost[],
  without: readonly string[],
): { unreportable: string[]; aggregate: string[] } {
  const nameOf = new Map(hosts.filter((h) => h.id).map((h) => [h.id as string, h.name]));
  const unreportable = new Set<string>();
  const aggregate = new Set<string>();
  for (const h of hosts) {
    if (h.state !== 'unavailable' || !h.reason) continue;
    if (UNREPORTABLE.test(h.reason)) unreportable.add(h.name);
    else if (AGGREGATE.test(h.reason)) aggregate.add(h.name);
  }
  for (const id of without) unreportable.add(nameOf.get(id) ?? id);
  return { unreportable: [...unreportable].sort(), aggregate: [...aggregate].sort() };
}

export function HostsWithoutCustomerBanner({
  hosts = [],
  without = [],
  className,
}: {
  hosts?: readonly BannerHost[];
  without?: readonly string[];
  className?: string;
}) {
  const { unreportable, aggregate } = hostsLeftOut(hosts, without);
  if (unreportable.length === 0 && aggregate.length === 0) return null;
  return (
    <div
      role="status"
      data-testid="hosts-without-customer"
      className={cn(
        'flex items-start gap-2 rounded-lg border border-warn/30 bg-warn-bg px-3 py-2 text-note text-warn',
        className,
      )}
    >
      <AlertTriangle size={13} aria-hidden className="mt-0.5 shrink-0" />
      <div className="space-y-0.5">
        {unreportable.length > 0 && (
          <p>
            {joinNames(unreportable)} {unreportable.length === 1 ? 'runs' : 'run'} an older rupu or{' '}
            {unreportable.length === 1 ? 'holds' : 'hold'} runs from before customers, so{' '}
            {unreportable.length === 1 ? 'it' : 'they'} can’t tag every run with a customer — those runs
            are left out of this view, not counted as zero.
          </p>
        )}
        {aggregate.length > 0 && (
          <p>
            {joinNames(aggregate)} can’t be filtered by customer ({aggregate.length === 1 ? 'its' : 'their'}{' '}
            totals are summed on the host) — left out of this view, not counted as zero.
          </p>
        )}
      </div>
    </div>
  );
}
