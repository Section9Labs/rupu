// Context card — `contextWindow` (default/1m) plus the two compaction knobs.
// Setting `contextWindowTokens` is what actually enables proactive
// compaction; `compactAtPercent` is the threshold for it.
import { CONTEXT_WINDOWS, type AgentDraft } from '../../../lib/agentBuilder/agentSpec';
import { LabeledRow, Segmented } from '../controls';
import type { CardProps } from './types';

const OPTIONS = CONTEXT_WINDOWS.map((v) => ({ label: v === '1m' ? '1M (beta)' : v, value: v as string }));

export default function Context({ draft, patch }: CardProps) {
  function setNumberField<K extends 'contextWindowTokens' | 'compactAtPercent'>(key: K, raw: string) {
    patch({ [key]: raw ? Number(raw) : undefined } as Partial<AgentDraft>);
  }

  return (
    <>
      <LabeledRow label="Context window" yamlKey="contextWindow">
        <Segmented<string>
          options={OPTIONS}
          value={draft.contextWindow ?? 'default'}
          onChange={(v) => patch({ contextWindow: v === 'default' ? undefined : v })}
        />
      </LabeledRow>
      <div className="ab-row ab-two">
        <LabeledRow label="Window tokens" yamlKey="contextWindowTokens">
          <input
            className="ab-txt mono"
            type="number"
            placeholder="(off)"
            value={draft.contextWindowTokens ?? ''}
            onChange={(e) => setNumberField('contextWindowTokens', e.target.value)}
            aria-label="context window tokens"
          />
        </LabeledRow>
        <LabeledRow label="Compact at %" yamlKey="compactAtPercent">
          <input
            className="ab-txt mono"
            type="number"
            value={draft.compactAtPercent ?? ''}
            onChange={(e) => setNumberField('compactAtPercent', e.target.value)}
            aria-label="compact at percent"
          />
        </LabeledRow>
      </div>
      <div className="ab-hint">Setting window tokens enables proactive LLM compaction at the threshold.</div>
    </>
  );
}
