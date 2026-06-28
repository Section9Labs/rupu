// HostSelect — a small dropdown that lists registered hosts via api.getHosts()
// and emits the chosen host_id. Defaults to "local". Falls back to a single
// "Local" option when the hosts fetch fails or has not resolved yet.

import { useEffect, useState } from 'react';
import { api, type HostView } from '../lib/api';
import { cn } from '../lib/cn';

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

  const baseCls =
    'rounded-md border border-border bg-panel px-2.5 py-1.5 text-lead text-ink ' +
    'focus:border-brand-500 focus:outline-none disabled:cursor-not-allowed disabled:opacity-60';

  return (
    <select
      value={value}
      onChange={(e) => onChange(e.target.value)}
      disabled={disabled}
      aria-label="Host"
      className={cn(baseCls, className)}
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
    </select>
  );
}
