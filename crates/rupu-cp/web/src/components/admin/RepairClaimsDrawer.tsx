// RepairClaimsDrawer — Activity → Autoflows' "Repair claims" (Plan 3b mock
// §9): dry run first, then release. The claims to repair are the ones
// `autoflow doctor` flags (`GET /api/autoflows/doctor` rows whose scope is a
// held claim's issue ref); each is dry-run through `POST
// /api/autoflows/claims/repair {release: true, dry_run: true}`, which lists
// what it would do. The danger "Release N claims" then sends the same calls
// with `dry_run: false`. A claim reads as released only once the next claims
// read no longer holds it — never from the POST alone.

import { useEffect, useState } from 'react';
import { api, apiErrorMessage, type AutoflowClaim } from '../../lib/api';
import { admin, type AutoflowDoctorRow } from '../../lib/api3b/admin';
import { shortIssueRef } from '../../lib/issueRef';
import { Button } from '../ui/Button';
import { Skeleton } from '../ui/Skeleton';
import { StatusChip } from '../ui/StatusChip';
import { Drawer } from './Drawer';
import { problemLabel } from './AutoflowDoctorDrawer';

interface Candidate {
  claim: AutoflowClaim;
  problems: AutoflowDoctorRow[];
  /** The dry run's `would …` lines, once it answered. */
  repairs?: string[];
  /** Why the dry run (or the release) failed. */
  error?: string;
  /** After a release: confirmed gone from the claims, or still held. */
  outcome?: 'released' | 'still_held';
}

type Phase = 'loading' | 'preview' | 'releasing' | 'done';

const claims = (n: number) => `${n} claim${n === 1 ? '' : 's'}`;

async function loadCandidates(): Promise<Candidate[]> {
  const [doctor, held] = await Promise.all([admin.getAutoflowDoctor(), api.getAutoflowClaims()]);
  const byRef = new Map(held.map((c) => [c.issue_ref, c]));
  const found = new Map<string, Candidate>();
  for (const row of doctor.rows) {
    const claim = byRef.get(row.scope);
    if (!claim) continue;
    const c = found.get(row.scope) ?? { claim, problems: [] };
    c.problems.push(row);
    found.set(row.scope, c);
  }
  const list = [...found.values()];
  const runs = await Promise.allSettled(
    list.map((c) => admin.repairClaim({ issue_ref: c.claim.issue_ref, release: true, dry_run: true })),
  );
  return list.map((c, i) => {
    const r = runs[i];
    return r.status === 'fulfilled' ? { ...c, repairs: r.value.repairs } : { ...c, error: apiErrorMessage(r.reason) };
  });
}

export function RepairClaimsDrawer({ onClose, onChanged }: { onClose: () => void; onChanged: () => void }) {
  const [phase, setPhase] = useState<Phase>('loading');
  const [rows, setRows] = useState<Candidate[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    loadCandidates()
      .then((c) => {
        if (!live) return;
        setRows(c);
        setPhase('preview');
      })
      .catch((e: unknown) => {
        if (!live) return;
        setError(apiErrorMessage(e));
        setPhase('preview');
      });
    return () => {
      live = false;
    };
  }, []);

  const releasable = rows.filter((r) => !r.error);

  async function release() {
    setPhase('releasing');
    const sent = await Promise.allSettled(
      releasable.map((r) => admin.repairClaim({ issue_ref: r.claim.issue_ref, release: true, dry_run: false })),
    );
    const failed = new Map<string, string>();
    sent.forEach((s, i) => {
      if (s.status === 'rejected') failed.set(releasable[i].claim.issue_ref, apiErrorMessage(s.reason));
    });
    let stillHeld: Set<string> | null = null;
    try {
      stillHeld = new Set((await api.getAutoflowClaims()).map((c) => c.issue_ref));
    } catch (e) {
      setError(`Released, but the claims could not be read back: ${apiErrorMessage(e)}`);
    }
    setRows((prev) =>
      prev.map((r) => {
        const ref = r.claim.issue_ref;
        if (failed.has(ref)) return { ...r, error: failed.get(ref) };
        if (r.error || !stillHeld) return r;
        return { ...r, outcome: stillHeld.has(ref) ? 'still_held' : 'released' };
      }),
    );
    setPhase('done');
    onChanged();
  }

  const released = rows.filter((r) => r.outcome === 'released').length;
  const subtitle =
    phase === 'loading'
      ? 'Dry run: checking claims…'
      : phase === 'releasing'
        ? `Releasing ${claims(releasable.length)}…`
        : phase === 'done'
          ? `Released ${claims(released)}`
          : releasable.length === 0
            ? 'Dry run: no claim needs repair'
            : `Dry run: ${claims(releasable.length)} would be released`;

  return (
    <Drawer
      title="Repair claims"
      subtitle={subtitle}
      onClose={onClose}
      footer={
        <>
          <Button variant="secondary" onClick={onClose}>
            Close
          </Button>
          {(phase === 'preview' || phase === 'releasing') && releasable.length > 0 && (
            <Button variant="danger" onClick={() => void release()} disabled={phase === 'releasing'}>
              {phase === 'releasing' ? 'Releasing…' : `Release ${claims(releasable.length)}`}
            </Button>
          )}
        </>
      }
    >
      {error && (
        <p role="alert" className="mb-3 text-note text-err">
          {error}
        </p>
      )}
      {phase === 'loading' ? (
        <div role="status" aria-label="Checking claims…" className="space-y-2">
          <Skeleton className="h-4 w-2/3" />
          <Skeleton className="h-4 w-1/2" />
        </div>
      ) : rows.length === 0 ? (
        <p className="text-sm text-ink-dim">
          No held claim has a problem the autoflow doctor reports, so nothing would be released.
        </p>
      ) : (
        <ul className="divide-y divide-border rounded-lg border border-border">
          {rows.map((r) => (
            <li key={r.claim.issue_ref} className="px-3 py-2 text-note">
              <div className="flex flex-wrap items-center gap-2">
                {r.problems.map((p, i) => (
                  <StatusChip key={i} tone="awaiting_approval">
                    {problemLabel(p.problem)}
                  </StatusChip>
                ))}
                <span className="font-mono text-ink" title={r.claim.issue_ref}>
                  {r.claim.issue_display_ref ?? shortIssueRef(r.claim.issue_ref)}
                </span>
                <span className="flex-1" />
                {r.outcome === 'released' && <StatusChip tone="completed">released</StatusChip>}
                {r.outcome === 'still_held' && <StatusChip tone="pending">still held</StatusChip>}
              </div>
              <div className="mt-1 text-ink-dim">
                {r.claim.workflow} · {r.problems.map((p) => p.detail).join('; ')}
              </div>
              {r.repairs && r.repairs.length > 0 && !r.outcome && (
                <ul className="mt-1 space-y-0.5 font-mono text-ink-mute">
                  {r.repairs.map((line) => (
                    <li key={line}>{line}</li>
                  ))}
                </ul>
              )}
              {r.error && <p className="mt-1 text-err">{r.error}</p>}
            </li>
          ))}
        </ul>
      )}
    </Drawer>
  );
}

export default RepairClaimsDrawer;
