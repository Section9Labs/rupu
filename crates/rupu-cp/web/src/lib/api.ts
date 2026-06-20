/**
 * rupu Control Plane — typed API client + SSE helpers.
 *
 * Pattern lifted from the Okesu sibling project:
 *   • `ApiError` class with status + body
 *   • `request<T>` typed-fetch wrapper (same-origin, JSON)
 *   • EventSource-based subscribe helpers (unnamed `data` events → `onmessage`)
 */

import type { TranscriptEvent, TranscriptResponse } from './transcript';
export type { TranscriptEvent, TranscriptResponse } from './transcript';
import type { UsageSummary, UsageOverview } from './usage';
export type { UsageSummary, UsageBreakdownRow, UsageOverview } from './usage';

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

export class ApiError extends Error {
  status: number;
  body: string;
  constructor(status: number, message: string, body = '') {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.body = body;
  }
}

// ---------------------------------------------------------------------------
// Core fetch wrapper
// ---------------------------------------------------------------------------

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, {
    credentials: 'same-origin',
    headers: { 'Content-Type': 'application/json' },
    ...init,
  });
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText);
    throw new ApiError(res.status, text || res.statusText, text);
  }
  if (res.status === 204) return undefined as T;
  const text = await res.text();
  if (!text) return undefined as T;
  return JSON.parse(text) as T;
}

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

export type RunStatusStr =
  | 'pending'
  | 'running'
  | 'completed'
  | 'failed'
  | 'awaiting_approval'
  | 'rejected';

/** Mirrors rupu-orchestrator's persisted `run.json` record. */
export interface RunRecord {
  id: string;
  workflow_name: string;
  status: RunStatusStr;
  started_at: string;
  finished_at?: string | null;
  inputs?: Record<string, string>;
  workspace_id?: string;
  workspace_path?: string;
  transcript_dir?: string;
  error_message?: string | null;
  awaiting_step_id?: string | null;
  approval_prompt?: string | null;
  awaiting_since?: string | null;
  expires_at?: string | null;
  issue_ref?: string | null;
  [k: string]: unknown;
}

/** Mirrors `StepResultRecord` from rupu-orchestrator/src/runs.rs. */
export interface StepResultRecord {
  run_id: string;
  step_id: string;
  transcript_path?: string;
  output?: string;
  success?: boolean;
  skipped?: boolean;
  rendered_prompt?: string;
  kind?: string;
  items?: unknown[];
  findings?: Array<{
    source: string;
    severity: string;
    title: string;
    body: string;
  }>;
  iterations?: number;
  resolved?: boolean;
  finished_at?: string;
  [k: string]: unknown;
}

/**
 * Internally-tagged event union (`type` discriminator, `snake_case`).
 *
 * Full variant set from rupu-orchestrator/src/executor/event.rs:
 *   run_started | step_started | step_working | step_awaiting_approval
 *   step_completed | step_failed | step_skipped
 *   unit_started | unit_completed
 *   run_completed | run_failed
 *
 * Common narrowed variants below; catch-all fields on the base allow
 * callers to read any variant without exhaustive narrowing.
 */
/**
 * The concrete, fully-narrowable event variants (each has a string-literal
 * `type`). Switching on `type` over this union narrows cleanly.
 */
export type KnownRunEvent =
  | RunStartedEvent
  | StepStartedEvent
  | StepWorkingEvent
  | StepAwaitingApprovalEvent
  | StepCompletedEvent
  | StepFailedEvent
  | StepSkippedEvent
  | UnitStartedEvent
  | UnitCompletedEvent
  | PanelRoundEvent
  | RunCompletedEvent
  | RunFailedEvent;

export type RunEvent = KnownRunEvent | UnknownRunEvent;

interface RunEventBase {
  type: string;
  run_id: string;
  [k: string]: unknown;
}

export interface RunStartedEvent extends RunEventBase {
  type: 'run_started';
  event_version: number;
  workflow_path: string;
  started_at: string;
}

export interface StepStartedEvent extends RunEventBase {
  type: 'step_started';
  step_id: string;
  kind: string;
  agent?: string | null;
}

export interface StepWorkingEvent extends RunEventBase {
  type: 'step_working';
  step_id: string;
  note?: string | null;
}

export interface StepAwaitingApprovalEvent extends RunEventBase {
  type: 'step_awaiting_approval';
  step_id: string;
  reason: string;
}

export interface StepCompletedEvent extends RunEventBase {
  type: 'step_completed';
  step_id: string;
  success: boolean;
  duration_ms: number;
}

export interface StepFailedEvent extends RunEventBase {
  type: 'step_failed';
  step_id: string;
  error: string;
}

export interface StepSkippedEvent extends RunEventBase {
  type: 'step_skipped';
  step_id: string;
  reason: string;
}

export interface UnitStartedEvent extends RunEventBase {
  type: 'unit_started';
  step_id: string;
  index: number;
  unit_key: string;
  agent?: string | null;
  transcript_path: string;
}

export interface UnitCompletedEvent extends RunEventBase {
  type: 'unit_completed';
  step_id: string;
  index: number;
  unit_key: string;
  success: boolean;
  tokens_in: number;
  tokens_out: number;
}

export interface PanelRoundEvent extends RunEventBase {
  type: 'panel_round';
  step_id: string;
  round: number;
  max_iterations: number;
  max_severity_remaining?: string | null;
}

export interface RunCompletedEvent extends RunEventBase {
  type: 'run_completed';
  status: RunStatusStr;
  finished_at: string;
}

export interface RunFailedEvent extends RunEventBase {
  type: 'run_failed';
  error: string;
  finished_at: string;
}

/** Catch-all for any variant not yet narrowed above. */
export interface UnknownRunEvent extends RunEventBase {
  type: string;
  step_id?: string;
}

const KNOWN_EVENT_TYPES: ReadonlySet<KnownRunEvent['type']> = new Set([
  'run_started',
  'step_started',
  'step_working',
  'step_awaiting_approval',
  'step_completed',
  'step_failed',
  'step_skipped',
  'unit_started',
  'unit_completed',
  'panel_round',
  'run_completed',
  'run_failed',
]);

/**
 * Type-guard that narrows a `RunEvent` to a fully-typed `KnownRunEvent`.
 * Needed because `UnknownRunEvent.type` is `string`, so a bare `switch` on
 * `type` never removes the catch-all from the union (its fields stay `unknown`).
 * Guard first, then switch over the narrowed `KnownRunEvent`.
 */
export function isKnownRunEvent(ev: RunEvent): ev is KnownRunEvent {
  return KNOWN_EVENT_TYPES.has(ev.type as KnownRunEvent['type']);
}

// ---------------------------------------------------------------------------
// Slim run list row (returned by /api/runs and /api/runs/workflows)
// ---------------------------------------------------------------------------

export interface RunListRow {
  id: string;
  workflow_name: string;
  status: RunStatusStr;
  started_at: string;
  finished_at?: string | null;
  trigger: 'manual' | 'cron' | 'event';
  usage: UsageSummary;
}

// ---------------------------------------------------------------------------
// Run-graph types
// ---------------------------------------------------------------------------

/** Step-DAG node from /api/runs/:id/graph .workflow.steps */
export interface StepNodeDto {
  id: string;
  kind: 'step' | 'for_each' | 'parallel' | 'panel';
  agent?: string | null;
  for_each?: string | null;
  parallel?: { id: string; agent: string }[] | null;
  panelists?: string[] | null;
  gate?: {
    max_iterations: number;
    until_severity: 'low' | 'medium' | 'high' | 'critical';
    fix_with: string;
  } | null;
}

/**
 * Unit checkpoint from /api/runs/:id/graph .units.
 * Written when a for_each unit's agent run completes (terminal).
 */
export interface UnitCheckpoint {
  step_id: string;
  index: number;
  item: unknown;            // serde_json::Value — may be a string, object, or array
  run_id: string;
  transcript_path: string;
  output: string;
  success: boolean | null;
  finished_at: string;      // ISO-8601
}

export interface RunGraphResponse {
  run: RunRecord;
  workflow: { steps: StepNodeDto[] };
  step_results: StepResultRecord[];
  units: UnitCheckpoint[];
  usage?: UsageSummary;
}

// ---------------------------------------------------------------------------
// Autoflow / agent-run list types
// ---------------------------------------------------------------------------

export interface AutoflowCycleRow {
  cycle_id: string;
  mode: string;
  worker_name?: string | null;
  started_at: string;
  finished_at: string;
  workflow_count: number;
  ran_cycles: number;
  skipped_cycles: number;
  failed_cycles: number;
  run_ids: string[];
}

/**
 * One actionable autoflow *event* — a launched run or an awaiting/failed
 * signal. `kind` is snake_case (`run_launched` | `awaiting_human` |
 * `awaiting_external` | `cycle_failed`). When `run_id` is present the row links
 * to the run graph.
 */
export interface AutoflowEventRow {
  event_id: string;
  cycle_id: string;
  at: string;
  kind: string;
  workflow?: string | null;
  issue_display_ref?: string | null;
  run_id?: string | null;
  status?: string | null;
  worker_name?: string | null;
}

export interface AgentRunRow {
  run_id: string;
  source: 'standalone' | 'session';
  agent?: string | null;
  session_id?: string | null;
  trigger_source?: string | null;
  status?: string | null;
  started_at?: string | null;
  transcript_path?: string | null;
}

export interface AutoflowDefRow {
  name: string;
  trigger: string;
  scope: string;
}

// ---------------------------------------------------------------------------
// Dashboard
// ---------------------------------------------------------------------------

export interface DashboardResponse {
  runs: {
    total: number;
    by_status: Record<RunStatusStr, number>;
  };
  recent_runs: Array<{
    id: string;
    workflow_name: string;
    status: RunStatusStr;
    started_at: string;
    finished_at?: string | null;
  }>;
  sessions: { total: number; active: number; archived: number };
  workers: { total: number };
  coverage: { targets: number; assertions: number };
}

// ---------------------------------------------------------------------------
// Agents
// ---------------------------------------------------------------------------

export interface AgentSummary {
  name: string;
  description?: string | null;
  provider?: string | null;
  model?: string | null;
  effort?: string | null;
  max_tokens?: number | null;
  /**
   * `"project"` | `"global"` — only populated by the per-project endpoint
   * (`/api/projects/:wsId/agents`); the global `/api/agents` list always
   * returns `"global"`.
   */
  scope?: string;
}

export interface AgentDetail extends AgentSummary {
  system_prompt: string;
}

// ---------------------------------------------------------------------------
// Workflows
// ---------------------------------------------------------------------------

export interface WorkflowSummary {
  name: string;
  scope: string;
}

export interface WorkflowDetail {
  /** Parsed Workflow object — typed loosely; the UI inspects what it needs. */
  workflow: Record<string, unknown>;
  yaml: string;
  usage?: UsageSummary;
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

export interface SessionSummary {
  session_id: string;
  agent_name: string;
  model: string;
  status: unknown;
  total_turns: number;
  created_at: string;
  updated_at: string;
  active_run_id?: string | null;
  target?: string | null;
  scope: string;
  provider_name?: string;
  total_tokens_in?: number;
  total_tokens_out?: number;
  total_tokens_cached?: number;
  usage?: UsageSummary;
}

// ---------------------------------------------------------------------------
// Workers
// ---------------------------------------------------------------------------

export interface WorkerCapabilities {
  backends?: string[];
  scm_hosts?: string[];
  permission_modes?: string[];
}

export interface WorkerRecord {
  version: number;
  worker_id: string;
  kind: string;
  name: string;
  host: string;
  capabilities: WorkerCapabilities;
  registered_at: string;
  last_seen_at: string;
}

// ---------------------------------------------------------------------------
// Coverage
// ---------------------------------------------------------------------------

export interface CoverageSummary {
  /** Owning workspace id — target_ids can collide across workspaces. */
  ws_id: string;
  /** Workspace path basename — group/attribution key. */
  project: string;
  target_id: string;
  assertion_lines: number;
  has_catalog: boolean;
  findings: number;
}

/** Evidence attached to a `ConcernAssertion`. */
export interface AssertionEvidence {
  summary: string;
  line_ranges: number[][];
}

/**
 * One concern assertion from the per-target coverage JSONL.
 * `status` is snake_case on the wire; map via `normAssertionStatus`.
 */
export interface ConcernAssertion {
  concern_id: string;
  file_path: string;
  /** Raw wire value — use `normAssertionStatus` to map to the display union. */
  status: string;
  evidence: AssertionEvidence;
  declared_by: unknown;
  declared_at: string;
}

/** Normalised assertion status — tolerant of unknown future values. */
export type AssertionStatus = 'clean' | 'finding' | 'examined' | 'not_applicable' | 'unknown';

export function normAssertionStatus(raw: string): AssertionStatus {
  switch (raw.toLowerCase()) {
    case 'clean':          return 'clean';
    case 'finding':        return 'finding';
    case 'examined':       return 'examined';
    case 'not_applicable': return 'not_applicable';
    default:               return 'unknown';
  }
}

/** Severity levels for findings — matches the `sev.*` Tailwind palette. */
export type FindingSeverity = 'critical' | 'high' | 'medium' | 'low' | 'info';

const SEV_ORDER: FindingSeverity[] = ['critical', 'high', 'medium', 'low', 'info'];

export function normFindingSeverity(raw: string): FindingSeverity {
  const v = raw.toLowerCase() as FindingSeverity;
  return SEV_ORDER.includes(v) ? v : 'info';
}

/** Severity sort key: lower = more severe (for ascending sort). */
export function sevRank(s: FindingSeverity): number {
  return SEV_ORDER.indexOf(s);
}

/** Evidence attached to a `FindingRecord`. */
export interface FindingEvidence {
  rationale: string;
  code_excerpt?: string | null;
  references?: string[];
}

/** One finding record from the per-target findings JSONL. */
export interface FindingRecord {
  id: string;
  file_path?: string | null;
  line_range?: [number, number] | null;
  scope: unknown;
  summary: string;
  /** Raw wire value — use `normFindingSeverity` to get a `FindingSeverity`. */
  severity: string;
  concern_id?: string | null;
  evidence: FindingEvidence;
  declared_by: unknown;
  declared_at: string;
}

/** Touch strength, strongest last — matches rupu-coverage's `TouchStrength`. */
export type TouchStrength = 'glob' | 'cmd' | 'grep' | 'read' | 'edit';

/**
 * Aggregated per-file touch record (heatmap row) from `file_views`.
 * `strongest` is the highest touch seen on this path; `read_lines` is loose
 * (the wire is `[start,end]` pairs but the UI only counts them).
 */
export interface FileView {
  path: string;
  strongest: string;
  touch_modes?: string[];
  read_lines: number[][];
  grep_matches: number;
  edits: number;
  first_at?: string;
  last_at: string;
  touched_by?: unknown[];
}

export interface CoverageDetail {
  ws_id?: string;
  project?: string;
  target_id: string;
  assertion_lines: number;
  has_catalog: boolean;
  assertions: ConcernAssertion[];
  findings: FindingRecord[];
  /** Per-file heatmap; may be absent for targets without a file ledger. */
  files?: FileView[];
}

// ---------------------------------------------------------------------------
// Projects
// ---------------------------------------------------------------------------

export interface ProjectRow {
  ws_id: string;
  name: string;
  path: string;
  repo_remote?: string | null;
  branch?: string | null;
  created_at: string;
  last_run_at?: string | null;
}

export interface ProjectDetail {
  project: ProjectRow;
  runs: {
    total: number;
    running: number;
    by_status: Record<string, number>;
    by_surface: { workflow: number; autoflow: number };
  };
  sessions: { total: number; active: number };
  /** Cheap rollup — targets count + findings count only.
   * `assessed_pct` is served lazily by `getProjectAssessedPct`. */
  coverage: { targets: number; findings: number };
  recent_runs: RunListRow[];
  usage: UsageSummary;
}

/** Response from the lazy `GET /api/projects/:wsId/coverage/assessed` endpoint. */
export interface ProjectAssessedPct {
  assessed_pct: number | null;
}

export interface ProjectCoverageRow {
  target_id: string;
  assertion_lines: number;
  has_catalog: boolean;
  findings: number;
}

// ---------------------------------------------------------------------------
// API object
// ---------------------------------------------------------------------------

export const api = {
  // --- Dashboard ---
  getDashboard(): Promise<DashboardResponse> {
    return request<DashboardResponse>('/api/dashboard');
  },

  // --- Usage ---
  getUsage(params?: { since?: string; until?: string; groupBy?: 'provider' | 'model' | 'agent' }): Promise<UsageOverview> {
    const q = new URLSearchParams();
    if (params?.since) q.set('since', params.since);
    if (params?.until) q.set('until', params.until);
    if (params?.groupBy) q.set('group_by', params.groupBy);
    const qs = q.toString();
    return request<UsageOverview>(`/api/usage${qs ? `?${qs}` : ''}`);
  },

  // --- Runs ---
  getRuns(): Promise<RunListRow[]> {
    return request<RunListRow[]>('/api/runs');
  },
  getRun(id: string): Promise<{ run: RunRecord; steps: StepResultRecord[]; usage: UsageSummary }> {
    return request<{ run: RunRecord; steps: StepResultRecord[]; usage: UsageSummary }>(
      `/api/runs/${encodeURIComponent(id)}`,
    );
  },
  getRunGraph(id: string): Promise<RunGraphResponse> {
    return request<RunGraphResponse>(`/api/runs/${encodeURIComponent(id)}/graph`);
  },
  getWorkflowRuns(): Promise<RunListRow[]> {
    return request<RunListRow[]>('/api/runs/workflows');
  },
  getAutoflowRuns(): Promise<AutoflowCycleRow[]> {
    return request<AutoflowCycleRow[]>('/api/runs/autoflows');
  },
  getAutoflowEvents(): Promise<AutoflowEventRow[]> {
    return request<AutoflowEventRow[]>('/api/runs/autoflows/events');
  },
  getAgentRuns(): Promise<AgentRunRow[]> {
    return request<AgentRunRow[]>('/api/runs/agents');
  },
  getAutoflowDefs(): Promise<AutoflowDefRow[]> {
    return request<AutoflowDefRow[]>('/api/autoflows');
  },

  // --- Agents ---
  getAgents(): Promise<AgentSummary[]> {
    return request<AgentSummary[]>('/api/agents');
  },
  getAgent(name: string): Promise<AgentDetail> {
    return request<AgentDetail>(`/api/agents/${encodeURIComponent(name)}`);
  },

  // --- Workflows ---
  getWorkflows(): Promise<WorkflowSummary[]> {
    return request<WorkflowSummary[]>('/api/workflows');
  },
  getWorkflow(name: string): Promise<WorkflowDetail> {
    return request<WorkflowDetail>(`/api/workflows/${encodeURIComponent(name)}`);
  },

  // --- Sessions ---
  getSessions(): Promise<SessionSummary[]> {
    return request<SessionSummary[]>('/api/sessions');
  },
  getSession(id: string): Promise<SessionSummary> {
    return request<SessionSummary>(`/api/sessions/${encodeURIComponent(id)}`);
  },

  // --- Workers ---
  getWorkers(): Promise<WorkerRecord[]> {
    return request<WorkerRecord[]>('/api/workers');
  },

  // --- Coverage ---
  getCoverage(): Promise<CoverageSummary[]> {
    return request<CoverageSummary[]>('/api/coverage');
  },
  getCoverageDetail(target: string, wsId?: string): Promise<CoverageDetail> {
    const qs = wsId ? `?ws_id=${encodeURIComponent(wsId)}` : '';
    return request<CoverageDetail>(`/api/coverage/${encodeURIComponent(target)}${qs}`);
  },

  /**
   * Subscribe to the JSONL event stream for a single run.
   * The backend sends unnamed SSE data events → arrives on `es.onmessage`.
   *
   * @returns Unsubscribe function — call it to close the EventSource.
   */
  subscribeRunLog(
    id: string,
    onEvent: (e: RunEvent) => void,
    onError?: (e: Event) => void,
  ): () => void {
    const es = new EventSource(`/api/runs/${encodeURIComponent(id)}/log`);
    es.onmessage = (m) => onEvent(JSON.parse(m.data) as RunEvent);
    if (onError) es.onerror = onError;
    return () => es.close();
  },

  /**
   * Subscribe to the global events stream, optionally filtered by run id.
   * The backend sends unnamed SSE data events → arrives on `es.onmessage`.
   *
   * @returns Unsubscribe function — call it to close the EventSource.
   */
  subscribeEvents(
    onEvent: (e: RunEvent) => void,
    opts?: { run?: string },
    onError?: (e: Event) => void,
  ): () => void {
    const url = opts?.run
      ? `/api/events/stream?run=${encodeURIComponent(opts.run)}`
      : '/api/events/stream';
    const es = new EventSource(url);
    es.onmessage = (m) => onEvent(JSON.parse(m.data) as RunEvent);
    if (onError) es.onerror = onError;
    return () => es.close();
  },

  // --- Projects ---

  getProjects(): Promise<ProjectRow[]> {
    return request<ProjectRow[]>('/api/projects');
  },
  getProject(wsId: string): Promise<ProjectDetail> {
    return request<ProjectDetail>(`/api/projects/${encodeURIComponent(wsId)}`);
  },
  getProjectRuns(wsId: string): Promise<RunListRow[]> {
    return request<RunListRow[]>(`/api/projects/${encodeURIComponent(wsId)}/runs`);
  },
  getProjectSessions(wsId: string): Promise<SessionSummary[]> {
    return request<SessionSummary[]>(`/api/projects/${encodeURIComponent(wsId)}/sessions`);
  },
  getProjectCoverage(wsId: string): Promise<ProjectCoverageRow[]> {
    return request<ProjectCoverageRow[]>(`/api/projects/${encodeURIComponent(wsId)}/coverage`);
  },
  getProjectAgents(wsId: string): Promise<AgentSummary[]> {
    return request<AgentSummary[]>(`/api/projects/${encodeURIComponent(wsId)}/agents`);
  },
  getProjectWorkflows(wsId: string): Promise<WorkflowSummary[]> {
    return request<WorkflowSummary[]>(`/api/projects/${encodeURIComponent(wsId)}/workflows`);
  },
  getProjectAutoflows(wsId: string): Promise<AutoflowDefRow[]> {
    return request<AutoflowDefRow[]>(`/api/projects/${encodeURIComponent(wsId)}/autoflows`);
  },
  /** Lazy heavy endpoint — runs run_audit per target.  Fetch in parallel with
   * `getProject` so the overview renders immediately while this resolves. */
  getProjectAssessedPct(wsId: string): Promise<ProjectAssessedPct> {
    return request<ProjectAssessedPct>(
      `/api/projects/${encodeURIComponent(wsId)}/coverage/assessed`,
    );
  },

  // --- Transcripts ---

  getTranscript(path: string): Promise<TranscriptResponse> {
    return request<TranscriptResponse>(`/api/transcript?path=${encodeURIComponent(path)}`);
  },

  /**
   * Subscribe to a live transcript stream for an in-progress agent run.
   * The backend sends unnamed SSE data events → arrives on `es.onmessage`.
   *
   * @returns Unsubscribe function — call it to close the EventSource.
   */
  subscribeTranscript(
    path: string,
    onEvent: (e: TranscriptEvent) => void,
    onError?: (e: Event) => void,
  ): () => void {
    const es = new EventSource(`/api/transcript/stream?path=${encodeURIComponent(path)}`);
    es.onmessage = (m) => onEvent(JSON.parse(m.data) as TranscriptEvent);
    if (onError) es.onerror = onError;
    return () => es.close();
  },
};
