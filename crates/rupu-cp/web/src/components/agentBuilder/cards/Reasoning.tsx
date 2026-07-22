// Reasoning card — `effort` (a Scale with a synthetic "(off)" = unset
// leading option) plus the two turn/token caps. Maps natively per provider
// (Anthropic thinking budget / OpenAI reasoning.effort / Gemini
// thinkingBudget) — the hint says so since the mapping isn't visible here.
import { EFFORT_LEVELS, type AgentDraft } from '../../../lib/agentBuilder/agentSpec';
import { LabeledRow, Scale } from '../controls';
import type { CardProps } from './types';

const OFF = '(off)';
const SCALE_OPTIONS = [OFF, ...EFFORT_LEVELS];

export default function Reasoning({ draft, patch }: CardProps) {
  function setNumberField<K extends 'maxTurns' | 'maxTokens'>(key: K, raw: string) {
    patch({ [key]: raw ? Number(raw) : undefined } as Partial<AgentDraft>);
  }

  return (
    <>
      <LabeledRow
        label="Reasoning effort"
        yamlKey="effort"
        hint="Maps natively per provider (Anthropic thinking budget / OpenAI reasoning.effort / Gemini thinkingBudget)."
      >
        <Scale
          options={SCALE_OPTIONS}
          value={draft.effort ?? OFF}
          onChange={(v) => patch({ effort: v === OFF ? undefined : v })}
        />
      </LabeledRow>
      <div className="ab-row ab-two">
        <LabeledRow label="Max turns" yamlKey="maxTurns">
          <input
            className="ab-txt mono"
            type="number"
            placeholder="∞"
            value={draft.maxTurns ?? ''}
            onChange={(e) => setNumberField('maxTurns', e.target.value)}
            aria-label="max turns"
          />
        </LabeledRow>
        <LabeledRow label="Max tokens" yamlKey="maxTokens">
          <input
            className="ab-txt mono"
            type="number"
            placeholder="8192"
            value={draft.maxTokens ?? ''}
            onChange={(e) => setNumberField('maxTokens', e.target.value)}
            aria-label="max tokens"
          />
        </LabeledRow>
      </div>
    </>
  );
}
