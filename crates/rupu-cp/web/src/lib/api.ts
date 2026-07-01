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
import type { UsageSummary, UsageOverview, UsageTimelineBucket } from './usage';
export type { UsageSummary, UsageBreakdownRow, UsageOverview, UsageTimelineBucket } from './usage';

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
  | 'rejected'
  | 'cancelled';

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
  resume_requested_at?: string | null;
  resume_mode?: string | null;
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
  /** Transcript file for this (running) step. Emitted once the step's
   *  sub-run transcript path is known, so the live UI can select and tail
   *  it before any persisted step_result exists. Absent on tool-call pings. */
  transcript_path?: string | null;
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
  turns: number;
  duration_ms?: number | null;
  usage: UsageSummary;
  /** Originating host id — `"local"` for runs on this CP; a remote host id
   *  for proxied runs. Absent on older server versions (treat as `"local"`). */
  host_id?: string;
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
  usage: UsageSummary;
  /** Originating host id — `"local"` for local cycles; a remote host id for
   *  proxied cycles. Absent on older server versions (treat as `"local"`). */
  host_id?: string;
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
  usage: UsageSummary;
  /** Originating host id — `"local"` for local events; a remote host id for
   *  proxied events. Absent on older server versions (treat as `"local"`). */
  host_id?: string;
}

/**
 * One active autoflow *claim* — an issue the autoflow worker has leased and is
 * (or was) driving through a workflow. `status` is lowercase snake_case
 * (`await_human` | `running` | `blocked` | `complete` | `released`).
 */
export interface AutoflowClaim {
  issue_ref: string;
  issue_display_ref?: string | null;
  repo_ref: string;
  issue_title?: string | null;
  issue_url?: string | null;
  workflow: string;
  status: string;
  last_run_id?: string | null;
  last_error?: string | null;
  last_summary?: string | null;
  pr_url?: string | null;
  claim_owner?: string | null;
  lease_expires_at?: string | null;
  updated_at: string;
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
  turns: number;
  duration_ms?: number | null;
  usage: UsageSummary;
  /** Originating host id — `"local"` for local runs; a remote host id for
   *  proxied runs. Absent on older server versions (treat as `"local"`). */
  host_id?: string;
}

export interface AutoflowDefRow {
  name: string;
  /** File stem — the identifier the workflow detail route (`/workflows/:name`)
   *  is keyed by. May differ from `name` (the parsed display name). */
  slug: string;
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
    usage: UsageSummary;
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
  usage: UsageSummary;
  run_count: number;
}

export interface AgentDetail extends AgentSummary {
  system_prompt: string;
  /** Full raw agent definition file (`.md` frontmatter + body). */
  raw: string;
}

// ---------------------------------------------------------------------------
// Workflows
// ---------------------------------------------------------------------------

export interface WorkflowSummary {
  name: string;
  scope: string;
  usage: UsageSummary;
  run_count: number;
  last_run?: string | null;
}

export interface WorkflowDetail {
  /** Parsed Workflow object — typed loosely; the UI inspects what it needs. */
  workflow: Record<string, unknown>;
  yaml: string;
  usage?: UsageSummary;
}

/** Permission mode a launched run starts in. */
export type LaunchMode = 'ask' | 'bypass' | 'readonly';

/** Response from workflow and agent run endpoints. */
export interface LaunchResult {
  run_id: string;
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

/**
 * One point on the aggregated per-turn usage timeline.
 * `turn` is a 1-based global index across all of a run's steps (or a session's
 * runs); `label` is the step_id (runs) / run_id (sessions) the turn belongs to.
 */
export interface UsageTimelinePoint {
  turn: number;
  label: string;
  tokens_in: number;
  tokens_out: number;
  tokens_cached: number;
}

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
  last_error?: string | null;
  /** Originating host id — `"local"` for local sessions; a remote host id for
   *  proxied sessions. Absent on older server versions (treat as `"local"`). */
  host_id?: string;
}

/**
 * One turn-run row in a session's conversation — returned by
 * `GET /api/sessions/:id/runs`. `status` is `null`/absent while the run is
 * in-flight; terminal values are `"ok"` | `"error"` | `"aborted"`.
 */
export interface SessionRunRow {
  run_id: string;
  prompt: string;
  transcript_path: string;
  status?: string | null;
  started_at?: string | null;
  completed_at?: string | null;
  tokens_in: number;
  tokens_out: number;
  tokens_cached: number;
  duration_ms: number;
  error?: string | null;
}

// ---------------------------------------------------------------------------
// Hosts
// ---------------------------------------------------------------------------

export type HostTransportKind = 'local' | 'http_cp' | 'tunnel' | 'ssh' | 'bucket';
export type HostStatus = 'online' | 'offline' | 'stale';

export interface HostCapabilities {
  backends: string[];
  scm_hosts: string[];
  permission_modes: string[];
}

/** JSON view of one registered host, enriched with live health data.
 *  Mirrors `HostView` from rupu-cp/src/api/hosts.rs. */
export interface HostView {
  id: string;
  name: string;
  transport_kind: HostTransportKind;
  base_url?: string;
  status: HostStatus;
  version?: string;
  capabilities?: HostCapabilities;
  active_run_count: number;
  last_seen_at?: string;
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

/**
 * A worker record enriched with run-activity attribution (returned by
 * `GET /api/workers`). The base `WorkerRecord` fields are flattened in; the
 * extra fields summarize the runs whose `worker_id` matches this worker.
 */
export interface WorkerView extends WorkerRecord {
  /** Runs currently Running / Pending / AwaitingApproval for this worker. */
  active_run_count: number;
  /** Every run ever attributed to this worker. */
  total_run_count: number;
  /** Most recent run `started_at`, or null when the worker has no runs. */
  last_run_at?: string | null;
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

/** Severity rollup for a set of findings — matches the `GET /api/findings`
 *  summary block. `total` is the count across every severity. */
export interface FindingsSummary {
  total: number;
  critical: number;
  high: number;
  medium: number;
  low: number;
  info: number;
}

/** One finding row from `GET /api/findings` — a `FindingRecord` flattened with
 *  its provenance keys (`ws_id` / `project` / `target_id`) at the top level. */
export interface FindingOut extends FindingRecord {
  ws_id: string;
  project: string;
  target_id: string;
  workflow_name?: string | null;
}

/** Response from `GET /api/findings` — the severity-sorted cross-project
 *  findings list plus the severity rollup. */
export interface FindingsResponse {
  findings: FindingOut[];
  summary: FindingsSummary;
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

// ── Coverage: concerns / catalog / audit / templates ──────────────────────

export interface CoverageConcern {
  id: string;
  name: string;
  description: string;
  severity: string; // lowercase: info|low|medium|high|critical
  applicable_globs: string[];
  min_strength: string;
  references: string[];
  tags: string[];
}

export interface TemplateSummary {
  name: string;
  version: number;
  description: string;
  concern_count: number;
  severity_breakdown: Record<string, number>;
}

export interface TemplateDetail {
  name: string;
  version: number;
  description: string;
  references: string[];
  concerns: CoverageConcern[];
  includes: string[];
}

export interface FlatCatalog {
  concerns: CoverageConcern[];
  sources: Record<string, string>;     // concern_id → template name / "inline"
  render_modes: Record<string, string>; // concern_id → full|index|auto
}

export interface ConcernCoverage {
  concern_id: string;
  name: string;
  severity: string;
  in_scope_files: string[];
  asserted_files: string[];
  gap_files: string[];
  clean: number;
  findings: number;
  examined: number;
  not_applicable: number;
}

export interface FileCoverage {
  path: string;
  strongest_touch: string;
  asserted_concerns: string[];
  missing_concerns: string[];
}

export interface CrossModelEntry {
  concern_id: string;
  file_path: string;
  model_statuses: [string, string][];
  disagreement: boolean;
}

export interface SerendipitousCluster {
  theme: string;
  finding_ids: string[];
  count: number;
}

export interface AuditReport {
  target_id: string;
  concerns: ConcernCoverage[];
  files: FileCoverage[];
  cross_model: CrossModelEntry[];
  serendipitous: SerendipitousCluster[];
  total_concerns: number;
  complete_concerns: number;
  total_gap_files: number;
}

// ── Coverage: runs + diff ─────────────────────────────────────────────────

export interface RunListEntry {
  run_id: string;
  started_at: string;
  model: string;
  surface: string; // lowercase: workflow|agent|autoflow|session
  cells_asserted: number;
  findings: number;
  files_touched: number;
}

export interface CellRef {
  concern_id: string;
  file_path: string;
  status: string; // snake_case: clean|finding|examined|not_applicable
}

export interface VerdictFlip {
  concern_id: string;
  file_path: string;
  base_status: string;
  compare_status: string;
  high_signal: boolean;
}

export interface FindingThemeRef {
  concern_id: string | null;
  theme: string;
}

export interface RunDiff {
  base_runs: string[];
  compare_runs: string[];
  newly_asserted: CellRef[];
  no_longer_asserted: CellRef[];
  verdict_flips: VerdictFlip[];
  findings_appeared: FindingThemeRef[];
  findings_disappeared: FindingThemeRef[];
  newly_touched: string[];
  no_longer_touched: string[];
}

// ---------------------------------------------------------------------------
// Config (CP Settings)
// ---------------------------------------------------------------------------

/** Provenance source for one resolved config key — mirrors `rupu_config::KeySource`. */
export type KeySource = 'global' | 'project' | 'env' | 'default';

/** Mirrors `rupu_config::KeyProvenance` — where a resolved key's value came
 *  from, and whether it is enforced by the global `[policy].lock` list. */
export interface KeyProvenance {
  source: KeySource;
  locked: boolean;
}

/** Runtime status block on `GET /api/config` — no secret VALUE is ever
 *  present, only `token_set: bool`. */
export interface ConfigRuntimeStatus {
  bind: string;
  token_set: boolean;
  restart_required_keys: string[];
}

/**
 * `GET /api/config` response. `effective` is the resolved `rupu_config::Config`
 * serialized loosely (the UI reads only the dotted keys it renders);
 * `provenance` is keyed by the same dotted paths used in a `patch` body.
 * `raw_global` / `raw_project` are the raw TOML text for each layer (used by
 * the Task 5 Raw tab); `raw_project` is `null` when no `?project=` was given
 * or that layer has no file yet.
 */
export interface ConfigView {
  effective: Record<string, unknown>;
  provenance: Record<string, KeyProvenance>;
  raw_global: string;
  raw_project: string | null;
  cp: Record<string, unknown>;
  status: ConfigRuntimeStatus;
}

/** Body for `PUT /api/config/global` and `PUT /api/config/project/:id` — either
 *  the full raw TOML text, or a flat `dotted.key -> value` patch. Exactly one
 *  should be set. */
export interface ConfigWriteBody {
  raw?: string;
  patch?: Record<string, unknown>;
}

export interface ConfigWriteResult {
  ok: boolean;
  restart_required?: string[];
}

// ---------------------------------------------------------------------------
// AI generation
// ---------------------------------------------------------------------------

export interface GeneratedDef {
  raw: string;
  provider: string;
  model: string;
  attempts: number;
}

export interface GenerateBody {
  description: string;
  provider?: string;
  model?: string;
}

export interface ProviderModels {
  provider: string;
  models: string[];
  is_default: boolean;
}

// ---------------------------------------------------------------------------
// Filesystem browse
// ---------------------------------------------------------------------------

export interface FsEntry { name: string; path: string; }
export interface BrowseResult { path: string; parent: string | null; dirs: FsEntry[]; }

// ---------------------------------------------------------------------------
// Repos
// ---------------------------------------------------------------------------

/** One repository entry from `GET /api/repos`. */
export interface RepoEntry {
  platform: string;
  repo: string; // "owner/name"
  default_branch: string;
  private: boolean;
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
  usage: UsageSummary;
  run_count: number;
  last_active?: string | null;
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
// List pagination
// ---------------------------------------------------------------------------

export interface ListParams {
  offset?: number;
  limit?: number;
}

function listQuery(params?: ListParams): string {
  const q = new URLSearchParams();
  if (params?.offset != null) q.set('offset', String(params.offset));
  if (params?.limit != null) q.set('limit', String(params.limit));
  const qs = q.toString();
  return qs ? `?${qs}` : '';
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
  /** Per-bucket usage timeline (chronological). `bucket` defaults to `day`. */
  getUsageTimeline(opts?: { since?: string; until?: string; bucket?: 'day' | 'week' }): Promise<UsageTimelineBucket[]> {
    const q = new URLSearchParams();
    if (opts?.since) q.set('since', opts.since);
    if (opts?.until) q.set('until', opts.until);
    if (opts?.bucket) q.set('bucket', opts.bucket);
    const qs = q.toString();
    return request<UsageTimelineBucket[]>(`/api/usage/timeline${qs ? `?${qs}` : ''}`);
  },

  // --- Runs ---
  getRuns(params?: ListParams & { host?: string }): Promise<RunListRow[]> {
    const q = new URLSearchParams();
    if (params?.offset != null) q.set('offset', String(params.offset));
    if (params?.limit != null) q.set('limit', String(params.limit));
    if (params?.host) q.set('host', params.host);
    const qs = q.toString();
    return request<RunListRow[]>(`/api/runs${qs ? `?${qs}` : ''}`);
  },
  getRun(id: string, opts?: { host?: string }): Promise<{ run: RunRecord; steps: StepResultRecord[]; usage: UsageSummary }> {
    const qs = opts?.host ? `?host=${encodeURIComponent(opts.host)}` : '';
    return request<{ run: RunRecord; steps: StepResultRecord[]; usage: UsageSummary }>(
      `/api/runs/${encodeURIComponent(id)}${qs}`,
    );
  },
  getRunGraph(id: string, opts?: { host?: string }): Promise<RunGraphResponse> {
    const qs = opts?.host ? `?host=${encodeURIComponent(opts.host)}` : '';
    return request<RunGraphResponse>(`/api/runs/${encodeURIComponent(id)}/graph${qs}`);
  },
  /** Record approval for an awaiting run. The run stays `awaiting_approval`
   *  but gains `resume_requested_at` (+ `resume_mode`); a worker then resumes
   *  it in the chosen permission mode (defaults to `ask`). */
  async approveRun(id: string, mode?: 'ask' | 'bypass' | 'readonly', host?: string): Promise<void> {
    const body: Record<string, unknown> = {};
    if (mode) body.mode = mode;
    const qs = host ? `?host=${encodeURIComponent(host)}` : '';
    await request<{ run: RunRecord; steps: StepResultRecord[]; usage: UsageSummary }>(
      `/api/runs/${encodeURIComponent(id)}/approve${qs}`,
      { method: 'POST', body: Object.keys(body).length ? JSON.stringify(body) : undefined },
    );
  },
  /** Reject an awaiting run (terminal → `rejected`). */
  async rejectRun(id: string, reason: string, host?: string): Promise<void> {
    const body: Record<string, unknown> = { reason };
    const qs = host ? `?host=${encodeURIComponent(host)}` : '';
    await request<{ run: RunRecord; steps: StepResultRecord[]; usage: UsageSummary }>(
      `/api/runs/${encodeURIComponent(id)}/reject${qs}`,
      { method: 'POST', body: JSON.stringify(body) },
    );
  },
  /** Cancel a non-terminal run (terminal → `cancelled`). */
  async cancelRun(id: string, reason?: string, host?: string): Promise<void> {
    const body: Record<string, unknown> = {};
    if (reason) body.reason = reason;
    const qs = host ? `?host=${encodeURIComponent(host)}` : '';
    await request<{ run: RunRecord; steps: StepResultRecord[]; usage: UsageSummary }>(
      `/api/runs/${encodeURIComponent(id)}/cancel${qs}`,
      { method: 'POST', body: Object.keys(body).length ? JSON.stringify(body) : undefined },
    );
  },
  /** Archive a terminal run (hides it from the default run list). */
  async archiveRun(id: string): Promise<void> {
    await request(`/api/runs/${encodeURIComponent(id)}/archive`, { method: 'POST' });
  },
  /** Restore a previously-archived run back to the active list. */
  async restoreRun(id: string): Promise<void> {
    await request(`/api/runs/${encodeURIComponent(id)}/restore`, { method: 'POST' });
  },
  /** Permanently delete a run and its on-disk transcripts. */
  async deleteRun(id: string): Promise<void> {
    await request(`/api/runs/${encodeURIComponent(id)}`, { method: 'DELETE' });
  },
  /** List archived runs. Pass `kind = 'workflow'` to restrict to workflow-kind only. */
  getArchivedRuns(kind?: string): Promise<RunListRow[]> {
    const qs = kind ? `?kind=${encodeURIComponent(kind)}` : '';
    return request<RunListRow[]>(`/api/runs/archived${qs}`);
  },
  getRunUsageTimeline(id: string, opts?: { host?: string }): Promise<UsageTimelinePoint[]> {
    const qs = opts?.host ? `?host=${encodeURIComponent(opts.host)}` : '';
    return request<UsageTimelinePoint[]>(`/api/runs/${encodeURIComponent(id)}/usage-timeline${qs}`);
  },
  getSessionUsageTimeline(id: string, opts?: { host?: string }): Promise<UsageTimelinePoint[]> {
    const qs = opts?.host ? `?host=${encodeURIComponent(opts.host)}` : '';
    return request<UsageTimelinePoint[]>(`/api/sessions/${encodeURIComponent(id)}/usage-timeline${qs}`);
  },
  /** Turn-runs of a session, oldest-first — one row per `send` (chat turn). */
  getSessionRuns(id: string, opts?: { host?: string }): Promise<SessionRunRow[]> {
    const qs = opts?.host ? `?host=${encodeURIComponent(opts.host)}` : '';
    return request<SessionRunRow[]>(`/api/sessions/${encodeURIComponent(id)}/runs${qs}`);
  },
  getWorkflowRuns(params?: ListParams & { lifecycle?: 'active' | 'completed' | 'failed'; host?: string }): Promise<RunListRow[]> {
    const q = new URLSearchParams();
    if (params?.offset != null) q.set('offset', String(params.offset));
    if (params?.limit != null) q.set('limit', String(params.limit));
    if (params?.lifecycle) q.set('lifecycle', params.lifecycle);
    if (params?.host) q.set('host', params.host);
    const qs = q.toString();
    return request<RunListRow[]>(`/api/runs/workflows${qs ? `?${qs}` : ''}`);
  },
  getAutoflowRuns(params?: ListParams & { host?: string }): Promise<AutoflowCycleRow[]> {
    const q = new URLSearchParams();
    if (params?.offset != null) q.set('offset', String(params.offset));
    if (params?.limit != null) q.set('limit', String(params.limit));
    if (params?.host) q.set('host', params.host);
    const qs = q.toString();
    return request<AutoflowCycleRow[]>(`/api/runs/autoflows${qs ? `?${qs}` : ''}`);
  },
  getAutoflowEvents(params?: ListParams & { host?: string }): Promise<AutoflowEventRow[]> {
    const q = new URLSearchParams();
    if (params?.offset != null) q.set('offset', String(params.offset));
    if (params?.limit != null) q.set('limit', String(params.limit));
    if (params?.host) q.set('host', params.host);
    const qs = q.toString();
    return request<AutoflowEventRow[]>(`/api/runs/autoflows/events${qs ? `?${qs}` : ''}`);
  },
  /** Active autoflow claims — leased issues the worker is (or was) driving. */
  getAutoflowClaims(): Promise<AutoflowClaim[]> {
    return request<AutoflowClaim[]>('/api/autoflows/claims');
  },
  /** Release a claim — frees the issue for re-claim on the next cycle. */
  releaseClaim(issueRef: string): Promise<{ released: boolean }> {
    return request<{ released: boolean }>('/api/autoflows/claims/release', {
      method: 'POST',
      body: JSON.stringify({ issue_ref: issueRef }),
    });
  },
  /** Requeue a claim — wakes the autoflow worker to re-drive the issue now. */
  requeueClaim(issueRef: string): Promise<{ wake_id: string }> {
    return request<{ wake_id: string }>('/api/autoflows/claims/requeue', {
      method: 'POST',
      body: JSON.stringify({ issue_ref: issueRef }),
    });
  },
  getAgentRuns(params?: ListParams & { lifecycle?: 'active' | 'completed' | 'failed'; host?: string }): Promise<AgentRunRow[]> {
    const q = new URLSearchParams();
    if (params?.offset != null) q.set('offset', String(params.offset));
    if (params?.limit != null) q.set('limit', String(params.limit));
    if (params?.lifecycle) q.set('lifecycle', params.lifecycle);
    if (params?.host) q.set('host', params.host);
    const qs = q.toString();
    return request<AgentRunRow[]>(`/api/runs/agents${qs ? `?${qs}` : ''}`);
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
  launchAgent(
    agent: string,
    opts: { prompt?: string; mode?: LaunchMode; target?: string; working_dir?: string; host?: string } = {},
  ): Promise<LaunchResult> {
    return request<LaunchResult>(`/api/agents/${encodeURIComponent(agent)}/run`, {
      method: 'POST',
      body: JSON.stringify({ prompt: opts.prompt, mode: opts.mode, target: opts.target, working_dir: opts.working_dir, host: opts.host }),
    });
  },
  startSession(
    agent: string,
    opts: { prompt?: string; mode?: LaunchMode; target?: string; working_dir?: string; host?: string } = {},
  ): Promise<{ session_id: string }> {
    return request<{ session_id: string }>(`/api/agents/${encodeURIComponent(agent)}/session`, {
      method: 'POST',
      body: JSON.stringify({ prompt: opts.prompt, mode: opts.mode, target: opts.target, working_dir: opts.working_dir, host: opts.host }),
    });
  },
  /**
   * Overwrite an agent's `.md` definition. The body is validated + reloaded
   * server-side; nothing is written on error. Throws `ApiError` with the parse
   * error message (400) — including when the frontmatter `name` mismatches the
   * route — and resolves to the reloaded `AgentDetail` on success.
   */
  saveAgent(name: string, raw: string): Promise<AgentDetail> {
    return request<AgentDetail>(`/api/agents/${encodeURIComponent(name)}`, {
      method: 'PUT',
      body: JSON.stringify({ raw }),
    });
  },
  /**
   * Create a new agent from a raw `.md` definition. 409 if the agent already
   * exists, 400 on parse error. Resolves to the reloaded `AgentDetail`.
   */
  createAgent(raw: string): Promise<AgentDetail> {
    return request<AgentDetail>('/api/agents', {
      method: 'POST',
      body: JSON.stringify({ raw }),
    });
  },
  /** Delete an agent definition. 404 if absent. */
  async deleteAgent(name: string): Promise<void> {
    await request<{ deleted: boolean }>(`/api/agents/${encodeURIComponent(name)}`, {
      method: 'DELETE',
    });
  },

  // --- Workflows ---
  getWorkflows(): Promise<WorkflowSummary[]> {
    return request<WorkflowSummary[]>('/api/workflows');
  },
  getWorkflow(name: string): Promise<WorkflowDetail> {
    return request<WorkflowDetail>(`/api/workflows/${encodeURIComponent(name)}`);
  },
  /**
   * Overwrite a workflow's `.yaml` definition. The body is validated + reloaded
   * server-side; nothing is written on error. Throws `ApiError` with the parse
   * error message (400) — including when the parsed `name` mismatches the route
   * — and resolves to the reloaded `WorkflowDetail` on success.
   */
  saveWorkflow(name: string, raw: string): Promise<WorkflowDetail> {
    return request<WorkflowDetail>(`/api/workflows/${encodeURIComponent(name)}`, {
      method: 'PUT',
      body: JSON.stringify({ raw }),
    });
  },
  /**
   * Create a new workflow from a raw `.yaml` definition. 409 if a workflow with
   * the parsed `name` already exists, 400 on parse error. Resolves to the
   * reloaded `WorkflowDetail`.
   */
  createWorkflow(raw: string): Promise<WorkflowDetail> {
    return request<WorkflowDetail>('/api/workflows', {
      method: 'POST',
      body: JSON.stringify({ raw }),
    });
  },
  /** Draft an agent definition from a description. 501 when `rupu cp serve` is not running. */
  generateAgent(body: GenerateBody): Promise<GeneratedDef> {
    return request<GeneratedDef>('/api/agents/generate', {
      method: 'POST',
      body: JSON.stringify(body),
    });
  },
  /** Draft a workflow definition from a description. 501 when `rupu cp serve` is not running. */
  generateWorkflow(body: GenerateBody): Promise<GeneratedDef> {
    return request<GeneratedDef>('/api/workflows/generate', {
      method: 'POST',
      body: JSON.stringify(body),
    });
  },
  /** Providers/models available for AI generation (empty when unavailable). */
  generateModels(): Promise<ProviderModels[]> {
    return request<ProviderModels[]>('/api/generate/models');
  },
  /** Delete a workflow definition. 404 if absent. */
  async deleteWorkflow(name: string): Promise<void> {
    await request<{ deleted: boolean }>(`/api/workflows/${encodeURIComponent(name)}`, {
      method: 'DELETE',
    });
  },
  /** Parse-check a workflow YAML server-side (writes nothing). Resolves
   *  {ok:true} on 200, or {ok:false, error} when the server returns 400. */
  async validateWorkflow(raw: string): Promise<{ ok: boolean; error?: string }> {
    try {
      await request<{ ok: boolean }>('/api/workflows/validate', {
        method: 'POST',
        body: JSON.stringify({ raw }),
      });
      return { ok: true };
    } catch (e) {
      if (e instanceof ApiError) return { ok: false, error: e.message };
      throw e;
    }
  },
  /**
   * Launch a fresh run of `workflow` via the configured launcher.
   * All options are optional — a bare call launches with no inputs in the
   * deployment's default mode. Resolves to the new run id; the run record
   * (and SSE) appears shortly after as the spawned child writes `run.json`.
   */
  launchRun(
    workflow: string,
    opts: { inputs?: Record<string, string>; mode?: LaunchMode; target?: string; working_dir?: string; host?: string } = {},
  ): Promise<LaunchResult> {
    return request<LaunchResult>(`/api/workflows/${encodeURIComponent(workflow)}/run`, {
      method: 'POST',
      body: JSON.stringify({ inputs: opts.inputs, mode: opts.mode, target: opts.target, working_dir: opts.working_dir, host: opts.host }),
    });
  },

  browseDir(path?: string): Promise<BrowseResult> {
    const qs = path ? `?path=${encodeURIComponent(path)}` : '';
    return request<BrowseResult>(`/api/fs/browse${qs}`);
  },

  // --- Sessions ---
  getSessions(params?: ListParams & { scope?: 'active' | 'archived'; host?: string }): Promise<SessionSummary[]> {
    const q = new URLSearchParams();
    if (params?.offset != null) q.set('offset', String(params.offset));
    if (params?.limit != null) q.set('limit', String(params.limit));
    if (params?.scope) q.set('scope', params.scope);
    if (params?.host) q.set('host', params.host);
    const qs = q.toString();
    return request<SessionSummary[]>(`/api/sessions${qs ? `?${qs}` : ''}`);
  },
  getSession(id: string, opts?: { host?: string }): Promise<SessionSummary> {
    const qs = opts?.host ? `?host=${encodeURIComponent(opts.host)}` : '';
    return request<SessionSummary>(`/api/sessions/${encodeURIComponent(id)}${qs}`);
  },
  /**
   * Send a message into a live session. Fire-and-forget — the session's worker
   * processes the turn asynchronously and a fresh run appears in the session's
   * turn-runs list shortly after. Resolves to the new run id.
   * Errors: 400 (empty prompt) · 404 (no such session) · 409 (session stopped)
   * · 501 (read-only deploy).
   */
  sendSessionMessage(id: string, prompt: string, host?: string): Promise<{ run_id: string }> {
    const qs = host ? `?host=${encodeURIComponent(host)}` : '';
    return request<{ run_id: string }>(`/api/sessions/${encodeURIComponent(id)}/send${qs}`, {
      method: 'POST',
      body: JSON.stringify({ prompt }),
    });
  },
  /** Archive an active session (hides it from the active list). */
  async archiveSession(id: string): Promise<void> {
    await request(`/api/sessions/${encodeURIComponent(id)}/archive`, { method: 'POST' });
  },
  /** Restore a previously-archived session back to the active list. */
  async restoreSession(id: string): Promise<void> {
    await request(`/api/sessions/${encodeURIComponent(id)}/restore`, { method: 'POST' });
  },
  /** Permanently delete a session and its on-disk data. */
  async deleteSession(id: string): Promise<void> {
    await request(`/api/sessions/${encodeURIComponent(id)}`, { method: 'DELETE' });
  },

  // --- Workers ---
  getWorkers(): Promise<WorkerView[]> {
    return request<WorkerView[]>('/api/workers');
  },

  // --- Hosts ---
  /** List all registered hosts, each enriched with live health data. */
  getHosts(): Promise<HostView[]> {
    return request<HostView[]>('/api/hosts');
  },
  /** Register a new remote host. Requires `rupu cp serve` (501 if absent). */
  addHost(body: { name: string; base_url: string; token?: string }): Promise<HostView> {
    return request<HostView>('/api/hosts', {
      method: 'POST',
      body: JSON.stringify(body),
    });
  },
  /** Remove a registered host. 400 if `id` is `"local"`. */
  async removeHost(id: string): Promise<void> {
    await request<void>(`/api/hosts/${encodeURIComponent(id)}`, {
      method: 'DELETE',
    });
  },
  /**
   * Enroll a new tunnel node.  Returns the new host view, the runnable command
   * (with the CP's WSS URL), and the **one-time** plaintext token.  The token
   * is only present in this response — the server never returns it again.
   * Requires `rupu cp serve` (501 if absent).
   */
  enrollNode(body: { name: string }): Promise<{ host: HostView; command: string; token: string }> {
    return request<{ host: HostView; command: string; token: string }>('/api/hosts/node', {
      method: 'POST',
      body: JSON.stringify(body),
    });
  },
  /** Register a new SSH host. `port` and `identity_file` are optional. */
  addSshHost(body: {
    name: string;
    host: string;
    port?: number;
    identity_file?: string;
  }): Promise<HostView> {
    return request<HostView>('/api/hosts/ssh', {
      method: 'POST',
      body: JSON.stringify(body),
    });
  },
  /** Register a new bucket (dead-drop) host. `prefix` is optional. */
  addBucketHost(body: {
    name: string;
    url: string;
    prefix?: string;
  }): Promise<HostView> {
    return request<HostView>('/api/hosts/bucket', {
      method: 'POST',
      body: JSON.stringify(body),
    });
  },

  // --- Coverage ---
  getCoverage(): Promise<CoverageSummary[]> {
    return request<CoverageSummary[]>('/api/coverage');
  },
  getCoverageDetail(target: string, wsId?: string): Promise<CoverageDetail> {
    const qs = wsId ? `?ws_id=${encodeURIComponent(wsId)}` : '';
    return request<CoverageDetail>(`/api/coverage/${encodeURIComponent(target)}${qs}`);
  },
  getCoverageTemplates(): Promise<TemplateSummary[]> {
    return request<TemplateSummary[]>('/api/coverage/templates');
  },
  getCoverageTemplate(name: string): Promise<TemplateDetail> {
    return request<TemplateDetail>(`/api/coverage/templates/${encodeURIComponent(name)}`);
  },
  getCoverageCatalog(target: string, wsId?: string): Promise<FlatCatalog> {
    const qs = wsId ? `?ws_id=${encodeURIComponent(wsId)}` : '';
    return request<FlatCatalog>(`/api/coverage/${encodeURIComponent(target)}/catalog${qs}`);
  },
  getCoverageAudit(target: string, wsId?: string): Promise<AuditReport> {
    const qs = wsId ? `?ws_id=${encodeURIComponent(wsId)}` : '';
    return request<AuditReport>(`/api/coverage/${encodeURIComponent(target)}/audit${qs}`);
  },
  getCoverageRuns(target: string, wsId?: string): Promise<RunListEntry[]> {
    const qs = wsId ? `?ws_id=${encodeURIComponent(wsId)}` : '';
    return request<RunListEntry[]>(`/api/coverage/${encodeURIComponent(target)}/runs${qs}`);
  },
  getCoverageDiff(
    target: string,
    opts?: { wsId?: string; base?: string; compare?: string },
  ): Promise<RunDiff> {
    const p = new URLSearchParams();
    if (opts?.wsId) p.set('ws_id', opts.wsId);
    if (opts?.base) p.set('base', opts.base);
    if (opts?.compare) p.set('compare', opts.compare);
    const qs = p.toString() ? `?${p.toString()}` : '';
    return request<RunDiff>(`/api/coverage/${encodeURIComponent(target)}/diff${qs}`);
  },

  // --- Findings ---
  getFindings(opts?: { wsId?: string; workflow?: string; runId?: string }): Promise<FindingsResponse> {
    const q = new URLSearchParams();
    if (opts?.wsId) q.set('ws_id', opts.wsId);
    if (opts?.workflow) q.set('workflow', opts.workflow);
    if (opts?.runId) q.set('run_id', opts.runId);
    const qs = q.toString();
    return request<FindingsResponse>(`/api/findings${qs ? `?${qs}` : ''}`);
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
    opts?: { host?: string },
  ): () => void {
    const qs = opts?.host ? `?host=${encodeURIComponent(opts.host)}` : '';
    const es = new EventSource(`/api/runs/${encodeURIComponent(id)}/log${qs}`);
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
    opts?: { run?: string; host?: string },
    onError?: (e: Event) => void,
  ): () => void {
    const q = new URLSearchParams();
    if (opts?.run) q.set('run', opts.run);
    if (opts?.host) q.set('host', opts.host);
    const qs = q.toString();
    const url = qs ? `/api/events/stream?${qs}` : '/api/events/stream';
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
  getProjectRuns(wsId: string, params?: ListParams): Promise<RunListRow[]> {
    return request<RunListRow[]>(`/api/projects/${encodeURIComponent(wsId)}/runs${listQuery(params)}`);
  },
  getProjectSessions(wsId: string, params?: ListParams): Promise<SessionSummary[]> {
    return request<SessionSummary[]>(`/api/projects/${encodeURIComponent(wsId)}/sessions${listQuery(params)}`);
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

  // --- Config (CP Settings) ---

  /** Effective resolved config + per-key provenance. Pass `project` (a
   *  workspace id) to also merge that project's `.rupu/config.toml` layer. */
  getConfig(project?: string): Promise<ConfigView> {
    const qs = project ? `?project=${encodeURIComponent(project)}` : '';
    return request<ConfigView>(`/api/config${qs}`);
  },
  /**
   * Persist a global config edit — either `{ raw }` (full TOML text) or
   * `{ patch }` (flat `dotted.key -> value` edits merged onto the existing
   * file). Reloads `AppState.config` server-side on success, so no restart is
   * needed to observe the change. Throws `ApiError` with the validation
   * message on 400 (unknown key / type mismatch), or 501 when this deploy has
   * no launcher (`rupu cp serve` not running).
   */
  putGlobalConfig(body: ConfigWriteBody): Promise<ConfigWriteResult> {
    return request<ConfigWriteResult>('/api/config/global', {
      method: 'PUT',
      body: JSON.stringify(body),
    });
  },
  /** Set the GLOBAL `[policy].lock` enforced-key list (replaces it wholesale —
   *  pass the full updated list, not a delta). 501 when this deploy has no
   *  launcher. */
  putPolicy(lock: string[]): Promise<{ ok: boolean }> {
    return request<{ ok: boolean }>('/api/config/policy', {
      method: 'PUT',
      body: JSON.stringify({ lock }),
    });
  },
  /**
   * Persist a PROJECT config edit (that workspace's `.rupu/config.toml`) —
   * either `{ raw }` (full TOML text) or `{ patch }` (flat `dotted.key ->
   * value` edits). Mirrors `putGlobalConfig`, but REJECTS (400) a write that
   * would set a key already enforced by the GLOBAL `[policy].lock` list —
   * locking is a global-only concept the project layer cannot override.
   */
  putProjectConfig(id: string, body: ConfigWriteBody): Promise<ConfigWriteResult> {
    return request<ConfigWriteResult>(`/api/config/project/${encodeURIComponent(id)}`, {
      method: 'PUT',
      body: JSON.stringify(body),
    });
  },

  // --- Repos ---

  /** List repositories visible to this rupu deployment.
   *  Returns `[]` when the backend has no SCM connectors configured (501). */
  async getRepos(): Promise<RepoEntry[]> {
    try {
      return await request<RepoEntry[]>('/api/repos');
    } catch (e) {
      if (e instanceof ApiError && e.status === 501) return [];
      throw e;
    }
  },

  // --- Transcripts ---

  getTranscript(path: string, opts?: { host?: string }): Promise<TranscriptResponse> {
    let url = `/api/transcript?path=${encodeURIComponent(path)}`;
    if (opts?.host) url += `&host=${encodeURIComponent(opts.host)}`;
    return request<TranscriptResponse>(url);
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
    opts?: { host?: string },
  ): () => void {
    let url = `/api/transcript/stream?path=${encodeURIComponent(path)}`;
    if (opts?.host) url += `&host=${encodeURIComponent(opts.host)}`;
    const es = new EventSource(url);
    es.onmessage = (m) => onEvent(JSON.parse(m.data) as TranscriptEvent);
    if (onError) es.onerror = onError;
    return () => es.close();
  },
};
