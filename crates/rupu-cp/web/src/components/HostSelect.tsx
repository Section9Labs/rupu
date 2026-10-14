// HostSelect — a small dropdown of hosts that emits the chosen host_id, for
// the launcher sheets (LauncherSheet, AgentLauncherSheet): api.getHosts(),
// whose status it shows, falling back to a single "Local" option while the
// fetch is pending or when it fails. The list pages pick hosts with the
// query line's `host:` terms instead.
//
// Restyled internally onto `ui/Select`'s shared chrome (visual parity — same
// classes, now sourced from one place) per the One Control Language kit.

import { useEffect, useState } from 'react';
import { api, type HostView } from '../lib/api';
import { Select } from './ui/Select';

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
