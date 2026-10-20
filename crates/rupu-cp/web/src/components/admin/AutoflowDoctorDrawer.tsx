// AutoflowDoctorDrawer — `rupu autoflow doctor` in Activity → Autoflows:
// consistency problems across claims, wakes and runs (`GET
// /api/autoflows/doctor`). Read-only; Repair claims is where a problem is
// acted on.

import { useEffect, useState } from 'react';
import { RefreshCw } from 'lucide-react';
import { apiErrorMessage } from '../../lib/api';
import { admin, type AutoflowDoctorReport } from '../../lib/api3b/admin';
import { shortIssueRef } from '../../lib/issueRef';
import { cn } from '../../lib/cn';
import { Button } from '../ui/Button';
import { Skeleton } from '../ui/Skeleton';
import { StatusChip } from '../ui/StatusChip';
import { Drawer } from './Drawer';

export const problemLabel = (problem: string) => problem.replace(/_/g, ' ');

export function AutoflowDoctorDrawer({ onClose }: { onClose: () => void }) {
  const [report, setReport] = useState<AutoflowDoctorReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [tick, setTick] = useState(0);

  useEffect(() => {
    let live = true;
    setLoading(true);
    admin
      .getAutoflowDoctor()
      .then((r) => {
        if (!live) return;
        setReport(r);
        setError(null);
      })
      .catch((e: unknown) => live && setError(apiErrorMessage(e)))
      .finally(() => live && setLoading(false));
    return () => {
      live = false;
    };
  }, [tick]);

  const rows = report?.rows ?? [];
  return (
    <Drawer
      title="Autoflow doctor"
      subtitle={
        report && !loading
          ? report.ok
            ? 'Claims, wakes and runs agree.'
            : `${rows.length} problem${rows.length === 1 ? '' : 's'} across claims, wakes and runs`
          : 'Checking claims, wakes and runs…'
      }
      onClose={onClose}
      footer={
        <Button variant="secondary" onClick={() => setTick((t) => t + 1)} className="gap-1.5">
          <RefreshCw size={12} className={cn(loading && 'animate-spin')} />
          Check again
        </Button>
      }
    >
      {error && (
        <p role="alert" className="text-note text-err">
          {error}
        </p>
      )}
      {loading && !report ? (
        <div role="status" aria-label="Checking…" className="space-y-2">
          <Skeleton className="h-4 w-2/3" />
          <Skeleton className="h-4 w-1/2" />
        </div>
      ) : report && rows.length === 0 ? (
        <p className="text-sm text-ink-dim">No problems found.</p>
      ) : (
        <ul className="divide-y divide-border rounded-lg border border-border">
          {rows.map((r, i) => (
            <li key={`${r.scope}-${r.problem}-${i}`} className="px-3 py-2 text-note">
              <div className="flex items-center gap-2">
                <StatusChip tone="awaiting_approval">{problemLabel(r.problem)}</StatusChip>
                <span className="font-mono text-ink" title={r.scope}>
                  {shortIssueRef(r.scope)}
                </span>
              </div>
              <div className="mt-1 text-ink-dim">{r.detail}</div>
            </li>
          ))}
        </ul>
      )}
    </Drawer>
  );
}

export default AutoflowDoctorDrawer;
