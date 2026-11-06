// Operator MCP Plan 3b, UI-1 — the client for the run and session controls
// Plan 3a added to the CP API (docs/cp-api.md): retry a finished workflow run,
// continue a standalone agent run, stop and compact a session. Every one is a
// write that is recorded, not done: a 2xx is never the confirmation; the run
// or session record changing is (CLAUDE.md rule 9, carried over to the CP).
//
// `runsApi` is an object, not loose functions, so tests can `vi.spyOn` it the
// way they spy on `api`.

import { apiRequest, type RunStatusStr } from '../api';

export type PermissionMode = 'ask' | 'bypass' | 'readonly';

export const PERMISSION_MODES: readonly PermissionMode[] = ['ask', 'bypass', 'readonly'];

const MODE_LABEL: Record<PermissionMode, string> = {
  ask: 'Ask',
  bypass: 'Bypass',
  readonly: 'Read-only',
};

export function modeLabel(mode: PermissionMode): string {
  return MODE_LABEL[mode];
}

/** The recorded `permission_mode` of a run, when it is one of the three. */
export function asPermissionMode(v: unknown): PermissionMode | null {
  return v === 'ask' || v === 'bypass' || v === 'readonly' ? v : null;
}

/** Only a failed, rejected or cancelled workflow run can be retried (a paused
 *  one is resumed; the server answers 409 for the rest). */
export function isRetryable(status: RunStatusStr | string): boolean {
  return status === 'failed' || status === 'rejected' || status === 'cancelled';
}

/** A run store record of a standalone agent run (`rupu run` records
 *  `agent:<name>`): it lists beside workflow runs but is continued, not
 *  retried (the retry endpoint answers 409). */
export function isAgentRunRecord(workflowName: string): boolean {
  return workflowName.startsWith('agent:');
}

/** How long a requested retry may go untaken (its resume marker still on the
 *  run) before the page says it never started and offers Retry again. */
export const RETRY_STALL_MS = 60_000;

/** The message for a retry whose resume never took the run. */
export const RETRY_STALLED_MESSAGE =
  'The retry was recorded but hasn’t started. Its log is retry.log in the run’s directory.';

/** `POST /api/runs/:id/retry` → 202. */
export interface RetryResponse {
  run_id: string;
  retrying: boolean;
  restart_interrupted: boolean;
  host_id: string;
}

/** `POST /api/runs/agents/:id/continue` → 202: the new run and the one it continues. */
export interface ContinueResponse {
  run_id: string;
  continues: string;
  host_id: string;
}

/** `POST /api/sessions/:id/compact` → 202. */
export interface CompactResponse {
  ok: boolean;
  id: string;
  compaction_id: string;
  log: string;
}

/** `POST /api/sessions/:id/stop` → 200 once the record says stopped. */
export interface StopSessionResponse {
  ok: boolean;
  id: string;
}

export type CompactionOutcome = 'compacted' | 'nothing_to_compact' | 'discarded' | 'failed';

/** How the session's last compaction ended (`last_compaction`). */
export interface CompactionResult {
  id: string;
  outcome: CompactionOutcome;
  detail: string;
  finished_at: string;
}

/** A compaction in progress (`compaction`). */
export interface CompactionMarker {
  id: string;
  pid: number;
  pid_start?: number | null;
  started_at: string;
}

/** The compaction keys a LOCAL `GET /api/sessions/:id` adds to the detail. */
export interface SessionCompaction {
  compacting?: boolean;
  compaction?: CompactionMarker | null;
  last_compaction?: CompactionResult | null;
}

function post<T>(path: string, body: unknown): Promise<T> {
  return apiRequest<T>(path, { method: 'POST', body: JSON.stringify(body) });
}

function blankToUndefined(v: string | undefined): string | undefined {
  const t = v?.trim();
  return t ? t : undefined;
}

export const runsApi = {
  /** Retry a failed, rejected or cancelled workflow run. No `mode` keeps the
   *  run's recorded launch mode. 409 for a run in any other state; local only. */
  retryRun(id: string, opts: { mode?: PermissionMode; restartInterrupted?: boolean }): Promise<RetryResponse> {
    const body: { restart_interrupted: boolean; mode?: PermissionMode } = {
      restart_interrupted: opts.restartInterrupted ?? false,
    };
    if (opts.mode) body.mode = opts.mode;
    return post(`/api/runs/${encodeURIComponent(id)}/retry`, body);
  },

  /** Continue a standalone agent run under a new run id, restoring every flag
   *  of its launch; `model` / `provider` replace their own. `prompt` is what
   *  the run is told next: sent as typed when it isn't blank (with it, any
   *  ended run carries on). 409 while the run is still running; local only. */
  continueAgentRun(
    id: string,
    opts: { prompt?: string; model?: string; provider?: string },
  ): Promise<ContinueResponse> {
    const body: { prompt?: string; model?: string; provider?: string } = {};
    const model = blankToUndefined(opts.model);
    const provider = blankToUndefined(opts.provider);
    if (opts.prompt && opts.prompt.trim() !== '') body.prompt = opts.prompt;
    if (model) body.model = model;
    if (provider) body.provider = provider;
    return post(`/api/runs/agents/${encodeURIComponent(id)}/continue`, body);
  },

  /** End a session's worker and its running turn. */
  stopSession(id: string): Promise<StopSessionResponse> {
    return post(`/api/sessions/${encodeURIComponent(id)}/stop`, {});
  },

  /** Compact the stored conversation now. `window` (tokens) sizes it in place
   *  of the session's own context window. */
  compactSession(id: string, window?: number): Promise<CompactResponse> {
    return post(`/api/sessions/${encodeURIComponent(id)}/compact`, window != null ? { window } : {});
  },
};

/** The text after the first `: ` of a server detail line, else the line. */
function afterColon(detail: string): string {
  const i = detail.indexOf(': ');
  return i >= 0 ? detail.slice(i + 2) : detail;
}

/** The one line the conversation shows for a finished compaction. */
export function compactionLine(r: CompactionResult): string {
  switch (r.outcome) {
    case 'compacted': {
      const m = /summarized (\d+) messages?, (history .+)$/.exec(r.detail);
      return m ? `Compacted ${m[1]} messages into 1: ${m[2]}` : 'Compacted the conversation';
    }
    case 'discarded':
      return `Compaction discarded: ${afterColon(r.detail)}`;
    case 'nothing_to_compact':
      return `Nothing to compact: ${afterColon(r.detail)}`;
    case 'failed':
      return `Compaction failed: ${r.detail}`;
  }
}
