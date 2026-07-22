// Anthropic card — provider-specific tuning knobs. `anthropicSpeed` and
// `anthropicContextManagement` are closed vocabs in agentSpec.ts today
// (`['fast']` / `['tool_clearing']` — single-value, no "off"/"standard"
// alternative shipped yet), so — same pattern as Model.tsx's `auth` field —
// each Segmented gets a synthetic "(default)" leading option for "unset"
// rather than inventing an out-of-vocab value that `validateAgentDraft`
// would reject.
import { ANTHROPIC_CTX_MGMT, ANTHROPIC_SPEED } from '../../../lib/agentBuilder/agentSpec';
import { LabeledRow, Segmented } from '../controls';
import type { CardProps } from './types';

const SPEED_OPTIONS = [
  { label: '(default)', value: '' },
  ...ANTHROPIC_SPEED.map((v) => ({ label: v, value: v as string })),
];
const CTX_MGMT_OPTIONS = [
  { label: '(default)', value: '' },
  ...ANTHROPIC_CTX_MGMT.map((v) => ({ label: v, value: v as string })),
];
const OAUTH_OPTIONS = [
  { label: 'on', value: 'on' },
  { label: 'off', value: 'off' },
];

export default function Anthropic({ draft, patch }: CardProps) {
  return (
    <>
      <div className="ab-row ab-two">
        <LabeledRow label="Speed" yamlKey="anthropicSpeed">
          <Segmented<string>
            options={SPEED_OPTIONS}
            value={draft.anthropicSpeed ?? ''}
            onChange={(v) => patch({ anthropicSpeed: v || undefined })}
          />
        </LabeledRow>
        <LabeledRow label="Context mgmt" yamlKey="anthropicContextManagement">
          <Segmented<string>
            options={CTX_MGMT_OPTIONS}
            value={draft.anthropicContextManagement ?? ''}
            onChange={(v) => patch({ anthropicContextManagement: v || undefined })}
          />
        </LabeledRow>
      </div>
      <LabeledRow label="Task budget" yamlKey="anthropicTaskBudget" hint="Soft output-token cap; the model self-paces.">
        <input
          className="ab-txt mono"
          type="number"
          placeholder="(off)"
          value={draft.anthropicTaskBudget ?? ''}
          onChange={(e) => patch({ anthropicTaskBudget: e.target.value ? Number(e.target.value) : undefined })}
          aria-label="anthropic task budget"
        />
      </LabeledRow>
      <LabeledRow
        label="OAuth prefix"
        yamlKey="anthropicOauthPrefix"
        hint="On (default) prefixes the system prompt for OAuth/SSO auth; off disables it explicitly."
      >
        <Segmented<string>
          options={OAUTH_OPTIONS}
          value={draft.anthropicOauthPrefix === false ? 'off' : 'on'}
          onChange={(v) => patch({ anthropicOauthPrefix: v === 'off' ? false : undefined })}
        />
      </LabeledRow>
    </>
  );
}
