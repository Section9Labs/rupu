// Shared prop contract for Agent Builder field-group cards (Tasks 6-7). Each
// card is a small controlled component: it reads whichever `AgentDraft`
// fields it owns and calls `patch` with a partial update — the shell
// (`AgentBuilder.tsx`) owns the single source of truth (the draft) and
// re-serializes the live `.md` preview on every patch. Cards never read or
// write anything outside their owned fields (see `CARD_FIELDS` in
// `AgentBuilder.tsx`).
import type { AgentDraft } from '../../../lib/agentBuilder/agentSpec';

export interface CardProps {
  draft: AgentDraft;
  patch: (p: Partial<AgentDraft>) => void;
}
