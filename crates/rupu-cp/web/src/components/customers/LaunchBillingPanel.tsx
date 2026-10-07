import { useEffect, useMemo, useRef, useState } from 'react';
import { AlertTriangle } from 'lucide-react';
import {
  api,
  ApiError,
  type CustomerRef,
  type ManifestEntry,
  type PreviewBody,
  type PreviewResponse,
} from '../../lib/api';
import { CustomerDot } from './CustomerDot';

const DEBOUNCE_MS = 250;

/** What the launcher needs back from the panel: the customer to show on its
 *  project field (`undefined` until a preview lands) and whether the launch
 *  would fail the same way the preview did (a 409: dangling assignment,
 *  unloadable config, malformed agent files). */
export interface BillingState {
  customer: CustomerRef | null | undefined;
  blocked: boolean;
}

type Load =
  | { kind: 'idle' }
  | { kind: 'loading' }
  | { kind: 'ready'; data: PreviewResponse }
  | { kind: 'error'; message: string; blocked: boolean };

const ROLES: { role: ManifestEntry['role']; label: string }[] = [
  { role: 'provider', label: 'Provider' },
  { role: 'fallback', label: 'Fallbacks' },
  { role: 'scm', label: 'SCM' },
];

/**
 * "This run uses <customer>'s accounts" — the launcher's preview of which
 * customer and accounts a launch would authenticate as
 * (`POST /api/launch/preview`). Re-requested (debounced) whenever the body
 * changes; an in-flight request is aborted and a stale answer is ignored.
 * Renders nothing without a body.
 */
export function LaunchBillingPanel({
  body,
  onResult,
}: {
  body: PreviewBody | null;
  onResult?: (state: BillingState) => void;
}) {
  const [load, setLoad] = useState<Load>({ kind: 'idle' });
  const onResultRef = useRef(onResult);
  onResultRef.current = onResult;

  // A stable key so an equal body built fresh each render doesn't refetch.
  const key = useMemo(() => (body ? JSON.stringify(body) : null), [body]);

  useEffect(() => {
    if (key === null) {
      setLoad({ kind: 'idle' });
      onResultRef.current?.({ customer: undefined, blocked: false });
      return;
    }
    const ctrl = new AbortController();
    setLoad({ kind: 'loading' });
    // Nothing is known about this body yet: don't keep blocking on the last one.
    onResultRef.current?.({ customer: undefined, blocked: false });
    const timer = setTimeout(() => {
      api
        .launchPreview(JSON.parse(key) as PreviewBody, ctrl.signal)
        .then((data) => {
          if (ctrl.signal.aborted) return;
          setLoad({ kind: 'ready', data });
          onResultRef.current?.({ customer: data.customer, blocked: false });
        })
        .catch((e: unknown) => {
          if (ctrl.signal.aborted) return;
          const blocked = e instanceof ApiError && e.status === 409;
          const message = e instanceof Error ? e.message : 'Could not preview the accounts';
          setLoad({ kind: 'error', message, blocked });
          onResultRef.current?.({ customer: undefined, blocked });
        });
    }, DEBOUNCE_MS);
    return () => {
      clearTimeout(timer);
      ctrl.abort();
    };
  }, [key]);

  if (load.kind === 'idle') return null;

  if (load.kind === 'loading') {
    return (
      <p className="rounded-lg border border-border bg-surface px-3 py-2 text-ui text-ink-mute">
        Checking which accounts this run uses…
      </p>
    );
  }

  if (load.kind === 'error') {
    return (
      <p role="alert" className="text-ui font-medium text-err">
        {load.message}
      </p>
    );
  }

  const { customer, accounts, warnings, host } = load.data;
  return (
    <section
      aria-label="Accounts this run uses"
      className="rounded-lg border border-brand-100 bg-brand-50 px-3 py-2.5"
    >
      <h3 className="flex items-center gap-1.5 text-ui font-semibold text-brand-700">
        {customer && <CustomerDot tint={customer.tint} />}
        <span>
          This run uses {customer ? `${customer.name}'s accounts` : 'the global accounts'}
        </span>
      </h3>
      {host && (
        <p className="mt-1 text-meta text-ink-dim">
          Host <span className="font-mono">{host}</span>
        </p>
      )}
      <dl className="mt-2 space-y-1">
        {ROLES.map(({ role, label }) => {
          const rows = accounts.filter((a) => a.role === role);
          if (rows.length === 0) return null;
          return (
            <div key={role} className="flex items-start gap-3">
              <dt className="w-16 shrink-0 text-meta font-semibold uppercase tracking-wide text-ink-dim">
                {label}
              </dt>
              <dd className="min-w-0 flex-1 space-y-0.5">
                {rows.map((a, i) => (
                  <div key={`${a.account}-${a.auth_mode ?? ''}-${i}`} className="flex flex-wrap items-baseline gap-x-2">
                    <span className="font-mono text-ui text-ink break-all">{a.account}</span>
                    {a.auth_mode && (
                      <span className="rounded border border-border px-1 text-meta text-ink-dim">
                        {a.auth_mode}
                      </span>
                    )}
                    <span className="text-meta text-ink-mute">{a.source}</span>
                  </div>
                ))}
              </dd>
            </div>
          );
        })}
      </dl>
      {warnings.length > 0 && (
        <ul className="mt-2 space-y-0.5">
          {warnings.map((w) => (
            <li key={w} className="flex items-start gap-1.5 text-meta text-warn">
              <AlertTriangle size={12} aria-hidden className="mt-0.5 shrink-0" />
              <span>{w}</span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
