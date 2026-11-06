// QueryHelp — the "?" at the end of a query line (Plan 3b mock §8). It opens
// the list's fields: key, kind, values and one example each. A list the
// server filters reads them from `GET /api/query/:list/fields`, the registry
// the server filters with; a list the browser filters shows its own.

import { useEffect, useId, useRef, useState } from 'react';
import type { QueryField } from '../../lib/findingQuery/fields';
import { apiErrorMessage } from '../../lib/api';
import { getQueryFields, type ServerQueryField } from '../../lib/api3b/library';
import { helpRows, serverListOf } from '../../lib/query/help';
import { Button } from '../ui/Button';
import { Spinner } from '../ui/Spinner';

/** "Filter findings" → "Findings". */
function nounOf(label: string): string {
  const noun = label.replace(/^filter\s+/i, '').trim() || 'Query';
  return noun.charAt(0).toUpperCase() + noun.slice(1);
}

export function QueryHelp({ fields, label }: { fields: readonly QueryField[]; label: string }) {
  const [open, setOpen] = useState(false);
  const [server, setServer] = useState<ServerQueryField[] | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const list = serverListOf(fields);
  const loading = open && list !== null && server === null && failed === null;

  useEffect(() => {
    if (!open || list === null || server !== null) return;
    let cancelled = false;
    setFailed(null);
    getQueryFields(list).then(
      (f) => !cancelled && setServer(f),
      (e: unknown) => !cancelled && setFailed(apiErrorMessage(e)),
    );
    return () => {
      cancelled = true;
    };
  }, [open, list, server]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      setOpen(false);
      // Button doesn't forward a ref (React 18): find it in the root.
      rootRef.current?.querySelector('button')?.focus();
    };
    const onDown = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('keydown', onKey);
    document.addEventListener('mousedown', onDown);
    return () => {
      document.removeEventListener('keydown', onKey);
      document.removeEventListener('mousedown', onDown);
    };
  }, [open]);

  const rows = helpRows(fields, list === null || failed !== null ? null : server);
  const title = `${nounOf(label)} query fields`;

  return (
    <div ref={rootRef} className="relative shrink-0">
      <Button
        variant="secondary"
        aria-label="Query fields"
        title="Query fields"
        aria-expanded={open}
        aria-haspopup="dialog"
        className="h-[34px] px-3"
        onClick={() => setOpen((o) => !o)}
      >
        ?
      </Button>
      {open && (
        <div
          role="dialog"
          aria-labelledby={titleId}
          className="absolute right-0 top-full z-40 mt-2 max-h-[70vh] w-[min(26rem,calc(100vw-2rem))] overflow-auto rounded-xl border border-border bg-panel p-4 text-left shadow-card"
        >
          <h2 id={titleId} className="mb-2 text-ui font-semibold text-ink">
            {title}
          </h2>
          {failed && (
            <p role="status" className="mb-2 text-note text-warn">
              This list's own fields are shown: couldn't read the server's fields ({failed}).
            </p>
          )}
          {loading ? (
            <Spinner size="sm" label="Loading fields…" />
          ) : (
            <dl className="divide-y divide-border text-note">
              {rows.map((r) => (
                <div key={r.key} className="grid grid-cols-[6.5rem_minmax(0,1fr)] gap-3 py-1.5">
                  <dt className="font-mono text-ink">
                    {r.key}
                    {r.aliases.length > 0 && <span className="text-ink-mute"> {r.aliases.join(' ')}</span>}
                  </dt>
                  <dd className="min-w-0 break-words text-ink-dim">
                    {r.kind}
                    {r.values.length > 0 && <> · {r.values.join(' ')}</>}
                    {' · '}
                    <span className="font-mono text-ink-mute">{r.example}</span>
                  </dd>
                </div>
              ))}
            </dl>
          )}
          <p className="mt-2 text-note text-ink-mute">
            Bare words search the text. <span className="font-mono">-</span> negates. Values OR, terms AND.
          </p>
        </div>
      )}
    </div>
  );
}

export default QueryHelp;
