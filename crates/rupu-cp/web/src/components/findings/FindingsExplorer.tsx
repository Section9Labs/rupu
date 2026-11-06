// FindingsExplorer — the one findings list: severity tiles, the query bar,
// import, export, bulk tagging and the table, over one scope of `GET /api/findings`
// (everything, a project `ws_id`, a run `run_id`, and/or a `customer`). The
// global Findings page, a project's Findings tab, a run's and an agentiflow's
// all mount it, so they filter, tag and export the same way.
//
// Filtering is one query (`?q=`, the findings query language) evaluated
// server-side within the scope — the explorer never filters rows itself. The
// severity tiles are shortcuts that toggle a `severity:<x>` token in that
// query; their counts come from the response's severity facets, which count
// the scope before `q`. Rows arrive severity-ordered (critical → info, then
// newest first; the backend pre-sorts).

import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { useSearchParams } from 'react-router-dom';
import {
  api,
  ApiError,
  apiErrorMessage,
  type FindingOut,
  type FindingRecord,
  type CustomerScope,
  type FindingsResponse,
  type FindingsSummary,
  type TagAcrossResult,
} from '../../lib/api';
import { SEVERITY_STYLE, type Severity } from '../../lib/severity';
import { withinSpan } from '../../lib/run/turnWindow';
import { FINDING_FIELDS } from '../../lib/findingQuery/fields';
import { parseQuery } from '../../lib/findingQuery/grammar';
import { soleValues, withTerm } from '../../lib/findingQuery/shortcut';
import type { RowSelection } from '../lists/SortableTable';
import { QueryBar } from '../query/QueryBar';
import { Button } from '../ui/Button';
import { EmptyState } from '../ui/EmptyState';
import { ErrorBanner } from '../ui/ErrorBanner';
import { Spinner } from '../ui/Spinner';
import { BulkTagBar } from './BulkTagBar';
import { ExportReportButton } from './ExportReportButton';
import { ImportFindingsDialog } from './ImportFindingsDialog';
import { FindingMetrics } from './FindingMetrics';
import { FindingsTable } from './FindingsTable';
import { summarizeTagResult } from './tags/tagResult';

const SEVS: Severity[] = ['critical', 'high', 'medium', 'low', 'info'];

/** At most this many findings per `POST /api/findings/tags` — the server's
 *  `MAX_TAG_BATCH` in `crates/rupu-cp/src/api/findings.rs`; keep them equal. */
const MAX_TAG_BATCH = 1000;

/** The selection key of a row: `ws_id/target_id/id`. */
function rowKey(f: FindingRecord): string {
  const o = f as FindingOut;
  return `${o.ws_id}/${o.target_id}/${o.id}`;
}

const plural = (n: number) => `${n} finding${n === 1 ? '' : 's'}`;

/** One answer for several batches: their workspace results in order, and
 *  every unknown id once. */
function mergeTagResults(rs: TagAcrossResult[]): TagAcrossResult {
  return {
    workspaces: rs.flatMap((r) => r.workspaces),
    unknown: [...new Set(rs.flatMap((r) => r.unknown))],
  };
}

function sevTone(key: string, value: string): string | null {
  return key === 'severity' && (SEVS as string[]).includes(value) ? SEVERITY_STYLE[value as Severity].pill : null;
}

export interface FindingsScope {
  wsId?: string;
  runId?: string;
  customer?: CustomerScope;
}

export interface FindingsExplorerProps {
  scope: FindingsScope;
  /** The global customer scope's `guard` (`useScopedList`): a 400 for the
   *  customer clears the scope. */
  guard?: <T>(p: Promise<T>) => Promise<T>;
  /** Reload silently this often (a live run). */
  pollMs?: number;
  /** The export dialog's default title, or how to make it from the rows. */
  exportTitle: string | ((rows: FindingOut[]) => string);
  /** Scopes the export to one project. */
  exportWsId?: string;
  /** The table's fallback owning workspace. */
  tableWsId?: string;
  /** What to say when the scope has no findings at all. */
  empty: { title: string; hint: ReactNode };
  /** The scope's finding count (the Total tile), once known. */
  onCount?: (n: number) => void;
  /** Only findings declared inside this span (ms since the epoch): a window
   *  the run page selected on its Token flow. */
  within?: [number, number] | null;
}

export function FindingsExplorer({ scope, guard, pollMs, exportTitle, exportWsId, tableWsId, empty, onCount, within }: FindingsExplorerProps) {
  const { wsId, runId, customer = null } = scope;
  const scopeKey = JSON.stringify([wsId ?? null, runId ?? null, customer]);
  const [params, setParams] = useSearchParams();
  const q = params.get('q') ?? '';
  // Other params (e.g. the Security tab) ride along untouched.
  const setQ = (next: string) =>
    setParams(
      (prev) => {
        const p = new URLSearchParams(prev);
        if (next.trim()) p.set('q', next);
        else p.delete('q');
        return p;
      },
      { replace: true },
    );
  const localError = useMemo(() => {
    const p = parseQuery(q, FINDING_FIELDS);
    return p.ok ? null : p.error.message;
  }, [q]);

  // Each answer and error is kept with the query and scope it answers: the
  // explorer shows rows, tiles and the export only for the query in the URL,
  // never an earlier one's while the next loads or after the server rejects it.
  const [answer, setAnswer] = useState<{ q: string; scope: string; resp: FindingsResponse } | null>(null);
  const [failure, setFailure] = useState<{ q: string; scope: string; message: string } | null>(null);
  // Facets describe the scope before `q`, so the last answer's keep serving
  // the bar's suggestions while the next query loads or after it fails.
  const [facets, setFacets] = useState<FindingsResponse['facets'] | undefined>(undefined);
  // Set once the first fetch settles, or once an invalid query has shown its
  // error: until then the explorer is one spinner. After it, the bar stays mounted.
  const [settled, setSettled] = useState(false);
  // Bulk tagging: the selected rows (keyed ws_id/target_id/id), a counter that
  // refetches the list after a change (or a poll), and the last bulk result.
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [reload, setReload] = useState(0);
  const [bulkNote, setBulkNote] = useState<{ message: string; ok: boolean } | null>(null);
  // The query and scope now shown: a bulk change that finishes after they
  // moved on doesn't put its result line on the new list.
  const currentQ = useRef(q);
  currentQ.current = q;
  const currentScope = useRef(scopeKey);
  currentScope.current = scopeKey;

  // A new query or scope is a new list: nothing carries over from the old one.
  useEffect(() => {
    setSelected(new Set());
    setBulkNote(null);
  }, [q, scopeKey]);

  useEffect(() => {
    if (!pollMs) return;
    const t = setInterval(() => setReload((n) => n + 1), pollMs);
    return () => clearInterval(t);
  }, [pollMs]);

  useEffect(() => {
    // A query that doesn't parse never reaches the server; the explorer shows
    // why in place of the results. That counts as settled, so fixing a shared
    // link's typo doesn't swap the bar for the full-page spinner.
    if (localError) {
      setSettled(true);
      return;
    }
    let cancelled = false;
    setFailure(null);
    // Called with no argument when unscoped and unfiltered, as the global page
    // always did; a customer scope rides with `q` and a 400 for it clears the
    // scope (`guard`).
    const opts = {
      ...(wsId ? { wsId } : {}),
      ...(runId ? { runId } : {}),
      ...(q ? { q } : {}),
      ...(customer ? { customer } : {}),
    };
    const req = Object.keys(opts).length === 0 ? api.getFindings() : api.getFindings(opts);
    (customer && guard ? guard(req) : req)
      .then((resp) => {
        if (cancelled) return;
        setAnswer({ q, scope: scopeKey, resp });
        setFacets(resp.facets);
        setSettled(true);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setAnswer(null);
        setFailure({ q, scope: scopeKey, message: apiErrorMessage(e) });
        setSettled(true);
      });
    return () => {
      cancelled = true;
    };
  }, [q, localError, reload, wsId, runId, customer, scopeKey, guard]);

  const answered = !localError && answer?.q === q && answer.scope === scopeKey ? answer.resp : null;
  // A window keeps the findings declared inside it, and the severity tiles
  // count those (the other facets keep describing the whole scope).
  const data = useMemo(() => {
    if (!answered || !within) return answered;
    const findings = answered.findings.filter((f) => withinSpan(within, f.declared_at));
    const severity = SEVS.map((value) => ({ value, count: findings.filter((f) => f.severity === value).length }));
    return { ...answered, findings, facets: { ...answered.facets, severity } };
  }, [answered, within]);
  const error = !localError && failure?.q === q && failure.scope === scopeKey ? failure.message : null;

  const tileSummary: FindingsSummary = useMemo(() => {
    const counts = Object.fromEntries((data?.facets?.severity ?? []).map((v) => [v.value, v.count]));
    const by = (s: Severity) => counts[s] ?? 0;
    return {
      total: SEVS.reduce((n, s) => n + by(s), 0),
      critical: by('critical'),
      high: by('high'),
      medium: by('medium'),
      low: by('low'),
      info: by('info'),
    };
  }, [data]);

  useEffect(() => {
    if (data) onCount?.(tileSummary.total);
  }, [data, tileSummary, onCount]);

  // Name the projects whose tag logs couldn't be read.
  const unreadable = useMemo(
    () => (data?.tags_unavailable ?? []).map((w) => w.project || w.ws_id),
    [data],
  );

  const unavailable = useMemo(() => new Set((data?.tags_unavailable ?? []).map((w) => w.ws_id)), [data]);
  const blockedReason = (f: FindingRecord) => {
    const o = f as FindingOut;
    return unavailable.has(o.ws_id) ? `${o.project || o.ws_id}'s tags couldn't be read, so they can't be changed` : null;
  };
  // A selected row whose workspace's tag log turned unreadable on a reload
  // leaves the selection: it can't be changed, so it is neither counted nor sent.
  useEffect(() => {
    if (!data || unavailable.size === 0) return;
    const blocked = new Set(data.findings.filter((f) => unavailable.has(f.ws_id)).map(rowKey));
    setSelected((prev) => {
      if (![...prev].some((k) => blocked.has(k))) return prev;
      return new Set([...prev].filter((k) => !blocked.has(k)));
    });
  }, [data, unavailable]);
  const selection: RowSelection<FindingRecord> = {
    isSelected: (f) => selected.has(rowKey(f)),
    blockedReason,
    label: (f) => `Select ${f.id}`,
    onToggle: (f) => {
      setBulkNote(null);
      setSelected((prev) => {
        const next = new Set(prev);
        const k = rowKey(f);
        if (next.has(k)) next.delete(k);
        else next.add(k);
        return next;
      });
    },
    onToggleAll: (rows, on) => {
      setBulkNote(null);
      setSelected((prev) => {
        const next = new Set(prev);
        for (const r of rows) {
          if (on) next.add(rowKey(r));
          else next.delete(rowKey(r));
        }
        return next;
      });
    },
  };
  const selectedRows = (data?.findings ?? []).filter((f) => selected.has(rowKey(f)) && !blockedReason(f));
  const projectOf = (ws: string) =>
    data?.tags_unavailable.find((w) => w.ws_id === ws)?.project ||
    (data?.findings ?? []).find((f) => f.ws_id === ws)?.project ||
    ws;
  // Sent in batches of MAX_TAG_BATCH, one after another. Only the rows that
  // were sent leave the selection, so a row ticked meanwhile stays ticked. A
  // failed first batch is a failed change (it throws); a later one stops the
  // rest and the result says what was applied before it. The server answers
  // a batch whose ids are all unknown with a 404: those findings were deleted
  // since the list loaded, so they read as gone and the rest go on.
  const applyBulk = async (mode: 'add' | 'remove', tag: string) => {
    const forQ = q;
    const forScope = scopeKey;
    const rows = selectedRows;
    // Each finding by (workspace, id): ids repeat across workspaces, and a
    // tag must land only on the rows ticked.
    const targets = [...new Map(rows.map((f) => [`${f.ws_id}\u0000${f.id}`, { ws_id: f.ws_id, id: f.id }])).values()];
    const change = mode === 'add' ? { add: [tag] } : { remove: [tag] };
    const results: TagAcrossResult[] = [];
    const sent = new Set<string>();
    const keyOf = (t: { ws_id: string; id: string }) => `${t.ws_id}\u0000${t.id}`;
    let stopped: { error: string; left: number } | null = null;
    for (let i = 0; i < targets.length; i += MAX_TAG_BATCH) {
      const batch = targets.slice(i, i + MAX_TAG_BATCH);
      try {
        results.push(await api.tagFindings(batch, change));
      } catch (e: unknown) {
        if (e instanceof ApiError && e.status === 404) {
          results.push({ workspaces: [], unknown: batch.map((t) => t.id) });
          for (const t of batch) sent.add(keyOf(t));
          continue;
        }
        if (results.length === 0) throw e;
        stopped = { error: apiErrorMessage(e), left: targets.length - i };
        break;
      }
      for (const t of batch) sent.add(keyOf(t));
    }
    const merged = summarizeTagResult(mergeTagResults(results), mode, projectOf);
    const summary = stopped
      ? {
          message: `${merged.message} The other ${plural(stopped.left)} ${stopped.left === 1 ? "wasn't" : "weren't"} changed: ${stopped.error}`,
          ok: false,
        }
      : merged;
    const sentKeys = new Set(rows.filter((f) => sent.has(keyOf(f))).map(rowKey));
    if (currentQ.current === forQ && currentScope.current === forScope) setBulkNote(summary);
    setSelected((prev) => new Set([...prev].filter((k) => !sentKeys.has(k))));
    setReload((n) => n + 1);
    return summary;
  };

  // The active tile: a lone, positive `severity:<x>` token.
  const sole = soleValues(q, FINDING_FIELDS, 'severity');
  const active = sole?.length === 1 ? (sole[0] as Severity) : null;
  // A fetch for this query is in flight (the last answer was for another).
  const loading = data === null && !localError && !error;
  const noneAtAll = !q && data !== null && tileSummary.total === 0 && data.findings.length === 0;

  return (
    <>
      {error && <ErrorBanner className="mb-4">{error}</ErrorBanner>}

      {loading && !settled ? (
        <div className="py-16 flex items-center justify-center">
          <Spinner label="Loading findings…" />
        </div>
      ) : noneAtAll ? (
        <EmptyState title={empty.title} hint={empty.hint} />
      ) : (
        <div className="space-y-6">
          {data && (
            <FindingMetrics
              summary={tileSummary}
              active={active}
              onSelect={(sev) => setQ(withTerm(q, FINDING_FIELDS, 'severity', sev && sev !== active ? [sev] : null))}
            />
          )}

          <div className="flex flex-wrap items-start gap-2.5">
            <div className="min-w-0 flex-1">
              <QueryBar
                value={q}
                onChange={setQ}
                fields={FINDING_FIELDS}
                facets={facets}
                valueTone={sevTone}
                label="Filter findings"
              />
            </div>
            {/* Attaches Markdown reports to findings the scope may hold; a run's
                findings come from that run, so its tab has no import. */}
            {data && !runId && <ImportReportsButton onImported={() => setReload((n) => n + 1)} />}
            {/* The report covers exactly the rows the query leaves. */}
            {data && (
              <ExportReportButton
                findings={data.findings}
                defaultTitle={typeof exportTitle === 'function' ? exportTitle(data.findings) : exportTitle}
                wsId={exportWsId}
              />
            )}
          </div>

          {localError && (
            <p role="alert" className="rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">
              {localError}
            </p>
          )}

          {loading && (
            <div className="flex items-center gap-2 text-note text-ink-mute">
              <Spinner size="sm" label="Loading findings…" />
            </div>
          )}

          {data && unreadable.length > 0 && (
            <div
              role="status"
              className="rounded-lg bg-warn-bg px-3 py-2 text-note text-warn ring-1 ring-warn/30"
            >
              Tags for {unreadable.join(', ')} couldn't be read, so tag filters may miss their findings.
            </div>
          )}

          {data && selectedRows.length > 0 && (
            <BulkTagBar
              count={selectedRows.length}
              suggestions={(data.facets?.tag ?? []).map((v) => ({ tag: v.value, count: v.count }))}
              onApply={applyBulk}
              onClear={() => setSelected(new Set())}
            />
          )}

          {/* The bar unmounts once the selection clears; this keeps its result. */}
          {/* About the write, not the list: it stays even if the reload fails. */}
          {selectedRows.length === 0 && bulkNote && (
            <p
              role={bulkNote.ok ? 'status' : 'alert'}
              className={bulkNote.ok ? 'text-note text-ink-dim' : 'text-note text-err'}
            >
              {bulkNote.message}
            </p>
          )}

          {data &&
            (data.findings.length === 0 && q ? (
              <EmptyState title="No matches" hint={`No findings match \`${q}\`.`} />
            ) : (
              <FindingsTable findings={data.findings} showProvenance wsId={tableWsId} selection={selection} />
            ))}
        </div>
      )}
    </>
  );
}

/** "Import": attach reports written before the full profile (a dry run first). */
function ImportReportsButton({ onImported }: { onImported: () => void }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button variant="secondary" onClick={() => setOpen(true)}>
        Import
      </Button>
      {open && <ImportFindingsDialog onClose={() => setOpen(false)} onImported={onImported} />}
    </>
  );
}
