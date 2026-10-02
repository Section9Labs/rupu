// HostSelect — a small dropdown of hosts that emits the chosen host_id. The
// allowAll (list-filter) variant reads the probe-free api.getRegisteredHosts();
// the launcher variant reads api.getHosts(), whose status it shows. The
// launcher variant falls back to a single "Local" option while the hosts fetch
// is pending or when it fails; the allowAll variant to "This host" + "All hosts".
//
// Restyled internally onto `ui/Select`'s shared chrome (visual parity — same
// classes, now sourced from one place) per the One Control Language kit.
//
// `allowAll` switches on the fan-out variant the run-list pages need: "This
// host" (local) + "All hosts" (value = ALL_HOSTS) + the registered non-local
// hosts, absorbing what used to be page-local
// host-listing logic duplicated across WorkflowRuns/AgentRuns. Default false
// keeps the launcher-sheet consumers (LauncherSheet, AgentLauncherSheet)
// unchanged.

import { useEffect, useState } from 'react';
import { api, type HostView, type RegisteredHostView } from '../lib/api';
import { Select } from './ui/Select';

/** Sentinel host-id meaning "fetch all hosts" (fan-out / no `?host=` param).
 *  Was duplicated as a local `const ALL_HOSTS = '__all__'` in 4 list pages;
 *  those migrate onto this export in Phase 2. */
export const ALL_HOSTS = '__all__';

interface Props {
  value: string;
  onChange: (hostId: string) => void;
  disabled?: boolean;
  className?: string;
  /** Render the "This host" / registered hosts / "All hosts" fan-out list
   *  instead of the plain registered-hosts list. Default false. */
  allowAll?: boolean;
  /** Override the default `aria-label` ("Host"). WorkflowRuns passes
   *  "Host filter" to keep its pre-migration label unchanged. */
  ariaLabel?: string;
}

export default function HostSelect({
  value,
  onChange,
  disabled,
  className,
  allowAll = false,
  ariaLabel = 'Host',
}: Props) {
  // The All-hosts filter only needs ids and names — the probe-free
  // `/api/hosts/registered` (spec 2026-10-01 §6.4). The launcher variant keeps
  // `/api/hosts`: its "(offline)" suffix matters when choosing where to launch.
  const [hosts, setHosts] = useState<HostView[] | null>(null);
  const [registered, setRegistered] = useState<RegisteredHostView[] | null>(null);

  useEffect(() => {
    let cancelled = false;
    if (allowAll) {
      api
        .getRegisteredHosts()
        .then((hs) => {
          if (!cancelled) setRegistered(hs);
        })
        .catch(() => {
          if (!cancelled) setRegistered([]);
        });
    } else {
      api
        .getHosts()
        .then((hs) => {
          if (!cancelled) setHosts(hs);
        })
        .catch(() => {
          if (!cancelled) setHosts([]);
        });
    }
    return () => {
      cancelled = true;
    };
  }, [allowAll]);

  if (allowAll) {
    return (
      <Select
        value={value}
        onChange={(e) => onChange(e.target.value)}
        disabled={disabled}
        aria-label={ariaLabel}
        className={className}
      >
        <option value="local">This host</option>
        <option value={ALL_HOSTS}>All hosts</option>
        {(registered ?? [])
          .filter((h) => h.transport_kind !== 'local')
          .map((h) => (
            <option key={h.id} value={h.id}>
              {h.name}
            </option>
          ))}
      </Select>
    );
  }

  return (
    <Select
      value={value}
      onChange={(e) => onChange(e.target.value)}
      disabled={disabled}
      aria-label={ariaLabel}
      className={className}
    >
      {/* While loading or empty, keep a stable local option so the select is always usable. */}
      {!hosts || hosts.length === 0 ? (
        <option value="local">Local</option>
      ) : (
        hosts.map((h) => (
          <option key={h.id} value={h.id}>
            {h.name}
            {h.status !== 'online' ? ` (${h.status})` : ''}
          </option>
        ))
      )}
    </Select>
  );
}
