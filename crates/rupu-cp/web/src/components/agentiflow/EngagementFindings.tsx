// EngagementFindings — the agentiflow detail's Findings tab. The engagement's
// verified findings, scoped by `GET /api/findings?run_id=<id>` (the server
// unions the lead run with every dispatched sub-run), rendered with the shared
// FindingsTable so rows, severity order, codenames and deep-links match the
// global Findings page. Polls while the run is live.

import { useCallback, useEffect, useRef, useState } from 'react';
import { RefreshCw } from 'lucide-react';
import { api, apiErrorMessage, type FindingsResponse } from '../../lib/api';
import { FindingsTable } from '../findings/FindingsTable';
import { Button } from '../ui/Button';
import { EmptyState } from '../ui/EmptyState';
import { ErrorBanner } from '../ui/ErrorBanner';
import { Spinner } from '../ui/Spinner';
import { SEVERITY_STYLE, type Severity } from '../../lib/severity';
import { cn } from '../../lib/cn';

const POLL_MS = 5000;
const SEVS: Severity[] = ['critical', 'high', 'medium', 'low', 'info'];

export default function EngagementFindings({ runId, live }: { runId: string; live?: boolean }) {
  const [resp, setResp] = useState<FindingsResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const seq = useRef(0);

  const load = useCallback(
    (silent: boolean) => {
      const mine = ++seq.current;
      if (!silent) setLoading(true);
      api
        .getFindings({ runId })
        .then((r) => {
          if (seq.current !== mine) return;
          setResp(r);
          setError(null);
        })
        .catch((e: unknown) => {
          if (seq.current !== mine) return;
          setError(apiErrorMessage(e));
        })
        .finally(() => {
          if (seq.current === mine) setLoading(false);
        });
    },
    [runId],
  );

  useEffect(() => {
    load(false);
    return () => {
      seq.current++;
    };
  }, [load]);

  useEffect(() => {
    if (!live) return;
    const t = setInterval(() => load(true), POLL_MS);
    return () => clearInterval(t);
  }, [live, load]);

  if (resp === null && loading) {
    return (
      <div className="py-10 flex items-center justify-center">
        <Spinner label="Loading findings…" />
      </div>
    );
  }
  if (error) return <ErrorBanner>{error}</ErrorBanner>;
  if (!resp || resp.findings.length === 0) {
    return (
      <EmptyState
        title="No findings recorded yet"
        hint="A finding appears here once the lead (or a reviewer/assessor) records it with the findings.report tool. The engagement's goal often counts these."
      />
    );
  }

  // The duplicate-workspace bug double-serves each finding; collapse by id (the
  // same band-aid the Assets view uses) until that registration bug is fixed.
  const findings = Array.from(new Map(resp.findings.map((f) => [f.id, f])).values());
  const counts: Record<string, number> = { critical: 0, high: 0, medium: 0, low: 0, info: 0 };
  for (const f of findings) {
    const sev = String(f.severity).toLowerCase();
    if (sev in counts) counts[sev] += 1;
  }
  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between gap-3">
        <div className="flex flex-wrap items-center gap-1.5">
          <span className="text-note tabular-nums text-ink-dim">{findings.length} findings</span>
          {SEVS.filter((s) => counts[s] > 0).map((s) => (
            <span key={s} className={cn('rounded px-1.5 py-0.5 text-meta font-medium', SEVERITY_STYLE[s].pill)}>
              {counts[s]} {s}
            </span>
          ))}
        </div>
        <Button variant="secondary" onClick={() => load(false)} className="gap-1.5">
          <RefreshCw size={12} className={cn(loading && 'animate-spin')} />
          Refresh
        </Button>
      </div>
      <FindingsTable findings={findings} />
    </div>
  );
}
