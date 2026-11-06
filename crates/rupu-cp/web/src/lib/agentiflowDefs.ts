// Agentiflow definition helpers shared by Library → Agentiflows, the
// definition page and the launcher (operator MCP Plan 3b, UI-2).

import { useEffect, useState } from 'react';
import { agentiflowsApi, type DefScopeKind, type DefSelector, type DefValidity } from './api3b/agentiflows';

/** The definition page of one layer's file. */
export function defHref(def: { name: string; scope_kind: DefScopeKind; scope_id: string | null }): string {
  const p = new URLSearchParams({ scope_kind: def.scope_kind });
  if (def.scope_kind === 'project' && def.scope_id) p.set('scope_id', def.scope_id);
  return `/agentiflows/defs/${encodeURIComponent(def.name)}?${p.toString()}`;
}

/** The selector a definition page's URL pins (`?scope_kind=&scope_id=`), or
 *  none: the server then reads the project-first match. */
export function selectorFromSearch(search: URLSearchParams): DefSelector | undefined {
  const kind = search.get('scope_kind');
  if (kind === 'global') return { scope_kind: 'global' };
  if (kind === 'project') return { scope_kind: 'project', scope_id: search.get('scope_id') };
  return undefined;
}

/** "heron · network, web": the lead and the engagement profiles. */
export function leadAndProfiles(lead: string | null, profiles: string[]): string {
  return [lead ?? '—', profiles.join(', ')].filter(Boolean).join(' · ');
}

const DEBOUNCE_MS = 400;

/** The server's verdict on `raw`, re-asked 400 ms after the last edit
 *  (`POST /api/agentiflow-defs/validate`, writing nothing). `null` until it
 *  answers, for an empty draft, and when the check itself fails (a network
 *  error is not a verdict). */
export function useDefValidity(raw: string, scopeId?: string | null): DefValidity | null {
  const [validity, setValidity] = useState<DefValidity | null>(null);
  useEffect(() => {
    if (!raw.trim()) {
      setValidity(null);
      return;
    }
    const ctl = new AbortController();
    const t = setTimeout(() => {
      agentiflowsApi
        .validateDef(raw, scopeId ?? undefined, ctl.signal)
        .then((v) => {
          if (!ctl.signal.aborted) setValidity(v);
        })
        .catch(() => {
          if (!ctl.signal.aborted) setValidity(null);
        });
    }, DEBOUNCE_MS);
    return () => {
      clearTimeout(t);
      ctl.abort();
    };
  }, [raw, scopeId]);
  return validity;
}
