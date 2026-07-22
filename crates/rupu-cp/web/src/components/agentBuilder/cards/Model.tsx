// Model card — `provider` + `model` + `auth`. Provider/auth are closed
// vocabularies (`PROVIDERS`/`AUTH_MODES` from agentSpec.ts) rendered as
// Segmented controls; `auth` gets an extra "(default)" empty option since
// omitting it is a valid, common choice (provider-default credential path).
import { AUTH_MODES, PROVIDERS, type AgentDraft } from '../../../lib/agentBuilder/agentSpec';
import { LabeledRow, Segmented } from '../controls';
import type { CardProps } from './Identity';

const PROVIDER_OPTIONS = PROVIDERS.map((p) => ({ label: p, value: p as string }));
const AUTH_OPTIONS = [
  { label: '(default)', value: '' as string },
  ...AUTH_MODES.map((a) => ({ label: a, value: a as string })),
];

export default function Model({ draft, patch }: CardProps) {
  function setField<K extends keyof AgentDraft>(key: K, value: string) {
    patch({ [key]: value || undefined } as Partial<AgentDraft>);
  }

  return (
    <>
      <div className="ab-row ab-two">
        <LabeledRow label="Provider" yamlKey="provider">
          <Segmented<string>
            options={PROVIDER_OPTIONS}
            value={draft.provider ?? ''}
            onChange={(v) => setField('provider', v)}
          />
        </LabeledRow>
        <LabeledRow label="Model" yamlKey="model">
          <input
            className="ab-txt mono"
            placeholder="provider default"
            value={draft.model ?? ''}
            onChange={(e) => setField('model', e.target.value)}
            aria-label="model"
          />
        </LabeledRow>
      </div>
      <LabeledRow label="Auth" yamlKey="auth" hint="Credential path — api-key or SSO/OAuth.">
        <Segmented<string> options={AUTH_OPTIONS} value={draft.auth ?? ''} onChange={(v) => setField('auth', v)} />
      </LabeledRow>
    </>
  );
}
