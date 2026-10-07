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
import { cn } from '../../lib/cn';
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

type Result =
  | { kind: 'ready'; data: PreviewResponse }
  | { kind: 'error'; message: string; blocked: boolean; remote: boolean };

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
  resolvedFrom = null,
}: {
  body: PreviewBody | null;
  onResult?: (state: BillingState) => void;
  /** `'cp-cwd'` when the launch has no working directory of its own (a repo
   *  target): the control plane resolves it from its own cwd, and says so. */
  resolvedFrom?: 'cp-cwd' | null;
}) {
  // The last answer stays on screen (dimmed) while the next one is pending.
  const [result, setResult] = useState<Result | null>(null);
  const [pending, setPending] = useState(false);
  const onResultRef = useRef(onResult);
  onResultRef.current = onResult;
  // What the last answer said, so a pending request neither drops the chip nor
  // lifts a block: only the next answer does.
  const lastRef = useRef<BillingState>({ customer: undefined, blocked: false });

  function report(state: BillingState) {
    lastRef.current = state;
    onResultRef.current?.(state);
  }

  // A stable key so an equal body built fresh each render doesn't refetch.
  const key = useMemo(() => (body ? JSON.stringify(body) : null), [body]);

  useEffect(() => {
    if (key === null) {
      setResult(null);
      setPending(false);
      report({ customer: undefined, blocked: false });
      return;
    }
    const ctrl = new AbortController();
    setPending(true);
    onResultRef.current?.(lastRef.current);
    const remote = !!(JSON.parse(key) as PreviewBody).host;
    const timer = setTimeout(() => {
      api
        .launchPreview(JSON.parse(key) as PreviewBody, ctrl.signal)
        .then((data) => {
          if (ctrl.signal.aborted) return;
          setResult({ kind: 'ready', data });
          setPending(false);
          report({ customer: data.customer, blocked: false });
        })
        .catch((e: unknown) => {
          if (ctrl.signal.aborted) return;
          const is409 = e instanceof ApiError && e.status === 409;
          // A remote host runs on its own config: a failure to resolve the
          // control plane's doesn't stop the launch.
          const blocked = is409 && !remote;
          const message = e instanceof Error ? e.message : 'Could not preview the accounts';
          setResult({ kind: 'error', message, blocked, remote: is409 && remote });
          setPending(false);
          report({ customer: undefined, blocked });
        });
    }, DEBOUNCE_MS);
    return () => {
      clearTimeout(timer);
      ctrl.abort();
    };
  }, [key]);

  if (result === null) {
    if (!pending) return null;
    return (
      <p
        aria-busy="true"
        className="rounded-lg border border-border bg-surface px-3 py-2 text-ui text-ink-mute"
      >
        Checking which accounts this run uses…
      </p>
    );
  }

  if (result.kind === 'error') {
    if (result.remote) {
      return (
        <p
          role="status"
          aria-busy={pending}
          className={cn('flex items-start gap-1.5 text-ui text-warn', pending && 'opacity-60')}
        >
          <AlertTriangle size={12} aria-hidden className="mt-1 shrink-0" />
          <span>
            The control plane's own config failed to resolve ({result.message}); the remote host
            uses its own config.
          </span>
        </p>
      );
    }
    return (
      <p
        role="alert"
        aria-busy={pending}
        className={cn('text-ui font-medium text-err', pending && 'opacity-60')}
      >
        {result.message}
      </p>
    );
  }

  const { customer, accounts, warnings, host } = result.data;
  return (
    <section
      aria-label="Accounts this run uses"
      aria-busy={pending}
      className={cn('rounded-lg border border-brand-100 bg-brand-50 px-3 py-2.5', pending && 'opacity-60')}
    >
      <h3 className="flex items-center gap-1.5 text-ui font-semibold text-brand-700">
        {customer && <CustomerDot tint={customer.tint} />}
        <span>
          This run uses {customer ? `${customer.name}'s accounts` : 'the global accounts'}
        </span>
      </h3>
      {resolvedFrom === 'cp-cwd' && (
        <p className="mt-1 text-meta text-ink-mute">
          Resolved from the control plane's working directory, not the repo — the run bills like a
          launch from there.
        </p>
      )}
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
          {warnings.map((w, i) => (
            <li key={`${i}-${w}`} className="flex items-start gap-1.5 text-meta text-warn">
              <AlertTriangle size={12} aria-hidden className="mt-0.5 shrink-0" />
              <span>{w}</span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
