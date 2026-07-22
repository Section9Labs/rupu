// Permission card — `permissionMode`, a closed 3-way vocab. Each value gets
// its own one-line hint since the practical effect (interactive prompt vs.
// unattended vs. mutation-blocked) isn't obvious from the name alone.
import { PERMISSION_MODES } from '../../../lib/agentBuilder/agentSpec';
import { LabeledRow, Segmented } from '../controls';
import type { CardProps } from './types';

const OPTIONS = PERMISSION_MODES.map((v) => ({ label: v, value: v as string }));

const HINTS: Record<string, string> = {
  ask: 'Interactive [y/n/a/s] prompt per tool call.',
  bypass: 'Allow every tool call — no prompts.',
  readonly: 'Read-only: mutating tools blocked.',
};

export default function Permission({ draft, patch }: CardProps) {
  const value = draft.permissionMode ?? '';
  return (
    <LabeledRow label="Permission mode" yamlKey="permissionMode" hint={value ? HINTS[value] : undefined}>
      <Segmented<string>
        options={OPTIONS}
        value={value}
        onChange={(v) => patch({ permissionMode: v || undefined })}
      />
    </LabeledRow>
  );
}
