// Autoflow panel — renders on RunDetail when the run was launched by the
// autoflow worker (GET /api/runs/:id/autoflow returned a context, not null).
// Surfaces the entity the run drove, the claim's lease/status (with Requeue /
// Release for a local run's claim), which cycle launched it, and which
// project/host it ran on. Mirrors the "chrome" panel styling RunDetail
// already uses for its persistent sections
// (bg-panel/border-border/rounded-xl/shadow-card).
//
// The full cycle history (this cycle + prior cycles for the same entity)
// lives in the Cycles TAB (components/run/CyclesTab.tsx) as a linked table,
// not here — this panel stays a compact entity/claim/scope summary.

import { useState } from 'react';
import { Link } from 'react-router-dom';
import { api, type AutoflowClaim, type AutoflowRunContext } from '../lib/api';
import { ScopeChip } from './ScopeChip';
import { Button } from './ui/Button';
import { ErrorBanner } from './ui/ErrorBanner';
import { cn } from '../lib/cn';
import { relativeTime } from '../lib/time';
import { shortId } from '../lib/shortId';

// The claim status palette (formerly shared with the Claims table on
// Runs → Autoflows, which this panel's actions replaced).
const CLAIM_STATUS_CLS: Record<string, string> = {
  await_human: 'bg-warn-bg text-warn ring-warn/30',
  running: 'bg-status-running/10 text-status-running ring-status-running/30',
  blocked: 'bg-err-bg text-err ring-err/30',
  complete: 'bg-ok-bg text-ok ring-ok/30',
  released: 'bg-surface text-ink ring-border',
};

function titleCase(s: string): string {
  return s
    .split('_')
    .filter(Boolean)
    .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
    .join(' ');
}

function ClaimStatusBadge({ status }: { status: string }) {
  const cls = CLAIM_STATUS_CLS[status] ?? 'bg-surface text-ink ring-border';
  return (
    <span
      className={cn(
        'inline-flex items-center rounded ring-1 text-meta font-medium uppercase tracking-wide px-1.5 py-0.5',
        cls,
      )}
    >
      {titleCase(status)}
    </span>
  );
}

function EntityLink({ context, claim }: { context: AutoflowRunContext; claim: AutoflowClaim | null }) {
  const label = claim?.issue_display_ref ?? context.entity ?? context.issue_ref;
  if (!label) return <span className="text-ink-mute">—</span>;
  if (claim?.issue_url) {
    return (
      <div className="min-w-0">
        <a
          href={claim.issue_url}
          target="_blank"
          rel="noreferrer"
          className="text-sm font-medium text-brand-600 hover:underline truncate"
        >
          {label}
        </a>
        {claim.issue_title && (
          <div className="mt-0.5 truncate text-note text-ink-dim">{claim.issue_title}</div>
        )}
      </div>
    );
  }
  return (
    <div className="min-w-0">
      <span className="text-sm font-medium text-ink truncate">{label}</span>
      {claim?.issue_title && (
        <div className="mt-0.5 truncate text-note text-ink-dim">{claim.issue_title}</div>
      )}
    </div>
  );
}

/** Last path segment of a workspace path, for a compact project chip. */
function projectLabel(workspacePath: string): string {
  const parts = workspacePath.split('/').filter(Boolean);
  return parts.length > 0 ? parts[parts.length - 1] : workspacePath;
}

type ClaimAction = 'requeue' | 'release';

// Requeue / Release for the run's claim. The claim store is local-only (the
// claim endpoints take no `host`), so RunDetail only mounts this for a local
// run. Confirmation wording is the one the Claims table used.
function ClaimActions({ claim }: { claim: AutoflowClaim }) {
  const [busy, setBusy] = useState<ClaimAction | null>(null);
  const [done, setDone] = useState<ClaimAction | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function run(action: ClaimAction) {
    if (busy) return;
    const prompt = action === 'requeue' ? 'Requeue this autoflow?' : 'Release this claim?';
    if (!window.confirm(prompt)) return;
    setBusy(action);
    setError(null);
    try {
      if (action === 'requeue') await api.requeueClaim(claim.issue_ref);
      else await api.releaseClaim(claim.issue_ref);
      setDone(action);
    } catch (e) {
      const fallback = action === 'requeue' ? 'Failed to requeue claim' : 'Failed to release claim';
      setError(e instanceof Error ? e.message : fallback);
    } finally {
      setBusy(null);
    }
  }

  // A released claim is gone: nothing left to requeue or release.
  if (done === 'release') {
    return <span className="text-note font-medium text-ok">Released</span>;
  }

  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-center gap-2">
        <Button variant="ring" onClick={() => void run('requeue')} disabled={busy !== null}>
          {busy === 'requeue' ? 'Requeuing…' : 'Requeue'}
        </Button>
        <Button variant="ring-danger" onClick={() => void run('release')} disabled={busy !== null}>
          {busy === 'release' ? 'Releasing…' : 'Release'}
        </Button>
        {done === 'requeue' && <span className="text-note font-medium text-ok">Requeued</span>}
      </div>
      {error && <ErrorBanner className="px-2 py-1 text-note">{error}</ErrorBanner>}
    </div>
  );
}

export default function AutoflowPanel({
  context,
  host,
}: {
  context: AutoflowRunContext;
  /** The `?host=` the run page is viewing; claim actions are local-only. */
  host?: string | null;
}) {
  const claim = context.claim;
  const local = !host || host === 'local';

  return (
    <section
      className="mt-3 bg-panel border border-border rounded-xl shadow-card px-4 py-3"
      data-testid="autoflow-panel"
    >
      <h2 className="mb-2 text-xs font-semibold uppercase tracking-wide text-ink-dim">Autoflow</h2>

      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <div>
          <div className="text-meta uppercase tracking-wide text-ink-mute">Entity</div>
          <div className="mt-1">
            <EntityLink context={context} claim={claim} />
          </div>
        </div>

        <div>
          <div className="text-meta uppercase tracking-wide text-ink-mute">Claim</div>
          <div className="mt-1 flex items-center gap-2">
            {claim ? (
              <>
                <ClaimStatusBadge status={claim.status} />
                {claim.lease_expires_at && (
                  <span className="text-note text-ink-mute">
                    lease expires {relativeTime(claim.lease_expires_at)}
                  </span>
                )}
              </>
            ) : (
              <span className="text-note text-ink-mute">no active claim on record</span>
            )}
          </div>
          {claim && (
            <div className="mt-2">
              {local ? (
                <ClaimActions claim={claim} />
              ) : (
                <span className="text-note text-ink-mute">
                  Claim actions are available on the host that ran this autoflow.
                </span>
              )}
            </div>
          )}
        </div>

        <div>
          <div className="text-meta uppercase tracking-wide text-ink-mute">Cycle</div>
          <div className="mt-1 flex items-center gap-2">
            <span className="font-mono text-note text-ink-dim">{shortId(context.cycle_id, 12)}</span>
            <Link to="/runs/autoflows" className="text-note text-brand-600 hover:underline">
              All autoflow runs
            </Link>
          </div>
        </div>

        <div>
          <div className="text-meta uppercase tracking-wide text-ink-mute">Ran on</div>
          <div className="mt-1 flex items-center gap-1.5">
            <ScopeChip scope={context.host_id ?? 'local'} />
            {context.workspace_path && (
              <ScopeChip scope={projectLabel(context.workspace_path)} />
            )}
          </div>
        </div>
      </div>
    </section>
  );
}
