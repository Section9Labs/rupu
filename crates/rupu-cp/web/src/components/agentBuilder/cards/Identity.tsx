// Identity card — `name` + `description`. `name` is the one field required
// by every agent; it is edited here AND in the AgentBuilder header (both
// bind straight to `draft.name` via the same `patch`, so they can never
// drift out of sync with each other).
import { type AgentDraft } from '../../../lib/agentBuilder/agentSpec';
import { LabeledRow } from '../controls';

export interface CardProps {
  draft: AgentDraft;
  patch: (p: Partial<AgentDraft>) => void;
}

export default function Identity({ draft, patch }: CardProps) {
  return (
    <>
      <LabeledRow
        label="Name"
        yamlKey="name"
        hint="Agent identity — how workflows reference it. Only required field."
      >
        <input
          className="ab-txt mono"
          value={draft.name}
          onChange={(e) => patch({ name: e.target.value })}
          aria-label="name"
        />
      </LabeledRow>
      <LabeledRow label="Description" yamlKey="description">
        <input
          className="ab-txt"
          value={draft.description ?? ''}
          onChange={(e) => patch({ description: e.target.value || undefined })}
          aria-label="description"
        />
      </LabeledRow>
    </>
  );
}
