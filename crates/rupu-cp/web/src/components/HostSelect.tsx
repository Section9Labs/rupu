// HostSelect — a small dropdown that lists registered hosts via api.getHosts()
// and emits the chosen host_id. Defaults to "local". Falls back to a single
// "Local" option when the hosts fetch fails or has not resolved yet.
//
// Restyled internally onto `ui/Select`'s shared chrome (visual parity — same
// classes, now sourced from one place) per the One Control Language kit.

import { useEffect, useState } from 'react';
import { api, type HostView } from '../lib/api';
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
}

export default function HostSelect({ value, onChange, disabled, className }: Props) {
  const [hosts, setHosts] = useState<HostView[] | null>(null);

  useEffect(() => {
    let cancelled = false;
    api
      .getHosts()
      .then((hs) => {
        if (!cancelled) setHosts(hs);
      })
      .catch(() => {
        if (!cancelled) setHosts([]);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <Select
      value={value}
      onChange={(e) => onChange(e.target.value)}
      disabled={disabled}
      aria-label="Host"
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
