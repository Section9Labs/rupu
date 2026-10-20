// Operator MCP Plan 3b, package UI-5: the client for the autoflow operations,
// cron, maintenance, accounts and tracked-repo endpoints Plan 3a added
// (docs/cp-api.md: "Autoflows", "Cron and maintenance", "SCM, repos and auth
// status"). Every one is local-host only, so none takes a `host`. The types
// mirror the JSON reports the matching CLI commands print under
// `--format json`.

import { apiRequest } from '../api';
import { formatBytes } from '../netflow';

const post = (body: unknown): RequestInit => ({ method: 'POST', body: JSON.stringify(body) });

// --- Autoflows ---------------------------------------------------------------

export type AutoflowRunMode = 'bypass' | 'readonly';

/** `POST /api/autoflows/:name/run` → 202 once the detached cycle runs. */
export interface AutoflowRunReply {
  started: boolean;
  name: string;
  issue: string;
  pid: number;
  /** Where the child's stderr goes. */
  log: string;
}

/** A queued or recently processed wake (`autoflow_wakes` rows). */
export interface AutoflowWakeRow {
  wake_id: string;
  state: 'queued' | 'processed' | string;
  source: string;
  event: string;
  entity: string;
  not_before: string;
  repo: string;
}

export interface AutoflowWakesReport {
  kind: 'autoflow_wakes';
  version: number;
  rows: AutoflowWakeRow[];
}

/** One `autoflow doctor` problem; `scope` is the issue ref (or claim dir) it is about. */
export interface AutoflowDoctorRow {
  scope: string;
  problem: string;
  detail: string;
}

export interface AutoflowDoctorReport {
  kind: 'autoflow_doctor';
  version: number;
  ok: boolean;
  rows: AutoflowDoctorRow[];
}

/** `autoflow explain`: one issue's state. The claim and run sub-objects are
 *  reports with many optional lines, so they are kept as plain records and
 *  rendered key by key. */
export interface AutoflowExplainItem {
  issue: string;
  repo: string;
  status: string;
  source?: string | null;
  candidate_workflows: string[];
  claim?: Record<string, unknown> | null;
  queued_wakes: AutoflowWakeRow[];
  recent_processed_wake?: AutoflowWakeRow | null;
  recent_cycle_events: Record<string, unknown>[];
}

export interface AutoflowExplainReport {
  kind: 'autoflow_explain';
  version: number;
  item: AutoflowExplainItem;
}

export interface RepairClaimRequest {
  issue_ref: string;
  release?: boolean;
  requeue?: boolean;
  /** The server defaults it to `true`; always sent so the intent is explicit. */
  dry_run: boolean;
}

/** One line per repair, `would …` on a dry run. */
export interface RepairClaimReply {
  issue_ref: string;
  dry_run: boolean;
  repairs: string[];
}

// --- Cron --------------------------------------------------------------------

export interface CronListRow {
  name: string;
  schedule: string;
  /** `YYYY-MM-DD HH:MM:SS` UTC; null when the schedule can't be computed. */
  next_utc: string | null;
  in_seconds: number | null;
}

export interface CronListReport {
  kind: 'cron_list';
  version: number;
  rows: CronListRow[];
}

export interface CronEventsRow {
  name: string;
  event: string;
  sources: string[];
  cursor: string | null;
}

export interface CronEventsReport {
  kind: 'cron_events';
  version: number;
  rows: CronEventsRow[];
  summary: { poll_sources: string[] };
}

export interface WouldFire {
  tier: 'cron' | 'event' | string;
  workflow: string;
  last_fired?: string;
  event?: string;
  delivery?: string;
  run_id?: string;
}

export type CronTickReply =
  | { kind: 'cron_tick'; dry_run: true; would_fire: WouldFire[] }
  | { started: true; dry_run: false };

// --- Maintenance -------------------------------------------------------------

export type PruneKind = 'transcripts' | 'sessions' | 'netflow' | 'cleanup';

export interface PruneRequest {
  kind: PruneKind;
  /** `30d`, `12h`, `1w`; absent → the configured retention. */
  older_than?: string;
  dry_run: boolean;
}

/** The CLI report's rows: their shape depends on the kind. */
export type PruneRow = Record<string, unknown>;

export interface PruneReply {
  kind: PruneKind;
  dry_run: boolean;
  would_remove?: PruneRow[];
  removed?: PruneRow[];
  /** Files or directories that couldn't be removed or scanned. */
  errors: string[];
}

const PRUNE_NOUNS: Record<PruneKind, [string, string]> = {
  transcripts: ['transcript', 'transcripts'],
  sessions: ['session', 'sessions'],
  netflow: ['ledger', 'ledgers'],
  cleanup: ['item', 'items'],
};

/** What a prune would remove, in one phrase: `412 transcripts`, `2 ledgers · 2.0 MB`, `nothing`. */
export function pruneSummary(kind: PruneKind, rows: PruneRow[]): string {
  if (rows.length === 0) return 'nothing';
  const [one, many] = PRUNE_NOUNS[kind];
  const count = `${rows.length} ${rows.length === 1 ? one : many}`;
  if (kind !== 'netflow') return count;
  const bytes = rows.reduce((sum, r) => sum + (typeof r.bytes === 'number' ? r.bytes : 0), 0);
  return `${count} · ${formatBytes(bytes)}`;
}

// --- Accounts ----------------------------------------------------------------

export type AuthState = 'ok' | 'needs_login' | 'absent';

export interface SsoStatus {
  expires_at: string | null;
  expired: boolean;
  /** A refresh token is stored: an expired token is renewed on next use. */
  refreshable: boolean;
  /** `expires in 8d`, `expired — re-login`, `no expiry`. */
  summary: string;
}

/** `GET /api/auth/status` row. Never carries a credential. */
export interface AccountAuth {
  account: string;
  kind: string | null;
  api_key: boolean;
  sso: SsoStatus | null;
  state: AuthState;
}

export type ScmRuleTarget = { owner: string } | { path: string };

/** `GET /api/scm/accounts` row. */
export interface ScmAccountRow {
  account: string;
  kind: string;
  base_url: string | null;
  rules: ScmRuleTarget[];
  credentials: boolean;
  state: AuthState;
}

export interface ScmRule {
  owner?: string;
  path?: string;
  account: string;
}

export interface ScmBindReply {
  rule: ScmRule;
  /** Set when `account` is not a declared account: the rule may never match. */
  warning?: string;
  rules: ScmRule[];
}

/** The credential kinds an account holds, as the CLI names them. */
export function accountModes(a: Pick<AccountAuth, 'api_key' | 'sso'>): string {
  const modes = [a.api_key && 'api-key', a.sso && 'sso'].filter(Boolean);
  return modes.length ? modes.join(', ') : '—';
}

/** The CLI line that fixes an account. Logging in stays human-only. */
export function loginCommand(account: string, sso: boolean): string {
  return `rupu auth login --account ${account}${sso ? ' --mode sso' : ''}`;
}

export function ruleLines(rules: ScmRule[]): { target: string; account: string }[] {
  return rules.map((r) => ({
    target: r.owner !== undefined ? `owner ${r.owner}` : `path ${r.path ?? ''}`,
    account: r.account,
  }));
}

// --- Tracked repos -----------------------------------------------------------

export interface TrackedRepo {
  /** The record key `DELETE /api/repos/tracked/:id` takes. */
  id: string;
  /** `<platform>:<owner>/<repo>`. */
  repo: string;
  preferred_path: string;
  known_paths: string[];
  default_branch?: string;
  last_seen_at: string;
}

export interface TrackRequest {
  /** Run-target syntax, e.g. `github:owner/repo`. */
  repo: string;
  /** Absolute path of the checkout. */
  path: string;
}

export interface ForgetReply {
  repo: string;
  path?: string;
  /** The record left, or null when the repo is forgotten. */
  remaining: TrackedRepo | null;
}

// --- Client ------------------------------------------------------------------

/** An object (not bare functions) so page tests can `vi.spyOn(admin, …)`. */
export const admin = {
  runAutoflow(name: string, body: { issue: string; repo?: string; mode?: AutoflowRunMode }): Promise<AutoflowRunReply> {
    return apiRequest(`/api/autoflows/${encodeURIComponent(name)}/run`, post(body));
  },
  getAutoflowExplain(issueRef: string): Promise<AutoflowExplainReport> {
    return apiRequest(`/api/autoflows/explain?${new URLSearchParams({ issue_ref: issueRef })}`);
  },
  getAutoflowDoctor(): Promise<AutoflowDoctorReport> {
    return apiRequest('/api/autoflows/doctor');
  },
  getAutoflowWakes(): Promise<AutoflowWakesReport> {
    return apiRequest('/api/autoflows/wakes');
  },
  repairClaim(body: RepairClaimRequest): Promise<RepairClaimReply> {
    return apiRequest('/api/autoflows/claims/repair', post(body));
  },

  getCronList(): Promise<CronListReport> {
    return apiRequest('/api/cron');
  },
  getCronEvents(): Promise<CronEventsReport> {
    return apiRequest('/api/cron/events');
  },
  cronTick(body: { dry_run: boolean }): Promise<CronTickReply> {
    return apiRequest('/api/cron/tick', post(body));
  },

  prune(body: PruneRequest): Promise<PruneReply> {
    return apiRequest('/api/maintenance/prune', post(body));
  },

  getAuthStatus(): Promise<AccountAuth[]> {
    return apiRequest('/api/auth/status');
  },
  getScmAccounts(): Promise<ScmAccountRow[]> {
    return apiRequest('/api/scm/accounts');
  },
  getScmRules(): Promise<ScmRule[]> {
    return apiRequest('/api/scm/rules');
  },
  addScmRule(rule: ScmRule): Promise<ScmBindReply> {
    return apiRequest('/api/scm/rules', post(rule));
  },

  getTrackedRepos(): Promise<TrackedRepo[]> {
    return apiRequest('/api/repos/tracked');
  },
  attachRepo(body: TrackRequest): Promise<TrackedRepo> {
    return apiRequest('/api/repos/tracked', post(body));
  },
  preferRepo(body: TrackRequest): Promise<TrackedRepo> {
    return apiRequest('/api/repos/tracked/prefer', post(body));
  },
  forgetRepo(id: string, path?: string): Promise<ForgetReply> {
    const q = path ? `?${new URLSearchParams({ path })}` : '';
    return apiRequest(`/api/repos/tracked/${encodeURIComponent(id)}${q}`, { method: 'DELETE' });
  },
};
