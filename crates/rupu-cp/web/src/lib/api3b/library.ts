// Operator MCP Plan 3b, UI-4: the client calls behind the Library → Skills
// install dialog, a skill page's Update / Validate, the findings Import and
// Report schema dialogs, a coverage target's Rerun… and the query bars' "?"
// fields popover. Every call goes through `apiRequest` (auth, error
// mapping); the endpoints are documented in docs/cp-api.md.

import { apiRequest } from '../api';

const json = (body: unknown): RequestInit => ({ method: 'POST', body: JSON.stringify(body) });

// ---------------------------------------------------------------------------
// Skills — `POST /api/skills/{install,update,validate}`
// ---------------------------------------------------------------------------

/** What installing a source skill would do (`dry_run`). */
export type SkillPreviewStatus = 'new' | 'up_to_date' | 'changed' | 'locally_modified' | 'invalid';

export interface SkillPreviewRow {
  name: string;
  /** Its folder in the source (`""` for a source that is one skill). */
  rel: string;
  description: string | null;
  status: SkillPreviewStatus;
  /** Why, for `locally_modified` and `invalid`. */
  reason?: string;
}

export interface SkillInstallPreview {
  dry_run: true;
  layer: 'global' | 'project';
  found: SkillPreviewRow[];
}

export interface SkillInstallRequest {
  source: string;
  ref?: string;
  /** A `ws_id`: install into that project's `.rupu/skills`. */
  project?: string;
}

export interface InstalledSkill {
  name: string;
  layer: 'global' | 'project';
  path: string;
  outcome: 'created' | 'replaced';
  content_hash: string;
  tracked: boolean;
}

export interface SkillInstallReport {
  installed: InstalledSkill[];
  skipped: { name: string; reason: string }[];
}

function installBody(req: SkillInstallRequest): Record<string, unknown> {
  const body: Record<string, unknown> = { source: req.source.trim() };
  const ref = req.ref?.trim();
  if (ref) body.ref = ref;
  if (req.project) body.project = req.project;
  return body;
}

/** The skills in a source and what installing each would do. Writes nothing. */
export function previewSkillInstall(req: SkillInstallRequest): Promise<SkillInstallPreview> {
  return apiRequest<SkillInstallPreview>('/api/skills/install', json({ ...installBody(req), dry_run: true }));
}

/** Install the named skills from a source; `force` replaces installed copies. */
export function installSkills(req: SkillInstallRequest & { skills: string[]; force: boolean }): Promise<SkillInstallReport> {
  return apiRequest<SkillInstallReport>(
    '/api/skills/install',
    json({ ...installBody(req), skills: req.skills, force: req.force }),
  );
}

export type SkillUpdateOutcome =
  | 'updated'
  | 'up_to_date'
  | 'no_record'
  | 'locally_modified'
  | 'not_installed'
  | 'failed';

export interface SkillUpdateResult {
  name: string;
  outcome: SkillUpdateOutcome;
  from?: string;
  to?: string;
  error?: string;
}

/** Re-fetch installed skills from their recorded source. A per-skill
 *  failure is a result row, never an error status. */
export async function updateSkills(req: { names: string[]; project?: string; force?: boolean }): Promise<SkillUpdateResult[]> {
  const body: Record<string, unknown> = { names: req.names };
  if (req.project) body.project = req.project;
  if (req.force) body.force = true;
  const res = await apiRequest<{ layer: string; results: SkillUpdateResult[] }>('/api/skills/update', json(body));
  return res.results;
}

export interface SkillValidation {
  name: string;
  layer?: string;
  path: string;
  status: 'ok' | 'invalid';
  error?: string | null;
  warnings: string[];
}

/** Check one installed skill against the format. */
export function validateSkill(req: { name: string; layer: 'global' | 'project'; wsId?: string }): Promise<{
  ok: boolean;
  skills: SkillValidation[];
}> {
  const body: Record<string, unknown> = { name: req.name, layer: req.layer };
  if (req.wsId) body.ws_id = req.wsId;
  return apiRequest('/api/skills/validate', json(body));
}

// ---------------------------------------------------------------------------
// Findings — `POST /api/findings/import`, `GET /api/findings/schema`
// ---------------------------------------------------------------------------

export type ImportedFile =
  | { path: string; status: 'attached'; finding_id: string }
  | { path: string; status: 'skipped'; reason: string }
  | { path: string; status: 'failed'; reason: string; details?: string[] };

export interface FindingsImportReport {
  dry_run: boolean;
  files: ImportedFile[];
  backups: string[];
  attached: number;
  skipped: number;
  failed: number;
  warnings?: string[];
}

/** Attach Markdown reports to their findings; `dryRun` writes nothing. */
export function importFindings(paths: string[], dryRun: boolean): Promise<FindingsImportReport> {
  return apiRequest<FindingsImportReport>('/api/findings/import', json({ paths, dry_run: dryRun }));
}

/** The finding report JSON Schema this build embeds. */
export function getFindingSchema(): Promise<unknown> {
  return apiRequest<unknown>('/api/findings/schema');
}

// ---------------------------------------------------------------------------
// Coverage — `POST /api/coverage/:target/rerun`
// ---------------------------------------------------------------------------

export interface CoverageRerun {
  run_id: string;
  host_id: string;
  agent: string;
  target_id: string;
  ws_id: string;
  replays: string;
}

/** Replay a recorded agent run on its target: a new run, started detached. */
export function rerunCoverage(target: string, run: string, wsId?: string): Promise<CoverageRerun> {
  const qs = wsId ? `?ws_id=${encodeURIComponent(wsId)}` : '';
  return apiRequest<CoverageRerun>(`/api/coverage/${encodeURIComponent(target)}/rerun${qs}`, json({ run }));
}

// ---------------------------------------------------------------------------
// Query help — `GET /api/query/:list/fields`
// ---------------------------------------------------------------------------

/** The lists the server filters with a registry of its own. */
export type ServerQueryList = 'findings' | 'workflow_runs' | 'agent_runs' | 'sessions' | 'autoflow_runs' | 'events';

export interface ServerQueryField {
  key: string;
  aliases: string[];
  kind: string;
  values: string[];
}

/** One list's query fields, in suggestion order. Fetched once per list. */
const fieldsCache = new Map<ServerQueryList, Promise<ServerQueryField[]>>();

export function getQueryFields(list: ServerQueryList): Promise<ServerQueryField[]> {
  let hit = fieldsCache.get(list);
  if (!hit) {
    hit = apiRequest<{ list: string; fields: ServerQueryField[] }>(`/api/query/${list}/fields`).then((r) => r.fields);
    // A failure is not remembered: the next open asks again.
    hit.catch(() => fieldsCache.delete(list));
    fieldsCache.set(list, hit);
  }
  return hit;
}

/** For tests: forget the fetched registries. */
export function clearQueryFieldsCache(): void {
  fieldsCache.clear();
}
