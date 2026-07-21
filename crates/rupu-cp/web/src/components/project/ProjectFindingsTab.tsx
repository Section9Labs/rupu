// Project Findings tab body — findings scoped to one project (`wsId`), with an
// interactive severity filter strip. Mirrors pages/Findings.tsx structure,
// scoped to a single workspace via `getFindings({ wsId })`. The backend
// pre-sorts findings by severity (critical → info, newest first).

import { useEffect, useMemo, useState } from 'react';
import { api, normFindingSeverity, type FindingsResponse } from '../../lib/api';
import { type Severity } from '../../lib/severity';
import { FindingMetrics } from '../findings/FindingMetrics';
import { FindingsTable } from '../findings/FindingsTable';

export default function ProjectFindingsTab({ wsId }: { wsId: string }) {
  const [resp, setResp] = useState<FindingsResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [activeSev, setActiveSev] = useState<Severity | null>(null);

  useEffect(() => {
    if (!wsId) return;
    let cancelled = false;
    setResp(null);
    setError(null);
    setActiveSev(null);
    api
      .getFindings({ wsId })
      .then((data) => {
        if (cancelled) return;
        setResp(data);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : 'Failed to load findings');
      });
    return () => {
      cancelled = true;
    };
  }, [wsId]);

  const all = resp?.findings ?? [];

  // Filter to the active severity tile (backend order is preserved). Total /
  // re-clicking the active tile clears the filter (activeSev === null).
  const rows = useMemo(
    () => (activeSev ? all.filter((f) => normFindingSeverity(f.severity) === activeSev) : all),
    [all, activeSev],
  );

  if (error) {
    return (
      <div className="rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">
        {error}
      </div>
    );
  }

  if (resp === null) {
    return <div className="text-sm text-ink-dim">Loading findings…</div>;
  }

  if (all.length === 0) {
    return (
      <div className="rounded-xl border border-dashed border-border bg-panel/50 px-4 py-8 text-center text-sm text-ink-mute">
        No findings for this project.
      </div>
    );
  }

  return (
    <div className="space-y-4">
      <FindingMetrics summary={resp.summary} active={activeSev} onSelect={setActiveSev} />

      {rows.length === 0 ? (
        <div className="rounded-xl border border-dashed border-border bg-panel/50 py-10 text-center text-sm text-ink-dim">
          No {activeSev} findings.
        </div>
      ) : (
        <FindingsTable findings={rows} showProvenance wsId={wsId} />
      )}
    </div>
  );
}
