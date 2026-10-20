// Settings → Maintenance (Plan 3b mock §10): one row per prune kind of `POST
// /api/maintenance/prune`. Each row dry-runs on open and whenever its
// older-than field changes, shows what it would remove, and only then offers
// "Prune N", which asks once before it deletes. The answer to the real prune
// lists what was removed; every row then dry-runs again (cleanup overlaps
// the transcript and session kinds), so each count is always the server's.

import { useCallback, useEffect, useRef, useState } from 'react';
import { ApiError, apiErrorMessage } from '../../lib/api';
import { admin, pruneSummary, type PruneKind, type PruneRow } from '../../lib/api3b/admin';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';

const DEBOUNCE_MS = 400;

interface KindSpec {
  kind: PruneKind;
  label: string;
  /** What the confirm names: `archived transcripts`. */
  noun: [string, string];
  verb: string;
  /** False for cleanup, which prunes at the configured retentions. */
  olderThan: boolean;
}

const KINDS: KindSpec[] = [
  { kind: 'transcripts', label: 'Archived transcripts', noun: ['archived transcript', 'archived transcripts'], verb: 'Prune', olderThan: true },
  { kind: 'sessions', label: 'Archived sessions', noun: ['archived session', 'archived sessions'], verb: 'Prune', olderThan: true },
  { kind: 'netflow', label: 'Netflow ledgers', noun: ['netflow ledger', 'netflow ledgers'], verb: 'Prune', olderThan: true },
  { kind: 'cleanup', label: 'Cleanup (archived sessions and transcripts)', noun: ['item', 'items'], verb: 'Clean', olderThan: false },
];

type Preview = { rows: PruneRow[] } | { error: string; unavailable: boolean } | null;

function PruneRowView({
  spec,
  initialOlderThan,
  generation,
  onPruned,
}: {
  spec: KindSpec;
  initialOlderThan: string;
  /** Bumped after any row prunes: every row counts again. */
  generation: number;
  onPruned: () => void;
}) {
  const [olderThan, setOlderThan] = useState(initialOlderThan);
  const [preview, setPreview] = useState<Preview>(null);
  const [loading, setLoading] = useState(true);
  const [confirming, setConfirming] = useState(false);
  const [pruning, setPruning] = useState(false);
  const [result, setResult] = useState<{ removed: PruneRow[]; errors: string[] } | null>(null);
  const [pruneError, setPruneError] = useState<string | null>(null);
  const seq = useRef(0);

  const body = useCallback(
    (dry: boolean) => ({
      kind: spec.kind,
      ...(spec.olderThan && olderThan.trim() ? { older_than: olderThan.trim() } : {}),
      dry_run: dry,
    }),
    [spec, olderThan],
  );

  const dryRun = useCallback(async () => {
    const mine = ++seq.current;
    setLoading(true);
    try {
      const reply = await admin.prune(body(true));
      if (mine === seq.current) setPreview({ rows: reply.would_remove ?? [] });
    } catch (e) {
      if (mine === seq.current) {
        setPreview({ error: apiErrorMessage(e), unavailable: e instanceof ApiError && e.status === 501 });
      }
    } finally {
      if (mine === seq.current) setLoading(false);
    }
  }, [body]);

  // Dry-run on open, and again (debounced) when the field changes.
  const first = useRef(true);
  useEffect(() => {
    if (first.current) {
      first.current = false;
      void dryRun();
      return;
    }
    const t = setTimeout(() => void dryRun(), DEBOUNCE_MS);
    return () => clearTimeout(t);
  }, [dryRun]);

  // Recount after a prune, this row's or another's.
  const seenGeneration = useRef(generation);
  useEffect(() => {
    if (seenGeneration.current === generation) return;
    seenGeneration.current = generation;
    void dryRun();
  }, [generation, dryRun]);

  async function prune() {
    setConfirming(false);
    setPruning(true);
    setPruneError(null);
    try {
      const reply = await admin.prune(body(false));
      setResult({ removed: reply.removed ?? [], errors: reply.errors });
    } catch (e) {
      setPruneError(apiErrorMessage(e));
    } finally {
      setPruning(false);
      onPruned();
    }
  }

  const rows = preview && 'rows' in preview ? preview.rows : null;
  const count = rows?.length ?? 0;
  const [one, many] = spec.noun;

  return (
    <>
      <tr aria-label={spec.label}>
        <td className="px-3 py-2.5 text-ink">{spec.label}</td>
        <td className="px-3 py-2.5">
          {spec.olderThan ? (
            <Input
              aria-label={`${spec.label} older than`}
              value={olderThan}
              onChange={(e) => {
                setResult(null);
                setOlderThan(e.target.value);
              }}
              placeholder="30d"
              className="w-20 px-2 py-0.5 font-mono text-note"
              spellCheck={false}
            />
          ) : (
            <span className="text-ink-mute">—</span>
          )}
        </td>
        <td className="px-3 py-2.5">
          {preview && 'error' in preview ? (
            <span className={preview.unavailable ? 'text-ink-mute' : 'text-err'}>{preview.error}</span>
          ) : (
            <span className="tabular-nums text-ink-dim">
              {rows ? pruneSummary(spec.kind, rows) : 'counting…'}
              {loading && rows && <span className="ml-1 text-ink-mute">· recounting…</span>}
            </span>
          )}
        </td>
        <td className="px-3 py-2.5 text-right">
          <Button
            variant="ring-danger"
            disabled={loading || pruning || count === 0}
            onClick={() => setConfirming(true)}
          >
            {pruning ? 'Pruning…' : count > 0 ? `${spec.verb} ${count}` : spec.verb}
          </Button>
        </td>
      </tr>
      {(confirming || result || pruneError) && (
        <tr>
          <td colSpan={4} className="bg-surface/60 px-3 py-2">
            {confirming ? (
              <div className="flex flex-wrap items-center gap-2">
                <span className="text-ink">
                  Delete {count} {count === 1 ? one : many}? This can't be undone.
                </span>
                <span className="flex-1" />
                <Button variant="secondary" size="sm" onClick={() => setConfirming(false)}>
                  Cancel
                </Button>
                <Button variant="danger" size="sm" onClick={() => void prune()}>
                  Delete
                </Button>
              </div>
            ) : pruneError ? (
              <p role="alert" className="text-err">
                {pruneError}
              </p>
            ) : result ? (
              <div>
                <span className="text-ok">Removed {pruneSummary(spec.kind, result.removed)}</span>
                {result.errors.length > 0 && (
                  <ul className="mt-1 space-y-0.5 text-err">
                    {result.errors.map((e) => (
                      <li key={e}>{e}</li>
                    ))}
                  </ul>
                )}
              </div>
            ) : null}
          </td>
        </tr>
      )}
    </>
  );
}

export function MaintenanceTab({ retention }: { retention: { transcripts?: string; sessions?: string } }) {
  const [generation, setGeneration] = useState(0);
  const initial: Record<PruneKind, string> = {
    transcripts: retention.transcripts ?? '30d',
    sessions: retention.sessions ?? '30d',
    netflow: '30d',
    cleanup: '',
  };
  return (
    <div className="space-y-3">
      <p className="text-sm text-ink-dim">
        Each kind shows what it would remove first. Older-than takes <span className="font-mono">30d</span>,{' '}
        <span className="font-mono">12h</span> or <span className="font-mono">1w</span>; it starts at the configured
        retention.
      </p>
      <table className="w-full text-note">
        <thead className="bg-surface">
          <tr>
            {['What', 'Older than', 'Would remove', ''].map((h, i) => (
              <th key={i} className="px-3 py-2 text-left text-meta font-semibold uppercase tracking-wide text-ink-mute">
                {h}
              </th>
            ))}
          </tr>
        </thead>
        <tbody className="divide-y divide-border">
          {KINDS.map((spec) => (
            <PruneRowView
              key={spec.kind}
              spec={spec}
              initialOlderThan={initial[spec.kind]}
              generation={generation}
              onPruned={() => setGeneration((g) => g + 1)}
            />
          ))}
        </tbody>
      </table>
    </div>
  );
}

export default MaintenanceTab;
