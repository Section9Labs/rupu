/**
 * rupu Control Plane — typed API client + SSE helpers.
 *
 * Pattern lifted from the Okesu sibling project:
 *   • `ApiError` class with status + body
 *   • `request<T>` typed-fetch wrapper (same-origin, JSON)
 *   • EventSource-based subscribe helpers (unnamed `data` events → `onmessage`)
 */

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
export type RunEvent =
  | RunStartedEvent
  | StepStartedEvent
  | StepWorkingEvent
  | StepAwaitingApprovalEvent
  | StepCompletedEvent
  | StepFailedEvent
  | StepSkippedEvent
  | UnitStartedEvent
  | UnitCompletedEvent
  | RunCompletedEvent
  | RunFailedEvent
  | UnknownRunEvent;

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
  target_id: string;
  assertion_lines: number;
  has_catalog: boolean;
  findings: number;
}

export interface CoverageDetail {
  target_id: string;
  assertion_lines: number;
  has_catalog: boolean;
  assertions: unknown[];
  findings: unknown[];
}

// ---------------------------------------------------------------------------
// API object
// ---------------------------------------------------------------------------

export const api = {
  // --- Dashboard ---
  getDashboard(): Promise<DashboardResponse> {
    return request<DashboardResponse>('/api/dashboard');
  },

  // --- Runs ---
  getRuns(): Promise<RunRecord[]> {
    return request<RunRecord[]>('/api/runs');
  },
  getRun(id: string): Promise<{ run: RunRecord; steps: StepResultRecord[] }> {
    return request<{ run: RunRecord; steps: StepResultRecord[] }>(
      `/api/runs/${encodeURIComponent(id)}`,
    );
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
  getCoverageDetail(target: string): Promise<CoverageDetail> {
    return request<CoverageDetail>(`/api/coverage/${encodeURIComponent(target)}`);
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
};
