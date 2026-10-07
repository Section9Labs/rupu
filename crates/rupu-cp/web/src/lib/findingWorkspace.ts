// Which workspace a finding belongs to, for the routes keyed by finding id.
// Finding ids are unique within a workspace, not across them, so the CP
// answers 409 for an id several workspaces share unless the request names one
// (`?ws_id=`). The report page and the inline card provide the finding's own
// `ws_id`; the artifact and export links read it from here.

import { createContext, useContext } from 'react';

export const FindingWorkspace = createContext<string | undefined>(undefined);

export function useFindingWorkspace(): string | undefined {
  return useContext(FindingWorkspace);
}

/** `/findings/:id`, carrying the workspace when known. */
export function findingPath(id: string, wsId?: string | null): string {
  const base = `/findings/${encodeURIComponent(id)}`;
  return wsId ? `${base}?ws_id=${encodeURIComponent(wsId)}` : base;
}
