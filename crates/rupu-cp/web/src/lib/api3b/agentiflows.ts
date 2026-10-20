// Agentiflow launch, stop and definition calls (operator MCP Plan 3b, UI-2).
// The endpoints are Plan 3a's (`docs/cp-api.md#agentiflows`): launch and
// stop a flow, and the `/api/agentiflow-defs` CRUD + validate. Flows are
// local-only, so nothing here takes a host.

import { apiErrorMessage, apiRequest, ApiError } from '../api';

export type DefScopeKind = 'global' | 'project';

/** One row of `GET /api/agentiflow-defs`. A file that does not parse is still
 *  listed, with `error` saying why. */
export interface AgentiflowDefRow {
  name: string;
  /** Display only: `global`, or the project's path basename. */
  scope: string;
  scope_kind: DefScopeKind;
  /** A project row's workspace id; pass it back to act on this row's file. */
  scope_id: string | null;
  description: string | null;
  lead: string | null;
  engagement_profiles: string[];
  error: string | null;
}

/** `GET /api/agentiflow-defs/:name` (and what create / save answer). `def` is
 *  the parsed definition, or null with `error` when the file does not parse. */
export interface AgentiflowDefDetail {
  name: string;
  def: Record<string, unknown> | null;
  yaml: string;
  error: string | null;
  scope: string;
  scope_kind: DefScopeKind;
  scope_id: string | null;
}

/** Which layer's file a read, save or delete acts on. */
export interface DefSelector {
  scope_kind: DefScopeKind;
  scope_id?: string | null;
}

export interface DeletedDef {
  deleted: boolean;
  scope: string;
  scope_kind: DefScopeKind;
}

/** The server's verdict on a draft: what `agentiflow run` would accept, or the
 *  first problem it names. */
export type DefValidity =
  | { ok: true; name: string; lead: string; engagement_profiles: string[] }
  | { ok: false; error: string };

/** Where a launched flow starts: the layer a definition lives in, or an
 *  absolute directory. The two are exclusive (the server 400s on both). */
export type LaunchScope =
  | { scope_kind: DefScopeKind; scope_id?: string | null }
  | { working_dir: string };

/** `POST /api/agentiflows`: `started` once the run recorded itself, `pending`
 *  when it was still starting after 30 s (with its log). */
export interface LaunchedFlow {
  id: string;
  status: 'started' | 'pending';
  log?: string;
}

/** `POST /api/agentiflows/:id/stop`: the graceful stop queues for the next
 *  round boundary; the hard stop answers once the run is closed out. */
export type StopAnswer =
  | { queued: true }
  | { stopped: true; status: string; stop_reason: string | null };

/** The selector a row or detail carries, for the call that acts on its file. */
export function defSelectorFor(row: { scope_kind: DefScopeKind; scope_id: string | null }): DefSelector {
  return row.scope_kind === 'project' ? { scope_kind: 'project', scope_id: row.scope_id } : { scope_kind: 'global' };
}

function selectorQuery(sel?: DefSelector): string {
  if (!sel) return '';
  const p = new URLSearchParams({ scope_kind: sel.scope_kind });
  if (sel.scope_id) p.set('scope_id', sel.scope_id);
  return `?${p.toString()}`;
}

function defPath(name: string, sel?: DefSelector): string {
  return `/api/agentiflow-defs/${encodeURIComponent(name)}${selectorQuery(sel)}`;
}

export const agentiflowsApi = {
  listDefs(opts?: { signal?: AbortSignal }): Promise<AgentiflowDefRow[]> {
    return apiRequest<AgentiflowDefRow[]>('/api/agentiflow-defs', { signal: opts?.signal });
  },

  getDef(name: string, sel?: DefSelector): Promise<AgentiflowDefDetail> {
    return apiRequest<AgentiflowDefDetail>(defPath(name, sel));
  },

  /** Create a definition in the global layer, named by its `name:`. */
  createDef(raw: string): Promise<AgentiflowDefDetail> {
    return apiRequest<AgentiflowDefDetail>('/api/agentiflow-defs', {
      method: 'POST',
      body: JSON.stringify({ raw }),
    });
  },

  saveDef(name: string, raw: string, sel?: DefSelector): Promise<AgentiflowDefDetail> {
    return apiRequest<AgentiflowDefDetail>(defPath(name, sel), {
      method: 'PUT',
      body: JSON.stringify({ raw }),
    });
  },

  deleteDef(name: string, sel?: DefSelector): Promise<DeletedDef> {
    return apiRequest<DeletedDef>(defPath(name, sel), { method: 'DELETE' });
  },

  /** Check a draft as `agentiflow run` does, writing nothing. A 400 (the first
   *  problem) or 404 (an unknown project) is an invalid draft; anything else
   *  throws, so a network failure never reads as a verdict. */
  async validateDef(raw: string, scopeId?: string | null, signal?: AbortSignal): Promise<DefValidity> {
    try {
      const r = await apiRequest<{ ok: true; name: string; lead: string; engagement_profiles: string[] }>(
        '/api/agentiflow-defs/validate',
        {
          method: 'POST',
          body: JSON.stringify(scopeId ? { raw, scope_id: scopeId } : { raw }),
          signal,
        },
      );
      return { ok: true, name: r.name, lead: r.lead, engagement_profiles: r.engagement_profiles ?? [] };
    } catch (e) {
      if (e instanceof ApiError && (e.status === 400 || e.status === 404)) {
        return { ok: false, error: apiErrorMessage(e) };
      }
      throw e;
    }
  },

  /** Start a flow detached, as `rupu agentiflow run --detach` does. */
  launch(definition: string, scope: LaunchScope): Promise<LaunchedFlow> {
    const body =
      'working_dir' in scope
        ? { definition, working_dir: scope.working_dir }
        : scope.scope_kind === 'project'
          ? { definition, scope_kind: 'project', scope_id: scope.scope_id }
          : { definition, scope_kind: 'global' };
    return apiRequest<LaunchedFlow>('/api/agentiflows', { method: 'POST', body: JSON.stringify(body) });
  },

  /** `now: false` queues the graceful stop (the lead finishes its round);
   *  `now: true` kills the coordinator and its units and records the run
   *  failed. */
  stop(id: string, now: boolean): Promise<StopAnswer> {
    return apiRequest<StopAnswer>(`/api/agentiflows/${encodeURIComponent(id)}/stop`, {
      method: 'POST',
      body: JSON.stringify({ now }),
    });
  },
};
