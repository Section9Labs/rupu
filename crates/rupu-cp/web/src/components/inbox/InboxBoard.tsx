// InboxBoard — the operator action board, rendered from a `UseInbox` result so
// it draws without network in tests. Two sections, split by what the operator
// can actually do (honest about the backend): "Waiting on you" (gates + paused,
// with the real approve/reject/resume mutations) and "Recent failures" (no web
// retry — open or clear). Every row can also be snoozed or dismissed, always
// one click, so the board can be worked to zero. When it is, an "inbox zero"
// fact sheet hands the operator back to the system.
//
// Visual language: StatusChip per row, a left-edge WaitBar sized by relative
// wait, mono codename, a meta line that names its baseline ("waiting 2h 14m"),
// a `solid` primary action + a `secondary`, and always-visible Snooze / Dismiss
// text controls. Loading shows skeleton rows in the real layout.

import { useState } from 'react';
import { Link } from 'react-router-dom';
import { Button } from '../ui/Button';
import { StatusChip, type ChipTone } from '../ui/StatusChip';
import { Skeleton } from '../ui/Skeleton';
import { AgentName } from '../codename/AgentName';
import { WaitBar } from './WaitBar';
import { SnoozeMenu } from './SnoozeMenu';
import { formatDuration } from '../../lib/duration';
import { runHref } from '../../lib/runs';
import type { InboxItem, SnoozedEntry, UseInbox } from '../../lib/dashboard/useInbox';
import { ChevronRight } from 'lucide-react';

const KIND_CHIP: Record<InboxItem['kind'], { tone: ChipTone; label: string }> = {
  awaiting: { tone: 'awaiting_approval', label: 'gate' },
  paused: { tone: 'paused', label: 'paused' },
  failed: { tone: 'failed', label: 'failed' },
};

const HOUR = 3_600_000;

function sinceMs(item: InboxItem): number {
  return item.since ? Date.parse(item.since) : 0;
}

function ageLabel(item: InboxItem, now: number): string {
  if (!item.since) return item.kind === 'failed' ? 'failed' : 'waiting';
  const ago = formatDuration(now - sinceMs(item));
  if (item.kind === 'failed') return `failed ${ago} ago`;
  if (item.kind === 'paused') return `paused ${ago}`;
  return `waiting ${ago}`;
}

/** A section heading in the triage flow. */
function SectionLabel({
  title,
  count,
  hint,
}: {
  title: string;
  count?: number;
  hint?: string;
}) {
  return (
    <h2 className="mb-2.5 mt-6 flex flex-wrap items-baseline gap-x-2 text-note font-semibold uppercase tracking-wide text-ink-dim first:mt-0">
      <span>{title}</span>
      {count != null && <span className="tabular-nums text-ink-mute">· {count}</span>}
      {hint && (
        <span className="font-normal normal-case tracking-normal text-ink-mute">{hint}</span>
      )}
    </h2>
  );
}

function ClearControls({
  item,
  data,
  disabled,
}: {
  item: InboxItem;
  data: UseInbox;
  disabled?: boolean;
}) {
  return (
    <div className="flex items-center gap-1">
      <SnoozeMenu disabled={disabled} onSnooze={(dur) => data.snooze(item, dur)} />
      <button
        type="button"
        disabled={disabled}
        onClick={() => data.dismiss(item)}
        className="rounded px-1.5 py-1 text-note font-medium text-ink-mute transition-colors hover:text-ink disabled:opacity-50"
      >
        Dismiss
      </button>
    </div>
  );
}

function Row({
  item,
  data,
  longestWait,
}: {
  item: InboxItem;
  data: UseInbox;
  longestWait: number;
}) {
  const chip = KIND_CHIP[item.kind];
  const now = data.now;
  const busy = data.pending[item.id] != null;
  const href = runHref({ id: item.id, host_id: item.host === 'local' ? undefined : item.host });
  const actionable = item.kind !== 'failed';
  const waitMs = now - sinceMs(item);
  const urgent = actionable && item.since != null && waitMs >= HOUR;

  return (
    <div className="flex items-center gap-3 border-t border-border px-4 py-3 first:border-t-0 hover:bg-surface/60">
      {actionable && longestWait > 0 ? (
        <WaitBar fraction={waitMs / longestWait} tone={item.kind === 'awaiting' ? 'awaiting' : 'paused'} />
      ) : (
        <span className="w-[3px] shrink-0" aria-hidden />
      )}
      <StatusChip tone={chip.tone} dot>
        {chip.label}
      </StatusChip>
      <Link to={href} className="min-w-0 shrink-0 truncate hover:underline">
        <AgentName codename={item.codename} derived={item.codenameDerived} showCrew />
      </Link>
      <span className="flex min-w-0 flex-1 items-center gap-2 truncate text-note text-ink-dim">
        <span className="truncate">{item.workflowName}</span>
        {item.host !== 'local' && (
          <span className="shrink-0 rounded bg-surface px-1.5 py-0.5 font-mono text-meta text-ink-mute">
            {item.host}
          </span>
        )}
      </span>
      <span
        className={`shrink-0 tabular-nums text-note ${urgent ? 'font-semibold text-warn' : 'text-ink-dim'}`}
      >
        {ageLabel(item, now)}
      </span>

      <div className="flex shrink-0 items-center gap-2">
        {item.kind === 'awaiting' && (
          <>
            <Button variant="danger-outline" size="sm" disabled={busy} onClick={() => data.reject(item)}>
              {data.pending[item.id] === 'reject' ? 'Rejecting…' : 'Reject'}
            </Button>
            <Button variant="solid" size="sm" disabled={busy} onClick={() => data.approve(item)}>
              {data.pending[item.id] === 'approve' ? 'Approving…' : 'Approve'}
            </Button>
          </>
        )}
        {item.kind === 'paused' && (
          <Button variant="solid" size="sm" disabled={busy} onClick={() => data.resume(item)}>
            {data.pending[item.id] === 'resume' ? 'Resuming…' : 'Resume'}
          </Button>
        )}
        {item.kind === 'failed' && (
          <Link
            to={href}
            className="inline-flex items-center justify-center rounded-md border border-border bg-panel px-2.5 py-1 text-note font-medium text-ink transition-colors hover:bg-bg"
          >
            View run
          </Link>
        )}
        <span className="mx-0.5 h-4 w-px bg-border" aria-hidden />
        <ClearControls item={item} data={data} disabled={busy} />
      </div>
    </div>
  );
}

function RowSkeleton() {
  return (
    <div className="flex items-center gap-3 border-t border-border px-4 py-3 first:border-t-0">
      <span className="w-[3px] shrink-0" />
      <Skeleton className="h-5 w-14 rounded-full" />
      <Skeleton className="h-4 w-40" />
      <span className="flex-1" />
      <Skeleton className="h-4 w-20" />
      <Skeleton className="h-7 w-24 rounded-md" />
    </div>
  );
}

function SnoozedDrawer({ data }: { data: UseInbox }) {
  const [open, setOpen] = useState(false);
  const entries = data.snoozed;
  if (entries.length === 0) return null;
  return (
    <div className="mt-3 overflow-hidden rounded-lg border border-dashed border-border-strong">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex w-full items-center gap-2 px-4 py-2.5 text-note font-semibold uppercase tracking-wide text-ink-dim transition-colors hover:bg-surface/60"
        aria-expanded={open}
      >
        <ChevronRight
          size={13}
          className={`transition-transform ${open ? 'rotate-90' : ''}`}
          aria-hidden
        />
        Snoozed
        <span className="tabular-nums text-ink-mute">· {entries.length}</span>
        {!open && entries[0] && (
          <span className="font-normal normal-case tracking-normal text-ink-mute">
            next returns {formatDuration(entries[0].until - data.now)}
          </span>
        )}
      </button>
      {open && (
        <div className="border-t border-border">
          {entries.map(({ item, until }: SnoozedEntry) => (
            <div
              key={`${item.host}:${item.id}`}
              className="flex items-center gap-3 border-t border-border px-4 py-2.5 first:border-t-0"
            >
              <StatusChip tone={KIND_CHIP[item.kind].tone}>{KIND_CHIP[item.kind].label}</StatusChip>
              <span className="min-w-0 flex-1 truncate">
                <AgentName codename={item.codename} derived={item.codenameDerived} showCrew />
                <span className="ml-2 text-note text-ink-dim">{item.workflowName}</span>
              </span>
              <span className="shrink-0 text-note text-ink-mute">
                returns in {formatDuration(until - data.now)}
              </span>
              <button
                type="button"
                onClick={() => data.unsnooze(item.id)}
                className="shrink-0 rounded px-1.5 py-1 text-note font-medium text-ink-dim transition-colors hover:text-ink"
              >
                Un-snooze
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

/** The "inbox zero" fact sheet — no illustration; it states the facts and hands
 *  the operator back to the system (snoozed queue, what's live). */
function InboxZero({ data }: { data: UseInbox }) {
  const snoozedNext = data.snoozed[0];
  return (
    <div className="rounded-xl border border-border bg-panel px-6 py-10">
      <p className="text-lg font-semibold text-ink">Nothing waiting on you.</p>
      <p className="mt-1.5 text-ui text-ink-dim">
        {data.clearedThisSession > 0 && (
          <>
            <span className="tabular-nums text-ink">{data.clearedThisSession}</span> cleared this
            session
          </>
        )}
        {data.clearedThisSession > 0 && (snoozedNext || true) ? ' · ' : ''}
        {snoozedNext ? (
          <>
            <span className="tabular-nums text-ink">{data.snoozed.length}</span> snoozed, next
            returns in {formatDuration(snoozedNext.until - data.now)}
            {' · '}
          </>
        ) : (
          ''
        )}
        <Link to="/overview" className="text-ink underline-offset-2 hover:underline">
          See the system overview →
        </Link>
      </p>
    </div>
  );
}

export type InboxFilter = 'all' | 'gates' | 'paused' | 'failed';

/** Failures are "clear elsewhere" (no act), so the board shows only the most
 *  recent few by default and tucks the rest behind "show all" — a wall of old
 *  failures would drown the gates and paused runs that actually need action. */
const FAILURE_CAP = 6;

export function InboxBoard({ data, filter = 'all' }: { data: UseInbox; filter?: InboxFilter }) {
  const [showAllFailures, setShowAllFailures] = useState(false);
  const longestWait = data.actionable.reduce(
    (m, it) => Math.max(m, data.now - sinceMs(it)),
    0,
  );

  if (data.loading && data.actionable.length === 0 && data.failures.length === 0) {
    return (
      <div className="overflow-hidden rounded-xl border border-border bg-panel">
        {[0, 1, 2].map((i) => (
          <RowSkeleton key={i} />
        ))}
      </div>
    );
  }

  // The filter narrows which rows are shown; the snoozed/dismissed drawers and
  // the inbox-zero fact sheet always read the full board.
  const actionable =
    filter === 'gates'
      ? data.actionable.filter((it) => it.kind === 'awaiting')
      : filter === 'paused'
        ? data.actionable.filter((it) => it.kind === 'paused')
        : filter === 'failed'
          ? []
          : data.actionable;
  const failures = filter === 'all' || filter === 'failed' ? data.failures : [];

  const emptyAll = data.actionable.length === 0 && data.failures.length === 0;
  const emptyFiltered = actionable.length === 0 && failures.length === 0;

  return (
    <div>
      {data.error && (
        <div className="mb-3 rounded-lg border border-err/30 bg-err-bg px-4 py-2.5 text-note text-err">
          Something didn’t go through: {data.error}
        </div>
      )}

      {emptyAll ? (
        <InboxZero data={data} />
      ) : emptyFiltered ? (
        <div className="rounded-xl border border-dashed border-border-strong px-6 py-8 text-center text-note text-ink-mute">
          Nothing in this view.
        </div>
      ) : (
        <>
          {actionable.length > 0 && (
            <>
              <SectionLabel
                title="Waiting on you"
                count={actionable.length}
                hint="a run is stopped until you act"
              />
              <div className="overflow-hidden rounded-xl border border-border bg-panel">
                {actionable.map((item) => (
                  <Row key={`${item.host}:${item.id}`} item={item} data={data} longestWait={longestWait} />
                ))}
              </div>
            </>
          )}

          {failures.length > 0 && (
            <>
              <SectionLabel
                title="Recent failures"
                count={failures.length}
                hint="nothing’s blocked — open the run or clear it"
              />
              <div className="overflow-hidden rounded-xl border border-border bg-panel">
                {(showAllFailures ? failures : failures.slice(0, FAILURE_CAP)).map((item) => (
                  <Row key={`${item.host}:${item.id}`} item={item} data={data} longestWait={longestWait} />
                ))}
                {failures.length > FAILURE_CAP && (
                  <button
                    type="button"
                    onClick={() => setShowAllFailures((v) => !v)}
                    className="flex w-full items-center justify-center border-t border-border px-4 py-2.5 text-note font-medium text-ink-dim transition-colors hover:bg-surface/60 hover:text-ink"
                  >
                    {showAllFailures ? 'Show fewer' : `Show all ${failures.length}`}
                  </button>
                )}
              </div>
            </>
          )}
        </>
      )}

      <SnoozedDrawer data={data} />

      {data.dismissed.length > 0 && (
        <div className="mt-3 text-note text-ink-mute">
          {data.dismissed.length} dismissed
          <button
            type="button"
            onClick={() => data.dismissed.forEach((it) => data.undismiss(it.id))}
            className="ml-2 text-ink-dim underline-offset-2 hover:text-ink hover:underline"
          >
            show again
          </button>
        </div>
      )}
    </div>
  );
}

export default InboxBoard;
