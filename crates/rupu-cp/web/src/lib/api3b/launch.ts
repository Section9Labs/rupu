// Operator MCP Plan 3b, package UI-3: the client for the launch fields
// (model, provider, engagement profiles), the YAML launch, agent validation,
// the stock fleet and project init (docs/cp-api.md). Kept out of `api.ts` so
// the Plan 3b packages don't conflict; every call goes through `apiRequest`.

import { ApiError, apiErrorMessage, apiRequest, type LaunchMode, type LaunchResult } from '../api';

// ---------------------------------------------------------------------------
// Engagement profiles (`GET /api/engagement-profiles`)
// ---------------------------------------------------------------------------

export interface ProfileAssetKind {
  id: string;
  parent?: string | null;
  coordinates?: string[];
  label: string;
}

export interface ProfileBundle {
  agents: string[];
  tools: string[];
  workflows: string[];
}

/** One profile: its TOML fields plus the layer it came from. */
export interface EngagementProfile {
  id: string;
  name: string;
  layer: 'builtin' | 'global' | 'project';
  includes: string[];
  asset_kinds: ProfileAssetKind[];
  bundle: ProfileBundle;
}

export interface EngagementProfiles {
  profiles: EngagementProfile[];
  /** Profile files that don't load; while non-empty every engagement launch fails. */
  errors: string[];
}

export function getEngagementProfiles(wsId?: string): Promise<EngagementProfiles> {
  const qs = wsId ? `?ws_id=${encodeURIComponent(wsId)}` : '';
  return apiRequest<EngagementProfiles>(`/api/engagement-profiles${qs}`);
}

/** What a profile brings, in one line: its bundle (or what it includes) and
 *  its asset kinds — "recon, service-analyst · asset kinds host, port". */
export function profileSummary(p: EngagementProfile): string {
  const brings = [...p.bundle.agents, ...p.bundle.workflows];
  const parts: string[] = [];
  if (brings.length > 0) parts.push(brings.join(', '));
  else if (p.includes.length > 0) parts.push(`includes ${p.includes.join(', ')}`);
  if (p.asset_kinds.length > 0) parts.push(`asset kinds ${p.asset_kinds.map((k) => k.id).join(', ')}`);
  return parts.length > 0 ? parts.join(' · ') : 'no bundle or asset kinds';
}

// ---------------------------------------------------------------------------
// Launches
// ---------------------------------------------------------------------------

/** The fields a launcher sheet sends on `POST /api/agents/:name/run`. */
export interface AgentRunBody {
  prompt?: string;
  mode?: LaunchMode;
  target?: string;
  working_dir?: string;
  host?: string;
  skills?: string[];
  preload_skills?: string[];
  engagement_profiles?: string[];
  model?: string;
  provider?: string;
}

export function launchAgentRun(agent: string, body: AgentRunBody): Promise<LaunchResult> {
  return apiRequest<LaunchResult>(`/api/agents/${encodeURIComponent(agent)}/run`, {
    method: 'POST',
    body: JSON.stringify(body),
  });
}

/** The fields `LauncherSheet` sends on `POST /api/workflows/:name/run`. */
export interface WorkflowRunBody {
  inputs?: Record<string, string>;
  mode?: LaunchMode;
  target?: string;
  working_dir?: string;
  host?: string;
  engagement_profiles?: string[];
}

export function launchWorkflowRun(workflow: string, body: WorkflowRunBody): Promise<LaunchResult> {
  return apiRequest<LaunchResult>(`/api/workflows/${encodeURIComponent(workflow)}/run`, {
    method: 'POST',
    body: JSON.stringify(body),
  });
}

export interface YamlRunBody {
  yaml: string;
  inputs?: Record<string, string>;
  mode?: LaunchMode;
}

/** `POST /api/workflows/run`: launch a workflow given as YAML, without saving
 *  it under a name. Local only. */
export function runWorkflowYaml(body: YamlRunBody): Promise<LaunchResult & { file: string }> {
  return apiRequest<LaunchResult & { file: string }>('/api/workflows/run', {
    method: 'POST',
    body: JSON.stringify(body),
  });
}

/** Why a launch field can't be sent, or null when it can. The API refuses a
 *  model/provider on any remote host and on a session (no transport or body
 *  carries one); engagement profiles likewise, except an agent run on a
 *  remote host, which that host decides. */
export interface LaunchFieldGates {
  model: string | null;
  profiles: string | null;
}

export function launchFieldGates(opts: {
  kind: 'agent' | 'workflow';
  host: string;
  session?: boolean;
}): LaunchFieldGates {
  const remote = opts.host !== 'local';
  if (opts.kind === 'workflow') {
    return {
      model: null,
      profiles: remote ? 'Local host only: a workflow launched on a remote host can’t take engagement profiles.' : null,
    };
  }
  if (opts.session) {
    return {
      model: 'A session runs on the agent’s own model and provider.',
      profiles: 'A session doesn’t take engagement profiles.',
    };
  }
  return {
    model: remote ? 'Local host only: no host transport carries a model or provider override.' : null,
    profiles: null,
  };
}

// ---------------------------------------------------------------------------
// Agent validation (`POST` / `GET /api/agents/validate`)
// ---------------------------------------------------------------------------

export type AgentValidity = { ok: true; name: string } | { ok: false; error: string };

/** Check an agent file's text without saving it. A 400 is the answer "invalid,
 *  because …"; anything else (404 for an unknown project, a network error)
 *  throws, so a caller keeps its own first-pass result. */
export async function validateAgentRaw(raw: string, wsId?: string): Promise<AgentValidity> {
  try {
    const r = await apiRequest<{ ok: true; name: string }>('/api/agents/validate', {
      method: 'POST',
      body: JSON.stringify(wsId ? { raw, ws_id: wsId } : { raw }),
    });
    return { ok: true, name: r.name };
  } catch (e) {
    if (e instanceof ApiError && e.status === 400) return { ok: false, error: apiErrorMessage(e) };
    throw e;
  }
}

export interface AgentValidationRow {
  name: string;
  layer: string;
  scope_id?: string | null;
  path: string;
  ok: boolean;
  error?: string | null;
}

export interface AgentValidationReport {
  ok: boolean;
  rows: AgentValidationRow[];
}

export function validateAllAgents(wsId?: string): Promise<AgentValidationReport> {
  const qs = wsId ? `?ws_id=${encodeURIComponent(wsId)}` : '';
  return apiRequest<AgentValidationReport>(`/api/agents/validate${qs}`);
}

// ---------------------------------------------------------------------------
// Stock fleet and project init
// ---------------------------------------------------------------------------

export type FleetInstalled = 'missing' | 'current' | 'modified' | 'unreadable';

export interface FleetEntry {
  kind: 'agent' | 'workflow';
  name: string;
  path: string;
  global: FleetInstalled;
  project?: FleetInstalled;
}

export interface FleetListing {
  entries: FleetEntry[];
  global_root: string;
  project_root: string | null;
}

export function getFleet(wsId?: string): Promise<FleetListing> {
  const qs = wsId ? `?ws_id=${encodeURIComponent(wsId)}` : '';
  return apiRequest<FleetListing>(`/api/fleet${qs}`);
}

export type FileAction = 'created' | 'skipped' | 'overwrote' | 'updated';

export interface WriteReport {
  root: string;
  created: number;
  skipped: number;
  overwrote: number;
  files: { path: string; action: FileAction; detail?: string }[];
}

/** Install the fleet into the global root, or `project`'s (a workspace id) `.rupu/`. */
export function installFleet(body: { force?: boolean; project?: string }): Promise<WriteReport> {
  return apiRequest<WriteReport>('/api/fleet/install', {
    method: 'POST',
    body: JSON.stringify(body),
  });
}

export function initProject(body: { path: string; with_samples?: boolean; force?: boolean }): Promise<WriteReport> {
  return apiRequest<WriteReport>('/api/projects/init', {
    method: 'POST',
    body: JSON.stringify(body),
  });
}

/** How many fleet files an install would write at a root: the missing ones,
 *  plus the edited or unreadable ones when forced. */
export function fleetWriteCount(states: FleetInstalled[], force: boolean): number {
  return states.filter((s) => s === 'missing' || (force && (s === 'modified' || s === 'unreadable'))).length;
}

/** The calls as one object, so a component test can `vi.spyOn(launchApi, …)`
 *  as the existing tests do with `api`. Components call through it. */
export const launchApi = {
  getEngagementProfiles,
  launchAgentRun,
  launchWorkflowRun,
  runWorkflowYaml,
  validateAgentRaw,
  validateAllAgents,
  getFleet,
  installFleet,
  initProject,
};
